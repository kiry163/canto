//! 播放引擎：一个后台线程持有音频设备与解码器，界面/命令行只发消息、读状态。
//!
//! 为什么要有这一层：rodio 的播放是阻塞的（`Player` 要一直拿着），而界面必须保持
//! 响应。所以设备与解码器都归音频线程，外面通过 [`Engine::play`] 之类的消息指挥，
//! 通过 [`Engine::snapshot`] 拿到进度——状态是普通的 `Arc<Mutex<..>>` 快照，
//! 界面每帧读一次，不需要异步。
//!
//! 候选地址在这里逐个**真去开**：只有能解码的才算数（服务端对不存在的文件也会
//! 返回 purl/vkey，见 [`crate::qq::vkey`]）。

use std::fs::File;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use rodio::source::Source;
use rodio::stream::DeviceSinkBuilder;
use rodio::{Decoder, Player};

use crate::cache;
use crate::media::{MediaStream, Source as MediaSource};
use crate::model::{Quality, Song};
use crate::qq::vkey::Candidate;

/// 状态刷新间隔：够进度条平滑，也不会白烧 CPU。
const TICK: Duration = Duration::from_millis(150);

/// 一次播放请求：歌 + 已按偏好排好序的候选地址。
#[derive(Debug, Clone)]
pub struct Track {
    pub song: Song,
    pub candidates: Vec<Candidate>,
}

impl Track {
    /// 列表里显示用的「标题 — 歌手」。
    pub fn label(&self) -> String {
        format!("{} — {}", self.song.title, self.song.singers)
    }
}

/// 播放阶段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// 没在播；可能是刚开机，也可能是上一首播完了（看 `just_finished`）。
    Idle,
    /// 正在取第一个能解码的候选。
    Buffering,
    Playing,
    Paused,
    /// 所有候选都开不了，附带原因。
    Failed(String),
}

/// 状态快照，界面每帧读一次。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// 「标题 — 歌手」。
    pub title: Option<String>,
    /// 正在播那首的 mid（界面用来在列表里标 `♪`；mid 全局唯一，比标题可靠）。
    pub mid: Option<String>,
    /// 实际开出来的档位（`Buffering` 时还是 `None`）。
    pub quality: Option<Quality>,
    /// 实际开出来的那份文件的字节数（拿不到 `Content-Length` 就是 `None`）。
    pub size: Option<u64>,
    pub phase: Phase,
    pub position: Duration,
    /// 容器声明的总时长，拿不到就是 `None`（部分 MP3）。
    pub duration: Option<Duration>,
    /// 音量百分比。
    pub volume: u8,
    /// 刚刚播完（自动续播用）。界面读走后由引擎清零。
    pub just_finished: bool,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            title: None,
            mid: None,
            quality: None,
            size: None,
            phase: Phase::Idle,
            position: Duration::ZERO,
            duration: None,
            volume: 100,
            just_finished: false,
        }
    }
}

impl Snapshot {
    /// 上一首是否完整播完（差 5 秒以内算播完）。流中断时音频线程只会静默结束，
    /// 这里是唯一能发现的地方。
    pub fn completed(&self) -> bool {
        match self.duration {
            Some(duration) => self.position + Duration::from_secs(5) >= duration,
            None => true,
        }
    }

    /// 进度 0.0~1.0。
    pub fn progress(&self) -> f64 {
        match self.duration {
            Some(duration) if duration > Duration::ZERO => {
                (self.position.as_secs_f64() / duration.as_secs_f64()).clamp(0.0, 1.0)
            }
            _ => 0.0,
        }
    }
}

enum Cmd {
    Play(Box<Track>),
    TogglePause,
    SeekBy(i64),
    AdjustVolume(i8),
}

/// 播放引擎句柄。可以随便克隆？不需要——一个进程一个，放在 `App` 里。
pub struct Engine {
    tx: Sender<Cmd>,
    state: Arc<Mutex<Snapshot>>,
    running: Arc<AtomicBool>,
    /// 刚缓存完的歌（mid、档位、字节数）。界面每帧取走，用来刷新列表里的「已缓存」标记。
    promoted: cache::Promoted,
}

impl Engine {
    /// 打开默认音频设备并启动音频线程。设备打不开会立刻报错（而不是等第一次播放）。
    ///
    /// `cache_on`：边播边落盘（`config.toml` 的 `cache_mb` 为 0 时关掉）。
    pub fn start(cache_on: bool) -> Result<Self> {
        let mut sink = DeviceSinkBuilder::open_default_sink()
            .context("打开默认音频设备失败（是不是有别的程序独占？）")?;
        sink.log_on_drop(false);
        let mixer = sink.mixer().clone();
        Self::spawn(sink, mixer, cache_on)
    }

    /// 哑引擎：不碰音频设备，直接接一个没人听的 mixer。
    ///
    /// 给没有声卡的机器用（CI 上没有音频设备）。测试要的是界面逻辑而不是声音，
    /// 真设备那条路只由 `start` 走。
    #[cfg(test)]
    pub(crate) fn silent(cache_on: bool) -> Self {
        let (mixer, source) = rodio::mixer::mixer(
            rodio::ChannelCount::new(2).expect("2 声道"),
            rodio::SampleRate::new(44_100).expect("44.1kHz"),
        );
        Self::spawn(source, mixer, cache_on).expect("启动音频线程")
    }

    /// 起音频线程。`keep_alive` 只是被线程攥着不放：真设备是 `MixerDeviceSink`
    /// （丢了声音就停），测试里是 mixer 的输出端。
    fn spawn(
        keep_alive: impl Send + 'static,
        mixer: rodio::mixer::Mixer,
        cache_on: bool,
    ) -> Result<Self> {
        let state = Arc::new(Mutex::new(Snapshot::default()));
        let running = Arc::new(AtomicBool::new(true));
        let promoted = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = channel();
        let thread_state = state.clone();
        let thread_running = running.clone();
        let thread_promoted = promoted.clone();
        std::thread::Builder::new()
            .name("canto-audio".to_string())
            .spawn(move || {
                run(
                    rx,
                    thread_state,
                    keep_alive,
                    mixer,
                    thread_running,
                    cache_on,
                    thread_promoted,
                )
            })
            .context("启动音频线程失败")?;
        Ok(Self {
            tx,
            state,
            running,
            promoted,
        })
    }

    pub fn snapshot(&self) -> Snapshot {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// 取走「刚缓存完」的歌（界面每帧调一次）。
    pub fn take_cached(&self) -> Vec<(String, Quality, u64)> {
        let mut promoted = self.promoted.lock().unwrap_or_else(|error| error.into_inner());
        std::mem::take(&mut *promoted)
    }

    /// 音频线程还活着吗。
    pub fn is_alive(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn play(&self, track: Track) {
        self.send(Cmd::Play(Box::new(track)));
    }

    pub fn toggle_pause(&self) {
        self.send(Cmd::TogglePause);
    }

    /// 相对当前位置跳转（秒，可为负）。
    pub fn seek_by(&self, seconds: i64) {
        self.send(Cmd::SeekBy(seconds));
    }

    /// 音量加减（百分点）。
    pub fn adjust_volume(&self, delta: i8) {
        self.send(Cmd::AdjustVolume(delta));
    }

    /// 界面读走「刚播完」标记。
    pub fn take_finished(&self) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        std::mem::take(&mut state.just_finished)
    }

    fn send(&self, cmd: Cmd) {
        if self.tx.send(cmd).is_err() {
            tracing::warn!("音频线程已退出，命令被丢弃");
        }
    }
}

/// 音频线程主循环：处理命令 + 定时刷新状态快照。
fn run(
    rx: Receiver<Cmd>,
    state: Arc<Mutex<Snapshot>>,
    _keep_alive: impl Send + 'static,
    mixer: rodio::mixer::Mixer,
    running: Arc<AtomicBool>,
    cache_on: bool,
    promoted: cache::Promoted,
) {
    let mut player: Option<Player> = None;
    let mut volume = 1.0_f32;
    // 连续快进/快退的锚点，见 `Cmd::SeekBy` 的说明。
    let mut seek_anchor: Option<Duration> = None;

    loop {
        match rx.recv_timeout(TICK) {
            Ok(cmd) => {
                handle(
                    cmd,
                    &mixer,
                    &state,
                    &mut player,
                    &mut volume,
                    &mut seek_anchor,
                    cache_on,
                    &promoted,
                );
                // 连按按键时一次收干净，别让命令排队等下一个 tick。
                while let Ok(cmd) = rx.try_recv() {
                    handle(
                        cmd,
                        &mixer,
                        &state,
                        &mut player,
                        &mut volume,
                        &mut seek_anchor,
                        cache_on,
                        &promoted,
                    );
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if let Some(player) = &player {
            refresh(player, &state, &mut seek_anchor);
        }
    }
    running.store(false, Ordering::SeqCst);
    tracing::debug!("音频线程退出");
}

#[allow(clippy::too_many_arguments)]
fn handle(
    cmd: Cmd,
    mixer: &rodio::mixer::Mixer,
    state: &Arc<Mutex<Snapshot>>,
    player: &mut Option<Player>,
    volume: &mut f32,
    seek_anchor: &mut Option<Duration>,
    cache_on: bool,
    promoted: &cache::Promoted,
) {
    match cmd {
        Cmd::Play(track) => {
            // 丢掉旧的播放器就是「停掉上一首」。
            *player = None;
            *seek_anchor = None;
            let title = track.label();
            tracing::info!("开始播放：{title}");
            update(state, |snapshot| {
                snapshot.title = Some(title);
                snapshot.mid = Some(track.song.mid.clone());
                snapshot.quality = None;
                snapshot.size = None;
                snapshot.phase = Phase::Buffering;
                snapshot.position = Duration::ZERO;
                snapshot.duration = None;
                snapshot.just_finished = false;
            });
            match open(&track, cache_on, promoted) {
                Ok(opened) => {
                    let new_player = Player::connect_new(mixer);
                    new_player.set_volume(*volume);
                    new_player.append(opened.decoder);
                    tracing::info!("播放中：{}", opened.quality.label());
                    update(state, |snapshot| {
                        snapshot.quality = Some(opened.quality);
                        snapshot.size = opened.size;
                        snapshot.phase = Phase::Playing;
                        snapshot.duration = opened.duration;
                    });
                    *player = Some(new_player);
                }
                Err(error) => {
                    let message = format!("{error:#}");
                    tracing::warn!("播放失败：{message}");
                    update(state, |snapshot| {
                        snapshot.phase = Phase::Failed(message);
                    });
                }
            }
        }
        Cmd::TogglePause => {
            if let Some(player) = player {
                if player.is_paused() {
                    player.play();
                } else {
                    player.pause();
                }
                refresh(player, state, seek_anchor);
            }
        }
        Cmd::SeekBy(seconds) => {
            if let Some(player) = player {
                // 不能只看 `get_pos()`：它每 5ms 音频才更新一次，连按快进的一批
                // 命令会读到同一个位置，最后只前进一格。用上次的目标当锚点。
                let reported = player.get_pos();
                let from = seek_anchor.unwrap_or(reported).max(reported);
                let delta = Duration::from_secs(seconds.unsigned_abs());
                let target = if seconds >= 0 {
                    from + delta
                } else {
                    from.saturating_sub(delta)
                };
                let target = match duration_of(state) {
                    Some(duration) => target.min(duration),
                    None => target,
                };
                *seek_anchor = Some(target);
                if let Err(error) = player.try_seek(target) {
                    tracing::debug!("跳转失败（{target:?}）：{error}");
                }
            }
        }
        Cmd::AdjustVolume(delta) => {
            let percent = (*volume * 100.0).round() as i32 + i32::from(delta);
            let percent = percent.clamp(0, 100);
            *volume = percent as f32 / 100.0;
            if let Some(player) = player {
                player.set_volume(*volume);
            }
            update(state, |snapshot| snapshot.volume = percent as u8);
        }
    }
}

/// 当前这首声明的总时长。
fn duration_of(state: &Arc<Mutex<Snapshot>>) -> Option<Duration> {
    state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .duration
}

/// 从当前播放器读一次进度。
fn refresh(player: &Player, state: &Arc<Mutex<Snapshot>>, seek_anchor: &mut Option<Duration>) {
    let position = player.get_pos();
    let paused = player.is_paused();
    let ended = player.empty();
    // 播放位置追上了跳转目标，锚点就没用了（不清掉会一直压着后续快退）。
    if seek_anchor.is_some_and(|anchor| position >= anchor) {
        *seek_anchor = None;
    }
    update(state, |snapshot| {
        if ended {
            if snapshot.phase != Phase::Idle {
                snapshot.phase = Phase::Idle;
                snapshot.just_finished = true;
                if let Some(duration) = snapshot.duration {
                    snapshot.position = duration.max(position);
                } else {
                    snapshot.position = position;
                }
            }
        } else {
            snapshot.position = position;
            snapshot.phase = if paused {
                Phase::Paused
            } else {
                Phase::Playing
            };
        }
    });
}

fn update(state: &Arc<Mutex<Snapshot>>, act: impl FnOnce(&mut Snapshot)) {
    let mut snapshot = state.lock().unwrap_or_else(|error| error.into_inner());
    act(&mut snapshot);
}

/// 开出来的那份流：解码器 + 它是哪个档位、多大、多长。
struct Opened {
    decoder: Decoder<MediaSource>,
    quality: Quality,
    size: Option<u64>,
    duration: Option<Duration>,
}

/// 逐个候选去开，返回第一个能解码的。
///
/// 先看本地缓存：命中就完全不碰网络（拖动也瞬时）。没命中才去 CDN，并顺手把
/// 读到的字节写进缓存（只有从头顺序听完才会被认作完整，见 [`crate::cache`]）。
fn open(
    track: &Track,
    cache_on: bool,
    promoted: &cache::Promoted,
) -> Result<Opened> {
    let mid = track.song.mid.as_str();
    // 缓存里已经有这首（任意档位）：直接读本地文件，一个请求都不用发。
    if cache_on
        && let Some((path, quality)) = cache::for_song(mid)
    {
        match File::open(&path) {
            Ok(file) => {
                let size = file.metadata().ok().map(|metadata| metadata.len());
                match Decoder::new(MediaSource::File(file)) {
                    Ok(decoder) => {
                        let duration = decoder.total_duration();
                        tracing::info!("命中缓存：{}", path.display());
                        return Ok(Opened {
                            decoder,
                            quality,
                            size,
                            duration,
                        });
                    }
                    Err(error) => tracing::warn!("缓存文件解不了码（{error}），改走网络"),
                }
            }
            Err(error) => tracing::warn!("缓存文件打不开（{error}），改走网络"),
        }
    }
    // 逐个候选真去开，第一个能解码的胜出（服务端对不存在的文件也会给 purl）。
    let mut failures = Vec::new();
    for candidate in &track.candidates {
        let mut stream = match MediaStream::open(&candidate.url) {
            Ok(stream) => stream,
            Err(error) => {
                tracing::debug!("{} 档打不开：{error}", candidate.quality.label());
                failures.push(format!("{} 档打不开：{error}", candidate.quality.label()));
                continue;
            }
        };
        if cache_on {
            stream = stream.with_cache(
                mid,
                candidate.quality.token(),
                candidate.ext,
                promoted.clone(),
            );
        }
        let size = stream.total();
        match Decoder::new(MediaSource::Stream(Box::new(stream))) {
            Ok(decoder) => {
                let duration = decoder.total_duration();
                return Ok(Opened {
                    decoder,
                    quality: candidate.quality,
                    size,
                    duration,
                });
            }
            Err(error) => {
                tracing::debug!("{} 档解不了码：{error}", candidate.quality.label());
                failures.push(format!("{} 档解不了码：{error}", candidate.quality.label()));
            }
        }
    }
    bail!("没有能播的档位：{}", failures.join("；"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(position_secs: u64, duration: Option<u64>) -> Snapshot {
        Snapshot {
            phase: Phase::Idle,
            position: Duration::from_secs(position_secs),
            duration: duration.map(Duration::from_secs),
            ..Snapshot::default()
        }
    }

    #[test]
    fn short_streams_are_reported_as_interrupted() {
        // 播到底：位置与声明时长一致。
        assert!(snapshot(236, Some(236)).completed());
        // 播放器收尾时会差个零点几秒，5 秒容差要能容忍。
        assert!(snapshot(235, Some(236)).completed());
        // 中途断了：音频线程只会静默结束，靠这个判定才能发现。
        assert!(!snapshot(30, Some(236)).completed());
        // 声明时长拿不到（部分 MP3）就没法判定，不误报。
        assert!(snapshot(30, None).completed());
    }

    #[test]
    fn progress_is_a_ratio_and_survives_missing_duration() {
        assert_eq!(snapshot(118, Some(236)).progress(), 0.5);
        assert_eq!(snapshot(118, None).progress(), 0.0);
        // 位置超过声明时长（有些文件头不准）也要夹在 1.0 以内。
        assert_eq!(snapshot(300, Some(236)).progress(), 1.0);
    }
}

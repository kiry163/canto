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
use crate::levels::{LevelMeter, Levels};
use crate::media::{MediaStream, Source as MediaSource};
use crate::model::{Quality, Song};
use crate::qq::vkey::Candidate;

/// 状态刷新间隔：够进度条平滑，也不会白烧 CPU。
const TICK: Duration = Duration::from_millis(150);

/// 一次播放请求：歌 + 字节从哪来。
#[derive(Debug, Clone)]
pub struct Track {
    pub song: Song,
    pub source: TrackSource,
}

/// 字节的来源。两支互斥，所以用枚举而不是「候选 + 可选路径」（后者能造出两种都没有的非法状态）。
///
/// 「本地已有这份吗」由 [`local_copy`] **一处**判定，命中就不再取流——一个请求都不发。
#[derive(Debug, Clone)]
pub enum TrackSource {
    /// 线上：一串候选地址（已按偏好排好序）。
    Cdn(Vec<Candidate>),
    /// 本地文件（曲库 / 播放缓存 / 下载目录）。
    ///
    /// `quality` 只在知道时才有值：缓存文件名里带着档位；曲库与下载目录那份读不出来
    /// （`.mp3` 是 128 还是 320，文件本身不说），播放条就只报体积、不瞎报档位。
    Local {
        path: std::path::PathBuf,
        quality: Option<Quality>,
    },
}

/// 本地已有这份吗——顺序就是优先级：曲库路径（`Song` 自带）→ 播放缓存 → 下载目录。
///
/// `cache_dir` 给 `None` 就是不查缓存（`config.toml` 里 `cache_mb = 0`）。目录传进来而不是
/// 内部写死 `paths::cache_dir()`：这条判定要能拿一个临时目录测。
pub fn local_copy(
    song: &Song,
    cache_dir: Option<&std::path::Path>,
    downloads: &std::collections::HashMap<String, crate::download::Downloaded>,
) -> Option<TrackSource> {
    if let Some(path) = song.path() {
        return Some(TrackSource::Local {
            path: path.to_path_buf(),
            quality: None,
        });
    }
    if let Some(dir) = cache_dir
        && let Some((path, quality)) = cache::for_song_in(dir, &song.mid)
    {
        return Some(TrackSource::Local {
            path,
            quality: Some(quality),
        });
    }
    let downloaded = downloads.get(&crate::download::file_stem(song))?;
    Some(TrackSource::Local {
        path: downloaded.path.clone(),
        quality: None,
    })
}

impl TrackSource {
    /// 本地那份的路径（线上源给 `None`）。
    pub fn path(&self) -> Option<&std::path::Path> {
        match self {
            TrackSource::Cdn(_) => None,
            TrackSource::Local { path, .. } => Some(path),
        }
    }
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
    /// 频谱分析用的采样通道：音频线程写、界面线程读（见 [`crate::levels`]）。
    levels: Levels,
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
        // 先建好再分身：句柄与音频线程必须是**同一个** Levels，
        // 否则界面读到的永远是零，柱子不会动。
        let levels = Levels::new();
        let thread_levels = levels.clone();
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
                    thread_levels,
                )
            })
            .context("启动音频线程失败")?;
        Ok(Self {
            tx,
            state,
            running,
            promoted,
            levels,
        })
    }

    pub fn snapshot(&self) -> Snapshot {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// 测试用：把相位直接推到某个状态。哑引擎（`silent`）不碰音频设备、也不会自己走到
    /// `Playing`，界面里那几条「正在播」的分支就靠它造出来。
    #[cfg(test)]
    pub(crate) fn force_phase(&self, phase: Phase) {
        update(&self.state, |snapshot| snapshot.phase = phase);
    }

    /// 频谱分析用的采样通道。`Clone` 只是克隆几个 `Arc`，界面拿一份长期持有即可。
    pub fn levels(&self) -> Levels {
        self.levels.clone()
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
#[allow(clippy::too_many_arguments)]
fn run(
    rx: Receiver<Cmd>,
    state: Arc<Mutex<Snapshot>>,
    _keep_alive: impl Send + 'static,
    mixer: rodio::mixer::Mixer,
    running: Arc<AtomicBool>,
    cache_on: bool,
    promoted: cache::Promoted,
    levels: Levels,
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
                    &levels,
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
                        &levels,
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
    levels: &Levels,
) {
    match cmd {
        Cmd::Play(track) => {
            // 丢掉旧的播放器就是「停掉上一首」。
            *player = None;
            *seek_anchor = None;
            // 上一首的波形不能留着，否则新歌起播时频谱会先亮一下旧的尾巴。
            levels.clear();
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
                    // 包一层：采样原样透传给播放器，顺带把第 0 声道记进频谱窗口。
                    new_player.append(LevelMeter::new(opened.decoder, levels.clone()));
                    tracing::info!(
                        "播放中：{}",
                        opened.quality.map_or("本地文件（读不出档位）", Quality::label)
                    );
                    update(state, |snapshot| {
                        snapshot.quality = opened.quality;
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
    /// CDN 与缓存那份知道档位（缓存文件名里带着）；本地文件读不出来就是 `None`。
    quality: Option<Quality>,
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
    // 本地文件（曲库 / 缓存 / 下载目录）：直接开，**不碰缓存也不碰网络**。
    if let TrackSource::Local { path, quality } = &track.source {
        let file = File::open(path).with_context(|| format!("打不开：{}", path.display()))?;
        let size = file.metadata().ok().map(|metadata| metadata.len());
        let decoder = Decoder::new(MediaSource::File(file))
            .with_context(|| format!("打不开：{}", path.display()))?;
        let duration = decoder.total_duration();
        return Ok(Opened {
            decoder,
            quality: *quality,
            size,
            duration,
        });
    }
    let mid = track.song.mid.as_str();
    // 缓存里已经有这首（任意档位）：直接读本地文件，一个请求都不用发。
    //
    // 这是 [`local_copy`] 之下的一道保险：取流前那一步已经按缓存判定过了，只有
    // 「`Song` 说是 QQ 歌、缓存却在这之后才出现」这类分叉才会走到这里。
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
                            quality: Some(quality),
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
    let TrackSource::Cdn(candidates) = &track.source else {
        // 本地那份在上面就返回了，所以这里只可能是 CDN——对不上就说明构造 `Track` 时写错了。
        bail!("播放源与路径不一致：{}", track.song.mid);
    };
    let mut failures = Vec::new();
    for candidate in candidates {
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
                    quality: Some(candidate.quality),
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

    /// 「本地已有这份吗」的顺序：曲库路径 → 播放缓存 → 下载目录；缓存关掉（`cache_mb = 0`）
    /// 就整段缓存判定不作数。这条判定只有一处，顺序错了就会出现「本地明明有那份还去取流」。
    #[test]
    fn local_copy_prefers_the_library_then_the_cache_then_the_download() {
        let cache_dir =
            std::env::temp_dir().join(format!("canto-copy-cache-{}", std::process::id()));
        let download_dir =
            std::env::temp_dir().join(format!("canto-copy-dl-{}", std::process::id()));
        std::fs::remove_dir_all(&cache_dir).ok();
        std::fs::remove_dir_all(&download_dir).ok();
        std::fs::create_dir_all(&cache_dir).expect("建缓存目录");
        std::fs::create_dir_all(&download_dir).expect("建下载目录");

        let song = Song {
            mid: "abc123".to_string(),
            song_id: None,
            media_mid: None,
            title: "成全".to_string(),
            singers: "刘若英".to_string(),
            album_mid: String::new(),
            album_name: String::new(),
            duration_secs: 276,
            sizes: crate::model::FileSizes::default(),
            local_bytes: 0,
            source: crate::model::Source::Qq,
        };
        // 下载目录那份（键就是文件名，跟列表里 `↓` 用的是同一份索引）。
        let stem = crate::download::file_stem(&song);
        let downloaded = download_dir.join(format!("{stem}.mp3"));
        std::fs::write(&downloaded, vec![0_u8; 4400]).expect("写下载文件");
        let downloads: std::collections::HashMap<String, crate::download::Downloaded> = [(
            stem,
            crate::download::Downloaded {
                path: downloaded.clone(),
                container: "MP3".to_string(),
                bytes: 4400,
            },
        )]
        .into_iter()
        .collect();
        // 缓存那份：文件名就是 `<mid>.<档位>.<扩展名>`，所以档位也读得出来。
        let cached = cache_dir.join("abc123.320.mp3");
        std::fs::write(&cached, vec![0_u8; 9800]).expect("写缓存文件");

        let local = |source: Option<TrackSource>| match source {
            Some(TrackSource::Local { path, quality }) => Some((path, quality)),
            Some(TrackSource::Cdn(_)) => panic!("本地判定不该给出线上源"),
            None => None,
        };

        assert_eq!(
            local(local_copy(&song, Some(&cache_dir), &downloads)),
            Some((cached.clone(), Some(Quality::High320))),
            "有缓存就用缓存，档位从文件名读"
        );

        std::fs::remove_file(&cached).expect("删缓存");
        assert_eq!(
            local(local_copy(&song, Some(&cache_dir), &downloads)),
            Some((downloaded.clone(), None)),
            "缓存没了就看下载目录；下载那份读不出档位"
        );

        std::fs::write(&cached, vec![0_u8; 9800]).expect("写回缓存");
        assert_eq!(
            local(local_copy(&song, None, &downloads)),
            Some((downloaded.clone(), None)),
            "`cache_mb = 0` 时缓存不算数，仍走下载目录"
        );

        let nothing = std::collections::HashMap::new();
        assert_eq!(
            local(local_copy(&song, Some(&cache_dir.join("没有这个目录")), &nothing)),
            None,
            "两边都没有才交给线上取流"
        );

        // 曲库歌：`Song` 自带路径，排在缓存前面（缓存里那个 mid 压根不会是它）。
        let path = std::env::temp_dir().join("canto-copy-local.wav");
        let local_song = Song {
            mid: crate::model::local_mid(&path),
            source: crate::model::Source::Local(path.clone()),
            ..song
        };
        assert_eq!(
            local(local_copy(&local_song, Some(&cache_dir), &nothing)),
            Some((path, None)),
            "曲库歌用自己的路径"
        );

        std::fs::remove_dir_all(&cache_dir).ok();
        std::fs::remove_dir_all(&download_dir).ok();
    }

    /// 本地文件：直接开文件解码，**不要候选也不写播放缓存**（候选取空也照样开）。
    #[test]
    fn a_local_file_opens_without_candidates_or_cache() {
        let path =
            std::env::temp_dir().join(format!("canto-local-play-{}.wav", std::process::id()));
        write_sine_wav(&path, 44_100, 1.0, 440.0);

        let song = crate::library::entry_to_song(&crate::library::read_entry(&path, 0, 0));
        assert!(song.is_local(), "库里造出来的歌是本地曲目");
        let track = Track {
            song,
            source: TrackSource::Local {
                path: path.clone(),
                quality: None,
            },
        };
        let opened = open(&track, false, &cache::Promoted::default()).expect("本地 WAV 要能开");
        assert!(
            opened.quality.is_none(),
            "本地文件读不出档位，不许瞎报一个：{:?}",
            opened.quality
        );
        assert!(
            opened.duration.is_some_and(|duration| duration > Duration::ZERO),
            "解码器报得出时长：{:?}",
            opened.duration
        );
        assert!(
            crate::cache::for_song(&crate::model::local_mid(&path)).is_none(),
            "本地文件不写播放缓存"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// 冒烟：真解码器 → 采集通道 → 分析器，整条链路在没有声卡的机器上也能跑。
    ///
    /// 别处的测试只能零散地验各段（哑引擎没人拉样本，频谱恒为零；分析器只喂合成正弦）。
    /// 这一条直接拉一个**真解码器**，把「真音频流进来之后窗口里有波形、频段不是全零、
    /// 而且有效采样率来自解码器」这件事一起钉住——它也是这条链路唯一能进 CI 的端到端证据。
    #[test]
    fn a_real_decoder_feeds_the_spectrum_window() {
        let path =
            std::env::temp_dir().join(format!("canto-spectrum-smoke-{}.wav", std::process::id()));
        write_sine_wav(&path, 44_100, 1.0, 440.0);

        let file = File::open(&path).expect("打开临时 WAV");
        let decoder = Decoder::new(MediaSource::File(file)).expect("解码 WAV");
        let levels = Levels::new();
        let mut meter = LevelMeter::new(decoder, levels.clone());
        // 拉满一个窗口（单声道 44.1kHz，抽稀倍率是 1）
        for _ in 0..crate::spectrum::WINDOW {
            meter.next().expect("还有采样");
        }
        let _ = std::fs::remove_file(&path);

        let mut window = Vec::new();
        let rate = levels.read_into(&mut window);
        assert_eq!(rate, 44_100, "有效采样率该是解码器报的");
        assert!(
            window.iter().any(|&sample| sample.abs() > 0.05),
            "窗口里应当有真实波形"
        );

        let mut analyzer = crate::spectrum::Analyzer::new();
        let mut bands = Vec::new();
        analyzer.analyze_into(&window, rate, &mut bands);
        assert_eq!(bands.len(), crate::spectrum::BANDS);
        assert!(
            bands.iter().any(|&value| value > 0.0),
            "440Hz 的正弦应当点亮某些频段，而不是全零"
        );
    }

    /// 手写一个最小 WAV（44 字节头 + 16bit PCM 单声道）。文件名带 pid，并行测试不会撞。
    fn write_sine_wav(path: &std::path::Path, rate: u32, seconds: f32, frequency: f32) {
        let count = (rate as f32 * seconds) as usize;
        let samples: Vec<i16> = (0..count)
            .map(|i| {
                (12_000.0 * (2.0 * std::f32::consts::PI * frequency * i as f32 / rate as f32).sin())
                    as i16
            })
            .collect();
        let data_len = (samples.len() * 2) as u32;
        let mut bytes = Vec::with_capacity(44 + data_len as usize);
        bytes.extend(b"RIFF");
        bytes.extend((36 + data_len).to_le_bytes());
        bytes.extend(b"WAVE");
        bytes.extend(b"fmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes()); // PCM
        bytes.extend(1u16.to_le_bytes()); // 单声道
        bytes.extend(rate.to_le_bytes());
        bytes.extend((rate * 2).to_le_bytes()); // 字节率
        bytes.extend(2u16.to_le_bytes()); // 块对齐
        bytes.extend(16u16.to_le_bytes()); // 位深
        bytes.extend(b"data");
        bytes.extend(data_len.to_le_bytes());
        for sample in samples {
            bytes.extend(sample.to_le_bytes());
        }
        std::fs::write(path, bytes).expect("写临时 WAV");
    }
}

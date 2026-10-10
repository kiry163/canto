//! 界面状态与消息处理。
//!
//! 这里只有「状态 + 网络消息 + 按键派发」，不画一帧（绘制在 [`super::draw`]）。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use crossterm::event::KeyEvent;
use ratatui::widgets::ListState;

use crate::config::Config;
use crate::download;
use crate::model::{Lyric, Playlist, Song};
use crate::player::{Engine, Track};
use crate::qq;
use crate::qq::vkey::Candidate;

use super::keys;

/// 连续遇到不可播曲目时最多自动跳几首——整个歌单都没版权时别一路刷到底。
const MAX_AUTO_SKIPS: u32 = 3;

/// 后台任务的回话。
pub(super) enum Msg {
    Playlists(Result<Vec<Playlist>>),
    Tracks {
        playlist_id: String,
        result: Result<(Playlist, Vec<Song>)>,
    },
    /// 取流结果。`request` 用来丢弃过期回话（用户可能已经换了歌单又按了播放）。
    Candidates {
        request: u64,
        index: usize,
        song: Song,
        result: Result<Vec<Candidate>>,
    },
    /// 搜索结果。
    Search {
        keyword: String,
        result: Result<Vec<Song>>,
    },
    /// 歌单写入（收藏/加歌单/移除）的结果；`refresh` 是要重新拉取的歌单。
    Wrote {
        done: String,
        refresh: Option<(String, String)>,
        result: Result<()>,
    },
    /// 歌词结果（同样是异步的，同样按 `request` 过滤过期回话）。
    Lyric {
        request: u64,
        title: String,
        result: Result<Option<Lyric>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Focus {
    Playlists,
    Tracks,
}

/// 后台下载的进度（界面每帧读一次）。
pub(super) struct Transfer {
    label: String,
    received: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
    /// 完成后写上结果文案。
    outcome: Option<String>,
}

/// 一次歌单写入的意图（界面线程 → 后台任务）。
pub(super) struct WriteRequest {
    /// 进行中时状态栏的文案。
    busy: String,
    /// 成功后的文案。
    done: String,
    /// 成功后要重新拉取的歌单（移除会改右栏）。
    refresh: Option<(String, String)>,
    song: Song,
    /// 写入接口要的目录号（「我喜欢」= 201）。
    dir_id: u64,
    /// 写入接口要的 `tid`。
    tid: u64,
    /// `true` = 加进去，`false` = 移除。
    add: bool,
}

pub(super) struct App {
    pub(super) handle: tokio::runtime::Handle,
    pub(super) engine: Engine,
    pub(super) tx: Sender<Msg>,
    /// 启动时载入的配置（音质下限、下载目录、历史上限）。
    pub(super) config: Config,

    pub(super) playlists: Vec<Playlist>,
    pub(super) playlist_state: ListState,
    pub(super) tracks: Vec<Song>,
    pub(super) track_state: ListState,
    /// 右栏当前显示的是哪个歌单（id, 名字）。
    pub(super) shown: Option<(String, String)>,
    pub(super) focus: Focus,

    /// 播放队列 = 按下播放那一刻的曲目快照；换歌单不影响它。
    pub(super) queue: Vec<Song>,
    pub(super) queue_index: usize,
    pub(super) queue_name: String,
    /// 取流请求序号，用于丢弃过期回话。
    pub(super) request: u64,
    /// 连续自动跳过的不可播曲目数（手动播放或成功播放后清零）。
    pub(super) skipped: u32,

    /// 当前这首歌的歌词（`None` = 还没拿到或确实没有）。
    pub(super) lyric: Option<Lyric>,
    pub(super) lyric_loading: bool,
    /// 歌词面板是否展开。
    pub(super) show_lyric: bool,
    pub(super) lyric_state: ListState,
    /// `Some` 表示正在输入搜索词。
    pub(super) input: Option<String>,
    /// `Some` 表示正在挑歌单（`a` 键）：候选歌单、高亮、要加进去的歌。
    pub(super) picker: Option<(Vec<Playlist>, ListState, Song)>,
    /// 正在跑的下载。
    pub(super) transfer: Option<Arc<Mutex<Transfer>>>,

    /// 正在后台做的事（状态栏）。
    pub(super) loading: Option<String>,
    /// 一次性提示（错误、结果）。
    pub(super) status: String,
    pub(super) quit: bool,
}

impl App {
    pub(super) fn new(
        handle: tokio::runtime::Handle,
        engine: Engine,
        config: Config,
        tx: Sender<Msg>,
    ) -> Self {
        Self {
            handle,
            engine,
            tx,
            config,
            playlists: Vec::new(),
            playlist_state: ListState::default(),
            tracks: Vec::new(),
            track_state: ListState::default(),
            shown: None,
            focus: Focus::Playlists,
            queue: Vec::new(),
            queue_index: 0,
            queue_name: String::new(),
            request: 0,
            skipped: 0,
            lyric: None,
            lyric_loading: false,
            show_lyric: true,
            lyric_state: ListState::default(),
            input: None,
            picker: None,
            transfer: None,
            loading: None,
            status: "正在加载歌单…".to_string(),
            quit: false,
        }
    }

    // -- 数据加载（都在后台线程，结果走通道回来） ----------------------------

    pub(super) fn reload_playlists(&mut self) {
        self.loading = Some("加载歌单…".to_string());
        let tx = self.tx.clone();
        self.handle.spawn(async move {
            let _ = tx.send(Msg::Playlists(qq::user_playlists().await));
        });
    }

    fn load_tracks(&mut self, id: String, name: String) {
        self.loading = Some(format!("加载《{name}》…"));
        let tx = self.tx.clone();
        let task_id = id.clone();
        self.handle.spawn(async move {
            let result = if task_id == Playlist::FAVORITES_ID {
                qq::favorites().await
            } else {
                qq::playlist_tracks(&task_id).await
            };
            let _ = tx.send(Msg::Tracks {
                playlist_id: task_id,
                result,
            });
        });
    }

    /// 下载右栏里选中的那首（含歌词），进度走状态栏。
    fn download_selected(&mut self) {
        if self.transfer.is_some() {
            self.status = "已经有一个下载在跑".to_string();
            return;
        }
        let Some(song) = self
            .tracks
            .get(self.track_state.selected().unwrap_or(0))
            .cloned()
        else {
            return;
        };
        let dir = self.config.download_dir.clone();
        let quality = self.config.quality;
        let received = Arc::new(AtomicU64::new(0));
        let total = Arc::new(AtomicU64::new(0));
        let transfer = Arc::new(Mutex::new(Transfer {
            label: format!("{} — {}", song.title, song.singers),
            received: received.clone(),
            total: total.clone(),
            outcome: None,
        }));
        self.transfer = Some(transfer.clone());
        self.loading = Some(format!("下载《{}》…", song.title));
        // 下载是阻塞 I/O（`reqwest::blocking`），放独立线程；取候选/歌词走运行时。
        let handle = self.handle.clone();
        std::thread::spawn(move || {
            let outcome = match handle.block_on(async {
                let candidates = qq::vkey::candidates(&song, Some(quality)).await?;
                let lyric = qq::lyric::get(&song).await.ok().flatten();
                Ok::<_, anyhow::Error>((candidates, lyric))
            }) {
                Ok((candidates, lyric)) => {
                    let (received, total) = (received.clone(), total.clone());
                    let mut on_progress = move |progress: u64, size: Option<u64>| {
                        received.store(progress, Ordering::Relaxed);
                        total.store(size.unwrap_or(0), Ordering::Relaxed);
                    };
                    match download::save(&song, &candidates, &dir, lyric.as_ref(), &mut on_progress)
                    {
                        Ok(saved) if saved.existed => {
                            format!("已存在：{}", saved.path.display())
                        }
                        Ok(saved) => format!(
                            "已下载：{}（{:.1} MB，{}）",
                            saved.path.display(),
                            saved.bytes as f64 / 1_048_576.0,
                            saved.quality.label()
                        ),
                        Err(error) => format!("下载失败：{error:#}"),
                    }
                }
                Err(error) => format!("下载失败：{error:#}"),
            };
            if let Ok(mut transfer) = transfer.lock() {
                transfer.outcome = Some(outcome);
            }
        });
    }

    /// 右栏当前选中的那首。
    fn selected_song(&self) -> Option<Song> {
        self.tracks
            .get(self.track_state.selected().unwrap_or(0))
            .cloned()
    }

    /// 歌单写入统一入口：后台跑，结果回状态栏。
    fn write_song(&mut self, request: WriteRequest) {
        let WriteRequest {
            busy,
            done,
            refresh,
            song,
            dir_id,
            tid,
            add,
        } = request;
        self.loading = Some(busy);
        let tx = self.tx.clone();
        self.handle.spawn(async move {
            let songs = std::slice::from_ref(&song);
            let result = if add {
                qq::write::add(dir_id, tid, songs).await.map(|_| ())
            } else {
                qq::write::remove(dir_id, tid, songs).await.map(|_| ())
            };
            let _ = tx.send(Msg::Wrote {
                done,
                refresh,
                result,
            });
        });
    }

    /// `f`：收藏右栏选中的歌。
    fn favorite_selected(&mut self) {
        let Some(song) = self.selected_song() else {
            return;
        };
        let request = WriteRequest {
            busy: format!("收藏《{}》…", song.title),
            done: format!("已收藏：{} — {}", song.title, song.singers),
            refresh: None,
            song,
            dir_id: qq::write::FAVORITES_DIR_ID,
            tid: 0,
            add: true,
        };
        self.write_song(request);
    }

    /// `a`：挑一个歌单把选中的歌加进去。
    fn open_picker(&mut self) {
        let Some(song) = self.selected_song() else {
            return;
        };
        if self.playlists.is_empty() {
            self.status = "歌单还没加载出来".to_string();
            return;
        }
        let mut state = ListState::default();
        state.select(Some(0));
        self.picker = Some((self.playlists.clone(), state, song));
    }

    fn confirm_picker(&mut self) {
        let Some((playlists, state, song)) = self.picker.take() else {
            return;
        };
        let Some(playlist) = state
            .selected()
            .and_then(|index| playlists.get(index))
            .cloned()
        else {
            return;
        };
        let request = WriteRequest {
            busy: format!("加进《{}》…", playlist.name),
            done: format!(
                "已把《{} — {}》加进《{}》",
                song.title, song.singers, playlist.name
            ),
            refresh: None,
            song,
            dir_id: playlist.dir_id,
            tid: playlist.id.parse().unwrap_or(0),
            add: true,
        };
        self.write_song(request);
    }

    /// `x`：从右栏当前显示的歌单里移除选中的歌。
    fn remove_selected(&mut self) {
        let Some(song) = self.selected_song() else {
            return;
        };
        let Some((id, name)) = self.shown.clone() else {
            return;
        };
        // 「我喜欢」在服务端走不通（实测 DelSonglist 一律 80105），早点说清楚。
        if id == Playlist::FAVORITES_ID {
            self.status = "QQ 服务端不允许接口取消收藏（80105），请在官方客户端操作".to_string();
            return;
        }
        let Some(playlist) = self.playlists.iter().find(|playlist| playlist.id == id) else {
            self.status = "右栏不是个人歌单，没法移除".to_string();
            return;
        };
        let request = WriteRequest {
            busy: format!("从《{name}》移除《{}》…", song.title),
            done: format!("已从《{name}》移除：{} — {}", song.title, song.singers),
            refresh: Some((id, name)),
            song,
            dir_id: playlist.dir_id,
            tid: playlist.id.parse().unwrap_or(0),
            add: false,
        };
        self.write_song(request);
    }

    /// 起一个后台搜索。
    fn search(&mut self, keyword: String) {
        self.loading = Some(format!("搜索「{keyword}」…"));
        let tx = self.tx.clone();
        self.handle.spawn(async move {
            let result = qq::search::songs(&keyword, 40).await;
            let _ = tx.send(Msg::Search { keyword, result });
        });
    }

    /// 从队列里播第 `index` 首：先取候选地址（后台），拿到就交给引擎。
    fn play_index(&mut self, index: usize) {
        let Some(song) = self.queue.get(index).cloned() else {
            return;
        };
        self.request += 1;
        let request = self.request;
        self.queue_index = index;
        self.loading = Some(format!("取流《{}》…", song.title));
        // 换歌先把旧歌词撤掉，免得新旧串行显示。
        self.lyric = None;
        self.lyric_loading = true;
        let tx = self.tx.clone();
        let handle_song = song.clone();
        let quality = self.config.quality;
        self.handle.spawn(async move {
            let result = qq::vkey::candidates(&handle_song, Some(quality)).await;
            let playable = result.is_ok();
            let _ = tx.send(Msg::Candidates {
                request,
                index,
                song: song.clone(),
                result,
            });
            // 取流成功才取歌词：放不了的歌没必要取。
            if playable {
                let result = qq::lyric::get(&song).await;
                let _ = tx.send(Msg::Lyric {
                    request,
                    title: song.title,
                    result,
                });
            }
        });
    }

    /// 用当前右栏的歌单开一个队列并从 `index` 开始播。
    fn play_from_tracks(&mut self, index: usize) {
        if self.tracks.is_empty() {
            return;
        }
        let index = index.min(self.tracks.len() - 1);
        self.skipped = 0;
        self.queue = self.tracks.clone();
        self.queue_name = self
            .shown
            .as_ref()
            .map(|(_, name)| name.clone())
            .unwrap_or_default();
        self.play_index(index);
    }

    // -- 消息与按键 ---------------------------------------------------------

    pub(super) fn on_msg(&mut self, message: Msg) {
        match message {
            Msg::Playlists(result) => {
                self.loading = None;
                match result {
                    Ok(playlists) => {
                        self.status = format!("共 {} 个歌单", playlists.len());
                        let first = playlists
                            .first()
                            .map(|playlist| (playlist.id.clone(), playlist.name.clone()));
                        self.playlists = playlists;
                        self.playlist_state
                            .select((!self.playlists.is_empty()).then_some(0));
                        if let Some((id, name)) = first {
                            self.load_tracks(id, name);
                        }
                    }
                    Err(error) => self.status = format!("{error:#}"),
                }
            }
            Msg::Tracks {
                playlist_id,
                result,
            } => {
                self.loading = None;
                match result {
                    Ok((playlist, songs)) => {
                        self.status = format!("《{}》{} 首", playlist.name, songs.len());
                        self.shown = Some((playlist_id, playlist.name));
                        self.track_state.select((!songs.is_empty()).then_some(0));
                        self.tracks = songs;
                    }
                    Err(error) => self.status = format!("{error:#}"),
                }
            }
            Msg::Search { keyword, result } => {
                self.loading = None;
                match result {
                    Ok(songs) => {
                        self.status = format!("搜索「{keyword}」：{} 首", songs.len());
                        self.shown =
                            Some((format!("search:{keyword}"), format!("搜索：{keyword}")));
                        self.track_state.select((!songs.is_empty()).then_some(0));
                        self.tracks = songs;
                        self.focus = Focus::Tracks;
                    }
                    Err(error) => self.status = format!("{error:#}"),
                }
            }
            Msg::Wrote {
                done,
                refresh,
                result,
            } => {
                self.loading = None;
                self.status = match result {
                    Ok(()) => {
                        // 移除后右栏就过期了，重新拉一遍。
                        if let Some((id, name)) = refresh {
                            self.load_tracks(id, name);
                        }
                        done
                    }
                    Err(error) => format!("{error:#}"),
                };
            }
            Msg::Lyric {
                request,
                title,
                result,
            } => self.on_lyric(request, title, result),
            Msg::Candidates {
                request,
                index,
                song,
                result,
            } => {
                if request != self.request {
                    tracing::debug!("丢弃过期的取流结果（{}）", song.title);
                    return;
                }
                self.loading = None;
                match result {
                    Ok(candidates) => {
                        tracing::info!("取到 {} 个候选档位：{}", candidates.len(), song.title);
                        self.skipped = 0;
                        self.queue_index = index;
                        self.engine.play(Track { song, candidates });
                    }
                    Err(error) => {
                        // 队列里夹着没版权的歌（实测常见）：跳过去接着放，
                        // 但连跳太多就停下报错，免得把整个歌单刷一遍。
                        let reason = format!("{error:#}");
                        if self.skipped < MAX_AUTO_SKIPS && self.queue_index + 1 < self.queue.len()
                        {
                            self.skipped += 1;
                            self.status = format!("{reason}（已跳到下一首）");
                            tracing::info!("跳过不可播的《{}》：{reason}", song.title);
                            self.play_index(self.queue_index + 1);
                        } else if self.skipped >= MAX_AUTO_SKIPS {
                            self.status =
                                format!("{reason}（连续 {MAX_AUTO_SKIPS} 首不可播，已停下）");
                        } else {
                            self.status = reason;
                        }
                    }
                }
            }
        }
    }

    fn on_lyric(&mut self, request: u64, title: String, result: Result<Option<Lyric>>) {
        if request != self.request {
            return;
        }
        self.lyric_loading = false;
        match result {
            Ok(lyric) => self.lyric = lyric,
            Err(error) => {
                tracing::warn!("取《{title}》歌词失败：{error:#}");
                self.lyric = None;
            }
        }
    }

    /// 按键入口：先问上下文，再交给纯函数映射表，最后派发。
    pub(super) fn handle_key(&mut self, key: KeyEvent) {
        let Some(action) = keys::resolve(self.context(), key) else {
            return;
        };
        self.apply(action);
    }

    /// 当前按键上下文：模态 > 输入态 > 焦点所在。
    fn context(&self) -> keys::Context {
        if self.picker.is_some() {
            return keys::Context::PlaylistPicker;
        }
        if self.input.is_some() {
            return keys::Context::SearchInput;
        }
        match self.focus {
            Focus::Playlists => keys::Context::Menu,
            Focus::Tracks => keys::Context::Content,
        }
    }

    fn apply(&mut self, action: keys::Action) {
        match action {
            keys::Action::Quit => self.quit = true,
            keys::Action::Up => self.move_selection(-1),
            keys::Action::Down => self.move_selection(1),
            keys::Action::First => self.select_edge(false),
            keys::Action::Last => self.select_edge(true),
            keys::Action::PageUp => self.move_selection(-10),
            keys::Action::PageDown => self.move_selection(10),
            keys::Action::FocusNext | keys::Action::FocusPrev => {
                self.focus = match self.focus {
                    Focus::Playlists => Focus::Tracks,
                    Focus::Tracks => Focus::Playlists,
                };
            }
            keys::Action::Enter => {
                if self.picker.is_some() {
                    self.confirm_picker();
                    return;
                }
                match self.focus {
                    Focus::Playlists => self.enter_playlist(),
                    Focus::Tracks => {
                        let index = self.track_state.selected().unwrap_or(0);
                        self.play_from_tracks(index);
                    }
                }
            }
            keys::Action::Search => self.input = Some(String::new()),
            keys::Action::Download => self.download_selected(),
            keys::Action::Favorite => self.favorite_selected(),
            keys::Action::AddToPlaylist => self.open_picker(),
            keys::Action::RemoveFromPlaylist => self.remove_selected(),
            keys::Action::ToggleLyric => self.show_lyric = !self.show_lyric,
            keys::Action::PlayPause => self.engine.toggle_pause(),
            keys::Action::Next => self.play_relative(1),
            keys::Action::Prev => self.play_relative(-1),
            keys::Action::SeekBack => self.engine.seek_by(-10),
            keys::Action::SeekForward => self.engine.seek_by(10),
            keys::Action::VolumeUp => self.engine.adjust_volume(5),
            keys::Action::VolumeDown => self.engine.adjust_volume(-5),
            keys::Action::InputChar(character) => {
                if let Some(buffer) = self.input.as_mut() {
                    buffer.push(character);
                }
            }
            keys::Action::InputBackspace => {
                if let Some(buffer) = self.input.as_mut() {
                    buffer.pop();
                }
            }
            keys::Action::InputSubmit => {
                let keyword = self.input.take().unwrap_or_default().trim().to_string();
                if !keyword.is_empty() {
                    self.search(keyword);
                }
            }
            keys::Action::InputCancel => self.input = None,
            keys::Action::Back => {
                if self.picker.is_some() {
                    self.picker = None;
                } else if self.input.is_some() {
                    self.input = None;
                }
            }
        }
    }

    fn enter_playlist(&mut self) {
        let Some(index) = self.playlist_state.selected() else {
            return;
        };
        let Some(playlist) = self.playlists.get(index) else {
            return;
        };
        self.focus = Focus::Tracks;
        self.load_tracks(playlist.id.clone(), playlist.name.clone());
    }

    fn play_relative(&mut self, delta: isize) {
        if self.queue.is_empty() {
            return;
        }
        self.skipped = 0;
        let next = self.queue_index as isize + delta;
        if next < 0 {
            self.status = "已经是第一首".to_string();
            return;
        }
        if next as usize >= self.queue.len() {
            self.status = "已经是最后一首".to_string();
            return;
        }
        self.play_index(next as usize);
    }

    fn move_selection(&mut self, delta: isize) {
        let (state, len) = match self.focus {
            Focus::Playlists => (&mut self.playlist_state, self.playlists.len()),
            Focus::Tracks => (&mut self.track_state, self.tracks.len()),
        };
        if len == 0 {
            return;
        }
        let current = state.selected().unwrap_or(0) as isize;
        let next = (current + delta).clamp(0, len as isize - 1) as usize;
        state.select(Some(next));
    }

    fn select_edge(&mut self, last: bool) {
        let (state, len) = match self.focus {
            Focus::Playlists => (&mut self.playlist_state, self.playlists.len()),
            Focus::Tracks => (&mut self.track_state, self.tracks.len()),
        };
        if len > 0 {
            state.select(Some(if last { len - 1 } else { 0 }));
        }
    }

    /// 每帧收尾：引擎死了要报，播完了要接上下一首，下载进度刷到状态栏。
    pub(super) fn tick(&mut self) {
        self.poll_transfer();
        if !self.engine.is_alive() {
            self.status = "音频线程已退出，重启 canto".to_string();
            return;
        }
        if self.engine.take_finished() {
            let snapshot = self.engine.snapshot();
            if !snapshot.completed() {
                self.status = format!(
                    "《{}》流被中断（只播了 {:.0} 秒）",
                    snapshot.title.unwrap_or_default(),
                    snapshot.position.as_secs_f64()
                );
            }
            if self.queue_index + 1 < self.queue.len() {
                self.play_index(self.queue_index + 1);
            }
        }
    }

    pub(super) fn poll_transfer(&mut self) {
        let Some(transfer) = self.transfer.clone() else {
            return;
        };
        let Ok(state) = transfer.lock() else { return };
        if let Some(outcome) = &state.outcome {
            let outcome = outcome.clone();
            drop(state);
            self.transfer = None;
            self.loading = None;
            self.status = outcome;
            return;
        }
        let (received, total) = (
            state.received.load(Ordering::Relaxed),
            state.total.load(Ordering::Relaxed),
        );
        self.loading = Some(
            match received
                .checked_mul(100)
                .and_then(|scaled| scaled.checked_div(total))
            {
                Some(percent) => format!("下载《{}》… {percent}%", state.label),
                None => format!("下载《{}》…", state.label),
            },
        );
    }
}

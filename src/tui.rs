//! 终端界面（ratatui）。
//!
//! 线程模型：本线程只做「画帧 + 读键盘」；网络请求丢给 tokio 运行时（结果用通道送回），
//! 播放交给 [`crate::player::Engine`]。三者之间没有共享可变状态，只有状态快照与消息。
//!
//! 键位：
//! - `q` / `Ctrl-C` 退出，`Tab` 切换左右栏
//! - `j`/`k`（或方向键上下）移动，`g`/`G` 首尾
//! - `Enter` 左栏=加载该歌单，右栏=播放该首
//! - `Space` 播放/暂停，`n`/`b` 下一首/上一首，`←`/`→`（或 `h`/`l`）±10 秒，`+`/`-` 音量
//! - `y` 开/关歌词面板（跟着播放位置高亮当前行，带翻译）
//! - `/` 搜索（回车搜、Esc 取消；结果显示在曲目栏，直接回车播放）
//! - `d` 下载选中的那首（含同名歌词）到默认目录，进度显示在状态栏

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Gauge, List, ListItem, ListState, Paragraph};

use crate::download;
use crate::model::{Lyric, Playlist, Song};
use crate::player::{Engine, Phase, Track};
use crate::qq;
use crate::qq::vkey::Candidate;

/// 界面刷新间隔，同时也是按键与后台消息的最长等待。
const TICK: StdDuration = StdDuration::from_millis(150);

/// 连续遇到不可播曲目时最多自动跳几首——整个歌单都没版权时别一路刷到底。
const MAX_AUTO_SKIPS: u32 = 3;

/// 后台任务的回话。
enum Msg {
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
enum Focus {
    Playlists,
    Tracks,
}

pub fn run(handle: tokio::runtime::Handle, engine: Engine) -> Result<()> {
    let (tx, rx) = channel();
    let mut app = App::new(handle, engine, tx);
    app.reload_playlists();

    let mut terminal = ratatui::init();
    let outcome = event_loop(&mut terminal, &mut app, &rx);
    ratatui::restore();
    outcome
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    rx: &Receiver<Msg>,
) -> Result<()> {
    while !app.quit {
        terminal.draw(|frame| app.draw(frame)).context("渲染失败")?;
        if event::poll(TICK).context("读取键盘事件失败")?
            && let Event::Key(key) = event::read().context("读取键盘事件失败")?
            && key.kind == KeyEventKind::Press
        {
            app.handle_key(key);
        }
        loop {
            match rx.try_recv() {
                Ok(message) => app.on_msg(message),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        app.tick();
    }
    Ok(())
}

/// 后台下载的进度（界面每帧读一次）。
struct Transfer {
    label: String,
    received: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
    /// 完成后写上结果文案。
    outcome: Option<String>,
}

/// 一次歌单写入的意图（界面线程 → 后台任务）。
struct WriteRequest {
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

struct App {
    handle: tokio::runtime::Handle,
    engine: Engine,
    tx: Sender<Msg>,

    playlists: Vec<Playlist>,
    playlist_state: ListState,
    tracks: Vec<Song>,
    track_state: ListState,
    /// 右栏当前显示的是哪个歌单（id, 名字）。
    shown: Option<(String, String)>,
    focus: Focus,

    /// 播放队列 = 按下播放那一刻的曲目快照；换歌单不影响它。
    queue: Vec<Song>,
    queue_index: usize,
    queue_name: String,
    /// 取流请求序号，用于丢弃过期回话。
    request: u64,
    /// 连续自动跳过的不可播曲目数（手动播放或成功播放后清零）。
    skipped: u32,

    /// 当前这首歌的歌词（`None` = 还没拿到或确实没有）。
    lyric: Option<Lyric>,
    lyric_loading: bool,
    /// 歌词面板是否展开。
    show_lyric: bool,
    lyric_state: ListState,
    /// `Some` 表示正在输入搜索词。
    input: Option<String>,
    /// `Some` 表示正在挑歌单（`a` 键）：候选歌单、高亮、要加进去的歌。
    picker: Option<(Vec<Playlist>, ListState, Song)>,
    /// 正在跑的下载。
    transfer: Option<Arc<Mutex<Transfer>>>,

    /// 正在后台做的事（状态栏）。
    loading: Option<String>,
    /// 一次性提示（错误、结果）。
    status: String,
    quit: bool,
}

impl App {
    fn new(handle: tokio::runtime::Handle, engine: Engine, tx: Sender<Msg>) -> Self {
        Self {
            handle,
            engine,
            tx,
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

    fn reload_playlists(&mut self) {
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
        let dir = download::default_dir();
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
                let candidates = qq::vkey::candidates(&song, None).await?;
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
        self.handle.spawn(async move {
            let result = qq::vkey::candidates(&handle_song, None).await;
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

    fn on_msg(&mut self, message: Msg) {
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

    fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        // 输入模式优先：这里 `q` 是字母，不是退出。
        if let Some(buffer) = self.input.as_mut() {
            match key.code {
                KeyCode::Enter => {
                    let keyword = buffer.trim().to_string();
                    self.input = None;
                    if !keyword.is_empty() {
                        self.search(keyword);
                    }
                }
                KeyCode::Esc => self.input = None,
                KeyCode::Backspace => {
                    buffer.pop();
                }
                KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    buffer.push(character);
                }
                _ => {}
            }
            return;
        }
        // 选择框模式优先：这里 `q` 也只是字母。
        if self.picker.is_some() {
            match key.code {
                KeyCode::Esc => self.picker = None,
                KeyCode::Char('j') | KeyCode::Down => {
                    if let Some((playlists, state, _)) = self.picker.as_mut() {
                        let next = state.selected().unwrap_or(0) + 1;
                        state.select(Some(next.min(playlists.len().saturating_sub(1))));
                    }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    if let Some((_, state, _)) = self.picker.as_mut() {
                        let previous = state.selected().unwrap_or(0).saturating_sub(1);
                        state.select(Some(previous));
                    }
                }
                KeyCode::Enter => self.confirm_picker(),
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::Char('g') => self.select_edge(false),
            KeyCode::Char('G') => self.select_edge(true),
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::Playlists => Focus::Tracks,
                    Focus::Tracks => Focus::Playlists,
                };
            }
            KeyCode::Enter => match self.focus {
                Focus::Playlists => self.enter_playlist(),
                Focus::Tracks => {
                    let index = self.track_state.selected().unwrap_or(0);
                    self.play_from_tracks(index);
                }
            },
            KeyCode::Char('/') => self.input = Some(String::new()),
            KeyCode::Char('d') => self.download_selected(),
            KeyCode::Char('f') => self.favorite_selected(),
            KeyCode::Char('a') => self.open_picker(),
            KeyCode::Char('x') => self.remove_selected(),
            KeyCode::Char('y') => self.show_lyric = !self.show_lyric,
            KeyCode::Char(' ') => self.engine.toggle_pause(),
            KeyCode::Char('n') => self.play_relative(1),
            KeyCode::Char('b') => self.play_relative(-1),
            // `h`/`l` 是 vim 风格的左右，跟上面的 j/k/g/G 一套。
            KeyCode::Left | KeyCode::Char('h') => self.engine.seek_by(-10),
            KeyCode::Right | KeyCode::Char('l') => self.engine.seek_by(10),
            KeyCode::Char('+') | KeyCode::Char('=') => self.engine.adjust_volume(5),
            KeyCode::Char('-') => self.engine.adjust_volume(-5),
            _ => {}
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
    fn tick(&mut self) {
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

    // -- 渲染 ---------------------------------------------------------------

    fn poll_transfer(&mut self) {
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

    fn draw(&mut self, frame: &mut Frame) {
        let rows = Layout::vertical([
            Constraint::Min(5),
            // 播放条：上下边框各 1 行 + 标题行 + 进度条行。
            Constraint::Length(4),
            Constraint::Length(1),
        ])
        .split(frame.area());
        let columns =
            Layout::horizontal([Constraint::Length(26), Constraint::Min(20)]).split(rows[0]);
        self.draw_playlists(frame, columns[0]);
        // 右栏：曲目 + 歌词（终端太矮时不展开，免得曲目只剩一两行）。
        let lyric_height = if self.show_lyric && columns[1].height >= 12 {
            (columns[1].height / 3).max(5)
        } else {
            0
        };
        let right = Layout::vertical([Constraint::Min(5), Constraint::Length(lyric_height)])
            .split(columns[1]);
        self.draw_tracks(frame, right[0]);
        if let Some(area) = right.get(1).filter(|_| lyric_height > 0) {
            self.draw_lyric(frame, *area);
        }
        self.draw_status(frame, rows[1], rows[2]);
        if self.picker.is_some() {
            self.draw_picker(frame, frame.area());
        }
    }

    /// `a` 键弹出的歌单选择框。
    fn draw_picker(&mut self, frame: &mut Frame, area: Rect) {
        let Some((playlists, state, song)) = self.picker.as_mut() else {
            return;
        };
        let width = area.width.saturating_sub(4).min(56);
        let height = (playlists.len() as u16 + 2).min(area.height.saturating_sub(2));
        let popup = Rect {
            x: area.x + (area.width.saturating_sub(width)) / 2,
            y: area.y + (area.height.saturating_sub(height)) / 2,
            width,
            height,
        };
        frame.render_widget(Clear, popup);
        let items: Vec<ListItem> = playlists
            .iter()
            .map(|playlist| ListItem::new(format!("{:>5}  {}", playlist.song_count, playlist.name)))
            .collect();
        let list = List::new(items)
            .block(Block::bordered().title(format!(" 把《{}》加进 ", song.title)))
            .highlight_style(highlight())
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(list, popup, state);
    }

    fn draw_playlists(&mut self, frame: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self
            .playlists
            .iter()
            .map(|playlist| {
                let marker = if playlist.is_favorites { "♥" } else { " " };
                ListItem::new(format!(
                    "{marker} {:>5}  {}",
                    playlist.song_count, playlist.name
                ))
            })
            .collect();
        let list = List::new(items)
            .block(focused_block(" 歌单 ", self.focus == Focus::Playlists))
            .highlight_style(highlight())
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(list, area, &mut self.playlist_state);
    }

    fn draw_tracks(&mut self, frame: &mut Frame, area: Rect) {
        let (title, playing) = (
            match &self.shown {
                Some((id, name)) => format!(" {name}（{id}） "),
                None => " 曲目 ".to_string(),
            },
            self.now_playing_label(),
        );
        let items: Vec<ListItem> = self
            .tracks
            .iter()
            .map(|song| {
                let marker = if Some(song_label(song)) == playing {
                    "♪"
                } else {
                    " "
                };
                ListItem::new(format!(
                    "{marker} {:>5}  {:<6}  {} — {}",
                    format_duration(StdDuration::from_secs(song.duration_secs)),
                    song.sizes.best_label(),
                    song.title,
                    song.singers,
                ))
            })
            .collect();
        let list = List::new(items)
            .block(focused_block(&title, self.focus == Focus::Tracks))
            .highlight_style(highlight())
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(list, area, &mut self.track_state);
    }

    /// 歌词面板：当前行高亮，跟着播放位置走，翻译跟在本行下面。
    fn draw_lyric(&mut self, frame: &mut Frame, area: Rect) {
        let title = match &self.lyric {
            Some(lyric) => format!(" 歌词（{} 行） ", lyric.lines.len()),
            None => " 歌词 ".to_string(),
        };
        let block = Block::bordered().title(title);
        let Some(lyric) = &self.lyric else {
            let text = if self.lyric_loading {
                "歌词加载中…"
            } else {
                "暂无歌词"
            };
            frame.render_widget(
                Paragraph::new(text)
                    .style(Style::new().fg(Color::DarkGray))
                    .block(block),
                area,
            );
            return;
        };
        let items: Vec<ListItem> = lyric
            .lines
            .iter()
            .map(|line| {
                let mut rows = vec![Line::from(line.text.clone())];
                if let Some(translation) = &line.translation {
                    rows.push(
                        Line::from(translation.clone()).style(Style::new().fg(Color::DarkGray)),
                    );
                }
                ListItem::new(rows)
            })
            .collect();
        let current = lyric.line_at(self.engine.snapshot().position);
        self.lyric_state.select(current);
        let list = List::new(items)
            .block(block)
            .highlight_symbol("")
            .highlight_style(Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD));
        frame.render_stateful_widget(list, area, &mut self.lyric_state);
    }

    /// 正在播的那首的「标题 — 歌手」（没在播/播完就返回 `None`）。
    fn now_playing_label(&self) -> Option<String> {
        let snapshot = self.engine.snapshot();
        if snapshot.phase == Phase::Idle {
            return None;
        }
        snapshot.title
    }

    fn draw_status(&self, frame: &mut Frame, player_area: Rect, hint_area: Rect) {
        let snapshot = self.engine.snapshot();
        let state_text = match &snapshot.phase {
            Phase::Idle => "已停止".to_string(),
            Phase::Buffering => "缓冲中…".to_string(),
            Phase::Playing => "播放中".to_string(),
            Phase::Paused => "已暂停".to_string(),
            Phase::Failed(message) => format!("失败：{message}"),
        };
        let quality = snapshot
            .quality
            .map(|quality| format!("[{}]", quality.label()))
            .unwrap_or_default();
        let queue = if self.queue.is_empty() {
            String::new()
        } else {
            format!(
                "  {}/{}  ·  音量 {}%",
                self.queue_index + 1,
                self.queue.len(),
                snapshot.volume
            )
        };
        let title = snapshot.title.clone().unwrap_or_else(|| "—".to_string());
        let colors = Style::new().fg(Color::Cyan);
        let line = Line::from(vec![
            Span::styled("♪ ", colors),
            Span::styled(title, colors.add_modifier(Modifier::BOLD)),
            Span::raw(format!("  {quality} {state_text}{queue}")),
        ]);
        let gauge = Gauge::default()
            .ratio(snapshot.progress())
            .label(format!(
                "{} / {}",
                format_duration(snapshot.position),
                snapshot
                    .duration
                    .map(format_duration)
                    .unwrap_or_else(|| "--:--".to_string())
            ))
            .gauge_style(Style::new().fg(Color::Green))
            .use_unicode(true);
        // 边框归块，里面两行分别给标题行和进度条——不然进度条会盖在标题上。
        let block = Block::bordered().title(" 播放 ");
        let inner = block.inner(player_area);
        frame.render_widget(block, player_area);
        let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(inner);
        if let Some(row) = rows.first() {
            frame.render_widget(Paragraph::new(line), *row);
        }
        if let Some(row) = rows.get(1) {
            frame.render_widget(gauge, *row);
        }

        if let Some(buffer) = &self.input {
            let line = Line::from(vec![
                Span::styled("搜索：", Style::new().fg(Color::Yellow)),
                Span::raw(buffer.clone()),
                Span::styled("▏", Style::new().fg(Color::Yellow)),
            ]);
            frame.render_widget(Paragraph::new(line), hint_area);
            return;
        }
        let hint = if let Some(loading) = &self.loading {
            format!("⏳ {loading}")
        } else if self.status.is_empty() {
            "Enter 播放 · Space 暂停 · n/b 切歌 · f 收藏 · a 加歌单 · x 移除 · Tab 切栏 · q 退出"
                .to_string()
        } else {
            self.status.clone()
        };
        frame.render_widget(
            Paragraph::new(hint).style(Style::new().fg(Color::DarkGray)),
            hint_area,
        );
    }
}

fn focused_block(title: &str, focused: bool) -> Block<'static> {
    let block = Block::bordered().title(title.to_string());
    if focused {
        block.border_style(Style::new().fg(Color::Yellow))
    } else {
        block
    }
}

fn highlight() -> Style {
    Style::new()
        .bg(Color::Indexed(236))
        .add_modifier(Modifier::BOLD)
}

/// 列表行与引擎快照共用的显示串（用来对齐"正在播的是哪一行"）。
fn song_label(song: &Song) -> String {
    format!("{} — {}", song.title, song.singers)
}

fn format_duration(duration: StdDuration) -> String {
    let seconds = duration.as_secs();
    let (hours, minutes, seconds) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

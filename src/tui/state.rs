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
use crate::history::{self, History};
use crate::download;
use crate::model::{Lyric, Playlist, Song};
use crate::player::{Engine, Track};
use crate::qq;
use crate::qq::vkey::Candidate;

use super::keys;
use super::queue;

/// 连续遇到不可播曲目时最多自动跳几首——整个歌单都没版权时别一路刷到底。
const MAX_AUTO_SKIPS: u32 = 3;

/// 后台任务的回话。
pub(super) enum Msg {
    Playlists(Result<Vec<Playlist>>),
    /// 「我喜欢」的曲目（菜单第一项，启动就拉）。
    Favorites(Result<Vec<Song>>),
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

/// 焦点所在的区域。菜单与主区之外，歌词在 Task 8 加进来。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Focus {
    Menu,
    Content,
}

/// 左侧菜单项。顺序即数字键 `1`–`4` 的顺序。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Menu {
    Favorites,
    Playlists,
    Search,
    History,
}

impl Menu {
    pub const ALL: [Menu; 4] = [Menu::Favorites, Menu::Playlists, Menu::Search, Menu::History];

    pub fn index(self) -> usize {
        Menu::ALL.iter().position(|item| *item == self).unwrap_or(0)
    }

    pub fn title(self) -> &'static str {
        match self {
            Menu::Favorites => "我的收藏",
            Menu::Playlists => "我的歌单",
            Menu::Search => "搜索",
            Menu::History => "历史",
        }
    }
}

/// 一屏列表的通用状态：条目 + 选中位置 + 这一屏的错误 + 可选的右侧副标题。
#[derive(Debug)]
pub(super) struct ListView<T> {
    pub items: Vec<T>,
    pub state: ListState,
    /// 这一屏自己的错误（别的屏没事）。
    pub error: Option<String>,
    /// 每行的右侧副标题（历史屏放「时间 · 来自《来源》」，别的屏留空）。
    /// 与 `items` 等长，改 `items` 的代码要顺手改它。
    pub labels: Vec<String>,
}

impl<T> Default for ListView<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            state: ListState::default(),
            error: None,
            labels: Vec::new(),
        }
    }
}

impl<T> ListView<T> {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn selected(&self) -> Option<&T> {
        self.state.selected().and_then(|index| self.items.get(index))
    }

    pub fn select_first(&mut self) {
        self.state.select((!self.items.is_empty()).then_some(0));
    }

    pub fn move_by(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let current = self.state.selected().unwrap_or(0) as isize;
        let next = (current + delta).clamp(0, self.items.len() as isize - 1);
        self.state.select(Some(next as usize));
    }

    pub fn edge(&mut self, last: bool) {
        if !self.items.is_empty() {
            self.state.select(Some(if last { self.items.len() - 1 } else { 0 }));
        }
    }
}

/// 界面导航状态：哪个菜单项、焦点在哪、四屏各自的列表与选中位置。
///
/// 每个菜单项各存一份列表，所以「切走再切回」不会丢上下文——这正是旧界面
/// 「搜一下歌单就没了」的修法。
#[derive(Debug)]
pub(super) struct Nav {
    pub menu: Menu,
    pub focus: Focus,
    pub favorites: ListView<Song>,
    pub playlists: ListView<Playlist>,
    /// 「我的歌单 › 某歌单」这一层的曲目。
    pub tracks: ListView<Song>,
    /// 搜索结果单独一份：跟 `tracks` 共用会把正在看的歌单曲目冲掉。
    pub search_results: ListView<Song>,
    pub history_view: ListView<Song>,
    /// 当前曲目层对应的歌单（`None` = 不在嵌套层里）。
    pub shown: Option<(String, String)>,
    pub breadcrumb: Vec<String>,
    /// 这两屏的数据拉过没有（空列表也可能是「真的没有」，不能靠 `is_empty` 判断）。
    pub favorites_loaded: bool,
    pub playlists_loaded: bool,
    /// 搜索框里的关键词（退出输入态也不清空，方便改一改再搜）。
    pub search_text: String,
    /// 输入框是否正在吃键。
    pub search_input: bool,
}

impl Nav {
    pub fn new() -> Self {
        Self {
            menu: Menu::Favorites,
            focus: Focus::Menu,
            favorites: ListView::default(),
            playlists: ListView::default(),
            tracks: ListView::default(),
            search_results: ListView::default(),
            history_view: ListView::default(),
            shown: None,
            breadcrumb: vec!["我的收藏".to_string()],
            favorites_loaded: false,
            playlists_loaded: false,
            search_text: String::new(),
            search_input: false,
        }
    }

    /// 切菜单项：内容换屏，但每屏自己的选中位置都留着。
    pub fn switch_menu(&mut self, menu: Menu) {
        self.menu = menu;
        self.focus = Focus::Content;
        self.breadcrumb = vec![menu.title().to_string()];
        // 搜索屏进来就该能打字，不用先按一下 `/`。
        self.search_input = menu == Menu::Search;
    }

    /// `/`：跳到搜索屏并聚焦输入框，任何位置都能用。
    pub fn begin_search_input(&mut self) {
        self.switch_menu(Menu::Search);
    }

    /// 退出输入态。关键词留着——退出输入不等于放弃这次搜索。
    pub fn end_search_input(&mut self) {
        self.search_input = false;
    }

    /// 主区这一屏的「来源名」，给队列显示用。
    pub fn origin_name(&self) -> String {
        match (self.menu, &self.shown) {
            (Menu::Playlists, Some((_, name))) => name.clone(),
            (menu, _) => menu.title().to_string(),
        }
    }

    /// 焦点在主区时，能不能进入下一层（歌单列表 → 曲目）。
    pub fn can_enter_depth(&self) -> bool {
        self.menu == Menu::Playlists && self.shown.is_none()
    }

    /// 进入「我的歌单 › 嗯」这一层：清掉旧曲目、写面包屑。
    pub fn push_tracks(&mut self, id: &str, name: &str, songs: Vec<Song>) {
        self.shown = Some((id.to_string(), name.to_string()));
        self.tracks = ListView {
            items: songs,
            state: ListState::default(),
            error: None,
            labels: Vec::new(),
        };
        self.tracks.select_first();
        self.breadcrumb = vec!["我的歌单".to_string(), name.to_string()];
    }

    /// 请求已发出、结果还没回来时先进入这一层（曲目留空，等 `Msg::Tracks` 填）。
    pub fn begin_tracks(&mut self, id: &str, name: &str) {
        self.push_tracks(id, name, Vec::new());
    }

    /// 返回上一层。返回 `true` 表示这次的 Esc 被吃掉了。
    pub fn pop_tracks(&mut self) -> bool {
        if self.shown.take().is_none() {
            return false;
        }
        self.tracks = ListView::default();
        self.breadcrumb = vec!["我的歌单".to_string()];
        true
    }

    /// 主区这一屏的曲目列表（跟焦点无关，绘制用）。
    pub fn song_view(&self) -> &ListView<Song> {
        match self.menu {
            Menu::Favorites => &self.favorites,
            Menu::Playlists => &self.tracks,
            Menu::Search => &self.search_results,
            Menu::History => &self.history_view,
        }
    }

    /// 上面那份列表的可变版本。
    pub fn song_view_mut(&mut self) -> &mut ListView<Song> {
        match self.menu {
            Menu::Favorites => &mut self.favorites,
            Menu::Playlists => &mut self.tracks,
            Menu::Search => &mut self.search_results,
            Menu::History => &mut self.history_view,
        }
    }

    /// 焦点在主区时「Enter / a / f / d 作用的那份曲目列表」。
    pub fn focused_songs(&self) -> Option<&ListView<Song>> {
        if self.focus != Focus::Content {
            return None;
        }
        match self.menu {
            Menu::Favorites => Some(&self.favorites),
            Menu::Playlists => self.shown.as_ref().map(|_| &self.tracks),
            Menu::Search => Some(&self.search_results),
            Menu::History => Some(&self.history_view),
        }
    }

    /// 上面那份列表的可变版本。
    pub fn focused_songs_mut(&mut self) -> Option<&mut ListView<Song>> {
        if self.focus != Focus::Content {
            return None;
        }
        match self.menu {
            Menu::Favorites => Some(&mut self.favorites),
            Menu::Playlists => self.shown.as_ref().map(|_| &mut self.tracks),
            Menu::Search => Some(&mut self.search_results),
            Menu::History => Some(&mut self.history_view),
        }
    }

    pub fn move_focus_by(&mut self, delta: isize) {
        if self.menu == Menu::Playlists && self.shown.is_none() {
            self.playlists.move_by(delta);
            return;
        }
        if let Some(view) = self.focused_songs_mut() {
            view.move_by(delta);
        }
    }

    pub fn focus_edge(&mut self, last: bool) {
        if self.menu == Menu::Playlists && self.shown.is_none() {
            self.playlists.edge(last);
            return;
        }
        if let Some(view) = self.focused_songs_mut() {
            view.edge(last);
        }
    }

    pub fn page_focus(&mut self, pages: isize) {
        // 一屏按 10 行挪：终端高度可变，10 行在常见窗口里都不到一屏，够用且不越界。
        self.move_focus_by(pages * 10);
    }
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
    /// 播放历史（内存里这份是权威，落盘在每次追加后同步）。
    pub(super) history: History,
    /// 历史文件路径（测试注入临时路径用）。
    pub(super) history_path: std::path::PathBuf,

    /// 菜单列表的高亮位置（每帧跟当前菜单项对齐）。
    pub(super) menu_state: ListState,

    /// 菜单、焦点、四屏列表与选中位置。
    pub(super) nav: Nav,

    /// 播放队列。按 Enter 把当前列表灌进来，之后增删插改的都是它；
    /// 换歌单、重新搜索都不影响已经在队列里的歌。
    pub(super) queue: queue::Queue,
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
    /// 帮助浮层开着（`?`）。
    pub(super) help_open: bool,
    /// 队列浮层开着（`u`）。
    pub(super) queue_open: bool,
    /// 队列浮层里的高亮位置。
    pub(super) queue_state: ListState,
    /// `Some` 表示正在挑歌单（`p` 键）：候选歌单、高亮、要加进去的歌。
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
        history_path: std::path::PathBuf,
        tx: Sender<Msg>,
    ) -> Self {
        Self {
            handle,
            engine,
            tx,
            config,
            history: History::default(),
            history_path,
            menu_state: ListState::default(),
            nav: Nav::new(),
            queue: queue::Queue::new(Vec::new(), 0, String::new()),
            request: 0,
            skipped: 0,
            lyric: None,
            lyric_loading: false,
            show_lyric: true,
            lyric_state: ListState::default(),
            help_open: false,
            queue_open: false,
            queue_state: ListState::default(),
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

    /// 拉「我喜欢」。它是菜单第一项，启动即拉。
    pub(super) fn reload_favorites(&mut self) {
        self.loading = Some("加载「我喜欢」…".to_string());
        let tx = self.tx.clone();
        self.handle.spawn(async move {
            let result = qq::favorites().await.map(|(_, songs)| songs);
            let _ = tx.send(Msg::Favorites(result));
        });
    }

    /// 启动：菜单停在「我的收藏」，并把它拉下来。
    pub(super) fn load_initial(&mut self) {
        self.switch_menu(Menu::Favorites);
    }

    /// 切菜单项。这一屏没拉过数据就顺手拉一次——每屏的选中位置由 `Nav` 留着，
    /// 所以来回切不会丢上下文。
    pub(super) fn switch_menu(&mut self, menu: Menu) {
        self.nav.switch_menu(menu);
        match menu {
            Menu::Favorites if !self.nav.favorites_loaded => self.reload_favorites(),
            Menu::Playlists if !self.nav.playlists_loaded => self.reload_playlists(),
            Menu::History => self.refresh_history_view(),
            _ => {}
        }
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

    /// 下载选中的那首（含歌词），进度走状态栏。
    fn download_selected(&mut self) {
        if self.transfer.is_some() {
            self.status = "已经有一个下载在跑".to_string();
            return;
        }
        let Some(song) = self.selected_song() else {
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

    /// 焦点所在曲目列表里选中的那首。
    fn selected_song(&self) -> Option<Song> {
        self.nav
            .focused_songs()
            .and_then(|view| view.selected().cloned())
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
        if self.nav.playlists.is_empty() {
            self.status = "歌单还没加载出来".to_string();
            return;
        }
        let mut state = ListState::default();
        state.select(Some(0));
        self.picker = Some((self.nav.playlists.items.clone(), state, song));
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
        let Some((id, name)) = self.nav.shown.clone() else {
            return;
        };
        // 「我喜欢」在服务端走不通（实测 DelSonglist 一律 80105），早点说清楚。
        if id == Playlist::FAVORITES_ID {
            self.status = "QQ 服务端不允许接口取消收藏（80105），请在官方客户端操作".to_string();
            return;
        }
        let Some(playlist) = self
            .nav
            .playlists
            .items
            .iter()
            .find(|playlist| playlist.id == id)
        else {
            self.status = "当前不是个人歌单，没法移除".to_string();
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
        let Some(song) = self.queue.items().get(index).cloned() else {
            return;
        };
        self.request += 1;
        let request = self.request;
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
    /// 用某个列表开新队列并从 `index` 播（`Enter` 就是这个语义）。
    fn open_queue(&mut self, songs: Vec<Song>, index: usize, origin: String) {
        if songs.is_empty() {
            return;
        }
        self.queue = queue::Queue::new(songs, index, origin);
        self.skipped = 0;
        self.start(self.queue.index());
    }

    /// 当前焦点列表 → `(曲目, 选中下标, 来源名)`。`Enter` 与 `a`/`A`（空队列时）都走这里。
    fn queue_from_focus(&self) -> Option<(Vec<Song>, usize, String)> {
        let view = self.nav.focused_songs()?;
        if view.items.is_empty() {
            return None;
        }
        Some((
            view.items.clone(),
            view.state.selected().unwrap_or(0),
            self.nav.origin_name(),
        ))
    }

    /// `Enter`：当前焦点列表 → 新队列。
    fn open_queue_from_focus(&mut self) {
        let Some((songs, index, origin)) = self.queue_from_focus() else {
            return;
        };
        self.open_queue(songs, index, origin);
    }

    /// `a` / `A`：把焦点列表里选中的那首加进队列（队列空时等价于开新队列）。
    fn push_to_queue(&mut self, front: bool) {
        let Some(song) = self.selected_song() else {
            return;
        };
        if self.queue.is_empty() {
            let origin = self.nav.origin_name();
            self.open_queue(vec![song], 0, origin);
            return;
        }
        if front {
            self.queue.insert_next(song.clone());
            self.status = format!("已插队：{} — {}", song.title, song.singers);
        } else {
            self.queue.push_back(song.clone());
            self.status = format!("已加进队列：{} — {}", song.title, song.singers);
        }
    }

    /// 按 `Step` 继续播：`Play` 去取流，`Stop` 停播并说清为什么。
    fn step(&mut self, step: queue::Step) {
        match step {
            queue::Step::Play(index) => self.start(index),
            queue::Step::Stop => {
                self.status = if self.queue.is_empty() {
                    "队列已空".to_string()
                } else {
                    "队列播完".to_string()
                };
            }
        }
    }

    /// 取流并开始播队列里的第 `index` 首。
    fn start(&mut self, index: usize) {
        self.play_index(index);
    }

    // -- 消息与按键 ---------------------------------------------------------

    pub(super) fn on_msg(&mut self, message: Msg) {
        match message {
            Msg::Favorites(result) => {
                self.loading = None;
                self.nav.favorites_loaded = true;
                match result {
                    Ok(songs) => {
                        self.status = format!("我喜欢：{} 首", songs.len());
                        self.nav.favorites.items = songs;
                        self.nav.favorites.select_first();
                    }
                    Err(error) => {
                        self.nav.favorites.error = Some(format!("{error:#}"));
                        self.status = format!("{error:#}");
                    }
                }
            }
            Msg::Playlists(result) => {
                self.loading = None;
                self.nav.playlists_loaded = true;
                match result {
                    Ok(playlists) => {
                        self.status = format!("共 {} 个歌单", playlists.len());
                        self.nav.playlists.items = playlists;
                        self.nav.playlists.select_first();
                    }
                    Err(error) => {
                        self.nav.playlists.error = Some(format!("{error:#}"));
                        self.status = format!("{error:#}");
                    }
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
                        self.nav.push_tracks(&playlist_id, &playlist.name, songs);
                    }
                    Err(error) => self.status = format!("{error:#}"),
                }
            }
            Msg::Search { keyword, result } => {
                self.loading = None;
                match result {
                    Ok(songs) => {
                        self.status = format!("搜索「{keyword}」：{} 首", songs.len());
                        self.nav.search_results.items = songs;
                        self.nav.search_results.select_first();
                        // 结果一到就把焦点放进结果列表，`j`/`k` 立刻可用。
                        self.nav.focus = Focus::Content;
                    }
                    Err(error) => {
                        self.nav.search_results.error = Some(format!("{error:#}"));
                        self.status = format!("{error:#}");
                    }
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
                        // `index` 是队列下标：从这里起，播放条与浮层的「第几首」都对得上。
                        self.queue.jump(index);
                        self.engine.play(Track {
                            song: song.clone(),
                            candidates,
                        });
                        self.record_history(&song);
                    }
                    Err(error) => {
                        // 队列里夹着没版权的歌（实测常见）：跳过去接着放，
                        // 但连跳太多就停下报错，免得把整个歌单刷一遍。
                        let reason = format!("{error:#}");
                        if self.skipped < MAX_AUTO_SKIPS {
                            self.skipped += 1;
                            self.status = format!("{reason}（已跳到下一首）");
                            tracing::info!("跳过不可播的《{}》：{reason}", song.title);
                            let step = self.queue.advance();
                            self.step(step);
                        } else {
                            self.status =
                                format!("{reason}（连续 {MAX_AUTO_SKIPS} 首不可播，已停下）");
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
    pub(super) fn context(&self) -> keys::Context {
        // 模态优先：帮助在最上层，其次是选择框，再是输入态。
        if self.help_open {
            return keys::Context::HelpOverlay;
        }
        if self.queue_open {
            return keys::Context::QueueOverlay;
        }
        if self.picker.is_some() {
            return keys::Context::PlaylistPicker;
        }
        if self.nav.search_input {
            return keys::Context::SearchInput;
        }
        match self.nav.focus {
            Focus::Menu => keys::Context::Menu,
            Focus::Content => keys::Context::Content,
        }
    }

    pub(super) fn apply(&mut self, action: keys::Action) {
        match action {
            keys::Action::Quit => self.quit = true,
            keys::Action::Help => self.help_open = !self.help_open,
            keys::Action::QueueOverlay => self.toggle_queue_overlay(),
            keys::Action::HistoryClear => {
                // 只在历史屏有意义；别的屏按了不做事（Esc 在没得返回时也一样）。
                if self.nav.menu == Menu::History {
                    match self.history.clear(&self.history_path) {
                        Ok(()) => {
                            self.refresh_history_view();
                            self.status = "已清空播放历史".to_string();
                        }
                        Err(error) => self.status = format!("{error:#}"),
                    }
                }
            }
            keys::Action::QueueDelete => self.delete_queue_item(),
            keys::Action::QueueClear => {
                self.queue.clear();
                self.queue_state.select(None);
                self.status = "队列已清空（当前这首不受影响）".to_string();
            }
            keys::Action::Menu(index) => {
                if let Some(menu) = Menu::ALL.get(index).copied() {
                    self.switch_menu(menu);
                }
            }
            keys::Action::Up => self.move_selection(-1),
            keys::Action::Down => self.move_selection(1),
            keys::Action::First => self.select_edge(false),
            keys::Action::Last => self.select_edge(true),
            keys::Action::PageUp => self.nav.page_focus(-1),
            keys::Action::PageDown => self.nav.page_focus(1),
            keys::Action::FocusNext | keys::Action::FocusPrev => {
                self.nav.focus = match self.nav.focus {
                    Focus::Menu => Focus::Content,
                    Focus::Content => Focus::Menu,
                };
            }
            keys::Action::Enter => {
                if self.picker.is_some() {
                    self.confirm_picker();
                    return;
                }
                if self.queue_open {
                    self.jump_in_queue();
                    return;
                }
                self.enter_selected();
            }
            keys::Action::Search => self.nav.begin_search_input(),
            keys::Action::Download => self.download_selected(),
            keys::Action::Favorite => self.favorite_selected(),
            keys::Action::AddToPlaylist => self.open_picker(),
            keys::Action::RemoveFromPlaylist => self.remove_selected(),
            keys::Action::ToggleLyric => self.show_lyric = !self.show_lyric,
            keys::Action::PlayPause => self.engine.toggle_pause(),
            keys::Action::Next => self.play_relative(true),
            keys::Action::Prev => self.play_relative(false),
            keys::Action::AppendQueue => self.push_to_queue(false),
            keys::Action::InsertQueue => self.push_to_queue(true),
            keys::Action::CycleMode => {
                let mode = self.queue.cycle_mode();
                self.status = format!("播放模式：{}", mode.label());
            }
            keys::Action::SeekBack => self.engine.seek_by(-10),
            keys::Action::SeekForward => self.engine.seek_by(10),
            keys::Action::VolumeUp => self.engine.adjust_volume(5),
            keys::Action::VolumeDown => self.engine.adjust_volume(-5),
            keys::Action::InputChar(character) => {
                self.nav.search_text.push(character);
            }
            keys::Action::InputBackspace => {
                self.nav.search_text.pop();
            }
            keys::Action::InputSubmit => {
                self.nav.end_search_input();
                let keyword = self.nav.search_text.trim().to_string();
                if !keyword.is_empty() {
                    self.search(keyword);
                }
            }
            keys::Action::InputCancel => self.nav.end_search_input(),
            keys::Action::Back => {
                // 顺序：先关模态，再退输入态，最后退一层视图。
                if self.help_open {
                    self.help_open = false;
                } else if self.queue_open {
                    self.queue_open = false;
                } else if self.picker.is_some() {
                    self.picker = None;
                } else if self.nav.search_input {
                    self.nav.end_search_input();
                } else if self.nav.pop_tracks() {
                    self.nav.focus = Focus::Content;
                }
            }
        }
    }

    /// `u`：开 / 关队列浮层。打开时高亮落在正在播的那首上。
    fn toggle_queue_overlay(&mut self) {
        if self.queue_open {
            self.queue_open = false;
            return;
        }
        if self.queue.is_empty() {
            self.status = "队列是空的".to_string();
            return;
        }
        self.queue_state.select(Some(self.queue.index()));
        self.queue_open = true;
    }

    /// 浮层里回车：跳播（只挪指针，不动队列顺序）。
    fn jump_in_queue(&mut self) {
        let Some(index) = self.queue_state.selected() else {
            return;
        };
        if index < self.queue.len() {
            self.skipped = 0;
            self.step(queue::Step::Play(index));
        }
    }

    /// 浮层里 `d`：从队列里删掉选中项。删到当前项就顺势播下一首。
    fn delete_queue_item(&mut self) {
        let Some(index) = self.queue_state.selected() else {
            return;
        };
        let Some(step) = self.queue.remove(index) else {
            // 删的不是当前项：队列短了一格，高亮跟着收一下。
            let next = index.min(self.queue.len().saturating_sub(1));
            self.queue_state.select((!self.queue.is_empty()).then_some(next));
            return;
        };
        if self.queue.is_empty() {
            self.queue_state.select(None);
            self.queue_open = false;
            self.status = "队列已空".to_string();
            return;
        }
        let next = index.min(self.queue.len() - 1);
        self.queue_state.select(Some(next));
        self.step(step);
    }

    /// 起播那一刻记一条历史（取流成功、交给引擎之后调用）。
    pub(super) fn record_history(&mut self, song: &Song) {
        self.history.push(
            history::Entry {
                mid: song.mid.clone(),
                title: song.title.clone(),
                singers: song.singers.clone(),
                at: history::now_secs(),
                origin: self.queue.origin().to_string(),
            },
            self.config.history_limit,
        );
        if let Err(error) = self.history.save(&self.history_path) {
            tracing::warn!("写历史失败：{error:#}");
        }
        self.refresh_history_view();
    }

    /// 历史屏：新在前；每行右侧写「时间 · 来自《来源》」。
    ///
    /// `history::Entry` 只有 mid / 标题 / 歌手，所以还原出来的 `Song` 没有时长与档位；
    /// 取流只认 mid（`qq::vkey::candidates` 用 mid 拼文件名，`media_mid` 缺失时回落到
    /// mid），因此能直接播——这一点由实测确认（见 README 的已知限制）。
    pub(super) fn refresh_history_view(&mut self) {
        let selected = self.nav.history_view.state.selected();
        self.nav.history_view.items = self
            .history
            .entries()
            .iter()
            .map(|entry| Song {
                mid: entry.mid.clone(),
                song_id: None,
                media_mid: None,
                title: entry.title.clone(),
                singers: entry.singers.clone(),
                album_mid: String::new(),
                album_name: String::new(),
                duration_secs: 0,
                sizes: crate::model::FileSizes::default(),
            })
            .collect();
        self.nav.history_view.labels = self
            .history
            .entries()
            .iter()
            .map(|entry| {
                format!(
                    "{} · 来自《{}》",
                    history::relative(history::now_secs(), entry.at),
                    entry.origin
                )
            })
            .collect();
        self.nav
            .history_view
            .state
            .select(selected.or((!self.history.is_empty()).then_some(0)));
        // 标题带上条数：一眼知道要不要清。
        self.nav.breadcrumb = vec![if self.history.is_empty() {
            "历史".to_string()
        } else {
            format!("历史（{} 条）", self.history.len())
        }];
    }

    /// `Enter`：歌单列表 = 往下进一层；曲目列表 = 以此列表开队列并从该首播。
    fn enter_selected(&mut self) {
        if self.nav.can_enter_depth() {
            let Some(playlist) = self.nav.playlists.selected().cloned() else {
                return;
            };
            self.nav.begin_tracks(&playlist.id, &playlist.name);
            self.load_tracks(playlist.id, playlist.name);
            return;
        }
        self.open_queue_from_focus();
    }

    /// `n` / `b`：按当前模式在队列里前进 / 后退。
    fn play_relative(&mut self, forward: bool) {
        if self.queue.is_empty() {
            self.status = "队列是空的".to_string();
            return;
        }
        self.skipped = 0;
        let step = if forward {
            self.queue.next_manual()
        } else {
            self.queue.prev_manual()
        };
        match step {
            queue::Step::Play(index) => self.start(index),
            queue::Step::Stop => {
                self.status = if forward {
                    "已经是最后一首".to_string()
                } else {
                    "已经是第一首".to_string()
                };
            }
        }
    }

    /// `j`/`k`：焦点在菜单上就是换菜单项（后台菜单的行为），在主区就是挪选中行。
    fn move_selection(&mut self, delta: isize) {
        if self.nav.focus == Focus::Menu {
            let next = (self.nav.menu.index() as isize + delta)
                .clamp(0, Menu::ALL.len() as isize - 1);
            self.switch_menu(Menu::ALL[next as usize]);
            return;
        }
        self.nav.move_focus_by(delta);
    }

    fn select_edge(&mut self, last: bool) {
        if self.nav.focus == Focus::Menu {
            self.switch_menu(if last { Menu::History } else { Menu::Favorites });
            return;
        }
        self.nav.focus_edge(last);
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
            let step = self.queue.advance();
            self.step(step);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::test_support::{test_playlist as playlist, test_song};

    /// 菜单切换是「换一屏内容」，不是「清空一屏内容」：切走再切回，选中位置要还在。
    #[test]
    fn switching_menus_keeps_each_views_selection() {
        let mut nav = Nav::new();
        nav.favorites.items = vec![test_song("a"), test_song("b"), test_song("c")];
        nav.favorites.select_first();
        nav.switch_menu(Menu::Favorites);
        nav.favorites.move_by(2);
        assert_eq!(nav.favorites.selected().map(|s| s.mid.as_str()), Some("c"));

        nav.switch_menu(Menu::Playlists);
        assert_eq!(nav.breadcrumb, ["我的歌单"]);
        nav.switch_menu(Menu::Favorites);
        assert_eq!(
            nav.favorites.selected().map(|s| s.mid.as_str()),
            Some("c"),
            "切回来选中位置该保留"
        );
    }

    #[test]
    fn entering_a_playlist_pushes_a_breadcrumb_and_esc_pops_it() {
        let mut nav = Nav::new();
        nav.switch_menu(Menu::Playlists);
        nav.playlists.items = vec![playlist("1", "嗯"), playlist("2", "循环")];
        nav.playlists.select_first();
        assert!(nav.can_enter_depth(), "歌单列表是最上一层，可以往下进");
        let target = nav.playlists.selected().cloned().expect("选中了歌单");
        assert_eq!(target.id, "1");
        nav.begin_tracks(&target.id, &target.name);
        assert_eq!(nav.breadcrumb, ["我的歌单", "嗯"]);
        assert!(!nav.can_enter_depth(), "已经在曲目层里了");

        nav.push_tracks("1", "嗯", vec![test_song("a")]);
        assert_eq!(nav.tracks.items.len(), 1);
        assert!(nav.pop_tracks(), "Esc 该返回歌单列表");
        assert_eq!(nav.breadcrumb, ["我的歌单"]);
        assert_eq!(nav.tracks.items.len(), 0, "退回上一层要清掉曲目");
        assert!(!nav.pop_tracks(), "已经在最上层，再返回没有动作");
    }

    #[test]
    fn paging_moves_ten_rows_and_stops_at_the_end() {
        let mut nav = Nav::new();
        nav.switch_menu(Menu::Favorites);
        nav.favorites.items = (0..25).map(|index| test_song(&format!("m{index}"))).collect();
        nav.favorites.select_first();
        nav.page_focus(1);
        assert_eq!(nav.favorites.state.selected(), Some(10));
        nav.page_focus(1);
        nav.page_focus(1);
        assert_eq!(nav.favorites.state.selected(), Some(24), "到尾就停，不越界");
        nav.page_focus(-1);
        assert_eq!(nav.favorites.state.selected(), Some(14));
    }

    /// 搜索把结果放在自己的列表里，不会把正在看的歌单曲目冲掉。
    #[test]
    fn search_results_have_their_own_slot() {
        let mut nav = Nav::new();
        nav.switch_menu(Menu::Playlists);
        nav.push_tracks("1", "嗯", vec![test_song("keep")]);
        nav.search_results.items = vec![test_song("found")];
        nav.switch_menu(Menu::Search);
        assert_eq!(nav.focused_songs().unwrap().items[0].mid, "found");
        nav.switch_menu(Menu::Playlists);
        assert_eq!(nav.tracks.items[0].mid, "keep", "歌单曲目没被搜索顶掉");
    }

    /// 搜完看别的屏、再切回搜索：关键词与结果都还在。
    #[test]
    fn search_keeps_its_screen_so_browsing_is_not_lost() {
        let mut nav = Nav::new();
        nav.switch_menu(Menu::Favorites);
        nav.favorites.items = vec![test_song("a"), test_song("b")];
        nav.favorites.select_first();
        nav.favorites.move_by(1);

        nav.begin_search_input();
        nav.search_text = "晴天".to_string();
        nav.search_results.items = vec![test_song("s1"), test_song("s2")];
        nav.search_results.select_first();

        nav.switch_menu(Menu::Favorites);
        assert_eq!(
            nav.favorites.selected().map(|s| s.mid.as_str()),
            Some("b"),
            "浏览上下文没丢"
        );
        nav.switch_menu(Menu::Search);
        assert_eq!(
            nav.search_results.selected().map(|s| s.mid.as_str()),
            Some("s1"),
            "搜索结果也还在"
        );
        assert_eq!(nav.search_text, "晴天");
    }

    /// 进入搜索屏自动聚焦输入框；退出输入态后关键词留在框里。
    #[test]
    fn entering_the_search_screen_focuses_the_box_and_leaving_it_keeps_the_keyword() {
        let mut nav = Nav::new();
        nav.switch_menu(Menu::Search);
        assert!(nav.search_input, "进来就能打字");
        nav.search_text = "晴天".to_string();
        nav.end_search_input();
        assert!(!nav.search_input);
        assert_eq!(nav.search_text, "晴天", "退出输入态不丢关键词");
    }

    /// 焦点在菜单上时，主区列表不该被 `j`/`k` 动到。
    #[test]
    fn movement_only_touches_the_focused_list() {
        let mut nav = Nav::new();
        nav.favorites.items = vec![test_song("a"), test_song("b")];
        nav.favorites.select_first();
        nav.focus = Focus::Menu;
        assert!(nav.focused_songs().is_none());
        nav.focus = Focus::Content;
        nav.move_focus_by(1);
        assert_eq!(nav.favorites.state.selected(), Some(1));
    }

    #[test]
    fn help_overlay_toggles_and_swallows_everything_else() {
        let (mut app, _runtime) = crate::tui::test_support::test_app();
        assert_eq!(app.context(), keys::Context::Menu);
        app.apply(keys::Action::Help);
        assert!(app.help_open);
        assert_eq!(app.context(), keys::Context::HelpOverlay);
        app.apply(keys::Action::Help);
        assert!(!app.help_open);
        assert_eq!(app.context(), keys::Context::Menu, "关掉后回到原来的焦点");
    }

    #[test]
    fn digits_switch_menu_and_move_focus_to_content() {
        let (mut app, _runtime) = crate::tui::test_support::test_app();
        app.apply(keys::Action::Menu(2));
        assert_eq!(app.nav.menu, Menu::Search);
        assert_eq!(app.nav.focus, Focus::Content);
        assert!(app.nav.search_input, "进搜索屏直接能输入");
        app.apply(keys::Action::Menu(1));
        assert_eq!(app.nav.menu, Menu::Playlists);
        assert!(!app.nav.search_input, "离开搜索屏要退出输入态");
    }

    #[test]
    fn p_opens_the_playlist_picker_and_a_stays_unbound_until_the_queue_lands() {
        let (mut app, _runtime) = crate::tui::test_support::test_app();
        app.nav.switch_menu(Menu::Favorites);
        app.nav.favorites.items = vec![test_song("a")];
        app.nav.favorites.select_first();
        app.nav.playlists.items = vec![crate::tui::test_support::test_playlist("1", "嗯")];
        app.apply(keys::Action::AddToPlaylist);
        assert!(app.picker.is_some(), "p 打开歌单选择框");
        assert_eq!(app.context(), keys::Context::PlaylistPicker);
        app.apply(keys::Action::Back);
        assert!(app.picker.is_none(), "Esc 关掉选择框");
    }

    /// `a`：选中那首进队尾（这里直接给一个已有队列，避开「空队列 = 开新队列并播放」那条路）。
    #[test]
    fn appending_puts_the_picked_song_at_the_tail() {
        let (mut app, _runtime) = crate::tui::test_support::test_app();
        app.nav.switch_menu(Menu::Favorites);
        app.nav.favorites.items = vec![test_song("a"), test_song("b")];
        app.nav.favorites.select_first();
        app.queue = queue::Queue::new(Vec::new(), 0, String::new());
        app.queue.push_back(test_song("seed"));
        app.push_to_queue(false);
        assert_eq!(app.queue.len(), 2);
        assert_eq!(app.queue.items()[0].mid, "seed");
        assert_eq!(app.queue.items()[1].mid, "a");
    }

    #[test]
    fn insert_next_lands_right_after_the_current_song() {
        let (mut app, _runtime) = crate::tui::test_support::test_app();
        app.nav.switch_menu(Menu::Favorites);
        app.nav.favorites.items = vec![test_song("c")];
        app.nav.favorites.select_first();
        app.queue = queue::Queue::new(vec![test_song("a"), test_song("b")], 0, "嗯");
        app.push_to_queue(true);
        assert_eq!(app.queue.items()[1].mid, "c", "A 插到当前项后面");
        assert_eq!(app.queue.index(), 0, "插队不动当前项");
        assert!(app.status.contains("已插队"), "状态行要说明做了什么：{}", app.status);
    }

    /// 删队列里「不是当前项」的那首：队列短一格，播放不动（也不去取流）。
    #[test]
    fn deleting_a_queued_song_that_is_not_playing_keeps_the_current_one() {
        let (mut app, _runtime) = crate::tui::test_support::test_app();
        app.queue = queue::Queue::new(
            vec![test_song("a"), test_song("b"), test_song("c")],
            0,
            "嗯",
        );
        app.queue_state.select(Some(2));
        app.delete_queue_item();
        assert_eq!(app.queue.len(), 2);
        assert_eq!(app.queue.items()[app.queue.index()].mid, "a", "当前这首没被换掉");
        assert_eq!(app.queue_state.selected(), Some(1), "高亮跟着收一格");
    }

    #[test]
    fn clearing_the_queue_keeps_playing_but_forgets_the_rest() {
        let (mut app, _runtime) = crate::tui::test_support::test_app();
        app.queue = queue::Queue::new(vec![test_song("a"), test_song("b")], 0, "嗯");
        app.apply(keys::Action::QueueClear);
        assert!(app.queue.is_empty());
        assert!(app.status.contains("已清空"), "{}", app.status);
    }

    #[test]
    fn the_queue_overlay_opens_on_the_current_song_and_closes_with_escape() {
        let (mut app, _runtime) = crate::tui::test_support::test_app();
        app.queue = queue::Queue::new(vec![test_song("a"), test_song("b")], 1, "嗯");
        app.apply(keys::Action::QueueOverlay);
        assert!(app.queue_open);
        assert_eq!(app.context(), keys::Context::QueueOverlay);
        assert_eq!(app.queue_state.selected(), Some(1), "高亮落在正在播的那首上");
        app.apply(keys::Action::Back);
        assert!(!app.queue_open);
    }

    #[test]
    fn an_empty_queue_does_not_open_the_overlay() {
        let (mut app, _runtime) = crate::tui::test_support::test_app();
        app.apply(keys::Action::QueueOverlay);
        assert!(!app.queue_open);
        assert_eq!(app.status, "队列是空的");
    }

    #[test]
    fn cycling_the_mode_walks_the_four_modes_and_says_so() {
        let (mut app, _runtime) = crate::tui::test_support::test_app();
        for expected in ["列表循环", "单曲", "随机", "顺序"] {
            app.apply(keys::Action::CycleMode);
            assert_eq!(app.queue.mode().label(), expected);
            assert!(app.status.contains(expected), "{}", app.status);
        }
    }

    #[test]
    fn starting_a_song_records_history_once_and_refreshes_the_view() {
        let (mut app, _runtime) = crate::tui::test_support::test_app();
        let song = test_song("a");
        app.queue = queue::Queue::new(vec![song.clone()], 0, "嗯");
        app.record_history(&song);
        app.record_history(&song);
        assert_eq!(app.history.len(), 1, "同一首连着播只记一条");
        assert_eq!(app.nav.history_view.items.len(), 1, "历史屏立刻能看到");
        assert_eq!(app.nav.history_view.items[0].mid, "a");
        assert!(
            app.nav.history_view.labels[0].contains("来自《嗯》"),
            "副标题要写来源：{:?}",
            app.nav.history_view.labels
        );
    }

    #[test]
    fn the_history_screen_plays_from_the_row_the_user_picked() {
        let (mut app, _runtime) = crate::tui::test_support::test_app();
        for mid in ["a", "b", "c"] {
            app.record_history(&test_song(mid));
        }
        app.switch_menu(Menu::History);
        app.nav.history_view.state.select(Some(1));

        let (songs, index, origin) = app.queue_from_focus().expect("历史屏有内容");
        assert_eq!(songs.len(), 3);
        assert_eq!(songs[index].mid, "b");
        assert_eq!(origin, "历史");
        assert!(app.nav.breadcrumb[0].contains("3 条"), "标题带条数：{:?}", app.nav.breadcrumb);
    }

    #[test]
    fn clearing_history_empties_the_view_and_the_file() {
        let (mut app, _runtime) = crate::tui::test_support::test_app();
        app.record_history(&test_song("a"));
        assert!(app.history_path.exists(), "记一条就该落盘");
        app.switch_menu(Menu::History);
        app.apply(keys::Action::HistoryClear);
        assert!(app.history.is_empty());
        assert!(app.nav.history_view.items.is_empty());
        assert_eq!(app.status, "已清空播放历史");
        assert!(!app.history_path.exists(), "磁盘上的也清了");
    }

    /// 历史屏之外按 `Shift+D` 不该动历史（它只在历史屏有意义）。
    #[test]
    fn shift_d_outside_the_history_screen_leaves_history_alone() {
        let (mut app, _runtime) = crate::tui::test_support::test_app();
        app.record_history(&test_song("a"));
        app.nav.switch_menu(Menu::Favorites);
        app.apply(keys::Action::HistoryClear);
        assert_eq!(app.history.len(), 1);
    }
}

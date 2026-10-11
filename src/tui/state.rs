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
use crate::session;
use crate::history::{self, History};
use crate::download;
use crate::model::{Lyric, Playlist, Song};
use crate::player::{Engine, Track};
use crate::qq;
use crate::paths;
use crate::qq::vkey::Candidate;
use crate::qr;

use super::keys;
use super::login::{self, LoginPhase, LoginState};
use super::queue;

/// 连续遇到不可播曲目时最多自动跳几首——整个歌单都没版权时别一路刷到底。
const MAX_AUTO_SKIPS: u32 = 3;

/// 把 `state` 的选择位置挪 `delta` 格（越界夹住，空列表不动）。
fn step_state(state: &mut ListState, len: usize, delta: isize) {
    if len == 0 {
        return;
    }
    let current = state.selected().unwrap_or(0) as isize;
    state.select(Some((current + delta).clamp(0, len as isize - 1) as usize));
}

/// 跳到首 / 尾。
fn edge_state(state: &mut ListState, len: usize, last: bool) {
    if len > 0 {
        state.select(Some(if last { len - 1 } else { 0 }));
    }
}

/// 当前在哪一屏。启动时看会话：没登录就先出登录屏。
pub(super) enum Screen {
    Login(LoginState),
    Main,
}

/// 后台任务的回话。
pub(super) enum Msg {
    /// 拿到一张二维码（`attempt` 是第几张）。
    Qr { png: Vec<u8>, attempt: u32 },
    /// 已扫码，等手机上确认。
    QrScanned,
    /// 扫码登录成功（`identity` 是账号）。
    LoggedIn(String),
    /// 登录流程失败（取码失败 / 多次过期 / 服务端拒绝）。
    LoginFailed(String),
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

/// 四个主界面屏（原左栏菜单项）。顺序即数字键 `1`–`4` 的顺序。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Menu {
    Favorites,
    Playlists,
    Search,
    History,
}

impl Menu {
    pub const ALL: [Menu; 4] = [Menu::Favorites, Menu::Playlists, Menu::Search, Menu::History];

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
///
/// 「筛选」是这一层自带的：`items` 永远是**全量**数据，`visible` 是当前可见项在
/// `items` 里的下标。所有对外行为（选中哪一条、画哪几行、回车播哪一队）都走
/// `visible`，所以筛选只影响「看得到什么」，不会破坏原始数据。
///
/// 高亮的 `state.selected()` 是**可见行里的位置**，不是 `items` 的下标——
/// 要拿原始下标用 [`ListView::selected_index`]。
#[derive(Debug)]
pub(super) struct ListView<T> {
    items: Vec<T>,
    pub state: ListState,
    /// 这一屏自己的错误（别的屏没事）。
    pub(super) error: Option<String>,
    /// 每行的右侧副标题（历史屏放「时间 · 来自《来源》」，别的屏留空）。
    /// 与 `items` 等长，改 `items` 的代码要顺手改它。
    labels: Vec<String>,
    /// 筛选关键词（按用户敲的原样存，用来显示）。
    filter: String,
    /// 这一屏的哪些字段参与筛选（曲目看标题/歌手，歌单看名字）。
    matcher: fn(&T, &str) -> bool,
    /// 当前可见项在 `items` 里的下标。`items` 或筛选词一变就重算（`refresh_visible`）。
    visible: Vec<usize>,
}

/// 曲目按「标题 / 歌手」筛（两边都转小写，大小写不敏感）。
fn matches_song(song: &Song, needle: &str) -> bool {
    song.title.to_lowercase().contains(needle) || song.singers.to_lowercase().contains(needle)
}

/// 歌单按名字筛。
fn matches_playlist(playlist: &Playlist, needle: &str) -> bool {
    playlist.name.to_lowercase().contains(needle)
}

impl ListView<Song> {
    pub(super) fn songs() -> Self {
        Self::with_matcher(matches_song)
    }
}

impl ListView<Playlist> {
    pub(super) fn playlists() -> Self {
        Self::with_matcher(matches_playlist)
    }
}

impl<T> ListView<T> {
    fn with_matcher(matcher: fn(&T, &str) -> bool) -> Self {
        Self {
            items: Vec::new(),
            state: ListState::default(),
            error: None,
            labels: Vec::new(),
            filter: String::new(),
            matcher,
            visible: Vec::new(),
        }
    }

    /// 换一批数据：清掉筛选，选中回到第一行。
    pub(super) fn set_items(&mut self, items: Vec<T>) {
        self.labels.clear();
        self.filter.clear();
        self.items = items;
        self.refresh_visible(false);
    }

    /// 换一批带右侧副标题的数据（历史屏）。
    pub(super) fn set_items_with_labels(&mut self, items: Vec<T>, labels: Vec<String>) {
        debug_assert_eq!(items.len(), labels.len(), "副标题要与条目一一对应");
        self.items = items;
        self.labels = labels;
        self.refresh_visible(false);
    }

    /// 全量数据（不受筛选影响）。
    pub(super) fn items(&self) -> &[T] {
        &self.items
    }

    /// 当前可见的条目。
    pub(super) fn visible_items(&self) -> impl Iterator<Item = &T> {
        self.visible.iter().filter_map(|index| self.items.get(*index))
    }

    /// 当前可见的行：条目 + 右侧副标题（没有副标题就是空串）。
    pub(super) fn visible_rows(&self) -> impl Iterator<Item = (&T, &str)> {
        self.visible.iter().filter_map(|index| {
            let item = self.items.get(*index)?;
            let label = self.labels.get(*index).map_or("", String::as_str);
            Some((item, label))
        })
    }

    /// 可见条数（筛选后）。
    pub(super) fn len(&self) -> usize {
        self.visible.len()
    }

    /// 可见项是否为空——「这一屏有没有东西可看」看它。
    pub(super) fn is_empty(&self) -> bool {
        self.visible.is_empty()
    }

    pub(super) fn selected(&self) -> Option<&T> {
        self.visible
            .get(self.state.selected()?)
            .and_then(|index| self.items.get(*index))
    }

    /// 选中项在 `items`（全量）里的下标。
    pub(super) fn selected_index(&self) -> Option<usize> {
        self.visible.get(self.state.selected()?).copied()
    }

    pub(super) fn select_first(&mut self) {
        self.state.select((!self.visible.is_empty()).then_some(0));
    }

    pub(super) fn move_by(&mut self, delta: isize) {
        if self.visible.is_empty() {
            return;
        }
        let current = self.state.selected().unwrap_or(0) as isize;
        let next = (current + delta).clamp(0, self.visible.len() as isize - 1);
        self.state.select(Some(next as usize));
    }

    pub(super) fn edge(&mut self, last: bool) {
        if !self.visible.is_empty() {
            self.state
                .select(Some(if last { self.visible.len() - 1 } else { 0 }));
        }
    }

    /// 当前筛选词。
    pub(super) fn filter_text(&self) -> &str {
        &self.filter
    }

    /// 换筛选词并重算可见项（输入即筛）。
    pub(super) fn set_filter(&mut self, keyword: &str) {
        if self.filter == keyword {
            return;
        }
        self.filter = keyword.to_string();
        self.refresh_visible(true);
    }

    /// 按当前筛选词重算可见项。`items` 或筛选词改了就**必须**走这里，
    /// 否则高亮会和画出来的行对不上。
    ///
    /// `keep` = 尽量把高亮留在原来那一条上（输入时越筛越窄，不该把光标弹回第一行）；
    /// 它已经被筛掉了就退回第一行。
    fn refresh_visible(&mut self, keep: bool) {
        let previous = keep.then(|| self.selected_index()).flatten();
        let needle = self.filter.to_lowercase();
        self.visible = if needle.is_empty() {
            (0..self.items().len()).collect()
        } else {
            (0..self.items().len())
                .filter(|index| (self.matcher)(&self.items[*index], &needle))
                .collect()
        };
        match previous.and_then(|index| self.visible.iter().position(|it| *it == index)) {
            Some(position) => self.state.select(Some(position)),
            None => self.select_first(),
        }
    }
}

/// 界面导航状态：正在看哪一屏、四屏各自的列表与选中位置。
///
/// 每个菜单项各存一份列表，所以「切走再切回」不会丢上下文——这正是旧界面
/// 「搜一下歌单就没了」的修法。切屏只有数字键 `1`–`4` 一个入口（菜单栏已经去掉），
/// 所以也没有「菜单游标」这种「高亮了但还没进去」的中间态。
#[derive(Debug)]
pub(super) struct Nav {
    /// 正在显示的那一屏。所有按屏分派的东西（列表、面包屑、来源名）都看它。
    pub menu: Menu,
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
            favorites: ListView::songs(),
            playlists: ListView::playlists(),
            tracks: ListView::songs(),
            search_results: ListView::songs(),
            history_view: ListView::songs(),
            shown: None,
            breadcrumb: vec!["我的收藏".to_string()],
            favorites_loaded: false,
            playlists_loaded: false,
            search_text: String::new(),
            search_input: false,
        }
    }

    /// 切到某一屏：内容换屏，但每屏自己的选中位置都留着（「换一屏内容，不是清空一屏内容」）。
    ///
    /// 「我的歌单」这一屏可能还停在某个歌单的曲目里（`shown`），这时面包屑也要跟着
    /// 还原成「我的歌单 › 嗯」——否则标题说「我的歌单」、内容却是曲目，对不上。
    /// 数字键切屏是**回到上次的位置**，退层交给 `Esc`。
    pub fn switch_menu(&mut self, menu: Menu) {
        self.menu = menu;
        self.breadcrumb = match (menu, &self.shown) {
            (Menu::Playlists, Some((_, name))) => vec!["我的歌单".to_string(), name.clone()],
            _ => vec![menu.title().to_string()],
        };
        // 搜索屏进来就该能打字，不用先按一下别的键。
        self.search_input = menu == Menu::Search;
    }

    /// `/`：跳到搜索屏并聚焦输入框，任何位置都能用。
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
        self.tracks = ListView::songs();
        self.tracks.set_items(songs);
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
        self.tracks = ListView::songs();
        self.breadcrumb = vec!["我的歌单".to_string()];
        true
    }

    /// 当前这一屏的筛选词（歌单列表在最上层时就是它自己的）。
    pub fn filter_text(&self) -> &str {
        match self.menu {
            Menu::Playlists if self.shown.is_none() => self.playlists.filter_text(),
            _ => self.song_view().filter_text(),
        }
    }

    /// 换筛选词（输入即筛）。
    pub fn set_filter(&mut self, keyword: &str) {
        match self.menu {
            Menu::Playlists if self.shown.is_none() => self.playlists.set_filter(keyword),
            _ => self.song_view_mut().set_filter(keyword),
        }
    }

    /// 当前列表的（命中数, 总数）——筛选时才画进标题。
    pub fn filter_counts(&self) -> (usize, usize) {
        match self.menu {
            Menu::Playlists if self.shown.is_none() => {
                (self.playlists.len(), self.playlists.items().len())
            }
            _ => {
                let view = self.song_view();
                (view.len(), view.items().len())
            }
        }
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

    /// 当前屏「Enter / a / f / d / 筛选 作用的那份曲目列表」。
    ///
    /// 歌单屏不在某个歌单里时没有曲目列表（那一屏显示的是歌单列表），返回 `None`。
    pub fn active_songs(&self) -> Option<&ListView<Song>> {
        match self.menu {
            Menu::Favorites => Some(&self.favorites),
            Menu::Playlists => self.shown.as_ref().map(|_| &self.tracks),
            Menu::Search => Some(&self.search_results),
            Menu::History => Some(&self.history_view),
        }
    }

    /// 上面那份列表的可变版本。
    pub fn active_songs_mut(&mut self) -> Option<&mut ListView<Song>> {
        match self.menu {
            Menu::Favorites => Some(&mut self.favorites),
            Menu::Playlists => self.shown.as_ref().map(|_| &mut self.tracks),
            Menu::Search => Some(&mut self.search_results),
            Menu::History => Some(&mut self.history_view),
        }
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.menu == Menu::Playlists && self.shown.is_none() {
            self.playlists.move_by(delta);
            return;
        }
        if let Some(view) = self.active_songs_mut() {
            view.move_by(delta);
        }
    }

    pub fn edge_selection(&mut self, last: bool) {
        if self.menu == Menu::Playlists && self.shown.is_none() {
            self.playlists.edge(last);
            return;
        }
        if let Some(view) = self.active_songs_mut() {
            view.edge(last);
        }
    }

    pub fn page_selection(&mut self, pages: isize) {
        // 一屏按 10 行挪：终端高度可变，10 行在常见窗口里都不到一屏，够用且不越界。
        self.move_selection(pages * 10);
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
    /// 当前屏幕。
    pub(super) screen: Screen,
    /// 播放历史（内存里这份是权威，落盘在每次追加后同步）。
    pub(super) history: History,
    /// 历史文件路径（测试注入临时路径用）。
    pub(super) history_path: std::path::PathBuf,

    /// 正在看哪一屏、四屏各自的列表与选中位置。
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
    /// `Y`：全屏歌词（内容区整块让给歌词，播放条与状态行照旧）。
    pub(super) lyric_full: bool,
    /// `/` 的筛选框开着（输入即筛当前列表）。
    pub(super) filter_input: bool,
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
            // 默认主界面；启动时由 `tui::run` 按会话状态决定，测试里也就不用管登录。
            screen: Screen::Main,
            history: History::default(),
            history_path,
            nav: Nav::new(),
            queue: queue::Queue::new(Vec::new(), 0, String::new()),
            request: 0,
            skipped: 0,
            lyric: None,
            lyric_loading: false,
            show_lyric: true,
            lyric_full: false,
            filter_input: false,
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

    /// 启动：停在「我的收藏」，并把它拉下来。
    pub(super) fn load_initial(&mut self) {
        self.switch_menu(Menu::Favorites);
    }

    /// 切屏。这一屏没拉过数据就顺手拉一次——每屏的选中位置由 `Nav` 留着，
    /// 所以来回切不会丢上下文。
    pub(super) fn switch_menu(&mut self, menu: Menu) {
        // 切屏就是离开这一屏：全屏歌词跟着收起来。
        self.lyric_full = false;
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
        let lyrics_dir = self.config.lyrics_dir.clone();
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
                    match download::save(
                        &song,
                        &candidates,
                        &dir,
                        &lyrics_dir,
                        lyric.as_ref(),
                        &mut on_progress,
                    ) {
                        Ok(saved) if saved.existed => {
                            format!("已存在：{}", saved.path.display())
                        }
                        Ok(saved) => format!(
                            "已下载：{}（{:.1} MB，{}{}）",
                            saved.path.display(),
                            saved.bytes as f64 / 1_048_576.0,
                            saved.quality.label(),
                            // 歌词落在别的目录里，说清楚落在哪，省得去翻。
                            if saved.lyrics.is_empty() {
                                String::new()
                            } else {
                                format!("，歌词在 {}", lyrics_dir.display())
                            }
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

    /// 当前屏曲目列表里选中的那首。
    fn selected_song(&self) -> Option<Song> {
        self.nav
            .active_songs()
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

    /// 当前屏的曲目列表 → `(曲目, 选中下标, 来源名)`。`Enter` 与 `a`/`A`（空队列时）都走这里。
    fn queue_from_focus(&self) -> Option<(Vec<Song>, usize, String)> {
        let view = self.nav.active_songs()?;
        // 灌的是**当前看得见**的那几首：筛过之后播的就是筛出来的子集。
        if view.is_empty() {
            return None;
        }
        Some((
            view.visible_items().cloned().collect(),
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
            Msg::Qr { png, attempt } => {
                // PNG 一律留一份：终端画不下、想用相册扫，都靠它。
                let saved = qr::save_png(&png, &paths::qr_png_path());
                let lines = qr::to_lines(&png).unwrap_or_default();
                let size = qr::dimensions(&png).unwrap_or((0, 0));
                if let Screen::Login(state) = &mut self.screen {
                    state.lines = lines;
                    state.qr_size = size;
                    state.attempt = attempt;
                    state.phase = LoginPhase::Ready;
                    state.message = format!("等待扫码…（第 {attempt} 张）");
                    if let Err(error) = saved {
                        state.phase = LoginPhase::Failed(format!("{error:#}"));
                    }
                }
            }
            Msg::QrScanned => {
                if let Screen::Login(state) = &mut self.screen {
                    state.phase = LoginPhase::Scanned;
                    state.message = "已扫码，请在手机上确认…".to_string();
                }
            }
            Msg::LoggedIn(identity) => {
                // 扫码成功后 cookie 已经在 jar 里，落盘才算登录完成。
                match session::jar().save() {
                    Ok(()) => self.finish_login(identity),
                    Err(error) => {
                        if let Screen::Login(state) = &mut self.screen {
                            state.phase = LoginPhase::Failed(format!("保存会话失败：{error:#}"));
                        }
                    }
                }
            }
            Msg::LoginFailed(message) => {
                if let Screen::Login(state) = &mut self.screen {
                    state.phase = LoginPhase::Failed(message);
                }
            }
            Msg::Favorites(result) => {
                self.loading = None;
                self.nav.favorites_loaded = true;
                match result {
                    Ok(songs) => {
                        self.status = format!("我喜欢：{} 首", songs.len());
                        self.nav.favorites.set_items(songs);
                    }
                    Err(error) => {
                        self.nav.favorites.error = Some(format!("{error:#}"));
                        self.note_error(&error);
                    }
                }
            }
            Msg::Playlists(result) => {
                self.loading = None;
                self.nav.playlists_loaded = true;
                match result {
                    Ok(mut playlists) => {
                        // 「我喜欢」在菜单里是单独一项（`f` 写它），不在「我的歌单」里重复出现。
                        playlists.retain(|playlist| !playlist.is_favorites);
                        self.status = format!("共 {} 个歌单", playlists.len());
                        self.nav.playlists.set_items(playlists);
                    }
                    Err(error) => {
                        self.nav.playlists.error = Some(format!("{error:#}"));
                        self.note_error(&error);
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
                    Err(error) => self.note_error(&error),
                }
            }
            Msg::Search { keyword, result } => {
                self.loading = None;
                match result {
                    Ok(songs) => {
                        self.status = format!("搜索「{keyword}」：{} 首", songs.len());
                        self.nav.search_results.set_items(songs);
                    }
                    Err(error) => {
                        self.nav.search_results.error = Some(format!("{error:#}"));
                        self.note_error(&error);
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

    /// 按键入口：先问上下文，再交给纯函数映射表，最后按屏幕派发。
    pub(super) fn handle_key(&mut self, key: KeyEvent) {
        let Some(action) = keys::resolve(self.context(), key) else {
            return;
        };
        match self.screen {
            Screen::Login(_) => self.apply_login(action),
            Screen::Main => self.apply(action),
        }
    }

    /// 当前按键上下文：模态 > 输入态 > 焦点所在。
    pub(super) fn context(&self) -> keys::Context {
        if let Screen::Login(state) = &self.screen {
            return login::context(state.input.is_some());
        }
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
        // 全屏歌词盖住内容区：列表看不见，就别让列表类按键生效。
        if self.lyric_full {
            return keys::Context::LyricFull;
        }
        // 筛选框跟搜索框一样是文本输入，只是 Enter / Esc 落点不同。
        if self.filter_input {
            return keys::Context::FilterInput;
        }
        if self.nav.search_input {
            return keys::Context::SearchInput;
        }
        keys::Context::Content
    }

    pub(super) fn apply(&mut self, action: keys::Action) {
        match action {
            keys::Action::Quit => self.quit = true,
            keys::Action::Help => self.help_open = !self.help_open,
            keys::Action::QueueOverlay => self.toggle_queue_overlay(),
            keys::Action::Logout => self.sign_out("已登出，请重新登录"),
            keys::Action::HistoryClear => {
                // 只在历史屏有意义；别的屏按了不做事（Esc 在没得返回时也一样）。
                if self.nav.menu == Menu::History {
                    match self.history.clear(&self.history_path) {
                        Ok(()) => {
                            self.refresh_history_view();
                            self.status = "已清空播放历史".to_string();
                        }
                        Err(error) => self.note_error(&error),
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
            keys::Action::PageUp => self.nav.page_selection(-1),
            keys::Action::PageDown => self.nav.page_selection(1),
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
            keys::Action::Filter => {
                // `/` 只筛当前列表；顺手把搜索屏的输入态让出去，免得两个输入框打架。
                self.nav.search_input = false;
                self.filter_input = true;
            }
            keys::Action::FilterChar(character) => {
                let mut keyword = self.nav.filter_text().to_string();
                keyword.push(character);
                self.nav.set_filter(&keyword);
            }
            keys::Action::FilterBackspace => {
                let mut keyword = self.nav.filter_text().to_string();
                keyword.pop();
                self.nav.set_filter(&keyword);
            }
            // 回车只是收起输入框：筛出来的子集留着，接着就能 `Enter` 播这一队。
            keys::Action::FilterSubmit => self.filter_input = false,
            keys::Action::FilterCancel => {
                self.filter_input = false;
                self.nav.set_filter("");
            }
            keys::Action::Download => self.download_selected(),
            keys::Action::Favorite => self.favorite_selected(),
            keys::Action::AddToPlaylist => self.open_picker(),
            keys::Action::RemoveFromPlaylist => self.remove_selected(),
            keys::Action::ToggleLyric => self.show_lyric = !self.show_lyric,
            keys::Action::ToggleLyricFull => self.lyric_full = !self.lyric_full,
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
            // 登录屏那三个动作由 `apply_login` 处理，不会走到这里。
            keys::Action::LoginCookie
            | keys::Action::LoginOpenPng
            | keys::Action::LoginRetry => {}
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
                } else if self.lyric_full {
                    // 全屏歌词是一层「盖在内容区上的视图」，Esc 先退出它。
                    self.lyric_full = false;
                } else if !self.nav.filter_text().is_empty() {
                    // 筛选也算一层：Esc 先清掉它，再按才退上一层视图。
                    self.nav.set_filter("");
                } else {
                    self.nav.pop_tracks();
                }
            }
        }
    }

    /// 进登录屏并开始扫码流程。
    pub(super) fn start_login(&mut self) {
        self.screen = Screen::Login(LoginState::default());
        login::spawn_qr_login(&self.handle, self.tx.clone());
    }

    /// 登录屏的按键。
    fn apply_login(&mut self, action: keys::Action) {
        match action {
            keys::Action::Quit => self.quit = true,
            keys::Action::LoginRetry => self.start_login(),
            keys::Action::LoginOpenPng => {
                if let (Err(error), Screen::Login(state)) = (login::open_qr_png(), &mut self.screen) {
                    state.phase = LoginPhase::Failed(error);
                }
            }
            keys::Action::LoginCookie => {
                if let Screen::Login(state) = &mut self.screen {
                    state.input = Some(String::new());
                }
            }
            keys::Action::InputChar(character) => {
                if let Screen::Login(state) = &mut self.screen
                    && let Some(buffer) = state.input.as_mut()
                {
                    buffer.push(character);
                }
            }
            keys::Action::InputBackspace => {
                if let Screen::Login(state) = &mut self.screen
                    && let Some(buffer) = state.input.as_mut()
                {
                    buffer.pop();
                }
            }
            keys::Action::InputCancel => {
                if let Screen::Login(state) = &mut self.screen {
                    state.input = None;
                }
            }
            keys::Action::InputSubmit => {
                let raw = match &mut self.screen {
                    Screen::Login(state) => state.input.take().unwrap_or_default(),
                    Screen::Main => return,
                };
                match login::login_with_cookie(&raw) {
                    Ok(identity) => self.finish_login(identity),
                    Err(error) => {
                        if let Screen::Login(state) = &mut self.screen {
                            state.phase = LoginPhase::Failed(error);
                        }
                    }
                }
            }
            // 登录屏只会产出上面这些动作（`Context::Login` / `SearchInput`）。
            _ => {}
        }
    }

    /// 登录成功：进主界面，从「我的收藏」开始。
    fn finish_login(&mut self, identity: String) {
        tracing::info!("登录成功：{identity}");
        self.screen = Screen::Main;
        self.status = format!("已登录：{identity}");
        self.switch_menu(Menu::Favorites);
    }

    /// 统一处理失败：会话失效就切回登录屏，其余写状态行。
    ///
    /// 判定点在 `qq::read_json`（所有接口共用的出口），这里只负责认这个错误类型。
    fn note_error(&mut self, error: &anyhow::Error) {
        if error.downcast_ref::<session::SessionExpired>().is_some() {
            self.sign_out("会话已过期，请重新登录");
            return;
        }
        self.status = format!("{error:#}");
    }

    /// 会话失效或被手动登出：清掉本地会话、回登录屏。
    pub(super) fn sign_out(&mut self, reason: &str) {
        if let Err(error) = session::jar().clear() {
            tracing::warn!("清本地会话失败：{error:#}");
        }
        self.queue.clear();
        self.start_login();
        if let Screen::Login(state) = &mut self.screen {
            state.phase = LoginPhase::Failed(reason.to_string());
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
        let previous = self.nav.history_view.state.selected();
        let songs: Vec<Song> = self
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
        let labels: Vec<String> = self
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
        let view = &mut self.nav.history_view;
        view.set_items_with_labels(songs, labels);
        // 重建之后把高亮放回原来那一行（比如刚从历史里删了一条，索引要夹住）。
        if let Some(index) = previous.filter(|index| *index < view.len()) {
            view.state.select(Some(index));
        }
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

    /// `j`/`k` 的落点：浮层开着就挪浮层自己的列表，否则挪当前屏的列表。
    ///
    /// 队列浮层与歌单选择框各有一份 `ListState`——它们盖在主区上，方向键绝不该
    /// 穿透到底下的列表去（那正是「浮层里选不动」这个 bug 的成因）。
    fn move_selection(&mut self, delta: isize) {
        if self.queue_open {
            step_state(&mut self.queue_state, self.queue.len(), delta);
            return;
        }
        if let Some((playlists, state, _)) = self.picker.as_mut() {
            step_state(state, playlists.len(), delta);
            return;
        }
        self.nav.move_selection(delta);
    }

    /// `g`/`G`（Home/End）：落点同上，跳到首 / 尾。
    fn select_edge(&mut self, last: bool) {
        if self.queue_open {
            edge_state(&mut self.queue_state, self.queue.len(), last);
            return;
        }
        if let Some((playlists, state, _)) = self.picker.as_mut() {
            edge_state(state, playlists.len(), last);
            return;
        }
        self.nav.edge_selection(last);
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
    use crate::tui::test_support::{test_app, test_playlist as playlist, test_song, titled_song};

    /// 菜单切换是「换一屏内容」，不是「清空一屏内容」：切走再切回，选中位置要还在。
    #[test]
    fn switching_menus_keeps_each_views_selection() {
        let mut nav = Nav::new();
        nav.favorites.set_items(vec![test_song("a"), test_song("b"), test_song("c")]);
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
        nav.playlists.set_items(vec![playlist("1", "嗯"), playlist("2", "循环")]);
        nav.playlists.select_first();
        assert!(nav.can_enter_depth(), "歌单列表是最上一层，可以往下进");
        let target = nav.playlists.selected().cloned().expect("选中了歌单");
        assert_eq!(target.id, "1");
        nav.begin_tracks(&target.id, &target.name);
        assert_eq!(nav.breadcrumb, ["我的歌单", "嗯"]);
        assert!(!nav.can_enter_depth(), "已经在曲目层里了");

        nav.push_tracks("1", "嗯", vec![test_song("a")]);
        assert_eq!(nav.tracks.items().len(), 1);
        assert!(nav.pop_tracks(), "Esc 该返回歌单列表");
        assert_eq!(nav.breadcrumb, ["我的歌单"]);
        assert_eq!(nav.tracks.items().len(), 0, "退回上一层要清掉曲目");
        assert!(!nav.pop_tracks(), "已经在最上层，再返回没有动作");
    }

    #[test]
    fn paging_moves_ten_rows_and_stops_at_the_end() {
        let mut nav = Nav::new();
        nav.switch_menu(Menu::Favorites);
        nav.favorites.set_items((0..25).map(|index| test_song(&format!("m{index}"))).collect());
        nav.favorites.select_first();
        nav.page_selection(1);
        assert_eq!(nav.favorites.state.selected(), Some(10));
        nav.page_selection(1);
        nav.page_selection(1);
        assert_eq!(nav.favorites.state.selected(), Some(24), "到尾就停，不越界");
        nav.page_selection(-1);
        assert_eq!(nav.favorites.state.selected(), Some(14));
    }

    /// 搜索把结果放在自己的列表里，不会把正在看的歌单曲目冲掉。
    #[test]
    fn search_results_have_their_own_slot() {
        let mut nav = Nav::new();
        nav.switch_menu(Menu::Playlists);
        nav.push_tracks("1", "嗯", vec![test_song("keep")]);
        nav.search_results.set_items(vec![test_song("found")]);
        nav.switch_menu(Menu::Search);
        assert_eq!(nav.active_songs().unwrap().items()[0].mid, "found");
        nav.switch_menu(Menu::Playlists);
        assert_eq!(nav.tracks.items()[0].mid, "keep", "歌单曲目没被搜索顶掉");
    }

    /// `/` 筛当前列表：只剩命中的，回车播的就是这一队；Esc 清掉。
    #[test]
    fn filtering_narrows_the_list_and_what_enter_plays() {
        let (mut app, _runtime) = test_app();
        app.nav.switch_menu(Menu::Favorites);
        app.nav.favorites.set_items(vec![
            titled_song("m1", "晴天", "周杰伦"),
            titled_song("m2", "非酋", "薛明媛"),
            titled_song("m3", "晴天娃娃", "某某"),
        ]);
        app.apply(keys::Action::Filter);
        assert_eq!(app.context(), keys::Context::FilterInput, "`/` 打开筛选框");
        assert_eq!(app.nav.menu, Menu::Favorites, "`/` 不再跳去搜索屏");
        for character in "晴天".chars() {
            app.apply(keys::Action::FilterChar(character));
        }
        assert_eq!(app.nav.filter_counts(), (2, 3), "标题要报命中 2/3");
        app.apply(keys::Action::FilterSubmit);
        assert_eq!(app.context(), keys::Context::Content, "回车收起筛选框");

        app.nav.favorites.move_by(1);
        let (songs, index, _) = app.queue_from_focus().expect("筛出来两首，可播");
        assert_eq!(
            songs
                .iter()
                .map(|song| song.mid.as_str())
                .collect::<Vec<_>>(),
            ["m1", "m3"],
            "队列只装筛出来的那几首"
        );
        assert_eq!(index, 1, "播的是高亮那首（子集里的第 2 首）");

        app.apply(keys::Action::Filter);
        app.apply(keys::Action::FilterCancel);
        assert_eq!(app.nav.filter_counts(), (3, 3), "Esc 清掉筛选");
        assert_eq!(app.context(), keys::Context::Content);
    }

    /// 歌手名也该命中（不只是标题），并且大小写不敏感。
    #[test]
    fn filtering_matches_the_singer_too() {
        let mut nav = Nav::new();
        nav.favorites.set_items(vec![
            titled_song("m1", "Sunny", "周杰伦"),
            titled_song("m2", "非酋", "薛明媛"),
        ]);
        nav.switch_menu(Menu::Favorites);
        nav.set_filter("薛明媛");
        assert_eq!(nav.favorites.selected().map(|s| s.mid.as_str()), Some("m2"));
        nav.set_filter("sun");
        assert_eq!(
            nav.favorites.selected().map(|s| s.mid.as_str()),
            Some("m1"),
            "标题匹配大小写不敏感"
        );
        nav.set_filter("没人叫这名字");
        assert_eq!(nav.filter_counts(), (0, 2), "全不命中就是 0 条");
        assert!(nav.favorites.selected().is_none(), "空结果没有可选项");
    }

    /// 越筛越窄时高亮跟着原来那一条走；它被筛掉了才回第一行。
    #[test]
    fn narrowing_keeps_the_highlight_while_it_still_matches() {
        let mut nav = Nav::new();
        nav.favorites.set_items(vec![
            titled_song("m1", "晴天", "周杰伦"),
            titled_song("m2", "晴天娃娃", "某某"),
            titled_song("m3", "非酋", "薛明媛"),
        ]);
        nav.switch_menu(Menu::Favorites);
        nav.favorites.move_by(1);
        nav.set_filter("晴天");
        assert_eq!(
            nav.favorites.selected().map(|s| s.mid.as_str()),
            Some("m2"),
            "原来那条还在筛出来的集合里，光标不该弹回第一行"
        );
        nav.set_filter("非酋");
        assert_eq!(
            nav.favorites.selected().map(|s| s.mid.as_str()),
            Some("m3"),
            "原来那条被筛掉了，退回第一行"
        );
        nav.set_filter("");
        assert_eq!(nav.favorites.len(), 3, "清空筛选后全回来");
    }

    /// 歌单列表（左栏那一屏）也能按名字筛。
    #[test]
    fn the_playlist_list_can_be_filtered_too() {
        let (mut app, _runtime) = test_app();
        app.nav.switch_menu(Menu::Playlists);
        app.nav
            .playlists
            .set_items(vec![playlist("1", "嗯"), playlist("2", "循环")]);
        app.apply(keys::Action::Filter);
        app.apply(keys::Action::FilterChar('循'));
        assert_eq!(app.nav.filter_counts(), (1, 2));
        assert_eq!(
            app.nav.playlists.selected().map(|p| p.name.as_str()),
            Some("循环")
        );
        app.apply(keys::Action::FilterCancel);
        assert_eq!(app.nav.playlists.len(), 2);
    }

    /// Esc 先清筛选（它算一层），再按才退上一层视图。
    #[test]
    fn esc_clears_the_filter_before_popping_a_level() {
        let (mut app, _runtime) = test_app();
        app.nav.switch_menu(Menu::Playlists);
        app.nav.playlists.set_items(vec![playlist("1", "嗯")]);
        app.nav.push_tracks(
            "1",
            "嗯",
            vec![
                titled_song("m1", "晴天", "周杰伦"),
                titled_song("m2", "非酋", "薛明媛"),
            ],
        );
        app.nav.set_filter("非酋");
        assert_eq!(app.nav.filter_counts(), (1, 2));

        app.apply(keys::Action::Back);
        assert_eq!(app.nav.filter_counts(), (2, 2), "第一次 Esc 清掉筛选");
        assert_eq!(app.nav.breadcrumb, ["我的歌单", "嗯"], "第一次 Esc 不退层");

        app.apply(keys::Action::Back);
        assert_eq!(app.nav.breadcrumb, ["我的歌单"], "第二次 Esc 才退上一层");
    }

    /// `Y` 开关全屏歌词；`Esc` / 数字切屏都会退出它。
    #[test]
    fn fullscreen_lyric_toggles_and_is_left_by_esc_or_switching() {
        let (mut app, _runtime) = test_app();
        assert!(!app.lyric_full);
        app.apply(keys::Action::ToggleLyricFull);
        assert!(app.lyric_full);
        assert_eq!(app.context(), keys::Context::LyricFull, "全屏时上下文跟着变");

        // 全屏里再按 Y = 退出；底部那块歌词的开关（y）不受影响。
        app.apply(keys::Action::ToggleLyricFull);
        assert!(!app.lyric_full);
        assert_eq!(app.context(), keys::Context::Content);

        app.apply(keys::Action::ToggleLyricFull);
        app.apply(keys::Action::Back);
        assert!(!app.lyric_full, "Esc 退出全屏");
        assert!(!app.quit, "Esc 不该顺手退出程序");

        app.apply(keys::Action::ToggleLyricFull);
        app.apply(keys::Action::Menu(3));
        assert!(!app.lyric_full, "切屏就是离开这一屏");
        assert_eq!(app.nav.menu, Menu::History);
    }

    /// Esc 的层级：全屏歌词在「退一层视图」之前，退完层才轮到歌单。
    #[test]
    fn esc_leaves_the_fullscreen_before_popping_a_level() {
        let (mut app, _runtime) = test_app();
        app.nav.begin_tracks("1", "嗯");
        app.apply(keys::Action::ToggleLyricFull);
        app.apply(keys::Action::Back);
        assert!(!app.lyric_full, "第一次 Esc 退全屏");
        assert_eq!(app.nav.breadcrumb, ["我的歌单", "嗯"], "还留在这一层");
        app.apply(keys::Action::Back);
        assert_eq!(app.nav.breadcrumb, ["我的歌单"], "第二次 Esc 才退层");
    }

    /// 搜完看别的屏、再切回搜索：关键词与结果都还在。
    #[test]
    fn search_keeps_its_screen_so_browsing_is_not_lost() {
        let mut nav = Nav::new();
        nav.switch_menu(Menu::Favorites);
        nav.favorites.set_items(vec![test_song("a"), test_song("b")]);
        nav.favorites.select_first();
        nav.favorites.move_by(1);

        nav.switch_menu(Menu::Search);
        nav.search_text = "晴天".to_string();
        nav.search_results.set_items(vec![test_song("s1"), test_song("s2")]);
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

    /// 数字键切屏 = 「回到这一屏上次的位置」：正在某个歌单的曲目里，
    /// 切走再切回来要还在那个歌单里（面包屑、曲目、选中位置一起回去）。
    #[test]
    fn switching_back_returns_to_where_you_left_the_screen() {
        let (mut app, _runtime) = test_app();
        app.apply(keys::Action::Menu(1));
        app.nav.playlists.set_items(vec![playlist("1", "嗯")]);
        // 进歌单（真请求会另外回填，这里只关心导航），再塞两首进去。
        app.nav.begin_tracks("1", "嗯");
        app.nav.tracks.set_items(vec![test_song("a"), test_song("b")]);
        app.nav.tracks.move_by(1);
        assert_eq!(app.nav.breadcrumb, ["我的歌单", "嗯"]);

        app.apply(keys::Action::Menu(3));
        assert_eq!(app.nav.breadcrumb, ["历史"]);

        app.apply(keys::Action::Menu(1));
        assert_eq!(
            app.nav.breadcrumb,
            ["我的歌单", "嗯"],
            "回到这一屏要还原「还在哪个歌单里」，否则标题和内容对不上"
        );
        assert_eq!(
            app.nav
                .active_songs()
                .and_then(|view| view.selected())
                .map(|song| song.mid.as_str()),
            Some("b"),
            "选中位置也留着"
        );

        // Esc 才是退层：退出歌单，回到歌单列表。
        app.apply(keys::Action::Back);
        assert_eq!(app.nav.breadcrumb, ["我的歌单"]);
    }

    /// `j`/`k` 直接挪当前屏的列表（没有「菜单游标」这层中间态了）。
    #[test]
    fn movement_goes_straight_to_the_list() {
        let mut nav = Nav::new();
        nav.favorites.set_items(vec![test_song("a"), test_song("b")]);
        nav.favorites.select_first();
        assert_eq!(nav.favorites.state.selected(), Some(0));
        nav.move_selection(1);
        assert_eq!(nav.favorites.state.selected(), Some(1));
    }

    #[test]
    fn help_overlay_toggles_and_swallows_everything_else() {
        let (mut app, _runtime) = crate::tui::test_support::test_app();
        assert_eq!(app.context(), keys::Context::Content);
        app.apply(keys::Action::Help);
        assert!(app.help_open);
        assert_eq!(app.context(), keys::Context::HelpOverlay);
        app.apply(keys::Action::Help);
        assert!(!app.help_open);
        assert_eq!(app.context(), keys::Context::Content, "关掉后回到主界面");
    }

    #[test]
    fn digits_switch_screens() {
        let (mut app, _runtime) = crate::tui::test_support::test_app();
        app.apply(keys::Action::Menu(2));
        assert_eq!(app.nav.menu, Menu::Search);
        assert!(app.nav.search_input, "进搜索屏直接能输入");
        app.apply(keys::Action::Menu(1));
        assert_eq!(app.nav.menu, Menu::Playlists);
        assert!(!app.nav.search_input, "离开搜索屏要退出输入态");
    }

    #[test]
    fn p_opens_the_playlist_picker_and_a_stays_unbound_until_the_queue_lands() {
        let (mut app, _runtime) = crate::tui::test_support::test_app();
        app.nav.switch_menu(Menu::Favorites);
        app.nav.favorites.set_items(vec![test_song("a")]);
        app.nav.favorites.select_first();
        app.nav.playlists.set_items(vec![crate::tui::test_support::test_playlist("1", "嗯")]);
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
        app.nav.favorites.set_items(vec![test_song("a"), test_song("b")]);
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
        app.nav.favorites.set_items(vec![test_song("c")]);
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
        assert_eq!(app.nav.history_view.items().len(), 1, "历史屏立刻能看到");
        assert_eq!(app.nav.history_view.items()[0].mid, "a");
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
        assert!(app.nav.history_view.items().is_empty());
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

    /// 「我喜欢」在菜单里是单独一项，不该在「我的歌单」里再出现一次。
    #[test]
    fn the_playlist_list_drops_the_favorites_pseudo_playlist() {
        let (mut app, _runtime) = crate::tui::test_support::test_app();
        let mut favorites = crate::tui::test_support::test_playlist("favorites", "我喜欢");
        favorites.is_favorites = true;
        app.on_msg(Msg::Playlists(Ok(vec![
            favorites,
            crate::tui::test_support::test_playlist("1", "嗯"),
        ])));
        assert_eq!(app.nav.playlists.items().len(), 1, "只剩真歌单");
        assert_eq!(app.nav.playlists.items()[0].name, "嗯");
    }

    /// 队列浮层里的 `j`/`k` 必须挪浮层自己的高亮——它盖在主区上，
    /// 方向键穿透到底下的列表就会变成「浮层里选不动」（回归 bug）。
    #[test]
    fn the_queue_overlay_moves_its_own_highlight() {
        let (mut app, _runtime) = crate::tui::test_support::test_app();
        app.nav.switch_menu(Menu::Favorites);
        app.nav.favorites.set_items(vec![test_song("keep1"), test_song("keep2")]);
        app.nav.favorites.select_first();
        app.queue = queue::Queue::new(
            vec![test_song("a"), test_song("b"), test_song("c")],
            0,
            "嗯",
        );
        app.apply(keys::Action::QueueOverlay);
        assert_eq!(app.queue_state.selected(), Some(0), "打开时落在正在播的那首上");

        app.apply(keys::Action::Down);
        assert_eq!(app.queue_state.selected(), Some(1), "j 该挪浮层的高亮");
        assert_eq!(app.nav.favorites.state.selected(), Some(0), "底下那份列表不许动");

        app.apply(keys::Action::Last);
        assert_eq!(app.queue_state.selected(), Some(2), "G 跳末尾");
        app.apply(keys::Action::First);
        assert_eq!(app.queue_state.selected(), Some(0), "g 回开头");

        // 高亮在末尾时删除：删掉的是第 3 首（不是正在播的那首），队列短一格。
        app.apply(keys::Action::Last);
        app.apply(keys::Action::QueueDelete);
        assert_eq!(app.queue.len(), 2);
        assert_eq!(app.queue.items()[app.queue.index()].mid, "a", "当前这首没被换掉");
    }

    /// 歌单选择框（`p`）里有同一类问题：它的高亮也得自己走。
    #[test]
    fn the_playlist_picker_moves_its_own_highlight() {
        let (mut app, _runtime) = crate::tui::test_support::test_app();
        app.nav.switch_menu(Menu::Favorites);
        app.nav.favorites.set_items(vec![test_song("a")]);
        app.nav.favorites.select_first();
        app.nav.playlists.set_items(vec![
            crate::tui::test_support::test_playlist("1", "嗯"),
            crate::tui::test_support::test_playlist("2", "循环"),
        ]);
        app.apply(keys::Action::AddToPlaylist);
        let Some((_, state, _)) = &app.picker else {
            panic!("选择框该开着");
        };
        assert_eq!(state.selected(), Some(0));

        app.apply(keys::Action::Down);
        let Some((_, state, _)) = &app.picker else {
            panic!("选择框还开着");
        };
        assert_eq!(state.selected(), Some(1), "j 该挪选择框的高亮");
        assert_eq!(app.nav.favorites.state.selected(), Some(0), "底下那份列表不许动");
    }
}

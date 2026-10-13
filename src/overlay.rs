//! 桌面歌词：取词、状态机与窗口位置（平台无关的那一半）。
//!
//! 平台相关的部分在下面 `cfg` 子模块里：Windows 是「专用线程 + 分层窗口 + DIB 绘制」，
//! macOS 是「主线程上建无边框 NSPanel + 三个透明 label」，其它平台空实现。状态机是两半
//! 共用的——多写一份窗口实现只需要补 `imp` 那三个方法（建、推、拆），
//! `Frame` / `Payload` / 窗口位置 / 开关状态一行都不用动。

use std::path::Path;
use std::sync::mpsc::Receiver;

use anyhow::{Context, Result};

use crate::config::DesktopLyricsConfig;
use crate::model::Lyric;
use crate::player::{Phase, Snapshot};

/// 夹取窗口位置时留的边距：中心点不会贴到屏幕边上。
pub const MARGIN: i32 = 40;

/// 解锁但没有歌词可比时显示的那句话——不然解锁了也找不到窗口，没法拖。
pub const UNLOCKED_HINT: &str = "拖动我（再按一次 ctrl+shift+l 锁定）";

/// 一帧的外部输入（都是借来的，不复制）。
#[derive(Clone, Copy)]
pub struct Frame<'a> {
    pub lyric: Option<&'a Lyric>,
    /// 引擎快照：`phase` 与 `position` 都在里面。
    pub snapshot: &'a Snapshot,
}

/// 覆盖层要显示的东西；`PartialEq` 就是「要不要重画」的判据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payload {
    /// `false` = 该隐藏（窗口不销毁，只是 `SW_HIDE`，位置与锁定态留着）。
    pub visible: bool,
    /// 当前句（可能是空串：歌词里的纯音乐段）。
    pub current: String,
    /// 下一句（最后一句之后是空串）。
    pub next: String,
    /// 当前句的译文（配置关掉或这首没有就是 `None`）。
    pub translation: Option<String>,
    /// 锁定态（决定鼠标穿透与那圈提示框）。
    pub locked: bool,
}

/// 状态机：记住上次推出去的 payload 与锁定态。
struct State {
    last: Option<Payload>,
    locked: bool,
}

impl Default for State {
    fn default() -> Self {
        // 启动一律锁定：锁定态刻意不持久化，省得下次开机「怎么点不到」。
        Self { last: None, locked: true }
    }
}

impl State {
    fn toggle_lock(&mut self) {
        self.locked = !self.locked;
    }

    /// 算「理想 payload」，跟上次推出去的比：不同才返回（相同就什么都不做）。
    fn advance(&mut self, frame: Frame<'_>, config: &DesktopLyricsConfig) -> Option<Payload> {
        // 取流中沿用上一次的样子，别闪——但锁定态变了要**立刻**推：按了解锁却半天摸不到
        // 窗口，是最容易被当成「功能坏了」的一种。
        if matches!(frame.snapshot.phase, Phase::Buffering) {
            let locked = self.locked;
            let last = self.last.as_ref()?;
            if last.locked == locked {
                return None;
            }
            let mut payload = last.clone();
            payload.locked = locked;
            self.last = Some(payload.clone());
            return Some(payload);
        }

        let (current, next, translation) = lines(frame, config);
        let has_something = !current.is_empty() || !next.is_empty() || translation.is_some();

        let playing = match frame.snapshot.phase {
            Phase::Playing => true,
            Phase::Paused => !config.hide_when_paused,
            // `Idle` / `Failed`（`Buffering` 在上面已经处理）
            _ => false,
        };
        let mut visible = playing && has_something;
        // 隐藏时不带文案：免得下次显示时先闪一下旧句。
        let (current, next, translation) = if visible {
            (current, next, translation)
        } else {
            ("", "", None)
        };

        // 解锁时强制可见：否则解锁了却找不到窗口，没法拖；一点内容都没有就给句提示语。
        let hint = !self.locked && current.is_empty() && next.is_empty() && translation.is_none();
        if !self.locked {
            visible = true;
        }

        // 比的是**借来的**文本：没变化的那一帧一次分配都不做（每 150ms 都会走到这里）。
        let unchanged = self.last.as_ref().is_some_and(|last| {
            last.visible == visible
                && last.locked == self.locked
                && last.current == current
                && last.next == next
                && last.translation.as_deref() == translation
        });
        if unchanged {
            return None;
        }
        let payload = Payload {
            visible,
            current: if hint { UNLOCKED_HINT.to_string() } else { current.to_string() },
            next: next.to_string(),
            translation: translation.map(str::to_string),
            locked: self.locked,
        };
        self.last = Some(payload.clone());
        Some(payload)
    }
}

/// 当前句、下一句（过界就是空串）、当前句的译文（配置关掉就是 `None`）。
///
/// 借的是歌词本身、不造 `String`：调用方只在自己真的要变了的时候才克隆，
/// 这样「内容没变的那一帧」连一次分配都没有。
fn lines<'a>(frame: Frame<'a>, config: &DesktopLyricsConfig) -> (&'a str, &'a str, Option<&'a str>) {
    let Some(lyric) = frame.lyric else {
        return ("", "", None);
    };
    let index = lyric.line_at(frame.snapshot.position);
    let line = index.and_then(|index| lyric.lines.get(index));
    let current = line.map(|line| line.text.as_str()).unwrap_or_default();
    let next = index
        .and_then(|index| lyric.lines.get(index + 1))
        .map(|line| line.text.as_str())
        .unwrap_or_default();
    let translation = if config.show_translation {
        line.and_then(|line| line.translation.as_deref())
    } else {
        None
    };
    (current, next, translation)
}

/// 默认位置：主屏底部居中。
fn default_center(screen: (i32, i32)) -> (i32, i32) {
    (screen.0 / 2, screen.1 * 85 / 100)
}

/// 把中心点夹进主屏内（留 `MARGIN` 边距）；屏幕特别小时也不会夹出反区间。
fn clamp_center(center: (i32, i32), screen: (i32, i32)) -> (i32, i32) {
    (
        center.0.clamp(MARGIN, (screen.0 - MARGIN).max(MARGIN)),
        center.1.clamp(MARGIN, (screen.1 - MARGIN).max(MARGIN)),
    )
}

/// 主屏尺寸（宽、高，逻辑点）。
#[cfg(target_os = "windows")]
fn primary_screen() -> (i32, i32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    // SAFETY: 只读两个系统指标，没有所有权问题。
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}

/// 主屏尺寸：`NSScreen.mainScreen`（带菜单栏那块屏）的 frame，单位是逻辑点。
///
/// 拿不到主屏（没有窗口服务器连接、比如 ssh 会话）就用一个确定的数顶着——那时也建不出
/// 歌词窗口，这个值只影响「默认位置」与「夹回屏内」的计算。
#[cfg(target_os = "macos")]
fn primary_screen() -> (i32, i32) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSScreen;

    let Some(mtm) = MainThreadMarker::new() else {
        return (1920, 1080);
    };
    NSScreen::mainScreen(mtm).map_or((1920, 1080), |screen| {
        let size = screen.frame().size;
        (size.width as i32, size.height as i32)
    })
}

/// 其它平台（Linux / Wayland 等）：不建窗口，也就没有屏幕尺寸可问——给个确定的数，
/// 好让默认位置与夹取的计算在测试与编译里都稳。
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn primary_screen() -> (i32, i32) {
    (1920, 1080)
}

/// 开关与窗口中心点，都记在 `desktop-lyrics.json` 里。
///
/// 两件都是**状态**不是偏好：开关由全局热键翻（`DesktopLyrics::toggle`），位置由拖动改，所以
/// 不写 `config.toml`——那里面写的是偏好，而且热键去改使用者的配置文件会连注释一起毁掉。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SavedState {
    /// `None` = 还没记过开关（第一次跑，或者文件是升级前那份）。启动初值见 [`start_shown`]。
    pub shown: Option<bool>,
    /// 窗口中心点（左上原点）。`None` = 没记过，用默认的底部居中。
    pub center: Option<(i32, i32)>,
}

/// 文件里的形状：开关与中心点都可缺（升级前那份只有中心点）。
#[derive(serde::Deserialize, serde::Serialize)]
struct SavedFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shown: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cx: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cy: Option<i32>,
}

/// 启动时开不开：状态文件优先（缺 `shown` = 还没记过），其次老配置里遗留的 `enabled`
/// （那个键已移除，但用它当一次初值，免得「明明设了关、升级后冒出个窗口」），再不然默认开。
fn start_shown(saved: Option<bool>, legacy: Option<bool>) -> bool {
    saved.or(legacy).unwrap_or(true)
}

/// 读开关与中心点；文件不在或读坏了就是「什么都没记」（调用方退回默认）。
pub fn load_state(path: &Path) -> SavedState {
    let Ok(text) = std::fs::read_to_string(path) else {
        return SavedState::default();
    };
    let Ok(file) = serde_json::from_str::<SavedFile>(&text) else {
        return SavedState::default();
    };
    // 半个坐标当没记：那种文件是人手改坏的。
    SavedState { shown: file.shown, center: file.cx.zip(file.cy) }
}

/// 写开关与中心点（临时文件 + rename，中途失败不留坏文件——跟 `history.rs` 一个套路）。
pub fn save_state(path: &Path, state: SavedState) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(parent)
        .with_context(|| format!("创建配置目录失败：{}", parent.display()))?;
    let file = SavedFile {
        shown: state.shown,
        cx: state.center.map(|center| center.0),
        cy: state.center.map(|center| center.1),
    };
    let text = serde_json::to_string(&file)?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text).with_context(|| format!("写 {} 失败", temp.display()))?;
    std::fs::rename(&temp, path).with_context(|| format!("替换 {} 失败", path.display()))
}

/// 桌面歌词的句柄。
pub struct DesktopLyrics {
    state: State,
    imp: imp::Imp,
    /// 意图：窗口该开着吗。落盘写的是它（真相看 [`DesktopLyrics::is_running`]）。
    shown: bool,
    /// 窗口中心点：上次拖动回传的，或启动时从状态文件读到的；没记过就是 `None`（用默认位置）。
    center: Option<(i32, i32)>,
    /// 拖动时窗口线程回传新中心点的通道。**跟窗口同生命周期**：拆窗口时一起丢、重建时换成
    /// 新的——所以它在这儿，不在 `App` 上（放 `App` 上的话重建后拿到的还是旧通道）。
    positions: Option<Receiver<(i32, i32)>>,
    /// 开关或位置变过、还没落盘。
    dirty: bool,
}

/// 装出来的东西。
pub struct Installed {
    pub overlay: DesktopLyrics,
    /// `Some` = 起不来（调用方报一句 + 日志），此时 `overlay` 是空句柄。
    pub error: Option<String>,
}

impl DesktopLyrics {
    /// 空句柄：Linux、`App::new` 与测试都用它；`sync` 什么都不做，`is_running()` 是 `false`。
    pub fn none() -> Self {
        Self {
            state: State::default(),
            imp: imp::Imp::none(),
            shown: false,
            center: None,
            positions: None,
            dirty: false,
        }
    }

    /// 启动：状态文件（或老配置里的 `enabled`）说开着就建窗口。
    ///
    /// 建不起来只报一句、界面照跑——**不改意图、不落盘**：文件记的是「你想开着」，按一次热键
    /// 就是重试。
    pub fn install(config: &DesktopLyricsConfig, saved: SavedState) -> Installed {
        let mut overlay = Self { center: saved.center, ..Self::none() };
        if !start_shown(saved.shown, config.legacy_enabled) {
            return Installed { overlay, error: None };
        }
        let error = overlay.enable(config);
        Installed { overlay, error }
    }

    /// 开 / 关（全局热键那一份）。返回新状态与「建不起来」的原因。
    ///
    /// 关 = 真拆窗口（Windows 收线程与窗口、macOS 放掉 panel），开 = 真建一个。两端共用的
    /// 记账都在这里，平台差异只在 `imp`。
    pub fn toggle(&mut self, config: &DesktopLyricsConfig) -> (bool, Option<String>) {
        if self.imp.running() {
            self.disable();
            return (false, None);
        }
        if let Some(error) = self.enable(config) {
            return (self.shown, Some(error));
        }
        if self.imp.running() {
            self.shown = true;
            self.dirty = true;
            return (true, None);
        }
        // 这个平台没有覆盖层实现（跟 `hotkeys` 的空实现一个口径）：没窗口可管，什么都不动。
        (self.shown, None)
    }

    /// 建窗口。`Some` = 起不来（此时 `imp` 仍是空句柄，`is_running()` 是 `false`）。
    fn enable(&mut self, config: &DesktopLyricsConfig) -> Option<String> {
        if self.imp.running() {
            return None;
        }
        // 存过中心点就用它（顺手夹回主屏内，换过显示器/分辨率时不会跑到屏外）；没有就底部居中。
        let screen = primary_screen();
        let center =
            self.center.map_or_else(|| default_center(screen), |center| clamp_center(center, screen));
        let (imp, positions, error) = imp::install(config, center);
        self.imp = imp;
        if error.is_some() || !self.imp.running() {
            return error;
        }
        self.adopt_window(center, positions);
        None
    }

    /// 拆窗口：连线程/panel 一起收掉，位置通道作废，锁定态与 payload 记账一并忘掉。
    ///
    /// 位置（`center`）留着——下次开回来还在原地。
    fn disable(&mut self) {
        self.imp = imp::Imp::none();
        self.positions = None;
        self.state = State::default();
        self.shown = false;
        self.dirty = true;
    }

    /// 建完窗口之后的记账（跟「建窗口」那件不能单测的事分开，这里能单测）。
    fn adopt_window(&mut self, center: (i32, i32), positions: Option<Receiver<(i32, i32)>>) {
        // `State` 必须回到初始态：它记着上一份窗口的 payload，不清的话新窗口第一帧
        // `advance()` 会判定「没变化」而什么都不推——窗口就永远空着。锁定态也跟着回默认
        // （拆了再建是锁定的，鼠标穿透）。
        self.state = State::default();
        self.center = Some(center);
        self.positions = positions;
    }

    /// 现在开着吗（真相，不是意图：起不来的窗口不算开着）。
    pub fn is_running(&self) -> bool {
        self.imp.running()
    }

    /// 开关或位置变过就给一份、并清掉标记；没变过给 `None`（调用方据此决定落不落盘）。
    pub fn take_state(&mut self) -> Option<SavedState> {
        if !self.dirty {
            return None;
        }
        self.dirty = false;
        Some(SavedState { shown: Some(self.shown), center: self.center })
    }

    /// 每帧调一次：先泵平台那边的事件，再有变化就把新 payload 推给窗口。
    ///
    /// 泵必须每帧都跑（payload 没变也要）：macOS 的拖动与重绘全靠它，而拖动过程中 payload
    /// 恰恰是不变的——只在变化时泵会把拖动冻住。
    pub fn sync(&mut self, frame: Frame<'_>, config: &DesktopLyricsConfig) {
        // 没窗口就整段不进：连 payload 都不造（「关掉就零成本」靠这一句守着）。
        if !self.imp.running() {
            return;
        }
        self.imp.pump();
        if let Some(payload) = self.state.advance(frame, config) {
            self.imp.push(payload);
        }
        self.collect_centers();
    }

    /// 切锁定/解锁（下一帧 `sync` 自然会把新锁定态推出去）。
    pub fn toggle_lock(&mut self) {
        self.state.toggle_lock();
    }

    /// 排空位置通道（拖动结束时窗口线程回传的新中心点）：取最后一条，变了才置 `dirty`
    /// ——中间那些没人看，反正写的都是同一个文件。
    fn collect_centers(&mut self) {
        let Some(positions) = &self.positions else {
            return;
        };
        let mut latest = None;
        while let Ok(center) = positions.try_recv() {
            latest = Some(center);
        }
        if let Some(center) = latest
            && self.center != Some(center)
        {
            self.center = Some(center);
            self.dirty = true;
        }
    }

    /// 现在是解锁的吗。
    ///
    /// 只给测试看：界面上没有别的消费者（提示框由覆盖层自己画），生产代码只负责把它翻过去。
    #[cfg(test)]
    pub fn locked(&self) -> bool {
        self.state.locked
    }
}

/// Windows：专用线程 + 分层窗口（`UpdateLayeredWindow` + 32 位 DIB）。
///
/// 两件 GDI 的坑，都在这里处理掉了（探针验过才敢这么写）：
/// 1. GDI 往 32 位 DIB 里画字**不写 alpha 字节**——所以先用「白字黑底」把它的输出当**覆盖率蒙版**，
///    再由 `composite` 按 over 合成到预乘 BGRA 上；这样抗锯齿才是干净的灰阶（不是彩边）。
/// 2. 分层窗口要有人**跑消息循环**，而且 `WM_NCHITTEST` 得按锁定态回 `HTTRANSPARENT`/`HTCAPTION`。
#[cfg(target_os = "windows")]
mod imp {
    // 这个模块就是一层薄薄的 Win32 绑定：每个函数都是 `unsafe fn`（契约写在签名上）、
    // 里面几乎每句都在调 Win32。edition 2024 的 `unsafe_op_in_unsafe_fn` 在这儿只会
    // 制造噪音，整体放宽；要盯的是「调用点有没有把前置条件写清楚」，不是有没有加括号。
    #![allow(unsafe_op_in_unsafe_fn)]

    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;

    use windows_sys::Win32::Foundation::{
        ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM,
    };
    use windows_sys::Win32::Graphics::Gdi::{
        AC_SRC_ALPHA, AC_SRC_OVER, ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER,
        BLENDFUNCTION, CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateDIBSection, CreateFontW,
        DEFAULT_CHARSET, DEFAULT_PITCH, DeleteDC, DeleteObject, FF_DONTCARE, FW_NORMAL, GdiFlush,
        GetTextExtentPoint32W, HBITMAP, HDC, HFONT, HGDIOBJ, OUT_DEFAULT_PRECIS, SelectObject,
        SetBkColor, SetBkMode, SetTextColor, TextOutW,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWL_EXSTYLE,
        GetMessageW, GetWindowLongW, GetWindowRect, HTCAPTION, HTTRANSPARENT, HWND_TOPMOST, MSG,
        PostMessageW, PostQuitMessage, RegisterClassW, SWP_FRAMECHANGED, SWP_NOACTIVATE,
        SWP_NOMOVE, SWP_NOSIZE, SW_SHOWNA, SW_HIDE, SetWindowLongW, SetWindowPos, ShowWindow,
        TranslateMessage, ULW_ALPHA, UpdateLayeredWindow, WM_APP, WM_CLOSE, WM_DESTROY,
        WM_EXITSIZEMOVE, WM_NCHITTEST, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
        WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
    };

    use super::{DesktopLyricsConfig, Payload};

    /// 「有新 payload 了」的自定义消息。
    const WM_PAYLOAD: u32 = WM_APP + 1;
    /// 窗口类名（进程内唯一就够）。
    const CLASS: &str = "canto_lyrics_overlay";

    /// 锁定态给 wndproc 看。wndproc 是个裸函数、拿不到线程里的状态，所以用个进程级原子
    /// （覆盖层只有一个，够用；`WM_PAYLOAD` 里同步）。
    static LOCKED: AtomicBool = AtomicBool::new(true);

    pub(super) type Started = (Imp, Option<Receiver<(i32, i32)>>, Option<String>);

    /// 主线程（写 payload）与窗口线程（读 payload）之间那一小块。
    struct Shared {
        payload: Mutex<Payload>,
        config: DesktopLyricsConfig,
    }

    pub(super) struct Imp {
        /// 窗口句柄。存 `isize` 而不是裸指针，省得 `Imp` 因为裸指针背上 `Send`/`Sync` 的麻烦。
        hwnd: isize,
        join: Option<JoinHandle<()>>,
        shared: Arc<Shared>,
    }

    /// 窗口线程自己的绘制状态（线程独占，不用锁）。
    struct Painter {
        /// 当前句的字体。
        big: HFONT,
        /// 译文与下一句的字体（小一号）。
        small: HFONT,
        /// `Some` = 样式已经按这个锁定态设过了。
        transparent: Option<bool>,
        /// 窗口中心点：每次 `draw` 都按窗口实际矩形刷新（拖动是系统直接改窗口的）。
        center: (i32, i32),
        /// 已经回传给主线程落盘过的中心点（免得每帧都写一遍文件）。
        reported: (i32, i32),
    }

    impl Imp {
        pub(super) fn none() -> Self {
            Self {
                hwnd: 0,
                join: None,
                shared: Arc::new(Shared {
                    payload: Mutex::new(hidden_payload()),
                    config: DesktopLyricsConfig::default(),
                }),
            }
        }

        /// 有新东西要画：写进共享块，然后叫窗口线程一声。
        pub(super) fn push(&mut self, payload: Payload) {
            *self.shared.payload.lock().unwrap_or_else(|error| error.into_inner()) = payload;
            if self.hwnd != 0 {
                // SAFETY: 句柄是我们自己线程建的窗口；`Drop` 之后不会再调 `push`。
                unsafe { PostMessageW(self.hwnd as HWND, WM_PAYLOAD, 0, 0) };
            }
        }

        /// Windows 的事件由窗口线程自己跑消息循环，主循环这边不用泵。
        pub(super) fn pump(&mut self) {}

        /// 有窗口才算开着。拆掉之后句柄清成 0，所以重建前后的判断都靠它。
        pub(super) fn running(&self) -> bool {
            self.hwnd != 0
        }
    }

    impl Drop for Imp {
        fn drop(&mut self) {
            let Some(join) = self.join.take() else {
                return;
            };
            // 让窗口线程自己收尾：`WM_CLOSE` → `DestroyWindow` → `WM_DESTROY` → 退出消息循环。
            if self.hwnd != 0 {
                unsafe { PostMessageW(self.hwnd as HWND, WM_CLOSE, 0, 0) };
            }
            if join.join().is_err() {
                tracing::warn!("歌词窗口线程中途炸了");
            }
        }
    }

    fn hidden_payload() -> Payload {
        Payload {
            visible: false,
            current: String::new(),
            next: String::new(),
            translation: None,
            locked: true,
        }
    }

    pub(super) fn install(config: &DesktopLyricsConfig, center: (i32, i32)) -> Started {
        let shared = Arc::new(Shared {
            payload: Mutex::new(hidden_payload()),
            // 配置在进程里是固定的，窗口线程拿走一份。
            config: config.clone(),
        });
        let (ready_tx, ready_rx) = channel::<Result<isize, String>>();
        let (places_tx, places_rx) = channel::<(i32, i32)>();
        let thread_shared = Arc::clone(&shared);
        let spawned = std::thread::Builder::new()
            .name("canto-lyrics".to_string())
            .spawn(move || run(thread_shared, center, ready_tx, places_tx));
        let join = match spawned {
            Ok(join) => join,
            Err(error) => {
                tracing::warn!(%error, "桌面歌词线程没起来");
                return (Imp::none(), None, Some("歌词窗口起不来".to_string()));
            }
        };
        match ready_rx.recv() {
            // 窗口建好了：把句柄收下，位置通道交给主线程。
            Ok(Ok(hwnd)) => (Imp { hwnd, join: Some(join), shared }, Some(places_rx), None),
            Ok(Err(reason)) => {
                let _ = join.join();
                (Imp::none(), None, Some(reason))
            }
            Err(_) => {
                let _ = join.join();
                (Imp::none(), None, Some("歌词窗口线程提前退出".to_string()))
            }
        }
    }

    /// 窗口线程：注册类 → 建窗口 → 回报 → 消息循环（画、拖动、收尾都在这儿）。
    fn run(
        shared: Arc<Shared>,
        center: (i32, i32),
        ready: Sender<Result<isize, String>>,
        places: Sender<(i32, i32)>,
    ) {
        // SAFETY: 这一整个函数都在窗口线程上跑，句柄与 GDI 对象都在本线程创建/释放。
        unsafe {
            let hinstance = GetModuleHandleW(std::ptr::null());
            let class = wide(CLASS);
            let registered = RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinstance,
                lpszClassName: class.as_ptr(),
                ..std::mem::zeroed()
            });
            if registered == 0 {
                // 类名在进程里注册过一次就够。窗口被拆掉再重建时这里必然「已存在」，
                // 那不是失败（以前只会建一次，所以看不出来）。
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(ERROR_CLASS_ALREADY_EXISTS as i32) {
                    tracing::warn!(%error, "注册歌词窗口类失败");
                    let _ = ready.send(Err("歌词窗口起不来".to_string()));
                    return;
                }
            }

            let ex_style = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW
                | WS_EX_NOACTIVATE;
            let hwnd = CreateWindowExW(
                ex_style,
                class.as_ptr(),
                wide("canto lyrics").as_ptr(),
                WS_POPUP,
                center.0 - 100,
                center.1 - 40,
                200,
                80,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                hinstance,
                std::ptr::null(),
            );
            if hwnd.is_null() {
                let error = std::io::Error::last_os_error();
                tracing::warn!(%error, "建歌词窗口失败");
                let _ = ready.send(Err("歌词窗口起不来".to_string()));
                return;
            }
            let _ = ready.send(Ok(hwnd as isize));

            // 字体按窗口 DPI 定一次（配置在进程里不再变）。
            let dpi = GetDpiForWindow(hwnd).max(96);
            // 安装记录在这儿打，不放在 `tui::run`：只有真的建了窗口的平台才该说这句
            // （非 Windows 那份是空实现，在那边说「就绪」是假话）。
            tracing::info!(dpi, "桌面歌词窗口已就绪");
            let face = wide(&shared.config.font);
            let big_size = (shared.config.font_size * dpi / 96).max(8) as i32;
            let small_size = ((shared.config.font_size * 3 / 5) * dpi / 96).max(8) as i32;
            let mut painter = Painter {
                big: make_font(big_size, face.as_ptr()),
                small: make_font(small_size, face.as_ptr()),
                transparent: None,
                center,
                reported: center,
            };

            let mut message: MSG = std::mem::zeroed();
            while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                match message.message {
                    // 画一帧（要在这一线程做，绘制状态是线程本地的）。
                    WM_PAYLOAD => draw(hwnd, &shared, &mut painter, &places),
                    _ => {
                        TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
            }

            DeleteObject(painter.big as HGDIOBJ);
            DeleteObject(painter.small as HGDIOBJ);
        }
    }

    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        // edition 2024：`unsafe fn` 里也要显式 `unsafe` 块。
        match message {
            // 锁定时整窗对鼠标透明（`WindowFromPoint` 会跳过我们）；解锁时当标题栏用，
            // 拖动交给系统自己做（`WM_EXITSIZEMOVE` 收尾）。
            WM_NCHITTEST => {
                if LOCKED.load(Ordering::Relaxed) {
                    HTTRANSPARENT as LRESULT
                } else {
                    HTCAPTION as LRESULT
                }
            }
            // 拖动结束：`WM_EXITSIZEMOVE` 是**发送**消息，进不了 `GetMessageW` 的队列，
            // 所以从这儿转投一条重画——`draw` 会把窗口实际矩形采纳成新中心点并回传落盘。
            // 没有这一手，暂停状态下拖完就没人记得新位置（那时候不会有别的重画）。
            WM_EXITSIZEMOVE => {
                unsafe { PostMessageW(hwnd, WM_PAYLOAD, 0, 0) };
                0
            }
            WM_CLOSE => unsafe {
                DestroyWindow(hwnd);
                0
            },
            WM_DESTROY => unsafe {
                PostQuitMessage(0);
                0
            },
            _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
        }
    }

    /// 画一帧：不可见就藏起来；可见就量尺寸、按中心点摆好、画 DIB、`UpdateLayeredWindow`。
    unsafe fn draw(hwnd: HWND, shared: &Shared, painter: &mut Painter, places: &Sender<(i32, i32)>) {
        let payload = shared
            .payload
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();

        // 锁定态变了才动样式（`WS_EX_TRANSPARENT` 决定鼠标穿透）。
        if painter.transparent != Some(payload.locked) {
            let style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            let style = if payload.locked {
                style | WS_EX_TRANSPARENT
            } else {
                style & !WS_EX_TRANSPARENT
            };
            SetWindowLongW(hwnd, GWL_EXSTYLE, style as i32);
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
            );
            painter.transparent = Some(payload.locked);
        }
        LOCKED.store(payload.locked, Ordering::Relaxed);

        if !payload.visible {
            ShowWindow(hwnd, SW_HIDE);
            return;
        }

        // 要画的三行：当前句（大、亮）→ 译文 → 下一句（都小、暗）。空的不画。
        let config = &shared.config;
        let mut rows: Vec<(HFONT, &str, (u8, u8, u8))> = Vec::new();
        if !payload.current.is_empty() {
            rows.push((painter.big, payload.current.as_str(), rgb(config.color)));
        }
        if let Some(translation) = payload.translation.as_deref().filter(|text| !text.is_empty()) {
            rows.push((painter.small, translation, rgb(config.next_color)));
        }
        if !payload.next.is_empty() {
            rows.push((painter.small, payload.next.as_str(), rgb(config.next_color)));
        }

        // 量尺寸：宽度取最宽一行，高度是各行之和，再加内边距。
        let padding = (8 * GetDpiForWindow(hwnd).max(96) / 96) as i32;
        let measure = CreateCompatibleDC(std::ptr::null_mut());
        let mut widths = Vec::new();
        let mut heights = Vec::new();
        for (font, text, _) in &rows {
            SelectObject(measure, *font as HGDIOBJ);
            let text: Vec<u16> = text.encode_utf16().collect();
            let mut size: SIZE = std::mem::zeroed();
            GetTextExtentPoint32W(measure, text.as_ptr(), text.len() as i32, &mut size);
            widths.push(size.cx);
            heights.push(size.cy);
        }
        DeleteDC(measure);
        let width = widths.iter().copied().max().unwrap_or(0) + padding * 2;
        let height: i32 = heights.iter().sum::<i32>() + padding * 2;

        // 位置以**窗口自己**为准：解锁态的拖动是系统直接改窗口，缓存里的中心点可能是旧的。
        // 每个 `draw` 都重新采纳一次，顺带得到两个好处：拖动途中换行不会把窗口拽回旧位置；
        // 也不用指望 `WM_EXITSIZEMOVE`——那是**发送**消息，进不了 `GetMessageW` 的队列。
        let mut rect: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut rect);
        let actual = ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
        if actual != painter.center {
            painter.center = actual;
            if painter.reported != actual {
                painter.reported = actual;
                let _ = places.send(actual);
            }
        }

        // 按中心点摆好（中心点由拖动更新）。
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            painter.center.0 - width / 2,
            painter.center.1 - height / 2,
            width,
            height,
            SWP_NOACTIVATE,
        );

        let Some(mut canvas) = Canvas::new(width, height) else {
            // 建不出画布（GDI 对象用光了之类）：这一帧不画总比崩掉好。
            tracing::warn!("建歌词画布失败（GDI 对象不够？）");
            return;
        };
        let mut y = padding;
        for (index, (font, text, color)) in rows.iter().enumerate() {
            let text_wide: Vec<u16> = text.encode_utf16().collect();
            let x = (width - widths[index]) / 2;
            // 描边：八方向各画一遍，再画正文（这样任何背景上都看得清）。
            for offset in OUTLINE_OFFSETS {
                canvas.stamp(
                    *font,
                    &text_wide,
                    x + offset.0,
                    y + offset.1,
                    rgb(config.outline),
                );
            }
            canvas.stamp(*font, &text_wide, x, y, *color);
            y += heights[index];
        }
        // 解锁态：画一圈细框，提示「现在能拖」。
        if !payload.locked {
            canvas.border();
        }

        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: (config.opacity as u32 * 255 / 100) as u8,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let size = SIZE { cx: width, cy: height };
        let origin = POINT { x: 0, y: 0 };
        UpdateLayeredWindow(
            hwnd,
            std::ptr::null_mut(),
            std::ptr::null(),
            &size,
            canvas.dc,
            &origin,
            0,
            &blend,
            ULW_ALPHA,
        );
        ShowWindow(hwnd, SW_SHOWNA);
    }

    fn rgb(color: crate::config::Rgb) -> (u8, u8, u8) {
        (color.r, color.g, color.b)
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe fn make_font(height: i32, face: *const u16) -> HFONT {
        CreateFontW(
            height,
            0,
            0,
            0,
            FW_NORMAL as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            // 灰度抗锯齿：ClearType 是子像素 AA，做 alpha 混合会出彩边。
            ANTIALIASED_QUALITY as u32,
            (DEFAULT_PITCH as u32) | (FF_DONTCARE as u32),
            face,
        )
    }

    /// 描边的八个方向。
    const OUTLINE_OFFSETS: [(i32, i32); 8] = [
        (-1, -1),
        (0, -1),
        (1, -1),
        (-1, 0),
        (1, 0),
        (-1, 1),
        (0, 1),
        (1, 1),
    ];

    /// 一块 32 位自上而下的 DIB（预乘 BGRA）+ 一块画蒙版用的同类 DIB。
    ///
    /// 两块位图**必须**留着句柄：DIB 的内存归位图句柄所有，`DeleteDC` 不会还它
    /// （MSDN 的 `CreateDIBSection` 明说要用 `DeleteObject` 释放）。每帧泄漏两块的话，
    /// 几千帧之后就会把进程的 GDI 配额用光，那时 `CreateDIBSection` 返回空指针。
    struct Canvas {
        width: i32,
        height: i32,
        dc: HDC,
        /// `SelectObject` 进 DC 的那块原始占位位图：`Drop` 得先换回它才能删我们的。
        dc_placeholder: HGDIOBJ,
        bitmap: HBITMAP,
        bits: *mut u8,
        mask_dc: HDC,
        mask_placeholder: HGDIOBJ,
        mask: HBITMAP,
        mask_bits: *mut u8,
    }

    impl Canvas {
        unsafe fn new(width: i32, height: i32) -> Option<Self> {
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    // 负高度 = 自上而下（第 0 行就是屏幕最上面那行）。
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    ..std::mem::zeroed()
                },
                ..std::mem::zeroed()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let mut mask_bits: *mut c_void = std::ptr::null_mut();
            let dc = CreateCompatibleDC(std::ptr::null_mut());
            let mask_dc = CreateCompatibleDC(std::ptr::null_mut());
            let bitmap = CreateDIBSection(dc, &info, 0, &mut bits, std::ptr::null_mut(), 0);
            let mask = CreateDIBSection(mask_dc, &info, 0, &mut mask_bits, std::ptr::null_mut(), 0);
            // 建不出来（GDI 配额用光之类）就整块放弃，绝不拿空指针往下写。
            if bitmap.is_null() || mask.is_null() {
                DeleteObject(bitmap as HGDIOBJ);
                DeleteObject(mask as HGDIOBJ);
                DeleteDC(dc);
                DeleteDC(mask_dc);
                return None;
            }
            let dc_placeholder = SelectObject(dc, bitmap as HGDIOBJ);
            let mask_placeholder = SelectObject(mask_dc, mask as HGDIOBJ);
            let bytes = (width * height * 4) as usize;
            std::ptr::write_bytes(bits as *mut u8, 0, bytes);
            std::ptr::write_bytes(mask_bits as *mut u8, 0, bytes);
            Some(Self {
                width,
                height,
                dc,
                dc_placeholder,
                bitmap,
                bits: bits as *mut u8,
                mask_dc,
                mask_placeholder,
                mask,
                mask_bits: mask_bits as *mut u8,
            })
        }

        /// 把一行字以某个颜色画上去：先用「白字黑底」画进蒙版（GDI 只写 RGB），
        /// 再按覆盖率合成到预乘 BGRA 上。
        unsafe fn stamp(&mut self, font: HFONT, text: &[u16], x: i32, y: i32, color: (u8, u8, u8)) {
            let pixels = (self.width * self.height) as usize;
            std::ptr::write_bytes(self.mask_bits, 0, pixels * 4);
            SelectObject(self.mask_dc, font as HGDIOBJ);
            // OPAQUE + 黑底：抗锯齿的中间值就会落成灰度（= 覆盖率）。
            SetBkMode(self.mask_dc, 2);
            SetBkColor(self.mask_dc, 0);
            SetTextColor(self.mask_dc, 0x00FF_FFFF);
            TextOutW(self.mask_dc, x, y, text.as_ptr(), text.len() as i32);
            // GDI 会批处理绘制调用；要读那块 DIB 的内存，得先让它落地（`CreateDIBSection`
            // 的文档就是这么要求的）。不刷的话 `composite` 可能读到全零/上一帧的覆盖率。
            GdiFlush();
            composite(self.mask_bits, self.bits, pixels, color);
        }

        /// 解锁提示框：直接往缓冲里描一圈 1px 的白线（alpha 140）。
        unsafe fn border(&mut self) {
            let (width, height) = (self.width as usize, self.height as usize);
            let alpha = 140_u8;
            let put = |x: usize, y: usize| {
                let pixel = self.bits.add((y * width + x) * 4);
                *pixel = alpha;
                *pixel.add(1) = alpha;
                *pixel.add(2) = alpha;
                *pixel.add(3) = alpha;
            };
            for x in 0..width {
                put(x, 0);
                put(x, height - 1);
            }
            for y in 0..height {
                put(0, y);
                put(width - 1, y);
            }
        }
    }

    impl Drop for Canvas {
        fn drop(&mut self) {
            unsafe {
                // 顺序要紧：还选在 DC 里的位图 `DeleteObject` 会被拒（这就是每帧泄漏的根）。
                SelectObject(self.dc, self.dc_placeholder);
                SelectObject(self.mask_dc, self.mask_placeholder);
                DeleteObject(self.bitmap as HGDIOBJ);
                DeleteObject(self.mask as HGDIOBJ);
                DeleteDC(self.dc);
                DeleteDC(self.mask_dc);
            }
        }
    }

    /// 把蒙版按覆盖率 over 合成到预乘 BGRA 缓冲上（`dst = color*α + dst*(1-α)`）。
    unsafe fn composite(mask: *const u8, dst: *mut u8, pixels: usize, color: (u8, u8, u8)) {
        for index in 0..pixels {
            let coverage = *mask.add(index * 4) as u32;
            if coverage == 0 {
                continue;
            }
            let inverse = 255 - coverage;
            let pixel = dst.add(index * 4);
            let blend = |channel: u8, value: u8| {
                ((value as u32 * coverage + channel as u32 * inverse) / 255) as u8
            };
            *pixel = blend(*pixel, color.2);
            *pixel.add(1) = blend(*pixel.add(1), color.1);
            *pixel.add(2) = blend(*pixel.add(2), color.0);
            *pixel.add(3) = (coverage + *pixel.add(3) as u32 * inverse / 255).min(255) as u8;
        }
    }
}

/// macOS：无边框 `NSPanel` + 三个透明 `NSTextField`，主线程每帧手动泵 AppKit 事件。
///
/// 三件事值得记住：
/// - 窗口必须在**主线程**建（AppKit 的硬要求）：`install` 第一句就是 `MainThreadMarker::new()`，
///   拿不到就降级报错。`objc2` 的类型系统也替我们守着这条（`sharedApplication`、`alloc` 都要标记）。
/// - 没有 `NSApplicationMain`，事件得自己拉：`sync` 每帧调 `pump`。CFRunLoop 泵不出 AppKit 的
///   事件（实测），`NSApp.run()` 会阻塞到退出，塞不进「画帧 + 读键盘」。
/// - 状态文件存的是**左上原点**的中心点（与 Windows / 旧数据同一口径），进出 AppKit 按主屏高换算。
#[cfg(target_os = "macos")]
mod imp {
    use std::sync::mpsc::{Receiver, Sender, channel};

    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{AllocAnyThread, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSFont,
        NSFontAttributeName, NSForegroundColorAttributeName, NSEventMask, NSPanel, NSScreen,
        NSStrokeColorAttributeName, NSStrokeWidthAttributeName, NSTextField, NSView,
        NSWindowCollectionBehavior, NSWindowStyleMask,
    };
    use objc2_foundation::{
        NSAttributedString, NSDate, NSDefaultRunLoopMode, NSDictionary, NSNumber, NSPoint, NSRect,
        NSSize, NSString,
    };

    use crate::config::Rgb;

    use super::{DesktopLyricsConfig, Payload};

    /// 次要行（译文 / 下一句）与正文的字号比：与 Windows 的 `small_size` 同一比例。
    const SMALL_RATIO: f64 = 3.0 / 5.0;
    /// 文字四周的留白（Windows 是 8 逻辑像素 × DPI；macOS 直接用点）。
    const PAD: f64 = 8.0;
    /// 描边粗细：**负值 = 填充 + 描边一次画完**（Windows 是八方向各画一遍再合成）。
    /// 观感常数，真机对着 Windows 调。
    const OUTLINE_MAIN: f64 = -3.0;
    const OUTLINE_SMALL: f64 = -2.0;
    /// 浮层高度：桌面之上的浮动层（`CGWindowList` 里能看到 layer=3）。
    const LEVEL: isize = 3;

    pub(super) type Started = (Imp, Option<Receiver<(i32, i32)>>, Option<String>);

    pub(super) struct Imp {
        /// `None` = 没有窗口（关掉功能 / 建不出来 / `App::new` 与测试）。
        inner: Option<Inner>,
    }

    struct Inner {
        panel: Retained<NSPanel>,
        /// 三行文本控件：当前句 / 译文 / 下一句。
        current: Retained<NSTextField>,
        translation: Retained<NSTextField>,
        next: Retained<NSTextField>,
        /// 配置在进程里是固定的，建窗口时拿一份（与 Windows 的 `Shared` 同一个理由）。
        config: DesktopLyricsConfig,
        /// 拖动结束后的新中心点回传给主线程（落盘在 `tui/state.rs`）。
        positions: Sender<(i32, i32)>,
        /// 中心点（左上原点）：既是重画时的摆位依据，也是报给主线程落盘的值。
        reported: (i32, i32),
        /// 主屏高：左上原点 ↔ AppKit 左下原点换算用。
        screen_height: f64,
        /// 现在是 `orderOut` 状态吗（避免每帧重复调）。
        hidden: bool,
        /// 现在是锁定态吗（跟随 `push` 的 payload）。锁定态窗口动不了，`pump` 连矩形都不量
        /// ——量回来的中心带着 `setFrame` 整点吸附的舍入，误当成拖动就会让位置自己飘。
        locked: bool,
    }

    impl Imp {
        pub(super) fn none() -> Self {
            Self { inner: None }
        }

        /// 把事件队列拉空并派发。每帧都跑，哪怕 payload 没变。
        pub(super) fn pump(&mut self) {
            let Some(inner) = &mut self.inner else {
                return;
            };
            inner.pump();
        }

        pub(super) fn push(&mut self, payload: Payload) {
            let Some(inner) = &mut self.inner else {
                return;
            };
            inner.push(&payload);
        }

        /// panel 还在才算开着（拆掉 = `inner` 变回 `None`）。
        pub(super) fn running(&self) -> bool {
            self.inner.is_some()
        }
    }

    impl Inner {
        fn pump(&mut self) {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let app = NSApplication::sharedApplication(mtm);
            loop {
                // SAFETY: 主线程；`distantPast` = 队列空就立刻返回，不阻塞界面。
                let event = unsafe {
                    app.nextEventMatchingMask_untilDate_inMode_dequeue(
                        NSEventMask::Any,
                        Some(&NSDate::distantPast()),
                        NSDefaultRunLoopMode,
                        true,
                    )
                };
                let Some(event) = event else { break };
                app.sendEvent(&event);
            }

            // 拖动：解锁态窗口被系统拖走，位置以**窗口自己**为准。每帧读一次真实矩形，
            // 跟上次报过的比——变了才回传（不指望「拖动结束」事件：Windows 那边踩过
            // `WM_EXITSIZEMOVE` 进不了消息队列的坑，改成逐 draw 采纳窗口自身的矩形）。
            //
            // 锁定态窗口动不了（点不着，也没有别的东西会挪它），干脆不读：`setFrame` 会把原点
            // **吸附到整点**，量回来的中心天生带 ±0.5 的舍入，当成「挪了」写进状态文件就会累积。
            if self.locked {
                return;
            }
            let center = self.window_center();
            // 1 点以内的差就是上面那个吸附的舍入（量回来最多差 1 点），不是拖动：真拖动是逐像素的，
            // 而这个回路的每一轮都会把那 1 点加上去——几次开关/解锁，窗口就自己往下飘。
            if (center.0 - self.reported.0).abs() <= 1 && (center.1 - self.reported.1).abs() <= 1 {
                return;
            }
            self.reported = center;
            // 通道那头没了（主线程收摊）就算了，不影响界面。
            let _ = self.positions.send(center);
        }

        /// 窗口自己的中心点（左上原点）。给 `pump` 判断「窗口被挪了没有」用（原点被整点吸附过，
        /// 所以这里量出来的值天生带 ±0.5 的误差）。
        fn window_center(&self) -> (i32, i32) {
            let frame = self.panel.frame();
            (
                (frame.origin.x + frame.size.width / 2.0).round() as i32,
                (self.screen_height - (frame.origin.y + frame.size.height / 2.0)).round() as i32,
            )
        }

        /// 一帧 payload → 窗口。内容没变化时 `State::advance` 根本不会调到这里。
        fn push(&mut self, payload: &Payload) {
            // 锁定态先落定，再管可见性：隐藏分支会提前返回，别让它把「点不着 / 点得着」漏过去
            // （隐藏窗口本来收不到点击，但不变式写在这儿更清楚）。锁定 = 点不着，解锁 = 收鼠标
            // ——于是 `setMovableByWindowBackground` 才能让系统把它拖走。传的就是 `locked` 本身；
            // 这行写反过一次（`!locked`），症状是「锁定态点得着、解锁后拖不动」。
            self.panel.setIgnoresMouseEvents(payload.locked);
            self.locked = payload.locked;

            // 隐藏：`orderOut` 而不是销毁（位置与锁定态留着），下次 `orderFrontRegardless` 回来。
            if !payload.visible {
                for field in [&self.current, &self.translation, &self.next] {
                    field.setHidden(true);
                }
                if !self.hidden {
                    self.panel.orderOut(None);
                    self.hidden = true;
                }
                return;
            }

            let main_size = f64::from(self.config.font_size);
            let small_size = (main_size * SMALL_RATIO).max(8.0);
            let (color, next_color, outline) =
                (self.config.color, self.config.next_color, self.config.outline);
            set_line(
                &self.current,
                &payload.current,
                main_size,
                color,
                outline,
                &self.config,
                OUTLINE_MAIN,
            );
            set_line(
                &self.translation,
                payload.translation.as_deref().unwrap_or(""),
                small_size,
                next_color,
                outline,
                &self.config,
                OUTLINE_SMALL,
            );
            set_line(
                &self.next,
                &payload.next,
                small_size,
                next_color,
                outline,
                &self.config,
                OUTLINE_SMALL,
            );

            // 量尺寸：宽度取最宽一行、高度是各行之和，再加内边距（与 Windows 同一算法）。
            let rows: Vec<&Retained<NSTextField>> = [&self.current, &self.translation, &self.next]
                .into_iter()
                .filter(|field| !field.isHidden())
                .collect();
            let width = rows.iter().map(|field| field.frame().size.width).fold(0.0_f64, f64::max)
                + PAD * 2.0;
            let height = rows.iter().map(|field| field.frame().size.height).sum::<f64>() + PAD * 2.0;

            // 位置以**我们记着的中心点**（`reported`）为准：建窗口时给的那个，或解锁态拖动跟出来的
            // 那个。不去读窗口自己的矩形再摆回去——`setFrame` 的整点吸附会让这个回路每转一圈多挪
            // 一点（见 `pump` 里那段）；拖动照样跟手，因为 `pump` 会把窗口的位移采纳进来。
            let center = self.reported;
            self.panel.setAlphaValue(f64::from(self.config.opacity) / 100.0);
            self.panel.setFrame_display(
                NSRect::new(
                    NSPoint::new(
                        f64::from(center.0) - width / 2.0,
                        self.screen_height - f64::from(center.1) - height / 2.0,
                    ),
                    NSSize::new(width, height),
                ),
                true,
            );

            // 每行水平居中，从上往下摆（AppKit 是左下原点：y 从顶边往下减）。
            let mut y = height - PAD;
            for field in &rows {
                let size = field.frame().size;
                y -= size.height;
                field.setFrame(NSRect::new(
                    NSPoint::new(PAD + (width - PAD * 2.0 - size.width) / 2.0, y),
                    size,
                ));
            }
            // 从 `orderOut` 回来（第一次显示也走这里）；`orderFrontRegardless` 不激活自己。
            self.panel.orderFrontRegardless();
            self.hidden = false;
        }
    }

    impl Drop for Inner {
        fn drop(&mut self) {
            // 只 release 窗口对象**不会**把它从屏幕上拿走：窗口上屏期间 AppKit 自己持有它，
            // 实测「release 不 close」的 panel 留在屏上，而 Rust 这边已经当它没了（`is_running()`
            // 变 `false`、`sync` 整段不再进）——症状就是「热键关不掉、窗口冻住不再换词」。
            // 拆窗口必须显式 close（Windows 那边对应的是 `WM_CLOSE` → `DestroyWindow`）。
            self.panel.close();
        }
    }

    /// 还没有真 payload 时先进去的那一帧：窗口建出来先藏着。
    fn hidden_payload() -> Payload {
        Payload {
            visible: false,
            current: String::new(),
            next: String::new(),
            translation: None,
            locked: true,
        }
    }

    /// `Rgb`（0-255）→ `NSColor`。
    fn ns_color(color: Rgb) -> Retained<NSColor> {
        NSColor::colorWithSRGBRed_green_blue_alpha(
            f64::from(color.r) / 255.0,
            f64::from(color.g) / 255.0,
            f64::from(color.b) / 255.0,
            1.0,
        )
    }

    /// 配置里的字体面认不出来（macOS 上默认那串就是 Windows 字体名）就回退系统字体。
    fn font_for(name: &str, size: f64) -> Retained<NSFont> {
        NSFont::fontWithName_size(&NSString::from_str(name), size)
            .unwrap_or_else(|| NSFont::systemFontOfSize(size))
    }

    /// `Retained<T>` **不是** `repr(transparent)`：`&retained` 不是对象指针（直接当 `AnyObject`
    /// 用会段错误，探针里踩过），必须 `&*retained` 再转。
    unsafe fn any<T>(object: &T) -> &AnyObject {
        unsafe { &*(object as *const T as *const AnyObject) }
    }

    /// 给一行设文本与样式。空文本 = 这行不画（`setHidden`），也就不参与量尺寸。
    fn set_line(
        field: &NSTextField,
        text: &str,
        size: f64,
        color: Rgb,
        outline: Rgb,
        config: &DesktopLyricsConfig,
        stroke_width: f64,
    ) {
        field.setHidden(text.is_empty());
        if text.is_empty() {
            return;
        }
        let font = font_for(&config.font, size);
        let text_color = ns_color(color);
        let outline_color = ns_color(outline);
        let stroke = NSNumber::numberWithDouble(stroke_width);
        // SAFETY: 属性名是 AppKit 导出的常量，值的类型与属性名要求的一致。
        let attributes = NSDictionary::from_slices(
            &[
                unsafe { NSFontAttributeName },
                unsafe { NSForegroundColorAttributeName },
                unsafe { NSStrokeColorAttributeName },
                unsafe { NSStrokeWidthAttributeName },
            ],
            &[
                unsafe { any(&*font) },
                unsafe { any(&*text_color) },
                unsafe { any(&*outline_color) },
                unsafe { any(&*stroke) },
            ],
        );
        // SAFETY: 属性键都是 AppKit 的常量，字典是我们刚建的。
        let attributed = unsafe {
            NSAttributedString::initWithString_attributes(
                NSAttributedString::alloc(),
                &NSString::from_str(text),
                Some(&attributes),
            )
        };
        field.setAttributedStringValue(&attributed);
        field.sizeToFit();
    }

    /// 建那个浮层窗口：无边框、透明、不抢焦点、不进 Dock、各 space 都在场。
    fn make_panel(mtm: MainThreadMarker) -> Retained<NSPanel> {
        // 先给个 1×1 的框；位置与尺寸随后由 `push` 按文本摆。
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0)),
            NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
            NSBackingStoreType::Buffered,
            false,
        );
        panel.setTitle(&NSString::from_str("canto lyrics"));
        panel.setLevel(LEVEL);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setHasShadow(false);
        // 启动一律锁定：点不着（锁定态刻意不持久化）。
        panel.setIgnoresMouseEvents(true);
        // 解锁后拖动交给系统自己拖（见 Task 3）。
        panel.setMovableByWindowBackground(true);
        // 关窗的释放权归 Rust 的 `Retained`（见 `Drop for Inner`）：显式关掉「关闭时自己 release」
        // 这条，免得 `close()` 与 `Retained` 各 release 一次。
        // SAFETY: panel 是主线程上刚建的，这个句柄是它的唯一所有者。
        unsafe { panel.setReleasedWhenClosed(false) };
        panel
    }

    /// 一行文本控件：透明、只读、不可选。
    fn make_label(mtm: MainThreadMarker) -> Retained<NSTextField> {
        let field = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        field.setBezeled(false);
        field.setDrawsBackground(false);
        field.setEditable(false);
        field.setSelectable(false);
        field.setHidden(true);
        field
    }

    pub(super) fn install(config: &DesktopLyricsConfig, center: (i32, i32)) -> Started {
        let Some(mtm) = MainThreadMarker::new() else {
            tracing::warn!("桌面歌词要在主线程建窗口");
            return (Imp::none(), None, Some("歌词窗口起不来".to_string()));
        };
        // 附属进程：不进 Dock、不抢别人的焦点。
        let app = NSApplication::sharedApplication(mtm);
        // 幂等：**已经**是 Accessory 时 `setActivationPolicy` 返回 `false`——那是「没变化」，不是失败
        // （实测第 2 次起必 `false`）。策略是进程级的，装机第一次真生效就够，所以这里只挡
        // 「想设却没设上」；不这么写的话每次热键重开窗口都会撞在这儿报「歌词窗口起不来」。
        if app.activationPolicy() != NSApplicationActivationPolicy::Accessory
            && !app.setActivationPolicy(NSApplicationActivationPolicy::Accessory)
        {
            tracing::warn!("设置 Accessory 激活策略失败");
            return (Imp::none(), None, Some("歌词窗口起不来".to_string()));
        }
        let screen_height = NSScreen::mainScreen(mtm).map_or(0.0, |screen| screen.frame().size.height);
        let (positions, places) = channel::<(i32, i32)>();

        let panel = make_panel(mtm);
        let (current, translation, next) = (make_label(mtm), make_label(mtm), make_label(mtm));
        // 内容视图装三行 label。
        let view = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0)),
        );
        view.addSubview(&current);
        view.addSubview(&translation);
        view.addSubview(&next);
        panel.setContentView(Some(&view));

        let mut inner = Inner {
            panel,
            current,
            translation,
            next,
            config: config.clone(),
            positions,
            reported: center,
            screen_height,
            hidden: false,
            // 启动一律锁定（`make_panel` 里也是这么设的）。
            locked: true,
        };
        // 先把窗口摆到记下来的中心点（首次 `push` 会按这个中心重新量尺寸；锁定态用的就是它）。
        inner.panel.setFrame_display(
            NSRect::new(
                NSPoint::new(
                    f64::from(center.0) - 50.0,
                    screen_height - f64::from(center.1) - 20.0,
                ),
                NSSize::new(100.0, 40.0),
            ),
            false,
        );
        // 第一帧就是「隐藏」：窗口建出来是空的，等第一次真 payload。
        inner.push(&hidden_payload());
        // 安装记录在这儿打（只有真建了窗口的平台才该说这句）。
        tracing::info!(font = %config.font, size = config.font_size, "桌面歌词窗口已就绪");
        (Imp { inner: Some(inner) }, Some(places), None)
    }
}

/// 其它平台（Linux / Wayland 等，macOS 见上面那份）：不建窗口，什么都不做。这份之后不再改。
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod imp {
    use std::sync::mpsc::Receiver;

    use super::{DesktopLyricsConfig, Payload};

    pub(super) struct Imp;

    impl Imp {
        pub(super) fn none() -> Self {
            Self
        }

        pub(super) fn push(&mut self, _payload: Payload) {}

        /// 不建窗口，没什么可泵的。
        pub(super) fn pump(&mut self) {}

        /// 这个平台永远没有窗口，所以永远是「没开着」（`sync` 也就什么都不做）。
        pub(super) fn running(&self) -> bool {
            false
        }
    }

    /// 平台实现的回报：句柄、位置通道（拖完回传中心点）、失败原因。
    pub(super) type Started = (Imp, Option<Receiver<(i32, i32)>>, Option<String>);

    pub(super) fn install(_config: &DesktopLyricsConfig, _center: (i32, i32)) -> Started {
        (Imp::none(), None, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DesktopLyricsConfig;
    use crate::model::{Lyric, LyricLine};
    use crate::player::{Phase, Snapshot};
    use std::time::Duration;

    /// 每句隔 10 秒——`line_at(position)` 就是按这个时间找当前句的。
    fn lyric(lines: &[(&str, Option<&str>)]) -> Lyric {
        Lyric {
            lines: lines
                .iter()
                .enumerate()
                .map(|(index, (text, translation))| LyricLine {
                    at: Duration::from_secs(index as u64 * 10),
                    text: text.to_string(),
                    translation: translation.map(str::to_string),
                })
                .collect(),
        }
    }

    fn snapshot_at(phase: Phase, secs: u64) -> Snapshot {
        Snapshot {
            title: None,
            mid: None,
            quality: None,
            size: None,
            phase,
            position: Duration::from_secs(secs),
            duration: Some(Duration::from_secs(600)),
            volume: 100,
            just_finished: false,
        }
    }

    /// 同一帧第二次不该再推——这就是「没有逐帧开销」的判据。
    #[test]
    fn the_same_frame_twice_pushes_once() {
        let lines = lyric(&[("第一句", None), ("第二句", None)]);
        let snapshot = snapshot_at(Phase::Playing, 15);
        let frame = Frame { lyric: Some(&lines), snapshot: &snapshot };

        let mut state = State::default();
        assert!(state.advance(frame, &DesktopLyricsConfig::default()).is_some(), "第一次该推");
        assert!(state.advance(frame, &DesktopLyricsConfig::default()).is_none(), "同一帧第二次不该再推");
    }

    /// 同一句里推进位置（进度条在动）不算变化。
    #[test]
    fn position_within_the_same_line_does_not_repush() {
        let lines = lyric(&[("第一句", None), ("第二句", None)]);
        let mut state = State::default();

        let early = snapshot_at(Phase::Playing, 10);
        let frame = Frame { lyric: Some(&lines), snapshot: &early };
        assert!(state.advance(frame, &DesktopLyricsConfig::default()).is_some());

        let later = snapshot_at(Phase::Playing, 19);
        let frame = Frame { lyric: Some(&lines), snapshot: &later };
        assert!(state.advance(frame, &DesktopLyricsConfig::default()).is_none(), "还在第二句里");
    }

    /// 换行时当前句与下一句都要对（下一句过界就是空串）。
    #[test]
    fn a_new_line_pushes_current_and_next() {
        let lines = lyric(&[("第一句", None), ("第二句", None), ("第三句", None)]);
        let snapshot = snapshot_at(Phase::Playing, 25);
        let frame = Frame { lyric: Some(&lines), snapshot: &snapshot };

        let mut state = State::default();
        let payload = state.advance(frame, &DesktopLyricsConfig::default()).expect("换行该推");
        assert_eq!(payload.current, "第三句");
        assert_eq!(payload.next, "", "最后一句之后没有下一句");
        assert!(payload.visible);
    }

    /// 纯音乐段（当前行是空文本）只显示下一句，不留空框。
    #[test]
    fn an_empty_line_shows_only_the_next_one() {
        let lines = lyric(&[("", None), ("下一句", None)]);
        let snapshot = snapshot_at(Phase::Playing, 5);
        let frame = Frame { lyric: Some(&lines), snapshot: &snapshot };

        let mut state = State::default();
        let payload = state.advance(frame, &DesktopLyricsConfig::default()).expect("该推");
        assert_eq!(payload.current, "", "这一句本来就是空的");
        assert_eq!(payload.next, "下一句");
        assert!(payload.visible, "有下一句可显示，窗口要留着");
    }

    /// 没在播就整窗隐藏——不显示上一句的残影（隐藏时连文案都不带）。
    #[test]
    fn nothing_playing_hides_the_overlay() {
        let lines = lyric(&[("第一句", None)]);
        let config = DesktopLyricsConfig::default();

        // 播放中 → 取流全挂：真实场景，要推一次「隐藏」。
        let mut state = State::default();
        let playing = snapshot_at(Phase::Playing, 5);
        let frame = Frame { lyric: Some(&lines), snapshot: &playing };
        assert!(state.advance(frame, &config).expect("该推").visible);

        let failed = snapshot_at(Phase::Failed("全挂了".to_string()), 5);
        let frame = Frame { lyric: Some(&lines), snapshot: &failed };
        assert!(!state.advance(frame, &config).expect("该推").visible, "取流全挂要收起来");

        // 空转（一直没在播）：隐藏态之间没有变化，不该反复推。
        let mut state = State::default();
        let idle = snapshot_at(Phase::Idle, 5);
        let frame = Frame { lyric: Some(&lines), snapshot: &idle };
        let payload = state.advance(frame, &config).expect("第一次要推一次隐藏");
        assert!(!payload.visible);
        assert!(payload.current.is_empty(), "隐藏时不留文案，免得下次显示先闪旧句");
        assert!(state.advance(frame, &config).is_none(), "还是隐藏，不用再推");
    }

    /// 暂停默认留着（想隐藏就配 `hide_when_paused`）。
    #[test]
    fn pausing_keeps_the_line_unless_configured() {
        let lines = lyric(&[("第一句", None)]);
        let snapshot = snapshot_at(Phase::Paused, 5);
        let frame = Frame { lyric: Some(&lines), snapshot: &snapshot };

        let mut state = State::default();
        assert!(state.advance(frame, &DesktopLyricsConfig::default()).expect("该推").visible);

        let mut state = State::default();
        let config = DesktopLyricsConfig { hide_when_paused: true, ..DesktopLyricsConfig::default() };
        assert!(!state.advance(frame, &config).expect("该推").visible);
    }

    /// 解锁且没歌词时强制可见并给提示语——否则解锁了却找不到窗口，没法拖。
    #[test]
    fn unlocking_forces_the_overlay_visible() {
        let mut state = State::default();
        state.toggle_lock();

        let snapshot = snapshot_at(Phase::Idle, 0);
        let frame = Frame { lyric: None, snapshot: &snapshot };
        let payload = state
            .advance(frame, &DesktopLyricsConfig::default())
            .expect("锁定态变了就该推");
        assert!(payload.visible);
        assert!(!payload.locked);
        assert_eq!(payload.current, UNLOCKED_HINT);
    }

    /// 译文跟着开关走。
    #[test]
    fn the_translation_follows_the_switch() {
        let lines = lyric(&[("第一句", Some("first"))]);
        let snapshot = snapshot_at(Phase::Playing, 5);
        let frame = Frame { lyric: Some(&lines), snapshot: &snapshot };

        let mut state = State::default();
        let payload = state.advance(frame, &DesktopLyricsConfig::default()).expect("该推");
        assert_eq!(payload.translation.as_deref(), Some("first"));

        let mut state = State::default();
        let config = DesktopLyricsConfig { show_translation: false, ..DesktopLyricsConfig::default() };
        let payload = state.advance(frame, &config).expect("该推（配置不同→上次推的不一样）");
        assert_eq!(payload.translation, None);
    }

    /// 空句柄（关掉功能 / 非 Windows / 测试）：sync 与 toggle_lock 都是空操作。
    #[test]
    fn a_none_handle_syncs_nothing() {
        let lines = lyric(&[("第一句", None)]);
        let snapshot = snapshot_at(Phase::Playing, 5);
        let frame = Frame { lyric: Some(&lines), snapshot: &snapshot };

        let mut overlay = DesktopLyrics::none();
        overlay.sync(frame, &DesktopLyricsConfig::default());
        overlay.toggle_lock();
        assert!(!overlay.locked(), "空句柄不记得锁定态");
    }

    /// 换过显示器/分辨率后，屏外的中心点要夹回主屏内（留边距）；默认位是底部居中。
    #[test]
    fn a_saved_center_is_clamped_onto_the_primary_screen() {
        assert_eq!(clamp_center((-5000, -5000), (1920, 1080)), (MARGIN, MARGIN));
        assert_eq!(clamp_center((99999, 99999), (1920, 1080)), (1920 - MARGIN, 1080 - MARGIN));
        assert_eq!(clamp_center((800, 500), (1920, 1080)), (800, 500), "本来就在屏内就不动");
        assert_eq!(default_center((1920, 1080)), (960, 918), "主屏底部居中");
    }

    #[test]
    fn the_state_file_round_trips() {
        let dir = std::env::temp_dir().join(format!("canto-lyrics-state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let path = dir.join("desktop-lyrics.json");

        let state = SavedState { shown: Some(false), center: Some((123, 456)) };
        save_state(&path, state).expect("落盘");
        assert_eq!(load_state(&path), state);

        // 关着、也从没拖过：文件里只该有开关，不该凭空记一个位置。
        let state = SavedState { shown: Some(true), center: None };
        save_state(&path, state).expect("落盘");
        assert_eq!(load_state(&path), state);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 文件不在 / 坏 JSON / 只写了半个坐标，一律当「什么都没记」；老文件（只有中心点）要读得回来
    /// —— 那正是升级前的那一份。
    #[test]
    fn a_missing_or_broken_state_file_is_no_opinion() {
        let dir = std::env::temp_dir().join(format!("canto-lyrics-broken-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let path = dir.join("desktop-lyrics.json");

        assert_eq!(load_state(&path), SavedState::default(), "文件不在");
        std::fs::write(&path, "{ 不是 JSON").expect("写坏文件");
        assert_eq!(load_state(&path), SavedState::default(), "坏文件当没有");
        std::fs::write(&path, r#"{"shown":false,"cx":5}"#).expect("写半边");
        assert_eq!(
            load_state(&path),
            SavedState { shown: Some(false), center: None },
            "半个坐标当没记"
        );
        std::fs::write(&path, r#"{"cx":11,"cy":22}"#).expect("写老文件");
        assert_eq!(load_state(&path), SavedState { shown: None, center: Some((11, 22)) });
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 启动时开不开：状态文件优先（缺 `shown` = 还没记过），其次老配置里遗留的 `enabled`。
    #[test]
    fn the_startup_switch_prefers_the_file_then_the_legacy_key() {
        assert!(start_shown(Some(true), Some(false)), "文件说了算");
        assert!(!start_shown(Some(false), Some(true)), "文件说了算");
        assert!(!start_shown(None, Some(false)), "没记过才轮到老配置里的 enabled");
        assert!(start_shown(None, None), "都没有就默认开（跟老 enabled = true 一个口径）");
    }

    /// 没窗口的时候连 payload 都不造——「关掉就零成本」这条性质靠它守着。
    #[test]
    fn a_stopped_handle_neither_runs_nor_builds_payloads() {
        let lines = lyric(&[("第一句", None)]);
        let snapshot = snapshot_at(Phase::Playing, 5);
        let frame = Frame { lyric: Some(&lines), snapshot: &snapshot };

        let mut overlay = DesktopLyrics::none();
        assert!(!overlay.is_running());
        overlay.sync(frame, &DesktopLyricsConfig::default());
        assert!(overlay.state.last.is_none(), "关着的时候不该造 payload");
    }

    /// 关掉要落一次盘（开关是状态，得记住）；同一个状态只说一次。
    #[test]
    fn stopping_writes_the_off_choice_once() {
        let mut overlay = DesktopLyrics::none();
        assert!(overlay.take_state().is_none(), "没变过就不用落盘");

        overlay.adopt_window((123, 456), None);
        assert!(overlay.take_state().is_none(), "建窗口本身不用落盘");

        overlay.disable();
        assert_eq!(
            overlay.take_state(),
            Some(SavedState { shown: Some(false), center: Some((123, 456)) }),
            "关掉要落一次，且位置留着（下次开回来还在原地）"
        );
        assert!(overlay.take_state().is_none(), "同一个状态只说一次");
        assert!(!overlay.is_running());

        overlay.toggle_lock();
        assert_eq!(overlay.take_state(), None, "锁定态不落盘（每次启动都是锁的）");
    }

    /// 拆了再建：`State` 必须回到初始态。它记着上一份窗口的 payload，不清的话新窗口第一帧
    /// `advance()` 会判定「没变化」而什么都不推——窗口永远空着。
    #[test]
    fn re_enabling_forgets_the_previous_window() {
        let mut overlay = DesktopLyrics::none();
        overlay.state.last = Some(Payload {
            visible: true,
            current: "上一句".to_string(),
            next: String::new(),
            translation: None,
            locked: true,
        });
        overlay.toggle_lock();
        assert!(!overlay.locked());

        overlay.adopt_window((10, 20), None);
        assert!(overlay.state.last.is_none(), "新窗口没有旧 payload 可比");
        assert!(overlay.locked(), "拆了再建回到锁定态");
        assert_eq!(overlay.center, Some((10, 20)));
    }

    /// 拖动回传的位置取最后一条（中间那些没人看，反正写的是同一个文件）。
    #[test]
    fn dragged_centers_are_coalesced_into_one_write() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut overlay = DesktopLyrics::none();
        overlay.shown = true;
        overlay.adopt_window((0, 0), Some(rx));

        tx.send((10, 20)).expect("回传");
        tx.send((11, 21)).expect("回传");
        overlay.collect_centers();
        assert_eq!(
            overlay.take_state(),
            Some(SavedState { shown: Some(true), center: Some((11, 21)) })
        );

        overlay.collect_centers();
        assert!(overlay.take_state().is_none(), "没有新位置就别再写一遍");
    }

    /// 取流中（`Buffering`）锁定态变了要**立刻**推：否则按了解锁，窗口要等取流完才有反应
    /// ——用户只会以为「功能坏了」。
    #[test]
    fn a_lock_flip_during_buffering_is_pushed_right_away() {
        let lines = lyric(&[("第一句", None)]);
        let config = DesktopLyricsConfig::default();
        let mut state = State::default();

        // 先推一帧正常的，让窗口里有东西。
        let playing = snapshot_at(Phase::Playing, 5);
        let frame = Frame { lyric: Some(&lines), snapshot: &playing };
        assert!(state.advance(frame, &config).expect("该推").visible);

        // 取流中：内容沿用上一次，不推。
        let buffering = snapshot_at(Phase::Buffering, 5);
        let frame = Frame { lyric: Some(&lines), snapshot: &buffering };
        assert!(state.advance(frame, &config).is_none(), "取流中别闪");

        // 但锁定态变了就得推，而且推出去的就是新锁定态。
        state.toggle_lock();
        let payload = state.advance(frame, &config).expect("解锁要立刻推，不然摸不到窗口");
        assert!(!payload.locked, "推出去的该是新锁定态");
        assert!(payload.visible, "解锁态强制可见");

        // 再算一次：没有新变化，不该重复推。
        assert!(state.advance(frame, &config).is_none(), "没变化别重复推");
    }
}

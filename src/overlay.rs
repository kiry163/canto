//! 桌面歌词：取词、状态机与窗口位置（平台无关的那一半）。
//!
//! 平台相关的部分在下面 `cfg` 子模块里：Windows 是「专用线程 + 分层窗口 + DIB 绘制」，
//! 其它平台是空实现。状态机是两半共用的——将来写 macOS 那份只需要补窗口与画字，
//! `Frame` / `Payload` / 位置状态一行都不用动。

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
        // 取流中沿用上一次的样子，别闪。
        if matches!(frame.snapshot.phase, Phase::Buffering) {
            return None;
        }

        let (mut current, mut next, mut translation) = lines(frame, config);
        let has_something = !current.is_empty() || !next.is_empty() || translation.is_some();

        let playing = match frame.snapshot.phase {
            Phase::Playing => true,
            Phase::Paused => !config.hide_when_paused,
            // `Idle` / `Failed`（`Buffering` 在上面已经返回）
            _ => false,
        };
        let mut visible = playing && has_something;
        if !visible {
            // 隐藏时不带文案：免得下次显示时先闪一下旧句。
            current.clear();
            next.clear();
            translation = None;
        }

        // 解锁时强制可见：否则解锁了却找不到窗口，没法拖。
        if !self.locked {
            visible = true;
            if current.is_empty() && next.is_empty() && translation.is_none() {
                current = UNLOCKED_HINT.to_string();
            }
        }

        let payload = Payload { visible, current, next, translation, locked: self.locked };
        if self.last.as_ref() == Some(&payload) {
            return None;
        }
        self.last = Some(payload.clone());
        Some(payload)
    }
}

/// 当前句、下一句（过界就是空串）、当前句的译文（配置关掉就是 `None`）。
fn lines(frame: Frame<'_>, config: &DesktopLyricsConfig) -> (String, String, Option<String>) {
    let Some(lyric) = frame.lyric else {
        return (String::new(), String::new(), None);
    };
    let index = lyric.line_at(frame.snapshot.position);
    let line = index.and_then(|index| lyric.lines.get(index));
    let current = line.map(|line| line.text.clone()).unwrap_or_default();
    let next = index
        .and_then(|index| lyric.lines.get(index + 1))
        .map(|line| line.text.clone())
        .unwrap_or_default();
    let translation = config
        .show_translation
        .then(|| line.and_then(|line| line.translation.clone()))
        .flatten();
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

/// 主屏尺寸（宽、高）。
#[cfg(target_os = "windows")]
fn primary_screen() -> (i32, i32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    // SAFETY: 只读两个系统指标，没有所有权问题。
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}

#[cfg(not(target_os = "windows"))]
fn primary_screen() -> (i32, i32) {
    // 空实现那份用不到；给个确定的数，好让测试与编译都稳。
    (1920, 1080)
}

/// 位置状态文件的内容。
#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct Center {
    cx: i32,
    cy: i32,
}

/// 读窗口中心点；文件不在或读坏了就是 `None`（调用方退回默认位置）。
pub fn load_center(path: &Path) -> Option<(i32, i32)> {
    let text = std::fs::read_to_string(path).ok()?;
    let center: Center = serde_json::from_str(&text).ok()?;
    Some((center.cx, center.cy))
}

/// 写窗口中心点（临时文件 + rename，中途失败不留坏文件——跟 `history.rs` 一个套路）。
pub fn save_center(path: &Path, center: (i32, i32)) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(parent)
        .with_context(|| format!("创建配置目录失败：{}", parent.display()))?;
    let text = serde_json::to_string(&Center { cx: center.0, cy: center.1 })?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text).with_context(|| format!("写 {} 失败", temp.display()))?;
    std::fs::rename(&temp, path).with_context(|| format!("替换 {} 失败", path.display()))
}

/// 桌面歌词的句柄。
pub struct DesktopLyrics {
    state: State,
    imp: imp::Imp,
}

/// 装出来的东西。
pub struct Installed {
    pub overlay: DesktopLyrics,
    /// 拖动结束时窗口线程发过来的新中心点（非 Windows、或关掉功能时是 `None`）。
    pub positions: Option<Receiver<(i32, i32)>>,
    /// `Some` = 起不来（调用方报一句 + 日志），此时 `overlay` 是空句柄。
    pub error: Option<String>,
}

impl DesktopLyrics {
    /// 空句柄：关掉功能、非 Windows、`App::new` 与测试都用它；`sync`/`toggle_lock` 是空操作。
    pub fn none() -> Self {
        Self { state: State::default(), imp: imp::Imp::none() }
    }

    /// 按配置与已存的中心点起覆盖层。
    pub fn install(config: &DesktopLyricsConfig, saved: Option<(i32, i32)>) -> Installed {
        if !config.enabled {
            return Installed { overlay: Self::none(), positions: None, error: None };
        }
        // 存过中心点就用它（顺手夹回主屏内）；没有就用默认的底部居中。
        let screen = primary_screen();
        let center =
            saved.map_or_else(|| default_center(screen), |center| clamp_center(center, screen));
        let (imp, positions, error) = imp::install(config, center);
        Installed { overlay: Self { state: State::default(), imp }, positions, error }
    }

    /// 每帧调一次；只有变化才会真的推给窗口。
    pub fn sync(&mut self, frame: Frame<'_>, config: &DesktopLyricsConfig) {
        if let Some(payload) = self.state.advance(frame, config) {
            self.imp.push(payload);
        }
    }

    /// 切锁定/解锁（下一帧 `sync` 自然会把新锁定态推出去）。
    pub fn toggle_lock(&mut self) {
        self.state.toggle_lock();
    }

    /// 现在是解锁的吗（接线与测试用）。
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

    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        AC_SRC_ALPHA, AC_SRC_OVER, ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER,
        BLENDFUNCTION, CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateDIBSection, CreateFontW,
        DEFAULT_CHARSET, DEFAULT_PITCH, DeleteDC, DeleteObject, FF_DONTCARE, FW_NORMAL,
        GetTextExtentPoint32W, HDC, HFONT, HGDIOBJ, OUT_DEFAULT_PRECIS, SelectObject, SetBkColor,
        SetBkMode, SetTextColor, TextOutW,
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
        /// 窗口中心点：装的时候给初值，拖完由 `WM_EXITSIZEMOVE` 更新。
        center: (i32, i32),
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
                let error = std::io::Error::last_os_error();
                tracing::warn!(%error, "注册歌词窗口类失败");
                let _ = ready.send(Err("歌词窗口起不来".to_string()));
                return;
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
            let face = wide(&shared.config.font);
            let big_size = (shared.config.font_size * dpi / 96).max(8) as i32;
            let small_size = ((shared.config.font_size * 3 / 5) * dpi / 96).max(8) as i32;
            let mut painter = Painter {
                big: make_font(big_size, face.as_ptr()),
                small: make_font(small_size, face.as_ptr()),
                transparent: None,
                center,
            };

            let mut message: MSG = std::mem::zeroed();
            while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                match message.message {
                    // 画一帧（要在这一线程做，绘制状态是线程本地的）。
                    WM_PAYLOAD => draw(hwnd, &shared, &mut painter),
                    // 拖完了：把新的中心点记下来（下次画按它摆），并回传给主线程落盘。
                    WM_EXITSIZEMOVE => {
                        let mut rect: RECT = std::mem::zeroed();
                        GetWindowRect(hwnd, &mut rect);
                        let center = ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
                        painter.center = center;
                        let _ = places.send(center);
                    }
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
    unsafe fn draw(hwnd: HWND, shared: &Shared, painter: &mut Painter) {
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

        let mut canvas = Canvas::new(width, height);
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
    struct Canvas {
        width: i32,
        height: i32,
        dc: HDC,
        bits: *mut u8,
        mask_dc: HDC,
        mask_bits: *mut u8,
    }

    impl Canvas {
        unsafe fn new(width: i32, height: i32) -> Self {
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
            SelectObject(dc, bitmap as HGDIOBJ);
            SelectObject(mask_dc, mask as HGDIOBJ);
            let bytes = (width * height * 4) as usize;
            std::ptr::write_bytes(bits as *mut u8, 0, bytes);
            std::ptr::write_bytes(mask_bits as *mut u8, 0, bytes);
            Self { width, height, dc, bits: bits as *mut u8, mask_dc, mask_bits: mask_bits as *mut u8 }
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

/// 其它平台（含 macOS，等以后单独做）：不建窗口，什么都不做。这份之后不再改。
#[cfg(not(target_os = "windows"))]
mod imp {
    use std::sync::mpsc::Receiver;

    use super::{DesktopLyricsConfig, Payload};

    pub(super) struct Imp;

    impl Imp {
        pub(super) fn none() -> Self {
            Self
        }

        pub(super) fn push(&mut self, _payload: Payload) {}
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
    fn the_position_file_round_trips() {
        let dir = std::env::temp_dir().join(format!("canto-lyrics-center-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let path = dir.join("desktop-lyrics.json");

        save_center(&path, (123, 456)).expect("落盘");
        assert_eq!(load_center(&path), Some((123, 456)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_broken_position_file_falls_back_to_the_default() {
        let dir = std::env::temp_dir().join(format!("canto-lyrics-broken-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let path = dir.join("desktop-lyrics.json");

        assert_eq!(load_center(&path), None, "文件不在就是 None");
        std::fs::write(&path, "{ 不是 JSON").expect("写坏文件");
        assert_eq!(load_center(&path), None, "坏文件当没有");
        std::fs::remove_dir_all(&dir).ok();
    }
}

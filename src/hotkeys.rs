//! 全局快捷键：canto 没在前台聚焦时也能控制播放。
//!
//! 三件事值得记住：
//!
//! - 热键出来统一是 [`Action`]，跟键盘走同一条派发路径（`tui::state::dispatch_hotkey`），
//!   所以不需要另一套动作语义。
//! - 平台差异全在 `cfg` 子模块里，对外只有 [`install`] 与 [`Hotkeys`] 的三个方法。
//! - 注册是**系统级**的：注册期间那些组合别的程序收不到（README 的已知限制里写了）。

use std::collections::HashMap;

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, HotKeyState};

use crate::tui::keys::Action;

/// 一条要注册的绑定：动作 + 配置里写的组合原文（原文留着报错与日志用）。
pub struct Binding {
    pub action: Action,
    pub keys: String,
}

/// 默认组合：字段名跟 `config::HotkeyConfig` 一一对应，也是配置模板与警告文案的真源。
pub const DEFAULTS: [(&str, &str, Action); 7] = [
    ("play_pause", "ctrl+shift+space", Action::PlayPause),
    ("next", "ctrl+shift+right", Action::Next),
    ("prev", "ctrl+shift+left", Action::Prev),
    ("volume_up", "ctrl+shift+up", Action::VolumeUp),
    ("volume_down", "ctrl+shift+down", Action::VolumeDown),
    ("lock", "ctrl+shift+l", Action::LyricsLock),
    ("lyrics", "ctrl+shift+d", Action::LyricsWindow),
];

/// 组合能不能用：crate 认键名（顺带给出解析好的 `HotKey`），另外**必须有修饰键**。
///
/// `win` 会被归一成 crate 认的 `super`（Windows 键在 macOS 上就是 cmd）。
pub fn validate(keys: &str) -> Result<HotKey, String> {
    // crate 允许裸键（`space`），但那等于让 canto 抢走全局的空格键——不用热键的人会被坑到。
    if !keys.contains('+') {
        return Err("必须带修饰键(如 ctrl+shift+space)".to_string());
    }
    // `win` 是使用者自然会写的名字（配置模板里就是这么写的），但 crate 的修饰键白名单里只有
    // `option/alt`、`control/ctrl`、`command/cmd/super`、`shift`——先归一，再交给它解析。
    // 不归一的话，照模板写 `win+shift+space` 会被判非法、那个动作静默失效。
    let normalized: String = keys
        .split('+')
        .map(|token| if token.trim().eq_ignore_ascii_case("win") { "super" } else { token })
        .collect::<Vec<_>>()
        .join("+");
    normalized.parse().map_err(|error| format!("{error}"))
}

/// 装成什么样：成功几条、哪几条没成、这个平台认不认。细节在日志里，这里只留能报给使用者的话。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InstallReport {
    /// 成功注册的条数（写日志用）。
    pub registered: usize,
    /// 注册失败的组合原文（原因已经在日志里）。
    pub failures: Vec<String>,
    /// 这个平台压根不支持全局热键。
    pub unsupported: bool,
}

impl InstallReport {
    /// 该往状态行写的那一句话；`None` = 什么都不用说。
    pub fn notice(&self) -> Option<String> {
        if self.unsupported {
            return Some("此平台暂不支持全局热键".to_string());
        }
        match self.failures.as_slice() {
            [] => None,
            // 一条要点名，好让人知道改哪一行。
            [one] => Some(format!("热键注册失败:{one}")),
            // 两条以上点不过来（通知槽 32 列），只报个数。
            many => Some(format!("热键注册失败 {} 个(见日志)", many.len())),
        }
    }
}

/// 注册成功的 `id → 动作`；同一个组合写两次时 id 相同，保留先注册的那条。
fn id_map(registered: impl IntoIterator<Item = (u32, Action)>) -> HashMap<u32, Action> {
    let mut map = HashMap::new();
    for (id, action) in registered {
        map.entry(id).or_insert(action);
    }
    map
}

/// 事件流 → 动作：只认「按下」（松键那半是系统补发的），未知 id 丢掉。
fn actions_from(map: &HashMap<u32, Action>, events: impl IntoIterator<Item = (u32, bool)>) -> Vec<Action> {
    events
        .into_iter()
        .filter(|(_, pressed)| *pressed)
        .filter_map(|(id, _)| map.get(&id).copied())
        .collect()
}

/// 平台实现的回报：注册成功的 `(id, 绑定下标)` + 没注册成的绑定下标 + 「整件事没办成」的原因。
///
/// 只认下标——平台实现不该关心配置里写了什么；组合原文与动作由 [`fold`] 在这一层取出来。
/// （顺带保证 `Binding` 的两个字段在任何平台上都是被读的，不留下 dead_code。）
#[derive(Default)]
struct Outcome {
    pairs: Vec<(u32, usize)>,
    failed: Vec<usize>,
    /// 跟具体某条绑定无关的失败（线程 / 窗口起不来），与 `failed` 互斥。
    fatal: Option<String>,
    unsupported: bool,
}

/// 把平台回报的下标折成「id → 动作」与对外的报告。
fn fold(bindings: &[Binding], outcome: &Outcome) -> (HashMap<u32, Action>, InstallReport) {
    // 下标都是平台实现从 `bindings` 上取的，按位取即可。
    let map = id_map(outcome.pairs.iter().map(|(id, index)| (*id, bindings[*index].action)));
    // 整件事没办成时只说那一句；否则逐条报组合原文（好让状态行能点名）。
    let failures = match &outcome.fatal {
        Some(reason) => vec![reason.clone()],
        None => outcome.failed.iter().map(|index| bindings[*index].keys.clone()).collect(),
    };
    let report = InstallReport { registered: outcome.pairs.len(), failures, unsupported: outcome.unsupported };
    (map, report)
}

/// 已注册的热键集合。
pub struct Hotkeys {
    imp: imp::Imp,
    map: HashMap<u32, Action>,
}

impl Hotkeys {
    /// 空句柄：不注册任何东西，`drain()` 永远为空。`App::new` 与测试用这个，
    /// 真注册只发生在 `tui::run` 调 [`install`] 的那一次。
    pub fn none() -> Self {
        Self { imp: imp::Imp::none(), map: HashMap::new() }
    }

    /// 取走这一轮攒下的动作（crate 的事件通道是全局的，谁取都行）。
    pub fn drain(&mut self) -> Vec<Action> {
        let events = GlobalHotKeyEvent::receiver()
            .try_iter()
            .map(|event| (event.id, event.state == HotKeyState::Pressed));
        actions_from(&self.map, events)
    }

    /// 每帧泵一次平台的活。只有 macOS 需要（Carbon 的热键事件得自己从队列里拉出来派发）。
    pub fn pump(&self) {
        self.imp.pump();
    }
}

/// 按配置里的绑定注册，返回句柄与「装成什么样」。
pub fn install(bindings: &[Binding]) -> (Hotkeys, InstallReport) {
    let (imp, outcome) = imp::install(bindings);
    let (map, report) = fold(bindings, &outcome);
    (Hotkeys { imp, map }, report)
}

/// Windows：crate 只建隐藏窗口、自己不起线程，而 `WM_HOTKEY` 是**投给建窗口那个线程**的，
/// 所以这里自备一个专用线程：建窗口 → 逐条注册 → 回报 → 跑消息循环。
#[cfg(target_os = "windows")]
mod imp {
    use std::sync::mpsc::{Sender, channel};
    use std::thread::JoinHandle;

    use global_hotkey::GlobalHotKeyManager;
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, MSG, PostThreadMessageW, TranslateMessage, WM_QUIT,
    };

    use super::{Binding, Outcome, validate};

    /// 线程的回报：`Err` = 线程活着但管理器 / 窗口起不来。
    struct Started {
        thread_id: u32,
        outcome: Outcome,
    }

    pub(super) struct Imp {
        /// 消息循环线程的 id（`Drop` 里投 `WM_QUIT` 用）；0 = 没有线程。
        thread_id: u32,
        join: Option<JoinHandle<()>>,
    }

    impl Imp {
        pub(super) fn none() -> Self {
            Self { thread_id: 0, join: None }
        }

        /// Windows 的消息由专用线程处理，主循环这边不用泵。
        pub(super) fn pump(&self) {}
    }

    impl Drop for Imp {
        fn drop(&mut self) {
            let Some(join) = self.join.take() else {
                return;
            };
            // 线程正卡在 `GetMessageW` 里，只能投消息把它叫醒。
            unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
            let _ = join.join();
        }
    }

    pub(super) fn install(bindings: &[Binding]) -> (Imp, Outcome) {
        let (tx, rx) = channel::<Result<Started, String>>();
        let owned: Vec<Binding> = bindings
            .iter()
            .map(|binding| Binding { action: binding.action, keys: binding.keys.clone() })
            .collect();
        let spawned = std::thread::Builder::new()
            .name("canto-hotkeys".to_string())
            .spawn(move || run(owned, tx));
        let join = match spawned {
            Ok(join) => join,
            Err(error) => {
                tracing::warn!(%error, "全局热键线程没起来");
                return (Imp::none(), Outcome { fatal: Some("无法启动热键线程".to_string()), ..Outcome::default() });
            }
        };
        match rx.recv() {
            // 拿到了回报：线程要么正在跑消息循环，要么已经因为失败退出了。
            Ok(Ok(started)) => (Imp { thread_id: started.thread_id, join: Some(join) }, started.outcome),
            Ok(Err(reason)) => {
                let _ = join.join();
                (Imp::none(), Outcome { fatal: Some(reason), ..Outcome::default() })
            }
            // 通道断了：线程在建窗口之前就没了。
            Err(_) => {
                let _ = join.join();
                (Imp::none(), Outcome { fatal: Some("热键线程提前退出".to_string()), ..Outcome::default() })
            }
        }
    }

    /// 专用线程：建窗口（crate 在**调用线程**上建）→ 逐条注册 → 回报 → 跑消息循环。
    fn run(bindings: Vec<Binding>, tx: Sender<Result<Started, String>>) {
        let thread_id = unsafe { GetCurrentThreadId() };
        let manager = match GlobalHotKeyManager::new() {
            Ok(manager) => manager,
            Err(error) => {
                tracing::warn!(%error, "全局热键管理器建不起来");
                let _ = tx.send(Err("无法初始化热键".to_string()));
                return;
            }
        };
        let mut pairs = Vec::new();
        let mut failed = Vec::new();
        for (index, binding) in bindings.iter().enumerate() {
            let hotkey = match validate(&binding.keys) {
                Ok(hotkey) => hotkey,
                Err(why) => {
                    tracing::warn!(combo = %binding.keys, %why, "全局热键不合法");
                    failed.push(index);
                    continue;
                }
            };
            match manager.register(hotkey) {
                Ok(()) => pairs.push((hotkey.id(), index)),
                Err(error) => {
                    tracing::warn!(combo = %binding.keys, %error, "全局热键注册失败");
                    failed.push(index);
                }
            }
        }
        // 窗口一建出来这个线程就有消息队列了，所以 `Drop` 里投的 `WM_QUIT` 一定有人收。
        let outcome = Outcome { pairs, failed, ..Outcome::default() };
        let _ = tx.send(Ok(Started { thread_id, outcome }));

        // `WM_HOTKEY` 是**投递**（不是发送）给这个线程的，得自己取出来派发。
        let mut message = MSG::default();
        while unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) } > 0 {
            unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        // 出循环 = 收到 `WM_QUIT`：`manager` 在这里 drop，隐藏窗口与所有注册一起归还。
    }
}

/// macOS：crate 把事件处理器 `InstallEventHandler` 挂在 **application event target** 上，热键事件由
/// Carbon 的事件队列送到那个 target。Carbon 的热键事件**不走 CFRunLoop 的 source**：实测同一个
/// 循环里 `CFRunLoopRunInMode(kCFRunLoopDefaultMode, …)` 一圈圈泵也收不到热键，`CGEventTap` 的回调
/// 却照收。所以主线程每轮自己用 `ReceiveNextEvent` 把队列里那两类事件拉出来、
/// `SendEventToEventTarget` 派发掉（见 `pump`）——这正是 `RunApplicationEventLoop` 内部做的事，
/// 而它要阻塞到退出，塞不进「画帧 + 读键盘」的循环。
#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::c_void;

    use global_hotkey::GlobalHotKeyManager;

    use super::{Binding, Outcome, validate};

    /// 不透明的 `EventRef` / `EventTargetRef`。
    type EventRef = *mut c_void;
    type EventTargetRef = *mut c_void;

    /// Carbon 的 `EventTypeSpec`：一类事件 = 事件类 + 事件类型。
    #[repr(C)]
    struct EventTypeSpec {
        event_class: u32,
        event_kind: u32,
    }

    /// `'keyb'`（`kEventClassKeyboard`）。
    const KEYBOARD_CLASS: u32 = u32::from_be_bytes(*b"keyb");
    /// `kEventHotKeyPressed` / `kEventHotKeyReleased`（热键按下与松开各来一条，crate 两边都发事件）。
    const HOTKEY_PRESSED: u32 = 5;
    const HOTKEY_RELEASED: u32 = 6;
    /// Carbon 的 `noErr`。
    const NO_ERR: i32 = 0;

    /// 只要这两类：按它们取值，队列里别人的事件原样留着不动。
    const HOTKEY_EVENTS: [EventTypeSpec; 2] = [
        EventTypeSpec { event_class: KEYBOARD_CLASS, event_kind: HOTKEY_PRESSED },
        EventTypeSpec { event_class: KEYBOARD_CLASS, event_kind: HOTKEY_RELEASED },
    ];

    pub(super) struct Imp {
        /// 拿住 manager 就等于拿住那几条注册；`None` = 什么都没注册。
        manager: Option<GlobalHotKeyManager>,
    }

    impl Imp {
        pub(super) fn none() -> Self {
            Self { manager: None }
        }

        /// 把队列里攒下的热键事件拉空并逐条派发：`0.0` 秒 + `pull` = 有什么拿什么，不阻塞界面。
        pub(super) fn pump(&self) {
            // 没注册过就没什么可泵的（`Hotkeys::none()` 走这条路）。
            if self.manager.is_none() {
                return;
            }
            loop {
                let mut event: EventRef = std::ptr::null_mut();
                // SAFETY: 只按 `HOTKEY_EVENTS` 这两类取值；`0.0` 秒 + `1`(= pull) 立即返回，
                // 队列里没有这类事件就出循环，不会卡住界面。
                let status = unsafe { ReceiveNextEvent(HOTKEY_EVENTS.len() as u32, HOTKEY_EVENTS.as_ptr(), 0.0, 1, &mut event) };
                // 没有这类事件了（超时的 `eventLoopTimedOutErr`）；剩下的都不是我们的事。
                if status != NO_ERR || event.is_null() {
                    break;
                }
                // SAFETY: `event` 是刚拉出来的一条、所有权已经归我们（`pull` 把它从队列摘掉了），
                // 派发完必须 `ReleaseEvent` 归还。派发目标跟 crate 装 handler 的那个 target 一致。
                unsafe {
                    let dispatched = SendEventToEventTarget(event, GetApplicationEventTarget());
                    if dispatched != NO_ERR {
                        tracing::warn!(status = dispatched, "派发热键事件失败");
                    }
                    ReleaseEvent(event);
                }
            }
        }
    }

    // edition 2024 起 `extern` 块必须写 `unsafe extern`。
    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        /// `EventTargetRef GetApplicationEventTarget()`：crate 的 handler 与热键注册都挂在这个 target 上。
        fn GetApplicationEventTarget() -> EventTargetRef;
        /// `OSStatus ReceiveNextEvent(UInt32, const EventTypeSpec *, EventTimeout, Boolean, EventRef *)`。
        /// ⚠️ `Boolean` 是 `unsigned char`，所以第四个参数（`pull`）用 `u8` 而不是 `bool`。
        fn ReceiveNextEvent(types: u32, list: *const EventTypeSpec, timeout: f64, pull: u8, event: *mut EventRef) -> i32;
        /// `OSStatus SendEventToEventTarget(EventRef, EventTargetRef)`。
        fn SendEventToEventTarget(event: EventRef, target: EventTargetRef) -> i32;
        /// `void ReleaseEvent(EventRef)`。
        fn ReleaseEvent(event: EventRef);
    }

    pub(super) fn install(bindings: &[Binding]) -> (Imp, Outcome) {
        // 必须在主线程建：`InstallEventHandler` 挂在 application event target 上。
        let manager = match GlobalHotKeyManager::new() {
            Ok(manager) => manager,
            Err(error) => {
                tracing::warn!(%error, "全局热键管理器建不起来");
                return (Imp::none(), Outcome { fatal: Some("无法初始化热键".to_string()), ..Outcome::default() });
            }
        };
        let mut pairs = Vec::new();
        let mut failed = Vec::new();
        for (index, binding) in bindings.iter().enumerate() {
            let hotkey = match validate(&binding.keys) {
                Ok(hotkey) => hotkey,
                Err(why) => {
                    tracing::warn!(combo = %binding.keys, %why, "全局热键不合法");
                    failed.push(index);
                    continue;
                }
            };
            match manager.register(hotkey) {
                Ok(()) => pairs.push((hotkey.id(), index)),
                Err(error) => {
                    tracing::warn!(combo = %binding.keys, %error, "全局热键注册失败");
                    failed.push(index);
                }
            }
        }
        let outcome = Outcome { pairs, failed, ..Outcome::default() };
        (Imp { manager: Some(manager) }, outcome)
    }
}

/// 其它平台（Linux / Wayland 等）：不注册，启动时说一句。这份是最终形态。
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod imp {
    use super::{Binding, Outcome};

    pub(super) struct Imp;

    impl Imp {
        pub(super) fn none() -> Self {
            Self
        }

        pub(super) fn pump(&self) {}
    }

    pub(super) fn install(_bindings: &[Binding]) -> (Imp, Outcome) {
        (Imp, Outcome { unsupported: true, ..Outcome::default() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    /// 没有修饰键的裸键会被系统级抢走，必须拒。
    #[test]
    fn a_combo_without_a_modifier_is_refused() {
        assert!(validate("space").is_err());
        assert!(validate("ctrl+space").is_ok());
        assert!(validate("ctrl+shift+space").is_ok());
    }

    #[test]
    fn an_unparsable_combo_is_refused() {
        for text in ["ctrl+shift+nope", "ctrl+", "+", "", "ctrl+shift+不存在的键"] {
            assert!(validate(text).is_err(), "{text:?} 不该被接受");
        }
    }

    /// 大小写与 crate 认的别名都算合法（`Ctrl` / `control` 是同一个修饰键）。
    #[test]
    fn case_and_aliases_parse_the_same() {
        assert!(validate("Ctrl+Shift+Space").is_ok());
        assert!(validate("control+shift+space").is_ok());
    }

    /// `win` 是使用者自然会写的名字（模板与 README 都这么教），但 crate 的修饰键白名单里
    /// 只有 `option/alt`、`control/ctrl`、`command/cmd/super`、`shift`——不归一的话，
    /// 照文档写 `win+shift+space` 会被判非法、那个动作静默失效。
    #[test]
    fn win_is_an_alias_for_super() {
        assert!(validate("win+shift+space").is_ok(), "`win` 该能用");
        assert!(validate("WIN+space").is_ok(), "大小写无关");
        assert!(validate("super+space").is_ok(), "crate 的原生写法当然也能用");
        assert!(validate("ctrl+win").is_err(), "只有修饰键、没有主键仍要拒");
    }

    #[test]
    fn the_defaults_are_the_seven_combos_the_spec_fixed() {
        let fields: Vec<&str> = DEFAULTS.iter().map(|(field, _, _)| *field).collect();
        assert_eq!(
            fields,
            ["play_pause", "next", "prev", "volume_up", "volume_down", "lock", "lyrics"]
        );
        let combos: Vec<&str> = DEFAULTS.iter().map(|(_, keys, _)| *keys).collect();
        assert_eq!(
            combos,
            [
                "ctrl+shift+space",
                "ctrl+shift+right",
                "ctrl+shift+left",
                "ctrl+shift+up",
                "ctrl+shift+down",
                "ctrl+shift+l",
                "ctrl+shift+d"
            ]
        );
        let actions: Vec<Action> = DEFAULTS.iter().map(|(_, _, action)| *action).collect();
        assert_eq!(
            actions,
            [
                Action::PlayPause,
                Action::Next,
                Action::Prev,
                Action::VolumeUp,
                Action::VolumeDown,
                Action::LyricsLock,
                Action::LyricsWindow
            ]
        );
        // 每个默认组合本身必须合法，否则开箱就报错。
        for (field, keys, _) in DEFAULTS {
            assert!(validate(keys).is_ok(), "默认组合 {field} = {keys:?} 不合法");
        }
    }

    /// 一条要点名（好让人知道改哪条），一堆只报个数（32 列放不下）。
    #[test]
    fn one_failure_is_named_and_several_are_counted() {
        assert_eq!(InstallReport::default().notice(), None);
        assert_eq!(
            InstallReport { registered: 4, failures: vec!["ctrl+shift+space".to_string()], unsupported: false }.notice(),
            Some("热键注册失败:ctrl+shift+space".to_string())
        );
        assert_eq!(
            InstallReport { registered: 3, failures: vec!["a".into(), "b".into()], unsupported: false }.notice(),
            Some("热键注册失败 2 个(见日志)".to_string())
        );
        // 不支持的平台说的是另一件事，不能被说成「注册失败」。
        assert_eq!(
            InstallReport { registered: 0, failures: vec![], unsupported: true }.notice(),
            Some("此平台暂不支持全局热键".to_string())
        );
        // 每条文案都得能完整放进状态行。
        for report in [
            InstallReport { registered: 4, failures: vec!["ctrl+shift+space".to_string()], unsupported: false },
            InstallReport { registered: 3, failures: vec!["a".into(), "b".into()], unsupported: false },
            InstallReport { registered: 0, failures: vec![], unsupported: true },
        ] {
            let text = report.notice().expect("该有文案");
            assert!(text.width() <= 32, "「{text}」{} 列，超过通知槽", text.width());
        }
    }

    #[test]
    fn ids_map_once_and_only_pressed_events_win() {
        // 同一个组合写两次：两个绑定同一个 id，保留先注册的那条（后注册的会失败）。
        let map = id_map([(7, Action::Next), (8, Action::VolumeUp), (7, Action::Prev)]);
        assert_eq!(map.get(&7), Some(&Action::Next));
        assert_eq!(map.get(&8), Some(&Action::VolumeUp));

        // 事件流：`Released` 忽略，未知 id 丢掉。
        let actions = actions_from(&map, [(7, true), (7, false), (9, true), (8, true), (7, true)]);
        assert_eq!(actions, [Action::Next, Action::VolumeUp, Action::Next]);
    }

    /// 平台实现只回报下标（它不该关心配置里写了什么）。组合原文与动作在这一层折出来，
    /// 失败的还要带回原文，好让状态行能点名。
    #[test]
    fn the_platform_reports_indices_and_we_lift_the_text() {
        let bindings = [
            Binding { action: Action::PlayPause, keys: "ctrl+shift+space".to_string() },
            Binding { action: Action::Next, keys: "ctrl+shift+right".to_string() },
            Binding { action: Action::VolumeUp, keys: "ctrl+shift+up".to_string() },
        ];
        let outcome = Outcome { pairs: vec![(11, 0), (12, 2)], failed: vec![1], fatal: None, unsupported: false };

        let (map, report) = fold(&bindings, &outcome);

        assert_eq!(map.get(&11), Some(&Action::PlayPause));
        assert_eq!(map.get(&12), Some(&Action::VolumeUp));
        assert_eq!(map.len(), 2, "失败的那条不该进映射");
        assert_eq!(report.registered, 2);
        assert_eq!(report.failures, ["ctrl+shift+right"], "失败要带组合原文");
        assert!(!report.unsupported);
    }

    /// 整件事没办成（窗口 / 线程起不来）时只说那一句，不要逐条报「全都失败」。
    #[test]
    fn a_fatal_install_failure_is_reported_once() {
        let bindings = [
            Binding { action: Action::PlayPause, keys: "ctrl+shift+space".to_string() },
            Binding { action: Action::Next, keys: "ctrl+shift+right".to_string() },
        ];
        let outcome = Outcome { fatal: Some("无法初始化热键".to_string()), ..Outcome::default() };

        let (map, report) = fold(&bindings, &outcome);

        assert!(map.is_empty());
        assert_eq!(report.registered, 0);
        assert_eq!(report.failures, ["无法初始化热键"]);
        assert_eq!(report.notice(), Some("热键注册失败:无法初始化热键".to_string()));
    }

    #[test]
    fn a_none_handle_yields_nothing() {
        let mut hotkeys = Hotkeys::none();
        assert!(hotkeys.drain().is_empty());
        hotkeys.pump(); // 不许 panic
    }

    /// 不支持的平台只出那一句话（Linux 的 CI 跑这条）。
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    #[test]
    fn an_unsupported_platform_says_so_once() {
        let (mut hotkeys, report) = install(&[]);
        assert_eq!(report.notice(), Some("此平台暂不支持全局热键".to_string()));
        assert_eq!(report.registered, 0);
        assert!(hotkeys.drain().is_empty());
    }
}

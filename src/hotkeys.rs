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
pub const DEFAULTS: [(&str, &str, Action); 5] = [
    ("play_pause", "ctrl+shift+space", Action::PlayPause),
    ("next", "ctrl+shift+right", Action::Next),
    ("prev", "ctrl+shift+left", Action::Prev),
    ("volume_up", "ctrl+shift+up", Action::VolumeUp),
    ("volume_down", "ctrl+shift+down", Action::VolumeDown),
];

/// 组合能不能用：crate 认键名（顺带给出解析好的 `HotKey`），另外**必须有修饰键**。
pub fn validate(keys: &str) -> Result<HotKey, String> {
    let hotkey: HotKey = keys.parse().map_err(|error| format!("{error}"))?;
    // crate 允许裸键（`space`），但那等于让 canto 抢走全局的空格键——不用热键的人会被坑到。
    if !keys.contains('+') {
        return Err("必须带修饰键(如 ctrl+shift+space)".to_string());
    }
    Ok(hotkey)
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

/// 平台实现的回报：注册成功的 `(id, 绑定下标)` + 没注册成的绑定下标。
///
/// 只认下标——平台实现不该关心配置里写了什么；组合原文与动作由 [`fold`] 在这一层取出来。
/// （顺带保证 `Binding` 的两个字段在任何平台上都是被读的，不留下 dead_code。）
#[derive(Default)]
struct Outcome {
    pairs: Vec<(u32, usize)>,
    failed: Vec<usize>,
    unsupported: bool,
}

/// 把平台回报的下标折成「id → 动作」与对外的报告。
fn fold(bindings: &[Binding], outcome: &Outcome) -> (HashMap<u32, Action>, InstallReport) {
    // 下标都是平台实现从 `bindings` 上取的，按位取即可。
    let map = id_map(outcome.pairs.iter().map(|(id, index)| (*id, bindings[*index].action)));
    let report = InstallReport {
        registered: outcome.pairs.len(),
        failures: outcome.failed.iter().map(|index| bindings[*index].keys.clone()).collect(),
        unsupported: outcome.unsupported,
    };
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

    /// 每帧泵一次平台的活。只有 macOS 需要（Carbon 事件靠主 run loop 派发）。
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

/// Windows：真实现见 Task 4（专用线程跑消息循环，crate 自己不起线程）。
#[cfg(target_os = "windows")]
mod imp {
    use super::{Binding, Outcome};

    pub(super) struct Imp;

    impl Imp {
        pub(super) fn none() -> Self {
            Self
        }

        /// Windows 的消息由专用线程处理，主循环这边不用泵。
        pub(super) fn pump(&self) {}
    }

    pub(super) fn install(_bindings: &[Binding]) -> (Imp, Outcome) {
        (Imp, Outcome::default())
    }
}

/// macOS：真实现见 Task 5（主线程建 manager + 每轮泵 run loop）。
#[cfg(target_os = "macos")]
mod imp {
    use super::{Binding, Outcome};

    pub(super) struct Imp;

    impl Imp {
        pub(super) fn none() -> Self {
            Self
        }

        /// macOS 的 Carbon 事件靠主 run loop 派发，真实现里这里会泵一次。
        pub(super) fn pump(&self) {}
    }

    pub(super) fn install(_bindings: &[Binding]) -> (Imp, Outcome) {
        (Imp, Outcome::default())
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

    #[test]
    fn the_defaults_are_the_five_combos_the_spec_fixed() {
        let fields: Vec<&str> = DEFAULTS.iter().map(|(field, _, _)| *field).collect();
        assert_eq!(fields, ["play_pause", "next", "prev", "volume_up", "volume_down"]);
        let combos: Vec<&str> = DEFAULTS.iter().map(|(_, keys, _)| *keys).collect();
        assert_eq!(
            combos,
            ["ctrl+shift+space", "ctrl+shift+right", "ctrl+shift+left", "ctrl+shift+up", "ctrl+shift+down"]
        );
        let actions: Vec<Action> = DEFAULTS.iter().map(|(_, _, action)| *action).collect();
        assert_eq!(actions, [Action::PlayPause, Action::Next, Action::Prev, Action::VolumeUp, Action::VolumeDown]);
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
        let outcome = Outcome { pairs: vec![(11, 0), (12, 2)], failed: vec![1], unsupported: false };

        let (map, report) = fold(&bindings, &outcome);

        assert_eq!(map.get(&11), Some(&Action::PlayPause));
        assert_eq!(map.get(&12), Some(&Action::VolumeUp));
        assert_eq!(map.len(), 2, "失败的那条不该进映射");
        assert_eq!(report.registered, 2);
        assert_eq!(report.failures, ["ctrl+shift+right"], "失败要带组合原文");
        assert!(!report.unsupported);
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

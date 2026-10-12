# canto 全局快捷键实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** canto 被切到后台（窗口未聚焦）时，用系统级热键控制播放/暂停、上下一首、音量加减。

**Architecture:** 新增平台无关模块 `src/hotkeys.rs`，`cfg` 分派三份实现——Windows 自建专用线程（crate 只建窗口、不跑循环，`WM_HOTKEY` 必须有人派发）、macOS 在主线程建 manager 并由 UI 循环每轮泵一次 CoreFoundation run loop、其它平台空实现。事件出来统一是 `keys::Action`，走 `App` 现有的 `apply()`；配置在 `[hotkeys]` 段。

**Tech Stack:** Rust 2021；`global-hotkey 0.8`（Apache-2.0 OR MIT，Linux 侧是纯 Rust 的 `x11rb`/`xkeysym`，不碰系统库）；Windows 侧复用已有 `windows-sys 0.61`（加 `Win32_UI_WindowsAndMessaging`）；macOS 侧 6 行自带 CoreFoundation FFI。

**Spec:** `docs/superpowers/specs/2026-10-12-global-hotkeys-design.md`

## 从 spec 到计划的四处落定（读 crate 源码后确认，与 spec 原文有别）

| spec 原文 | 计划落成 | 依据 |
|---|---|---|
| macOS 额外加 `core-foundation` 依赖 | **不加**：自带 2 个符号的 FFI（`CFRunLoopRunInMode` + `kCFRunLoopDefaultMode`，`#[link(name = "CoreFoundation", kind = "framework")]`） | crate 的 Cargo.toml 里没有 core-foundation；CoreFoundation 是系统框架，声明即用 |
| 「Linux 上 crate 的 x11 依赖不编进产物」 | `global-hotkey` 进**无条件** `[dependencies]`；Linux 会编 `x11rb`/`xkeysym` 两个纯 Rust crate | crate Cargo.toml 的 `cfg(any(linux, dragonfly, …))` 段；无 `build.rs`、无系统库 |
| `install` 返回 `(Hotkeys, Vec<String>)` | 返回 `(Hotkeys, InstallReport)`，文案由 `InstallReport::notice()` 出 | 「这个平台不支持」不该被套上「注册失败」前缀 |
| 非法组合「不注册 + 警告」 | 配置阶段就把该字段**清成 `""`**，于是全程只有一条不变式：**非空 = 合法 = 会被注册** | 免得 `bindings()` 再判一次非法，两处规则会漂 |

## Global Constraints

- 平台：Windows、macOS 真实现；其它平台空实现 + 启动时一句通知。
- 依赖只加一个：`global-hotkey = "0.8"`；Windows 目标给已有 `windows-sys` 补一个 feature。不升级既有依赖（接受树里 `windows-sys` 0.59 与 0.61 共存）。**`Cargo.lock` 必须一起提交**（CI 用 `--locked`）。
- 默认组合（唯一真源 `hotkeys::DEFAULTS`）：`play_pause = "ctrl+shift+space"`、`next = "ctrl+shift+right"`、`prev = "ctrl+shift+left"`、`volume_up = "ctrl+shift+up"`、`volume_down = "ctrl+shift+down"`。
- 配置语义：缺段/缺字段 = 默认组合；`""` = 明确禁用；非法值（键名不认 / 没有修饰键）= 清成 `""` + 一条启动警告。
- 派发：只在 `Screen::Main`；一次按下一次动作（crate 固定带 `MOD_NOREPEAT`）；音量步长 ±5。
- `install` 只在 `tui::run` 里调——**`App::new` 不许碰**（`test_app()` 会跟着注册真热键）。
- 通知文案：`NOTICE_COLS = 32`（`src/tui/draw/status.rs`），短句、无路径、无错误链，细节进日志（2026-10-12 那次状态行改造定的约定）。
- 每个 commit 都要在三个平台编得过：CI = Linux `clippy --all-targets --locked -- -D warnings` + ubuntu/windows/macos `cargo test --locked`。
- 新增模块只有一个：`src/hotkeys.rs`。

## Review Focus

（spec 没写但真会遇到的输入，各挂到拥有它的任务上）

1. **组合被别的程序占用，或两个动作写了同一组合** → 只有那一条不生效，其余照常，状态行一行 + 日志；不能整组失败，也不能静默。（Task 1 `InstallReport::notice` 测试 + Task 4 真机清单第 5 条）
2. **配置里写了裸键**（`play_pause = "space"`）→ 必须拒，否则 canto 会把全局空格吞掉，不用热键的人会莫名其妙。（Task 1 `validate` 测试 + Task 2 `a_bare_key_is_refused_and_cleared`）
3. **五个动作全禁用**（五个 `""`）→ 不注册、不通知，行为与没有这个功能时一模一样。（Task 2 `an_empty_string_disables_one_action` + `bindings()` 条数）
4. **非法键名**（`"ctrl+shift+f13"`）→ 一条启动警告，其余四个字段不受影响，程序照常起。（Task 2 `a_bad_key_name_is_refused_and_cleared`）
5. **登录屏按热键** → 什么都不发生（登录屏没有播放可控），不能崩、不能落到主界面逻辑里。（Task 3 `hotkeys_never_reach_the_login_screen`）
6. **退出后组合要归还** → `Drop` 里反注册 + 线程收尾，不许 panic、不许留下「按了就发事件给死进程」的注册。（Task 4 真机清单第 6 条）

## 文件结构

| 文件 | 责任 |
|---|---|
| `src/hotkeys.rs`（新增，约 320 行含测试） | 对外只有 `Binding` / `DEFAULTS` / `validate` / `install` / `InstallReport` / `Hotkeys`；纯逻辑（`id_map`、`actions_from`）与三份 `cfg` 实现全在这一个文件里 |
| `src/config.rs` | `HotkeyConfig`、`Config.hotkeys`、`Raw` 解析、默认值、校验、模板注释 |
| `src/tui/state.rs` | `App.hotkeys` 字段、`tick()` 里 drain、`dispatch_hotkey()` 的屏门 |
| `src/tui/mod.rs` | `tui::run` 里 install + 失败通知 + 循环里 `pump()` |
| `Cargo.toml` / `Cargo.lock` | 依赖 |
| `README.md`、`docs/design.md` | 文档 |

---

# Task 1: `hotkeys.rs`——接口、纯逻辑与三份空实现

先把平台无关的骨架立起来：接口、纯逻辑（全部可单测）、以及三份 `cfg` 实现（不支持的平台这份就是最终形态；Windows/macOS 这两份此时是空壳，Task 4/5 换成真实现）。

**Files:**
- Create: `src/hotkeys.rs`
- Modify: `src/main.rs`（模块清单里 `mod history;` 与 `mod http;` 之间插 `mod hotkeys;`）
- Modify: `Cargo.toml`、`Cargo.lock`

**Interfaces:**
- Consumes: `crate::tui::keys::Action`（`pub mod keys;` 已经可见，无需改可见性）
- Produces（Task 2/3/4/5 都按这些名字与签名用）:
  - `pub struct Binding { pub action: Action, pub keys: String }`
  - `pub const DEFAULTS: [(&str, &str, Action); 5]`（字段名、默认组合、动作）
  - `pub fn validate(keys: &str) -> Result<HotKey, String>`
  - `pub struct InstallReport { pub registered: usize, pub failures: Vec<String>, pub unsupported: bool }` + `impl InstallReport { pub fn notice(&self) -> Option<String> }`
  - `pub fn install(bindings: &[Binding]) -> (Hotkeys, InstallReport)`
  - `pub struct Hotkeys` + `none() -> Self`、`drain(&mut self) -> Vec<Action>`、`pump(&self)`

- [ ] **Step 1: 在 `src/hotkeys.rs` 里写失败的测试**

测试放在文件末尾的 `#[cfg(test)] mod tests`。所有断言都按表：字符串、条数、`Option` 形态。

```rust
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
        for text in ["ctrl+shift+f13", "ctrl+", "+", "", "ctrl+shift+不存在的键"] {
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
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib hotkeys 2>&1 || cargo test hotkeys`
Expected: 编译失败（`hotkeys` 模块还不存在）或 `validate` 等符号未定义。

- [ ] **Step 3: 实现 `src/hotkeys.rs`**

文件头的模块文档写清三件事：为什么需要（TUI 在后台时也要能控制播放）、事件出来是 `keys::Action`、平台差异都在 `cfg` 子模块里。

```rust
use std::collections::HashMap;

use global_hotkey::GlobalHotKeyEvent;
use global_hotkey::hotkey::{HotKey, HotKeyState};

use crate::tui::keys::Action;

/// 一条要注册的绑定：动作 + 配置里写的组合原文（原文要留着报错用）。
pub struct Binding {
    pub action: Action,
    pub keys: String,
}

/// 默认组合；字段名与 `HotkeyConfig` 一一对应，也是模板与警告文案的真源。
pub const DEFAULTS: [(&str, &str, Action); 5] = [/* Step 1 测试里的五条 */];

/// 组合能不能用：crate 认键名（顺带拿到解析好的 `HotKey`），另外必须有修饰键。
pub fn validate(keys: &str) -> Result<HotKey, String> {
    let hotkey: HotKey = keys.parse().map_err(|error| format!("{error}"))?;
    // crate 允许裸键（`space`），但那会被系统级抢走，canto 不该这么干。
    if !keys.contains('+') {
        return Err("必须带修饰键（如 ctrl+shift+space）".to_string());
    }
    Ok(hotkey)
}
```

- `InstallReport` 三个字段 + `notice()`：`unsupported` 优先；`failures` 空返回 `None`；一条点名；多条报个数。
- `id_map(registered: impl IntoIterator<Item = (u32, Action)>) -> HashMap<u32, Action>`：`entry(id).or_insert(action)`（同 id 保留先到的）。
- `actions_from(map: &HashMap<u32, Action>, events: impl IntoIterator<Item = (u32, bool)>) -> Vec<Action>`：过滤出 `true`，再查表，未知 id 丢掉。
- `Hotkeys { imp: imp::Imp, map: HashMap<u32, Action> }`：
  - `none()` → `imp::Imp::none()` + 空表；
  - `drain(&mut self)` → `actions_from(&self.map, GlobalHotKeyEvent::receiver().try_iter().map(|event| (event.id, event.state == HotKeyState::Pressed)))`；
  - `pump(&self)` → `self.imp.pump()`。
- `install(bindings) -> (Hotkeys, InstallReport)`：调 `imp::install(bindings)`，把返回的 `(Imp, Vec<(u32, Action)>, InstallReport)` 折成 `(Hotkeys { imp, map: id_map(registered) }, report)`。
- `mod imp` 三份（`#[cfg(target_os = "windows")]` / `#[cfg(target_os = "macos")]` / `#[cfg(not(any(…)))]`），**本步骤里两份空壳**、每份提供 `struct Imp` + `Imp::none()` + `Imp::pump(&self)` + `fn install(bindings: &[Binding]) -> (Imp, Vec<(u32, Action)>, InstallReport)`：
  - Windows / macOS 空壳返回 `(Imp::none(), Vec::new(), InstallReport { registered: 0, failures: Vec::new(), unsupported: false })`，并注明 `// 真实现见 Task 4 / Task 5`；
  - 不支持平台最终形态：`unsupported: true`，其余为空——**这份之后不再改**。
  - 空壳不许留 `unused` 警告（Linux 的 clippy 是 `-D warnings`）：参数用 `_bindings`，或直接用上。

- [ ] **Step 4: 跑测试与 clippy 确认通过**

Run: `cargo test --lib hotkeys`（本机 Windows）
Expected: 上面 6 条测试（`an_unsupported_platform_says_so_once` 会因 `cfg` 跳过）全 PASS。

Run: `cargo clippy --all-targets --locked -- -D warnings`
Expected: 无输出（`--locked` 也要过，说明 `Cargo.lock` 已经跟着改好了）。

- [ ] **Step 5: 提交**

```bash
git add src/hotkeys.rs src/main.rs Cargo.toml Cargo.lock
git commit -m "feat(hotkeys): 全局热键的接口与纯逻辑(三平台空实现)"
```

---

# Task 2: `[hotkeys]` 配置段

**Files:**
- Modify: `src/config.rs`（`HotkeyConfig`、`Config.hotkeys`、`Raw.hotkeys`、`Raw::resolve`、`write_default` 模板、tests）

**Interfaces:**
- Consumes: `hotkeys::{DEFAULTS, validate, Binding}`
- Produces:
  - `pub struct HotkeyConfig { pub play_pause: String, pub next: String, pub prev: String, pub volume_up: String, pub volume_down: String }`，派生 `Debug, Clone, PartialEq, Eq, Deserialize` + `#[serde(default)]`；
  - `impl Default for HotkeyConfig`（按 `DEFAULTS` 填）；
  - `impl HotkeyConfig { fn get(&self, field: &str) -> &str; fn set(&mut self, field: &str, keys: &str); pub fn bindings(&self) -> Vec<hotkeys::Binding> }`；
  - `Config.hotkeys: HotkeyConfig`

- [ ] **Step 1: 写失败的测试**

加到 `src/config.rs` 现有的 `mod tests` 里，复用那里的 `load_from(tag, text) -> (Config, Option<String>)`。

```rust
#[test]
fn the_missing_hotkey_section_gives_the_default_combos() {
    let (config, warning) = load_from("hotkeys-missing", "quality = \"flac\"\n");
    assert_eq!(config.hotkeys, HotkeyConfig::default());
    assert_eq!(config.hotkeys.play_pause, "ctrl+shift+space");
    assert_eq!(config.hotkeys.volume_down, "ctrl+shift+down");
    assert_eq!(config.hotkeys.bindings().len(), 5);
    assert!(warning.is_none(), "缺段不是错误：{warning:?}");
}

/// 写了 `[hotkeys]` 但只写一项：其余四项还得是默认值。
#[test]
fn a_partial_section_keeps_the_other_defaults() {
    let (config, warning) = load_from("hotkeys-partial", "[hotkeys]\nnext = \"ctrl+alt+n\"\n");
    assert_eq!(config.hotkeys.next, "ctrl+alt+n");
    assert_eq!(config.hotkeys.prev, "ctrl+shift+left", "没写的字段用默认组合");
    assert_eq!(config.hotkeys.bindings().len(), 5);
    assert!(warning.is_none(), "{warning:?}");
}

#[test]
fn an_empty_string_disables_one_action() {
    let (config, warning) = load_from("hotkeys-empty", "[hotkeys]\nplay_pause = \"\"\n");
    assert_eq!(config.hotkeys.play_pause, "");
    let bindings = config.hotkeys.bindings();
    assert_eq!(bindings.len(), 4);
    assert!(!bindings.iter().any(|binding| binding.action == keys::Action::PlayPause));
    assert!(warning.is_none(), "明确禁用不该警告：{warning:?}");
}

/// 裸键会被系统级抢走：不注册、清成空串、给一条点名到字段的警告。
#[test]
fn a_bare_key_is_refused_and_cleared() {
    let (config, warning) = load_from("hotkeys-bare", "[hotkeys]\nplay_pause = \"space\"\n");
    assert_eq!(config.hotkeys.play_pause, "", "非法值要清掉，别留个不会注册的字符串");
    assert_eq!(config.hotkeys.bindings().len(), 4, "其余四条照旧");
    let warning = warning.expect("应给出警告");
    assert!(warning.contains("[hotkeys] play_pause"), "{warning}");
    assert!(warning.contains("space"), "{warning}");
}

#[test]
fn a_bad_key_name_is_refused_and_cleared() {
    let (config, warning) = load_from("hotkeys-bad", "[hotkeys]\nvolume_up = \"ctrl+shift+f13\"\n");
    assert_eq!(config.hotkeys.volume_up, "");
    assert_eq!(config.hotkeys.next, "ctrl+shift+right", "坏字段不影响别的字段");
    let warning = warning.expect("应给出警告");
    assert!(warning.contains("[hotkeys] volume_up"), "{warning}");
}

#[test]
fn a_combo_may_be_written_in_any_case() {
    let (config, warning) = load_from("hotkeys-case", "[hotkeys]\nplay_pause = \"Ctrl+Shift+Space\"\n");
    assert_eq!(config.hotkeys.play_pause, "Ctrl+Shift+Space", "合法就原样留着");
    assert!(warning.is_none(), "{warning:?}");
    assert_eq!(config.hotkeys.bindings()[0].keys, "Ctrl+Shift+Space");
}

/// 模板里五组默认组合都要写出来（值跟 `DEFAULTS` 一致），并提醒会吞键。
#[test]
fn the_generated_template_documents_the_hotkeys() {
    let dir = std::env::temp_dir().join(format!("canto-config-hotkeys-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("建临时目录");
    let path = dir.join("config.toml");
    write_default(&path).expect("写默认配置");
    let text = std::fs::read_to_string(&path).expect("读回配置");
    std::fs::remove_dir_all(&dir).ok();

    assert!(text.contains("[hotkeys]"), "{text}");
    for (field, keys, _) in hotkeys::DEFAULTS {
        assert!(text.contains(&format!("{field} = \"{keys}\"")), "模板里缺 {field}：{text}");
    }
    assert!(text.contains("别的程序收不到"), "得说清注册期间组合是被独占的：{text}");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib config`
Expected: 编译失败（`HotkeyConfig` / `Config.hotkeys` / `hotkeys::DEFAULTS` 未定义）。

- [ ] **Step 3: 实现**

- `use crate::{hotkeys, tui::keys};`（`keys` 仅供测试里的 `Action` 比较；若正式代码用不到，把它放进 `mod tests` 的 `use`，别让 clippy 报 unused）。
- `Config` 加字段 `pub hotkeys: HotkeyConfig,`（`Config::default()` 里 `hotkeys: HotkeyConfig::default()`）。
- `HotkeyConfig` 定义 + `#[serde(default)]`；`Default` 实现里先建 5 个空串再 `for (field, keys, _) in hotkeys::DEFAULTS { config.set(field, keys); }`。
- `get` / `set` 就是 5 个 `match` 分支（`_ => ""` / `_ => {}`），注释说明「只有这一处需要按名字找字段」。
- `bindings()`：按 `hotkeys::DEFAULTS` 顺序取 `get(field)`，空串跳过，其余 `Binding { action, keys: 取到的原文.to_string() }`。
- `Raw` 加 `hotkeys: Option<HotkeyConfig>,`。
- `Raw::resolve` 里（其余字段解析之后）：
  ```rust
  let mut hotkeys = self.hotkeys.unwrap_or_default();
  for (field, _, _) in hotkeys::DEFAULTS {
      let keys = hotkeys.get(field).to_string();
      if keys.is_empty() {
          continue;
      }
      if let Err(why) = crate::hotkeys::validate(&keys) {
          warnings.push(format!("[hotkeys] {field} 不可用:{why}"));
          hotkeys.set(field, "");
      }
  }
  config.hotkeys = hotkeys;
  ```
  （`to_string()` 是为了不再借 `hotkeys`，接着要 `set` 它。）
- `write_default` 的模板里加一段，注释照抄 spec §3 的写法，末尾带「注册期间别的程序收不到这些组合」的提醒；五个值直接写字面量（`the_generated_template_documents_the_hotkeys` 会盯着它们跟 `DEFAULTS` 一致）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib config`
Expected: 新增 7 条 + 原有 config 测试全 PASS。

- [ ] **Step 5: 提交**

```bash
git add src/config.rs
git commit -m "feat(config): [hotkeys] 段(默认组合/空串禁用/裸键拒收)"
```

---

# Task 3: 接线——`App.hotkeys`、`tick()` 派发、`tui::run` 的安装与通知

这一步做完，热键在 Windows 上还不会真响（Task 4 才换真实现），但派发路径与屏门已经钉死了。

**Files:**
- Modify: `src/tui/state.rs`（字段、`tick()`、`dispatch_hotkey()`、tests）
- Modify: `src/tui/mod.rs`（`tui::run`）

**Interfaces:**
- Consumes: `hotkeys::{install, Hotkeys, InstallReport}`、`config.hotkeys.bindings()`
- Produces: `App.hotkeys: hotkeys::Hotkeys`、`App::dispatch_hotkey(&mut self, action: keys::Action)`

- [ ] **Step 1: 写失败的测试**

加到 `src/tui/state.rs` 的 `mod tests`（那里已经有 `crate::tui::test_support::test_app()` 的用法）。用 `Action::Help`（可观察、不依赖真音频）验屏门，用不到引擎：

```rust
/// 全局热键只在主界面生效：登录屏没有播放可控，按了就当没按。
#[test]
fn hotkeys_never_reach_the_login_screen() {
    let (mut app, _runtime) = crate::tui::test_support::test_app();
    app.screen = Screen::Login(LoginState::default());
    app.dispatch_hotkey(keys::Action::Help);
    assert!(!app.help_open, "登录屏不该被全局热键穿透");

    app.screen = Screen::Main;
    app.dispatch_hotkey(keys::Action::Help);
    assert!(app.help_open, "主界面要跟按 `?` 一样生效");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib hotkeys_never_reach`
Expected: 编译失败（`dispatch_hotkey` / `App.hotkeys` 不存在）。

- [ ] **Step 3: 实现**

`src/tui/state.rs`：
- `use crate::hotkeys;`
- `App` 加字段 `pub(super) hotkeys: hotkeys::Hotkeys,`，`App::new` 里 `hotkeys: hotkeys::Hotkeys::none(),`（注释：真注册只在 `tui::run` 里发生，测试里永远是空句柄）。
- 方法：
  ```rust
  /// 全局热键落到哪：只有主界面有东西可控，登录屏直接丢。
  pub(super) fn dispatch_hotkey(&mut self, action: keys::Action) {
      if let Screen::Main = self.screen {
          self.apply(action);
      }
  }
  ```
- `tick()` 里加（放在最前面，跟其它「先收事件再画」的顺序一致）：
  ```rust
  // 全局热键：canto 在后台时也要能控制播放。
  for action in self.hotkeys.drain() {
      self.dispatch_hotkey(action);
  }
  ```

`src/tui/mod.rs` 的 `tui::run`：
- `App::new` 之前：`let bindings = config.hotkeys.bindings();`（`config` 接下来会被 move 进去）。
- `App::new` 之后：
  ```rust
  let (hotkeys, report) = hotkeys::install(&bindings);
  app.hotkeys = hotkeys;
  if report.registered > 0 {
      tracing::info!("已注册 {} 个全局热键", report.registered);
  }
  ```
- 通知：现有那条配置警告之后再加
  ```rust
  if let Some(text) = report.notice() {
      app.notify(text);
  }
  ```
  （后写的占槽，配置警告先报、注册失败后报，完整内容都在日志里。）
- 主循环每轮开头（第一次 `tick()` 之前）加 `app.hotkeys.pump();`（只有 macOS 干实事）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib` 与 `cargo clippy --all-targets --locked -- -D warnings`
Expected: 全 PASS、无警告。

- [ ] **Step 5: 提交**

```bash
git add src/tui/state.rs src/tui/mod.rs
git commit -m "feat(tui): 全局热键接进 tick 派发(只在主界面生效)"
```

---

# Task 4: Windows 真实现——专用线程 + 消息循环

crate 的 `GlobalHotKeyManager::new()` 在**调用线程**建隐藏窗口，`RegisterHotKey(hwnd, …)` 把 `WM_HOTKEY` 投给那个窗口所在线程的队列；crate 自己不跑消息循环（只有 WndProc，收到 `WM_HOTKEY` 后 `send` 事件、再 spawn 一个线程轮询到松键补发 `Released`）。所以循环得我们跑。

**Files:**
- Modify: `src/hotkeys.rs`（`#[cfg(target_os = "windows")] mod imp`）
- Modify: `Cargo.toml`（给已有 `windows-sys` 那一行补 `Win32_UI_WindowsAndMessaging`）

**Interfaces:**
- Consumes: `super::{Binding, InstallReport, Action, id_map, validate}`
- Produces: `imp::{Imp, Imp::none(), Imp::pump(), install(&[Binding]) -> (Imp, Vec<(u32, Action)>, InstallReport)}`（签名与 Task 1 的空壳一致，只换实现）

- [ ] **Step 1: 实现（这个任务的验收是真机清单，不是单测）**

`mod imp` 的内容：

```rust
#[cfg(target_os = "windows")]
mod imp {
    use std::collections::HashMap;
    use std::sync::mpsc::{channel, Sender};
    use std::thread::JoinHandle;

    use global_hotkey::GlobalHotKeyManager;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, PostThreadMessageW, TranslateMessage, MSG, WM_QUIT,
    };

    use super::{id_map, validate, Action, Binding, InstallReport};

    /// 线程回报：`Err` = 线程活着但起不来，`Ok(Started)` = 注册结果。
    struct Started {
        thread_id: u32,
        registered: Vec<(u32, Action)>,
        failures: Vec<String>,
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

        /// Windows 没有「每帧要做的事」：消息由专用线程处理。
        pub(super) fn pump(&self) {}
    }

    impl Drop for Imp {
        fn drop(&mut self) {
            let Some(join) = self.join.take() else { return };
            // 线程卡在 `GetMessageW` 上，只能投消息叫醒它。
            unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
            let _ = join.join();
        }
    }

    pub(super) fn install(bindings: &[Binding]) -> (Imp, Vec<(u32, Action)>, InstallReport) {
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
                return (
                    Imp::none(),
                    Vec::new(),
                    InstallReport { registered: 0, failures: vec!["无法启动热键线程".to_string()], unsupported: false },
                );
            }
        };
        match rx.recv() {
            Ok(Ok(started)) => {
                let report = InstallReport {
                    registered: started.registered.len(),
                    failures: started.failures,
                    unsupported: false,
                };
                (
                    Imp { thread_id: started.thread_id, join: Some(join) },
                    started.registered,
                    report,
                )
            }
            Ok(Err(reason)) => {
                let _ = join.join();
                (
                    Imp::none(),
                    Vec::new(),
                    InstallReport { registered: 0, failures: vec![reason], unsupported: false },
                )
            }
            Err(_) => (
                Imp::none(),
                Vec::new(),
                InstallReport { registered: 0, failures: vec!["热键线程提前退出".to_string()], unsupported: false },
            ),
        }
    }

    /// 专用线程：建窗口（crate 在那个线程上建）→ 逐条注册 → 回报 → 跑消息循环。
    fn run(bindings: Vec<Binding>, tx: Sender<Result<Started, String>>) {
        let thread_id = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
        let manager = match GlobalHotKeyManager::new() {
            Ok(manager) => manager,
            Err(error) => {
                tracing::warn!(%error, "全局热键管理器建不起来");
                let _ = tx.send(Err(format!("无法初始化热键管理器：{error}")));
                return;
            }
        };
        let mut registered = Vec::new();
        let mut failures = Vec::new();
        for binding in &bindings {
            let hotkey = match validate(&binding.keys) {
                Ok(hotkey) => hotkey,
                Err(why) => {
                    tracing::warn!(combo = %binding.keys, %why, "全局热键不合法");
                    failures.push(binding.keys.clone());
                    continue;
                }
            };
            match manager.register(hotkey) {
                Ok(()) => registered.push((hotkey.id(), binding.action)),
                Err(error) => {
                    tracing::warn!(combo = %binding.keys, %error, "全局热键注册失败");
                    failures.push(binding.keys.clone());
                }
            }
        }
        // 窗口一建出来这个线程就有消息队列了，所以 `Drop` 里投的 `WM_QUIT` 一定有人收。
        let _ = tx.send(Ok(Started { thread_id, registered, failures }));

        // `WM_HOTKEY` 是投递（不是发送）给这个线程的，得自己取出来派发。
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
```

`Cargo.toml`：已有那行 `windows-sys = { version = "0.61", features = [...] }` 的 feature 里加 `"Win32_UI_WindowsAndMessaging"`（`GetCurrentThreadId` 走已有的 `Win32_System_Threading`）。

- [ ] **Step 2: 编译与测试（本机能全绿）**

Run: `cargo test --locked` 与 `cargo clippy --all-targets --locked -- -D warnings`
Expected: 全 PASS、无警告（Task 1 的 `ids_map_once_and_only_pressed_events_win` 覆盖映射与事件过滤；线程本身没法单测）。

- [ ] **Step 3: 真机验收（Windows 本机，tuios 里跑 `cargo run --release`）**

1. 启动后 `canto.log` 有 `已注册 5 个全局热键`，状态行没有失败通知。
2. **焦点切到别的窗口**（另一个 pane / 编辑器），按 `ctrl+shift+space` → 播放/暂停（看播放条与日志）。
3. `ctrl+shift+up` / `ctrl+shift+down` → 播放条音量 ±5%。
4. `ctrl+shift+left` / `ctrl+shift+right` → 播放条曲名切换。
5. **冲突**：临时把 `next` 改成与 `prev` 相同 → 启动后状态行出现 `热键注册失败:…`，日志有原因，其余 4 条仍可用。
6. **归还**：退出 canto 后用一个小程序（或另开一个 canto 实例之前）`RegisterHotKey` 同一组合能成功注册；退出时不许 panic、不许卡住。
7. 把配置改回默认，确认 `cargo test` 仍全绿。

- [ ] **Step 4: 提交**

```bash
git add src/hotkeys.rs Cargo.toml Cargo.lock
git commit -m "feat(hotkeys): Windows 真实现(专用线程跑消息循环)"
```

---

# Task 5: macOS 真实现——主线程 manager + 每轮泵 run loop

crate 在 macOS 上把事件处理器 `InstallEventHandler` 装在 **application event target** 上，事件靠应用主 run loop 派发；canto 没有 NSApplication，所以得自己在主线程每轮泵一次。**这一步的代码本机（Windows）编不了**，第一次编译发生在 CI 的 `test (macos-15)` job。

**Files:**
- Modify: `src/hotkeys.rs`（`#[cfg(target_os = "macos")] mod imp`）

**Interfaces:**
- Consumes: 同 Task 4
- Produces: `imp::install` 的 macOS 实现（签名不变）

- [ ] **Step 1: 实现**

```rust
#[cfg(target_os = "macos")]
mod imp {
    use std::collections::HashMap;
    use std::ffi::c_void;

    use global_hotkey::GlobalHotKeyManager;

    use super::{id_map, validate, Action, Binding, InstallReport};

    pub(super) struct Imp {
        /// `None` = 没注册任何东西（`Hotkeys::none()` 与「建 manager 失败」都是这个形态）。
        manager: Option<GlobalHotKeyManager>,
    }

    impl Imp {
        pub(super) fn none() -> Self {
            Self { manager: None }
        }

        /// 泵一次主 run loop：`0.0` 秒 = 处理完手上已有的事件就返回，不阻塞 UI。
        pub(super) fn pump(&self) {
            unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.0, 1) };
        }
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        /// `CFStringRef kCFRunLoopDefaultMode`（系统导出的是全局变量）。
        static kCFRunLoopDefaultMode: *const c_void;
        /// `CFRunLoopRunResult CFRunLoopRunInMode(CFStringRef, CFTimeInterval, Boolean)`。
        fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after_source_handled: u8) -> i32;
    }

    pub(super) fn install(bindings: &[Binding]) -> (Imp, Vec<(u32, Action)>, InstallReport) {
        // 必须在主线程建（`InstallEventHandler` 挂在 application event target 上）。
        let manager = match GlobalHotKeyManager::new() {
            Ok(manager) => manager,
            Err(error) => {
                tracing::warn!(%error, "全局热键管理器建不起来");
                return (
                    Imp::none(),
                    Vec::new(),
                    InstallReport { registered: 0, failures: vec![format!("无法初始化热键管理器：{error}")], unsupported: false },
                );
            }
        };
        let mut registered = Vec::new();
        let mut failures = Vec::new();
        for binding in bindings {
            let hotkey = match validate(&binding.keys) {
                Ok(hotkey) => hotkey,
                Err(why) => {
                    tracing::warn!(combo = %binding.keys, %why, "全局热键不合法");
                    failures.push(binding.keys.clone());
                    continue;
                }
            };
            match manager.register(hotkey) {
                Ok(()) => registered.push((hotkey.id(), binding.action)),
                Err(error) => {
                    tracing::warn!(combo = %binding.keys, %error, "全局热键注册失败");
                    failures.push(binding.keys.clone());
                }
            }
        }
        let report = InstallReport { registered: registered.len(), failures, unsupported: false };
        (Imp { manager: Some(manager) }, registered, report)
    }
}
```

不需要新的 crate 依赖：`CoreFoundation` 是系统框架，`#[link(..., kind = "framework")]` 声明就够了（crate 自己链的是 Carbon，我们补一个）。

- [ ] **Step 2: 本机能做的验证（编译 + Windows 侧不回归）**

Run: `cargo test --locked` 与 `cargo clippy --all-targets --locked -- -D warnings`
Expected: 全 PASS（macOS 代码不参与本机编译）。
推分支后看 CI 的 `test (macos-15)` job：这一步才算 macOS 代码「编得过」。

- [ ] **Step 3: 交给使用者的真机验收（macOS，必须有）**

1. 启动后日志有 `已注册 5 个全局热键`，状态行无失败通知；**不应出现辅助功能授权弹窗**（我们不用媒体键，不装 CGEventTap）。
2. 焦点切到别的应用，按 `ctrl+shift+space` / `up` / `down` / `left` / `right` → 播放条上能看到暂停、音量、曲名变化（延迟 ≤ 200ms）。
3. 冲突场景同 Task 4 第 5 条。
4. 退出后组合归还（重开 canto 能再注册）。
5. **若第 2 条不生效**（`CFRunLoopRunInMode` 泵不出 Carbon 热键事件）：按 spec §9 的退路——macOS 直接不注册，启动时给一句 `此平台暂不支持全局热键`，并在 `docs/design.md` 的已知限制里写明。

- [ ] **Step 4: 提交**

```bash
git add src/hotkeys.rs
git commit -m "feat(hotkeys): macOS 实现(主线程 manager + 每轮泵 run loop)"
```

---

# Task 6: 文档与收尾

**Files:**
- Modify: `README.md`、`docs/design.md`
- Modify: `docs/superpowers/specs/2026-10-12-global-hotkeys-design.md`（把「core-foundation 依赖」那处改成实际方案）

- [ ] **Step 1: README**

- 「功能」表加一行：全局快捷键（Windows / macOS）——后台也能播放/暂停、上下首、音量。
- 配置段加 `[hotkeys]`：五个字段、默认值、`""` 禁用、必须带修饰键。
- 已知限制加两条：Linux 不注册（启动时提示一句）；注册期间这些组合被 canto 独占，别的程序收不到。

- [ ] **Step 2: `docs/design.md` 加一节「全局快捷键」**

写清三件实现层的事：Windows 为什么要自建消息循环线程（crate 只建窗口）；macOS 为什么每轮泵 run loop（事件挂在 application event target 上）；为什么只在 `Screen::Main` 派发、失败为什么逐条报。

- [ ] **Step 3: 改 spec 里那处过期描述**

spec §1「macOS：额外 `core-foundation`（我们要用它泵 run loop）」与实际不符：crate 没有这个依赖，我们用的是 2 个符号的自带 FFI。改成实际做法，并在「9. 风险与已知限制」保留「本机验不了、退路是不注册」。

- [ ] **Step 4: 收尾验证**

Run: `cargo fmt --all --check`、`cargo clippy --all-targets --locked -- -D warnings`、`cargo test --locked`
Expected: 三条都干净。
再按 Task 4 的清单在本机真跑一遍（这是 Task 6 的唯一人工验收项）。

- [ ] **Step 5: 提交**

```bash
git add README.md docs/design.md docs/superpowers/specs/2026-10-12-global-hotkeys-design.md
git commit -m "docs: 全局快捷键(README 功能/配置/限制 + design 线程模型)"
```

---

## 自查记录（写计划时跑过）

- **spec 覆盖**：默认组合与字段名（Task 1 `DEFAULTS` + 测试、Task 2 配置测试）、`[hotkeys]` 解析与 `""` 禁用（Task 2）、必须带修饰键（Task 1 `validate` + Task 2）、非法值警告（Task 2）、逐条注册失败 + 一行通知 + 日志（Task 1 `notice` + Task 4/5 的 `tracing::warn!`）、不支持平台一句话（Task 1 的 `cfg` 测试）、只在 `Screen::Main` 派发（Task 3）、`install` 在 `tui::run` 而不是 `App::new`（Task 3）、Windows 专用线程与消息循环（Task 4）、macOS 主线程 + 泵 run loop（Task 5）、退出反注册（Task 4 `Drop`）、README/design/模板文档（Task 2 模板测试 + Task 6）、macOS 未验证的退路（Task 5 Step 3 第 5 条）。
- **步骤扫描**：每个步骤一件事、有可检查的结果；没有 TBD / 「稍后处理」；唯一给出完整代码的是 Task 4 的线程与消息循环（签名与测试定不下来，属于「签名定不了的部分」）与 Task 5 的 FFI 声明（符号形状必须写死），其余都是签名 + 断言。
- **类型一致**：`Binding` / `DEFAULTS` / `validate` / `InstallReport::notice` / `Hotkeys::{none, drain, pump}` / `imp::install` 的签名在 Task 1 定死，Task 2–5 只引用不改名；`HotkeyConfig::bindings()` 产出的 `keys` 一律是**原文**（大小写原样），要注册时才解析。
- **Review Focus 逐条挂了测试**：1→Task 1 `one_failure_is_named_and_several_are_counted` + Task 4 清单 5；2→Task 1 `a_combo_without_a_modifier_is_refused` + Task 2 `a_bare_key_is_refused_and_cleared`；3→Task 2 `an_empty_string_disables_one_action`；4→Task 2 `a_bad_key_name_is_refused_and_cleared`；5→Task 3 `hotkeys_never_reach_the_login_screen`；6→Task 4 清单 6。
- **篇幅**：计划约 500 行，对应新增代码约 320 行（`hotkeys.rs` 含测试）+ 配置/接线约 120 行 + 文档，同一量级——没有把代码抄进计划。

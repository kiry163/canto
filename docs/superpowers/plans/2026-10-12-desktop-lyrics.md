# canto 桌面歌词实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 播放时在 Windows 桌面之上浮一条歌词（当前句 + 翻译 + 下一句），不抢焦点、默认鼠标穿透、按全局热键解锁后可拖动。

**Architecture:** 新增 `src/overlay.rs`，切成两半：平台无关的**状态机**（`Frame` → `Payload`，只在变化时产出）与平台相关的**窗口线程**（自建分层窗口 + 消息循环 + 32 位 DIB + `UpdateLayeredWindow`）。TUI 线程每帧 `sync()` 一次，窗口线程只收 `Arc<Mutex<Payload>>` + `PostMessageW` 唤醒。

**Tech Stack:** Rust 2021/2024、`windows-sys 0.61`（补 `Win32_Graphics_Gdi`、`Win32_UI_HiDpi` 两个 feature，无新 crate）、GDI 画字、`image`（探针存 PNG 用，已有依赖）。

**Spec:** `docs/superpowers/specs/2026-10-12-desktop-lyrics-design.md`

## Global Constraints

- **平台**：只做 Windows；`#[cfg(not(windows))]` 那份是空实现（`none()` + 空方法）。macOS 留给以后，接缝见 spec §5。
- **依赖**：**不新增任何 crate**；只给已有的 `windows-sys`（`[target.'cfg(windows)'.dependencies]`）补两个 feature。
- **更新节奏**：只在 payload 变化时推一次；**没有逐帧绘制、没有轮询定时器**。
- **位置**：存**中心点**在独立状态文件 `~/.config/canto/desktop-lyrics.json`（临时文件 + rename，与 `history.rs` 同一套）；**不写 `config.toml`**。锁定态不持久化，启动一律锁定。
- **配置**：`[desktop_lyrics]` 9 个键，默认值照 spec §4；颜色非法 → 用默认值 + 一条点名警告；`opacity` 超范围 → 夹住 + 警告；`enabled` 默认 `true`。
- **`[hotkeys]` 从 5 条变 6 条**（`lock = "ctrl+shift+l"`）：这是本次唯一改到已发布功能的地方，Task 4 里逐条列出要一起改的断言。
- **通知文案**：`NOTICE_COLS = 32`（`src/tui/draw/status.rs`），短句、无路径、无错误链，细节进 `canto.log`（2026-10-12 定的约定）。
- **热键派发**仍只在 `Screen::Main`（包括新的 `lock`）。
- 每个 commit 都要三平台编得过：CI = Linux `clippy --all-targets --locked -- -D warnings` + ubuntu/windows/macos `cargo test --locked`。

## Review Focus

（spec 没铺开写、但真会遇到的输入，各挂到拥有它的任务上）

1. **「纯音乐段」（歌词里当前行是空文本）与「完全没有歌词」要分开**：前者显示下一句、后者整窗隐藏；两者都不能留下上一句的残影。（Task 2 `an_empty_line_shows_only_the_next_one`、`nothing_playing_hides_the_overlay`）
2. **状态文件坏掉 / 中心点落在主屏之外**（换过显示器）→ 不阻塞启动、夹回主屏可见位置。（Task 2 `a_broken_position_file_falls_back_to_the_default`、`a_saved_center_is_clamped_onto_the_primary_screen`）
3. **关掉功能或非 Windows** → 零成本、不建线程、不报错。（Task 2 `a_none_handle_syncs_nothing` + Task 6 真机）
4. **拖出来的位置与配置里的偏好互不覆盖**，重启回到拖到的位置。（Task 2 `the_position_file_round_trips` + Task 6 真机）
5. **建窗口失败**（极少见）→ 一句话 + 日志，播放与终端界面照常。（Task 3 的失败路径 + Task 4 的通知接线）

## 文件结构

| 文件 | 责任 |
|---|---|
| `src/overlay.rs`（新，约 400–450 行含测试） | 平台无关状态机 + 位置状态 + Windows 窗口线程（`cfg` 子模块） |
| `src/config.rs` | `[desktop_lyrics]` 段（含颜色解析）；Task 4 里加 `HotkeyConfig.lock` |
| `src/paths.rs` | `desktop_lyrics_path()` |
| `src/tui/state.rs`、`src/tui/mod.rs` | 接线：字段、`tick()` 里 sync、位置落盘、启动安装与失败通知 |
| `src/tui/keys.rs` | `Action::LyricsLock` + `apply` 分支 |
| `src/hotkeys.rs` | `DEFAULTS` 第 6 条 |
| `Cargo.toml` | `windows-sys` 补两个 feature |
| `README.md`、`docs/design.md` | 功能表、配置段、已知限制、实现说明 |

---

# Task 1: `[desktop_lyrics]` 配置段

**Files:**
- Modify: `src/config.rs`（`DesktopLyricsConfig`、`Raw.desktop_lyrics`、`resolve`、`write_default` 模板、tests）

**Interfaces:**
- Produces:
  - `pub struct DesktopLyricsConfig { pub enabled: bool, pub font: String, pub font_size: u32, pub color: Rgb, pub outline: Rgb, pub next_color: Rgb, pub opacity: u8, pub show_translation: bool, pub hide_when_paused: bool }`（`Debug, Clone, PartialEq, Eq, Deserialize` + `#[serde(default)]`）
  - `pub struct Rgb { pub r: u8, pub g: u8, pub b: u8 }` + `impl Rgb { pub fn parse(text: &str) -> Option<Self> }`（认 `#RRGGBB` 与 `RRGGBB`，大小写都行）
  - `Config.desktop_lyrics: DesktopLyricsConfig`
- 这个任务**不改** `[hotkeys]`（第 6 条留给 Task 4，那时才有 `LyricsLock` 动作可指）。

- [ ] **Step 1: 写失败的测试**

加到 `src/config.rs` 现有的 `mod tests`（复用那边的 `load_from(tag, text)`）：

```rust
#[test]
fn the_missing_desktop_lyrics_section_gives_the_documented_defaults() {
    let (config, warning) = load_from("lyrics-missing", "quality = \"flac\"\n");
    assert_eq!(config.desktop_lyrics, DesktopLyricsConfig::default());
    assert!(config.desktop_lyrics.enabled, "默认就开（跟 [hotkeys] 一个口径）");
    assert_eq!(config.desktop_lyrics.font, "Microsoft YaHei UI");
    assert_eq!(config.desktop_lyrics.font_size, 28);
    assert_eq!(config.desktop_lyrics.opacity, 90);
    assert!(!config.desktop_lyrics.hide_when_paused);
    assert!(warning.is_none(), "缺段不是错误：{warning:?}");
}

#[test]
fn colors_parse_in_both_cases_and_warn_on_garbage() {
    assert_eq!(Rgb::parse("#FFFFFF"), Some(Rgb { r: 255, g: 255, b: 255 }));
    assert_eq!(Rgb::parse("ff0000"), Some(Rgb { r: 255, g: 0, b: 0 }));
    assert_eq!(Rgb::parse("#00ff7F"), Some(Rgb { r: 0, g: 255, b: 127 }));
    assert_eq!(Rgb::parse("red"), None);
    assert_eq!(Rgb::parse("#12345"), None);
    assert_eq!(Rgb::parse(""), None);

    // 写坏了只影响那一个字段：用默认值 + 点名字段的一条警告。
    let (config, warning) = load_from("lyrics-color", "[desktop_lyrics]\ncolor = \"red\"\n");
    assert_eq!(config.desktop_lyrics.color, DesktopLyricsConfig::default().color);
    let warning = warning.expect("应给出警告");
    assert!(warning.contains("[desktop_lyrics] color"), "{warning}");
    assert!(warning.contains("red"), "{warning}");
}

#[test]
fn opacity_is_clamped_and_warned() {
    let (config, warning) = load_from("lyrics-opacity", "[desktop_lyrics]\nopacity = 150\n");
    assert_eq!(config.desktop_lyrics.opacity, 100, "夹到上限");
    let warning = warning.expect("应给出警告");
    assert!(warning.contains("[desktop_lyrics] opacity"), "{warning}");

    let (config, warning) = load_from("lyrics-opacity-ok", "[desktop_lyrics]\nopacity = 40\n");
    assert_eq!(config.desktop_lyrics.opacity, 40);
    assert!(warning.is_none(), "{warning:?}");
}

/// 只写一项：其余字段还是默认值。
#[test]
fn a_partial_desktop_lyrics_section_keeps_the_other_defaults() {
    let (config, warning) =
        load_from("lyrics-partial", "[desktop_lyrics]\nfont_size = 36\nhide_when_paused = true\n");
    assert_eq!(config.desktop_lyrics.font_size, 36);
    assert!(config.desktop_lyrics.hide_when_paused);
    assert_eq!(config.desktop_lyrics.font, "Microsoft YaHei UI", "没写的还是默认");
    assert_eq!(config.desktop_lyrics.opacity, 90);
    assert!(warning.is_none(), "{warning:?}");
}

/// 模板里九个键都写出来（值跟默认值一致），否则「改配置」这件事没有说明书。
#[test]
fn the_generated_template_documents_desktop_lyrics() {
    let dir = std::env::temp_dir().join(format!("canto-config-lyrics-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("建临时目录");
    let path = dir.join("config.toml");
    write_default(&path).expect("写默认配置");
    let text = std::fs::read_to_string(&path).expect("读回配置");
    std::fs::remove_dir_all(&dir).ok();

    assert!(text.contains("[desktop_lyrics]"), "{text}");
    let defaults = DesktopLyricsConfig::default();
    for key in [
        format!("enabled = {}", defaults.enabled),
        format!("font = \"{}\"", defaults.font),
        format!("font_size = {}", defaults.font_size),
        format!("opacity = {}", defaults.opacity),
        format!("show_translation = {}", defaults.show_translation),
        format!("hide_when_paused = {}", defaults.hide_when_paused),
    ] {
        assert!(text.contains(&key), "模板里缺 {key}：{text}");
    }
    assert!(text.contains("color = \"#FFFFFF\""), "{text}");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test config::`
Expected: 编译失败（`DesktopLyricsConfig` / `Rgb` / `Config.desktop_lyrics` 未定义）。

- [ ] **Step 3: 实现**

- `Rgb`：`parse` 去掉可选 `#`，要求 6 个十六进制字符（`u8::from_str_radix`），两头都 `Option`。
- `DesktopLyricsConfig`：`#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]` + `#[serde(default)]`，`Default` 按 spec §4 的九个值。
- `Config` 加 `pub desktop_lyrics: DesktopLyricsConfig`（`Default` 里 `DesktopLyricsConfig::default()`）。
- `Raw::resolve` 里（`hotkeys` 之后）：
  - `Raw` 加 `desktop_lyrics: Option<RawDesktopLyrics>`，`RawDesktopLyrics` 是**全 `Option` 的原始形状**
    （九个字段；三个颜色先当 `Option<String>` 收——人会写错），`resolve` 里折成 `DesktopLyricsConfig`：
    ```rust
    let raw = self.desktop_lyrics.unwrap_or_default();
    let mut lyrics = DesktopLyricsConfig::default();
    if let Some(enabled) = raw.enabled { lyrics.enabled = enabled; }
    if let Some(font) = raw.font.filter(|font| !font.trim().is_empty()) { lyrics.font = font; }
    if let Some(size) = raw.font_size { lyrics.font_size = size.max(8); }
    take_color("color", raw.color, &mut lyrics.color, warnings);
    take_color("outline", raw.outline, &mut lyrics.outline, warnings);
    take_color("next_color", raw.next_color, &mut lyrics.next_color, warnings);
    if let Some(opacity) = raw.opacity {
        if opacity > 100 {
            warnings.push(format!("[desktop_lyrics] opacity = {opacity} 超出 0-100，用 100"));
        }
        lyrics.opacity = opacity.min(100);
    }
    if let Some(show) = raw.show_translation { lyrics.show_translation = show; }
    if let Some(hide) = raw.hide_when_paused { lyrics.hide_when_paused = hide; }
    config.desktop_lyrics = lyrics;
    ```
    其中 `fn take_color(field: &str, text: Option<String>, slot: &mut Rgb, warnings: &mut Vec<String>)`：
    `Some(text)` 解析失败就 `warnings.push(format!("[desktop_lyrics] {field} 不是 #RRGGBB：{text:?}（用默认值）"))`
    并保持默认值；`None` 什么都不做。（写成助手是为了三处不抄三遍，也避开「同时借 warnings 与字段」的麻烦。）
- `write_default` 模板加一段（九个键 + 注释，注释讲清 `enabled`/`opacity`/`hide_when_paused` 是什么意思、颜色写 `#RRGGBB`）。

- [ ] **Step 4: 跑测试与 clippy 确认通过**

Run: `cargo test config::` → 新增 5 条 + 原有全 PASS。
Run: `cargo clippy --all-targets --locked` → 除「消费方还没接线」那类 dead_code 外无其他告警（Task 4 收尾时要求 `-D warnings` 干净）。

- [ ] **Step 5: 提交**

```bash
git add src/config.rs
git commit -m "feat(config): [desktop_lyrics] 段(九个键 + 颜色/透明度校验)"
```

---

# Task 2: `overlay.rs` 的平台无关一半（状态机 + 位置状态）

这一半是这份功能里唯一能单测的部分，也是将来 macOS 版不用动的部分。

**Files:**
- Create: `src/overlay.rs`
- Modify: `src/main.rs`（模块清单里 `mod overlay;`）、`src/paths.rs`（`desktop_lyrics_path()`）

**Interfaces**（Step 3 要落成的形状，Task 3/4 按它接）：
- Consumes: `crate::model::{Lyric, LyricLine}`、`crate::player::{Phase, Snapshot}`、`crate::config::DesktopLyricsConfig`、`crate::paths::desktop_lyrics_path()`
- Produces（公开的）:
  - `#[derive(Clone, Copy)] pub struct Frame<'a> { pub lyric: Option<&'a Lyric>, pub snapshot: &'a Snapshot }`
  - `#[derive(Debug, Clone, PartialEq, Eq)] pub struct Payload { pub visible: bool, pub current: String, pub next: String, pub translation: Option<String>, pub locked: bool }`
  - `pub struct Installed { pub overlay: DesktopLyrics, pub positions: Option<Receiver<(i32, i32)>>, pub error: Option<String> }`
  - `pub struct DesktopLyrics { … }` + `none() -> Self`、`install(&DesktopLyricsConfig, Option<(i32, i32)>) -> Installed`、
    `sync(&mut self, Frame<'_>, &DesktopLyricsConfig)`、`toggle_lock(&mut self)`、`locked(&self) -> bool`
  - `pub fn load_center(path: &Path) -> Option<(i32, i32)>`、`pub fn save_center(path: &Path, center: (i32, i32)) -> Result<()>`
    （跟 `history.rs` 一样**路径当参数**：生产代码传 `paths::desktop_lyrics_path()`，测试传临时路径）
  - `pub const MARGIN: i32 = 40;`（夹取时留的边距）、`pub const UNLOCKED_HINT: &str = "拖动我（再按一次 ctrl+shift+l 锁定）";`
- 内部（同文件测试可见，不用 pub）: `struct State { last: Option<Payload>, locked: bool }`（`Default`）、
  `State::advance`、`State::toggle_lock`、`fn clamp_center(center: (i32, i32), screen: (i32, i32)) -> (i32, i32)`、
  `fn primary_screen() -> (i32, i32)`（主屏宽高）

- [ ] **Step 1: 写失败的测试**

文件末尾 `#[cfg(test)] mod tests`。两个造数据的助手 + 12 条断言（`Snapshot` 的字段都是 `pub`）：

```rust
use crate::model::{Lyric, LyricLine};
use crate::player::Phase;
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

fn snapshot(phase: Phase, secs: u64) -> Snapshot {
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

fn frame<'a>(lines: &'a Lyric, phase: Phase, secs: u64) -> (Frame<'a>, Snapshot) {
    let snapshot = snapshot(phase, secs);
    (Frame { lyric: Some(lines), snapshot: &snapshot }, snapshot)  // ← 见下面 `Frame` 是 Copy
}

/// 同一帧第二次不该再推——这就是「没有逐帧开销」的判据。
#[test]
fn the_same_frame_twice_pushes_once() {
    let lines = lyric(&[("第一句", None), ("第二句", None)]);
    let (frame, _keep) = frame(&lines, Phase::Playing, 15);

    let mut state = State::default();
    assert!(state.advance(frame, &DesktopLyricsConfig::default()).is_some(), "第一次该推");
    assert!(state.advance(frame, &DesktopLyricsConfig::default()).is_none(), "同一帧第二次不该再推");
}

/// 同一句里推进位置（进度条在动）不算变化。
#[test]
fn position_within_the_same_line_does_not_repush() {
    let lines = lyric(&[("第一句", None), ("第二句", None)]);
    let mut state = State::default();

    let (first, _keep) = frame(&lines, Phase::Playing, 10);
    assert!(state.advance(first, &DesktopLyricsConfig::default()).is_some());
    let (later, _keep2) = frame(&lines, Phase::Playing, 19);
    assert!(state.advance(later, &DesktopLyricsConfig::default()).is_none(), "还在第二句里");
}

/// 换行时当前句与下一句都要对（下一句过界就是空串）。
#[test]
fn a_new_line_pushes_current_and_next() {
    let lines = lyric(&[("第一句", None), ("第二句", None), ("第三句", None)]);
    let mut state = State::default();

    let (last, _keep) = frame(&lines, Phase::Playing, 25);
    let payload = state.advance(last, &DesktopLyricsConfig::default()).expect("换行该推");
    assert_eq!(payload.current, "第三句");
    assert_eq!(payload.next, "", "最后一句之后没有下一句");
    assert!(payload.visible);
}

/// 纯音乐段（当前行是空文本）只显示下一句，不留空框。
#[test]
fn an_empty_line_shows_only_the_next_one() {
    let lines = lyric(&[("", None), ("下一句", None)]);
    let mut state = State::default();

    let (quiet, _keep) = frame(&lines, Phase::Playing, 5);
    let payload = state.advance(quiet, &DesktopLyricsConfig::default()).expect("该推");
    assert_eq!(payload.current, "", "这一句本来就是空的");
    assert_eq!(payload.next, "下一句");
    assert!(payload.visible, "有下一句可显示，窗口要留着");
}

/// 没在播（空闲 / 取流全挂）就整窗隐藏——不显示上一句的残影。
#[test]
fn nothing_playing_hides_the_overlay() {
    let lines = lyric(&[("第一句", None)]);
    let mut state = State::default();

    let (idle, _keep) = frame(&lines, Phase::Idle, 5);
    assert!(!state.advance(idle, &DesktopLyricsConfig::default()).expect("该推").visible);
    let (failed, _keep2) = frame(&lines, Phase::Failed("全挂了".to_string()), 5);
    assert!(!state.advance(failed, &DesktopLyricsConfig::default()).expect("该推").visible);
}

/// 暂停默认留着（想隐藏就配 `hide_when_paused`）。
#[test]
fn pausing_keeps_the_line_unless_configured() {
    let lines = lyric(&[("第一句", None)]);
    let (paused, _keep) = frame(&lines, Phase::Paused, 5);

    let mut state = State::default();
    assert!(state.advance(paused, &DesktopLyricsConfig::default()).expect("该推").visible);

    let mut state = State::default();
    let config = DesktopLyricsConfig { hide_when_paused: true, ..DesktopLyricsConfig::default() };
    assert!(!state.advance(paused, &config).expect("该推").visible);
}

/// 解锁且没歌词时强制可见并给提示语——否则解锁了却找不到窗口，没法拖。
#[test]
fn unlocking_forces_the_overlay_visible() {
    let mut state = State::default();
    state.toggle_lock();

    let bare = Snapshot { ..snapshot(Phase::Idle, 0) };
    let payload = state
        .advance(Frame { lyric: None, snapshot: &bare }, &DesktopLyricsConfig::default())
        .expect("锁定态变了就该推");
    assert!(payload.visible);
    assert!(!payload.locked);
    assert_eq!(payload.current, UNLOCKED_HINT);
}

/// 译文跟着开关走。
#[test]
fn the_translation_follows_the_switch() {
    let lines = lyric(&[("第一句", Some("first"))]);
    let (with, _keep) = frame(&lines, Phase::Playing, 5);

    let mut state = State::default();
    let payload = state.advance(with, &DesktopLyricsConfig::default()).expect("该推");
    assert_eq!(payload.translation.as_deref(), Some("first"));

    let mut state = State::default();
    let config = DesktopLyricsConfig { show_translation: false, ..DesktopLyricsConfig::default() };
    let payload = state.advance(with, &config).expect("该推（配置不同→上次推的不一样）");
    assert_eq!(payload.translation, None);
}

/// 空句柄（关掉功能 / 非 Windows / 测试）：sync 与 toggle_lock 都是空操作。
#[test]
fn a_none_handle_syncs_nothing() {
    let lines = lyric(&[("第一句", None)]);
    let (frame, _keep) = frame(&lines, Phase::Playing, 5);

    let mut overlay = DesktopLyrics::none();
    overlay.sync(frame, &DesktopLyricsConfig::default());
    overlay.toggle_lock();
    assert!(!overlay.locked(), "空句柄不记得锁定态");
}

/// 换过显示器/分辨率后，屏外的中心点要夹回主屏内（留边距）。
#[test]
fn a_saved_center_is_clamped_onto_the_primary_screen() {
    assert_eq!(clamp_center((-5000, -5000), (1920, 1080)), (MARGIN, MARGIN));
    assert_eq!(clamp_center((99999, 99999), (1920, 1080)), (1920 - MARGIN, 1080 - MARGIN));
    assert_eq!(clamp_center((800, 500), (1920, 1080)), (800, 500), "本来就在屏内就不动");
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
```

（`frame()` 助手返回 `(Frame, Snapshot)` 是为了让 `Snapshot` 活到 `advance` 用完——所以 `Frame` 要能 Copy，
见 Step 3 的派生要求；测试里用 `_keep` 绑住那个 `Snapshot`。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test overlay::`
Expected: 编译失败（`overlay` 模块不存在 / 符号未定义）。

- [ ] **Step 3: 实现**

- 状态机：`struct State { last: Option<Payload>, locked: bool }`（`#[derive(Default)]`）+
  `fn advance(&mut self, frame: Frame<'_>, config: &DesktopLyricsConfig) -> Option<Payload>`，
  里面按 spec §1 那张表算「理想 payload」，跟 `self.last` 比，不同才 `self.last = Some(…)` 并返回。
  `DesktopLyrics::sync` 就是 `if let Some(payload) = self.state.advance(frame, config) { self.imp.push(payload) }`；
  `DesktopLyrics::locked` 转发 `self.state.locked`；`toggle_lock` 只翻它（下一帧 `advance` 自然发现锁定态变了 → 推一次）。
- 位置：`save_center(path, center)` / `load_center(path)` 走 `serde_json`（`{"cx":…,"cy":…}`）+ 临时文件 rename
  （抄 `history.rs` 的 `save`）；读不到/解析失败 → `None`。
- `clamp_center(center, screen)`：留 `MARGIN` 边距夹进 `(0,0)-(w,h)`。
- `primary_screen()`：Windows 上 `GetSystemMetrics(SM_CXSCREEN/SM_CYSCREEN)`；非 Windows 返回 `(1920, 1080)`
  （空实现那份用不到，取个确定的数让测试与编译都稳）。
- `impl DesktopLyrics { pub fn none() … }` + `imp` 两份 `cfg`：
  - `#[cfg(target_os = "windows")] mod imp`：**本步骤是空壳**（`Imp` 零尺寸、`push` 空、`install` 返回
    `Installed { overlay: …none-ish, positions: None, error: None }`，注明 `// 真实现见 Task 3`）；
  - `#[cfg(not(target_os = "windows"))] mod imp`：同样的空壳（**这份之后不再改**）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test overlay::` 与 `cargo test --locked --quiet`（全套回归）
Expected: 新增 12 条全 PASS、原有 219 条不受影响。

- [ ] **Step 5: 提交**

```bash
git add src/overlay.rs src/main.rs src/paths.rs
git commit -m "feat(overlay): 桌面歌词的平台无关一半(状态机 + 位置状态)"
```

---

# Task 3: Windows 实现——先探针，后实现

**这一步的头两道是门**：spec §2 的渲染路线（`UpdateLayeredWindow` + 32 位 DIB + 灰度 AA + 描边）
只有亲眼看过才敢往上盖代码。探针不过就走 spec §8 的退路（半透明背板），**停下来先改设计**（记 ruling），
不要硬做。

**Files:**
- Modify: `src/overlay.rs`（`#[cfg(target_os = "windows")] mod imp`）
- Modify: `Cargo.toml`（`windows-sys` 补 `Win32_Graphics_Gdi`、`Win32_UI_HiDpi`）
- Create（临时，用完即删）: `examples/desktop_lyrics_probe.rs`

**Interfaces:**
- Consumes: Task 2 的 `Payload`/`Installed`/`clamp_center`/`primary_screen`
- Produces: `imp::{Imp, Imp::none, Imp::push(&mut self, Payload), imp::install(&DesktopLyricsConfig, Option<(i32,i32)>) -> (Imp, Option<Receiver<(i32,i32)>>, Option<String>)}`

- [ ] **Step 1: 写探针（~30–60 行，可丢弃）**

`examples/desktop_lyrics_probe.rs` 干四件事：

1. 建窗口：`RegisterClassW`（自己的 `wndproc`）→ `CreateWindowExW(WS_EX_LAYERED | WS_EX_TRANSPARENT |
   WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, WS_POPUP, …)` → `SetWindowPos(HWND_TOPMOST, SWP_NOACTIVATE)`。
2. 画一帧：`GetDpiForWindow` → `CreateFontW("Microsoft YaHei UI", 高, ANTIALIASED_QUALITY)` →
   `GetTextExtentPoint32W` 量一行中文 + 一行英文 → `CreateDIBSection`（32 位、自上而下、`BI_RGB`，
   `BITMAPINFOHEADER` 的 `biHeight` 取负）→ 选进内存 DC → 清零 → 8 个偏移画描边 → 画正文 →
   `UpdateLayeredWindow(hwnd, …, BLENDFUNCTION { BlendOp: AC_SRC_OVER, SourceConstantAlpha: 230, AlphaFormat: AC_SRC_ALPHA }, …)`。
3. **存一张 PNG**：把 DIB 的 BGRA 反预乘成 RGBA（`image::RgbaImage::from_raw(w, h, buf)`）→
   `save(".superpowers/…/probe.png")` 到 workspace（git-ignored 的临时目录即可）。
4. **打印机械判据**：`GetWindowLongW(GWL_EXSTYLE)` 的位（要有 LAYERED/TRANSPARENT/TOPMOST/TOOLWINDOW/NOACTIVATE）、
   `WindowFromPoint(窗口中心)` 返回的 hwnd（**必须不是自己**——这就是「鼠标穿透」的机械证明）、
   以及 z 序：`GetTopWindow` 起沿 `GW_HWNDNEXT` 找到自己，看紧邻它下面那个窗口是不是非 topmost（是 → 我们在最上层带）。
   最后 `GetMessageW` 循环跑 3 秒（让窗口真的显示出来），`DestroyWindow` 退出。

- [ ] **Step 2: 跑探针并看结果**

Run: `cargo run --example desktop_lyrics_probe`（会打印判据；3 秒后自己退出）
Expected: 五项 ex-style 位都在；`WindowFromPoint` 不是自己；z 序判据成立。

- [ ] **Step 3: 看 PNG 再决定往下走**

用 `read` 打开那张 PNG（`image` 解码成图）。
**判据**：字边缘是灰阶过渡（没有彩色像素）、描边均匀、笔画不糊。
**不通过** → 按 spec §8 的退路改设计（画半透明背板再画字），**停下来记 ruling 并改 spec**，
不要带着难看的渲染往下盖代码。

- [ ] **Step 4: 实现真的 `mod imp`**

与探针同一套调用，但装进线程里：

```rust
pub(super) struct Imp { thread_id: u32, join: Option<JoinHandle<()>>, payload: Arc<Mutex<Payload>> }
impl Imp { pub(super) fn push(&mut self, payload: Payload) { *self.payload.lock()…= payload; PostMessageW(self.hwnd_ish… 或 thread_id, WM_APP, 0, 0) } }
```

- 线程体：`RegisterClassW`/`CreateWindowExW` → 回报 `thread_id` + `hwnd`（通道，`install` 阻塞到收到）→
  `GetMessageW` 循环：`WM_APP` → 取 `Mutex<Payload>` 的克隆 → 隐藏就 `ShowWindow(SW_HIDE)`，
  否则量尺寸 + `SetWindowPos`（按中心点摆）+ 画 DIB + `UpdateLayeredWindow`；
  `WM_NCHITTEST` → 解锁时回 `HTCAPTION`，锁定回 `HTTRANSPARENT`；
  `WM_EXITSIZEMOVE` → `GetWindowRect` 取中心 → `Sender<(i32,i32)>` 发给 TUI 线程；
  `WM_QUIT`/`WM_DESTROY` → `PostQuitMessage`，出循环后 `DestroyWindow`。
- 锁定/解锁由 payload 里的 `locked` 驱动：解锁时 `SetWindowLongW(GWLEXSTYLE)` 去掉 `WS_EX_TRANSPARENT`
  再 `SetWindowPos(SWP_FRAMECHANGED)`；锁定时加回来。
- `Drop for Imp`：`PostThreadMessageW(thread_id, WM_QUIT, 0, 0)` + `join`（与热键线程同一套）。
- 失败路径：建窗口/注册/建线程任一失败 → `(Imp::none(), None, Some("歌词窗口起不来".into()))` + `tracing::warn!` 记原因。

- [ ] **Step 5: 删探针、跑门禁**

```bash
rm examples/desktop_lyrics_probe.rs
cargo test --locked --quiet
cargo clippy --all-targets --locked   # 仍可能有「消费方还没接线」的 dead_code（Task 4 收尾）
```

- [ ] **Step 6: 真机看一眼（不完整，整轮在 Task 6）**

Run: 临时把 `[desktop_lyrics] enabled = true` 写进配置 → `cargo run`（或 tuios 里跑 debug 二进制）。
Expected: 主屏底部出现两行描边中文歌词，浮在 tuios 之上；在歌词上点击/滚轮，事件落到下面窗口。

- [ ] **Step 7: 提交**

```bash
git add src/overlay.rs Cargo.toml Cargo.lock
git commit -m "feat(overlay): Windows 覆盖层(线程 + 分层窗口 + DIB 绘制)"
```

---

# Task 4: 接线——`App.overlay`、第 6 条热键、位置落盘

**Files:**
- Modify: `src/tui/state.rs`、`src/tui/mod.rs`、`src/tui/keys.rs`、`src/hotkeys.rs`、`src/config.rs`

**Interfaces:**
- Consumes: Task 2/3 的 `DesktopLyrics::{none, install, sync, toggle_lock, locked}`、`Frame`、`load_center`、`save_center`
- Produces: `App.overlay: overlay::DesktopLyrics`、`keys::Action::LyricsLock`、`hotkeys::DEFAULTS` 第 6 条

- [ ] **Step 1: 写失败的测试**

`src/tui/state.rs` 的 `mod tests`：

```rust
/// `lock` 是第 6 条热键，落到 overlay 的锁定态上（这条同时钉住「动作接对了」）。
#[test]
fn lyrics_lock_flips_the_overlay_lock() {
    let (mut app, _runtime) = crate::tui::test_support::test_app();
    let before = app.overlay.locked();
    app.apply(keys::Action::LyricsLock);
    assert_ne!(app.overlay.locked(), before, "锁定态该翻一下");
    app.screen = Screen::Login(LoginState::default());
    app.dispatch_hotkey(keys::Action::LyricsLock);
    assert_eq!(app.overlay.locked(), !before, "登录屏也要按同一条规则被挡住");
}
```

`src/hotkeys.rs` 的 `mod tests`：把 `the_defaults_are_the_five_combos_the_spec_fixed` 改成
`the_defaults_are_the_six_combos_the_spec_fixed`，三个数组各加一项
（`"lock"` / `"ctrl+shift+l"` / `Action::LyricsLock`）。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test lyrics_lock`
Expected: 编译失败（`Action::LyricsLock` / `App.overlay` / `overlay.locked()` 未定义）。

- [ ] **Step 3: 实现（含所有「5 条变 6 条」的连带改动）**

`src/tui/keys.rs`：`Action` 加 `LyricsLock`（注释写「全局热键 `lock`：切桌面歌词的锁定/解锁」）。
Hmm——注意 `Action` 带 `Menu(usize)`，新变体加在末尾，**不要**插到中间（`Debug`/测试里对顺序没有依赖，但保持尾部追加的习惯）。

`src/tui/state.rs`：
- `use crate::overlay;`、`App` 加 `pub(super) overlay: overlay::DesktopLyrics` + `pub(super) lyrics_positions: Option<Receiver<(i32, i32)>>`
  （`App::new` 里 `overlay: overlay::DesktopLyrics::none()`、`lyrics_positions: None`，注释说明真安装只在 `tui::run`）。
- `apply` 加分支：`keys::Action::LyricsLock => self.overlay.toggle_lock(),`
- `tick()` 里（放在热键 drain 之后）：
  ```rust
  // 桌面歌词：只在变化时推；拖出来的位置顺手落盘。
  let snapshot = self.engine.snapshot();
  self.overlay.sync(overlay::Frame { lyric: self.lyric.as_ref(), snapshot: &snapshot }, &self.config.desktop_lyrics);
  if let Some(positions) = &self.lyrics_positions {
      let path = crate::paths::desktop_lyrics_path();
      while let Ok(center) = positions.try_recv() {
          if let Err(error) = overlay::save_center(&path, center) {
              tracing::warn!("写歌词窗口位置失败：{error:#}");
          }
      }
  }
  ```

`src/tui/mod.rs`：`tui::run` 里（`hotkeys::install` 之后）
```rust
let installed = overlay::install(&app.config.desktop_lyrics, overlay::load_center());
app.overlay = installed.overlay;
app.lyrics_positions = installed.positions;
if let Some(error) = installed.error {
    tracing::warn!(%error, "桌面歌词起不来");
    app.notify("歌词窗口起不来");
}
```
（`app.config` 在 `App::new` 之后就能读；`enabled = false` 时 `install` 内部直接返回 `none()`。）

`src/hotkeys.rs`：`DEFAULTS` 类型从 `[(&str, &str, Action); 5]` 变 `[(&str, &str, Action); 6]`，加
`("lock", "ctrl+shift+l", Action::LyricsLock)`。

`src/config.rs`：`HotkeyConfig` 加 `pub lock: String`、`get`/`set` 各加一个 `"lock"` 分支、
`write_default` 的热键段自动跟着 `DEFAULTS` 走（不用改），并把这几处断言改成 6/5：

| 位置 | 现在 | 改成 |
|---|---|---|
| `config.rs` `the_missing_hotkey_section_gives_the_default_combos` | `bindings().len(), 5` | `6` |
| `config.rs` `a_partial_section_keeps_the_other_defaults` | `bindings().len(), 5` | `6` |
| `config.rs` `an_empty_string_disables_one_action` | `bindings.len(), 4` | `5` |
| `config.rs` `a_bare_key_is_refused_and_cleared` | `bindings().len(), 4, "其余四条照旧"` | `5` + 「其余五条照旧」 |
| `hotkeys.rs` 的默认组合测试 | 名字里带 `five`、三个数组 5 项 | `six`、各 6 项 |

- [ ] **Step 4: 跑测试与 clippy 确认通过**

Run: `cargo test --locked --quiet` → 全套 PASS（含新两条）。
Run: `cargo clippy --all-targets --locked -- -D warnings` → **exit 0**（接线完成后，Task 1–3 那批临时
dead_code 告警应当全部消失；这一条是「本特性没留下死代码」的判据）。

- [ ] **Step 5: 提交**

```bash
git add src/tui/state.rs src/tui/mod.rs src/tui/keys.rs src/hotkeys.rs src/config.rs
git commit -m "feat(overlay): 接进 App 与热键(第 6 条 lock)，位置变化落盘"
```

---

# Task 5: 文档

**Files:**
- Modify: `README.md`、`docs/design.md`、`docs/superpowers/specs/2026-10-12-desktop-lyrics-design.md`

- [ ] **Step 1: README**

- 「功能」表加一行：桌面歌词（Windows）：当前句 + 翻译 + 下一句浮在桌面之上，默认穿透、热键解锁可拖。
- 「配置」段加 `[desktop_lyrics]` 九个键的示例（与模板一致）。
- 「键位」段那两条语义之外，加一句：全局热键表多了第 6 条 `lock = "ctrl+shift+l"`（切歌词窗口的锁定/解锁）。
- 「已知限制」加三条：只支持 Windows；只认主屏、位置在屏外会被夹回；全屏独占应用之上压不过。
- 「文件」清单加 `desktop-lyrics.json`（位置状态，可随时删，删了回默认位置）。

- [ ] **Step 2: `docs/design.md`**

加一节「桌面歌词（Windows）」，写清四件事：为什么分成平台无关的状态机与平台相关的窗口线程；
为什么用 `UpdateLayeredWindow` + DIB 而不是取色透明（灰度 AA 的字边缘会混 key 色）；
位置为什么存中心点、为什么写独立状态文件；锁定态为什么不持久化。
「代码结构」清单加 `overlay.rs`；「跨平台与发布」那张表加一行（Windows 才有；其它平台空实现）。

- [ ] **Step 3: spec 里那处过期描述**

`2026-10-12-global-hotkeys-design.md` 里凡写「五个动作 / 5 条」的地方，补一句「另有一条
`lock = "ctrl+shift+l"` 由桌面歌词引入（见 `2026-10-12-desktop-lyrics-design.md` §4）」。

- [ ] **Step 4: 验证 + 提交**

Run: `cargo test --locked --quiet`、`cargo clippy --all-targets --locked -- -D warnings`
Expected: 干净（文档不改代码，这一步是确认没有打字改错地方）。

```bash
git add README.md docs/design.md docs/superpowers/specs/2026-10-12-global-hotkeys-design.md
git commit -m "docs: 桌面歌词(README 功能/配置/限制 + design 实现说明)"
```

---

# Task 6: 整轮真机验收与收尾

**Files:** 无（只跑、只看、必要时记 ruling）

- [ ] **Step 1: 按 spec §8 的真机清单整轮走一遍**

在 tuios 的 `canto-dev` 会话里跑新编的 `target/debug/canto.exe --log debug`，窗口命名 `canto-lyrics`：

1. `enabled = true` 启动 → 窗口在主屏底部居中；`canto.log` 有安装记录；状态行无通知。
2. 播放一首有译文的歌 → 三行按 当前句/译文/下一句 显示，换行及时（≤ 200ms）；`show_translation = false`
   重启后中间那行消失。
3. 把 tuios 切到前台/全屏，歌词仍在上面（`tuios capture-pane` 只能证明终端内容，这步靠眼睛 + 步骤 4 的机械判据）。
4. 在歌词位置点击 → 事件落到 tuios（终端响应），证明穿透。
5. `ctrl+shift+l` → 出现提示框、能拖；拖到屏幕左上角附近松手 → `~/.config/canto/desktop-lyrics.json` 出现且值合理；
   **重启后回到拖到的位置**。
6. 再按 `ctrl+shift+l` → 提示框消失、鼠标又穿透。
7. 暂停 → 默认留着；`hide_when_paused = true` → 隐藏。
8. `enabled = false` 重启 → 不建窗口（`canto.log` 无相关行）、终端功能照常。
9. 把状态文件改成屏幕外坐标 → 重启后夹回主屏内可见。
10. 退出 canto → 窗口消失、任务管理器无残留进程。

- [ ] **Step 2: 收尾验证**

Run: `cargo test --locked --quiet`、`cargo clippy --all-targets --locked -- -D warnings`
Expected: 219 + 新增（约 19 条）全 PASS；clippy exit 0。

Run: `cargo fmt --all --check`
Expected: **不要求干净**（仓库没有 `rustfmt.toml`、CI 也不跑 fmt、既有文件同样「不合」）——只确认
新增文件与所在文件的风格一致；**不要跑 `cargo fmt`**。

- [ ] **Step 3: 清掉临时物**

`rm -rf` 探针留下来的 PNG/日志（都在 git-ignored 的临时目录里）；`git status` 应只剩本次该提交的东西。

- [ ] **Step 4: 提交（如果前面几步动了代码或文档）**

```bash
git add -A
git commit -m "chore(overlay): 整轮真机验收后的小修"    # 没有要改的就跳过这一步
```

---

## 自查记录（写计划时跑过）

- **spec 覆盖**：取词与状态机（Task 2 全表）、窗口与渲染（Task 3 步骤 1–4，含 DIB/AA/描边/透明度/DPI/尺寸随文本）、
  锁定与拖动（Task 3 的 `WM_NCHITTEST`/`WM_EXITSIZEMOVE` + Task 4 的落盘 + Task 6 第 5 步）、
  配置契约九键（Task 1）、模块与接口（Task 2/3/4 的 Interfaces 块与 spec §5 逐条对上）、
  线程模型与生命周期（Task 3 的 Drop/线程 + Task 4 的安装顺序）、退化与错误（Task 3 失败路径 + Task 4 通知）、
  测试计划（Task 2 的 12 条 + Task 1/4 新增 + Task 6 真机十步）、改动清单（文件结构 + 各任务 Files）、
  风险与已知限制（Task 5 写进 README/design，含「先探针」这条门）。
- **步骤扫描**：每步一件事、有可检查的结果；无 TBD；唯一写死整段代码的是 Task 3 的探针与 `mod imp`
  （Win32 调用顺序、DIB 布局、`WM_NCHITTEST` 的返回值这些**签名与测试定不下来**的部分），其余都是签名 + 断言。
- **类型一致**：`Frame`/`Payload`/`Installed`/`DesktopLyrics::{none,install,sync,toggle_lock,locked}`/
  `Rgb`/`DesktopLyricsConfig`/`Action::LyricsLock` 在 Task 1–4 之间名字与签名一致；
  `DEFAULTS` 从 5 变 6 后，三处断言的清单在 Task 4 Step 3 里逐条列了（`config.rs` 359/369/378/388 + `hotkeys.rs` 的默认组合测试）。
- **Review Focus 逐条挂测试**：1→Task 2 两条；2→Task 2 两条；3→Task 2 `a_none_handle_syncs_nothing` + Task 6 第 8 步；
  4→Task 2 往返 + Task 6 第 5 步；5→Task 3 失败路径 + Task 4 的通知接线。
- **篇幅**：计划约 480 行，对应新增代码约 450 行 + 配置/接线/文档约 200 行 —— 同一量级，没把代码抄进计划。
- **一处已知的「计划里做不到」**（提前写在 Task 6）：`cargo fmt --all --check` 不干净是仓库既有状态，
  别为了它去 `cargo fmt`（会造出与本次无关的整仓 diff）。

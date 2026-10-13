# 内容区的两个栏（歌词栏 / 频谱区）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把「全屏正在播放」这个**模式**拆成内容区里的两个独立开关——`y` 管歌词栏、`v` 管频谱区，顺带删掉 `Context::NowPlaying` 与 `Esc` 那一层。

**Architecture:** 两个开关量（`show_lyric` 已有、`show_spectrum` 新增）+ 一条合成规则：内容区 = `[频谱区][列表]` 之上再叠一条**歌词栏**，其中**频谱区与列表互斥**、**歌词栏永远在最下面**。`draw::layout` 的签名与 `Areas` 都不动（`content` 仍是「内容区 − 歌词栏」，频谱区画进它里面），所以这次改的只是**判据与分支**，不是骨架。没有模式，就没有第二个按键上下文。

**Tech Stack:** Rust 2024（edition 2024）· ratatui 0.30 · crossterm · unicode-width；测试与代码同文件（`#[cfg(test)] mod tests`），跑 `cargo test` 与 `cargo clippy --all-targets --locked -- -D warnings`。

**Spec:** `docs/superpowers/specs/2026-10-13-content-area-panels-design.md`（本次实现的权威；冲突以它为准）

## Global Constraints

- **键位约定**：默认小写；大写只留给「升级型能力」或小写已被占用的情形。
- **歌词栏占位条件**：`show_lyric && engine.snapshot().phase != Idle && 窗口高 >= MIN_LYRIC_HEIGHT(20)`。
- **歌词栏高度**：`(内容区 / 3).clamp(5, MAX_LYRIC_HEIGHT(12))`——**只有这一套**（今天全屏那套 `内容区/2` 夹 7..12 废弃）。
- `show_lyric` 初值 `true`、`show_spectrum` 初值 `false`；两者**都不落盘**。
- **两个开关正交**；换屏（`switch_menu` / `go_home`）收掉频谱区。
- **提速档**：`spectrum_wanted()` = `show_spectrum && phase == Playing`；`spectrum_animating()` 在它之上再加「屏是 `Screen::Main | Screen::Home`」。
- 内容区 = 窗口高 − 播放栏 4 行 − 状态栏 1 行；`Areas` 的字段与 `layout(area, lyric_wanted)` 签名**不变**。
- 模态浮层（帮助 / 队列 / 选择框）与输入框里**不绑** `y` / `v`（`y`、`v` 在输入态是打字）。
- 提交前必过：`cargo test` 全绿 + `cargo clippy --all-targets --locked -- -D warnings` 干净。

## Review Focus

spec 没有明说、但使用者会撞上、且没有现成测试钉住的五条——每条都指到负责它的任务：

1. **窗口 15–19 行时**：歌词栏不占位（`MIN_LYRIC_HEIGHT` = 20），所以按 `y` **画面不动**——那不是键坏了；而 `v` 会拿到整个内容区。→ Task 2 的尺寸测试。
2. **没有歌词但在播**：`lyric = None`（写「暂无歌词」）与 `lyric_loading`（写「歌词加载中…」）两种都得画得出来。→ Task 2。
3. **频谱区开着时浮层**：`u` 的队列浮层必须还看得见（它画在 `Areas.content` 上，`content` 的含义没变）。→ Task 2。
4. **换屏收频谱、退层不收**：`1`..`4`、`Enter` 进屏、`0` 回首页都收；`Esc` 退歌单层级**不**收（那不是换屏）。→ Task 1。
5. **频谱区开着时列表键仍可用**：`d` / `p` / `x` / `Enter` 作用在看不见的列表上——刻意如此，没有模式就不该有「按键随视图变脸」。→ Task 1。

## 文件结构

| 文件 | 负责什么 | 本计划动它 |
|---|---|---|
| `src/tui/keys.rs` | 键 → `Action` 的映射、`Context` 分类 | Task 1 |
| `src/tui/state.rs` | `App` 状态、`Action` 的执行、`context()` | Task 1 |
| `src/tui/draw/mod.rs` | 骨架（`layout` / `Areas`）、内容区那几条分支、`hints` | Task 1（`hints` 一支）+ Task 2（分支与判据） |
| `src/tui/draw/overlay.rs` | 帮助浮层的键位表（`help_rows`） | Task 3 |
| `README.md` · `docs/design.md` | 使用者文档 · 设计约定 | Task 3 |

---

### Task 1: 把「全屏模式」换成 `v` 开关（键位 + 状态机 + 提示表）

**Files:**
- Modify: `src/tui/keys.rs`（`Action` 枚举、`Context` 枚举、`resolve`、`common`，以及 `keys.rs` 测试模块）
- Modify: `src/tui/state.rs`（字段与初值、`Action` 分支、`switch_menu` / `go_home`、`context()`、`Action::Back`、`spectrum_wanted()`，以及 `state.rs` 测试模块）
- Modify: `src/tui/draw/mod.rs`（`draw()` 里 109 / 111 两行的判据与函数名、`hints()` 里 `Context::NowPlaying` 那一支）

**Interfaces:**
- Produces（Task 2 / Task 3 会用到）：
  - `keys::Action::ToggleSpectrum`（无参数）
  - `App.show_spectrum: bool`（初值 `false`）
  - `App::spectrum_wanted(&self) -> bool`（判据见 Global Constraints）
  - `App::player_active(&self) -> bool`（已有，不改）
  - `App::context()` 不再返回 `keys::Context::NowPlaying`
- 删掉：`keys::Action::ToggleNowPlaying`、`keys::Context::NowPlaying`、`App.now_playing_full`
- Consumes：`App.show_lyric`（已有）、`App::draw_spectrum_area(&self, frame, area, title: &str)`（已有）

- [ ] **Step 1: 测试先红（键位 + 状态一起改，指向新名字）**

`keys.rs` 里：
- `the_main_screen_has_one_key_table`（约 489 行）与 `the_home_screen_has_its_own_keys`（约 427 行）的表里：`(KeyCode::Char('Y'), Action::ToggleNowPlaying)` → `(KeyCode::Char('v'), Action::ToggleSpectrum)`；首页那张表**再加** `(KeyCode::Char('y'), Action::ToggleLyric)`。
- `uppercase_y_opens_the_fullscreen_view_and_hides_list_keys`（约 272 行）**整条换成**下面这条（它顺带删掉了 `Context::NowPlaying` 的键位表与「全屏里该没绑定」那个循环）：

```rust
/// 小写 `v` 开关频谱区；大写 `Y` 不再绑；列表键不许因为开了频谱就被禁。
#[test]
fn lowercase_v_toggles_the_spectrum_and_leaves_the_list_keys_alone() {
    for context in [Context::Content, Context::Home] {
        assert_eq!(press(context, KeyCode::Char('v')), Some(Action::ToggleSpectrum));
        assert_eq!(press(context, KeyCode::Char('y')), Some(Action::ToggleLyric));
        assert_eq!(press(context, KeyCode::Char('Y')), None, "大写 Y 不再绑");
        for code in [KeyCode::Char('d'), KeyCode::Char('p'), KeyCode::Char('x'), KeyCode::Enter] {
            assert!(press(context, code).is_some(), "{context:?} 里列表键不许动：{code:?}");
        }
    }
    assert_eq!(press(Context::PlaylistPicker, KeyCode::Char('v')), None, "选择框里 v 不抢");
    assert_eq!(press(Context::SearchInput, KeyCode::Char('v')), Some(Action::InputChar('v')), "搜索框里 v 是打字");
}
```

- `zero_goes_home_from_the_main_screen`（约 476 行）与 `logout_is_available_from_the_main_screen`（约 411 行）里各有一句 `press(Context::NowPlaying, …)`：**删掉那一句**（那个上下文要没了）。

`state.rs` 里（同时改，别分两轮——`Context` 这个枚举把键位与状态耦合在一起，只改一边整套编不过）：
- `only_a_playing_fullscreen_view_raises_the_frame_rate`（约 2139 行）→ 改名 `only_a_playing_spectrum_raises_the_frame_rate`，`now_playing_full = true` → `show_spectrum = true`。
- `a_fullscreen_view_over_the_home_raises_the_frame_rate_too`（约 2151 行）→ 改名 `the_spectrum_over_the_home_raises_the_frame_rate_too`，同上换字段。
- `fullscreen_lyric_toggles_and_is_left_by_esc_or_switching`（约 2198 行）→ 拆成两条：

```rust
/// 两个开关正交：`v` 只动频谱区，`y` 只动歌词栏。
#[test]
fn the_spectrum_and_the_lyric_bar_are_independent() {
    let (mut app, _runtime) = crate::tui::test_support::test_app();
    app.apply(keys::Action::ToggleSpectrum);
    assert!(app.show_spectrum);
    assert!(app.show_lyric, "v 不该动歌词栏");
    app.apply(keys::Action::ToggleLyric);
    assert!(!app.show_lyric);
    assert!(app.show_spectrum, "y 不该动频谱区");
}

/// 换屏（数字键 / Enter 进屏 / 0 回首页）收掉频谱区；退层不算换屏，不收。
#[test]
fn switching_screens_closes_the_spectrum_but_popping_a_level_does_not() {
    let (mut app, _runtime) = crate::tui::test_support::test_app();
    app.nav.switch_menu(Menu::Playlists);
    app.nav.push_tracks("1", "嗯", vec![crate::tui::test_support::test_song("a")]);
    app.apply(keys::Action::ToggleSpectrum);
    app.apply(keys::Action::Back); // 退层
    assert!(app.show_spectrum, "退层不是换屏");
    app.apply(keys::Action::Menu(3));
    assert!(!app.show_spectrum, "切屏要收");
    app.apply(keys::Action::ToggleSpectrum);
    app.apply(keys::Action::Home);
    assert!(!app.show_spectrum, "回首页也要收");
}
```

- `esc_leaves_the_fullscreen_before_popping_a_level`（约 2223 行）→ 改名 `esc_pops_a_level_and_does_not_touch_the_spectrum`：删掉「第一次 `Esc` 退全屏」那一段，只留「`Esc` 退层」，并加 `assert!(app.show_spectrum, "Esc 不退频谱")`。
- `the_home_fullscreen_view_uses_the_now_playing_context`（约 2471 行）→ 整条换成：

```rust
/// 首页上开频谱区，上下文**不变**（还是首页那套键）。
#[test]
fn the_spectrum_keeps_the_normal_context() {
    let (mut app, _runtime) = crate::tui::test_support::test_app();
    app.go_home();
    app.apply(keys::Action::ToggleSpectrum);
    assert_eq!(app.context(), keys::Context::Home, "没有第二个上下文了");
}
```

- `home_key_in_the_fullscreen_view_collapses_it`（约 2484 行）→ 改名 `going_home_closes_the_spectrum`，`now_playing_full` → `show_spectrum`、`ToggleNowPlaying` → `ToggleSpectrum`（断言结构不变：首页开、`0` 收）。

- [ ] **Step 2: 跑，确认红**

Run: `cargo test`
Expected: 编不过，报 `Action::ToggleSpectrum` 不存在 / `App` 没有 `show_spectrum`——这就算红（跨文件改名没法只让断言红）。

- [ ] **Step 3: 实现（keys.rs + state.rs + draw/mod.rs 一次改完）**

这三个文件被 `Context` 这个枚举耦合，拆开没有任何一步编得过，所以合成一步：
- `keys.rs`：`Action` 枚举 `ToggleLyric` 旁边加 `ToggleSpectrum`、删 `ToggleNowPlaying`；`Context` 枚举删 `NowPlaying`；`resolve` 删 `Context::NowPlaying => …` 整支（约 160-207 行）；`common` 与 `Context::Home` 各加 `KeyCode::Char('v') => Some(Action::ToggleSpectrum)`，`Context::Home` 再加 `KeyCode::Char('y') => Some(Action::ToggleLyric)`，三处 `KeyCode::Char('Y') => …` 全删。
- `state.rs`：`now_playing_full: bool` → `show_spectrum: bool`（注释：「频谱区开着（`v`）；与列表互斥」），初值 `false`；`Action::ToggleNowPlaying => …` → `Action::ToggleSpectrum => self.show_spectrum = !self.show_spectrum`；`switch_menu` 与 `go_home` 里 `self.now_playing_full = false;` → `self.show_spectrum = false;`；`context()` 删 `if self.now_playing_full { return keys::Context::NowPlaying }` 那一支；`Action::Back` 里「第一次 `Esc` 退全屏」那一段（约 1471-1473 行）删掉；`spectrum_wanted()`（约 1793 行）`self.now_playing_full && …` → `self.show_spectrum && …`（`spectrum_animating()` 的屏判断不动）。
- `draw/mod.rs`：109 / 111 两行 `self.now_playing_full` → `self.show_spectrum`（这一轮只改名，`draw_now_playing(…)` 的调用保留——绘制形态由 Task 2 改）；`hints()` 删 `keys::Context::NowPlaying => …` 那一支。

- [ ] **Step 4: 跑全量，确认绿**

Run: `cargo test`
Expected: 全绿（含 Step 1 改写的全部测试）。

- [ ] **Step 5: clippy + 提交**

Run: `cargo clippy --all-targets --locked -- -D warnings`
Expected: 干净。

```bash
git add src/tui/keys.rs src/tui/state.rs src/tui/draw/mod.rs
git commit -m "feat(panels): 全屏模式换成 v 开关（删 Context::NowPlaying 与 Esc 那一层）"
```

---

### Task 2: 内容区的合成（歌词栏 + 频谱区）

**Files:**
- Modify: `src/tui/draw/mod.rs`（`draw()` 的 `lyric_wanted` 与内容区分支、`draw_now_playing` 的拆分与删除、`SPLIT_MIN_HEIGHT` 常量，以及 `draw/mod.rs` 测试模块）
- 参考（不改）：`src/tui/draw/lyric.rs`（「暂无歌词」/「歌词加载中…」已经画得出来）

**Interfaces:**
- Consumes：`App.show_spectrum`、`App.show_lyric`、`App::player_active()`（Task 1 后的形态）、`Areas { content, lyric, player, status }`、`App::draw_spectrum_area(&self, frame, area, title: &str)`、`App::draw_lyric(&self, frame, area)`、`spectrum_title(song: Option<&str>) -> String`、`const SPLIT_TITLE: &str`、`spectrum_message(phase: &Phase) -> &'static str`（全部已有）。
- Produces：`App::draw()` 的新分支次序（`show_spectrum` → `Screen::Home` → 列表）与 `lyric_wanted = self.show_lyric && self.player_active()`。

- [ ] **Step 1: 绘制测试先红**

`draw/mod.rs` 里：
- `fullscreen_lyric_takes_over_the_content_area`（约 731 行）→ 改名 `the_spectrum_takes_over_the_content_area`：`show_spectrum = true` 且 `show_lyric = false` 时，整帧里有频谱（或它的提示文案），**没有**面包屑/列表标题。
- `the_now_playing_view_splits_into_spectrum_above_lyric`（约 778 行）→ 改名 `the_spectrum_sits_above_the_lyric_bar`：`show_spectrum` 与 `show_lyric` 都开时，歌词栏那几行在**最下面**、频谱块在它上面；歌词栏高度按新规则（`(内容区/3).clamp(5,12)`）算。
- 新增四条：

```rust
/// 没有歌词也画歌词栏：没拉完写「歌词加载中…」，拉完没有写「暂无歌词」。
#[test]
fn the_lyric_bar_shows_even_without_lyrics() {
    let (mut app, _runtime) = test_app();
    app.engine.force_phase(crate::player::Phase::Playing);
    app.lyric_loading = true;
    assert!(frame_text(&mut app, 120, 40).contains("歌词加载中…"));
    app.lyric_loading = false;
    assert!(frame_text(&mut app, 120, 40).contains("暂无歌词"));
}

/// 窗口太矮（< MIN_LYRIC_HEIGHT = 20）时歌词栏不占位：`y` 开着也一样。
#[test]
fn the_lyric_bar_needs_a_tall_enough_window() {
    let (mut app, _runtime) = test_app();
    app.engine.force_phase(crate::player::Phase::Playing);
    let text = frame_text(&mut app, 60, 15);
    assert!(!text.contains("暂无歌词"), "15 行窗口里歌词栏不占位：{text}");
}

/// 频谱区开着时队列浮层照样看得见（它画在 Areas.content 上）。
#[test]
fn the_queue_overlay_stays_visible_over_the_spectrum() {
    let (mut app, _runtime) = test_app();
    app.engine.force_phase(crate::player::Phase::Playing);
    app.show_spectrum = true;
    app.queue = crate::tui::queue::Queue::new(vec![test_song("a")], 0, String::new());
    app.apply(crate::tui::keys::Action::QueueOverlay);
    // 曲子标题只在浮层里出现（频谱开着时列表不画；测试里引擎没有曲名）。
    assert!(frame_text(&mut app, 120, 40).contains("歌a"), "浮层该压得住频谱区");
}

/// 频谱区开着时列表键的上下文不变（列表只是被替掉，没有被「模式」踢出去）。
#[test]
fn the_spectrum_leaves_the_list_keys_usable() {
    let (mut app, _runtime) = test_app();
    app.show_spectrum = true;
    assert_eq!(app.context(), crate::tui::keys::Context::Content);
}
```

- [ ] **Step 2: 跑，确认红**

Run: `cargo test -- the_lyric_bar_shows_even_without the_spectrum_sits_above_the_lyric_bar`
Expected: FAIL（歌词栏现在要求「有歌词」，所以「暂无歌词」画不出来）。

- [ ] **Step 3: `draw/mod.rs` 实现**

- `draw()`：`let lyric_wanted = self.show_lyric && self.lyric.is_some() && self.player_active() && !self.show_spectrum;` → `let lyric_wanted = self.show_lyric && self.player_active();`
- `draw()` 的分支：`if self.show_spectrum { … }` 里按标题规则画频谱，替代今天的 `if self.now_playing_full { self.draw_now_playing(frame, areas.content); }`：

```rust
        if self.show_spectrum {
            // 歌词栏在时两块都挂名字没意义；不在时名字得跟着频谱走。
            let title = if areas.lyric.is_some() {
                SPLIT_TITLE
            } else {
                spectrum_title(self.engine.snapshot().title.as_deref())
            };
            self.draw_spectrum_area(frame, areas.content, &title);
        } else if matches!(self.screen, Screen::Home) {
            self.draw_home(frame, areas.content);
        } else {
            self.draw_content(frame, areas.content);
        }
```

- 删掉 `draw_now_playing` 整个函数（它的三条规则里，只剩「没有数据 → `spectrum_message` 提示」那条还有用，而它已经在 `draw_spectrum_area` 里）与常量 `SPLIT_MIN_HEIGHT`；`draw_now_playing` 的文档注释里那套「歌词高 `clamp(内容区/2, 7, 12)`」的说法随之消失。

- [ ] **Step 4: 跑全量，确认绿**

Run: `cargo test`
Expected: 全绿。

- [ ] **Step 5: clippy + 提交**

Run: `cargo clippy --all-targets --locked -- -D warnings`

```bash
git add src/tui/draw/mod.rs
git commit -m "feat(panels): 内容区合成——歌词栏在下、频谱区与列表互斥，没歌词也画歌词栏"
```

---

### Task 3: 帮助表与文档

**Files:**
- Modify: `src/tui/draw/overlay.rs`（`help_rows` 的「全局」段 + `overlay.rs` 测试模块）
- Modify: `README.md`（第 5、17、18、110 行）
- Modify: `docs/design.md`（第 231、250、253、272 行附近 + 新增键位约定一条）

**Interfaces:**
- Consumes：`keys::Action::ToggleLyric` / `ToggleSpectrum` 的名字（Task 1）；`help_rows(screen: Option<Menu>, hotkeys: &HotkeyConfig) -> Vec<Row>` 的现有形状（不改签名）。
- Produces：帮助表「全局」段两行：`y` → `歌词栏开 / 关`、`v` → `频谱区开 / 关`（`Y` → `全屏：歌词 + 频谱` 那行删除）。

- [ ] **Step 1: 测试先红**

`overlay.rs` 的 `the_table_pairs_every_key_with_a_description`（约 344 行）里加：

```rust
        assert!(cells.contains(&("y", "歌词栏开 / 关")), "{cells:?}");
        assert!(cells.contains(&("v", "频谱区开 / 关")), "{cells:?}");
        assert!(
            !cells.iter().any(|(_, help)| help.contains("全屏")),
            "「全屏」这个模式没了：{cells:?}"
        );
```

- [ ] **Step 2: 跑，确认红**

Run: `cargo test -- the_table_pairs_every_key_with_a_description`
Expected: FAIL（表里还是 `Y 全屏：歌词 + 频谱`）。

- [ ] **Step 3: `overlay.rs` 实现**

`help_rows` 的「全局」段里 `Row::Keys { keys: "Y", help: "全屏：歌词 + 频谱" }` → `Row::Keys { keys: "y", help: "歌词栏开 / 关" }` 与 `Row::Keys { keys: "v", help: "频谱区开 / 关" }`。`help_rows` 的文档注释里 `Y`/「全屏」的说法一并改。

- [ ] **Step 4: 文档**

- `README.md`：`:5`「常驻的底部歌词」→ 把两块都算上；`:17`、`:18` 两条功能表里的「`Y` 全屏」→「`v` 频谱区（与列表互斥；歌词栏不在时吃满内容区）」；`:110` 键位表那一行拆成 `y`（歌词栏开 / 关）与 `v`（频谱区开 / 关）两行，删掉「全屏正在播放 / `Y` 或 `Esc` 退出」的措辞。
- `docs/design.md`：先给术语定名（**内容区**里三样：**列表**、**歌词栏**、**频谱区**；从下到上是状态栏 → 播放栏 → 内容区），再把 `:231`「队列浮层与全屏正在播放视图」→「队列浮层与频谱区」；`:250`「底部那块（`y`）与全屏视图的下半块（`Y`）」→「歌词栏（`y`）与频谱区（`v`）」；`:253`「全屏正在播放（`Y`）是两块」→ 改写成「内容区的两个栏 + 四种合成」那一套；`:272` 的「`?` 帮助里那一行写的是「Y 全屏：歌词 + 频谱」」→ 改成 `y` / `v` 两行。另在键位约定那一段加一条：**默认小写；大写只留给升级型能力或小写被占用**。

- [ ] **Step 5: 跑全量 + clippy + 提交**

Run: `cargo test && cargo clippy --all-targets --locked -- -D warnings`

```bash
git add src/tui/draw/overlay.rs README.md docs/design.md
git commit -m "docs(panels): 帮助表与 README/design 换成两个栏（y 歌词栏 / v 频谱区）"
```

---

## 收尾（不在任务里，执行者最后做一次）

- 真机（tuios，真二进制）：播放中 `y` 收起/展开歌词栏、`v` 进出频谱区、`v` + `y` 得到大频谱、切屏收掉频谱。
- 全支分支终审（`review-package <plan> <merge-base> HEAD` + 一个全新 reviewer），再收尾合并。

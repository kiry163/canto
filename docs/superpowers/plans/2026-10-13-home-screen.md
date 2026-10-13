# 首页（`0` 界面）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 进主界面时落在新的首页上（四行入口 + 编号），`j`/`k` 选、`Enter` 进、数字 `1`–`4` 直跳、`0` 回首页。

**Architecture:** 新增屏幕层的第三个状态 `Screen::Home`（`Menu`/`Nav` 那套「一份歌曲列表 + 选中位置 + 筛选」不套在首页上）；首页的选中位置用 `ListState`（跟队列浮层同一套 `step_state`/`edge_state`），绘制是一个新的 `draw/home.rs`，只有内容区不同、其余外壳（播放条、状态行、歌词块、浮层）照旧。派发不另立路径：`Screen::Home | Screen::Main => apply`，差异靠新键位上下文 `Context::Home` 与 `apply` 里的四处屏判断。

**Tech Stack:** Rust 2024；ratatui 0.30（`List` + `render_stateful_widget` + `ListState`）；无新依赖。

**Spec:** `docs/superpowers/specs/2026-10-13-home-screen-design.md`

## Global Constraints

- 无新依赖、无新配置项、无新落盘文件。
- 每个 commit 都要三平台编得过：CI = Linux `clippy --all-targets --locked -- -D warnings` + ubuntu/windows/macos `cargo test --locked`。
- 键位只有两处真源：`keys.rs` 的 `resolve()`（键 → `Action`）与 `draw/mod.rs` 的 `hints()`（状态行提示）；帮助表由 `draw/overlay.rs` 的 `help_rows()` 生成。加了键要三处对齐。
- 状态行通知位固定 32 列（`NOTICE_COLS`）：文案短句、无路径、无错误链，细节进 `canto.log`。
- 首页**不发网络请求、不读会话**：它只用 `Menu::ALL` 与静态标题。
- `test_app()` 走 `App::new`（初值仍是 `Screen::Main`），所以现有测试的上下文不用改。
- `Esc` 在首页**不绑**（首页在最外层）；列表专属键（`/`、`d`、`p`、`x`、`a`、`A`、`Shift+D`、`PgUp`/`PgDn`）在首页也不绑。

## Review Focus

最可能咬到使用者的五处（每处都在下面某个任务里有对应测试）：

1. **首页上按 `d`/`/`（列表专属键）**：必须什么都不发生——不弹「没有选中那首歌」的通知、不换屏。（Task 2）
2. **在首页按 `Esc`**：不许弹掉别的屏记住的层级（`nav.shown` 在首页上可能还是 `Some`：先进过「我的歌单 › 某歌单」再回首页）。（Task 2）
3. **登录后落首页但没有预取收藏**：`favorites_loaded` 仍是 `false`，状态行没有「加载 「我喜欢」…」进度。（Task 1）
4. **首页上按 `Y`（全屏视图）**：上下文仍是 `NowPlaying`（「列表看不见就不给列表键」那条规则不能破），`Esc`/`Y` 能退回来。（Task 1）
5. **最小窗口（60×15）**：首页四行都在界内、状态行是首页那条提示（不是主界面的）。（Task 1）

---

### Task 1: 首页这一层（`Screen::Home`、`Context::Home`、落屏、绘制、进屏）

**Files:**
- Modify: `src/tui/keys.rs`（`Action::Home`、`Context::Home` + 映射、`0` 绑进 `common()` 与 `NowPlaying`）
- Modify: `src/tui/state.rs`（`Screen::Home`、`home_state`、`go_home()`、`dispatch`/`apply_login`/`dispatch_hotkey`/`context()``/`finish_login`/`load_initial`、`apply` 的 `Action::Home`/`Enter`/`Menu(n)`/`move_selection`/`select_edge`）
- Modify: `src/tui/draw/mod.rs`（`mod home;`、内容区分支、`hints(Context::Home)`）
- Create: `src/tui/draw/home.rs`（首页内容区那一块）
- Test: 各文件末尾的 `#[cfg(test)] mod tests`（本仓库的测试就在同文件里）

**Interfaces:**
- Produces（Task 2、3 要用）：
  - `keys::Action::Home`（无参变体，含义「回首页」）
  - `keys::Context::Home`（首页的键位上下文）
  - `App.home_state: ListState`（`pub(super)`；`selected()` 是 `Menu::ALL` 的下标）
  - `App::go_home(&mut self)`（私有；`screen = Screen::Home` + 高亮落在 `nav.menu` 那一项）
  - `App::draw_home(&mut self, frame: &mut Frame, area: Rect)`（`draw/home.rs`）
- Consumes：`Menu::ALL`、`Menu::title()`、`step_state()`/`edge_state()`（`state.rs:35/44`，收 `&mut ListState`）、`draw::highlight()`、`draw::panel_block()`、`draw::hints()`

- [ ] **Step 1: 写失败的测试（`src/tui/keys.rs`）**

```rust
    /// 首页自己一套键位：`Enter` 进屏、`0` 回首页，列表专属键一个都不绑。
    #[test]
    fn the_home_screen_has_its_own_keys() {
        for (key, action) in [
            (KeyCode::Char('j'), Action::Down),
            (KeyCode::Down, Action::Down),
            (KeyCode::Char('k'), Action::Up),
            (KeyCode::Up, Action::Up),
            (KeyCode::Char('g'), Action::First),
            (KeyCode::Home, Action::First),
            (KeyCode::Char('G'), Action::Last),
            (KeyCode::End, Action::Last),
            (KeyCode::Enter, Action::Enter),
            (KeyCode::Char('0'), Action::Home),
            (KeyCode::Char('1'), Action::Menu(0)),
            (KeyCode::Char('4'), Action::Menu(3)),
            (KeyCode::Char(' '), Action::PlayPause),
            (KeyCode::Char('n'), Action::Next),
            (KeyCode::Char('b'), Action::Prev),
            (KeyCode::Left, Action::SeekBack),
            (KeyCode::Right, Action::SeekForward),
            (KeyCode::Char('+'), Action::VolumeUp),
            (KeyCode::Char('-'), Action::VolumeDown),
            (KeyCode::Char('m'), Action::CycleMode),
            (KeyCode::Char('u'), Action::QueueOverlay),
            (KeyCode::Char('Y'), Action::ToggleNowPlaying),
            (KeyCode::Char('?'), Action::Help),
            (KeyCode::Char('L'), Action::Logout),
            (KeyCode::Char('q'), Action::Quit),
        ] {
            assert_eq!(press(Context::Home, key), Some(action), "{key:?} 在首页该有绑定");
        }
        // 首页没有「选中的歌」，`Esc` 也没有上一层可退：这些一个都不绑。
        for key in [
            KeyCode::Esc,
            KeyCode::Char('/'),
            KeyCode::Char('d'),
            KeyCode::Char('p'),
            KeyCode::Char('x'),
            KeyCode::Char('a'),
            KeyCode::Char('A'),
            KeyCode::Char('D'),
            KeyCode::PageUp,
            KeyCode::PageDown,
        ] {
            assert_eq!(press(Context::Home, key), None, "{key:?} 不该在首页有绑定");
        }
    }

    /// `0` 回首页：主界面与全屏视图都能按，模态浮层与输入态不抢。
    #[test]
    fn zero_goes_home_from_the_main_screen() {
        assert_eq!(press(Context::Content, KeyCode::Char('0')), Some(Action::Home));
        assert_eq!(press(Context::NowPlaying, KeyCode::Char('0')), Some(Action::Home));
        assert_eq!(press(Context::QueueOverlay, KeyCode::Char('0')), None, "模态浮层里 0 不抢");
        assert_eq!(press(Context::PlaylistPicker, KeyCode::Char('0')), None);
        assert_eq!(
            press(Context::SearchInput, KeyCode::Char('0')),
            Some(Action::InputChar('0')),
            "输入框里 0 是打字"
        );
    }
```

同时把 `Context::Home` 加进 `ctrl_c_quits_from_every_context` 那个上下文列表（`keys.rs` 里 `for context in [...]` 那处）。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --bin canto keys::tests::the_home_screen_has_its_own_keys`
Expected: 编译失败——`cannot find variant Home` / `cannot find value Context::Home`。

- [ ] **Step 3: `keys.rs` 加变体与映射**

- `Action` 里加一个变体（照本文件既有风格写注释，例）：

```rust
    /// `0`：回首页（首页是屏幕层最外的一层，各屏入口与编号都在它上面）。
    Home,
```
- `resolve()` 里加一支 `Context::Home => match key.code { … }`，内容照 Step 1 的表：`j`/`k`/`↑`/`↓`、`g`/`G`/`Home`/`End`、`Enter`、`0`、`1`..`4`、播放键（`Space n b ← → + - m`）、`u`、`Y`、`?`/`F1`、`L`、`q`，其余 `None`。`Ctrl-C` 已在函数开头统一处理，不用重复。
- `common()`（`Context::Content`）与 `Context::NowPlaying` 里各加 `KeyCode::Char('0') => Some(Action::Home)`。

- [ ] **Step 4: `state.rs` 加屏幕状态与派发**

按顺序做这几处（`Screen` 是穷尽 match，改完一处下一处会立刻编不过，照报错走也行）：

1. `Screen` 加 `Home,`（`Login(LoginState)` 与 `Main` 之间）；`App` 加 `pub(super) home_state: ListState,`，`App::new` 里 `ListState::default()`（初值仍是 `Screen::Main`，加一句注释说明它只活到 `tui::run` 决定落屏）。
2. `dispatch()` 的 `match self.screen`：`Screen::Login(_) => self.apply_login(action),`，`Screen::Home | Screen::Main => self.apply(action),`。
3. `apply_login()` 里粘贴 cookie 提交那个 `match &mut self.screen`：加 `Screen::Home => return,`。
4. `dispatch_hotkey()` 的门：`if matches!(self.screen, Screen::Main | Screen::Home) {`。
5. `context()`：在 `now_playing_full` 那支之后、`search_input` 之前加 `if matches!(self.screen, Screen::Home) { return keys::Context::Home; }`。
6. `finish_login()`：`self.screen = Screen::Home; self.go_home();`（**不再** `switch_menu(Menu::Favorites)`——那是预取收藏）；`load_initial()`：`self.screen = Screen::Home; self.go_home();`。
7. 新私有方法（放 `switch_menu` 附近）：

```rust
    /// 回首页：屏幕换成首页，高亮停在**刚离开的那一屏**。
    ///
    /// 只动这两样：别的屏记住的层级（`nav.shown`）、筛选、选中位置都不碰——
    /// 首页是最外一层，不该顺手改动谁。
    pub(super) fn go_home(&mut self) {
        self.screen = Screen::Home;
        let index = Menu::ALL.iter().position(|menu| *menu == self.nav.menu).unwrap_or(0);
        self.home_state.select(Some(index));
    }
```

8. `apply()` 里加四处首页分支：
   - `keys::Action::Home => self.go_home(),`
   - `Enter` 那支开头：`if matches!(self.screen, Screen::Home) { let menu = Menu::ALL[self.home_state.selected().unwrap_or(0)]; self.screen = Screen::Main; self.switch_menu(menu); return; }`
   - `Menu(n)` 那支：进屏时顺手 `self.screen = Screen::Main;`（照现有写法插在 `switch_menu` 前）
   - `move_selection()` / `select_edge()`：首页那一支要放在**队列浮层与选择框那两支之后**、`self.nav.move_selection(delta)` 之前——首页上开着队列浮层（`u`）时，`j`/`k` 该挪浮层的高亮，次序跟 `context()` 一致（帮助 > 队列/选择框 > 首页 > 列表）：

```rust
        if matches!(self.screen, Screen::Home) {
            step_state(&mut self.home_state, Menu::ALL.len(), delta);
            return;
        }
```

（`select_edge` 同理用 `edge_state(&mut self.home_state, Menu::ALL.len(), last)`。）

- [ ] **Step 5: `draw/home.rs` + 内容区分支 + 状态行提示**

新建 `src/tui/draw/home.rs`：`impl App { pub(super) fn draw_home(&mut self, frame: &mut Frame, area: Rect) }`，照 `content.rs` 里歌单列表那个写法（`content.rs:38-41`）：`List::new(items).block(panel_block(" 首页 ")).highlight_style(highlight()).highlight_symbol("▶ ")` + `render_stateful_widget(list, area, &mut self.home_state)`；`items` 由 `Menu::ALL` 生成，每行 `format!("{}  {}", index + 1, menu.title())`。

`draw/mod.rs`：`mod home;`（跟 `mod content;` 并列）、内容区那句改成分三支——

```rust
        if self.now_playing_full {
            self.draw_now_playing(frame, areas.content);
        } else if matches!(self.screen, Screen::Home) {
            self.draw_home(frame, areas.content);
        } else {
            self.draw_content(frame, areas.content);
        }
```

并在 `hints()` 里加 `keys::Context::Home => "Enter 进入 · 1-4 直跳 · Space 暂停 · n/b 切歌 · u 队列 · ? 帮助",`。

- [ ] **Step 6: 跑 Step 1 的测试**

Run: `cargo test --bin canto keys::tests`
Expected: PASS。

- [ ] **Step 7: 写 state 层与绘制层的测试**

`src/tui/state.rs` 的 `mod tests`：

```rust
    /// 登录成功后落首页——而且**没有**顺手预取收藏（原来落收藏并当场拉一遍）。
    #[test]
    fn logging_in_lands_on_the_home_screen_without_prefetching_favorites() {
        let (mut app, _runtime) = crate::tui::test_support::test_app();
        app.finish_login("o123".to_string());
        assert!(matches!(app.screen, Screen::Home), "落屏该是首页");
        assert!(!app.nav.favorites_loaded, "首页不预取收藏");
        assert_eq!(app.home_state.selected(), Some(0), "高亮在第一项");
    }

    /// 首页 `Enter` / 数字：进那一屏；`0` 回首页；首页 `Esc` 连绑定都没有。
    #[test]
    fn the_home_enters_screens_and_comes_back() {
        let (mut app, _runtime) = crate::tui::test_support::test_app();
        app.screen = Screen::Home;
        app.home_state.select(Some(1));
        app.apply(keys::Action::Enter);
        assert!(matches!(app.screen, Screen::Main));
        assert_eq!(app.nav.menu, Menu::Playlists, "进的是高亮那一项");

        app.apply(keys::Action::Home);
        assert!(matches!(app.screen, Screen::Home), "`0` 回首页");
        app.apply(keys::Action::Menu(3));
        assert!(matches!(app.screen, Screen::Main));
        assert_eq!(app.nav.menu, Menu::History, "数字直达");
    }

    /// 首页上挪选中：`j`/`k` 到底 / 到顶都不越界（`step_state` 夹住）。
    #[test]
    fn the_home_moves_its_own_highlight() {
        let (mut app, _runtime) = crate::tui::test_support::test_app();
        app.screen = Screen::Home;
        app.go_home(); // 高亮 = nav.menu 那一项
        let start = app.home_state.selected().expect("该有高亮");
        app.apply(keys::Action::Down);
        assert_eq!(app.home_state.selected(), Some(start + 1));
        app.apply(keys::Action::Last);
        assert_eq!(app.home_state.selected(), Some(Menu::ALL.len() - 1));
        app.apply(keys::Action::Down);
        assert_eq!(app.home_state.selected(), Some(Menu::ALL.len() - 1), "到底不动");
        app.apply(keys::Action::First);
        assert_eq!(app.home_state.selected(), Some(0));
        app.apply(keys::Action::Up);
        assert_eq!(app.home_state.selected(), Some(0), "到顶不动");
    }

    /// 首页上开队列浮层（`u`）时，`j`/`k` 挪的是**浮层**的高亮，不是首页的（次序：
    /// 帮助 > 队列/选择框 > 首页 > 列表，跟 `context()` 一致）。
    #[test]
    fn the_home_queue_overlay_takes_the_arrow_keys() {
        let (mut app, _runtime) = crate::tui::test_support::test_app();
        app.screen = Screen::Home;
        app.go_home();
        let home_before = app.home_state.selected();
        app.queue = crate::tui::queue::Queue::new(
            vec![crate::tui::test_support::test_song("a"), crate::tui::test_support::test_song("b")],
            0,
            String::new(),
        );
        app.apply(keys::Action::QueueOverlay);
        assert!(app.queue_open);

        app.apply(keys::Action::Down);
        assert_eq!(app.queue_state.selected(), Some(1), "队列高亮该动");
        assert_eq!(app.home_state.selected(), home_before, "首页的高亮不许动");
    }

    /// 首页算主界面：全局热键在那儿照样生效（原来只在 `Screen::Main`）。
    #[test]
    fn the_global_hotkeys_work_on_the_home_screen() {
        let (mut app, _runtime) = crate::tui::test_support::test_app();
        app.screen = Screen::Home;
        app.dispatch_hotkey(keys::Action::Help);
        assert!(app.help_open, "首页也算主界面");
    }

    /// 首页上按 `Y` 之后是「全屏视图」的上下文（列表看不见那条规则不能破）。
    #[test]
    fn the_home_fullscreen_view_uses_the_now_playing_context() {
        let (mut app, _runtime) = crate::tui::test_support::test_app();
        app.screen = Screen::Home;
        assert_eq!(app.context(), keys::Context::Home);
        app.apply(keys::Action::ToggleNowPlaying);
        assert_eq!(app.context(), keys::Context::NowPlaying);
        app.apply(keys::Action::ToggleNowPlaying);
        assert_eq!(app.context(), keys::Context::Home, "退回来还是首页");
    }
```

`src/tui/draw/mod.rs` 的 `mod tests`：

```rust
    /// 首页画出来：标题 + 四行「编号 名字」 + 选中高亮。
    #[test]
    fn the_home_screen_lists_the_four_screens_with_numbers() {
        let (mut app, _runtime) = test_app();
        app.screen = Screen::Home;
        app.go_home();
        let rows = frame_rows(&mut app, 120, 30);
        assert!(rows.iter().any(|row| row.contains("┌首页")), "标题：{rows:?}");
        for (number, title) in [(1, "我的收藏"), (2, "我的歌单"), (3, "搜索"), (4, "历史")] {
            assert!(
                rows.iter().any(|row| row.contains(&format!("{number}{title}"))),
                "{number} {title} 那一行没画出来"
            );
        }
        assert!(rows.iter().any(|row| row.contains("▶1我的收藏")), "高亮在第一项：{rows:?}");
    }

    /// 首页的状态行是首页那条提示，不是主界面的。
    #[test]
    fn the_home_screen_has_its_own_status_line_hint() {
        let (mut app, _runtime) = test_app();
        app.screen = Screen::Home;
        let status = frame_rows(&mut app, 120, 30).pop().expect("状态行");
        assert!(status.contains("Enter进入"), "{status:?}");
        assert!(status.contains("1-4直跳"), "{status:?}");
        assert!(!status.contains("Enter播放"), "不是主界面那条：{status:?}");
    }

    /// 最小窗口（内容区 10 行）里四行都在界内。
    #[test]
    fn the_home_screen_fits_the_smallest_window() {
        let (mut app, _runtime) = test_app();
        app.screen = Screen::Home;
        let rows = frame_rows(&mut app, 60, 15);
        assert!(rows.iter().any(|row| row.contains("1我的收藏")), "四行都得在：{rows:?}");
        assert!(rows.iter().any(|row| row.contains("4历史")), "四行都得在：{rows:?}");
    }
```

- [ ] **Step 8: 跑测试**

Run: `cargo test --bin canto`
Expected: PASS（若 Step 7 先红，说明 Step 4/5 里少了一支，照测试信息补）。

- [ ] **Step 9: clippy + 提交**

```bash
cargo clippy --all-targets --locked -- -D warnings
git add src/tui/keys.rs src/tui/state.rs src/tui/draw/mod.rs src/tui/draw/home.rs
git commit -m "feat(home): 首页(0 界面)这一层——Screen::Home、Context::Home、落屏与进屏"
```

---

### Task 2: 回首页的另两条路（列表顶部 `Esc`）+ 首页上列表键无副作用

**Files:**
- Modify: `src/tui/state.rs`（`apply()` 的 `Back` 链末尾）
- Test: `src/tui/state.rs` 的 `mod tests`

**Interfaces:**
- Consumes：Task 1 的 `App::go_home()`、`Screen::Home`、`Menu::ALL`
- Produces：无新对外接口

- [ ] **Step 1: 写失败的测试**

```rust
    /// 列表顶部（没进歌单、没开筛选）再按 `Esc` 回首页；首页上 `Esc` 连绑定都没有（Task 1 的
    /// `the_home_screen_has_its_own_keys` 已钉）。
    #[test]
    fn esc_at_the_top_of_a_list_goes_home() {
        let (mut app, _runtime) = crate::tui::test_support::test_app();
        app.screen = Screen::Main;
        app.nav.switch_menu(Menu::Favorites);
        app.apply(keys::Action::Back);
        assert!(matches!(app.screen, Screen::Home), "顶层 Esc 该回首页");
    }

    /// 回首页**不许**动别的屏记住的层级：先进「我的歌单 › 某歌单」再回首页，
    /// 那个屏仍是曲目那一层（`nav.shown` 留着）。
    #[test]
    fn going_home_leaves_the_other_screens_levels_alone() {
        let (mut app, _runtime) = crate::tui::test_support::test_app();
        app.nav.switch_menu(Menu::Playlists);
        app.nav.push_tracks("1", "嗯", vec![crate::tui::test_support::test_song("a")]);
        assert_eq!(app.nav.title_chain(), vec!["我的歌单".to_string(), "嗯".to_string()]);

        app.apply(keys::Action::Home);
        assert!(matches!(app.screen, Screen::Home));
        assert!(app.nav.shown.is_some(), "回首页不该把别人的层级弹掉");

        app.apply(keys::Action::Menu(1));
        assert_eq!(
            app.nav.title_chain(),
            vec!["我的歌单".to_string(), "嗯".to_string()],
            "回到歌单屏还在原来那一层"
        );
    }

    /// 首页上没有「选中的歌」：列表专属键按下去什么都不发生（不弹通知、不换屏）。
    #[test]
    fn the_home_ignores_song_keys() {
        let (mut app, _runtime) = crate::tui::test_support::test_app();
        app.screen = Screen::Home;
        for action in [
            keys::Action::Download,
            keys::Action::AddToPlaylist,
            keys::Action::RemoveFromPlaylist,
            keys::Action::AppendQueue,
            keys::Action::InsertQueue,
            keys::Action::Filter,
        ] {
            app.apply(action);
            assert!(matches!(app.screen, Screen::Home), "{action:?} 不该换屏");
            assert!(app.notice_text().is_none(), "{action:?} 不该弹通知");
        }
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --bin canto state::tests::esc_at_the_top_of_a_list_goes_home`
Expected: FAIL（`assertion failed: matches!(app.screen, Screen::Home)`）。

- [ ] **Step 3: 改 `apply()` 的 `Back` 链末尾**

现在是 `} else { self.nav.pop_tracks(); }`，改成：

```rust
                } else if !self.nav.pop_tracks() {
                    // 到头了：退一层就是回首页（首页自己不在 `Context::Home` 里绑 `Esc`，
                    // 所以这条链在首页上根本不会进——不用再挡一次）。
                    self.go_home();
                }
```

（`Nav::pop_tracks()` 本来就返回 `bool`，`state.rs:386`。）

- [ ] **Step 4: 跑测试**

Run: `cargo test --bin canto`
Expected: PASS。

- [ ] **Step 5: clippy + 提交**

```bash
cargo clippy --all-targets --locked -- -D warnings
git add src/tui/state.rs
git commit -m "feat(home): 列表顶部 Esc 回首页，且不动别的屏记住的层级"
```

---

### Task 3: 帮助表那一行 + 文档 + 真机验收

**Files:**
- Modify: `src/tui/draw/overlay.rs`（`help_rows()` 的「切屏」段加 `0` 首页）
- Modify: `README.md`（键位表加 `0` 与首页那一行、功能表、落屏说明）
- Modify: `docs/design.md`（界面一节：首页这一层与「切屏」的关系）
- Test: `src/tui/draw/overlay.rs` 的 `mod tests`

**Interfaces:**
- Consumes：Task 1/2 的 `Screen::Home`、`Context::Home`
- Produces：无

- [ ] **Step 1: 写失败的测试（`src/tui/draw/overlay.rs`）**

在 `the_table_pairs_every_key_with_a_description` 里补两行断言（别新建测试——那张表就是这条断言的归属）：

```rust
        assert!(cells.contains(&("0", "首页")), "首页入口要在「切屏」段里：{cells:?}");
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --bin canto draw::overlay::tests::the_table_pairs_every_key_with_a_description`
Expected: FAIL（`首页入口要在「切屏」段里`）。

- [ ] **Step 3: 改 `help_rows()`**

「切屏」段（`Row::Section("切屏")` 之后）加一行 `Row::Keys { keys: "0", help: "首页" }`，顺序放最前（`0` 是首页、`1`–`4` 是四屏）。

- [ ] **Step 4: 跑测试**

Run: `cargo test --bin canto draw::overlay::tests`
Expected: PASS。

- [ ] **Step 5: 文档**

- `README.md`：键位表加 `| 0 | 回首页（各屏入口与编号；j/k 选、Enter 进、1-4 直跳） |`；功能表加一行「首页」；「键位」那节说明里补一句「登录后落在首页，不再是收藏」。
- `docs/design.md` 的界面一节：补一条——首页是屏幕层的第三个状态（`Screen::Home`），`Menu`/`Nav` 不套在它上面；切屏仍是数字键直达，首页只是把入口列出来；`Esc` 到顶回首页、首页自己按 `Esc` 无绑定。

- [ ] **Step 6: 全量测试 + clippy**

Run: `cargo test` 与 `cargo clippy --all-targets --locked -- -D warnings`
Expected: 全绿、clippy 干净。

- [ ] **Step 7: 真机验收（tuios，真二进制）**

按仓库根目录的 `AGENTS.md`：窗口 `canto-home`，`tuios new-window … -- <二进制> --log debug`，等 `我的收藏`…… 注意：**落屏变了**，等的字符串要换成首页上一定会出现的（`首页` 或 `我的收藏`，后者仍在首页第一行里）。

| # | 动作 | 期望 |
|---|---|---|
| 1 | 起 canto（已登录） | 直接停在首页；内容区四行带编号；状态行是 `Enter 进入 · 1-4 直跳 …` |
| 2 | `j` `j` `Enter` | 进「我的歌单」；再 `0` 回首页，高亮停在「我的歌单」那一行 |
| 3 | `4` | 直达历史屏 |
| 4 | 在历史屏按 `Esc`（在顶层） | 回首页 |
| 5 | 首页按 `Y` | 全屏视图；`Esc` 或 `Y` 退回首页 |
| 6 | 首页按 `d` / `/` | 什么都不发生（状态行不弹「没有选中」） |
| 7 | `?` | 帮助表「切屏」段里能看到 `0  首页` |
| 8 | 首页按 `ctrl+shift+space`（canto 不在前台） | 播放 / 暂停照样生效 |

- [ ] **Step 8: 提交**

```bash
git add src/tui/draw/overlay.rs README.md docs/design.md
git commit -m "docs(home): 帮助表加 0 首页 + README/design 的首页说明"
```

---

## 自查记录（写计划时跑过）

- **spec 覆盖**：spec §1 的 8 处改动 → Task 1 Step 4 逐条列全（含 `apply_login` 那个 match 与 `Back` 链——后者在 Task 2）；§2 键位 → Task 1 Step 3；§3 导航规则 → Task 1 Step 4/7 与 Task 2；§4 绘制 → Task 1 Step 5/7；§6 测试计划 → 三个任务的测试；§7 文档 → Task 3。
- **类型一致性**：`Screen::Home` / `home_state: ListState` / `go_home()` / `draw_home()` / `Action::Home` / `Context::Home` 在三个任务里名字一致；`home_selected: usize`（spec 里的写法）在计划里落成 `home_state: ListState`——行为相同（`selected()` 就是那个下标），但能复用 `step_state`/`edge_state` 与 `render_stateful_widget`；spec 的意图（下标即 `Menu::ALL` 的下标）不变。
- **Review Focus**：五条各自挂在 Task 1（3/4/5 条）与 Task 2（1/2 条）的测试上。

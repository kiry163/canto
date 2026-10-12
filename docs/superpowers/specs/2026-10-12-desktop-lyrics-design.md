# canto 桌面歌词（Windows 覆盖层）设计

日期：2026-10-12
状态：待使用者复核

## 背景与目标

canto 是终端程序，歌词只有两个去处：底部那块常驻歌词、以及 `Y` 全屏视图。共同点是**都得看着终端**。
需求是**桌面歌词**：播放时在当前桌面之上浮一条歌词（当前句 + 下一句），不抢焦点、不挡操作，
跟终端在哪、是不是前台都无关。

**这一份只做 Windows**。macOS 由使用者在 Mac 上另起一轮设计与实现（本 spec 负责把接缝留干净：
取词与状态机放平台无关的那一半，macOS 只需要补「建窗口 + 画字」）。

数据侧不用做：`Lyric::line_at(position)`（`draw/lyric.rs:118` 正在用）已经能回答「现在唱到哪句」，
歌词本身（含翻译）在 `qq/lyric.rs` 抓取并缓存。

## 非目标（YAGNI，明确不做）

- **macOS / Linux/Wayland**：这一份不写。macOS 换成另一份 `cfg` 实现即可（接缝见 §5），
  Linux 没有通用办法（要 wlr-layer-shell / GTK 那一套），不做。
- **逐字（卡拉 OK）着色**：歌词数据是行级的，逐字要另一条数据链路（KRC / 加密 QRC），不做。
- **界面内设置项**：字号、颜色、字体、透明度都在 `config.toml` 里改——跟 canto 其它配置一个口径。
- **多显示器选择**：只认主屏；换过显示器/分辨率导致窗口在屏外时夹回主屏内。
- **动画与过渡**：不淡入淡出、不滚动，换行直接换。
- **覆盖层自己的播放控制**（点它暂停之类）：不做，它是展示层；控制走全局热键或终端界面。

## 决策记录

| 决策 | 取值 | 理由 |
|---|---|---|
| 平台 | 只做 Windows | 唯一我能真机验收的平台；现有依赖就够（只给 `windows-sys` 补 feature） |
| 进程模型 | 同进程内的**专用线程** | 与刚做完的全局热键线程同一套路（自建窗口 + 消息循环 + `PostMessageW` 唤醒），无 IPC |
| 渲染 | `UpdateLayeredWindow` + 32 位 DIB | 取色透明（`SetLayeredWindowAttributes` 的 color key）会让灰度抗锯齿的文字边缘混进 key 色、出彩边；per-pixel alpha 才干净，而且 `opacity` 正好映射到 `BLENDFUNCTION.SourceConstantAlpha` |
| 文字抗锯齿 | `ANTIALIASED_QUALITY`（灰度），不用 ClearType | ClearType 是子像素 AA，做 alpha 混合会出彩色边 |
| 字体 | 默认 `Microsoft YaHei UI`，配置可改 | Windows 中文界面字体；缺了 GDI 自己回退 |
| 位置持久化 | 存**中心点**在独立状态文件 | 窗口宽度随文字变，存左上角会一行长一行短地左右抖；`config.toml` 是手写的，程序不改写它（跟 `history.json` 一个口径） |
| 拖动 | 解锁后 `WM_NCHITTEST` 回 `HTCAPTION`，交给 Windows 自己拖 | 比自己跟鼠标少一半代码；`WM_EXITSIZEMOVE` 收尾最可靠 |
| 解锁热键 | 复用 `[hotkeys]` 表，加第 6 条 `lock = "ctrl+shift+l"` | 一条注册/报错/日志链路，不另立一套 |
| 默认开不开 | `enabled = true` | 跟 `[hotkeys]` 一样「装上就可用」；建窗口失败也只报一句、播放照常 |
| 窗口尺寸 | 每次更新按文字实测尺寸 `SetWindowPos` | 背板贴字，拖动命中区也就贴字 |

## 1. 取词与状态机（平台无关）

输入是「一帧」：歌词、引擎快照（位置/播放态）、配置、锁定态。输出是**要不要推** + 新 payload。

```rust
/// 一帧的外部输入（都是借来的，不复制）。
pub struct Frame<'a> {
    pub lyric: Option<&'a Lyric>,
    /// 引擎快照：`phase`（`Idle`/`Buffering`/`Playing`/`Paused`/`Failed`）、`position` 都在里面。
    pub snapshot: &'a Snapshot,
}

/// 覆盖层要显示的东西；`PartialEq` 就是「要不要重画」的判据。
#[derive(Clone, PartialEq, Eq)]
pub struct Payload {
    /// `false` = 该隐藏（窗口不销毁，只是 `SW_HIDE`，位置与锁定态留着）。
    pub visible: bool,
    /// 当前句（可能是空串：歌词里的纯音乐段）。
    pub current: String,
    /// 下一句（最后一句之后是空串）。
    pub next: String,
    /// 当前句的译文（配置关掉或没有就是 `None`）。
    pub translation: Option<String>,
    /// 锁定态（决定鼠标穿透与那圈提示框）。
    pub locked: bool,
}
```

`advance(&mut self, frame: Frame<'_>, config: &DesktopLyricsConfig) -> Option<Payload>`：算出「理想
payload」，跟**上一次推出去的**比较，不同才返回 `Some`。表：

| 情形 | 结果 |
|---|---|
| 当前句/下一句/译文/锁定态任一变了 | 推一次 |
| 位置在**同一句**内推进、音量/进度之类的变化 | 不推（逐帧零工作） |
| `config.enabled == false` | 不推（句柄是 `none()`，连线程都没有） |
| 没有歌词（`lyric == None`） | `visible = false` |
| 有歌词、但当前句是空文本（纯音乐段） | `visible = true`，`current = ""`（只显示下一句） |
| `phase` 是 `Idle` / `Failed` | `visible = false` |
| `phase` 是 `Paused` 且 `hide_when_paused` | `visible = false` |
| `phase` 是 `Buffering` | 内容不推（沿用上一次的样子，取流时别闪一下）；但**锁定态变了要推**——按了解锁却摸不到窗口，会被当成功能坏了 |
| 解锁态（`locked == false`） | **强制 `visible = true`**，没歌词时 `current` 显示提示语（否则解锁了却找不到窗口，就没法拖） |

> 表里「当前句是空文本」这一格**没法由真实数据造出来**：`Lyric::parse` 会把空文本的行丢掉
> （见 `src/model.rs`），所以纯音乐段实际看到的是「上一句一直挂着」。状态机仍然把这一格当契约
> 实现（`an_empty_line_shows_only_the_next_one`），换个歌词来源就用得上。

## 2. 窗口与渲染（Windows）

窗口：`WS_POPUP` + `WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW`，
`WS_EX_NOACTIVATE` 保证永不抢焦点、`WS_EX_TOOLWINDOW` 保证不进任务栏/Alt-Tab。

每帧绘制：
1. 取当前 DPI（`GetDpiForWindow`）→ 字号 = `font_size × dpi / 96`。
2. `CreateFontW`：字体面（默认 `Microsoft YaHei UI`）、`ANTIALIASED_QUALITY`、粗细可后续再加。
3. 用 `GetTextExtentPoint32W` 量当前句/下一句/译文三行 → 窗口宽 = 最宽行 + 内边距，
   高 = 三行的和 + 内边距；`SetWindowPos` 把窗口按**中心点**摆好。
4. `CreateDIBSection`（32 位、自上而下、`BI_RGB`）→ 选进内存 DC → 先整块清零（全透明）。
5. 三行照配置画，**自上而下：当前句 → 译文 → 下一句**（译文关掉或没有就没有中间那行）：
   当前句 `color`，下一句与译文 `next_color`；
   每行先按 8 个方向偏移画描边（`outline`），再把正文画在上面 → 有描边的字在任何背景上都看得清。
6. `UpdateLayeredWindow(hwnd, …, &BLENDFUNCTION { SourceConstantAlpha: opacity×255/100, AC_SRC_ALPHA })`。
7. 解锁态：在 DIB 上再画一圈 1px 实线框（提示「现在能拖」）。

颜色配置写 `#RRGGBB`（小写也认），解析失败给一条警告并用默认值（跟 `quality` 一个口径）。

## 3. 锁定与拖动

| 时刻 | 行为 |
|---|---|
| 启动 | **锁定**（`WS_EX_TRANSPARENT` 开着，鼠标完全穿透）。锁定态不持久化——省得下次启动「怎么点不到」 |
| 按 `ctrl+shift+l`（`[hotkeys]` 第 6 条，`""` 可禁） | 切到解锁：去掉 `WS_EX_TRANSPARENT`、强制可见、画提示框 |
| 解锁时在窗口上按下拖动 | `WM_NCHITTEST` 回 `HTCAPTION` → Windows 自己拖整个窗口 |
| 松手（`WM_EXITSIZEMOVE`） | 取窗口矩形中心 → 通过 `Sender<(i32, i32)>` 回传给 TUI 线程 |
| TUI 侧 `tick()` | 排空那个通道 → 用「临时文件 + rename」写 `desktop-lyrics.json`（`{"cx":…, "cy":…}`） |
| 再按一次 `ctrl+shift+l` | 回到锁定（提示框消失） |
| 启动读不到/读坏了状态文件 | 用默认：主屏底部居中（`cy ≈ 屏高 × 0.85`、`cx = 屏宽 / 2`） |
| 读到的中心落在主屏之外 | 夹回主屏内（留一圈边距），免得「换过显示器就再也找不到」 |

热键同样只在 `Screen::Main` 派发（跟其它热键一条规则）：登录屏没有拖歌词的理由。

## 4. 配置契约

```toml
[desktop_lyrics]
enabled = true            # 关掉 = 不建线程、不建窗口（零成本）
font = "Microsoft YaHei UI"
font_size = 28            # 逻辑像素（按窗口 DPI 缩放）
color = "#FFFFFF"         # 当前句
outline = "#000000"       # 描边
next_color = "#9E9E9E"    # 下一句与译文
opacity = 90              # 0-100，整层透明度
show_translation = true
hide_when_paused = false
```

| 规则 | 行为 |
|---|---|
| 缺段 / 缺字段 | 用上表默认值（`#[serde(default)]`，与 `[hotkeys]` 同一套写法） |
| `opacity` 超出 0-100 | 夹到范围内 + 一条警告 |
| 颜色不是 `#RRGGBB` | 用默认值 + 一条点名到字段的警告（清成默认而不是禁用——这里没有「禁用」的语义） |
| `font` 写了系统里没有的字体 | 不报错，GDI 自己回退（想确认就去日志看实际用的字体面） |
| 模板 | `write_default` 里加这一段并带注释；README 的配置示例同步 |
| 热键表 | `[hotkeys]` 加 `lock = "ctrl+shift+l"`（`hotkeys::DEFAULTS` 从 5 条变 6 条） |

## 5. 模块与接口

新增 `src/overlay.rs`（跟 `hotkeys.rs` / `instance.rs` 一样放 crate 根，是平台集成模块）：

| 单元 | 位置 | 职责 | 依赖 |
|---|---|---|---|
| 状态机 | `overlay.rs` 顶部（平台无关） | `Frame` → `Payload`、变化检测、锁定态 | `model::Lyric`、`player::Snapshot`、`config` |
| 位置状态 | 同文件 | 读/写 `desktop-lyrics.json`、默认中心点、夹进主屏 | `paths`、`serde_json` |
| Windows 实现 | 同文件 `#[cfg(target_os = "windows")] mod imp` | 线程 + 窗口 + 消息循环 + DIB 绘制；`Arc<Mutex<Payload>>` 收更新、`PostMessageW` 唤醒 | `windows-sys`（新增 `Win32_Graphics_Gdi`、`Win32_UI_HiDpi`） |
| 其它平台 | `#[cfg(not(windows))] mod imp` | 空实现（`none()` + 两个空方法） | — |

```rust
pub struct DesktopLyrics {
    state: State,          // 平台无关：上次推的 payload、锁定态
    imp: imp::Imp,         // 平台：线程/窗口，或者空
}

/// 装出来的东西：句柄、位置回传通道（只有 Windows 有）、失败原因。
pub struct Installed {
    pub overlay: DesktopLyrics,
    /// 拖动结束时窗口线程发过来的新中心点；非 Windows / 关掉时是 `None`。
    pub positions: Option<Receiver<(i32, i32)>>,
    /// `Some` = 起不来（调用方报一句 + 日志），此时 `overlay` 是 `none()`。
    pub error: Option<String>,
}

impl DesktopLyrics {
    /// 空句柄：`enabled = false`、非 Windows、`App::new` 与测试都用它；`sync`/`toggle_lock` 是空操作。
    pub fn none() -> Self;
    /// 按配置与已存中心点起覆盖层（Windows）。
    pub fn install(config: &DesktopLyricsConfig, saved: Option<(i32, i32)>) -> Installed;
    /// 每帧调一次；只有变化才会真的推给窗口。
    pub fn sync(&mut self, frame: Frame<'_>, config: &DesktopLyricsConfig);
    /// 全局热键 `lock`：切锁定/解锁（下一帧自然会把新锁定态推出去）。
    pub fn toggle_lock(&mut self);
}
```

接线：
- `App` 加 `pub(super) overlay: DesktopLyrics` + 一个 `Option<Receiver<(i32, i32)>>`；
  `tick()` 里先 `let snapshot = self.engine.snapshot();`（它返回**落锁后克隆出来的一份**，借它最省事），
  再 `self.overlay.sync(Frame { lyric: self.lyric.as_ref(), snapshot: &snapshot }, &self.config.desktop_lyrics)`，
  然后排空位置通道、落盘。
- `tui::run` 里装一次（像 `hotkeys::install` 那样：配置已拿到、`App::new` 之后），失败就 `app.notify` + `tracing::warn!`。
- `keys::Action` 加一个 `LyricsLock`，`apply` 里 `self.overlay.toggle_lock()`。
- **macOS 的接缝**：将来写 macOS 那份只需实现 `imp::{Imp, Imp::none, install, push}` 三件（建窗口、画字、销毁），
  `State`/`Payload`/配置/接线/测试一行都不用动。

## 6. 线程模型与生命周期

```mermaid
sequenceDiagram
participant Loop as TUI 循环(主线程)
participant St as State(平台无关)
participant W as 覆盖层线程(Windows)
Loop->>St: sync(Frame)
St-->>Loop: Some(Payload) 只在变化时
Loop->>W: payload 写进 Arc<Mutex> + PostMessageW(WM_APP)
W->>W: 量宽 → 按窗口实际矩形取中心点 → SetWindowPos → 画 DIB → UpdateLayeredWindow
W-->>Loop: 中心点变了（拖动之后）→ Sender
Loop->>Loop: 落盘 desktop-lyrics.json
```

- `Drop for Imp`：`PostMessageW(WM_CLOSE)` → wndproc 里 `DestroyWindow` → `WM_DESTROY` 里
  `PostQuitMessage` → 出消息循环后收尾 → `join`（与热键线程同一套）。
- 退出顺序：`App` 里 `engine` 声明在 `overlay` 之前，所以是**引擎先析构**。这不影响什么
  ——窗口线程压根不碰引擎（只读那把 `Mutex<Payload>`），谁先走都留不下「歌词服务已死」。
- 窗口线程不读任何共享状态，除了那把 `Mutex<Payload>`（锁区间只有一次 clone）。

## 7. 退化与错误处理

| 情形 | 行为 |
|---|---|
| 建线程 / 建窗口失败 | 一条通知（`歌词窗口起不来`）+ 日志（原因），播放与界面照常 |
| `enabled = false` | 不建线程、不建窗口；`App::tick` 连 `sync` 都不调（「关掉就零成本」） |
| 非 Windows | 同上（配置段整体忽略，`cfg` 那半是空实现） |
| 没有歌词 / 未播放 | `SW_HIDE`（不销毁，位置与锁定态留着） |
| 状态文件坏了 / 没有 | 默认位置（主屏底部居中），不报错、不阻塞启动 |
| 主屏 DPI 变了 | 每次重画按当前 DPI 缩放字号，位置照旧按中心点算 |
| 全屏独占应用（游戏等） | 覆盖层压不过它——Windows 的既有边界，写进 README 已知限制 |

## 8. 测试计划

**先做的探针（可丢弃，不进仓库）**：一个 ~30 行的独立程序，验证 `UpdateLayeredWindow` + 32 位 DIB +
`ANTIALIASED_QUALITY` 中文描边字在**浅色与深色桌面背景上**都干净（无彩边、无锯齿块、不闪）。
判据与退路写在这里：若 DIB 里的 GDI 文字仍然难看，退路是「文字 + 一块半透明圆角背板」
（桌面歌词也常见），把 §2 的第 4–6 步换成画背板再画字。**探针通过才开始写正式代码。**

**单测**（平台无关那半，沿用现有「纯函数 + 断言」套路）：

| 测试 | 盯住什么 |
|---|---|
| `the_same_frame_twice_pushes_once` | 变化检测：同一帧第二次返回 `None` |
| `a_none_handle_syncs_nothing` | `enabled = false` / 非 Windows / 测试：`none()` 的 `sync` 与 `toggle_lock` 都是空操作 |
| `position_within_the_same_line_does_not_repush` | 同一句内推进位置不重画 |
| `a_new_line_pushes_current_and_next` | 换行时当前句/下一句都对 |
| `an_empty_line_shows_only_the_next_one` | 纯音乐段：`current` 空、`next` 有 |
| `nothing_playing_hides_the_overlay` | `Idle`/`Failed` → `visible = false` |
| `pausing_keeps_the_line_unless_configured` | `hide_when_paused` 两种取值 |
| `unlocking_forces_the_overlay_visible` | 解锁时无歌词也可见（含提示语） |
| `the_translation_follows_the_switch` | `show_translation` 开/关 |
| `a_saved_center_is_clamped_onto_the_primary_screen` | 屏外/负坐标夹回 |
| `the_position_file_round_trips` | 写出去再读回来一致（临时目录） |
| `a_broken_position_file_falls_back_to_the_default` | 坏 JSON 当没有 |
| `colors_parse_in_both_cases_and_warn_on_garbage` | `#FFFfff` / `#ff0000` / `red` / `#12345` |
| `opacity_is_clamped_and_warned` | 150 → 100 + 警告 |
| `the_default_config_file_documents_desktop_lyrics` | 模板含全部键 + 默认值 |
| `lock_is_the_sixth_hotkey` | `hotkeys::DEFAULTS` 有 `lock`，且 `LyricsLock` 动作对得上 |

**真机验收（我这台，Windows）**：

1. `enabled = true` 启动 → 窗口出现在主屏底部居中；日志有安装成功的记录；状态行无通知。
2. 播放一首 → 当前句与下一句出现且换行及时（≤ 200ms）；歌词自带翻译时译文跟着显示。
3. **浮在最上面**：把 tuios 全屏化、切到别的应用，歌词仍在上面。
4. **点不着**（锁定态）：在歌词位置上点击/滚动，事件落到下面的窗口（用 tuios 的 pane 验证）。
5. 按 `ctrl+shift+l` → 出现提示框、鼠标能拖；拖到别处松手 → `desktop-lyrics.json` 被写；**重启后回到拖到的位置**。
6. 再按 `ctrl+shift+l` → 记住所有配置键都在（`opacity` 生效、字号随 `font_size` 变、颜色生效）。
7. 暂停 → 默认留着；`hide_when_paused = true` → 隐藏。
8. 退出 canto → 窗口消失、无残留进程；`enabled = false` → 不建窗口（任务管理器里看不到额外窗口，`canto.log` 无相关行）。
9. 把状态文件写成屏幕外的坐标 → 重启后夹回主屏内可见位置。

**不做的**：性能基准（更新频率是「换行才推」，一次绘制是自己的窗口，不测）。

## 9. 改动清单与成本

| 文件 | 改动 |
|---|---|
| `src/overlay.rs` | 新增，约 400–450 行（平台无关状态机 + 位置状态 + Windows 实现 + 约 150 行单测） |
| `src/config.rs` | `[desktop_lyrics]` 段（默认值、颜色/透明度校验、模板、测试），约 +90 行 |
| `src/tui/state.rs`、`src/tui/mod.rs` | 接线（字段、`tick()` 里 sync、位置落盘、启动安装与失败通知），约 +60 行 |
| `src/paths.rs` | `desktop_lyrics_path()` |
| `src/hotkeys.rs` | `DEFAULTS` 加第 6 条 `lock`（连带把「五条」的测试改成六条） |
| `src/tui/keys.rs` | `Action::LyricsLock` + `apply` 分支 |
| `Cargo.toml` | `windows-sys` 补 `Win32_Graphics_Gdi`、`Win32_UI_HiDpi`（仍然只在 Windows 目标上） |
| `README.md`、`docs/design.md` | 功能表一行、配置段、已知限制（全屏独占压不过、只认主屏）、一节实现说明 |

量级与全局热键那次相当（那次 933 行含文档）。

## 10. 风险与已知限制

- **渲染质量是这份设计里唯一「先验了才敢往下」的东西**：`UpdateLayeredWindow` + 灰度 AA + 描边在
  DIB 上的观感只能真看（§8 的探针），不通过就走「半透明背板」退路。
- **DPI 只影响字号**：窗口位置按物理像素存，跨不同 DPI 的显示器拖过去，观感可能偏一档；不做多屏适配。
- **只认主屏**：副屏上的位置会被夹回主屏；多显示器选择明确不做。
- **全屏独占应用之上压不过**：Windows 的既定行为（`WS_EX_TOPMOST` 不等于能盖住独占全屏）。
- **解锁态不持久化**：每次启动都是锁定的；解锁后忘了锁，下次启动自动锁回来（不会「点不到」）。
- **`[hotkeys]` 从 5 条变 6 条**：`HotkeyConfig` 多一个字段、模板多一行、相关测试与断言要跟着改——
  这是这次改动唯一碰到已发布功能的地方（改完 `lock = ""` 就能回到 5 条的行为）。
- **状态文件是新增的第 5 个落盘文件**：小 JSON，跟 `history.json` 同一套容错（坏了当没有）。
  `~/.config/canto/desktop-lyrics.json`（位置是状态不是配置，所以不写进 `config.toml`）。

## 与旧决议的关系

- `2026-10-12-global-hotkeys-design.md`：本 spec 复用它的 `[hotkeys]` 表（加第 6 条）与同一套
  「逐条失败 → 一条通知 + 日志」口径；热键那次的线程模型（Windows 专用线程 + 消息循环 + `Drop` 投
  `WM_QUIT`）在这里被第二次照用，说明这个形状在 Windows 上确实够用。
- `2026-10-11-tui-redesign-design.md`：那次把界面收成「内容区 + 播放条 + 状态行」。桌面歌词不改终端界面
  一寸，是终端之外的第二个显示面。
- 这是项目里**第一次画 GUI**：因此把「取词/状态机」与「窗口/渲染」切成平台无关与平台相关两半，
  macOS 那份以后只需要补后一半。

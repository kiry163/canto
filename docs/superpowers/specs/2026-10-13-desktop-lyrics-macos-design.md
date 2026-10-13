# canto 桌面歌词（macOS 覆盖层）设计

**这一份只做 macOS**。平台无关的那半（取词、状态机、位置状态、`Frame`/`Payload` 契约）已经在
`2026-10-12-desktop-lyrics-design.md` 里做完并上线（Windows 侧）；这里只补
`#[cfg(target_os = "macos")] mod imp`，外加一个「每帧泵」的接口处改动。Linux 仍然不建窗口。

## 背景与目标

Windows 那份现在是唯一实现：非 Windows 走空实现，`[desktop_lyrics]` 整段不生效。目标是让 macOS
上的行为与 Windows 对齐：

- 播放时在桌面最上层浮「当前句 + 译文 + 下一句」，不占终端、**不抢焦点**、不进 Dock；
- 默认**鼠标穿透**（点不着），`ctrl+shift+l`（`[hotkeys]` 第 6 条）解锁后可拖，再按一次锁回去；
- 位置存**中心点**在 `desktop-lyrics.json`（独立状态文件），重启回到原位，换过显示器会被夹回主屏内；
- 关掉功能（`enabled = false`）零成本：不建窗口、`App::tick` 连 `sync` 都不调。

已实测的前提（细节见 §8）：非打包的 CLI 进程能建出真窗口、`setActivationPolicy(Accessory)` 能生效、
窗口能收鼠标也能被拖、全程不激活自己；`screencapture` 与离屏渲染都能出图，验证不用系统授权。

## 非目标（YAGNI，明确不做）

| 不做 | 理由 |
|---|---|
| 多显示器 | 跟 Windows 同一口径（只认主屏 + 夹取）。跨屏要改 `desktop-lyrics.json` 的坐标语义，另起一轮 |
| 自绘文字（Core Text 渲染到位图） | `NSTextField` + `NSAttributedString` 已经够：中文 shaping、描边、换行都交给系统 |
| 窗口库（winit / tao） | 它们没有「泵一次」的 API，`run()` 会独占主线程，跟 crossterm 循环没法共存 |
| 单独的 helper 进程 + IPC | 干净但代价大（第二个二进制、打包/安装说明、payload 序列化、进程生命周期）；留作 §9 的退路 |
| Linux / Wayland | 维持现状：不注册，启动时说一句 |

## 决策记录

| 决策 | 取值 | 理由 |
|---|---|---|
| 进程模型 | **同进程 + 主线程每帧手动泵 AppKit** | 主线程本来就是 TUI 循环；AppKit 要求主线程；不引第二个二进制、不动 `release.yml` |
| 事件怎么来 | `nextEventMatchingMask:untilDate:[NSDate distantPast] inMode:NSDefaultRunLoopMode dequeue:true` + `sendEvent:` | 与刚做完的全局热键泵同一套路（`design.md` 那节有实测）。CFRunLoop 泵不出 AppKit 事件，`NSApp.run()` 会阻塞到退出 |
| 窗口 | 无边框 `NSPanel`（`Borderless | NonactivatingPanel`），`level = Floating`，`canJoinAllSpaces + stationary + fullScreenAuxiliary` | 不抢焦点、不要 Dock 图标、各 space 都在场（探针验过前两条） |
| 文字 | 透明 `NSTextField` + `NSAttributedString`，描边用**负 `strokeWidth`** | 一次画完描边 + 填充（Windows 那边是 8 方向各画一遍再合成）；中文 shaping 由系统负责 |
| 整层透明度 | `setAlphaValue(opacity / 100)` | 等价 Windows 的 `BLENDFUNCTION.SourceConstantAlpha`，同样的语义（连文字一起淡） |
| 拖动 | `isMovableByWindowBackground = true`，解锁态每帧读 `frame()` | 不自己跟鼠标；探针验过窗口跟手、且不激活自己 |
| 坐标系 | 状态文件仍存**左上原点**的中心点；进出 AppKit 时按主屏高换算 | 与 Windows / 状态文件同一口径，`overlay.rs` 那半一行不改 |
| 字体 | 配置字体面认不出来就回退系统字体 | 模板里的默认值 `Microsoft YaHei UI` 在 macOS 不存在（探针里就是回退路径） |
| 每帧泵 | 新增 `Imp::pump()`，`DesktopLyrics::sync` 每帧调一次 | `push` 只在 payload 变化时调，而拖动与重绘要求每帧都泵 |
| 解锁提示 | 只用 payload 里那句 `UNLOCKED_HINT` 文本 | Windows 额外描一圈 1px 白线；macOS 上不写这层代码（提示语已经在 payload 里） |

## 1. 接口与改动面

`overlay.rs` 平台无关那半（`State`/`Frame`/`Payload`/`Center`/`primary_screen`/`clamp_center`/
`DesktopLyrics::{none, install, sync, toggle_lock}`/`Installed.positions`）**一行不改**。

例外只有一处，实施时才发现：`primary_screen()` 的**非 Windows 分支**当年是给空实现占位的
硬编码 `(1920, 1080)`，macOS 要问 `NSScreen.mainScreen` 的真尺寸（否则「默认底部居中」与
「夹回屏内」都按错的屏算——实测默认中心点因此落在 (960, 918) 而不是真实主屏的 (896, 952)）。
所以这个函数从两个 `cfg` 分支变成三个。

| 位置 | 改动 |
|---|---|
| `DesktopLyrics::sync` | 加一句 `self.imp.pump();`（在 `advance` 之前，每帧都跑，与 payload 是否变化无关） |
| `#[cfg(target_os = "macos")] mod imp` | 新实现：`Imp`、`Imp::none`、`install`、`push`、`pump` |
| 其它平台 `mod imp` | `cfg` 收紧成 `not(any(windows, macos))`，加一个空的 `pump`（Linux 那份仍是最终形态） |
| `primary_screen()` | 非 Windows 分支原来是硬编码 `(1920, 1080)` 的占位；macOS 分支改成问 `NSScreen.mainScreen` |

不变式（用类型强约束）：

- **`install` 必须在主线程**。macOS 侧第一句就是 `MainThreadMarker::new()`，拿不到就降级报错
  （`objc2` 的类型系统会替我们把这条守住：`NSApplication::sharedApplication(mtk)`、窗口与视图的
  `alloc(mtk)` 都要那个标记）。
- **测试与 `App::new` 永远走 `none()`**：`install` 只在 `tui::run` 里调（跟 Windows 一个理由——
  测试里真建窗口会污染开发机、也会在 CI 上弹东西）。

## 2. 窗口（macOS）

| 项 | 取值 |
|---|---|
| 类型 | `NSPanel`（`Borderless` + `NonactivatingPanel`） |
| `level` | `Floating`（=3；`CGWindowList` 里实测也是 3） |
| `collectionBehavior` | `CanJoinAllSpaces` + `Stationary` + `FullScreenAuxiliary` |
| 透明 | `opaque = false`、`backgroundColor = clearColor`、`hasShadow = false` |
| 鼠标 | 锁定态 `ignoresMouseEvents = true`，解锁态 `false`；`movableByWindowBackground = true` |
| 激活 | `NSApplication::sharedApplication` + `setActivationPolicy(Accessory)`；窗口用 `orderFrontRegardless()` 显示（不激活、不进 Dock） |
| 可见性 | `push` 的 `visible == false` → `orderOut:`（不销毁窗口，位置与锁定态留着） |
| 尺寸 | 每次 `push` 按三行文本的实测尺寸 + padding 重算（跟 Windows 一样「背板贴字」） |
| 位置 | 存的是左上原点中心点 → AppKit 左下原点：`x = cx − w/2`、`y = 主屏高 − cy − h/2` |

被 `push` 驱动：文本变了就重设三行 label、`sizeToFit` 重新量、按当前中心点摆。内容不变时
`State::advance` 根本不调 `push`（逐帧零绘制、零分配，与 Windows 同一套）。

## 3. 文字与配色

三行，尺寸与配色沿用配置（`[desktop_lyrics]`）：

| 行 | 字号 | 颜色 | 描边 |
|---|---|---|---|
| 当前句 | `font_size` | `color` | 有（`outline`） |
| 译文（`show_translation` 且这首有） | `font_size × 3/5` | `next_color` | 有（`outline`） |
| 下一句 | `font_size × 3/5` | `next_color` | 有（`outline`） |

空的那行不画（与 Windows 一致：`rows` 只收非空行）；每行**水平居中**，行宽取最宽的一行，
高度是各行之和加内边距（Windows 是 8 逻辑像素 × DPI，macOS 直接用 8 点）。

- `font_size` 按**逻辑点**算（Windows 那边是逻辑像素 × DPI/96；macOS 由系统做 2x，
  代码里没有 DPI 乘除）。`3/5` 与 Windows 的 `small_size` 同一比例。
- 描边：`NSStrokeWidthAttributeName` 取**负值**（负 = 填充 + 描边，一次画完），
  `NSStrokeColorAttributeName` 取 `outline`。粗细是观感常数，先取探针里看着对的那组
  （正文 −3.0、次要行 −2.0），真机对比 Windows 的观感再调。
- 整层透明度：`panel.setAlphaValue(opacity as f64 / 100.0)`。
- 字体：`NSFont::fontWithName_size(font, size)` 返回 `None` 就 `NSFont::systemFontOfSize(size)`；
  实际用到哪个字体面写进日志（配置写错了不用猜）。

## 4. 事件泵与拖动

`Imp::pump()` 每帧（`sync` 里）做两件事：

1. **把事件队列拉空并派发**：循环 `nextEventMatchingMask:untilDate:[NSDate distantPast] inMode:
   NSDefaultRunLoopMode dequeue:true`，有事件就 `sendEvent:`，直到返回 `nil`。拖动、重绘、
   窗口 invalidate 全靠这一步——`distantPast` 表示队列空就立刻返回，不阻塞界面。
2. **收拖动结果**：读一次 `panel.frame()`，算出中心点；与**上次报过的**不同就换算回左上原点、
   `positions.send((cx, cy))`。主循环照旧排空这个通道写 `desktop-lyrics.json`。
   （与 Windows 同一口径：每个 draw 都重新采纳窗口**自己**的矩形、只在变化时报一次，不指望
   「拖动结束」事件——那边就是踩过 `WM_EXITSIZEMOVE` 是发送消息、进不了队列这个坑才改成这样的。
   只有解锁态能拖，所以实际上也只有那时会变。）

拖动本身交给系统（`isMovableByWindowBackground`）：探针里合成 `mouseDown` + 12 段 `mouseDragged`
+ `mouseUp`，窗口跟着走，`NSApp.isActive()` 全程 `false`。

## 5. 与 Windows 的行为对齐

| 行为 | 两侧一致的做法 |
|---|---|
| 配置 | 同一份 `[desktop_lyrics]`（`font`/`font_size`/`color`/`outline`/`next_color`/`opacity`/`show_translation`/`hide_when_paused`/`enabled`），解析与校验都在 `config.rs`，平台无关 |
| 启动锁定 | 每次启动都是锁定（`ignoresMouseEvents = true`）；锁定态**不持久化** |
| 默认位置 | 主屏底部居中；`desktop-lyrics.json` 读坏了/没有 → 默认；落在屏外 → 夹回（`clamp_center`，`MARGIN = 40`） |
| 无歌词 / 未播放 | 窗口 `orderOut`（Windows 是 `SW_HIDE`），位置与锁定态留着 |
| 解锁 | 强制可见；没有可比内容时显示 `UNLOCKED_HINT` |
| 热键 | 复用 `[hotkeys].lock`（`ctrl+shift+l`），走同一条 `keys::Action` 派发 |

## 6. 退化与错误处理

| 情形 | 行为 |
|---|---|
| 拿不到主线程标记 / `setActivationPolicy` 失败 / 建窗口失败 | 返回 `Started::error = Some("歌词窗口起不来")`：状态行一句 + `canto.log` 里原因，句柄是 `none()`，**播放与界面照常** |
| 没有窗口服务器连接（比如 ssh 会话） | 同上（探针没验这一条，属于「建不出来就报一句」的同一类） |
| `enabled = false` | 不建窗口；`App::tick` 连 `sync` 都不进（与其它平台一样零成本） |
| 位置文件写坏 / 读不出 | 不阻塞启动，用默认位置（`overlay.rs` 既有逻辑） |

## 7. 验证

**第一层（`cargo test`）**：状态机那 12 条既有测试不动（本来就平台无关）；macOS 的窗口/文字不写单测
——跟 Windows 一个口径：这一半只能真机看。`Imp::pump` 为空实现的那条路径（`none()` 的 `sync`）
已经有测试覆盖（`a_none_handle_syncs_nothing`），macOS 上也要保持能过。

**第二层（tuios 真机，`target/debug/canto --log debug` + 真播放）**：

| # | 检查 | 手段 |
|---|---|---|
| 1 | 三行字真的画出来了（中文 + 描边 + 位置） | `screencapture` 截那块区域 → 看 PNG（本机有权限） |
| 2 | 窗口的属性对：`level=3`、`alpha≈opacity/100`、bounds 随文本长度变 | `CGWindowListCopyWindowInfo`（探针里的 `windows` 子命令） |
| 3 | 暂停（`hide_when_paused = true`）窗口消失、继续回来 | 同上（列表里有没有那条窗口） |
| 4 | 解锁 → 提示语出现；拖动跟手；松手后 `desktop-lyrics.json` 更新；重启回原位 | 热键注入 + 探针的 `drag` 合成拖动 + 读状态文件 |
| 5 | 锁定态点不着、不抢焦点 | 探针 `click` + 探针窗口当观察者；另用**别的 app 的窗口**确认点击落到了下面那个 app（探针里观察不到，只有自己的窗口） |
| 6 | 压在别的 app 的全屏之上 / 切 space 还在 | 真机看（`canJoinAllSpaces` + `FullScreenAuxiliary`） |
| 7 | `enabled = false` 时进程里没有那个 level 3 的窗口 | `CGWindowList` |
| 8 | 退出 canto 窗口随之消失 | 同上 + `ps` |
| 9 | 热键 `ctrl+shift+d` 关掉再开**连按 8 轮**：每轮窗口可真下屏、再建成功、位置一点不飘（`ctrl+shift+l` 连按 4 次同理，拖动后重启回原位） | 真机 + `CGWindowList` 探针 + 读状态文件 + 日志里「桌面歌词窗口已就绪」的条数 |

**实测结果（2026-10-13，macOS 15.5 / x86_64，tuios 里真二进制 + 真播放）：**

| # | 结果 |
|---|---|
| 1 | ✅ `screencapture` 出图：当前句（大、白、带黑描边）+ 下一句（小、灰）水平居中，中文不是方框 |
| 2 | ✅ `layer=3`、`alpha=0.90`、中心点 (896, 952) = 真实主屏 1792×1120 的底部居中；宽度随文本变（361 ↔ 237 ↔ 438） |
| 3 | ✅ `hide_when_paused = false` 时暂停留着；改成 `true` 后暂停消失、继续播放又回来 |
| 4 | ✅ 解锁后合成拖动跟手（(715,918) → (1038,769)）；`desktop-lyrics.json` 写 (1116,806)，与窗口实际中心 (1115,804) 差 1 点；锁回去再重启回到 (1115,807) |
| 5 | ⚠️ 一半：锁定态「点不着」✅（合成拖动窗口纹丝不动、应用也不激活）；但「点击落到下面那个 app」没测到——探针自己的窗口（任何 level、跨进程）都收不到穿透过去的点击，落到谁身上观察不到。留一条人工确认 |
| 6 | ❌ 未验：本机没有可切全屏的应用（不想动使用者的桌面）。`CanJoinAllSpaces` + `FullScreenAuxiliary` 已设，但没有实测 |
| 7 | ✅ `enabled = false` 时日志里连「桌面歌词窗口已就绪」都没有（`install` 根本没进），进程里也没有 level 3 的窗口 |
| 8 | ✅ `q` 退出后窗口消失、桌面无残留（截图确认） |
| 9 | ✅（2026-10-13 补测，见下）8 轮开关每轮都下屏/再建成功，`bounds.y` 与 `cy` 全程不变；`ctrl+shift+l` 4 次同理；合成拖动 (896,956) → (746,876) 后状态文件写 (746,877)，重启回到该处 |

第 9 条是补上的：原清单没有「关掉再开」这一格，第 8 条只覆盖进程退出（进程退出本来就什么都不剩），
所以下面这三个只在建→拆→再建里露头的 bug 一次都没被跑到。

后来抓到三个真机才会暴露的实现 bug（细节与实测数据见 `docs/design.md`「开关两次才发现的三个坑」）：
1. **关窗口没真下屏**：macOS `Imp` 只靠 `Retained<NSPanel>` drop 的 `release`，而上屏期间 AppKit
   自己持有窗口——窗口留在屏上，`is_running()` 却已是 `false`（`sync` 早退，歌词冻在最后一句）。
   修法：`Drop for Inner` 里显式 `close()`，建窗口时 `setReleasedWhenClosed(false)`。
2. **再开必失败**：`setActivationPolicy(Accessory)` 是幂等 setter，第二次起返回 `false`，原代码当
   失败处理 → 每次重开都在建面板之前报「歌词窗口起不来」。修法：先查 `activationPolicy()`。
3. **位置自己往下飘**：`setFrame` 把原点吸附到整点，量回来的中心带 ±0.5 舍入被写回状态文件，
   每次开关累积 +1~2 点（8 轮实测 961 → 972）。修法：重画按记着的整数中心摆，只有解锁态才量
   矩形，且差过 1 点才算拖动。

实施时抓到的一个同类真机 bug：`setIgnoresMouseEvents` 的极性写反（写成 `!payload.locked`），
症状是「锁定态点得着、解锁后拖不动」——修法就是传 `locked` 本身。教训：第 5 条这种**双向**判据
（锁定不跟手 + 解锁跟手）才是能抓住极性错误的形式，单测一边抓不住；第 9 条同理——「关掉再开」
这种**往返**判据才抓得住上面那三个。

## 8. 实测依据（探针，2026-10-13）

> 探针是 `/tmp/lyricsprobe`（一次性，不进仓库）：同一个二进制分 `show`/`shot`/`windows`/`click`/
> `drag`/`screens` 几个子命令，用 `objc2` + `objc2-app-kit`（这两个 macOS 上**已经在依赖图里**，
> 是 `global-hotkey` 拖进来的，直接依赖不新增下载）。
>
> 实施时量到的实情：把 AppKit 那几族 feature 打开后，`Cargo.lock` 会多出 8 个 objc2 家族的
> **纯 Rust 绑定 crate**（`objc2-cloud-kit` / `objc2-core-data` / `objc2-core-graphics` /
> `objc2-core-image` / `objc2-core-text` / `objc2-core-video` / `objc2-io-surface` /
> `objc2-quartz-core`，都是 `objc2-app-kit` 可选依赖被我启用的 feature 拉进来的）。本机缓存里
> 都有（`--offline --locked` 能编），CI 上按常规从 crates.io 取；没有 C 代码要编。

| 验的 | 结果 |
|---|---|
| 建窗口 / 激活策略 | `setActivationPolicy(Accessory) -> true`，readback `Accessory`；`CGWindowList` 里 `layer=3/2`、`alpha=1.00`，bounds 与 AppKit frame 逐像素对上 |
| 不抢焦点 | 全程 `app_active=false`；前台 app 一直是 ghostty（点击、拖动、泵都如此） |
| 锁定态 | `ignoresMouseEvents=true`：窗口收不到任何鼠标事件，也不激活 |
| 解锁态 | 收到 `mouseDown`/`mouseDragged`/`mouseUp`；frame `300,384 → 400,370 → 463,370` 跟着走 |
| 坐标换算 | `NSScreen` 1792×1120 @2x（左下原点）；`y = 屏高 − 左上y` 换算后与 `CGWindowList` 的 bounds 一致 |
| 字体回退 | `Microsoft YaHei UI` 认不出 → `.AppleSystemUIFont`；中文正常 |
| 描边 | 负 `strokeWidth`（−3.0 / −2.0）一次画完，中文带描边渲染正确 |
| 离屏出图 | `bitmapImageRepForCachingDisplayInRect` + `cacheDisplayInRect:toBitmapImageRep:` 不需要任何授权就能出 PNG |
| 泵开销 | 150ms 泵一次，CPU 0.0% |
| 全屏 space / 多屏 | 未验（本机一块屏、没开全屏应用）→ §7 第 6 条 |

两个坑记下来（写实现时别重踩）：

- **`Retained<T>` 不是 `repr(transparent)`**：`&retained` 不是对象指针，直接当 `AnyObject` 指针塞进
  `NSDictionary::from_slices` 会段错误（第一版探针就这么崩的）。要么 `&*retained`，要么用
  `Retained::as_ref()`。
- **锁定态下点击不会落到我们自己的另一个窗口**：`ignoresMouseEvents` 开着时，同一个 rect 下面的
  另一个浮层窗口（level 0/1/2、跨进程都试过）一个鼠标事件都收不到。落在谁身上（别的 app？桌面？）
  探针观察不到——所以「锁定态点不着、不抢焦点」是实测结论，「点击确实穿到下面的 app」留给 §7 第 5 条。

## 9. 风险与已知限制

- **手动泵 AppKit 不是官方推荐的入口**。非 `NSApplicationMain` 的进程这么干是老做法（CLI 工具常见），
  但这意味着 AppKit 的某些机制（模态、菜单、`NSApp.run` 相关语义）我们没走标准路径。若真机上出现
  「不重绘 / 事件丢」这类症状，退路是**单独的 helper 进程 + IPC**（helper 自己 `NSApp.run()`，
  正统姿势；见「非目标」表最后一行）：§1 的接口面不变，只换 `imp` 的实现与打包方式。
- **全屏应用之上、多显示器**：前者真机验（§7 第 6 条）；后者不做（与 Windows 一致）。
- **屏幕录制权限**：真截图 `screencapture` 要（本机已有）；离屏渲染不要——E2E 里两个都能用，
  实在没权限就退到离屏 PNG。
- **`font_size` 的观感**：逻辑点与 Windows 的逻辑像素在 2x 屏上视觉接近但不完全等价，
  真机上跟 Windows 对比着定；描边粗细同理。
- **解锁态在「有歌词」时没有额外视觉标记**：Windows 解锁时会描一圈 1px 白线，macOS 这一版只有
  payload 里那句 `UNLOCKED_HINT`（没内容时才显示）。真机试用后若觉得不够显眼，再补一圈边框
  （`NSView` 的 layer border 或 `NSBox`）——不影响接口面。

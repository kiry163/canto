# canto — 开发约定

## 平台

三平台都能跑（macOS / Linux / Windows）。下面所有命令都以**仓库根目录**为基准、走 POSIX 写法
（PowerShell 同样吃 `--flag` 与正斜杠路径）：

| | 仓库根目录 | 二进制 |
|---|---|---|
| macOS / Linux | 例如 `/Users/kiry/code/canto` | `target/debug/canto` |
| Windows | 例如 `G:/code/canto` | `target/debug/canto.exe` |

凡是跟平台有关、又该由 tuios 自己说了算的路径（配置文件、截图字体），先问它——`tuios config path`、
`tuios config files`，别在心里拼 `~/.config` / `~/Library/Application Support` / `%LOCALAPPDATA%`。

## 验证分两层

- **精确断言**：`cargo test`。布局 / 措辞 / 尺寸 / 边界走 `tui/draw` 的 `TestBackend` + `frame_flat`
  （任意尺寸、逐字符），键位走 `keys::press`（纯函数），状态机走 `test_support` 的哑引擎。
  不需要终端，CI 也跑。
- **端到端**：在 tuios 里跑真二进制（真登录态、真网络、真按键）。文字断言用 `capture-pane`，
  颜色 / 布局看 `tuios screenshot`。
- 尺寸矩阵（窄屏 <100 列 / 矮屏 <20 行 / 过小窗口）归第一层；tuios 那层只管「真程序真的这样反应」。
- 全局热键也归 tuios 那层（要按出真按键，且 canto **不在前台**才算数）。macOS 上实测能触发的是
  `osascript -e 'tell application "System Events" to key code 49 using {control down, shift down}'`
  （49 = 空格，126/125/124/123 = 上/下/右/左；发起它的进程要有辅助功能授权，没授权的话 osascript
  那边会报 -1719）。Windows 上用什么触发没验过，不写。

## tuios 冒烟（session `canto-dev`）

窗口按任务命名 `canto-<任务>`。TUI 直接作为窗口进程跑，不经过 shell：

```sh
tuios new-window -s canto-dev canto-x --cwd <仓库根目录> --no-focus -- <二进制> --log debug
tuios wait-for window-output -s canto-dev -w canto-x --pattern '在线收藏' --timeout 20000
tuios capture-pane         -s canto-dev -w canto-x          # 全屏程序不加 --lines
tuios send-keys            -s canto-dev -w canto-x '?'
tuios capture-pane         -s canto-dev -w canto-x
tuios send-keys            -s canto-dev -w canto-x Escape    # 浮层开着时 q 会被浮层吞掉
tuios send-keys            -s canto-dev -w canto-x q
tuios wait-for window-exit -s canto-dev -w canto-x --timeout 10000
tuios close-window         -s canto-dev canto-x              # 程序退出后窗口不会自己关
```

`<仓库根目录>` 与 `<二进制>` 按上面那张表替换（写绝对路径，别写 `~`）。
第二条是「等界面起来」的写法：`在线收藏`（首页第一格）那串出来 = 界面画起来了——登录与否都会出现，
所以它只证「起来了」，要等真拉完列表得换别的字符串。**别用 `wait-for window-idle`**，见「坑」。

命令类工作用 `tuios run -s canto-dev -w <窗口> -- cargo test`（等它跑完并拿退出码）——前提是那个 pane 的
shell 会发 OSC 133 标记：fish 4+ 自带，zsh / bash 4.4+ 要在启动文件里加几行（`tuios doctor shell` 把
那几行打出来）。没配标记的 pane 会被它以 `no_shell_integration` 拒掉、一个字都不输入；这时退回
`send-text` + `wait-for window-output --pattern <命令自己打印的标记>`。
需要交互式 shell 时用 `send-text` + `send-keys Enter`，**不要**把回车写进 `send-text`。

## 坑（都实测过）

- `send-text` 里带结尾换行会被 **Windows** 的 PowerShell PSReadLine 当成**多行续行**（提示符变 `>>`）：
  文本和 `send-keys Enter` 分开送（macOS / Linux 的 bash / zsh / fish 没这问题，但分开送两边都通用）。
  TUI 走上面的 argv 方式就绕过了这一整类问题。
- **别用 `wait-for window-idle` 等 canto 的界面**：它一直在往终端写东西，永远等不到「安静 N 毫秒」
  （实测：停稳之后等 2 秒也等不到；同一台机器上普通 shell 窗口 1 秒就到）。改等一段**一定会出现的
  输出**——`tuios wait-for window-output -w canto-x --pattern '<文案>'`；或者在 `q` 之后等
  `tuios wait-for window-exit -w canto-x`，比 `sleep` 稳。其它条件见 `tuios wait-for --help`。
- `capture-pane --lines N` 从最后一行有内容处往上数，**会切掉全屏程序的顶部**：全屏不加 `--lines`。
- `tuios screenshot -o` 的**相对路径按目标窗口的 cwd 解析**：一律给绝对路径。
- 中文场景只用 PNG：SVG / HTML 会内嵌字体（CJK 字体下实测 13 MB）。截图前先确认字体配好了：
  用 `tuios config path` 打开配置，看 `[screenshot] font_file` 是否指着一份带 CJK + Nerd 图标的字体
  （Windows 开发机上指 `%LOCALAPPDATA%\tuios\fonts\SarasaTermSCNerd-Regular.ttf`；macOS 这台还是空的，
  中文会渲染成方框）。没配的话只打印一行 `The font file could not be read` 就静默降级。
- 窗口尺寸 = 会话尺寸：detached 会话默认 80×24，`tuios attach` 一次就把它改成客户端尺寸，
  **detach 之后保持**（实测）。没有 CLI 开关能直接改尺寸（控制协议有 `resize` 动词，但 `tuios`
  没有对应命令）；其它尺寸的矩阵本来归第一层 `TestBackend`。两台开发机的实测值：Windows 210×42、
  macOS 208×55——开测前对一下真机的尺寸，别当它是某个固定数。
- 浮层开着时 `q` 会被浮层吞掉（`?` 帮助、`u` 队列都一样）：先 `Escape` 再 `q`。
- argv 方式起的窗口，程序退出后**窗口不会自己关**（实测：canto 已退出、窗口留着空屏），收尾一律 `close-window`。

## 单例与声音

- 数据目录写死 `~/.config/canto`（三平台一样），锁是 `canto.lock`：起测试实例前若被占，直接关掉占锁的那个实例。
- 声音不用管，该播放就播放。

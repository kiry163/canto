# canto — 开发约定

## 验证分两层

- **精确断言**：`cargo test`。布局 / 措辞 / 尺寸 / 边界走 `tui/draw` 的 `TestBackend` + `frame_flat`
  （任意尺寸、逐字符），键位走 `keys::press`（纯函数），状态机走 `test_support` 的哑引擎。
  不需要终端，CI 也跑。
- **端到端**：在 tuios 里跑真二进制（真登录态、真网络、真按键）。文字断言用 `capture-pane`，
  颜色 / 布局看 `tuios screenshot`。
- 尺寸矩阵（窄屏 <100 列 / 矮屏 <20 行 / 过小窗口）归第一层；tuios 那层只管「真程序真的这样反应」。

## tuios 冒烟（session `canto-dev`）

窗口按任务命名 `canto-<任务>`。TUI 直接作为窗口进程跑，不经过 shell：

```sh
tuios new-window -s canto-dev canto-x --cwd G:/code/canto --no-focus -- target/debug/canto.exe --log debug
tuios wait-for window-idle -s canto-dev -w canto-x --idle 1500
tuios capture-pane         -s canto-dev -w canto-x          # 全屏程序不加 --lines
tuios send-keys            -s canto-dev -w canto-x '?'
tuios capture-pane         -s canto-dev -w canto-x
tuios send-keys            -s canto-dev -w canto-x Escape    # 浮层开着时 q 会被浮层吞掉
tuios send-keys            -s canto-dev -w canto-x q
tuios close-window         -s canto-dev canto-x              # 程序退出后窗口不会自己关
```

命令类工作用 `tuios run -s canto-dev -w <窗口> -- cargo test`（等它跑完并拿退出码）。
需要交互式 shell 时用 `send-text` + `send-keys Enter`，**不要**把回车写进 `send-text`。

## 坑（都实测过）

- `send-text` 里带结尾换行会被 PowerShell 的 PSReadLine 当成**多行续行**（提示符变 `>>`）：
  文本和 `send-keys Enter` 分开送。TUI 走上面的 argv 方式就绕过了这一整类问题。
- `capture-pane --lines N` 从最后一行有内容处往上数，**会切掉全屏程序的顶部**：全屏不加 `--lines`。
- `tuios screenshot -o` 的**相对路径按目标窗口的 cwd 解析**：一律给绝对路径。
- 中文场景只用 PNG：SVG / HTML 会内嵌字体（CJK 字体下实测 13 MB）。截图字体已配：
  `screenshot.font_file` → `%LOCALAPPDATA%\tuios\fonts\SarasaTermSCNerd-Regular.ttf`；
  没配的话中文是方框、Nerd 图标缺失，而且只打印一行 `The font file could not be read` 就静默降级。
- `canto-dev` 的窗口是 **210×42**（= 真机尺寸）。detached 会话默认 80×24；`tuios attach canto-dev`
  一次就会把它改成客户端尺寸，**detach 之后保持**（实测）。没有 CLI 开关能直接改尺寸
  （控制协议有 `resize` 动词，但 `tuios` 没有对应命令）；其它尺寸的矩阵本来归第一层 `TestBackend`。
- 浮层开着时 `q` 会被浮层吞掉（`?` 帮助、`u` 队列都一样）：先 `Escape` 再 `q`。
- argv 方式起的窗口，程序退出后**窗口不会自己关**（实测：canto 已退出、窗口留着空屏），收尾一律 `close-window`。

## 单例与声音

- 数据目录写死 `~/.config/canto`，锁是 `canto.lock`：起测试实例前若被占，直接关掉占锁的那个实例。
- 声音不用管，该播放就播放。

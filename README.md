# canto

自用的 QQ 音乐播放器（Rust）。先把**数据链路**跑通——登录、歌单、曲目——再往上加播放与终端界面。

## 进度

| 能力 | 状态 |
|---|---|
| 登录：粘贴 cookie / **扫码**（二维码画在终端里） | ✅ |
| 我的歌单 / 我喜欢 / 歌单曲目（含音质档位） | ✅ |
| 播放：vkey 取流 → 边下边解码出声 | ✅ |
| 终端界面（歌单/曲目/播放条/歌词） | ✅ |
| 歌词（行级 + 翻译，跟随播放高亮） | ✅ |
| 搜索（界面 `/`，结果直接可播） | ✅ |
| 下载（歌曲 + 同名歌词/翻译） | ✅ |
| 歌单写入：收藏 / 加进歌单 / 从歌单移出 | ✅（「取消收藏」见「已知限制」） |

## 用法

```bash
cargo run -- login               # 扫码登录：二维码画在终端里（装不下就存成 PNG 文件）
cargo run                        # 直接进终端界面（默认命令）
cargo run -- --help              # 子命令仍是排查入口
```

界面：

```
┌ 歌单 ────────────┐┌ 嗯（7675761234） ──────────────────────────┐
│♥   488  我喜欢   ││  04:36  FLAC    山水之间 — 许嵩            │
│     197  嗯       ││▶ 01:53  FLAC    02:52 非酋 — 薛明媛、朱贺  │
│                  │└────────────────────────────────────────────┘
│                  │┌ 歌词（50 行） ─────────────────────────────┐
│                  ││把我变成世上最笨的女人                     │
│                  ││他说的每句话我都会当真   ← 当前行高亮       │
└──────────────────┘└────────────────────────────────────────────┘
┌ 播放 ──────────────────────────────────────────────────────────┐
│ ♪ 非酋 — 薛明媛、朱贺  [无损] 播放中  2/195 · 音量 100%        │
│ ███████████████░░░░░░░░░░░░░░░░░░  01:32 / 02:52              │
└────────────────────────────────────────────────────────────────┘
Enter 播放 · Space 暂停 · n/b 切歌 · f 收藏 · a 加歌单 · x 移除 · Tab 切栏 · q 退出
```

| 键 | 作用 |
|---|---|
| `q` / `Ctrl-C` | 退出 |
| `Tab` | 左右栏切换焦点（当前栏边框变黄） |
| `j` `k` / `↑` `↓` | 移动选择 |
| `g` / `G` | 跳到列表首 / 尾 |
| `Enter` | 左栏：加载该歌单并跳到右栏；右栏：播放该首 |
| `Space` | 播放 / 暂停 |
| `n` / `b` | 下一首 / 上一首 |
| `←` / `→`（或 `h` / `l`） | 后退 / 前进 10 秒 |
| `+` / `-` | 音量 ±5% |
| `y` | 开/关歌词面板（当前行高亮，翻译跟在本行下面） |
| `/` | 搜索（回车搜、Esc 取消；结果进曲目栏，直接回车播放） |
| `d` | 下载选中的那首（含同名歌词），进度在状态栏 |
| `f` | 收藏选中的那首（写进「我喜欢」） |
| `a` | 把选中的那首加进某个歌单（弹出歌单列表，`j`/`k` 选，回车确认） |
| `x` | 从右栏当前歌单里移出选中的那首（移完自动重拉列表；「我喜欢」不行，见「已知限制」） |

**队列语义**：按下播放那一刻的曲目列表就是队列（换歌单不影响它），一首播完自动接下一首；
右栏里 `♪` 标的是正在播的那首。每首播放都会从「立体声优先」的档位链里挑第一个能解码的。
队列里没版权的歌（实测常见）会自动跳过并在状态栏说明，连着 3 首都不可播就停下报错。

界面模式下日志写文件（`~/Library/Application Support/canto/canto.log`）——屏幕归界面，
stdout/stderr 打印会糊屏；`cargo run -- -l info` 可以把播放细节记进去。

## 命令行子命令

```bash
# 扫码登录（推荐）：手机 QQ 扫终端里的二维码
cargo run -- login

# 或者粘贴 cookie：浏览器打开 y.qq.com，DevTools → Network → 任一请求 → Request Headers → Cookie，
# 复制整串（至少要含 uin 和 qqmusic_key）
cargo run -- login --cookie 'uin=o0123456789; qqmusic_key=…; login_type=1'
# 不想留在 shell 历史里：
pbpaste | cargo run -- login --cookie -

cargo run -- whoami              # 会话状态
cargo run -- playlists           # 我的歌单
cargo run -- favorites           # 我喜欢
cargo run -- tracks 7412851234   # 某个歌单的曲目（id 用 playlists 打出来的）
cargo run -- play 7412851234 3   # 播这个歌单的第 3 首（命令行调试用）
cargo run -- play favorites 1 --quality hires   # 指定音质
cargo run -- lyric favorites 1   # 打印歌词（带时间戳与翻译，排查用）
cargo run -- search "晴天 周杰伦" --limit 5   # 搜索
cargo run -- download favorites 1 --dir ~/Music   # 下载（默认 <音乐目录>/canto）
cargo run -- favorite favorites 1            # 收藏（歌从哪个歌单里挑、序号从 1 开始）
cargo run -- add 7412851234 favorites 1      # 把「我喜欢」第 1 首加进歌单 7412851234
cargo run -- remove 7412851234 favorites 1   # 从歌单 7412851234 移出那一首
cargo run -- logout              # 清除本地会话
```

会话存在 `<系统配置目录>/canto/session.json`（macOS：`~/Library/Application Support/canto/session.json`）。
cookie 就是账号凭据：**别提交、别分享这个文件**。

## 音质档位（实测，2026-10-09）

同一首歌在 CDN 上有一组候选文件。`canto` 按「立体声优先、音质降级」排队，逐个**真去开**，
第一个能解码的胜出——因为服务端对不存在/无版权的文件**也会**返回 `purl` + `vkey`，
只有真去取才 404（`file not exist`），光看接口结果判断不出来。

| 前缀 | 实测规格 | 体积（4 分钟量级） | 用途 |
|---|---|---|---|
| `AI00` | flac 192kHz/24bit 立体声 | ~150MB | 母带，仅 `--quality hires` |
| `F000` | flac 44.1k/16bit 立体声（个别 48k/24bit） | ~25MB | **默认首选** |
| `Q000` | flac 44.1k/16bit 立体声 | ~25MB | 默认次选 |
| `M800` | mp3 320k 立体声 | ~9MB | 无损不可用时 |
| `M500` | mp3 128k 立体声 | ~3.5MB | 兜底 |
| `Q001` | flac 44.1k/16bit **6 声道** | ~62MB | 5.1 环绕，排在 mp3 之后 |
| `O801` | vorbis 44.1k **4 声道** | ~19MB | 环绕，最后兜底 |

`--quality` 语义是「从这一档往下找」：`hires`（母带）→ `flac`（默认）→ `320` → `128`。
环绕声版本排在立体声之后，是因为它们只会经过 rodio 的降混，白白多花带宽。


## 已知限制

- **「取消收藏」接口走不通**：收藏能写进去，但把歌**从「我喜欢」移出**恒回 `80105`
  （`DelSonglist` + `dirId=201`，换 `tid`、换 `songType`、换 `tid=0` 都一样），
  暂时只能去官方客户端取消收藏。普通歌单的加/删都正常。
- 经典写入接口 `fcg_music_add2songdir.fcg` 现在对任何会话都回 `{"code":403,"title":"no permit"}`
  ——**与登录方式无关**（扫码会话照样拒绝），`g_tk=5381` 与按各 cookie 算的 hash33 都不行。
  写歌单走 `musicu.fcg` 的 `music.musicasset.PlaylistDetailWrite`（见 `qq/write.rs`）。
  它是否也接受「粘贴 cookie」的会话**未实测**（手头只有扫码会话：55 个 cookie、含 `p_skey`）。
- 搜索接口不返回 hires 大小，所以搜索结果的音质标签可能低估（不影响取流，
  候选链是固定的）。
- 下载的 `hires` 档位体积很大（单曲约 150MB），默认仍是「立体声无损优先」。

## 设计要点

- **域名感知的 cookie jar**（`cookie_store`），不是扁平 map。QQ 服务端会用
  `Set-Cookie: p_skey=` 明确清掉某个**域**上的旧凭据，扁平存储会把别的域的同名
  cookie 一起覆盖——voicefox 的扫码换 code 就栽在这上面。请求头由 reqwest 按 URL 自动拼。
- 粘贴来的 cookie 统一挂 `.qq.com` 并给 30 天过期时间。不这样做它只是**会话 cookie**，
  序列化时会被静默跳过，登录跨不了进程；`session.rs` 里有测试钉住这条。
- 只落模型需要的字段，接口返回的其余字段一律不存。
- 接口路径、`musicu.fcg` 的模块名、UA 都来自实测（voicefox 的 tx 音源跑通同一套接口），
  不是照文档猜的；改一个字符就可能 404。
- **边下边播**：`media.rs` 把 HTTP 响应做成 `Read + Seek`（rodio 的解码器要 `Seek`）。
  CDN 支持 `Accept-Ranges: bytes`，所以往回 seek 就重开一次 `Range` 请求，
  往前 seek 就近读掉——正常顺序播放只有一次请求，不落盘。
- 声道数与采样率交给 rodio 的 mixer 归一（它会做 6 声道→立体声、192kHz→设备采样率），
  但**选档时仍然立体声优先**：多声道版本只会被降混，白多花带宽。
- 开流时校验**魔数**（`fLaC`/`OggS`/`ID3`/`ftyp`…）：CDN 的 `Content-Type` 不可信
  （实测 FLAC 被标成 `audio/x-ogg`、错误页标成 `audio/mpeg`），错误页正文不该被
  解码器当成音频播出去。
- 下载复用播放用的 `MediaStream`（同一套魔数嗅探与 `Range` 逻辑）：先写 `.part`
  再改名，中途失败不会留下坏文件；同名 `.lrc`（有翻译再写 `.trans.lrc`）一起落盘。
- 扫码登录的每一跳都靠会话 jar 自动收 `Set-Cookie`；跟跳转链用的是**不自动跟随**的
  客户端（`http::client_without_redirect`）——自动跟随时 reqwest 把 302 吃掉，
  `Location` 与中间各跳（`p_skey`/`skey`）的 cookie 全丢，这正是 voicefox 当初
  「扫码成功但拿不到音乐凭证」的结构性原因。
- 歌单写入也走 `musicu.fcg`：`music.musicasset.PlaylistDetailWrite` 的 `AddSonglist` / `DelSonglist`，
  参数里 `songId` 必须是**数字 id**（mid 不行），「我喜欢」是 `dirId=201`（`tid` 给 0 就行）。
  实测服务端码：`0` 成功（重复加/删同样返回 0，幂等）、`80092` 歌单不存在或无权、
  `40000` 方法不存在、`80105` 拒绝对「我喜欢」做移除。
- 歌词走 `musicu.fcg` 的 `PlayLyricInfo`，参数里 **`crypt=0` + `qrc=0`** 时返回的是
  明文 base64 的 LRC（正文 `lyric`、翻译 `trans`）。传 `qrc=1` 会换成 QQ 自己的加密
  QRC（hex 密文），要多一整套解密——行级歌词不需要它。
- `--log` 传裸级别（如 `debug`）时只作用于 canto 自己，其它库保持 `warn`
  ——否则 reqwest/h2 会把每条连接、每个 HTTP/2 帧都打出来，把你的日志淹掉。

## 代码结构

```
src/
  cli.rs      命令行定义
  config.rs   会话文件路径
  http.rs     HTTP 客户端（UA / 超时 / 挂会话 jar）
  session.rs  登录态：cookie jar、完整性校验、落盘
  model.rs    Song / Playlist / Quality
  media.rs    HTTP 流 → Read + Seek（Range 重连）
  player.rs   候选档位 → rodio 解码 → 默认音频设备
  qq/
    mod.rs    接口：个人歌单、我喜欢、歌单曲目
    vkey.rs   取播放地址与候选档位顺序
    cookie.rs 粘贴 cookie 的解析
    parse.rs  响应 JSON → 模型
    write.rs  歌单写入：收藏 / 加歌 / 删歌
```

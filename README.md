# canto

自用的 QQ 音乐播放器（Rust）。先把**数据链路**跑通——登录、歌单、曲目——再往上加播放与终端界面。

## 进度

| 能力 | 状态 |
|---|---|
| 粘贴 cookie 登录（域名感知会话，跨进程保留） | ✅ |
| 我的歌单 / 我喜欢 / 歌单曲目（含音质档位） | ✅ |
| 播放（取 vkey + 流式解码） | 未开始（M2） |
| 终端界面（ratatui） | 未开始（M3） |
| 下载 / 歌词 / 歌单写入 | 未开始（M4） |

## 用法

```bash
# 登录：浏览器打开 y.qq.com，DevTools → Network → 任一请求 → Request Headers → Cookie，
# 复制整串（至少要含 uin 和 qqmusic_key）
cargo run -- login --cookie 'uin=o0123456789; qqmusic_key=…; login_type=1'
# 不想留在 shell 历史里：
pbpaste | cargo run -- login --cookie -

cargo run -- whoami              # 会话状态
cargo run -- playlists           # 我的歌单
cargo run -- favorites           # 我喜欢
cargo run -- tracks 7412851234   # 某个歌单的曲目（id 用 playlists 打出来的）
cargo run -- logout              # 清除本地会话
```

会话存在 `<系统配置目录>/canto/session.json`（macOS：`~/Library/Application Support/canto/session.json`）。
cookie 就是账号凭据：**别提交、别分享这个文件**。

## 设计要点

- **域名感知的 cookie jar**（`cookie_store`），不是扁平 map。QQ 服务端会用
  `Set-Cookie: p_skey=` 明确清掉某个**域**上的旧凭据，扁平存储会把别的域的同名
  cookie 一起覆盖——voicefox 的扫码换 code 就栽在这上面。请求头由 reqwest 按 URL 自动拼。
- 粘贴来的 cookie 统一挂 `.qq.com` 并给 30 天过期时间。不这样做它只是**会话 cookie**，
  序列化时会被静默跳过，登录跨不了进程；`session.rs` 里有测试钉住这条。
- 只落模型需要的字段，接口返回的其余字段一律不存。
- 接口路径、`musicu.fcg` 的模块名、UA 都来自实测（voicefox 的 tx 音源跑通同一套接口），
  不是照文档猜的；改一个字符就可能 404。

## 代码结构

```
src/
  cli.rs      命令行定义
  config.rs   会话文件路径
  http.rs     HTTP 客户端（UA / 超时 / 挂会话 jar）
  session.rs  登录态：cookie jar、完整性校验、落盘
  model.rs    Song / Playlist
  qq/
    mod.rs    接口：个人歌单、我喜欢、歌单曲目
    cookie.rs 粘贴 cookie 的解析
    parse.rs  响应 JSON → 模型
```

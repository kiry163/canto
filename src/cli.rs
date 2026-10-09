//! 命令行入口。
//!
//! M1 是"先命令行跑通数据链路"，TUI 在 M1 第 4 步接上；这些子命令
//! 之后仍然是排查问题的入口（TUI 里出问题直接命令行复现）。

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "canto",
    version,
    about = "canto —— 自用的 QQ 音乐播放器（M1：登录与歌单数据链路）"
)]
pub struct Cli {
    /// 日志级别：error / warn / info / debug / trace
    #[arg(short, long, default_value = "warn", global = true)]
    pub log: String,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// 用浏览器里复制的 cookie 登录
    Login {
        /// cookie 文本；传 `-` 表示从标准输入读（避免留在 shell 历史里）
        #[arg(long, value_name = "COOKIE")]
        cookie: String,
    },
    /// 查看当前会话状态
    Whoami,
    /// 打印我的歌单
    Playlists,
    /// 打印「我喜欢」
    Favorites,
    /// 打印某个歌单的曲目（`favorites`，或 `playlists` 里的 id）
    Tracks {
        /// 歌单 id，或 `favorites`
        id: String,
    },
    /// 播放某个歌单里的第 N 首
    Play {
        /// 歌单 id，或 `favorites`
        id: String,
        /// 第几首，从 1 开始（用 `tracks` 看到的序号）
        index: usize,
        /// 音质：hires / flac / 320 / 128；默认无损立体声优先
        #[arg(long)]
        quality: Option<String>,
    },
    /// 清除本地会话
    Logout,
}

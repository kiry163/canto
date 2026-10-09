//! canto —— 自用的 QQ 音乐播放器。
//!
//! 当前是 M1：登录（粘贴浏览器 cookie）与歌单数据链路，先命令行跑通。
//! 播放与 TUI 在后续里程碑接上，命令行子命令保留为排查入口。

mod cli;
mod config;
mod http;
mod model;
mod qq;
mod session;

use std::io::Read;

use anyhow::{Context, Result};
use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Command};
use crate::model::{Playlist, Song};

#[tokio::main]
async fn main() -> Result<()> {
    restore_sigpipe();
    let cli = Cli::parse();
    init_tracing(&cli.log);
    match cli.command {
        Command::Login { cookie } => login(&cookie)?,
        Command::Whoami => whoami(),
        Command::Playlists => print_playlists(&qq::user_playlists().await?),
        Command::Favorites => {
            let (playlist, songs) = qq::favorites().await?;
            print_tracks(&playlist, &songs);
        }
        Command::Tracks { id } if id == Playlist::FAVORITES_ID => {
            let (playlist, songs) = qq::favorites().await?;
            print_tracks(&playlist, &songs);
        }
        Command::Tracks { id } => {
            let (playlist, songs) = qq::playlist_tracks(&id).await?;
            print_tracks(&playlist, &songs);
        }
        Command::Logout => {
            session::jar().clear()?;
            println!("已清除本地会话");
        }
    }
    Ok(())
}

/// 恢复 SIGPIPE 的系统默认行为。
///
/// Rust 运行时会把 SIGPIPE 设成忽略，于是 `canto tracks … | head` 这种用法里，
/// 下游先退出后 `println!` 会 panic 并打出难看的 backtrace；这里改回默认，
/// 让进程像普通 Unix 工具那样安静退出。
#[cfg(unix)]
fn restore_sigpipe() {
    // SAFETY: 只把信号处置恢复成系统默认，进程启动时调用一次。
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
fn restore_sigpipe() {}

fn init_tracing(level: &str) {
    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

fn login(cookie: &str) -> Result<()> {
    let raw = if cookie == "-" {
        let mut raw = String::new();
        std::io::stdin()
            .read_to_string(&mut raw)
            .context("从标准输入读取 cookie 失败")?;
        raw
    } else {
        cookie.to_string()
    };
    let jar = session::jar();
    let state = jar.login_with_cookie(&raw)?;
    jar.save()?;
    println!(
        "已登录：uin={}（会话已保存到 {}）",
        state.identity.unwrap_or_default(),
        config::session_path().display()
    );
    Ok(())
}

fn whoami() {
    let state = session::jar().state();
    println!("会话文件：{}", config::session_path().display());
    if state.logged_in {
        println!("登录状态：已登录");
        println!("账号：{}", state.identity.as_deref().unwrap_or("（未知）"));
    } else {
        println!("登录状态：未登录");
        println!("登录方式：canto login --cookie '<从浏览器复制的 cookie>'");
    }
}

/// 歌单表格。列宽按数据算，避免中文列名与内容对不上时还要靠 `\t` 硬凑。
fn print_playlists(playlists: &[Playlist]) {
    println!("我的歌单（{} 个）", playlists.len());
    if playlists.is_empty() {
        println!("（空：可能登录已失效，或用 `canto favorites` 看「我喜欢」）");
        return;
    }
    let id_width = playlists
        .iter()
        .map(|playlist| playlist.id.len())
        .max()
        .unwrap_or(2)
        .max("id".len());
    println!("  {:<id_width$}  {:>6}  名称", "id", "歌曲数");
    for playlist in playlists {
        println!(
            "  {:<id_width$}  {:>6}  {}",
            playlist.id, playlist.song_count, playlist.name
        );
    }
}

/// 曲目表格：音质标签与时长是"数据对不对"最直接的证据。
fn print_tracks(playlist: &Playlist, songs: &[Song]) {
    println!(
        "{}（{} 首，本次返回 {} 首）",
        playlist.name,
        playlist.song_count,
        songs.len()
    );
    if let Some(cover) = &playlist.cover_url {
        println!("封面：{cover}");
    }
    let index_width = songs.len().to_string().len();
    for (index, song) in songs.iter().enumerate() {
        println!(
            "  {:>index_width$}  {:<6}  {}  {} — {}",
            index + 1,
            song.sizes.best_label(),
            song.duration_display(),
            song.title,
            song.singers,
        );
    }
}

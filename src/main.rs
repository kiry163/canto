//! canto —— 自用的 QQ 音乐播放器。
//!
//! 里程碑：M1 数据链路（登录/歌单/曲目）+ 播放引擎 + 终端界面。
//! 命令行子命令保留为排查入口（界面里出问题可以照着复现）。

mod cli;
mod config;
mod download;
mod http;
mod media;
mod model;
mod paths;
mod player;
mod qq;
mod qr;
mod session;
mod tui;

use std::io::Read;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Command};
use crate::model::{Playlist, Quality, Song};
use crate::player::{Engine, Phase, Snapshot};

/// 这里**不用** `#[tokio::main]`：播放与 `reqwest::blocking` 的媒体流都在自己的
/// 线程上同步跑，只有网络请求交给运行时。
fn main() -> Result<()> {
    restore_sigpipe();
    let cli = Cli::parse();
    let command = cli.command.unwrap_or(Command::Tui);
    // 界面模式下 stdout/stderr 就是屏幕，日志必须落文件，否则糊屏。
    let to_file = matches!(command, Command::Tui);
    init_tracing(&cli.log, to_file)?;
    let runtime = tokio::runtime::Runtime::new().context("创建异步运行时失败")?;

    match command {
        Command::Tui => {
            if !session::jar().state().logged_in {
                bail!("未登录：先 `canto login --cookie '<从浏览器复制的 cookie>'`");
            }
            let (config, warning) = config::load();
            let engine = Engine::start()?;
            tui::run(runtime.handle().clone(), engine, config, warning)?;
        }
        Command::Login { cookie } => login(&runtime, cookie.as_deref())?,
        Command::Whoami => whoami(),
        Command::Logout => {
            session::jar().clear()?;
            println!("已清除本地会话");
        }
        Command::Playlists => print_playlists(&runtime.block_on(qq::user_playlists())?),
        Command::Favorites => {
            let (playlist, songs) = runtime.block_on(qq::favorites())?;
            print_tracks(&playlist, &songs);
        }
        Command::Tracks { id } => {
            let (playlist, songs) = runtime.block_on(load_tracks(&id))?;
            print_tracks(&playlist, &songs);
        }
        Command::Download {
            id,
            index,
            quality,
            dir,
        } => {
            let quality = quality.as_deref().map(parse_quality).transpose()?;
            let (_, songs) = runtime.block_on(load_tracks(&id))?;
            let song = songs
                .get(index.saturating_sub(1))
                .with_context(|| format!("歌单里没有第 {index} 首（共 {} 首）", songs.len()))?
                .clone();
            let candidates = runtime.block_on(qq::vkey::candidates(&song, quality))?;
            let lyric = runtime
                .block_on(qq::lyric::get(&song))
                .unwrap_or_else(|error| {
                    tracing::warn!("取歌词失败，只下歌：{error:#}");
                    None
                });
            let dir = dir.unwrap_or_else(download::default_dir);
            println!("下载 {} — {} → {}", song.title, song.singers, dir.display());
            let mut reported = 0;
            let saved = download::save(
                &song,
                &candidates,
                &dir,
                lyric.as_ref(),
                &mut |received, total| {
                    let Some(total) = total.filter(|total| *total > 0) else {
                        return;
                    };
                    let percent = received * 100 / total;
                    if percent >= reported + 25 {
                        reported = percent;
                        println!("  {percent}%");
                    }
                },
            )?;
            if saved.existed {
                println!("已存在，跳过：{}", saved.path.display());
            } else {
                println!(
                    "完成：{}（{}，{:.1} MB）",
                    saved.path.display(),
                    saved.quality.label(),
                    saved.bytes as f64 / 1_048_576.0
                );
                for lrc in &saved.lyrics {
                    println!("  歌词：{}", lrc.display());
                }
            }
        }
        Command::Search { keyword, limit } => {
            let songs = runtime.block_on(qq::search::songs(&keyword, limit))?;
            println!("搜索「{keyword}」（{} 首）", songs.len());
            print_song_rows(&songs);
        }
        Command::Favorite { id, index } => {
            let song = runtime.block_on(pick_song(&id, index))?;
            runtime.block_on(qq::write::set_favorite(&song, true))?;
            println!("已收藏：{} — {}", song.title, song.singers);
        }
        Command::Unfavorite { id, index } => {
            let song = runtime.block_on(pick_song(&id, index))?;
            runtime.block_on(qq::write::set_favorite(&song, false))?;
            println!("已取消收藏：{} — {}", song.title, song.singers);
        }
        Command::Add { target, id, index } => {
            let song = runtime.block_on(pick_song(&id, index))?;
            let (dir_id, tid, name) = if target == Playlist::FAVORITES_ID {
                (qq::write::FAVORITES_DIR_ID, 0, "我喜欢".to_string())
            } else {
                let playlists = runtime.block_on(qq::user_playlists())?;
                let playlist = playlists
                    .iter()
                    .find(|playlist| playlist.id == target)
                    .with_context(|| {
                        format!("没有 id 为 {target} 的歌单（用 `canto playlists` 看）")
                    })?;
                (
                    playlist.dir_id,
                    playlist.id.parse::<u64>().unwrap_or(0),
                    playlist.name.clone(),
                )
            };
            runtime.block_on(qq::write::add(dir_id, tid, std::slice::from_ref(&song)))?;
            println!("已把《{} — {}》加进《{name}》", song.title, song.singers);
        }
        Command::Remove { target, id, index } => {
            let song = runtime.block_on(pick_song(&id, index))?;
            let (dir_id, tid, name) = runtime.block_on(resolve_playlist(&target))?;
            runtime.block_on(qq::write::remove(dir_id, tid, std::slice::from_ref(&song)))?;
            println!("已把《{} — {}》从《{name}》移除", song.title, song.singers);
        }
        Command::Lyric { id, index } => {
            let song = runtime.block_on(pick_song(&id, index))?;
            match runtime.block_on(qq::lyric::get(&song))? {
                Some(lyric) => {
                    println!(
                        "{} — {}（{} 行）",
                        song.title,
                        song.singers,
                        lyric.lines.len()
                    );
                    for line in &lyric.lines {
                        let seconds = line.at.as_secs();
                        let stamp = format!(
                            "{:02}:{:02}.{:02}",
                            seconds / 60,
                            seconds % 60,
                            line.at.subsec_millis() / 10
                        );
                        match &line.translation {
                            Some(translation) => {
                                println!("  [{stamp}] {}   ⟨{translation}⟩", line.text)
                            }
                            None => println!("  [{stamp}] {}", line.text),
                        }
                    }
                }
                None => println!("{} — {}：暂无歌词", song.title, song.singers),
            }
        }
        Command::Play { id, index, quality } => {
            let quality = quality.as_deref().map(parse_quality).transpose()?;
            let song = runtime.block_on(pick_song(&id, index))?;
            let candidates = runtime.block_on(qq::vkey::candidates(&song, quality))?;
            println!(
                "{} — {}  [{} 个候选档位]",
                song.title,
                song.singers,
                candidates.len()
            );
            let engine = Engine::start()?;
            engine.play(player::Track { song, candidates });
            let snapshot = wait_until_finished(&engine)?;
            let short = if snapshot.completed() {
                String::new()
            } else {
                format!(
                    "（应为 {:.0}s，流被中断）",
                    snapshot.duration.unwrap_or_default().as_secs_f64()
                )
            };
            println!(
                "播放结束：{} · 已播 {:.1}s{short}",
                snapshot.quality.map(Quality::label).unwrap_or("未知档位"),
                snapshot.position.as_secs_f64(),
            );
        }
    }
    Ok(())
}

/// 命令行播放：等这一首播完（或失败），返回最终快照。
fn wait_until_finished(engine: &Engine) -> Result<Snapshot> {
    loop {
        let snapshot = engine.snapshot();
        if let Phase::Failed(message) = &snapshot.phase {
            bail!("{message}");
        }
        if snapshot.just_finished {
            return Ok(snapshot);
        }
        if !engine.is_alive() {
            bail!("音频线程已退出");
        }
        std::thread::sleep(Duration::from_millis(150));
    }
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

/// 日志过滤：裸级别（`-l debug`）只作用于 canto 自己，其它库保持 `warn`。
///
/// 否则 reqwest/h2 在 debug 上会把每条连接、每个 TLS/HTTP2 帧都打出来，
/// 真正想看的日志全被淹掉（实测）。要看它们的日志就写完整指令
/// （`-l debug,h2=debug`）。
fn init_tracing(level: &str, to_file: bool) -> Result<()> {
    let filter = if level.contains('=') {
        EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("warn"))
    } else {
        EnvFilter::builder()
            .with_default_directive(tracing_subscriber::filter::LevelFilter::WARN.into())
            .parse_lossy(format!("canto={level}"))
    };
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false);
    if to_file {
        // 界面模式下日志写文件（`-l info` 之后可以翻它看播放细节）。
        let path = paths::log_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建日志目录失败：{}", parent.display()))?;
        }
        let file = std::fs::File::create(&path)
            .with_context(|| format!("写入日志文件失败：{}", path.display()))?;
        builder.with_writer(std::sync::Mutex::new(file)).init();
    } else {
        builder.init();
    }
    Ok(())
}

fn parse_quality(text: &str) -> Result<Quality> {
    Quality::parse(text)
        .with_context(|| format!("不认识的音质 {text:?}（可用：hires / flac / 320 / 128）"))
}

/// 把命令行给的歌单标识换成写入接口要的 `(dirId, tid, 名字)`。
async fn resolve_playlist(target: &str) -> Result<(u64, u64, String)> {
    if target == Playlist::FAVORITES_ID {
        return Ok((qq::write::FAVORITES_DIR_ID, 0, "我喜欢".to_string()));
    }
    let playlists = qq::user_playlists().await?;
    let playlist = playlists
        .iter()
        .find(|playlist| playlist.id == target)
        .with_context(|| format!("没有 id 为 {target} 的歌单（用 `canto playlists` 看）"))?;
    Ok((
        playlist.dir_id,
        playlist.id.parse().unwrap_or(0),
        playlist.name.clone(),
    ))
}

/// 取某个歌单里的第 `index` 首（从 1 开始）。
async fn pick_song(id: &str, index: usize) -> Result<Song> {
    let (_, songs) = load_tracks(id).await?;
    songs
        .get(index.saturating_sub(1))
        .cloned()
        .with_context(|| format!("歌单里没有第 {index} 首（共 {} 首）", songs.len()))
}

/// 歌单 id（或 `favorites`）→ 歌单与曲目。
async fn load_tracks(id: &str) -> Result<(Playlist, Vec<Song>)> {
    if id == Playlist::FAVORITES_ID {
        return qq::favorites().await;
    }
    qq::playlist_tracks(id).await
}

/// 二维码最多重新取几次（每次过期算一轮）。
const QR_ATTEMPTS: usize = 3;
/// 轮询间隔：够灵敏又不至于把接口刷爆。
const QR_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// 登录：给了 cookie 就粘贴，没给就走扫码。
fn login(runtime: &tokio::runtime::Runtime, cookie: Option<&str>) -> Result<()> {
    let Some(cookie) = cookie else {
        return qr_login(runtime);
    };
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
        paths::session_path().display()
    );
    Ok(())
}

/// 扫码登录：取二维码 → 画到终端 → 轮询到成功（过期就换一张）。
fn qr_login(runtime: &tokio::runtime::Runtime) -> Result<()> {
    let jar = session::jar();
    for attempt in 1..=QR_ATTEMPTS {
        let png = runtime.block_on(qq::login::create())?;
        let png_path = paths::qr_png_path();
        match qr::show(&png, &png_path)? {
            Some(path) => println!(
                "终端装不下二维码，已存成文件：{}\n用手机 QQ 扫描它。",
                path.display()
            ),
            None => println!("用手机 QQ 扫描上面的二维码。"),
        }
        if attempt > 1 {
            println!("（第 {attempt} 张二维码）");
        }
        println!("等待扫码…");
        loop {
            match runtime.block_on(qq::login::poll())? {
                qq::login::Poll::Waiting => std::thread::sleep(QR_POLL_INTERVAL),
                qq::login::Poll::Scanned => {
                    println!("已扫码，请在手机上确认登录…");
                    std::thread::sleep(QR_POLL_INTERVAL);
                }
                qq::login::Poll::Expired => {
                    println!("二维码已过期，重新获取…");
                    break;
                }
                qq::login::Poll::Success(identity) => {
                    jar.save()?;
                    println!(
                        "已登录：uin={identity}（会话已保存到 {}）",
                        paths::session_path().display()
                    );
                    return Ok(());
                }
                qq::login::Poll::Failed(message) => bail!("{message}"),
            }
        }
    }
    bail!("扫码登录未完成（二维码多次过期，请重跑 `canto login`）");
}

fn whoami() {
    let state = session::jar().state();
    println!("会话文件：{}", paths::session_path().display());
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
    print_song_rows(songs);
}

/// 带序号、音质、时长的曲目行（歌单与搜索结果共用）。
fn print_song_rows(songs: &[Song]) {
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

//! canto —— 自用的 QQ 音乐播放器。
//!
//! 只有一条启动路径：日志 → 配置 → 会话 → 终端界面。没登录时界面自己会先出
//! 登录屏（扫码或粘贴 cookie）；所有功能都在界面里，命令行只剩 `--log`。

mod cli;
mod cache;
mod config;
mod download;
mod history;
mod http;
mod instance;
mod levels;
mod media;
mod model;
mod paths;
mod player;
mod qq;
mod qr;
mod session;
mod spectrum;
mod tui;

use anyhow::{Context, Result};
use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::cli::Cli;
use crate::player::Engine;

/// 这里**不用** `#[tokio::main]`：播放与 `reqwest::blocking` 的媒体流都在自己的
/// 线程上同步跑，只有网络请求交给运行时。
fn main() -> Result<()> {
    restore_sigpipe();
    let cli = Cli::parse();
    // 数据目录搬过一次家（老版本在 `~/Library/Application Support/canto`）：必须在
    // `init_tracing` 之前搬，否则新位置先被建出来、老目录就成孤儿了。
    match paths::migrate_legacy() {
        Ok(Some(legacy)) => eprintln!(
            "数据目录已搬到 {}（原 {}）",
            paths::data_dir().display(),
            legacy.display()
        ),
        // 搬不动就让程序照常起：新目录会重新生成配置，登录态重扫一次码即可。
        Err(error) => eprintln!(
            "数据目录搬迁失败（{error:#}）；仍用 {}（老目录里的登录态需要手动搬过去）",
            paths::data_dir().display()
        ),
        Ok(None) => {}
    }

    // 单例要在写日志**之前**：`init_tracing` 会清空日志文件，第二个实例被拒时
    // 不该顺手把第一个实例的日志抹掉。
    let lock = paths::lock_path();
    let _instance = match instance::Instance::acquire(&lock)? {
        Some(instance) => instance,
        None => {
            let who = instance::pid_in(&lock)
                .map(|pid| format!("（pid {pid}）"))
                .unwrap_or_default();
            anyhow::bail!("已经有一个 canto 在跑{who}；同一份数据目录只允许一个实例");
        }
    };
    init_tracing(&cli.log)?;
    let runtime = tokio::runtime::Runtime::new().context("创建异步运行时失败")?;
    let (config, warning) = config::load();
    // 起播之前把缓存整理一下：删掉上次没退干净留下的半截文件，再按上限压一压。
    let (swept, freed) = cache::sweep(config.cache_mb * 1024 * 1024);
    if swept > 0 {
        tracing::info!("缓存清理：删除 {swept} 个文件，释放 {freed} 字节");
    }
    let engine = Engine::start(config.cache_mb > 0)?;
    tui::run(
        runtime.handle().clone(),
        engine,
        config,
        paths::history_path(),
        warning,
    )
}

/// 恢复 SIGPIPE 的系统默认行为。
///
/// Rust 运行时会把 SIGPIPE 设成忽略：下游（比如 `| head`）先退出后再往屏幕写
/// 就会 panic 并打出难看的 backtrace；这里改回默认，让进程像普通 Unix 工具那样
/// 安静退出。
#[cfg(unix)]
fn restore_sigpipe() {
    // SAFETY: 只把信号处置恢复成系统默认，进程启动时调用一次。
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
fn restore_sigpipe() {}

/// 日志写文件：界面里 stdout/stderr 就是屏幕，往那儿打印会糊屏。
///
/// 过滤规则：裸级别（`--log debug`）只作用于 canto 自己，其它库保持 `warn`——
/// 否则 reqwest/h2 在 debug 上会把每条连接、每个 TLS/HTTP2 帧都打出来，真正想看的
/// 日志全被淹掉（实测）。要看它们的日志就写完整指令（`--log debug,h2=debug`）。
fn init_tracing(level: &str) -> Result<()> {
    let filter = if level.contains('=') {
        EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("warn"))
    } else {
        EnvFilter::builder()
            .with_default_directive(tracing_subscriber::filter::LevelFilter::WARN.into())
            .parse_lossy(format!("canto={level}"))
    };
    let path = paths::log_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建日志目录失败：{}", parent.display()))?;
    }
    let file = std::fs::File::create(&path)
        .with_context(|| format!("写入日志文件失败：{}", path.display()))?;
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::sync::Mutex::new(file))
        .init();
    Ok(())
}

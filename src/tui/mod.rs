//! 终端界面（ratatui）。
//!
//! 线程模型：本线程只做「画帧 + 读键盘」；网络请求丢给 tokio 运行时（结果用通道送回），
//! 播放交给 [`crate::player::Engine`]。三者之间没有共享可变状态，只有状态快照与消息。
//!
//! 键位：
//! - `q` / `Ctrl-C` 退出，`Tab` 切换左右栏
//! - `j`/`k`（或方向键上下）移动，`g`/`G` 首尾
//! - `Enter` 左栏=加载该歌单，右栏=播放该首
//! - `Space` 播放/暂停，`n`/`b` 下一首/上一首，`←`/`→`（或 `h`/`l`）±10 秒，`+`/`-` 音量
//! - `y` 开/关歌词栏（跟着播放位置高亮当前行，带翻译；没有歌词也画）
//! - `v` 开/关频谱区：真频谱（跟着音频采样动）替掉列表；`Esc` 不退它，要退按 `v`
//! - `/` 搜索（回车搜、Esc 取消；结果显示在曲目栏，直接回车播放）
//! - `d` 下载选中的那首（含同名歌词）到默认目录，进度显示在状态栏

pub mod draw;
pub mod keys;
pub mod login;
pub mod queue;
pub mod state;

#[cfg(test)]
pub mod test_support;

use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Duration as StdDuration;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyEventKind};

use self::state::{App, Msg};
use crate::config::Config;
use crate::hotkeys;
use crate::overlay;
use crate::player::Engine;

/// 界面刷新间隔，同时也是按键与后台消息的最长等待。
const TICK: StdDuration = StdDuration::from_millis(150);
/// 频谱在动时的刷新间隔（约 30fps）。终端一次重绘是整屏 diff，再往上（60fps）
/// 观感差别很小、重绘成本却翻倍，所以取这个折中。
const SPECTRUM_TICK: StdDuration = StdDuration::from_millis(33);

pub fn run(
    handle: tokio::runtime::Handle,
    engine: Engine,
    config: Config,
    history_path: std::path::PathBuf,
    warning: Option<String>,
) -> Result<()> {
    let (tx, rx) = channel();
    // 全局快捷键：`config` 马上被 move 进 `App`，绑定先算出来。
    let bindings = config.hotkeys.bindings();
    let mut app = App::new(
        handle,
        engine,
        config,
        history_path,
        crate::paths::local_playlists_path(),
        crate::paths::session_path(),
        tx,
    );
    let (hotkeys, report) = hotkeys::install(&bindings);
    app.hotkeys = hotkeys;
    if report.registered > 0 {
        tracing::info!("已注册 {} 个全局热键", report.registered);
    }
    app.history =
        crate::history::History::load(&app.history_path.clone(), app.config.history.limit);
    // 曲库扫描在后台跑（读 tag 是磁盘密集型），界面不等它——先读索引的那一步在
    // `start_library_scan` 里已经同步做掉了，所以「本地歌曲」屏第一次进去就有歌。
    app.start_library_scan();
    // 落屏**只在这儿决定一次**：两种登录态都落首页——QQ 那几屏进去时给一句
    // 「需要登录：按 L」，本地歌曲与本地歌单独立可用；登录屏改从 `L` 进（进了才取码）。
    app.load_initial();
    if let Some(warning) = warning {
        app.notify(warning);
    }
    // 注册失败 / 这个平台不支持：接在配置警告后面说。通知槽只有一个，后写的占位，
    // 两条的细节都在日志里。
    if let Some(text) = report.notice() {
        app.notify(text);
    }
    // 桌面歌词：装一个覆盖层窗口（Windows / macOS）。状态文件说开着才建窗口；之后由全局热键
    // 真拆真建。平台不支持 / 起不来都得到空句柄，界面照跑——起不来的原因在日志里。
    let installed = overlay::DesktopLyrics::install(
        &app.config.desktop_lyrics,
        overlay::load_state(&crate::paths::desktop_lyrics_path()),
    );
    app.overlay = installed.overlay;
    if let Some(error) = installed.error {
        tracing::warn!(%error, "桌面歌词起不来");
        app.notify("歌词窗口起不来");
    }

    let mut terminal = ratatui::init();
    let outcome = event_loop(&mut terminal, &mut app, &rx);
    ratatui::restore();
    outcome
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    rx: &Receiver<Msg>,
) -> Result<()> {
    while !app.quit {
        // 全局热键：只有 macOS 在这里真干活（Carbon 的热键事件得自己从队列里拉出来派发）。
        app.hotkeys.pump();
        terminal.draw(|frame| app.draw(frame)).context("渲染失败")?;
        // 频谱在动时提速：6.7fps 下柱子是一格一跳。
        let tick = if app.spectrum_animating() {
            SPECTRUM_TICK
        } else {
            TICK
        };
        if event::poll(tick).context("读取键盘事件失败")?
            && let Event::Key(key) = event::read().context("读取键盘事件失败")?
            && key.kind == KeyEventKind::Press
        {
            app.handle_key(key);
        }
        loop {
            match rx.try_recv() {
                Ok(message) => app.on_msg(message),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        app.tick();
    }
    Ok(())
}

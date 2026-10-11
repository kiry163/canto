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
//! - `y` 开/关歌词面板（跟着播放位置高亮当前行，带翻译）
//! - `Y` 全屏正在播放：上半屏真频谱（跟着音频采样动）、下半屏歌词；`Y` 或 `Esc` 退出
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
    let mut app = App::new(handle, engine, config, history_path, tx);
    app.history = crate::history::History::load(&app.history_path.clone(), app.config.history_limit);
    // 没登录就先出登录屏：所有功能都在界面里，命令行不再有 login 这条路。
    if crate::session::jar().state().logged_in {
        app.load_initial();
    } else {
        app.start_login();
    }
    if let Some(warning) = warning {
        app.status = warning;
    }
    app.load_initial();

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

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
//! - `/` 搜索（回车搜、Esc 取消；结果显示在曲目栏，直接回车播放）
//! - `d` 下载选中的那首（含同名歌词）到默认目录，进度显示在状态栏

pub mod draw;
pub mod keys;
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

pub fn run(
    handle: tokio::runtime::Handle,
    engine: Engine,
    config: Config,
    warning: Option<String>,
) -> Result<()> {
    let (tx, rx) = channel();
    let mut app = App::new(handle, engine, config, tx);
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
        if event::poll(TICK).context("读取键盘事件失败")?
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

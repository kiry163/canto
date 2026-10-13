//! 首页（`0` 界面）：四行屏入口，`j`/`k` 选、`Enter` 进、数字 `1`–`4` 直跳。
//!
//! 它只画内容区那一块——播放条、状态行、歌词块、浮层都由主界面那套外壳负责（首页不是
//! 另一套界面，只是内容区换了东西）。行由 `Menu::ALL` 生成，编号就是下标 + 1。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::{List, ListItem};

use super::super::state::{App, Menu};
use super::{highlight, panel_block};

impl App {
    pub(super) fn draw_home(&mut self, frame: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = Menu::ALL
            .iter()
            .enumerate()
            .map(|(index, menu)| ListItem::new(format!("{}  {}", index + 1, menu.title())))
            .collect();
        let list = List::new(items)
            .block(panel_block(" 首页 "))
            .highlight_style(highlight())
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(list, area, &mut self.home_state);
    }
}

//! 状态行：左边是当前焦点的键位提示，右边是一次性状态（加载 / 错误 / 结果）。
//!
//! 两段分开是有意的：旧界面把它们挤在同一行里互相顶掉，加载中就看不到键位提示。

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use super::super::state::App;
use super::hints;

impl App {
    pub(super) fn draw_status(&self, frame: &mut Frame, area: Rect) {
        let status = Line::from(self.status_text());
        // 列宽要用**显示宽度**而不是字符个数：中文、书名号都是双宽字符，
        // 按字符数留列会把尾巴裁掉（实测「已加载《嗯》」6 个字符占 9 列）。
        let right = (status.width() as u16 + 1).min(area.width / 2).max(1);
        let columns =
            Layout::horizontal([Constraint::Min(1), Constraint::Length(right)]).split(area);
        frame.render_widget(Paragraph::new(hints(self.context())), columns[0]);
        if !status.spans.is_empty() && status.width() > 0 {
            frame.render_widget(Paragraph::new(status).alignment(Alignment::Right), columns[1]);
        }
    }

    fn status_text(&self) -> String {
        match &self.loading {
            Some(loading) => format!("⏳ {loading}"),
            None => self.status.clone(),
        }
    }
}

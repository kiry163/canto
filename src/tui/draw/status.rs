//! 状态行：左边是当前焦点的键位提示，右边是一条**通知**（进度、错误、结果）。
//!
//! 两侧分开是有意的：旧界面把它们挤在同一行里互相顶掉，加载中就看不到键位提示。
//! 右边是固定列宽的单槽（见 `NOTICE_COLS`），内容只放短消息——长文案该进日志。

use std::borrow::Cow;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::super::state::App;
use super::hints;

/// 通知位的列宽。
///
/// **固定**而不是随内容伸缩：右边一伸缩，左边那半行键位提示就会忽长忽短地跳。
/// 代价是超过这些列的通知要截断，所以往这里写的东西必须短（长错误链进日志）。
const NOTICE_COLS: u16 = 32;

/// 按**显示宽度**截断（中文、书名号都是双宽），超出就用省略号收尾。
///
/// 固定列宽下必须截：整条画不完时至少让人看出「被裁了」，而不是话说到一半。
fn truncate_to_cols(text: &str, cols: u16) -> Cow<'_, str> {
    let cols = cols as usize;
    if text.width() <= cols {
        return Cow::Borrowed(text);
    }
    let mut kept = String::new();
    let mut used = 0;
    for character in text.chars() {
        // 留一列给省略号本身。
        let width = unicode_width::UnicodeWidthChar::width(character).unwrap_or(0);
        if used + width > cols.saturating_sub(1) {
            break;
        }
        kept.push(character);
        used += width;
    }
    kept.push('…');
    Cow::Owned(kept)
}

impl App {
    pub(super) fn draw_status(&self, frame: &mut Frame, area: Rect) {
        let columns = Layout::horizontal([
            Constraint::Min(1),
            Constraint::Length(NOTICE_COLS),
        ])
        .split(area);
        frame.render_widget(
            Paragraph::new(hints(self.context(), self.local_features_on())),
            columns[0],
        );
        if let Some(text) = self.notice_text() {
            // 常见情况（没超宽）不拷贝：`Cow` 借的就是通知本身。
            let line = match truncate_to_cols(text, NOTICE_COLS) {
                Cow::Borrowed(text) => Line::from(text),
                Cow::Owned(text) => Line::from(text),
            };
            frame.render_widget(
                Paragraph::new(line).alignment(Alignment::Right),
                columns[1],
            );
        }
    }
}

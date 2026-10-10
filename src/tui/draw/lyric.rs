//! 歌词块：当前行高亮，跟着播放位置走，翻译跟在本行下面。

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, List, ListItem, Paragraph};

use super::super::state::App;

impl App {
    pub(super) fn draw_lyric(&mut self, frame: &mut Frame, area: Rect) {
        let title = match &self.lyric {
            Some(lyric) => format!(" 歌词（{} 行） ", lyric.lines.len()),
            None => " 歌词 ".to_string(),
        };
        let block = Block::bordered().title(title);
        let Some(lyric) = &self.lyric else {
            let text = if self.lyric_loading {
                "歌词加载中…"
            } else {
                "暂无歌词"
            };
            frame.render_widget(
                Paragraph::new(text)
                    .alignment(Alignment::Center)
                    .style(Style::new().fg(Color::DarkGray))
                    .block(block),
                area,
            );
            return;
        };
        let items: Vec<ListItem> = lyric
            .lines
            .iter()
            .map(|line| {
                let mut rows = vec![Line::from(line.text.clone()).alignment(Alignment::Center)];
                if let Some(translation) = &line.translation {
                    rows.push(
                        Line::from(translation.clone())
                            .style(Style::new().fg(Color::DarkGray))
                            .alignment(Alignment::Center),
                    );
                }
                ListItem::new(rows)
            })
            .collect();
        let current = lyric.line_at(self.engine.snapshot().position);
        self.lyric_state.select(current);
        let list = List::new(items)
            .block(block)
            .highlight_symbol("")
            .highlight_style(Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD));
        frame.render_stateful_widget(list, area, &mut self.lyric_state);
    }

}

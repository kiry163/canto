//! 左侧菜单（宽屏是左列，窄屏是顶部一行页签）。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph};

use super::super::state::{App, Focus, Menu};
use super::{focused_block, highlight};

impl App {
    pub(super) fn draw_menu(&mut self, frame: &mut Frame, area: Rect) {
        let focused = self.nav.focus == Focus::Menu;
        let items: Vec<ListItem> = Menu::ALL
            .iter()
            .map(|menu| ListItem::new(format!("{} {}", icon(*menu), menu.title())))
            .collect();
        // 高亮跟着菜单游标走：`j`/`k` 只动它，进去要按 Enter。
        self.menu_state.select(Some(self.nav.cursor.index()));
        let list = List::new(items)
            .block(focused_block(" 菜单 ", focused))
            .highlight_style(highlight())
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(list, area, &mut self.menu_state);
    }

}

/// 窄屏：菜单收成顶部一行页签，主区吃满全宽。
impl App {
    pub(super) fn draw_tabs(&mut self, frame: &mut Frame, area: Rect) {
        let mut spans = Vec::new();
        for menu in Menu::ALL {
            let label = format!("{} {} {}", menu.index() + 1, icon(menu), menu.title());
            let style = if menu == self.nav.menu {
                highlight()
            } else {
                Style::new().fg(Color::DarkGray)
            };
            spans.push(Span::styled(label, style));
            spans.push(Span::styled("  ·  ", Style::new().fg(Color::DarkGray)));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }
}

fn icon(menu: Menu) -> &'static str {
    match menu {
        Menu::Favorites => "♥",
        Menu::Playlists => "≡",
        Menu::Search => "⌕",
        Menu::History => "↺",
    }
}

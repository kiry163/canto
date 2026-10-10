//! 浮层：歌单选择框（队列浮层与帮助浮层随后加在这里）。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Clear, List, ListItem};

use super::super::state::App;
use super::highlight;

impl App {
    pub(super) fn draw_picker(&mut self, frame: &mut Frame, area: Rect) {
        let Some((playlists, state, song)) = self.picker.as_mut() else {
            return;
        };
        let width = area.width.saturating_sub(4).min(56);
        let height = playlists.len() as u16 + 2;
        let popup = super::overlay_rect(area, width, height);
        frame.render_widget(Clear, popup);
        let items: Vec<ListItem> = playlists
            .iter()
            .map(|playlist| ListItem::new(format!("{:>5}  {}", playlist.song_count, playlist.name)))
            .collect();
        let list = List::new(items)
            .block(Block::bordered().title(format!(" 把《{}》加进 ", song.title)))
            .highlight_style(highlight())
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(list, popup, state);
    }

}

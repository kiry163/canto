//! 主区：按菜单项渲染当前这一屏。

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph};

use std::time::Duration as StdDuration;

use crate::player::Phase;

use super::super::state::{App, Focus, Menu};
use super::{focused_block, format_duration, format_size, highlight};

impl App {
    pub(super) fn draw_content(&mut self, frame: &mut Frame, area: Rect) {
        let title = format!(" {} ", self.nav.breadcrumb.join(" › "));
        let focused = self.nav.focus == Focus::Content;
        match self.nav.menu {
            Menu::Playlists if self.nav.shown.is_none() => {
                let items: Vec<ListItem> = self
                    .nav
                    .playlists
                    .items
                    .iter()
                    .map(|playlist| {
                        let marker = if playlist.is_favorites { "♥" } else { " " };
                        ListItem::new(format!(
                            "{marker} {:>5}  {}",
                            playlist.song_count, playlist.name
                        ))
                    })
                    .collect();
                let list = List::new(items)
                    .block(focused_block(&title, focused))
                    .highlight_style(highlight())
                    .highlight_symbol("▶ ");
                frame.render_stateful_widget(list, area, &mut self.nav.playlists.state);
            }
            Menu::Search => {
                // 输入行占第一行（Task 7 把它做成常驻），结果在下面。
                let rows =
                    Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
                let keyword = self.nav.search_text.clone();
                let prompt = if self.nav.search_input {
                    Line::from(vec![
                        Span::styled("搜索：", Style::new().fg(Color::Yellow)),
                        Span::raw(keyword),
                        Span::styled("▏", Style::new().fg(Color::Yellow)),
                    ])
                } else if keyword.is_empty() {
                    Line::from(Span::styled(
                        "按 / 输入关键词，回车搜索",
                        Style::new().fg(Color::DarkGray),
                    ))
                } else {
                    Line::from(vec![
                        Span::styled("搜索：", Style::new().fg(Color::DarkGray)),
                        Span::raw(keyword),
                    ])
                };
                frame.render_widget(Paragraph::new(prompt), rows[0]);
                self.draw_songs(frame, rows[1], &title, focused);
            }
            _ => self.draw_songs(frame, area, &title, focused),
        }
    }

    /// 曲目列表：所有「装着一堆歌」的屏共用（收藏 / 歌单曲目 / 搜索 / 历史）。
    pub(super) fn draw_songs(&mut self, frame: &mut Frame, area: Rect, title: &str, focused: bool) {
        let playing = self.now_playing_mid();
        let items: Vec<ListItem> = self
            .nav
            .song_view()
            .items
            .iter()
            .map(|song| {
                let marker = if Some(song.mid.as_str()) == playing.as_deref() {
                    "♪"
                } else {
                    " "
                };
                ListItem::new(format!(
                    "{marker} {:>5}  {:<6} {:>7}  {} — {}",
                    format_duration(StdDuration::from_secs(song.duration_secs)),
                    song.sizes.best_label(),
                    song.sizes
                        .best_size()
                        .map_or_else(|| "—".to_string(), format_size),
                    song.title,
                    song.singers,
                ))
            })
            .collect();
        let mut state = std::mem::take(&mut self.nav.song_view_mut().state);
        let list = List::new(items)
            .block(focused_block(title, focused))
            .highlight_style(highlight())
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(list, area, &mut state);
        self.nav.song_view_mut().state = state;
    }

    /// 正在播的那首的 mid（没在播/播完就是 `None`）。
    ///
    /// 按 mid 比对而不是「标题 — 歌手」字符串：QQ 的 mid 全局唯一，同歌手的歌
    /// 不再互相误标。
    fn now_playing_mid(&self) -> Option<String> {
        let snapshot = self.engine.snapshot();
        if snapshot.phase == Phase::Idle {
            return None;
        }
        snapshot.mid
    }
}

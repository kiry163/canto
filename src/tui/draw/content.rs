//! 内容区：按当前屏渲染。

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph};

use std::time::Duration as StdDuration;

use crate::player::Phase;

use super::super::state::{App, Menu};
use super::{format_duration, format_size, highlight, panel_block};

impl App {
    pub(super) fn draw_content(&mut self, frame: &mut Frame, area: Rect) {
        let title = self.content_title();
        // 筛选框开着时，第一行让给输入行（跟搜索屏的输入行一个样子）。
        let area = if self.filter_input {
            let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
            self.draw_filter_input(frame, rows[0]);
            rows[1]
        } else {
            area
        };
        match self.nav.menu {
            Menu::Playlists if self.nav.shown.is_none() => {
                let items: Vec<ListItem> = self
                    .nav
                    .playlists
                    .visible_rows()
                    .map(|(playlist, _)| {
                        ListItem::new(format!("{:>5}  {}", playlist.song_count, playlist.name))
                    })
                    .collect();
                let list = List::new(items)
                    .block(panel_block(&title))
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
                        "按 3 回到输入框（输入框开着时数字键是打字）",
                        Style::new().fg(Color::DarkGray),
                    ))
                } else {
                    Line::from(vec![
                        Span::styled("搜索：", Style::new().fg(Color::DarkGray)),
                        Span::raw(keyword),
                    ])
                };
                frame.render_widget(Paragraph::new(prompt), rows[0]);
                self.draw_songs(frame, rows[1], &title);
            }
            Menu::History => self.draw_history(frame, area, &title),
            _ => self.draw_songs(frame, area, &title),
        }
    }

    /// 内容区标题：面包屑，外加「筛选」徽标（知道自己在看子集、剩下几条）。
    fn content_title(&self) -> String {
        let breadcrumb = self.nav.breadcrumb.join(" › ");
        let keyword = self.nav.filter_text();
        if keyword.is_empty() {
            return format!(" {breadcrumb} ");
        }
        let (hits, total) = self.nav.filter_counts();
        format!(" {breadcrumb} 筛选「{keyword}」{hits}/{total} ")
    }

    /// `/` 打开的筛选输入行。
    fn draw_filter_input(&self, frame: &mut Frame, area: Rect) {
        let line = Line::from(vec![
            Span::styled("筛选：", Style::new().fg(Color::Yellow)),
            Span::raw(self.nav.filter_text().to_string()),
            Span::styled("▏", Style::new().fg(Color::Yellow)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    }

    /// 历史屏：行是「标题 — 歌手」，右侧写「时间 · 来自《来源》」。
    ///
    /// 不画时长 / 档位 / 体积那三列——历史里只有 mid / 标题 / 歌手，
    /// 画出来会是假的 00:00。
    fn draw_history(&mut self, frame: &mut Frame, area: Rect, title: &str) {
        let block = panel_block(title);
        if self.history.is_empty() {
            frame.render_widget(
                Paragraph::new("暂无播放历史")
                    .style(Style::new().fg(Color::DarkGray))
                    .alignment(ratatui::layout::Alignment::Center)
                    .block(block),
                area,
            );
            return;
        }
        let playing = self.now_playing_mid();
        let items: Vec<ListItem> = self
            .nav
            .song_view()
            .visible_rows()
            .map(|(song, label)| {
                let marker = if playing.as_deref() == Some(song.mid.as_str()) {
                    "♪"
                } else {
                    " "
                };
                ListItem::new(Line::from(vec![
                    Span::raw(format!("{marker} {} — {}", song.title, song.singers)),
                    Span::raw("  "),
                    Span::styled(label.to_string(), Style::new().fg(Color::DarkGray)),
                ]))
            })
            .collect();
        let mut state = std::mem::take(&mut self.nav.song_view_mut().state);
        let list = List::new(items)
            .block(block)
            .highlight_style(highlight())
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(list, area, &mut state);
        self.nav.song_view_mut().state = state;
    }

    /// 曲目列表：所有「装着一堆歌」的屏共用（收藏 / 歌单曲目 / 搜索）。
    pub(super) fn draw_songs(&mut self, frame: &mut Frame, area: Rect, title: &str) {
        let playing = self.now_playing_mid();
        // 档位列按**配置的下限**算：配置成 320 就不该再列 `FLAC 31MB`——那会让人
        // 以为它要下无损。真正取到的那一档仍以播放条为准。
        let want = self.config.quality;
        let items: Vec<ListItem> = self
            .nav
            .song_view()
            .visible_rows()
            .map(|(song, _)| {
                let marker = if Some(song.mid.as_str()) == playing.as_deref() {
                    "♪"
                } else {
                    " "
                };
                let (quality, size) = song.sizes.at_or_below(want);
                let mut line = vec![Span::raw(format!(
                    "{marker} {:>5}  {:<6} {:>7}  ",
                    format_duration(StdDuration::from_secs(song.duration_secs)),
                    quality.file_label(),
                    size.map_or_else(|| "—".to_string(), format_size),
                ))];
                // 已缓存：本地有完整的一份，重听 0 网络。占位宽度固定，行间才不会错位。
                line.push(Span::styled(
                    if self.cached.contains(&song.mid) {
                        "已缓存 "
                    } else {
                        "       "
                    },
                    Style::new().fg(Color::Green),
                ));
                line.push(Span::raw(format!("{} — {}", song.title, song.singers)));
                ListItem::new(Line::from(line))
            })
            .collect();
        let mut state = std::mem::take(&mut self.nav.song_view_mut().state);
        let list = List::new(items)
            .block(panel_block(title))
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

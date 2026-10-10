//! 绘制：布局与各面板。
//!
//! 所有「怎么画」都在这里；状态与按键在 [`super::state`]。

use std::time::Duration as StdDuration;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Gauge, List, ListItem, Paragraph};

use crate::player::Phase;

use super::state::{App, Focus, Menu};

impl App {

    pub(super) fn draw(&mut self, frame: &mut Frame) {
        let rows = Layout::vertical([
            Constraint::Min(5),
            // 播放条：上下边框各 1 行 + 标题行 + 进度条行。
            Constraint::Length(4),
            Constraint::Length(1),
        ])
        .split(frame.area());
        let columns =
            Layout::horizontal([Constraint::Length(26), Constraint::Min(20)]).split(rows[0]);
        self.draw_menu(frame, columns[0]);
        // 右栏：曲目 + 歌词（终端太矮时不展开，免得曲目只剩一两行）。
        let lyric_height = if self.show_lyric && columns[1].height >= 12 {
            (columns[1].height / 3).max(5)
        } else {
            0
        };
        let right = Layout::vertical([Constraint::Min(5), Constraint::Length(lyric_height)])
            .split(columns[1]);
        self.draw_content(frame, right[0]);
        if let Some(area) = right.get(1).filter(|_| lyric_height > 0) {
            self.draw_lyric(frame, *area);
        }
        self.draw_status(frame, rows[1], rows[2]);
        if self.picker.is_some() {
            self.draw_picker(frame, frame.area());
        }
    }

    /// `a` 键弹出的歌单选择框。
    fn draw_picker(&mut self, frame: &mut Frame, area: Rect) {
        let Some((playlists, state, song)) = self.picker.as_mut() else {
            return;
        };
        let width = area.width.saturating_sub(4).min(56);
        let height = (playlists.len() as u16 + 2).min(area.height.saturating_sub(2));
        let popup = Rect {
            x: area.x + (area.width.saturating_sub(width)) / 2,
            y: area.y + (area.height.saturating_sub(height)) / 2,
            width,
            height,
        };
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

    /// 左侧菜单：四项 + 底部当前账号。它是「我在哪」，不是歌单列表。
    fn draw_menu(&mut self, frame: &mut Frame, area: Rect) {
        let focused = self.nav.focus == Focus::Menu;
        let items: Vec<ListItem> = Menu::ALL
            .iter()
            .map(|menu| {
                let icon = match menu {
                    Menu::Favorites => "♥",
                    Menu::Playlists => "≡",
                    Menu::Search => "⌕",
                    Menu::History => "↺",
                };
                ListItem::new(format!("{icon} {}", menu.title()))
            })
            .collect();
        let rows = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(area);
        // 高亮跟着当前菜单项走（菜单项就是当前这一屏，不需要单独的选中态）。
        self.menu_state.select(Some(self.nav.menu.index()));
        let list = List::new(items)
            .block(focused_block(" 菜单 ", focused))
            .highlight_style(highlight())
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(list, rows[0], &mut self.menu_state);

        let identity = crate::session::jar()
            .state()
            .identity
            .unwrap_or_else(|| "未登录".to_string());
        frame.render_widget(
            Paragraph::new(format!(" {identity}")).style(Style::new().fg(Color::DarkGray)),
            rows[1],
        );
    }

    /// 主区：按菜单项渲染当前这一屏，标题写面包屑。
    fn draw_content(&mut self, frame: &mut Frame, area: Rect) {
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
    fn draw_songs(&mut self, frame: &mut Frame, area: Rect, title: &str, focused: bool) {
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

    /// 歌词面板：当前行高亮，跟着播放位置走，翻译跟在本行下面。
    fn draw_lyric(&mut self, frame: &mut Frame, area: Rect) {
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

    fn draw_status(&self, frame: &mut Frame, player_area: Rect, hint_area: Rect) {
        let snapshot = self.engine.snapshot();
        let state_text = match &snapshot.phase {
            Phase::Idle => "已停止".to_string(),
            Phase::Buffering => "缓冲中…".to_string(),
            Phase::Playing => "播放中".to_string(),
            Phase::Paused => "已暂停".to_string(),
            Phase::Failed(message) => format!("失败：{message}"),
        };
        // 档位后面跟实际文件的字节数：同一档位不同歌、不同 CDN 文件大小差很多，
        // 一眼能看出「这首为什么大」。
        let quality = match (snapshot.quality, snapshot.size) {
            (Some(quality), Some(size)) => {
                format!("[{} {}]", quality.label(), format_size(size))
            }
            (Some(quality), None) => format!("[{}]", quality.label()),
            (None, _) => String::new(),
        };
        let queue = if self.queue.is_empty() {
            String::new()
        } else {
            format!(
                "  {}/{}  ·  音量 {}%",
                self.queue_index + 1,
                self.queue.len(),
                snapshot.volume
            )
        };
        let title = snapshot.title.clone().unwrap_or_else(|| "—".to_string());
        let colors = Style::new().fg(Color::Cyan);
        let line = Line::from(vec![
            Span::styled("♪ ", colors),
            Span::styled(title, colors.add_modifier(Modifier::BOLD)),
            Span::raw(format!("  {quality} {state_text}{queue}")),
        ]);
        let gauge = Gauge::default()
            .ratio(snapshot.progress())
            .label(format!(
                "{} / {}",
                format_duration(snapshot.position),
                snapshot
                    .duration
                    .map(format_duration)
                    .unwrap_or_else(|| "--:--".to_string())
            ))
            .gauge_style(Style::new().fg(Color::Green))
            .use_unicode(true);
        // 边框归块，里面两行分别给标题行和进度条——不然进度条会盖在标题上。
        let block = Block::bordered().title(" 播放 ");
        let inner = block.inner(player_area);
        frame.render_widget(block, player_area);
        let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(inner);
        if let Some(row) = rows.first() {
            frame.render_widget(Paragraph::new(line), *row);
        }
        if let Some(row) = rows.get(1) {
            frame.render_widget(gauge, *row);
        }

        let hint = if let Some(loading) = &self.loading {
            format!("⏳ {loading}")
        } else if self.status.is_empty() {
            "Enter 播放 · Space 暂停 · n/b 切歌 · f 收藏 · a 加歌单 · x 移除 · Tab 切栏 · q 退出"
                .to_string()
        } else {
            self.status.clone()
        };
        frame.render_widget(
            Paragraph::new(hint).style(Style::new().fg(Color::DarkGray)),
            hint_area,
        );
    }
}


fn focused_block(title: &str, focused: bool) -> Block<'static> {
    let block = Block::bordered().title(title.to_string());
    if focused {
        block.border_style(Style::new().fg(Color::Yellow))
    } else {
        block
    }
}

fn highlight() -> Style {
    Style::new()
        .bg(Color::Indexed(236))
        .add_modifier(Modifier::BOLD)
}

/// 文件大小（十进制 MB，跟下载进度的写法一致）。
fn format_size(bytes: u64) -> String {
    const MIB: f64 = 1_048_576.0;
    if bytes as f64 >= MIB {
        format!("{:.1}MB", bytes as f64 / MIB)
    } else {
        format!("{}KB", bytes / 1024)
    }
}

fn format_duration(duration: StdDuration) -> String {
    let seconds = duration.as_secs();
    let (hours, minutes, seconds) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

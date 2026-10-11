//! 歌词：当前行**居中**高亮，翻译跟在本行下面。
//!
//! 两种画法共用这一份：底部那一块（`y`）与全屏（`Y`，占满内容区，播放条与状态行不动）。
//!
//! 滚动位置是自己算的（见 [`lyric_rows`] / [`centered_scroll`]），没交给 `List` 的
//! 「自动保可见」——那个只在选中行撞到边缘时才滚，当前行永远贴在下沿，不是居中。

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};

use crate::model::Lyric;

use super::super::state::App;

/// 要画的一行：正文或翻译，以及是不是当前行。
#[derive(Debug, PartialEq, Eq)]
struct LyricRow {
    text: String,
    translation: bool,
    current: bool,
}

/// 一行歌词占几行：正文一行，带翻译再加一行。
fn row_span(has_translation: bool) -> usize {
    1 + usize::from(has_translation)
}

/// 让第 `current` 行（占 `span` 行）落在 `height` 行的可视区正中。
///
/// 到开头 / 结尾就夹住：否则第一句之前、最后一句之后会飘出整屏空行。
fn centered_scroll(rows_before: usize, span: usize, total_rows: usize, height: usize) -> usize {
    if height == 0 || total_rows <= height {
        return 0;
    }
    let half = height.saturating_sub(span) / 2;
    rows_before.saturating_sub(half).min(total_rows - height)
}

/// 歌词块左上角的标题：底部那块与全屏**共用同一句**「歌词 · 歌名」，全屏再加退出提示。
///
/// 不写歌词行数：那是只有调试才关心的数字，占着标题最显眼的位置纯属噪音。
fn lyric_title(song: Option<&str>, full: bool) -> String {
    let mut title = String::from("歌词");
    if let Some(song) = song.filter(|song| !song.is_empty()) {
        title.push_str(" · ");
        title.push_str(song);
    }
    if full {
        title.push_str("（Y 或 Esc 退出）");
    }
    format!(" {title} ")
}

/// 摊平成要画的行 + 滚动位置。抽成纯函数：居中的行为可以用**假时间**单测，
/// 不用真起播放。
fn lyric_rows(lyric: &Lyric, position: Duration, height: usize) -> (Vec<LyricRow>, usize) {
    let current = lyric.line_at(position);
    let rows: Vec<LyricRow> = lyric
        .lines
        .iter()
        .enumerate()
        .flat_map(|(index, line)| {
            let current = Some(index) == current;
            let mut rows = vec![LyricRow {
                text: line.text.clone(),
                translation: false,
                current,
            }];
            if let Some(translation) = &line.translation {
                rows.push(LyricRow {
                    text: translation.clone(),
                    translation: true,
                    current,
                });
            }
            rows
        })
        .collect();
    // 当前行之前（不含它自己）占了多少行 → 减半屏就是滚动位置。
    let before = current.map_or(0, |index| {
        lyric.lines[..index]
            .iter()
            .map(|line| row_span(line.translation.is_some()))
            .sum()
    });
    let span = current.map_or(1, |index| row_span(lyric.lines[index].translation.is_some()));
    let scroll = centered_scroll(before, span, rows.len(), height);
    (rows, scroll)
}

impl App {
    /// `full = true` 时占满内容区（全屏歌词）。
    pub(super) fn draw_lyric(&self, frame: &mut Frame, area: Rect, full: bool) {
        let title = lyric_title(self.engine.snapshot().title.as_deref(), full);
        let block = Block::bordered().title(title);
        // 去掉上下边框之后真正能放歌词的行数。
        let height = area.height.saturating_sub(2) as usize;
        let Some(lyric) = &self.lyric else {
            let text = if self.lyric_loading {
                "歌词加载中…"
            } else {
                "暂无歌词"
            };
            // 竖着也居中：上面垫掉一半空行。
            let mut rows = vec![Line::default(); height.saturating_sub(1) / 2];
            rows.push(
                Line::from(text)
                    .alignment(Alignment::Center)
                    .style(Style::new().fg(Color::DarkGray)),
            );
            frame.render_widget(Paragraph::new(rows).block(block), area);
            return;
        };

        let (rows, scroll) = lyric_rows(lyric, self.engine.snapshot().position, height);
        let rows: Vec<Line> = rows
            .into_iter()
            .map(|row| {
                let style = match (row.current, row.translation) {
                    (true, _) => Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    (false, true) => Style::new().fg(Color::DarkGray),
                    (false, false) => Style::new(),
                };
                Line::from(row.text).style(style).alignment(Alignment::Center)
            })
            .collect();
        frame.render_widget(
            Paragraph::new(rows)
                .block(block)
                .scroll((scroll as u16, 0)),
            area,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::LyricLine;

    /// `count` 句歌词，每句间隔 10 秒；`translated` 每隔一句带一条翻译。
    fn lyrics(count: usize, translated: bool) -> Lyric {
        Lyric {
            lines: (0..count)
                .map(|index| LyricLine {
                    at: Duration::from_secs(index as u64 * 10),
                    text: format!("第 {} 句", index + 1),
                    translation: (translated && index % 2 == 1).then(|| format!("line {}", index + 1)),
                })
                .collect(),
        }
    }

    /// 标题：两种画法共用「歌词 · 歌名」，全屏只多一句退出提示；都不写行数。
    #[test]
    fn both_panel_and_fullscreen_share_the_same_title() {
        let song = Some("曲终人散 — 张宇");
        assert_eq!(lyric_title(song, false), " 歌词 · 曲终人散 — 张宇 ");
        assert_eq!(
            lyric_title(song, true),
            " 歌词 · 曲终人散 — 张宇（Y 或 Esc 退出） "
        );
        // 还没开始放（拿不到歌名）也不能出现空的一段或行数。
        assert_eq!(lyric_title(None, false), " 歌词 ");
        assert_eq!(lyric_title(Some(""), false), " 歌词 ");
        assert_eq!(lyric_title(None, true), " 歌词（Y 或 Esc 退出） ");
    }

    /// 整首歌、10 行窗口：当前句落在正中（上下各留 4 行，末行则是 4/5 分摊）。
    #[test]
    fn the_current_line_lands_in_the_middle() {
        let lyric = lyrics(40, false);
        // 第 20 句（下标 20）在播：前面 20 行，窗口 10 行 → 滚到 16。
        let (rows, scroll) = lyric_rows(&lyric, Duration::from_secs(205), 10);
        let current = rows.iter().position(|row| row.current).expect("有当前行");
        assert_eq!(current, 20, "当前句是第 20 句");
        assert_eq!(current - scroll, 4, "屏幕上它上方留 4 行，下面 5 行");
    }

    /// 带翻译的当前句占两行，居中时当成一整块。
    #[test]
    fn a_translated_current_line_counts_as_two_rows() {
        let lyric = lyrics(40, true);
        // 下标 21（句 22）带翻译：它前面 11 句里 5 句有翻译 → 16 行。
        let (rows, scroll) = lyric_rows(&lyric, Duration::from_secs(215), 10);
        let first = rows.iter().position(|row| row.current).expect("有当前行");
        assert!(rows[first + 1].translation && rows[first + 1].current, "翻译也算当前");
        assert_eq!(first - scroll, 4, "两行一起居中");
    }

    /// 开头 / 结尾夹住：不飘空行。
    #[test]
    fn the_ends_are_clamped() {
        let lyric = lyrics(40, false);
        let (_, scroll) = lyric_rows(&lyric, Duration::from_secs(0), 10);
        assert_eq!(scroll, 0, "第一句就在最上面");
        let (rows, scroll) = lyric_rows(&lyric, Duration::from_secs(400), 10);
        assert_eq!(scroll, rows.len() - 10, "最后一句贴底，不留一屏空行");
    }

    /// 窗口装得下全部歌词：永远不滚。
    #[test]
    fn a_window_taller_than_the_lyric_never_scrolls() {
        assert_eq!(centered_scroll(3, 1, 8, 20), 0);
        assert_eq!(centered_scroll(0, 1, 0, 0), 0, "零高度也不炸");
        let (_, scroll) = lyric_rows(&lyrics(3, false), Duration::from_secs(20), 30);
        assert_eq!(scroll, 0);
    }

    /// 前奏（还没到第一句的时间）不亮任何一行。
    #[test]
    fn the_intro_highlights_nothing() {
        let lyric = Lyric {
            lines: vec![LyricLine {
                at: Duration::from_secs(30),
                text: "迟到的第一句".to_string(),
                translation: None,
            }],
        };
        let (rows, scroll) = lyric_rows(&lyric, Duration::from_secs(1), 10);
        assert!(rows.iter().all(|row| !row.current), "前奏阶段不亮");
        assert_eq!(scroll, 0);
    }
}

//! 播放条：信息行 + 进度条，全宽固定四行。

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Gauge, Paragraph};

use crate::player::Phase;

use super::super::state::App;
use super::format_duration;

impl App {
    /// 播放条。四行：边框 + 信息行 + 进度条 + 边框。
    pub(super) fn draw_player(&self, frame: &mut Frame, player_area: Rect) {
        let snapshot = self.engine.snapshot();
        let state_text = match &snapshot.phase {
            Phase::Idle => "已停止".to_string(),
            Phase::Buffering => "缓冲中…".to_string(),
            Phase::Playing => "播放中".to_string(),
            Phase::Paused => "已暂停".to_string(),
            Phase::Failed(message) => format!("失败：{message}"),
        };
        // 那一格是「这份字节从哪来 + 什么档位 + 多大」：知道几样写几样。
        let copy = copy_label(&snapshot);
        let queue = if self.queue.is_empty() {
            format!("  ·  {}  ·  无队列", self.queue.mode().label())
        } else {
            format!(
                "  ·  {}  ·  {}/{}  ·  音量 {}%",
                self.queue.mode().label(),
                self.queue.index() + 1,
                self.queue.len(),
                snapshot.volume
            )
        };
        let title = snapshot.title.clone().unwrap_or_else(|| "—".to_string());
        let colors = Style::new().fg(Color::Cyan);
        let line = Line::from(vec![
            Span::styled("♪ ", colors),
            Span::styled(title, colors.add_modifier(Modifier::BOLD)),
            Span::raw(format!("  {copy} {state_text}{queue}")),
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
    }
}

/// 播放条上那一格：`[来源 档位 体积]`，知道几样写几样；一样都不知道就整格不画。
///
/// 来源（在线 / 缓存 / 下载 / 曲库）由 [`crate::player::open`] 按**实际打开的**那一份填，
/// 所以「缓存文件解不开、退回网络」那次报的是「在线」。档位只有缓存与 CDN 那份知道，
/// 本地文件读不出来就不写（只报体积），不瞎报一个。
fn copy_label(snapshot: &crate::player::Snapshot) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(source) = snapshot.source {
        parts.push(source.label().to_string());
    }
    if let Some(quality) = snapshot.quality {
        parts.push(quality.label().to_string());
    }
    if let Some(size) = snapshot.size {
        parts.push(super::format_size(size));
    }
    if parts.is_empty() {
        return String::new();
    }
    format!("[{}]", parts.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Quality;
    use crate::player::{PlaySource, Snapshot};

    fn snapshot(
        source: Option<PlaySource>,
        quality: Option<Quality>,
        size: Option<u64>,
    ) -> Snapshot {
        Snapshot {
            source,
            quality,
            size,
            ..Snapshot::default()
        }
    }

    /// 四种来源都写得出人话，档位/体积知道几样写几样。
    #[test]
    fn the_copy_label_says_where_the_bytes_came_from() {
        assert_eq!(
            copy_label(&snapshot(Some(PlaySource::Cache), Some(Quality::High320), Some(10_485_760))),
            "[缓存 MP3 320k 10.0MB]",
            "缓存那份：来源 + 档位（文件名里带）+ 真实大小"
        );
        assert_eq!(
            copy_label(&snapshot(Some(PlaySource::Download), None, Some(3_774_873))),
            "[下载 3.6MB]",
            "下载那份读不出档位，就别瞎报一个"
        );
        assert_eq!(
            copy_label(&snapshot(Some(PlaySource::Online), Some(Quality::Low128), Some(4_404_019))),
            "[在线 MP3 128k 4.2MB]"
        );
        assert_eq!(
            copy_label(&snapshot(Some(PlaySource::Library), None, Some(31_457_280))),
            "[本地 30.0MB]"
        );
    }

    /// 空态：起播前（`Buffering`）一样都没有，那一格整格不画（不是画一对空方括号）。
    #[test]
    fn an_empty_copy_draws_nothing() {
        assert_eq!(copy_label(&snapshot(None, None, None)), "");
        // 少见但合法：只拿到大小（比如以后有源不报档位又还没定来源）。
        assert_eq!(copy_label(&snapshot(None, None, Some(1_048_576))), "[1.0MB]");
    }
}

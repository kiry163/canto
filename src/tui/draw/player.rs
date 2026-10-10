//! 播放条：信息行 + 进度条，全宽固定四行。

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Gauge, Paragraph};

use crate::player::Phase;

use super::super::state::App;
use super::{format_duration, format_size};

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
    }
}

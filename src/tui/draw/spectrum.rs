//! 频谱柱：底对齐的柱子，柱高按 1/8 格取整，柱顶之上有一条落得更慢的峰值刻度。
//!
//! 频段数（[`crate::spectrum::BANDS`]）通常多于终端列数，所以先把若干频段压成一根
//! 柱子，压的时候取**最大值**而不是平均值——平均会把鼓点那一下的尖峰抹平，柱子就
//! 只剩一团钝钝的起伏。

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

/// 柱顶收口用的竖直填充块：1/8 …… 8/8。
///
/// 只用 `█` 的话柱高被量化到整行，低矮的柱子会在「空 / 满」之间整行跳变；用这一组
/// 字符，柱顶能落在一格之内的任意 1/8 处。
const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// 每格切成 8 个刻度。`height` 行的柱区就是 `height * 8` 个刻度。
const EIGHTHS: usize = 8;

/// 柱体最多占几列。宽屏下多出来的列全给空隙，柱子才不成一块块方砖。
const MAX_BODY: usize = 2;

/// 柱体颜色。与播放条的进度条同色：语义都是「电平」。
const BAR: Color = Color::Green;

/// 峰值刻度用终端默认前景色：必须与柱体不同色，否则那条浮线会被读成柱子的一部分。
const PEAK: Color = Color::Reset;

/// 画频谱块（含边框与标题）。`bands` / `peaks` 为空时只画框。
pub(super) fn draw_spectrum(frame: &mut Frame, area: Rect, bands: &[f32], peaks: &[f32], title: &str) {
    let block = Block::bordered().title(title.to_string());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 || bands.is_empty() {
        return;
    }
    bars(frame, inner, bands, peaks);
}

/// 没有数据可画时：一块只有标题的框，中间一句话。
///
/// 不画残留的柱子——暂停时 rodio 不再吐采样，缓冲会冻在最后一帧，画出来就是
/// 「看着在播却不出声」。竖着垫掉一半空行，视觉重心才不在顶上。
pub(super) fn draw_spectrum_message(frame: &mut Frame, area: Rect, title: &str, message: &str) {
    let block = Block::bordered().title(title.to_string());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut lines = vec![Line::default(); inner.height.saturating_sub(1) as usize / 2];
    lines.push(Line::from(Span::styled(
        message,
        Style::new().fg(Color::DarkGray),
    )));
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
}

/// 柱区：每根柱子一行一个 `Span`（柱体同宽同色，一格 `Span` 就够）。
fn bars(frame: &mut Frame, area: Rect, bands: &[f32], peaks: &[f32]) {
    let height = area.height as usize;
    let stride = bar_stride(area.width as usize, bands.len());
    let count = (area.width as usize / stride).min(bands.len());
    if count == 0 {
        return;
    }
    // 柱体封顶，步长里多出来的列全给空隙
    let body = stride.saturating_sub(1).clamp(1, MAX_BODY);
    let gap = stride.saturating_sub(body);

    let columns = aggregate(bands, count);
    let caps = aggregate(peaks, count);
    let marks: Vec<(usize, usize)> = columns
        .iter()
        .zip(&caps)
        .map(|(&level, &cap)| mark(level, cap, height))
        .collect();

    // 柱子整块居中：靠左铺的话右边会空出一条，看着像图没画完。宽度有余量时两侧对称留白。
    let used = (count - 1) * stride + body;
    let pad = (area.width as usize).saturating_sub(used) / 2;

    let mut lines = Vec::with_capacity(height);
    for row in 0..height {
        // 从底部数起的行号，用来判断这一格要不要点亮
        let from_bottom = height - row;
        let mut spans: Vec<Span> = Vec::with_capacity(count + 1);
        if pad > 0 {
            spans.push(Span::raw(" ".repeat(pad)));
        }
        for (index, &(filled, peak_cell)) in marks.iter().enumerate() {
            let (glyph, color) = cell(from_bottom, filled, peak_cell);
            let mut chunk = String::with_capacity(body + gap);
            for _ in 0..body {
                chunk.push(glyph);
            }
            // 柱间空隙。最后一根后面不留，否则右边会多出一列空白
            if index + 1 < count {
                for _ in 0..gap {
                    chunk.push(' ');
                }
            }
            spans.push(Span::styled(chunk, Style::new().fg(color)));
        }
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// 每根柱子占几列：由可用宽度与频段数算出来，而不是写死。
///
/// * 铺满宽度：柱子只占左边一小段、右边大片空着，看着像图没画完。
/// * 柱子之间要有空隙：铺满每一列的话相邻柱子会粘成一整片，看不出是一根根柱子。
fn bar_stride(area_width: usize, bands: usize) -> usize {
    if bands == 0 {
        return 1;
    }
    let fit = area_width / bands;
    if fit >= 2 {
        return fit;
    }
    // fit <= 1：柱子只能 1 列宽。此时若还有余量，走 2 列步长换取空隙 + 铺满
    if area_width > bands { 2 } else { 1 }
}

/// 把 `values` 压成 `count` 根柱子，每根取组内最大值（不抹平尖峰）。
fn aggregate(values: &[f32], count: usize) -> Vec<f32> {
    (0..count)
        .map(|bar| {
            let start = bar * values.len() / count;
            let end = ((bar + 1) * values.len() / count).max(start + 1);
            values[start..end.min(values.len())]
                .iter()
                .fold(0.0f32, |max, &value| max.max(value))
                .clamp(0.0, 1.0)
        })
        .collect()
}

/// 一根柱子的刻度：柱体高度（1/8 格为单位）与峰值所在的行（从底部数，0 = 不画）。
fn mark(level: f32, cap: f32, height: usize) -> (usize, usize) {
    let scale = (height * EIGHTHS) as f32;
    let filled = (level.clamp(0.0, 1.0) * scale).round() as usize;
    let peak = (cap.clamp(0.0, 1.0) * scale).round() as usize;

    // 峰值只在**高过柱顶一整格**时才画：否则它会和柱顶撞在同一格里，要么被柱体
    // 盖住、要么糊成一团。柱顶追上去时峰值消失、回落时再浮现，正是频谱仪该有的行为。
    let fill_cell = filled.div_ceil(EIGHTHS);
    let peak_cell = peak.div_ceil(EIGHTHS);
    (filled, if peak_cell > fill_cell { peak_cell } else { 0 })
}

/// 一格该画什么字符、什么颜色（`from_bottom` 从 1 数起）。
fn cell(from_bottom: usize, filled: usize, peak_cell: usize) -> (char, Color) {
    let lo = (from_bottom - 1) * EIGHTHS;
    let hi = from_bottom * EIGHTHS;
    if filled >= hi {
        ('█', BAR)
    } else if filled > lo {
        // 柱顶落在这一格之内：取对应高度的填充块收口
        (BLOCKS[filled - lo - 1], BAR)
    } else if peak_cell == from_bottom {
        ('▔', PEAK)
    } else {
        (' ', PEAK)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// 画一块频谱并拿回缓冲区。
    ///
    /// 柱子位置**不能写死列号**：柱体宽度与两侧留白都是由可用宽度算出来的
    /// （`bar_stride` + 居中），写死列号会在调宽度时莫名其妙地挂。
    fn bars_of(width: u16, height: u16, bands: &[f32], peaks: &[f32]) -> ratatui::buffer::Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("测试终端");
        terminal
            .draw(|frame| draw_spectrum(frame, Rect::new(0, 0, width, height), bands, peaks, "频谱"))
            .expect("绘制");
        terminal.backend().buffer().clone()
    }

    /// 数一行里有几段被点亮的柱子。
    fn lit_groups(buffer: &ratatui::buffer::Buffer, width: u16, row: u16) -> usize {
        let (mut groups, mut previous) = (0, false);
        for x in 0..width {
            let lit = buffer[(x, row)].symbol() == "█";
            if lit && !previous {
                groups += 1;
            }
            previous = lit;
        }
        groups
    }

    /// 某一列从底往上的柱体格子（`▁▂▃▄▅▆▇█` 都算），遇到第一个空就停。
    fn column(buffer: &ratatui::buffer::Buffer, x: u16, bottom: u16, top: u16) -> String {
        let mut text = String::new();
        for y in (top..=bottom).rev() {
            let symbol = buffer[(x, y)].symbol();
            if symbol == " " {
                break;
            }
            text.push_str(symbol);
        }
        text
    }

    #[test]
    fn mark_quantises_to_eighths() {
        // 柱区 3 行 = 24 个刻度：0.5 → 12 刻度（底部满格 + 第二行半格）
        assert_eq!(mark(0.5, 0.0, 3), (12, 0));
        // 峰值低过柱顶时不该画（否则会和柱顶糊在一起）
        assert_eq!(mark(0.5, 0.5, 3), (12, 0));
        // 峰值高过柱顶一整格才画
        assert_eq!(mark(0.25, 0.75, 3), (6, 3));
    }

    #[test]
    fn cell_picks_the_sub_cell_block() {
        // filled = 12、柱区 3 行：最底一格满、第二格 4/8、第三格空
        assert_eq!(cell(1, 12, 0), ('█', BAR));
        assert_eq!(cell(2, 12, 0), ('▄', BAR));
        assert_eq!(cell(3, 12, 0), (' ', PEAK));
        // 峰值所在的那一格画 `▔`
        assert_eq!(cell(3, 6, 3), ('▔', PEAK));
    }

    #[test]
    fn bar_height_follows_the_level() {
        // 柱区 4 行：满格那根四行全亮，半格那根只亮两行
        let buffer = bars_of(20, 6, &[1.0, 0.0, 0.5], &[0.0, 0.0, 0.0]);
        // 空的那根把两段柱子隔开，所以从最底一行数出两段亮块
        assert_eq!(lit_groups(&buffer, 20, 4), 2, "1.0 与 0.5 两根该亮");

        // 第一根（满格）那一列：4 行全亮
        let full_x = (0..20u16)
            .find(|&x| buffer[(x, 1)].symbol() == "█")
            .expect("满格柱子的最顶一行");
        assert_eq!(column(&buffer, full_x, 4, 1), "████");

        // 半格那根：底部两行亮、第三行空
        let half_x = (0..20u16)
            .rfind(|&x| buffer[(x, 3)].symbol() == "█")
            .expect("半格柱子");
        assert_eq!(column(&buffer, half_x, 4, 1), "██");
    }

    #[test]
    fn the_bar_top_uses_sub_cell_blocks() {
        // 柱区 3 行 = 24 个刻度。0.5 → 12 刻度 → 底部一行满格、第二行 4/8 块
        let buffer = bars_of(8, 5, &[0.5], &[0.0]);
        let x = (0..8u16)
            .find(|&x| buffer[(x, 3)].symbol() == "█")
            .expect("柱子");
        assert_eq!(column(&buffer, x, 3, 1), "█▄");
    }

    #[test]
    fn the_peak_marker_sits_above_the_bar() {
        // 柱子 0.25（6 刻度 → 底格 6/8 块），峰值 0.75（18 刻度）→ 峰值画在从底数第 3 格，
        // 与柱顶之间隔着一格空白（`column` 那种自底向上、遇空即停的读法跨不过这一格）。
        let buffer = bars_of(8, 5, &[0.25], &[0.75]);
        let x = (0..8u16)
            .find(|&x| buffer[(x, 1)].symbol() == "▔")
            .expect("峰值刻度");
        assert_eq!(buffer[(x, 3)].symbol(), "▆", "柱顶是 6/8 块");
        assert_eq!(buffer[(x, 2)].symbol(), " ", "柱与峰值之间是空的");
        assert_eq!(buffer[(x, 1)].symbol(), "▔", "峰值刻度在柱顶之上");
    }

    #[test]
    fn bar_count_follows_the_band_count() {
        let bands = vec![1.0f32; 6];
        let buffer = bars_of(60, 6, &bands, &bands);
        assert_eq!(lit_groups(&buffer, 60, 4), 6, "柱子数应当等于频段数");
    }

    #[test]
    fn nothing_is_drawn_without_bands() {
        let buffer = bars_of(20, 6, &[], &[]);
        assert_eq!(buffer[(1, 1)].symbol(), " ", "没有数据时柱区应当是空的");
        assert!(
            (0..20u16).any(|x| buffer[(x, 0)].symbol() == "┌"),
            "框还是要画"
        );
    }

    #[test]
    fn a_message_is_centered_when_there_is_nothing_to_show() {
        let mut terminal = Terminal::new(TestBackend::new(40, 8)).expect("测试终端");
        terminal
            .draw(|frame| {
                draw_spectrum_message(frame, Rect::new(0, 0, 40, 8), " 频谱 ", "未在播放");
            })
            .expect("绘制");
        let buffer = terminal.backend().buffer();

        let row = (0..8u16)
            .find(|&y| (0..40u16).any(|x| buffer[(x, y)].symbol() == "未"))
            .expect("提示文案");
        assert_eq!(row, 3, "6 行内区里 2 行垫空，文案落在第 3 行（大致居中）");
        assert!(
            !(0..40u16).any(|x| (0..8u16).any(|y| buffer[(x, y)].symbol() == "█")),
            "提示态不该有柱子"
        );
    }
}

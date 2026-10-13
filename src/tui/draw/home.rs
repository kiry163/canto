//! 首页（`0` 界面）：屏幕入口的 3 列网格，`j`/`k` 选、`Enter` 进、数字 `1`–`4` 直跳。
//!
//! 它只画内容区那一块——播放条、状态行、歌词栏、浮层都由主界面那套外壳负责（首页不是
//! 另一套界面，只是内容区换了东西）。格由 `Menu::ALL` 生成，编号就是下标 + 1，按 3 列
//! 换行（`ceil(n/3)` 行），整块在内容区里居中；选中只是前面那个 `▶ `，不涂底色。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Padding, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::super::state::{App, Menu};
use super::panel_block;

/// 网格列数：条目按它换行（4 项 = 3 + 1）。
const COLUMNS: usize = 3;
/// 格子高：上框 + 一行文字 + 下框。
const CELL_HEIGHT: u16 = 3;
/// 同一行里格子之间空几列。
const COLUMN_GAP: u16 = 2;
/// 行与行之间空几行。
const ROW_GAP: u16 = 1;

impl App {
    pub(super) fn draw_home(&mut self, frame: &mut Frame, area: Rect) {
        let labels: Vec<String> = Menu::menus(self.local_features_on())
            .iter()
            .enumerate()
            .map(|(index, menu)| format!("{}  {}", index + 1, menu.title()))
            .collect();
        // 格子等宽：`▶ ` 两格 + 最宽那项 + 左右各一格内边距 + 两条边框。等宽才好对列。
        let widest = labels.iter().map(|label| label.width()).max().unwrap_or(0) as u16;
        let cell_width = widest + 2 /* ▶  */ + 2 /* 内边距 */ + 2 /* 边框 */;
        let selected = self.home_state.selected().unwrap_or(usize::MAX);
        let panel = panel_block(" 首页 ");
        let cells = grid_cells(panel.inner(area), cell_width, labels.len());
        frame.render_widget(panel, area);
        for (index, cell) in cells.into_iter().enumerate() {
            // 选中只靠前面那个 `▶ `：不涂底色、也不改颜色（等宽格子里补两格空格，文字才对得齐）。
            let marker = if index == selected { "▶ " } else { "  " };
            let block = Block::bordered().padding(Padding::horizontal(1));
            frame.render_widget(
                Paragraph::new(Line::from(format!("{marker}{}", labels[index]))).block(block),
                cell,
            );
        }
    }
}

/// 每一格的矩形：按 [`COLUMNS`] 列换行，整块（连同格间留空）在 `inner` 里居中。
///
/// 装不下时按可用空间夹住——越界那部分由 ratatui 裁掉，不 panic（窗口过小本来就有
/// `too_small` 那一行提示兜着）。
fn grid_cells(inner: Rect, cell_width: u16, count: usize) -> Vec<Rect> {
    let columns = count.clamp(1, COLUMNS) as u16;
    let rows = count.div_ceil(COLUMNS).max(1) as u16;
    let block_width = columns * cell_width + (columns - 1) * COLUMN_GAP;
    let block_height = rows * CELL_HEIGHT + (rows - 1) * ROW_GAP;
    let x = inner.x + inner.width.saturating_sub(block_width) / 2;
    let y = inner.y + inner.height.saturating_sub(block_height) / 2;
    (0..count)
        .map(|index| {
            let (column, row) = (index % COLUMNS, index / COLUMNS);
            Rect::new(
                x + column as u16 * (cell_width + COLUMN_GAP),
                y + row as u16 * (CELL_HEIGHT + ROW_GAP),
                cell_width,
                CELL_HEIGHT,
            )
        })
        .collect()
}

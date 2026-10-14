//! 绘制：骨架布局与各面板。
//!
//! 骨架（内容区 / 歌词 / 播放条 / 状态行）、断点与共用的小工具都在这里；
//! 各面板自己的画法在子模块里。状态与按键在 [`super::state`]。

use std::borrow::Cow;
use std::time::Duration as StdDuration;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Paragraph};
use unicode_width::UnicodeWidthStr;

pub mod content;
pub mod home;
pub mod lyric;
pub mod overlay;
pub mod player;
pub mod rows;
pub mod spectrum;
pub mod status;

use crate::player::Phase;

use super::keys;
use super::state::{App, Menu, Screen};

/// 低于这个高度就不展开歌词栏（矮屏里歌词会把主区挤成一两行）。
pub const MIN_LYRIC_HEIGHT: u16 = 20;
/// 窗口小于这个尺寸就只画一行提示。
pub const MIN_WIDTH: u16 = 60;
pub const MIN_HEIGHT: u16 = 15;
/// 歌词栏高度上限（行）。
pub const MAX_LYRIC_HEIGHT: u16 = 12;

/// 骨架切出来的几块。
pub struct Areas {
    pub content: Rect,
    pub lyric: Option<Rect>,
    pub player: Rect,
    pub status: Rect,
}

/// 切骨架。`lyric_wanted` 由调用方判断（有没有在放 + 歌词栏开没开）。
///
/// 菜单栏去掉后左右只有一列，宽度不再是变量：内容区吃满全宽，歌词栏在最下面。
pub fn layout(area: Rect, lyric_wanted: bool) -> Areas {
    let rows = Layout::vertical([
        Constraint::Min(3),
        // 播放条：上下边框各 1 行 + 标题行 + 进度条行。
        Constraint::Length(4),
        Constraint::Length(1),
    ])
    .split(area);
    let body = rows[0];
    let lyric_height = if lyric_wanted && area.height >= MIN_LYRIC_HEIGHT {
        (body.height / 3).clamp(5, MAX_LYRIC_HEIGHT)
    } else {
        0
    };
    let inner =
        Layout::vertical([Constraint::Min(3), Constraint::Length(lyric_height)]).split(body);
    Areas {
        content: inner[0],
        lyric: (lyric_height > 0).then_some(inner[1]),
        player: rows[1],
        status: rows[2],
    }
}

/// 居中弹层（队列浮层 / 帮助浮层 / 歌单选择框共用）。
pub fn overlay_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(4)).max(1);
    let height = height.min(area.height.saturating_sub(2)).max(1);
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

pub fn too_small(area: Rect) -> bool {
    area.width < MIN_WIDTH || area.height < MIN_HEIGHT
}

impl App {
    pub(super) fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        if let Screen::Login(state) = &self.screen {
            super::login::draw(state, frame, area);
            return;
        }
        if too_small(area) {
            frame.render_widget(
                ratatui::widgets::Paragraph::new(format!(
                    "终端太小，请拉大到 ≥{}×{}（当前 {}×{}）",
                    MIN_WIDTH, MIN_HEIGHT, area.width, area.height
                ))
                .alignment(ratatui::layout::Alignment::Center),
                area,
            );
            return;
        }
        // 歌词栏在「开着 + 有歌在放」时占位（有没有歌词都画：没有就写「暂无歌词」）。
        let lyric_wanted = self.show_lyric && self.player_active();
        let areas = layout(area, lyric_wanted);
        if self.show_spectrum {
            // 两个栏都在时歌名归歌词栏（两块都挂一遍没意义）；频谱区独占内容区时
            // 名字得跟着它走，不然屏幕上没有歌名。
            let title = if areas.lyric.is_some() {
                SPLIT_TITLE.to_string()
            } else {
                spectrum_title(self.engine.snapshot().title.as_deref())
            };
            // 频谱区替掉列表，但**筛选输入行照旧**：输入态不属于列表（`/` 在频谱区开着时
            // 也能按），它就占内容区顶上那一行——不画出来的话打进去的字看不见。
            let area = if self.filter_input {
                let rows =
                    Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(areas.content);
                self.draw_filter_input(frame, rows[0]);
                rows[1]
            } else {
                areas.content
            };
            self.draw_spectrum_area(frame, area, &title);
        } else if matches!(self.screen, Screen::Home) {
            // 首页先判：`go_home()` 只动 `screen`，`nav.menu` 还记着上一屏（比如刚从「本地曲库」
            // 按 `Esc` 回来）——本地曲库那条分支排在它前面的话，状态行提示是首页那套、
            // 画面却还是曲库那屏，看着就像「`Esc` 没反应」。
            self.draw_home(frame, areas.content);
        } else if self.nav.menu == Menu::LocalLibrary {
            // 本地曲库：列表来自索引，**不发任何请求**；空曲库给配置引导，扫描中带进度。
            if self.library.is_empty() {
                self.draw_empty_library(frame, areas.content);
            } else {
                let title = if self.library_scanning {
                    format!(" 本地曲库 · 扫描中 {} 首 ", self.library.len())
                } else {
                    format!(" 本地曲库 · {} 首 ", self.library.len())
                };
                self.draw_songs(frame, areas.content, &title);
            }
        } else {
            self.draw_content(frame, areas.content);
        }
        if let Some(lyric) = areas.lyric {
            self.draw_lyric(frame, lyric);
        }
        self.draw_player(frame, areas.player);
        self.draw_status(frame, areas.status);
        // 浮层只盖**内容区**：播放条与状态行是常驻的，不该被挡住。
        if self.help_open {
            self.draw_help(frame, area);
        } else if self.queue_open {
            self.draw_queue(frame, areas.content);
        } else if self.local_picker.is_some() {
            self.draw_local_picker(frame, area);
        } else if self.picker.is_some() {
            self.draw_picker(frame, area);
        }
    }

    /// 音频引擎是否已经离开 `Idle`（有歌在放、缓冲或暂停）。
    fn player_active(&self) -> bool {
        self.engine.snapshot().phase != Phase::Idle
    }

    /// 画频谱那一块：有数据就画柱子，没有就给提示。
    ///
    /// 「有没有数据」**不按播放态判断，按数据本身判断**：采样窗口只在
    /// `spectrum_wanted()` 为真时才更新，其余时候 `tick()` 会把平滑与峰值清空，
    /// 所以「空」就等于「现在没有真实电平可画」。这样暂停时不会留下上一帧的柱子，
    /// 而正在播但此刻真是静音的歌，画出来的是一条平的柱子（不是提示）。
    fn draw_spectrum_area(&self, frame: &mut Frame, area: Rect, title: &str) {
        if self.spectrum_smooth.is_empty() {
            let message = spectrum_message(&self.engine.snapshot().phase);
            spectrum::draw_spectrum_message(frame, area, title, message);
            return;
        }
        spectrum::draw_spectrum(
            frame,
            area,
            &self.spectrum_smooth,
            &self.spectrum_peak,
            title,
        );
    }
}

/// 状态行左侧的键位提示：按当前上下文给三到五个，只写**真的接了行为**的键。
pub fn hints(context: keys::Context, local_on: bool) -> &'static str {
    match context {
        keys::Context::Content => {
            "Enter 播放 · Space 暂停 · n/b 切歌 · / 筛选 · p 加歌单 · ? 帮助"
        }
        // 首页没有「选中的歌」：提示里只给进屏 / 切屏与播放这几条。数字直跳的上限跟着
        // 「本地功能开没开」走（`dirs = []` 时只有四屏）。
        keys::Context::Home => {
            if local_on {
                "Enter 进入 · 1-6 直跳 · Space 暂停 · n/b 切歌 · u 队列 · ? 帮助"
            } else {
                "Enter 进入 · 1-4 直跳 · Space 暂停 · n/b 切歌 · u 队列 · ? 帮助"
            }
        }
        keys::Context::SearchInput => "输入关键词 · Enter 搜索 · Esc 退出输入",
        keys::Context::FilterInput => "输入即筛选 · Enter 确认 · Esc 清空",
        keys::Context::LocalInput => "输入歌单名 · Enter 确认 · Esc 放弃",
        keys::Context::PlaylistPicker => "j/k 选歌单 · Enter 确认 · Esc 取消",
        // 帮助浮层自己不再带底栏：关闭 / 滚动在这条状态行提示里；**登出不写**——它已经
        // 是全局键，浮层「全局」段里有它那一行，状态行只留这会儿最顺手的几个。
        keys::Context::HelpOverlay => "Esc/? 关闭 · j/k ↑/↓ 滚动 · g/G 首尾",
        keys::Context::QueueOverlay => {
            "Enter 跳播 · d 删除 · Shift+D 清空 · j/k 选择 · Esc 关闭"
        }
        keys::Context::Login => "c 粘贴 cookie · o 打开二维码图片 · r 重新扫码 · q 退出"
    }
}

/// 内容块：只有一块内容区，所以不再有「焦点在这块」的黄边框。
fn panel_block(title: &str) -> Block<'static> {
    Block::bordered().title(title.to_string())
}

/// 歌单行：`{:>5}  {名字}`。四处（QQ/本地歌单列表、两个选择框）共用这一份，缩进口径只有一处。
///
/// `count` 给 `None` 就是选择框里那行「新建…」——计数那 5 列留白，跟歌单行同一列起名字。
pub(super) fn playlist_row(count: Option<u64>, name: &str) -> String {
    format!("{:>5}  {name}", count.map_or_else(String::new, |count| count.to_string()))
}

/// 空态占位：`DarkGray` 居中 + 带标题的框（空本地歌单 / 空歌单 / 空历史三处共用）。
fn empty_panel(text: &str, title: &str) -> Paragraph<'static> {
    Paragraph::new(text.to_string())
        .style(Style::new().fg(Color::DarkGray))
        .alignment(Alignment::Center)
        .block(panel_block(title))
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

/// 按**显示宽度**截断（中文、书名号都是双宽），超出就用省略号收尾。
///
/// 固定列宽下必须截：整条画不完时至少让人看出「被裁了」，而不是话说到一半。
/// 状态行的通知位（`status.rs`）与曲目列表的列模型（`rows.rs`）都用它。
pub(super) fn truncate_to_cols(text: &str, cols: u16) -> Cow<'_, str> {
    let cols = cols as usize;
    if text.width() <= cols {
        return Cow::Borrowed(text);
    }
    let mut kept = String::new();
    let mut used = 0;
    for character in text.chars() {
        // 留一列给省略号本身。
        let width = unicode_width::UnicodeWidthChar::width(character).unwrap_or(0);
        if used + width > cols.saturating_sub(1) {
            break;
        }
        kept.push(character);
        used += width;
    }
    kept.push('…');
    Cow::Owned(kept)
}

/// 频谱区没数据可画时那句话。缓冲与「没在播」要分开报——前者等一会儿就有了。
fn spectrum_message(phase: &Phase) -> &'static str {
    match phase {
        Phase::Buffering => "缓冲中…",
        // Idle / Paused / Failed / Playing（刚起播、窗口还没填满）
        _ => "未在播放",
    }
}

/// 拆两半时频谱区的标题：歌名在下方的「歌词 · 歌名」里已经出现一次，这里只报
/// 「频谱」——标题只报「这是什么」。
const SPLIT_TITLE: &str = " 频谱 ";

/// 整块归频谱时的标题：歌名得带上，因为下方没有歌词栏了。
fn spectrum_title(song: Option<&str>) -> String {
    match song.filter(|song| !song.is_empty()) {
        Some(song) => format!(" 频谱 · {song} "),
        None => SPLIT_TITLE.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use unicode_width::UnicodeWidthStr;

    use crate::tui::queue;
    use crate::tui::state::Menu;
    use crate::tui::test_support::{test_app, test_song};

    /// 本地歌单屏：一张都没有时给入口；进去看空歌单有话说；文件没了的条目照样列出来、
    /// 并且标「（已丢失）」。
    #[test]
    fn the_local_playlist_screen_paints_empty_and_lost_entries() {
        let (mut app, _runtime) = test_app();
        app.switch_menu(Menu::LocalPlaylists);
        let empty = frame_rows(&mut app, 100, 24).join("\n");
        assert!(empty.contains("还没有本地歌单"), "{empty:?}");

        app.playlists_store.create("通勤").expect("建歌单");
        app.switch_menu(Menu::LocalPlaylists);
        let listed = frame_rows(&mut app, 100, 24).join("\n");
        assert!(listed.contains("通勤"), "{listed:?}");

        app.apply(crate::tui::keys::Action::Enter);
        let opened = frame_rows(&mut app, 100, 24).join("\n");
        assert!(opened.contains("空歌单"), "{opened:?}");

        // 条目在，文件（与曲库）里没有：列表照样画这一行，后面缀一句「（已丢失）」。
        app.apply(crate::tui::keys::Action::Back);
        app.playlists_store.add(
            0,
            &crate::library::entry_to_song(&crate::library::Entry {
                path: std::path::PathBuf::from("C:/m/gone.wav"),
                mtime: 0,
                size: 0,
                title: "走远了".to_string(),
                artist: "人".to_string(),
                album: String::new(),
                duration_secs: 0,
            }),
        );
        app.switch_menu(Menu::LocalPlaylists);
        app.apply(crate::tui::keys::Action::Enter);
        let lost = frame_rows(&mut app, 100, 24).join("\n");
        assert!(lost.contains("走远了"), "{lost:?}");
        assert!(lost.contains("（已丢失）"), "{lost:?}");
    }

    /// `dirs = []` 时首页只有四格，提示里的直跳上限也跟着退到 `1-4`。
    #[test]
    fn the_home_grid_drops_the_local_cells_when_they_are_off() {
        let (mut app, _runtime) = test_app();
        app.config.library.dirs = Some(Vec::new());
        app.go_home();
        let text = frame_rows(&mut app, 120, 40).join("\n");
        assert!(text.contains("1在线收藏"), "{text:?}");
        assert!(text.contains("4播放历史"), "{text:?}");
        assert!(!text.contains("本地曲库"), "第 5 格不该画：{text:?}");
        assert!(!text.contains("本地歌单"), "第 6 格不该画：{text:?}");
        assert!(text.contains("1-4直跳"), "{text:?}");
    }

    /// 标题的层级判断按**当前屏**取：在 `6` 上不该拿 QQ 那一层的状态当自己的层级。
    #[test]
    fn the_local_playlist_title_ignores_the_qq_layer() {
        let (mut app, _runtime) = test_app();
        app.playlists_store.create("通勤").expect("建歌单");
        app.nav
            .push_tracks("id", "QQ 歌单", vec![test_song("m1")]);
        app.switch_menu(Menu::LocalPlaylists);
        let text = frame_rows(&mut app, 100, 24).join("\n");
        assert!(text.contains("本地歌单1张"), "{text:?}");
        assert!(!text.contains("本地歌单0首"), "别把 QQ 那一层当成本地层级：{text:?}");
    }

    /// 把一帧按行铺开（每行去空白），要比较「谁在上面」时用得上。
    fn frame_rows(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("测试终端");
        terminal.draw(|frame| app.draw(frame)).expect("绘制");
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .chars()
                    .filter(|character| !character.is_whitespace())
                    .collect()
            })
            .collect()
    }

    /// 原样铺开（保留空格、按**显示宽度**拼）的每一行。
    ///
    /// 对齐类断言只能用它：`frame_flat` 会把空格一起吃掉——「说明从第几列开始」这种问题它
    /// 答不了；而直接拼缓冲区单元格会把宽字符拼成「我 的 收 藏」（它占两格、第二格是空的），
    /// 所以这里跳过那第二格，`find` 给出的列号才是显示列。
    fn frame_raw(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("测试终端");
        terminal.draw(|frame| app.draw(frame)).expect("绘制");
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                let mut row = String::new();
                let mut x = 0;
                while x < width {
                    let symbol = buffer[(x, y)].symbol();
                    row.push_str(symbol);
                    x += if symbol.width() >= 2 { 2 } else { 1 };
                }
                row
            })
            .collect()
    }

    /// 帮助浮层那几行（含上下框线）：判「浮层里有什么 / 状态行里有什么」用。
    ///
    /// 按**标题**找顶边，不按 `┌ 帮助`：浮层左上角那一格可能正好落在下面某行的中文字
    /// 后半格上——ratatui 的缓冲约定是「宽字符后面那一格不再重画」（见 `Buffer::diff` 的
    /// 「double width symbol at index 0 - skip index 1」），那时候角就画不出来。
    fn help_box_rows(rows: &[String]) -> Vec<String> {
        let top = rows.iter().position(|row| row.contains("帮助")).expect("帮助浮层");
        let bottom = rows[top + 1..]
            .iter()
            .position(|row| row.contains('└'))
            .map_or(rows.len() - 1, |offset| top + 1 + offset);
        rows[top..=bottom].to_vec()
    }

    /// `needle` 在一个**显示列**从第几列开始。
    ///
    /// 不能用 `find` 的返回值直接比：那是字节偏移，而键列里有 `、`、`↑`、`↓` 这些多字节字符
    /// ——两行的字节偏移一样，显示列却不一样。
    fn column_of(row: &str, needle: &str) -> usize {
        row.find(needle).map(|index| row[..index].width()).expect("该在那儿")
    }

    /// 一行里某个字符出现的所有**显示列**（宽字符按两列算）。
    fn columns_of(row: &str, needle: char) -> Vec<usize> {
        let mut columns = Vec::new();
        let mut column = 0;
        for character in row.chars() {
            if character == needle {
                columns.push(column);
            }
            column += character.to_string().width();
        }
        columns
    }

    /// 格子顶边框那一行：内容区自己的边框只有一次 `┌`，网格一行至少两格。
    fn cell_border_row(rows: &[String]) -> &String {
        rows.iter().find(|row| row.matches('┌').count() >= 2).expect("格子的顶边框")
    }

    /// 内容区（`layout` 切出来的那一块）面板边框**里面**那块。
    fn content_inner(width: u16, height: u16) -> Rect {
        let content = layout(Rect::new(0, 0, width, height), false).content;
        Rect::new(content.x + 1, content.y + 1, content.width - 2, content.height - 2)
    }

    /// 内容区面板里头那几行（不含它自己的上下边框）。
    fn body_rows(rows: &[String], inner: Rect) -> &[String] {
        &rows[inner.y as usize..(inner.y + inner.height) as usize]
    }

    /// 一帧里的**文字**：空白与边框线都去掉，内容类断言用它。
    fn frame_text(app: &mut App, width: u16, height: u16) -> String {
        frame_flat(app, width, height)
            .chars()
            .filter(|character| !"│─┌┐└┘▶♪".contains(*character))
            .collect()
    }

    /// 把一帧铺成字符串并去掉所有空白。
    ///
    /// 去空白是必要的：宽字符在缓冲区里占两个单元格，第二个是空的，
    /// 拼起来就是「我 的 收 藏」；去空白后既好断言也不丢信息（这些用例
    /// 关心的是「画了什么」，不是「第几列」）。
    fn frame_flat(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("测试终端");
        terminal.draw(|frame| app.draw(frame)).expect("绘制");
        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..height {
            for x in 0..width {
                text.push_str(buffer[(x, y)].symbol());
            }
        }
        text.chars().filter(|character| !character.is_whitespace()).collect()
    }

    #[test]
    fn the_breadcrumb_and_the_status_row_share_the_frame() {
        let (mut app, _runtime) = test_app();
        app.nav.switch_menu(Menu::Playlists);
        app.nav.push_tracks("1", "嗯", vec![test_song("a")]);
        app.notify("已加载《嗯》");
        let text = frame_flat(&mut app, 160, 40);
        assert!(text.contains("在线歌单›嗯"), "面包屑要带层级：{text}");
        // 菜单栏没了：整帧里不该再出现菜单那几行。
        assert!(!text.contains("♥在线收藏"), "菜单项不该再画出来：{text}");
        assert!(text.contains("已加载《嗯》"), "状态行右侧：{text}");
        assert!(text.contains("Enter播放"), "状态行左侧的键位提示不能被状态顶掉：{text}");
        assert!(text.contains("已停止"), "空闲时播放条也要在：{text}");
    }

    /// 窄屏不再有「页签行」这一层：内容区就是全部宽度（画出来的东西和宽屏一致）。
    #[test]
    fn a_narrow_window_has_no_tab_row_any_more() {
        let (mut app, _runtime) = test_app();
        let text = frame_flat(&mut app, 90, 24);
        assert!(!text.contains("♥在线收藏"), "窄屏也不该有页签行：{text}");
        assert!(text.contains("在线收藏"), "当前屏写在内容块标题里：{text}");
    }

    /// 歌词栏只在「要它 + 屏幕够高」时占位，否则主区吃满。
    #[test]
    fn lyric_block_takes_space_only_when_asked_and_tall_enough() {
        let wide = layout(Rect::new(0, 0, 160, 40), true);
        let lyric = wide.lyric.expect("宽屏且要歌词时该有歌词栏");
        assert!(
            (5..=MAX_LYRIC_HEIGHT).contains(&lyric.height),
            "歌词栏高度 {} 越界",
            lyric.height
        );
        assert!(layout(Rect::new(0, 0, 160, 40), false).lyric.is_none(), "不要就不占");
        assert!(
            layout(Rect::new(0, 0, 160, 18), true).lyric.is_none(),
            "矮屏不展开歌词栏"
        );
        let without = layout(Rect::new(0, 0, 160, 40), false);
        assert!(
            wide.content.height < without.content.height,
            "歌词栏是从主区下方拿走高度（不是并排），主区该变矮"
        );
        assert_eq!(wide.content.width, without.content.width, "宽度不受歌词栏影响");
    }

    /// 菜单栏去掉后布局与宽度无关：任何宽度下内容区都吃满全宽。
    #[test]
    fn the_content_area_takes_the_full_width_at_any_window_size() {
        for width in [60, 90, 100, 160, 240] {
            let areas = layout(Rect::new(0, 0, width, 30), true);
            assert_eq!(areas.content.width, width, "{width} 列时内容区该吃满");
            assert_eq!(areas.content.x, 0, "内容区从最左边开始");
            assert_eq!(areas.player.width, width, "播放条同样横跨全宽");
        }
    }

    #[test]
    fn help_overlay_lists_keys_for_the_current_screen() {
        let (mut app, _runtime) = test_app();
        app.nav.switch_menu(Menu::Playlists);
        app.apply(crate::tui::keys::Action::Help);
        let text = frame_text(&mut app, 160, 40);
        assert!(text.contains("帮助"), "浮层标题");
        assert!(text.contains("列表"), "分组标题");
        assert!(text.contains("j/k、↑/↓选曲目"), "方向键要跟 `j`/`k` 写在一起：{text}");
        assert!(text.contains("Enter进入该歌单的曲目"), "当前屏那一节要跟着菜单变：{text}");
        // 「登出」那一行现在落在 18 行视口之外（表多了一行），
        // 它属于「全局」段这点由 `the_table_pairs_every_key_with_a_description` 钉。
        assert!(text.contains("y歌词栏开/关"), "视图开关写在「全局」里：{text}");
        assert!(text.contains("v频谱区开/关"), "视图开关写在「全局」里：{text}");
    }

    /// 首页不是列表屏：`?` 里只该列**首页上真按得出来**的键，而通用的挪 / `Enter`
    /// 不用进表（状态行那句提示就够了）。
    #[test]
    fn the_home_help_table_only_lists_keys_that_work_there() {
        let (mut app, _runtime) = test_app();
        app.screen = crate::tui::state::Screen::Home;
        app.apply(crate::tui::keys::Action::Help);
        let text = frame_text(&mut app, 160, 40);
        for dead in ["选曲目", "翻页（Ctrl+U / Ctrl+D）", "筛选当前列表", "选入口"] {
            assert!(!text.contains(dead), "首页上按不出来（或通用到不必列），别写进表：{dead}");
        }
    }

    /// 操作提示统一归状态行：浮层里没有底栏，关闭/滚动那句话在最后一行。
    #[test]
    fn the_help_hints_live_in_the_status_row() {
        let (mut app, _runtime) = test_app();
        app.apply(crate::tui::keys::Action::Help);
        let rows = frame_raw(&mut app, 160, 40);
        let popup_rows = help_box_rows(&rows);
        assert!(
            !popup_rows.iter().any(|row| row.contains("关闭")),
            "浮层里不该再有底栏：{popup_rows:?}"
        );
        let status = rows.last().expect("状态行");
        assert!(status.contains("Esc/? 关闭"), "状态行要给关闭方式：{status:?}");
        assert!(status.contains("j/k ↑/↓ 滚动"), "状态行要给滚动方式：{status:?}");
        assert!(!status.contains("登出"), "登出是全局键，说明在浮层里，状态行不重复：{status:?}");
        // 登出那一行还在浮层「全局」段里（这一段在首屏可见范围内）。
        assert!(
            popup_rows.iter().any(|row| row.contains("L") && row.contains("登出")),
            "浮层里该有登出那一行：{popup_rows:?}"
        );
    }

    /// 两列表的判据：所有键行的说明都从**同一列**开始，键也从同一列开始。
    #[test]
    fn the_help_table_lines_up_key_and_description() {
        let (mut app, _runtime) = test_app();
        app.apply(crate::tui::keys::Action::Help);
        let rows = frame_raw(&mut app, 160, 40);
        // 只取浮层里的行：这样就不会被浮层外面的内容（面包屑里也有「在线收藏」）误导。
        let table = help_box_rows(&rows);
        let column = |needle: &str| -> usize {
            table
                .iter()
                .find_map(|row| row.contains(needle).then(|| column_of(row, needle)))
                .unwrap_or_else(|| panic!("{needle} 没画在浮层里：{table:?}"))
        };
        // 只用**首屏可见**的行：浮层视口 18 行，「播放」段已在视口外。
        let columns: Vec<usize> = ["选曲目", "在线收藏", "筛选当前列表", "登出（清空本地会话）"]
            .iter()
            .map(|help| column(help))
            .collect();
        assert!(
            columns.windows(2).all(|pair| pair[0] == pair[1]),
            "说明该从同一列开始：{columns:?}"
        );
        let key_columns: Vec<usize> =
            ["j / k、", "1 ", "y ", "L "].iter().map(|key| column(key)).collect();
        assert!(
            key_columns.windows(2).all(|pair| pair[0] == pair[1]),
            "键列该从同一列开始：{key_columns:?}"
        );
        assert!(key_columns[0] < columns[0], "说明该在键的右边");
    }

    /// `j`/`k`（还有 `g`/`G`）滚的是这张表，滚到两头就停——和队列浮层同一套键。
    #[test]
    fn the_help_scrolls_and_stops_at_both_ends() {
        let (mut app, _runtime) = test_app();
        app.apply(crate::tui::keys::Action::Help);
        // 先画一帧：可见行数由浮层的实际高度决定，滚动量在绘制时被回夹。
        let _ = frame_raw(&mut app, 160, 40);
        assert_eq!(app.help_scroll, 0);
        assert!(frame_text(&mut app, 160, 40).contains("列表"), "开头看得见");

        app.apply(crate::tui::keys::Action::Down);
        assert_eq!(app.help_scroll, 1, "j 滚一行");
        app.apply(crate::tui::keys::Action::Up);
        app.apply(crate::tui::keys::Action::Up);
        assert_eq!(app.help_scroll, 0, "到顶就不动了");

        for _ in 0..60 {
            app.apply(crate::tui::keys::Action::Down);
        }
        let _ = frame_raw(&mut app, 160, 40);
        let bottom = app.help_scroll;
        assert!(bottom > 0, "表比框长，应该滚得动");
        assert!(bottom < 60, "上限该被框的高度夹住：{bottom}");

        app.apply(crate::tui::keys::Action::Down);
        let _ = frame_raw(&mut app, 160, 40);
        assert_eq!(app.help_scroll, bottom, "到底之后 j 不动");
        let end = frame_text(&mut app, 160, 40);
        assert!(end.contains("从歌单移除"), "末尾那几行该看得见：{end}");
        assert!(!end.contains("选曲目"), "开头已经滚过去了");

        app.apply(crate::tui::keys::Action::First);
        let _ = frame_raw(&mut app, 160, 40);
        assert_eq!(app.help_scroll, 0, "g 回开头");
        app.apply(crate::tui::keys::Action::Last);
        let _ = frame_raw(&mut app, 160, 40);
        assert_eq!(app.help_scroll, bottom, "G 到底");
    }

    /// 尺寸矩阵：最小窗口也得画得下（框在画面里、提示在状态行、说明列照样对齐）。
    #[test]
    fn the_help_fits_the_smallest_windows() {
        for (width, height) in [(60u16, 15u16), (80, 24), (100, 30)] {
            let (mut app, _runtime) = test_app();
            app.apply(crate::tui::keys::Action::Help);
            let rows = frame_raw(&mut app, width, height);
            let popup_rows = help_box_rows(&rows);
            assert!(popup_rows.len() >= 3, "{width}x{height} 浮层没画出来：{popup_rows:?}");
            assert!(
                rows.last().expect("状态行").contains("关闭"),
                "{width}x{height} 状态行缺提示"
            );
            let columns: Vec<usize> = ["选曲目", "在线收藏"]
                .iter()
                .filter_map(|help| {
                    popup_rows.iter().find_map(|row| row.contains(help).then(|| column_of(row, help)))
                })
                .collect();
            assert!(
                columns.windows(2).all(|pair| pair[0] == pair[1]),
                "{width}x{height} 说明列该对齐：{columns:?}"
            );
            // 滚一格也得能滚（矮屏里可见行更少，但至少不是一步不动的死表）。
            app.apply(crate::tui::keys::Action::Down);
            let _ = frame_raw(&mut app, width, height);
            assert_eq!(app.help_scroll, 1, "{width}x{height} 应该滚得动");
        }
    }

    /// 首页画出来：标题 + 四格「编号 名字」 + 选中高亮。
    #[test]
    fn the_home_screen_lists_the_four_screens_with_numbers() {
        let (mut app, _runtime) = test_app();
        app.go_home();
        let rows = frame_rows(&mut app, 120, 30);
        assert!(rows.iter().any(|row| row.contains("┌首页")), "标题：{rows:?}");
        for (number, title) in [(1, "在线收藏"), (2, "在线歌单"), (3, "在线搜索"), (4, "播放历史")] {
            assert!(
                rows.iter().any(|row| row.contains(&format!("{number}{title}"))),
                "{number} {title} 那一行没画出来"
            );
        }
        assert!(rows.iter().any(|row| row.contains("▶1在线收藏")), "高亮在第一项：{rows:?}");
    }

    /// 首页的状态行是首页那条提示，不是主界面的。
    #[test]
    fn the_home_screen_has_its_own_status_line_hint() {
        let (mut app, _runtime) = test_app();
        app.screen = Screen::Home;
        let status = frame_rows(&mut app, 120, 30).pop().expect("状态行");
        assert!(status.contains("Enter进入"), "{status:?}");
        assert!(status.contains("1-6直跳"), "{status:?}");
        assert!(!status.contains("Enter播放"), "不是主界面那条：{status:?}");
    }

    /// 首页是 3 列网格：1/2/3 在第 1 行，第 4 项换到第 2 行。
    #[test]
    fn the_home_grid_wraps_at_three_columns() {
        let (mut app, _runtime) = test_app();
        app.go_home();
        let rows = frame_rows(&mut app, 120, 40);
        let row_of = |needle: &str| {
            rows.iter().position(|row| row.contains(needle)).expect("该画出来")
        };
        assert_eq!(row_of("1在线收藏"), row_of("2在线歌单"), "前两格同一行");
        assert_eq!(row_of("2在线歌单"), row_of("3在线搜索"), "一行三格");
        assert!(row_of("4播放历史") > row_of("3在线搜索"), "第 4 格换到下一行");
    }

    /// 列距一致、换行那一格对齐第 1 列。
    #[test]
    fn the_home_grid_lines_up_its_columns() {
        let (mut app, _runtime) = test_app();
        app.go_home();
        let (width, height) = (120, 40);
        let rows = frame_raw(&mut app, width, height);
        let tops: Vec<Vec<usize>> = body_rows(&rows, content_inner(width, height))
            .iter()
            .map(|row| columns_of(row, '┌'))
            .filter(|columns| !columns.is_empty())
            .collect();
        assert_eq!(tops.len(), 2, "两行格子（6 项 = 3 + 3）：{tops:?}");
        assert_eq!(tops[0].len(), 3, "第 1 行三格：{tops:?}");
        assert_eq!(tops[1].len(), 3, "第 2 行三格：{tops:?}");
        assert_eq!(tops[0][1] - tops[0][0], tops[0][2] - tops[0][1], "列距要一致：{tops:?}");
        assert_eq!(tops[1][0], tops[0][0], "换行那一格对齐第 1 列：{tops:?}");
    }

    /// 整块在内容区里居中：横竖留白都相等（差不超过 1——余数是奇数时只能差一格）。
    #[test]
    fn the_home_grid_block_is_centered() {
        let (mut app, _runtime) = test_app();
        app.go_home();
        let (width, height) = (120, 40);
        let rows = frame_raw(&mut app, width, height);
        let inner = content_inner(width, height);
        let border = cell_border_row(&rows);
        let left = *columns_of(border, '┌').first().expect("第 1 格");
        let right = *columns_of(border, '┐').last().expect("第 3 格");
        let margin_left = left - inner.x as usize;
        let margin_right = (inner.x + inner.width - 1) as usize - right;
        assert!(margin_left > 4, "别贴着左边：{margin_left}");
        assert!(margin_left.abs_diff(margin_right) <= 1, "左边 {margin_left}、右边 {margin_right}");

        let body = body_rows(&rows, inner);
        let grid_top = body.iter().position(|row| row.contains('┌')).expect("格子上边框") as u16;
        let grid_bottom = body.iter().rposition(|row| row.contains('└')).expect("格子下边框") as u16;
        let above = grid_top;
        let below = inner.height - 1 - grid_bottom;
        assert!(above.abs_diff(below) <= 1, "上留 {above} 行、下留 {below} 行");
    }

    /// 选中只是前面那个 `▶`：整帧不涂任何底色（原来是 `highlight()` 那层深底）。
    #[test]
    fn the_home_highlight_is_only_the_marker() {
        let (mut app, _runtime) = test_app();
        app.go_home();
        let (width, height) = (120, 40);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("测试终端");
        terminal.draw(|frame| app.draw(frame)).expect("绘制");
        let buffer = terminal.backend().buffer();
        let painted: Vec<(u16, u16)> = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .filter(|&(x, y)| buffer[(x, y)].bg != Color::Reset)
            .collect();
        assert!(painted.is_empty(), "首页不该有底色，{painted:?} 这些格子涂了");
        let text = frame_flat(&mut app, width, height);
        assert!(text.contains("▶1在线收藏"), "选中还是靠 `▶`：{text}");
    }

    /// 选中那格带 `▶`；`j` 挪一格，`▶` 跟着走、只在一格上。
    #[test]
    fn the_home_grid_moves_the_highlight() {
        let (mut app, _runtime) = test_app();
        app.go_home();
        let text = frame_flat(&mut app, 120, 40);
        assert!(text.contains("▶1在线收藏"), "{text}");
        // 首页挪格是 `Tab`（`NextMenuItem`）；`j`/`k` 与上下方向键在首页已经没绑定。
        app.apply(crate::tui::keys::Action::NextMenuItem);
        let text = frame_flat(&mut app, 120, 40);
        assert!(text.contains("▶2在线歌单"), "高亮该挪到第 2 格：{text}");
        assert!(!text.contains("▶1在线收藏"), "`▶` 只该在选中那格：{text}");
    }

    /// 从「本地曲库」回首页，画出来的**必须是首页那排格子**。
    ///
    /// 回归：`draw` 里「本地曲库」那条分支原来排在 `Screen::Home` 之前，而 `go_home()` 不动
    /// `nav.menu` —— 于是 `Esc`/`0` 回首页后状态是首页、画面还是曲库那屏（看着像「Esc 没反应」）。
    #[test]
    fn coming_home_from_the_local_library_draws_the_home_grid() {
        let (mut app, _runtime) = test_app();
        app.library =
            vec![crate::tui::test_support::local_song("C:/Music/canto/曲终人散.flac")];
        app.switch_menu(Menu::LocalLibrary);
        assert!(
            frame_flat(&mut app, 120, 40).contains("本地曲库"),
            "前提：这一屏画的是本地曲库"
        );

        app.apply(crate::tui::keys::Action::Back);

        let text = frame_flat(&mut app, 120, 40);
        assert!(matches!(app.screen, Screen::Home), "状态上该回首页");
        assert!(text.contains("1在线收藏"), "画出来的得是首页那排格子：{text}");
        // 高亮落在「本地曲库」那一格（`go_home()` 记住的就是它）——曲库那屏自己没有这串。
        assert!(text.contains("▶5本地曲库"), "高亮该在曲库那一格：{text}");
        // 曲库那屏的标题带条数（`本地曲库 · 1 首`），首页格子不带。
        assert!(!text.contains("本地曲库·1首"), "不该还留着曲库那屏：{text}");
    }

    /// 最小窗口（内容区 10 行）与常规 80×24 里，四格都在界内。
    #[test]
    fn the_home_screen_fits_the_smallest_windows() {
        let (mut app, _runtime) = test_app();
        app.go_home();
        for (width, height) in [(60, 15), (80, 24)] {
            let rows = frame_rows(&mut app, width, height);
            for label in ["1在线收藏", "2在线歌单", "3在线搜索", "4播放历史"] {
                assert!(
                    rows.iter().any(|row| row.contains(label)),
                    "{width}×{height} 缺 `{label}`：{rows:?}"
                );
            }
        }
    }

    /// 频谱区替掉列表：内容区里只剩频谱区（歌词栏关着）。
    #[test]
    fn the_spectrum_takes_over_the_content_area() {
        let (mut app, _runtime) = test_app();
        app.nav.favorites.set_items(vec![test_song("a")]);
        app.show_lyric = false;
        app.show_spectrum = true;
        let text = frame_flat(&mut app, 120, 30);
        assert!(text.contains("频谱"), "内容区变成频谱区：{text}");
        assert!(!text.contains("歌a"), "列表让位给频谱区：{text}");
        assert!(!text.contains("歌词"), "歌词栏关着就没有歌词栏：{text}");

        // 操作提示统一在底部状态行：没有「模式」之后给的就是列表那套。
        let rows = frame_rows(&mut app, 120, 30);
        let status_row = rows.last().expect("状态行");
        assert!(
            status_row.contains("Enter播放"),
            "操作提示该在状态行：{status_row}"
        );

        // 关掉频谱区就回到列表。
        app.show_spectrum = false;
        let back = frame_flat(&mut app, 120, 30);
        assert!(back.contains("歌a"), "回到列表：{back}");
    }

    /// 频谱区的提示文案按播放态选：缓冲中与「没在播」不是一回事（纯函数，所以
    /// 哑引擎走不到的那些状态也能钉住）。
    #[test]
    fn the_spectrum_message_follows_the_playback_phase() {
        use crate::player::Phase;
        assert_eq!(spectrum_message(&Phase::Buffering), "缓冲中…");
        assert_eq!(spectrum_message(&Phase::Idle), "未在播放");
        assert_eq!(spectrum_message(&Phase::Paused), "未在播放");
        assert_eq!(spectrum_message(&Phase::Playing), "未在播放");
        assert_eq!(spectrum_message(&Phase::Failed("x".into())), "未在播放");
    }

    /// 频谱区在上、歌词栏在下；歌词栏高度是 `(内容区/3).clamp(5, 12)`。
    #[test]
    fn the_spectrum_sits_above_the_lyric_bar() {
        let (mut app, _runtime) = test_app();
        app.engine.force_phase(crate::player::Phase::Playing);
        app.lyric = Some(crate::tui::test_support::test_lyric());
        app.show_spectrum = true;
        app.spectrum_smooth = vec![1.0; crate::spectrum::BANDS];
        app.spectrum_peak = vec![1.0; crate::spectrum::BANDS];

        let rows = frame_rows(&mut app, 120, 30);
        let spectrum = rows
            .iter()
            .position(|row| row.starts_with("┌频谱"))
            .expect("频谱区");
        let lyric = rows
            .iter()
            .position(|row| row.starts_with("┌歌词"))
            .expect("歌词栏");
        assert!(spectrum < lyric, "频谱区该在歌词栏上面：{spectrum} vs {lyric}");
        assert!(rows[spectrum + 1].contains('█'), "频谱区里该有柱子");
        assert!(
            rows[lyric + 1..].iter().any(|row| row.contains("第1句")),
            "歌词栏里该有歌词"
        );
        // 内容区 = 30 − 播放条 4 − 状态行 1 = 25 行 → (25/3).clamp(5, 12) = 8 行。
        let height = rows[lyric..]
            .iter()
            .position(|row| row.starts_with("└"))
            .expect("歌词栏下边框")
            + 1;
        assert_eq!(height, 8, "歌词栏高度该是内容区的三分之一：{rows:?}");

        // 歌名只在歌词栏上出现一次（频谱区整块归它时才带歌名）。
        let text = frame_flat(&mut app, 120, 30);
        assert_eq!(text.matches("频谱").count(), 1, "频谱区只有一块：{text}");
    }

    /// 歌词栏关着时，频谱区吃满整个内容区（不留空框、也不留歌词栏）。
    #[test]
    fn without_the_lyric_bar_the_spectrum_takes_the_whole_area() {
        let (mut app, _runtime) = test_app();
        app.engine.force_phase(crate::player::Phase::Playing);
        app.lyric = Some(crate::tui::test_support::test_lyric());
        app.show_lyric = false;
        app.show_spectrum = true;
        app.spectrum_smooth = vec![1.0; crate::spectrum::BANDS];
        app.spectrum_peak = vec![1.0; crate::spectrum::BANDS];

        let rows = frame_rows(&mut app, 120, 30);
        assert!(
            rows.iter().any(|row| row.starts_with("┌频谱")),
            "频谱区：{rows:?}"
        );
        assert!(
            !rows.iter().any(|row| row.starts_with("┌歌词")),
            "歌词栏关着就不画：{rows:?}"
        );
    }

    /// 没在播时：进过 tick 之后不该留着上一帧的柱子（`tick()` 会清掉缓冲）；
    /// 也没有歌词栏（它要求有歌在放）。
    #[test]
    fn an_idle_engine_shows_a_message_instead_of_stale_bars() {
        let (mut app, _runtime) = test_app();
        app.lyric = Some(crate::tui::test_support::test_lyric());
        app.show_spectrum = true;
        // 故意塞一份「上一帧」的柱子：没在播时它不该被画出来
        app.spectrum_smooth = vec![1.0; crate::spectrum::BANDS];
        app.spectrum_peak = vec![1.0; crate::spectrum::BANDS];
        // 真实运行时每帧都会走 tick：它发现「没在播」就把缓冲清空
        app.tick();

        let rows = frame_rows(&mut app, 120, 30);
        let spectrum = rows
            .iter()
            .position(|row| row.starts_with("┌频谱"))
            .expect("频谱区");
        let height = rows[spectrum..]
            .iter()
            .position(|row| row.starts_with("└"))
            .expect("频谱区下边框");
        let inside = &rows[spectrum..spectrum + height];
        assert!(
            inside.iter().any(|row| row.contains("未在播放")),
            "空闲该给提示：{rows:?}"
        );
        assert!(
            !inside.iter().any(|row| row.contains('█')),
            "不该画残留的柱子：{rows:?}"
        );
        assert!(
            !rows.iter().any(|row| row.starts_with("┌歌词")),
            "没在播就没有歌词栏：{rows:?}"
        );
    }

    /// 窗口太矮（< `MIN_LYRIC_HEIGHT` = 20）时歌词栏不占位：`y` 开着也一样；
    /// 频谱区则拿到整个内容区。
    #[test]
    fn the_lyric_bar_needs_a_tall_enough_window() {
        let (mut app, _runtime) = test_app();
        app.engine.force_phase(crate::player::Phase::Playing);
        app.lyric = Some(crate::tui::test_support::test_lyric());
        app.show_spectrum = true;

        let rows = frame_rows(&mut app, 100, 15);
        assert!(
            !rows.iter().any(|row| row.starts_with("┌歌词")),
            "15 行窗口里歌词栏不占位：{rows:?}"
        );
        assert!(
            rows.iter().any(|row| row.starts_with("┌频谱")),
            "频谱区拿到整个内容区：{rows:?}"
        );
    }

    /// 本地曲库屏：列曲目（体积 `—`、时长未知 `--:--`）、空曲库给配置引导、首页第五格。
    #[test]
    fn the_local_library_screen_lists_files_and_explains_an_empty_one() {
        let (mut app, _runtime) = test_app();
        app.engine.force_phase(crate::player::Phase::Playing);
        app.library = vec![
            crate::tui::test_support::local_song("C:/Music/canto/曲终人散.wav"),
            crate::tui::test_support::local_song("C:/Music/canto/没时长的.wav"),
        ];
        app.refresh_local_view();
        app.switch_menu(crate::tui::state::Menu::LocalLibrary);
        let text = frame_text(&mut app, 140, 40);
        assert!(text.contains("曲终人散") && text.contains("没时长的"), "{text}");
        assert!(text.contains("--:--"), "读不出时长就是 --:--：{text}");
        assert!(!text.contains("MB"), "本地曲目没有体积列：{text}");

        app.library.clear();
        app.refresh_local_view();
        let empty = frame_text(&mut app, 140, 40);
        assert!(
            empty.contains("[library]") && empty.contains("dirs"),
            "空曲库要说清去哪儿加目录：{empty}"
        );

        app.go_home();
        assert!(frame_text(&mut app, 140, 40).contains("本地曲库"), "首页第五格");
    }

    /// 没有歌词也画歌词栏：还在拉写「歌词加载中…」，拉完没有写「暂无歌词」。
    #[test]
    fn the_lyric_bar_shows_even_without_lyrics() {
        let (mut app, _runtime) = test_app();
        app.engine.force_phase(crate::player::Phase::Playing);
        app.lyric_loading = true;
        assert!(frame_text(&mut app, 120, 40).contains("歌词加载中…"));
        app.lyric_loading = false;
        assert!(frame_text(&mut app, 120, 40).contains("暂无歌词"));
    }

    /// 频谱区开着时队列浮层照样看得见（它画在 `Areas.content` 上）。
    #[test]
    fn the_queue_overlay_stays_visible_over_the_spectrum() {
        let (mut app, _runtime) = test_app();
        app.engine.force_phase(crate::player::Phase::Playing);
        app.show_spectrum = true;
        app.queue = queue::Queue::new(vec![test_song("a")], 0, String::new());
        app.apply(crate::tui::keys::Action::QueueOverlay);
        // 曲子标题只在浮层里出现（频谱开着时列表不画；测试里引擎没有曲名）。
        assert!(frame_text(&mut app, 120, 40).contains("歌a"), "浮层该压得住频谱区");
    }

    /// 频谱区开着时 `/` 照样能用：筛选输入行得看得见——它不属于列表，是内容区顶上那一行。
    #[test]
    fn the_filter_input_stays_visible_over_the_spectrum() {
        let (mut app, _runtime) = test_app();
        app.engine.force_phase(crate::player::Phase::Playing);
        app.show_spectrum = true;
        app.apply(crate::tui::keys::Action::Filter);
        assert!(app.filter_input, "`/` 该进输入态");
        for character in "ab".chars() {
            app.apply(crate::tui::keys::Action::FilterChar(character));
        }
        let text = frame_text(&mut app, 120, 40);
        assert!(text.contains("筛选：ab"), "输入行得画在频谱区上面：{text}");
    }

    /// 歌词栏里当前句是高亮的（黄 + 粗），别的行不是——看真实单元格的颜色。
    #[test]
    fn the_current_lyric_line_is_highlighted() {
        let (mut app, _runtime) = test_app();
        app.engine.force_phase(crate::player::Phase::Playing);
        app.lyric = Some(crate::tui::test_support::test_lyric());
        app.show_spectrum = true;
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).expect("测试终端");
        terminal.draw(|frame| app.draw(frame)).expect("绘制");
        let buffer = terminal.backend().buffer();

        // 引擎是空闲的，位置为 0 → 第 1 句（时间戳 0s）就是当前句。
        // 跟 `frame_flat` 一样去掉空白：宽字符在缓冲区里占两个单元格，直接拼会多出空格。
        let rows: Vec<String> = (0..30u16)
            .map(|y| {
                (0..120u16)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .chars()
                    .filter(|character| !character.is_whitespace())
                    .collect()
            })
            .collect();
        let row_of = |needle: &str| {
            rows.iter()
                .position(|row| row.contains(needle))
                .unwrap_or_else(|| panic!("帧里没有 {needle}"))
        };
        let current = row_of("第1句");
        let other = row_of("第3句");
        let styled = |y: usize| {
            (0..120u16).any(|x| buffer[(x, y as u16)].fg == ratatui::style::Color::Yellow)
        };
        assert!(styled(current), "当前句该是黄的：{:?}", rows[current]);
        assert!(!styled(other), "别的行不该染色：{:?}", rows[other]);
        // 加粗也是高亮的一部分：只要那一行里有粗体就算。
        let bold = (0..120u16).any(|x| {
            buffer[(x, current as u16)]
                .modifier
                .contains(ratatui::style::Modifier::BOLD)
        });
        assert!(bold, "当前句该加粗：{:?}", rows[current]);
    }

    /// 历史屏的标题带条数（现算），切到别的屏就该显示那一屏的名字。
    #[test]
    fn the_title_follows_the_screen_and_the_history_counts_its_rows() {
        let (mut app, _runtime) = test_app();
        for mid in ["a", "b"] {
            app.record_history(&test_song(mid));
        }
        app.switch_menu(Menu::History);
        let history = frame_flat(&mut app, 120, 30);
        assert!(history.contains("播放历史2条"), "历史标题带条数：{history}");

        app.switch_menu(Menu::Favorites);
        app.nav.favorites.set_items(vec![test_song("a")]);
        let favorites = frame_flat(&mut app, 120, 30);
        assert!(favorites.contains("在线收藏"), "{favorites}");
        assert!(
            !favorites.contains("播放历史2条"),
            "切走之后不该还挂着历史标题：{favorites}"
        );
    }

    /// 回归：在别的屏播放（= 记一条播放历史）之后，那一屏的标题不能被改写成「历史 N 条」。
    ///
    /// 以前标题是存在 `breadcrumb` 里的，`refresh_history_view` 一跑就把它覆盖成历史屏的标题。
    #[test]
    fn recording_history_does_not_rewrite_another_screens_title() {
        let (mut app, _runtime) = test_app();
        app.switch_menu(Menu::Favorites);
        app.nav.favorites.set_items(vec![test_song("a")]);
        assert!(frame_flat(&mut app, 120, 30).contains("在线收藏"));

        // 正好是播放一首歌之后走的那条路：记一笔历史 + 重建历史屏。
        app.record_history(&test_song("b"));
        let text = frame_flat(&mut app, 120, 30);
        assert!(text.contains("在线收藏"), "标题还是当前屏：{text}");
        assert!(!text.contains("播放历史1条"), "不该出现历史标题：{text}");
    }

    /// 队列浮层只盖内容区：播放条还在下面，标题也不写「来自某歌单」。
    #[test]
    fn the_queue_overlay_sits_above_the_player_bar() {
        let (mut app, _runtime) = test_app();
        app.queue = queue::Queue::new(vec![test_song("a"), test_song("b")], 0, "嗯");
        app.apply(crate::tui::keys::Action::QueueOverlay);

        let rows = frame_rows(&mut app, 120, 30);
        let queue_row = rows
            .iter()
            .position(|row| row.contains("队列1/2"))
            .expect("该有队列浮层");
        let player_row = rows
            .iter()
            .position(|row| row.contains("已停止"))
            .expect("播放条还在");
        assert!(
            queue_row < player_row,
            "浮层该在播放条上面（队列在第 {queue_row} 行，播放条在第 {player_row} 行）"
        );
        let text = frame_flat(&mut app, 120, 30);
        assert!(!text.contains("来自"), "标题不写来源：{text}");
        assert!(text.contains("队列1/2"), "标题报进度与模式：{text}");
    }

    /// 操作提示统一在底部状态行/帮助里：内容区不再出现「按 3 回到输入框」这类字样。
    #[test]
    fn the_content_area_has_no_key_hints() {
        let (mut app, _runtime) = test_app();
        app.nav.switch_menu(Menu::Search);
        app.nav.end_search_input();
        let text = frame_flat(&mut app, 120, 30);
        assert!(text.contains("搜索："), "搜索行只留标签：{text}");
        assert!(!text.contains("回到输入框"), "提示该收进帮助浮层：{text}");
        assert!(!text.contains("按3"), "提示该收进帮助浮层：{text}");
    }

    /// 队列浮层里的高亮要真画在挪动后的那一行上。
    ///
    /// 回归：方向键原本穿透到底下的主区列表，浮层里的高亮永远不动（「浮层选不动」）。
    /// 这里用 `TestBackend` 看真实单元格，避免增量重绘造成的误判。
    /// 有筛选时标题报「命中/总数」，列表只画命中行；筛选框开着时多一行输入行。
    #[test]
    fn the_filter_is_visible_in_the_title_and_narrows_the_rows() {
        let (mut app, _runtime) = test_app();
        app.nav.switch_menu(Menu::Favorites);
        app.nav.favorites.set_items(vec![
            crate::tui::test_support::titled_song("m1", "晴天", "周杰伦"),
            crate::tui::test_support::titled_song("m2", "非酋", "薛明媛"),
        ]);
        let before = frame_flat(&mut app, 160, 40);
        assert!(!before.contains("筛选「"), "没筛的时候不该有徽标：{before}");

        app.nav.set_filter("非酋");
        let after = frame_flat(&mut app, 160, 40);
        assert!(after.contains("筛选「非酋」1/2"), "标题要报命中数：{after}");
        assert!(!after.contains("晴天"), "没命中的行不该画出来：{after}");

        // 输入框开着时第一行是输入行（画的是当前关键词）。
        app.filter_input = true;
        let typing = frame_flat(&mut app, 160, 40);
        assert!(typing.contains("筛选：非酋"), "筛选框要画出来：{typing}");
    }

    #[test]
    fn the_queue_overlay_draws_the_highlight_on_the_selected_row() {
        let (mut app, _runtime) = test_app();
        app.queue = queue::Queue::new(vec![test_song("a"), test_song("b")], 0, "嗯");
        app.apply(crate::tui::keys::Action::QueueOverlay);
        let before = frame_flat(&mut app, 160, 40);
        assert!(before.contains("队列1/2"), "浮层标题要显示序号：{before}");

        // 第 2 行（歌 b）与第 1 行之间有没有高亮符号——主区列表那个 ▶ 不算。
        let arrow_between = |text: &str| {
            let (first, second) = (
                text.find("歌a").expect("第 1 行还在"),
                text.find("歌b").expect("第 2 行还在"),
            );
            text.char_indices()
                .filter(|(_, character)| *character == '▶')
                .map(|(index, _)| index)
                .find(|index| *index > first && *index < second)
        };
        assert!(arrow_between(&before).is_none(), "一开始高亮在第 1 行上");

        app.apply(crate::tui::keys::Action::Down);
        let after = frame_flat(&mut app, 160, 40);
        assert!(
            arrow_between(&after).is_some(),
            "按 j 之后 ▶ 该落在第 2 行（歌 b）上：{after}"
        );
    }

    #[test]
    fn tiny_window_only_asks_for_a_bigger_terminal() {
        let (mut app, _runtime) = test_app();
        let text = frame_flat(&mut app, 40, 10);
        assert!(text.contains("终端太小"), "小窗口只给一句提示：{text}");
        assert!(!text.contains("♪"), "不再画播放条");
    }

    /// 状态行右侧是**固定 32 列**的通知位：长了就按显示宽度截断（中文双宽），
    /// 而且是省略号收尾——不然长错误链会把左边的键位提示挤掉、还看不清被裁到哪。
    #[test]
    fn the_notice_slot_is_fixed_width_and_truncates_with_an_ellipsis() {
        let (mut app, _runtime) = test_app();
        app.notify("下载失败：网络断了");
        let short = frame_flat(&mut app, 120, 30);
        assert!(short.contains("下载失败：网络断了"), "短信该整条画出来：{short}");

        let long = "这是一条特别长的通知".repeat(4);
        app.notify(long.clone());
        let text = frame_flat(&mut app, 120, 30);
        assert!(!text.contains(&long), "超长通知不该整条画出来：{text}");
        assert!(text.contains('…'), "该以省略号收尾：{text}");
        // 32 列里留 1 列给省略号 → 15 个汉字（每字 2 列）。
        assert!(
            text.contains("这是一条特别长的通知这是一条特…"),
            "该截到 15 个字再加省略号：{text}"
        );
        // 固定宽度的意义：不动左边那半行键位提示。
        assert!(text.contains("Enter播放·Space暂停"), "键位提示不该被挤掉：{text}");
    }

    /// 各屏「有多少条」挂在内容块标题上——右区只放通知，不再报计数。
    #[test]
    fn the_title_reports_how_many_rows_each_screen_holds() {
        let (mut app, _runtime) = test_app();

        app.switch_menu(Menu::Favorites);
        app.nav.favorites.set_items(vec![test_song("a"), test_song("b")]);
        let favorites = frame_flat(&mut app, 120, 30);
        assert!(favorites.contains("在线收藏2首"), "{favorites}");

        app.switch_menu(Menu::Playlists);
        app.nav.playlists.set_items(vec![
            crate::tui::test_support::test_playlist("1", "嗯"),
            crate::tui::test_support::test_playlist("2", "循环"),
        ]);
        let playlists = frame_flat(&mut app, 120, 30);
        assert!(playlists.contains("在线歌单2个"), "{playlists}");

        app.nav.push_tracks("1", "嗯", vec![test_song("a")]);
        let tracks = frame_flat(&mut app, 120, 30);
        assert!(tracks.contains("在线歌单›嗯1首"), "{tracks}");

        app.switch_menu(Menu::Search);
        app.nav.search_results.set_items(vec![test_song("a")]);
        let search = frame_flat(&mut app, 120, 30);
        assert!(search.contains("在线搜索1首"), "{search}");

        app.switch_menu(Menu::History);
        let history = frame_flat(&mut app, 120, 30);
        assert!(history.contains("播放历史0条"), "{history}");
    }

    /// 筛过之后标题只报「命中/总数」——别变成「2 首 筛选「非酋」1/2」那种重复。
    #[test]
    fn a_filtered_title_reports_hits_instead_of_the_total() {
        let (mut app, _runtime) = test_app();
        app.switch_menu(Menu::Favorites);
        app.nav.favorites.set_items(vec![
            crate::tui::test_support::titled_song("a", "晴天", "周杰伦"),
            crate::tui::test_support::titled_song("b", "非酋", "薛明媛"),
        ]);
        app.nav.set_filter("非酋");

        let text = frame_flat(&mut app, 120, 30);
        assert!(text.contains("筛选「非酋」1/2"), "{text}");
        assert!(!text.contains("2首"), "筛过之后不该再挂一个总数：{text}");
    }

    /// 历史屏也有表头与那些列；备注列写「时间 · 来源」。
    #[test]
    fn the_history_screen_has_a_header_and_a_note_column() {
        let (mut app, _runtime) = test_app();
        // 独立的下载目录：默认那个是 `~/Music/canto`，开发机上真躺着「刘若英 - 成全.mp3」，
        // 这一行就会走「下载里已有」（容器 `MP3`）而不是记录里那份。列口径是 缓存 > 下载 > 记录。
        app.config.paths.download_dir = std::env::temp_dir()
            .join(format!("canto-draw-hist-{}", std::process::id()));
        std::fs::remove_dir_all(&app.config.paths.download_dir).ok();
        std::fs::create_dir_all(&app.config.paths.download_dir).expect("建下载目录");
        app.refresh_downloads();

        // 一条「刚听完 QQ 歌」的记录：时长/体积是 Task 8 存进去的那份。
        // （不真按 Enter 播：QQ 歌要等 vkey 拿到才落历史，测试里没有网络。）
        app.history.push(
            crate::history::Entry {
                mid: "a".to_string(),
                title: "成全".to_string(),
                singers: "刘若英".to_string(),
                at: crate::clock::now_secs(),
                origin: "在线收藏".to_string(),
                local_path: None,
                duration_secs: 276,
                sizes: crate::model::FileSizes { mp3_128: 4_400_000, ..Default::default() },
                album_name: "年华".to_string(),
            },
            50,
        );
        app.refresh_history_view();
        app.switch_menu(Menu::History);

        let rows = frame_raw(&mut app, 140, 30);
        let header = rows
            .iter()
            .find(|row| row.contains("时长") && row.contains("备注"))
            .expect("历史屏有表头");
        let data = rows.iter().find(|row| row.contains("成全")).expect("有数据行");
        // 时长右对齐：比右端（表头「时长」4 列、数据 `04:36` 5 列，起点天然差一列）。
        let width = unicode_width::UnicodeWidthStr::width;
        assert_eq!(
            column_of(header, "时长") + width("时长"),
            column_of(data, "04:36") + width("04:36"),
            "时长列右端对齐"
        );
        assert_eq!(column_of(header, "备注"), column_of(data, "刚刚"), "备注列对齐");
        assert!(data.contains("MP3 128k"), "历史也报音质：{data}");
        assert!(data.contains("4.2MB"), "历史也报体积：{data}");
        assert!(data.contains("年华"), "历史也有专辑列：{data}");
        assert_eq!(column_of(header, "专辑"), column_of(data, "年华"), "专辑列对齐");
        assert!(data.contains("刚刚 · 来自《在线收藏》"), "备注列写时间与来源：{data}");
        assert!(!data.contains("成全 — 刘若英"), "不该再拼「标题 — 歌手」：{data}");

        std::fs::remove_dir_all(&app.config.paths.download_dir).ok();
    }

    /// 老条目（记录里没有时长/体积）显示占位符，不写假的 0。
    #[test]
    fn an_old_history_entry_shows_placeholders() {
        let (mut app, _runtime) = test_app();
        app.history.push(
            crate::history::Entry {
                mid: "a".to_string(),
                title: "老条目".to_string(),
                singers: "某人".to_string(),
                at: crate::clock::now_secs(),
                origin: "在线收藏".to_string(),
                local_path: None,
                duration_secs: 0,
                sizes: crate::model::FileSizes::default(),
                album_name: String::new(),
            },
            50,
        );
        app.refresh_history_view();
        app.switch_menu(Menu::History);
        let rows = frame_raw(&mut app, 140, 30);
        let data = rows.iter().find(|row| row.contains("老条目")).expect("有数据行");
        assert!(data.contains("--:--"), "老记录写占位符：{data}");
        assert!(data.contains('—'), "老记录没有专辑也写占位符：{data}");
        // 状态栏那行本来就写引擎位置（`00:00/--:--`），这里只看数据行。
        assert!(!data.contains("00:00"), "不许写假的 00:00：{data}");
    }

    /// 历史屏的空态照旧（不是表头的空壳）。
    #[test]
    fn the_history_screen_keeps_its_empty_state() {
        let (mut app, _runtime) = test_app();
        app.switch_menu(Menu::History);
        let text = frame_text(&mut app, 100, 30);
        assert!(text.contains("暂无播放历史"), "{text}");
        assert!(!text.contains("标题"), "空态不画表头：{text}");
    }

    /// 60 列（最小窗口）下「备注」被撤掉、`缓存`/`下载` 还在；矮屏把表头那一行还给数据。
    #[test]
    fn a_sixty_column_history_drops_the_note_and_a_short_list_drops_the_header() {
        let (mut app, _runtime) = test_app();
        app.history.push(
            crate::history::Entry {
                mid: "a".to_string(),
                title: "成全".to_string(),
                singers: "刘若英".to_string(),
                at: crate::clock::now_secs(),
                origin: "在线收藏".to_string(),
                local_path: None,
                duration_secs: 276,
                sizes: crate::model::FileSizes::default(),
                album_name: String::new(),
            },
            50,
        );
        app.refresh_history_view();
        app.switch_menu(Menu::History);

        let rows = frame_raw(&mut app, 60, 15);
        let header = rows
            .iter()
            .find(|row| row.contains("缓存"))
            .expect("60 列下也有表头");
        assert!(header.contains("下载"), "两个标记列都在：{header:?}");
        assert!(!header.contains("备注"), "60 列撤掉「备注」：{header:?}");
        assert!(rows.iter().any(|row| row.contains("成全")), "数据行还在");
        // 最小的合法窗口（60×15 → 内容区 10 行）仍然画表头：这一档是 spec 尺寸矩阵里最紧的
        // 一档。「内容区 < 6 行不画表头」那条走 `group` 的单元测试（`rows::tests`）——它要
        // 内容区真的矮到 6 行以下才触发，而窗口下限（60×15）与歌词栏下限（高 ≥ 20 才占位）
        // 一起把这条路堵住了：最紧也只是 10 行。
    }

    /// 歌单行的缩进口径只有一处：选择框的「新建…」跟普通行同一列起名字。
    #[test]
    fn the_playlist_row_indent_has_one_source() {
        let normal = crate::tui::draw::playlist_row(Some(3), "嗯");
        let blank = crate::tui::draw::playlist_row(None, "新建…");
        assert_eq!(normal, "    3  嗯");
        assert_eq!(blank, "       新建…", "计数那一列留白");
        assert_eq!(column_of(&blank, "新建"), column_of(&normal, "嗯"), "名字同一列起");

        // 列表真的走这份口径（不是各写各的）。
        let (mut app, _runtime) = test_app();
        app.switch_menu(Menu::Playlists);
        app.nav
            .playlists
            .set_items(vec![crate::tui::test_support::test_playlist("1", "嗯")]);
        let rows = frame_raw(&mut app, 100, 30);
        let data = rows.iter().find(|row| row.contains("嗯")).expect("有歌单行");
        assert!(data.contains(&normal), "列表行就是 playlist_row 那一份：{data:?}");
    }

    /// 队列浮层也有表头（高度跟着 +1），缓存/下载两列在。
    #[test]
    fn the_queue_overlay_has_a_header() {
        let (mut app, _runtime) = test_app();
        app.config.paths.download_dir =
            std::env::temp_dir().join(format!("canto-draw-queue-{}", std::process::id()));
        std::fs::remove_dir_all(&app.config.paths.download_dir).ok();
        std::fs::create_dir_all(&app.config.paths.download_dir).expect("建下载目录");
        app.refresh_downloads();
        app.queue = crate::tui::queue::Queue::new(
            vec![
                crate::tui::test_support::titled_song("a", "成全", "刘若英"),
                crate::tui::test_support::titled_song("b", "后来", "刘若英"),
            ],
            0,
            "嗯",
        );
        app.apply(crate::tui::keys::Action::QueueOverlay);

        let rows = frame_raw(&mut app, 160, 40);
        let at = rows
            .iter()
            .position(|row| row.contains("队列 1/2"))
            .expect("浮层标题还在");
        // 标题挂在框线上，表头是它下面那一行。
        let header = &rows[at + 1];
        assert!(
            header.contains("缓存") && header.contains("下载") && header.contains("标题"),
            "表头紧跟在框线下面：{header:?}"
        );
        let data = rows
            .iter()
            .find(|row| row.contains("成全") && row.contains("刘若英"))
            .expect("有数据行");
        assert_eq!(
            column_of(header, "时长") + unicode_width::UnicodeWidthStr::width("时长"),
            column_of(data, "03:20") + unicode_width::UnicodeWidthStr::width("03:20"),
            "时长列右端对齐：\n{header:?}\n{data:?}"
        );
        assert!(
            !data.contains("成全 — 刘若英"),
            "不该再拼「标题 — 歌手」：{data:?}"
        );

        app.apply(crate::tui::keys::Action::QueueOverlay);
        std::fs::remove_dir_all(&app.config.paths.download_dir).ok();
    }

    /// 曲目列表有表头，且与数据同列；缓存/下载两个标记按数据落在正确的列上。
    #[test]
    fn song_lists_have_a_header_aligned_with_the_rows() {
        let (mut app, _runtime) = test_app();
        let mut song = crate::tui::test_support::titled_song("a", "成全", "刘若英");
        song.sizes.mp3_128 = 4_400_000;
        app.nav.favorites.set_items(vec![song]);
        app.cached
            .insert("a".to_string(), (crate::model::Quality::High320, 9_800_000));
        app.config.paths.download_dir =
            std::env::temp_dir().join(format!("canto-draw-dl-{}", std::process::id()));
        std::fs::remove_dir_all(&app.config.paths.download_dir).ok();
        std::fs::create_dir_all(&app.config.paths.download_dir).expect("建下载目录");
        std::fs::write(
            app.config.paths.download_dir.join("刘若英 - 成全.mp3"),
            vec![0_u8; 1_024],
        )
        .expect("写下载文件");
        app.refresh_downloads();
        app.switch_menu(Menu::Favorites);

        let rows = frame_raw(&mut app, 100, 30);
        let header = rows
            .iter()
            .find(|row| row.contains("标题") && row.contains("缓存"))
            .expect("有表头");
        let data = rows.iter().find(|row| row.contains("成全")).expect("有数据行");
        // 对齐规则跟 `rows` 的单元测试一致：左对齐比起点、右对齐比终点、居中允许差 1 列
        // （表头「缓存」占满 4 列，数据 `●` 居中，居中余数可以差一格）。
        let width = unicode_width::UnicodeWidthStr::width;
        for (needle, value, align) in [
            ("标题", "成全", "left"),
            ("歌手", "刘若英", "left"),
            ("时长", "03:20", "right"),
            ("音质", "MP3 320k", "left"),
            ("体积", "9.3MB", "right"),
            ("缓存", "●", "center"),
            ("下载", "↓", "center"),
        ] {
            let (head_at, value_at) = (column_of(header, needle), column_of(data, value));
            match align {
                "left" => assert_eq!(
                    head_at, value_at,
                    "「{needle}」列没对齐：\n表头 {header:?}\n数据 {data:?}"
                ),
                "right" => assert_eq!(
                    head_at + width(needle),
                    value_at + width(value),
                    "「{needle}」列的右端没对齐：\n表头 {header:?}\n数据 {data:?}"
                ),
                _ => assert!(
                    head_at.abs_diff(value_at) <= 1,
                    "「{needle}」列不该偏出 1 列：\n表头 {header:?}\n数据 {data:?}"
                ),
            }
        }

        std::fs::remove_dir_all(&app.config.paths.download_dir).ok();
    }

    /// 本地曲库屏：容器（扩展名）与真实大小，不再显示配置档位。
    #[test]
    fn a_local_row_reports_its_own_container_and_size() {
        let (mut app, _runtime) = test_app();
        let mut song = crate::tui::test_support::local_song("C:/Music/canto/曲终人散.flac");
        song.local_bytes = 31_457_280;
        song.duration_secs = 270;
        song.album_name = "曲终人散".to_string();
        app.library = vec![song];
        app.refresh_local_view();
        app.switch_menu(Menu::LocalLibrary);

        let text = frame_text(&mut app, 140, 40);
        assert!(text.contains("FLAC"), "本地歌报容器：{text}");
        assert!(text.contains("曲终人散"), "本地歌也有专辑列：{text}");
        assert!(text.contains("30.0MB"), "本地歌报真实大小：{text}");
        assert!(!text.contains("MP3 128k"), "不该再显示配置档位：{text}");
    }
}

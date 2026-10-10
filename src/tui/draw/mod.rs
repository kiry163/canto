//! 绘制：骨架布局与各面板。
//!
//! 骨架（菜单 / 主区 / 歌词 / 播放条 / 状态行）、断点与共用的小工具都在这里；
//! 各面板自己的画法在子模块里。状态与按键在 [`super::state`]。

use std::time::Duration as StdDuration;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Block;

pub mod content;
pub mod lyric;
pub mod menu;
pub mod overlay;
pub mod player;
pub mod status;

use crate::player::Phase;

use super::keys;
use super::state::{App, Screen};

/// 菜单列宽。
pub const MENU_WIDTH: u16 = 16;
/// 低于这个宽度就把菜单横过来当顶部页签行。
pub const NARROW_WIDTH: u16 = 100;
/// 低于这个高度就不展开歌词块（矮屏里歌词会把主区挤成一两行）。
pub const MIN_LYRIC_HEIGHT: u16 = 20;
/// 窗口小于这个尺寸就只画一行提示。
pub const MIN_WIDTH: u16 = 60;
pub const MIN_HEIGHT: u16 = 15;
/// 歌词块高度上限（行）。
pub const MAX_LYRIC_HEIGHT: u16 = 12;

/// 骨架切出来的几块。
pub struct Areas {
    pub menu: Rect,
    pub content: Rect,
    pub lyric: Option<Rect>,
    pub player: Rect,
    pub status: Rect,
    /// 窄屏：菜单在顶部一行，不在左列。
    pub narrow: bool,
}

/// 切骨架。`lyric_wanted` 由调用方判断（有没有在放 + 歌词拿没拿到 + 面板开没开）。
pub fn layout(area: Rect, lyric_wanted: bool) -> Areas {
    let narrow = area.width < NARROW_WIDTH;
    let rows = Layout::vertical([
        // 窄屏时这里给顶部页签行留一行，宽屏时长度为 0（不占位）。
        Constraint::Length(u16::from(narrow)),
        Constraint::Min(3),
        // 播放条：上下边框各 1 行 + 标题行 + 进度条行。
        Constraint::Length(4),
        Constraint::Length(1),
    ])
    .split(area);
    let body = rows[1];
    let lyric_height = if lyric_wanted && area.height >= MIN_LYRIC_HEIGHT {
        (body.height / 3).clamp(5, MAX_LYRIC_HEIGHT)
    } else {
        0
    };
    let lyric_area = |area: Rect| (lyric_height > 0).then_some(area);
    let (menu, content, lyric) = if narrow {
        let inner = Layout::vertical([Constraint::Min(3), Constraint::Length(lyric_height)])
            .split(body);
        (rows[0], inner[0], lyric_area(inner[1]))
    } else {
        let columns = Layout::horizontal([Constraint::Length(MENU_WIDTH), Constraint::Min(20)])
            .split(body);
        let inner = Layout::vertical([Constraint::Min(3), Constraint::Length(lyric_height)])
            .split(columns[1]);
        (columns[0], inner[0], lyric_area(inner[1]))
    };
    Areas {
        menu,
        content,
        lyric,
        player: rows[2],
        status: rows[3],
        narrow,
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
        // 歌词块只在「正在放 + 有歌词 + 面板开着」时占位，不播放时不画空框。
        let lyric_wanted = self.show_lyric && self.lyric.is_some() && self.player_active();
        let areas = layout(area, lyric_wanted);
        if areas.narrow {
            self.draw_tabs(frame, areas.menu);
        } else {
            self.draw_menu(frame, areas.menu);
        }
        self.draw_content(frame, areas.content);
        if let Some(lyric) = areas.lyric {
            self.draw_lyric(frame, lyric);
        }
        self.draw_player(frame, areas.player);
        self.draw_status(frame, areas.status);
        if self.help_open {
            self.draw_help(frame, area);
        } else if self.queue_open {
            self.draw_queue(frame, area);
        } else if self.picker.is_some() {
            self.draw_picker(frame, area);
        }
    }

    /// 音频引擎是否已经离开 `Idle`（有歌在放、缓冲或暂停）。
    fn player_active(&self) -> bool {
        self.engine.snapshot().phase != Phase::Idle
    }
}

/// 状态行左侧的键位提示：按当前上下文给三到五个，只写**真的接了行为**的键。
pub fn hints(context: keys::Context) -> &'static str {
    match context {
        keys::Context::Menu => "j/k 选菜单 · Enter 进入 · 1-4 直达 · Tab 到主区 · ? 帮助",
        keys::Context::Content => {
            "Enter 播放 · Space 暂停 · n/b 切歌 · f 收藏 · p 加歌单 · ? 帮助"
        }
        keys::Context::SearchInput => "输入关键词 · Enter 搜索 · Esc 退出输入",
        keys::Context::PlaylistPicker => "j/k 选歌单 · Enter 确认 · Esc 取消",
        keys::Context::HelpOverlay => "Esc 或 ? 关闭帮助",
        keys::Context::QueueOverlay => {
            "Enter 跳播 · d 删除 · Shift+D 清空 · j/k 选择 · Esc 关闭"
        }
        keys::Context::Login => "c 粘贴 cookie · o 打开二维码图片 · r 重新扫码 · q 退出"
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::tui::state::Menu;
    use crate::tui::test_support::{test_app, test_song};

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
    fn wide_screen_shows_menu_breadcrumb_and_two_part_status_row() {
        let (mut app, _runtime) = test_app();
        app.nav.switch_menu(Menu::Playlists);
        app.nav.push_tracks("1", "嗯", vec![test_song("a")]);
        app.status = "已加载《嗯》".to_string();
        let text = frame_flat(&mut app, 160, 40);
        for item in ["♥我的收藏", "≡我的歌单", "⌕搜索", "↺历史"] {
            assert!(text.contains(item), "菜单里缺 {item}：{text}");
        }
        assert!(text.contains("我的歌单›嗯"), "面包屑要带层级：{text}");
        assert!(text.contains("已加载《嗯》"), "状态行右侧：{text}");
        assert!(text.contains("Enter播放"), "状态行左侧的键位提示不能被状态顶掉：{text}");
        assert!(text.contains("已停止"), "空闲时播放条也要在：{text}");
    }

    #[test]
    fn narrow_screen_turns_the_menu_into_a_tab_row() {
        let (mut app, _runtime) = test_app();
        let text = frame_flat(&mut app, 90, 24);
        assert!(text.contains("1♥我的收藏"), "窄屏菜单收成页签行：{text}");
        assert!(text.contains("4↺历史"), "四项都在：{text}");
        assert!(!text.contains("┌菜单"), "窄屏不再画左侧菜单块：{text}");
    }

    /// 歌词块只在「要它 + 屏幕够高」时占位，否则主区吃满。
    #[test]
    fn lyric_block_takes_space_only_when_asked_and_tall_enough() {
        let wide = layout(Rect::new(0, 0, 160, 40), true);
        let lyric = wide.lyric.expect("宽屏且要歌词时该有歌词块");
        assert!(
            (5..=MAX_LYRIC_HEIGHT).contains(&lyric.height),
            "歌词块高度 {} 越界",
            lyric.height
        );
        assert!(layout(Rect::new(0, 0, 160, 40), false).lyric.is_none(), "不要就不占");
        assert!(
            layout(Rect::new(0, 0, 160, 18), true).lyric.is_none(),
            "矮屏不展开歌词块"
        );
        let without = layout(Rect::new(0, 0, 160, 40), false);
        assert!(
            wide.content.height < without.content.height,
            "歌词块是从主区下方拿走高度（不是并排），主区该变矮"
        );
        assert_eq!(wide.content.width, without.content.width, "宽度不受歌词块影响");
    }

    /// 窄屏：菜单收成一行页签，主区吃满宽度。
    #[test]
    fn narrow_layout_moves_the_menu_to_a_single_row() {
        let narrow = layout(Rect::new(0, 0, 90, 24), true);
        assert!(narrow.narrow);
        assert_eq!(narrow.menu.height, 1, "窄屏菜单只有一行");
        assert_eq!(narrow.menu.width, 90, "页签行横跨全宽");
        assert_eq!(narrow.content.width, 90, "主区也吃满宽");
    }

    #[test]
    fn help_overlay_lists_keys_for_the_current_screen() {
        let (mut app, _runtime) = test_app();
        app.nav.switch_menu(Menu::Playlists);
        app.apply(crate::tui::keys::Action::Help);
        let text = frame_flat(&mut app, 160, 40);
        assert!(text.contains("帮助"), "浮层标题");
        assert!(text.contains("全局Tab焦点"), "全局键位：{text}");
        assert!(text.contains("歌单Enter进入曲目"), "当前屏那一行要跟着菜单变：{text}");
        assert!(text.contains("关闭Esc或?"), "关闭方式");
    }

    #[test]
    fn tiny_window_only_asks_for_a_bigger_terminal() {
        let (mut app, _runtime) = test_app();
        let text = frame_flat(&mut app, 40, 10);
        assert!(text.contains("终端太小"), "小窗口只给一句提示：{text}");
        assert!(!text.contains("♪"), "不再画播放条");
    }
}

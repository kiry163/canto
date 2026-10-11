//! 绘制：骨架布局与各面板。
//!
//! 骨架（内容区 / 歌词 / 播放条 / 状态行）、断点与共用的小工具都在这里；
//! 各面板自己的画法在子模块里。状态与按键在 [`super::state`]。

use std::time::Duration as StdDuration;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Block;

pub mod content;
pub mod lyric;
pub mod overlay;
pub mod player;
pub mod status;

use crate::player::Phase;

use super::keys;
use super::state::{App, Screen};

/// 低于这个高度就不展开歌词块（矮屏里歌词会把主区挤成一两行）。
pub const MIN_LYRIC_HEIGHT: u16 = 20;
/// 窗口小于这个尺寸就只画一行提示。
pub const MIN_WIDTH: u16 = 60;
pub const MIN_HEIGHT: u16 = 15;
/// 歌词块高度上限（行）。
pub const MAX_LYRIC_HEIGHT: u16 = 12;

/// 骨架切出来的几块。
pub struct Areas {
    pub content: Rect,
    pub lyric: Option<Rect>,
    pub player: Rect,
    pub status: Rect,
}

/// 切骨架。`lyric_wanted` 由调用方判断（有没有在放 + 歌词拿没拿到 + 面板开没开）。
///
/// 菜单栏去掉后左右只有一列，宽度不再是变量：内容区吃满全宽，歌词块在最下面。
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
        // 歌词块只在「正在放 + 有歌词 + 面板开着」时占位，不播放时不画空框。
        // 全屏视图时底部不再单开一块——同一份歌词没有画两遍的道理。
        let lyric_wanted =
            self.show_lyric && self.lyric.is_some() && self.player_active() && !self.now_playing_full;
        let areas = layout(area, lyric_wanted);
        if self.now_playing_full {
            self.draw_lyric(frame, areas.content);
        } else {
            self.draw_content(frame, areas.content);
        }
        if let Some(lyric) = areas.lyric {
            self.draw_lyric(frame, lyric);
        }
        self.draw_player(frame, areas.player);
        self.draw_status(frame, areas.status);
        // 浮层只盖**内容区**：播放条与状态行是常驻的，不该被挡住
        // （全屏视图也是这个规矩）。
        if self.help_open {
            self.draw_help(frame, area);
        } else if self.queue_open {
            self.draw_queue(frame, areas.content);
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
        keys::Context::Content => {
            "Enter 播放 · Space 暂停 · n/b 切歌 · / 筛选 · f 收藏 · p 加歌单 · ? 帮助"
        }
        keys::Context::NowPlaying => {
            "Y/Esc 退出 · Space 暂停 · n/b 切歌 · ←/→ 10 秒 · +/- 音量 · ? 帮助"
        }
        keys::Context::SearchInput => "输入关键词 · Enter 搜索 · Esc 退出输入",
        keys::Context::FilterInput => "输入即筛选 · Enter 确认 · Esc 清空",
        keys::Context::PlaylistPicker => "j/k 选歌单 · Enter 确认 · Esc 取消",
        keys::Context::HelpOverlay => "Esc 或 ? 关闭帮助",
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

    use crate::tui::queue;
    use crate::tui::state::Menu;
    use crate::tui::test_support::{test_app, test_song};

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
        app.status = "已加载《嗯》".to_string();
        let text = frame_flat(&mut app, 160, 40);
        assert!(text.contains("我的歌单›嗯"), "面包屑要带层级：{text}");
        // 菜单栏没了：整帧里不该再出现菜单那几行。
        assert!(!text.contains("♥我的收藏"), "菜单项不该再画出来：{text}");
        assert!(text.contains("已加载《嗯》"), "状态行右侧：{text}");
        assert!(text.contains("Enter播放"), "状态行左侧的键位提示不能被状态顶掉：{text}");
        assert!(text.contains("已停止"), "空闲时播放条也要在：{text}");
    }

    /// 窄屏不再有「页签行」这一层：内容区就是全部宽度（画出来的东西和宽屏一致）。
    #[test]
    fn a_narrow_window_has_no_tab_row_any_more() {
        let (mut app, _runtime) = test_app();
        let text = frame_flat(&mut app, 90, 24);
        assert!(!text.contains("♥我的收藏"), "窄屏也不该有页签行：{text}");
        assert!(text.contains("我的收藏"), "当前屏写在内容块标题里：{text}");
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
        let text = frame_flat(&mut app, 160, 40);
        assert!(text.contains("帮助"), "浮层标题");
        assert!(
            text.contains("切屏1我的收藏·2我的歌单·3搜索·4历史"),
            "切屏表：{text}"
        );
        assert!(text.contains("歌单Enter进入曲目"), "当前屏那一行要跟着菜单变：{text}");
        assert!(text.contains("关闭Esc或?"), "关闭方式");
    }

    /// 全屏视图：歌词画在内容区，底部不再另开一块（同一份歌词不画两遍）。
    #[test]
    fn fullscreen_lyric_takes_over_the_content_area() {
        let (mut app, _runtime) = test_app();
        app.nav.favorites.set_items(vec![test_song("a")]);
        app.lyric = Some(crate::tui::test_support::test_lyric());
        app.now_playing_full = true;
        let text = frame_flat(&mut app, 120, 30);
        // 空闲时没有歌名，标题就只有「歌词」——不写行数，也不写操作提示。
        assert!(text.contains("歌词"), "内容区标题变成歌词：{text}");
        assert!(!text.contains("行）"), "标题不该写行数：{text}");
        // 操作提示统一在底部状态行：标题里没有，状态行里得有（这才是「统一交互」）。
        let rows = frame_rows(&mut app, 120, 30);
        let title_row = rows
            .iter()
            .find(|row| row.starts_with("┌歌词"))
            .expect("歌词块标题");
        assert!(!title_row.contains("退出"), "标题不写操作提示：{title_row}");
        let status_row = rows.last().expect("状态行");
        assert!(
            status_row.contains("Y/Esc退出"),
            "操作提示该在状态行：{status_row}"
        );
        assert!(text.contains("第1句"), "歌词正文画出来了：{text}");
        assert_eq!(text.matches("歌词").count(), 1, "只有一块歌词：{text}");
        assert!(!text.contains("歌a"), "列表让位给歌词：{text}");

        // 关掉全屏就回到列表。底部那块歌词画不画是另一条规则（只在播放中开），
        // 这里引擎是空闲的，所以它本来就不出现。
        app.now_playing_full = false;
        let back = frame_flat(&mut app, 120, 30);
        assert!(back.contains("歌a"), "回到列表：{back}");
        assert!(!back.contains("歌词"), "回到列表就没有歌词块了：{back}");
    }

    /// 全屏视图里当前句是高亮的（黄 + 粗），别的行不是——看真实单元格的颜色。
    #[test]
    fn the_current_lyric_line_is_highlighted() {
        let (mut app, _runtime) = test_app();
        app.lyric = Some(crate::tui::test_support::test_lyric());
        app.now_playing_full = true;
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
        assert!(history.contains("历史（2条）"), "历史标题带条数：{history}");

        app.switch_menu(Menu::Favorites);
        app.nav.favorites.set_items(vec![test_song("a")]);
        let favorites = frame_flat(&mut app, 120, 30);
        assert!(favorites.contains("我的收藏"), "{favorites}");
        assert!(!favorites.contains("历史（"), "切走之后不该还挂着历史标题：{favorites}");
    }

    /// 回归：在别的屏播放（= 记一条播放历史）之后，那一屏的标题不能被改写成「历史（N 条）」。
    ///
    /// 以前标题是存在 `breadcrumb` 里的，`refresh_history_view` 一跑就把它覆盖成历史屏的标题。
    #[test]
    fn recording_history_does_not_rewrite_another_screens_title() {
        let (mut app, _runtime) = test_app();
        app.switch_menu(Menu::Favorites);
        app.nav.favorites.set_items(vec![test_song("a")]);
        assert!(frame_flat(&mut app, 120, 30).contains("我的收藏"));

        // 正好是播放一首歌之后走的那条路：记一笔历史 + 重建历史屏。
        app.record_history(&test_song("b"));
        let text = frame_flat(&mut app, 120, 30);
        assert!(text.contains("我的收藏"), "标题还是当前屏：{text}");
        assert!(!text.contains("历史（"), "不该出现历史标题：{text}");
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
}

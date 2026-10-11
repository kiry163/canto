//! 浮层：帮助浮层与歌单选择框（队列浮层在 Task 12 加在这里）。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, List, ListItem, Paragraph, Wrap};

use super::super::state::{App, Menu};
use super::format_duration;
use super::highlight;

impl App {
    pub(super) fn draw_picker(&mut self, frame: &mut Frame, area: Rect) {
        let Some((playlists, state, song)) = self.picker.as_mut() else {
            return;
        };
        let width = area.width.saturating_sub(4).min(56);
        let height = playlists.len() as u16 + 2;
        let popup = super::overlay_rect(area, width, height);
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

}

impl App {
    /// 帮助浮层：按当前屏列出**真的接了行为**的键，底部一行说明怎么关。这是唯一的键位表。
    pub(super) fn draw_help(&self, frame: &mut Frame, area: Rect) {
        let lines = help_lines(self.nav.menu);
        // 宽度要装得下最长那行（「歌曲 …」约 62 个显示列），否则 Wrap 会把它折成两行、
        // 把最后一行顶出框外。
        let height = lines.len() as u16 + 2;
        let popup = super::overlay_rect(area, 72, height);
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(lines)
                .block(Block::bordered().title(" 帮助 "))
                .wrap(Wrap { trim: false }),
            popup,
        );
    }
}

/// 每屏一套键位说明：切屏 + 全局 + 播放 + 歌曲 + 这一屏特有的 + 关闭方式。
///
/// **这里是唯一的键位表**：菜单栏没了，切屏只有数字键，得让人在哪儿都能查到。
fn help_lines(menu: Menu) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from("切屏  1 我的收藏 · 2 我的歌单 · 3 搜索 · 4 历史"),
        Line::from("全局  / 筛选当前列表 · ? 帮助 · q 退出"),
        Line::from("播放  Space 暂停 · n/b 切歌 · ←/→ 10 秒 · +/- 音量"),
        Line::from("歌曲  Enter 播放 · f 收藏 · d 下载 · p 加进歌单 · x 从歌单移除"),
    ];
    lines.push(match menu {
        Menu::Favorites => Line::from("收藏  Enter 播放选中 · j/k 选曲目 · / 筛选"),
        Menu::Playlists => Line::from("歌单  Enter 进入曲目 · Esc 回歌单列表 · / 筛选"),
        Menu::Search => Line::from("搜索  Enter 搜 · Esc 取消输入 · 结果里 Enter 播放"),
        Menu::History => Line::from("历史  Enter 从那首重播 · / 筛选"),
    });
    lines.push(Line::from("会话  L 登出（清空本地会话，回登录屏）"));
    lines.push(Line::from("关闭  Esc 或 ?"));
    lines
}

impl App {
    /// 队列浮层：能看到接下来播什么，也能跳播 / 删除 / 清空。
    pub(super) fn draw_queue(&mut self, frame: &mut Frame, area: Rect) {
        let playing = self.queue.index();
        let items: Vec<ListItem> = self
            .queue
            .items()
            .iter()
            .enumerate()
            .map(|(index, song)| {
                let marker = if index == playing { "♪" } else { " " };
                ListItem::new(format!(
                    "{marker} {:>5}  {} — {}",
                    format_duration(std::time::Duration::from_secs(song.duration_secs)),
                    song.title,
                    song.singers,
                ))
            })
            .collect();
        let title = format!(
            " 队列 {}/{} · 来自《{}》 · {} ",
            playing + 1,
            self.queue.len(),
            self.queue.origin(),
            self.queue.mode().label()
        );
        let height = items.len() as u16 + 2;
        let popup = super::overlay_rect(area, area.width.saturating_sub(8).min(72), height);
        frame.render_widget(Clear, popup);
        let list = List::new(items)
            .block(Block::bordered().title(title))
            .highlight_style(highlight())
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(list, popup, &mut self.queue_state);
    }
}

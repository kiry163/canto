//! 浮层：帮助浮层、歌单选择框与队列浮层。

use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, List, ListItem, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::config::HotkeyConfig;
use crate::model::Song;

use super::super::state::{App, Menu, Screen};
use super::{highlight, rows};

impl App {
    /// `P` 的本地歌单选择框：第一行固定是「新建…」，下面才是已有的歌单。
    pub(super) fn draw_local_picker(&mut self, frame: &mut Frame, area: Rect) {
        let Some((playlists, state, song)) = self.local_picker.as_mut() else {
            return;
        };
        let width = area.width.saturating_sub(4).min(56);
        let height = playlists.len() as u16 + 3;
        let popup = super::overlay_rect(area, width, height);
        frame.render_widget(Clear, popup);
        // 第一行是「新建…」，缩进跟歌单行走同一份口径（计数那 5 列留白）。
        let mut items = vec![ListItem::new(super::playlist_row(None, "新建…"))];
        items.extend(playlists.iter().map(|playlist| {
            ListItem::new(super::playlist_row(Some(playlist.songs.len() as u64), &playlist.name))
        }));
        let list = List::new(items)
            .block(Block::bordered().title(format!(" 把《{}》加进本地歌单 ", song.title)))
            .highlight_style(highlight())
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(list, popup, state);
    }

    /// `p`：加入歌单的选择框（QQ 那套）。
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
            .map(|playlist| {
                ListItem::new(super::playlist_row(Some(playlist.song_count), &playlist.name))
            })
            .collect();
        let list = List::new(items)
            .block(Block::bordered().title(format!(" 把《{}》加进 ", song.title)))
            .highlight_style(highlight())
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(list, popup, state);
    }

}

impl App {
    /// 帮助浮层：按当前屏列出**真的接了行为的**键（含 `[hotkeys]` 里那几条全局热键），
    /// 左侧按键、右侧说明；装不下时 `j`/`k` 滚。这是唯一的键位表。
    ///
    /// 首页要单独认（它不是 `Menu` 的一员）：在首页上按 `?` 得看首页那套键，而不是
    /// `nav.menu` 停在的那一屏的。
    ///
    /// 浮层里**不写操作提示**：那统一归状态行（见 `draw::hints`），所以浮层高度就是
    /// 内容 + 上下边框。
    pub(super) fn draw_help(&mut self, frame: &mut Frame, area: Rect) {
        let screen = match self.screen {
            Screen::Home => None,
            _ => Some(self.nav.menu),
        };
        let rows = help_rows(screen, &self.config.hotkeys);
        let key_col = key_column(&rows);
        // 内容行数有上限（浮层不是全屏页面）：多了就滚。
        let height = rows.len().min(HELP_ROWS) as u16 + 2;
        let popup = super::overlay_rect(area, table_width(&rows, key_col), height);
        frame.render_widget(Clear, popup);

        // 可见行数由**框的实际高度**决定（窗口矮的时候 `overlay_rect` 还会再夹一次），
        // 所以滚动量在这儿回夹：滚到底之后再按 `j` 就真的不动了。
        let viewport = popup.height.saturating_sub(2) as usize;
        let max_scroll = rows.len().saturating_sub(viewport);
        self.help_scroll = self.help_scroll.min(max_scroll as u16);

        let start = self.help_scroll as usize;
        let lines: Vec<Line> = rows[start..(start + viewport).min(rows.len())]
            .iter()
            .map(|row| row.line(key_col))
            .collect();
        frame.render_widget(Paragraph::new(lines).block(super::panel_block(" 帮助 ")), popup);
    }
}

/// 浮层里最多显示多少行内容（再多就滚）。
const HELP_ROWS: usize = 18;

/// 帮助表的一行。
enum Row {
    /// 分组标题：占满一整行、弱色。
    Section(&'static str),
    /// 写死在表里的键位。
    Keys { keys: &'static str, help: &'static str },
    /// 全局热键：键列是从配置里读出来的，要现拼。
    Hotkey { keys: String, help: &'static str },
}

impl Row {
    /// 左键右述；分组标题没有键列。（说明永远是表里写死的那份，键列可能是配置里读出来的。）
    fn cells(&self) -> Option<(&str, &'static str)> {
        match self {
            Row::Section(_) => None,
            Row::Keys { keys, help } => Some((keys, help)),
            Row::Hotkey { keys, help } => Some((keys, help)),
        }
    }

    /// 渲染成一行：分组标题弱色顶到最左；键行把键列补到 `key_col`、空两格再接说明，
    /// 于是所有说明都从同一列开始。
    fn line(&self, key_col: usize) -> Line<'static> {
        match self {
            Row::Section(title) => Line::from(*title).style(Style::new().fg(Color::DarkGray)),
            Row::Keys { keys, help } => Row::text(keys, help, key_col),
            Row::Hotkey { keys, help } => Row::text(keys, help, key_col),
        }
    }

    fn text(keys: &str, help: &str, key_col: usize) -> Line<'static> {
        let pad = key_col.saturating_sub(keys.width()) + 2;
        Line::from(format!("{keys}{}{help}", " ".repeat(pad)))
    }
}

/// 每屏一套键位说明。`Some(menu)` = 那四屏之一：屏专属 + 列表 + 切屏 + 全局 + 播放 + 歌曲 +
/// 全局热键；`None` = 首页（它不是 `Menu` 的一员，`Menu` 的含义是「一份歌曲列表 + 选中位置 +
/// 筛选」）：切屏 + 全局（不含 `/`）+ 播放 + 全局热键——**通用的挪格 / `Enter` 不进表**（那些
/// 谁都知道，状态行那句提示里也写着），而首页上没有的段整段不列（`d`、`a`、`PgUp`、`/` 在它
/// 那儿一个都没接，列出来就是骗人）。
///
/// **这是唯一的键位表**：菜单栏没了，切屏只有数字键，得让人在哪儿都能查到。同一按键只写
/// 一次——`/`、`Enter`、`L` 这些通用的在后面几块写过，当前屏那节就只留这一屏特有的。
///
/// 当前屏那节排在最前、且**只在真有屏专属键时才在**（收藏屏就没有：那点内容所有列表通用，
/// 已经在「列表」段里）。表格比浮层长（内容上限 18 行），最常查的得在上面。
fn help_rows(screen: Option<Menu>, hotkeys: &HotkeyConfig) -> Vec<Row> {
    let mut rows = Vec::new();
    // 首页（`None`）单开一段只为 `Tab`：挪格那一套**只在首页**成立（列表屏是 `j`/`k`），
    // 不写出来没人猜得到。`Enter` 进屏到处都一样，照旧不进表。
    let list = screen.is_some();
    if screen.is_none() {
        rows.extend([
            Row::Section("首页"),
            Row::Keys { keys: "Tab / Shift+Tab", help: "挪格（到最后 / 最开始那一格会绕回去）" },
        ]);
    }
    if let Some(menu) = screen {
        let (title, section): (&'static str, Vec<Row>) = match menu {
            // 收藏屏没有专属键：`j`/`k` 选曲目、`/` 筛选都是所有列表共有的。
            Menu::Favorites => ("", Vec::new()),
            // 本地歌曲也没有屏专属段：`Enter`/`a`/`A`/`r` 已经在下面「歌曲」那一段里。
            Menu::LocalLibrary => ("", Vec::new()),
            Menu::LocalPlaylists => (
                "本地歌单",
                vec![
                    Row::Keys { keys: "Enter", help: "进入该歌单的曲目" },
                    Row::Keys { keys: "Esc", help: "回歌单列表" },
                    Row::Keys { keys: "N", help: "新建歌单" },
                    Row::Keys { keys: "R", help: "改名" },
                    Row::Keys { keys: "D", help: "删除这张歌单（按两次）" },
                ],
            ),
            Menu::Playlists => (
                "歌单",
                vec![
                    Row::Keys { keys: "Enter", help: "进入该歌单的曲目" },
                    Row::Keys { keys: "Esc", help: "回歌单列表" },
                ],
            ),
            Menu::Search => (
                Menu::Search.title(),
                vec![
                    Row::Keys { keys: "Enter", help: "搜索" },
                    Row::Keys { keys: "Esc", help: "取消输入" },
                ],
            ),
            Menu::History => (
                Menu::History.title(),
                vec![Row::Keys { keys: "Enter", help: "从那首重播" }],
            ),
        };
        if !section.is_empty() {
            rows.push(Row::Section(title));
            rows.extend(section);
        }
        rows.extend([
            Row::Section("列表"),
            Row::Keys { keys: "j / k、↑ / ↓", help: "选曲目" },
            Row::Keys { keys: "g / G、Home / End", help: "首 / 尾" },
            Row::Keys { keys: "PgUp / PgDn", help: "翻页（Ctrl+U / Ctrl+D）" },
        ]);
    }
    rows.extend([
        Row::Section("切屏"),
        Row::Keys { keys: "0", help: "首页" },
        Row::Keys { keys: "1", help: Menu::Favorites.title() },
        Row::Keys { keys: "2", help: Menu::Playlists.title() },
        Row::Keys { keys: "3", help: Menu::Search.title() },
        Row::Keys { keys: "4", help: Menu::History.title() },
        Row::Section("全局"),
    ]);
    // `/` 筛的是列表：首页上没有列表。
    if list {
        rows.push(Row::Keys { keys: "/", help: "筛选当前列表" });
    }
    rows.extend([
        Row::Keys { keys: "y", help: "歌词栏开 / 关" },
        Row::Keys { keys: "v", help: "频谱区开 / 关" },
        Row::Keys { keys: "?", help: "帮助" },
        Row::Keys { keys: "L", help: "登出（清空本地会话）" },
        Row::Keys { keys: "q", help: "退出" },
        Row::Section("播放"),
        Row::Keys { keys: "Space", help: "播放 / 暂停" },
        Row::Keys { keys: "n / b", help: "下一首 / 上一首" },
        Row::Keys { keys: "← / →", help: "后退 / 前进 10 秒" },
        Row::Keys { keys: "+ / -", help: "音量 ±5%" },
    ]);
    // 本地歌曲屏：曲目那一套少一半——本地歌没有 QQ 身份，`d`/`p`/`x` 不绑，别许诺。
    if matches!(screen, Some(Menu::LocalLibrary)) {
        rows.extend([
            Row::Section("歌曲"),
            Row::Keys { keys: "Enter", help: "播放选中" },
            Row::Keys { keys: "a / A", help: "追加到队尾 / 插到下一首" },
            Row::Keys { keys: "P", help: "加进本地歌单" },
            Row::Keys { keys: "r", help: "重扫（本地歌曲目录 + 下载索引）" },
        ]);
    } else if matches!(screen, Some(Menu::LocalPlaylists)) {
        rows.extend([
            Row::Section("本地歌单里的歌"),
            Row::Keys { keys: "Enter", help: "播放选中" },
            Row::Keys { keys: "a / A", help: "追加到队尾 / 插到下一首" },
            Row::Keys { keys: "x", help: "移出这张歌单" },
        ]);
    } else if list {
        rows.extend([
            Row::Section("歌曲"),
            Row::Keys { keys: "Enter", help: "播放选中" },
            Row::Keys { keys: "a / A", help: "追加到队尾 / 插到下一首" },
            Row::Keys { keys: "d", help: "下载（含 .lrc）" },
            Row::Keys { keys: "p", help: "加进歌单" },
            Row::Keys { keys: "x", help: "从歌单移除" },
        ]);
    }
    let global = hotkey_rows(hotkeys);
    if !global.is_empty() {
        rows.push(Row::Section("全局热键（后台也能按）"));
        rows.extend(global);
    }
    rows
}

/// 全局热键那几行：组合从配置读（`bindings()` 已经跳过空串 = 禁用），说明跟默认组合同一张表
/// （`hotkeys::DEFAULTS`），不另抄一份「动作 → 说明」。
fn hotkey_rows(hotkeys: &HotkeyConfig) -> Vec<Row> {
    // 这个平台一条都不注册（Linux）：别在帮助里许诺按得出来的键。
    if !crate::hotkeys::SUPPORTED {
        return Vec::new();
    }
    hotkeys
        .bindings()
        .into_iter()
        .filter_map(|binding| {
            let entry = crate::hotkeys::DEFAULTS.iter().find(|entry| entry.action == binding.action)?;
            Some(Row::Hotkey { keys: pretty(&binding.keys), help: entry.help })
        })
        .collect()
}

/// 键列宽：所有键里最宽的那个（说明从它右边固定两格开始）。
fn key_column(rows: &[Row]) -> usize {
    rows.iter().filter_map(Row::cells).map(|(keys, _)| keys.width()).max().unwrap_or(0)
}

/// 浮层宽度：最宽的一行（分组标题也算）+ 左右边框，夹在 `[40, 72]`
/// ——比 40 窄就成了细长条，比 72 宽在大屏上会横成一条带子。
fn table_width(rows: &[Row], key_col: usize) -> u16 {
    let widest = rows
        .iter()
        .map(|row| match row {
            Row::Section(title) => title.width(),
            _ => row.cells().map_or(0, |(_, help)| key_col + 2 + help.width()),
        })
        .max()
        .unwrap_or(0);
    (widest as u16 + 2).clamp(40, 72)
}

/// 配置里的组合（`ctrl+shift+right`）→ 键列里那副样子（`Ctrl+Shift+→`）。
///
/// 认不出的词只把首字母大写——组合是使用者自己写的，宁可显示得笨一点，也不能丢信息或翻错。
fn pretty(combo: &str) -> String {
    combo.split('+').map(pretty_key).collect::<Vec<_>>().join("+")
}

fn pretty_key(key: &str) -> String {
    match key.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => "Ctrl".to_string(),
        "alt" | "option" => "Alt".to_string(),
        "shift" => "Shift".to_string(),
        "space" => "Space".to_string(),
        "enter" | "return" => "Enter".to_string(),
        "tab" => "Tab".to_string(),
        "esc" | "escape" => "Esc".to_string(),
        "backspace" => "Backspace".to_string(),
        "delete" | "del" => "Del".to_string(),
        "insert" | "ins" => "Ins".to_string(),
        "up" | "arrowup" => "↑".to_string(),
        "down" | "arrowdown" => "↓".to_string(),
        "left" | "arrowleft" => "←".to_string(),
        "right" | "arrowright" => "→".to_string(),
        // `win`/`super`/`cmd` 各按各的写法显示（它们本来是同一个键的不同叫法，翻成一个
        // 反而会跟使用者在配置里写的对不上）。`f5`、`a` 这类落到这里，就是首字母大写。
        other => {
            let mut characters = other.chars();
            match characters.next() {
                Some(first) => first.to_uppercase().chain(characters).collect(),
                None => String::new(),
            }
        }
    }
}

impl App {
    /// 队列浮层：能看到接下来播什么，也能跳播 / 删除 / 清空。
    /// 队列浮层：跟别的曲目列表同一副表（列集少「音质/体积/备注」，见 `rows::queue_columns`）。
    ///
    /// `♪` 按**队列下标**标，不按 mid：队列里同一首可能出现两次，按 mid 会两行一起亮。
    /// 表头那一行总是画——浮层高度是照着内容算的，不存在「抢矮屏里那一行数据」的问题。
    pub(super) fn draw_queue(&mut self, frame: &mut Frame, area: Rect) {
        let current = self.queue.index();
        // 标题只报「看到第几首 / 队列多长」和播放模式；来源（来自《某歌单》）不写，
        // 队列里可能混着好几个来源，写了反而误导。
        let title = format!(
            " 队列 {}/{} · {} ",
            current + 1,
            self.queue.len(),
            self.queue.mode().label()
        );
        let block = Block::bordered().title(title);
        let height = self.queue.len() as u16 + 3;
        let popup = super::overlay_rect(area, area.width.saturating_sub(8).min(72), height);
        let inner = block.inner(popup);
        let (head, list_area) = rows::group(inner, 1);
        let columns = rows::fit(&rows::queue_columns(), rows::row_width(list_area.width));

        let want = self.config.playback.quality;
        let library: HashMap<&str, &Song> =
            self.library.iter().map(|song| (song.mid.as_str(), song)).collect();
        let items: Vec<ListItem> = self
            .queue
            .items()
            .iter()
            .enumerate()
            .map(|(index, song)| {
                let mut row = self.song_row(song, &library, None, want);
                row.playing = index == current;
                ListItem::new(rows::line(&columns, &row))
            })
            .collect();

        frame.render_widget(Clear, popup);
        frame.render_widget(block, popup);
        if let Some(head) = head {
            frame.render_widget(Paragraph::new(rows::header(&columns)), head);
        }
        let list = List::new(items)
            .highlight_style(highlight())
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(list, list_area, &mut self.queue_state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    use crate::config::HotkeyConfig;
    use crate::tui::state::Menu;

    /// 组合从配置里来，写的是 crate 那套小写别名；表里得是给人看的样子。
    #[test]
    fn pretty_rewrites_the_configured_combos() {
        assert_eq!(pretty("ctrl+shift+space"), "Ctrl+Shift+Space");
        assert_eq!(pretty("ctrl+shift+right"), "Ctrl+Shift+→");
        assert_eq!(pretty("ctrl+shift+left"), "Ctrl+Shift+←");
        assert_eq!(pretty("ctrl+shift+up"), "Ctrl+Shift+↑");
        assert_eq!(pretty("ctrl+shift+down"), "Ctrl+Shift+↓");
        assert_eq!(pretty("ctrl+shift+l"), "Ctrl+Shift+L");
        assert_eq!(pretty("f5"), "F5");
        assert_eq!(pretty("win+a"), "Win+A", "配置里写 win，表里也写 Win（macOS 上它就是 cmd）");
        assert_eq!(pretty("super+space"), "Super+Space", "别名各按各的写法显示，不硬翻");
        assert_eq!(pretty("cmd+q"), "Cmd+Q");
        assert_eq!(pretty("mediaplaypause"), "Mediaplaypause", "认不出的词只首字母大写，不丢信息");
        assert_eq!(pretty(""), "", "空串不该进表，但也不该炸");
    }

    /// 本地歌曲屏的帮助：列 `r`，不列 QQ 专有的 `d`/`p`/`x`（本地歌没有 QQ 身份）。
    #[test]
    fn the_local_screen_help_omits_qq_only_keys() {
        let rows = help_rows(
            Some(crate::tui::state::Menu::LocalLibrary),
            &HotkeyConfig::default(),
        );
        let cells: Vec<(&str, &str)> = rows.iter().filter_map(Row::cells).collect();
        assert!(cells.iter().any(|(keys, _)| *keys == "r"), "{cells:?}");
        for dead in ["d", "p", "x"] {
            assert!(
                !cells.iter().any(|(keys, _)| *keys == dead),
                "本地屏不绑 `{dead}`：{cells:?}"
            );
        }
    }

    /// 表就是「左键右述」两列：分组标题没有键列，键行两边都有。
    #[test]
    fn the_table_pairs_every_key_with_a_description() {
        let rows = help_rows(Some(Menu::Favorites), &HotkeyConfig::default());
        let cells: Vec<(&str, &str)> = rows.iter().filter_map(Row::cells).collect();
        assert!(cells.contains(&("1", "在线收藏")), "{cells:?}");
        assert!(cells.contains(&("0", "首页")), "首页入口要在「切屏」段里：{cells:?}");
        assert!(cells.contains(&("?", "帮助")), "{cells:?}");
        assert!(cells.contains(&("Space", "播放 / 暂停")), "{cells:?}");
        assert!(cells.contains(&("L", "登出（清空本地会话）")), "登出是全局键：{cells:?}");
        assert!(cells.contains(&("y", "歌词栏开 / 关")), "{cells:?}");
        assert!(cells.contains(&("v", "频谱区开 / 关")), "{cells:?}");
        assert!(
            !cells.iter().any(|(_, help)| help.contains("全屏")),
            "「全屏」这个模式没了：{cells:?}"
        );
        // 方向键也在表里（它们跟 `j`/`k` 是同一件事的两种按法）。
        assert!(cells.contains(&("j / k、↑ / ↓", "选曲目")), "{cells:?}");
        assert!(cells.contains(&("PgUp / PgDn", "翻页（Ctrl+U / Ctrl+D）")), "{cells:?}");
        // 老版把「/ 筛选」在「全局」和当前屏各写了一遍，表里只该有一行。
        assert_eq!(
            cells.iter().filter(|(_, help)| help.contains("筛选")).count(),
            1,
            "同一个键不该出现两次：{cells:?}"
        );
        // 分组标题占整行，不进键列；收藏屏没有屏专属段。
        let sections: Vec<&str> = rows
            .iter()
            .filter_map(|row| match row {
                Row::Section(title) => Some(*title),
                Row::Keys { .. } | Row::Hotkey { .. } => None,
            })
            .collect();
        assert_eq!(
            &sections[..5],
            ["列表", "切屏", "全局", "播放", "歌曲"],
            "收藏屏没有专属段（`j`/`k`、`/` 都是所有列表共有的）"
        );
        assert!(!sections.contains(&"会话"), "登出挪进「全局」了，不再单开一段");
    }

    /// 屏专属段：只有真有专属键的屏才有，且排在最前（表格比浮层长，最常查的得在上面）。
    /// 首页（`None`）自成一套：没有「选中的歌」，列表 / 歌曲两段整段不列。
    #[test]
    fn the_screen_section_follows_the_menu() {
        let hotkeys = HotkeyConfig::default();
        let rows = |screen| help_rows(screen, &hotkeys);
        let sections = |screen| -> Vec<&'static str> {
            rows(screen)
                .iter()
                .filter_map(|row| match row {
                    Row::Section(title) => Some(*title),
                    Row::Keys { .. } | Row::Hotkey { .. } => None,
                })
                .collect()
        };
        let helps = |screen| -> Vec<&'static str> {
            rows(screen).iter().filter_map(Row::cells).map(|(_, help)| help).collect()
        };
        assert_eq!(sections(Some(Menu::Favorites))[0], "列表", "收藏屏直接从「列表」开始");
        assert_eq!(sections(Some(Menu::Playlists))[0], "歌单");
        assert!(helps(Some(Menu::Playlists)).contains(&"进入该歌单的曲目"));
        assert!(!helps(Some(Menu::Favorites)).contains(&"进入该歌单的曲目"), "切屏换了那一节就该换");
        assert_eq!(sections(Some(Menu::History))[0], "播放历史");

        // 首页单开一段只为 `Tab`（挪格那一套只在首页成立）；`Enter` 到处都一样，不进表。
        assert_eq!(sections(None)[0], "首页", "{:?}", sections(None));
        assert_eq!(
            sections(None)[1],
            "切屏",
            "首页那一段之后照旧是「切屏」：{:?}",
            sections(None)
        );
        assert!(
            helps(None).iter().any(|help| help.starts_with("挪格")),
            "首页得说清 `Tab` 挪格：{:?}",
            helps(None)
        );
        assert!(
            !helps(None).iter().any(|help| matches!(*help, "选入口" | "进选中那一屏" | "首 / 尾")),
            "`Enter` 进屏到处都一样，不必进帮助：{:?}",
            helps(None)
        );
        assert!(!sections(None).contains(&"列表") && !sections(None).contains(&"歌曲"));
    }

    /// 首页那一套：列表 / 歌曲两段的键一个都不许在（`Context::Home` 里一个都没绑），
    /// `Enter` 进屏也不进表。
    #[test]
    fn the_home_table_lists_no_keys_the_home_ignores() {
        let rows = help_rows(None, &HotkeyConfig::default());
        let cells: Vec<(&str, &str)> = rows.iter().filter_map(Row::cells).collect();
        for dead in [
            "选曲目",
            "翻页（Ctrl+U / Ctrl+D）",
            "筛选当前列表",
            "播放选中",
            "下载（含 .lrc）",
            "加进歌单",
            "从歌单移除",
            "选入口",
            "进选中那一屏",
        ] {
            assert!(
                !cells.iter().any(|(_, help)| *help == dead),
                "首页上按不出来（或通用到不必列），别写进表：{dead}（{cells:?}）"
            );
        }
        assert!(cells.contains(&("0", "首页")), "切屏那一节首页也得有：{cells:?}");
    }

    /// 全局热键那一节读的是**配置里的实际组合**，不是默认值。
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    #[test]
    fn the_hotkey_section_reads_the_configured_combos() {
        let rows = help_rows(Some(Menu::Favorites), &HotkeyConfig::default());
        let cells: Vec<(&str, &str)> = rows.iter().filter_map(Row::cells).collect();
        assert_eq!(
            cells.iter().filter(|(keys, _)| keys.starts_with("Ctrl+")).count(),
            7,
            "默认七条都该在表里：{cells:?}"
        );
        assert!(cells.contains(&("Ctrl+Shift+Space", "播放 / 暂停")), "{cells:?}");
        assert!(cells.contains(&("Ctrl+Shift+→", "下一首")), "{cells:?}");
        assert!(cells.contains(&("Ctrl+Shift+D", "桌面歌词开 / 关")), "{cells:?}");

        // 空串 = 不注册 = 不列；改过的组合照着显示。
        let hotkeys = HotkeyConfig {
            desktop_lyrics_lock: String::new(),
            next: "ctrl+alt+n".to_string(),
            ..HotkeyConfig::default()
        };
        let rows = help_rows(Some(Menu::Favorites), &hotkeys);
        let cells: Vec<(&str, &str)> = rows.iter().filter_map(Row::cells).collect();
        assert!(
            !cells.iter().any(|(_, help)| *help == "桌面歌词锁定 / 解锁"),
            "禁用的那条不该在表里：{cells:?}"
        );
        assert!(cells.contains(&("Ctrl+Alt+N", "下一首")), "{cells:?}");
    }

    /// 这个平台一条都不注册（`hotkeys::SUPPORTED` 为假），就别在帮助里许诺。
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    #[test]
    fn the_hotkey_section_is_absent_where_nothing_registers() {
        let rows = help_rows(Some(Menu::Favorites), &HotkeyConfig::default());
        let cells: Vec<(&str, &str)> = rows.iter().filter_map(Row::cells).collect();
        assert!(
            !cells.iter().any(|(keys, _)| keys.contains("Ctrl+") || keys.contains("Win+")),
            "不注册的关键字不该出现：{cells:?}"
        );
    }

    /// 键列宽要装得下最宽的那个键（美化后的组合最长）。
    #[test]
    fn the_key_column_fits_the_widest_key() {
        let rows = help_rows(Some(Menu::Favorites), &HotkeyConfig::default());
        let key_col = key_column(&rows);
        for (keys, _) in rows.iter().filter_map(Row::cells) {
            assert!(key_col >= keys.width(), "{keys} 比键列还宽");
        }
        assert!(key_col > 0);
    }

    /// 两列的账：键补到键列宽、空两格、接说明；分组标题就是标题本身（不参与两列）。
    #[test]
    fn the_columns_are_padded_to_the_widest_key() {
        assert_eq!(Row::Section("切屏").line(10).to_string(), "切屏");
        let row = Row::Keys { keys: "1", help: "在线收藏" };
        // 键列 10、键 1 个字 → 补 9 格，再空 2 格。
        assert_eq!(row.line(10).to_string(), format!("1{}在线收藏", " ".repeat(11)));
        // 键本身比键列宽时也不许少空格（宁可挤一格，也不能把两列粘一起）。
        let narrow = Row::Keys { keys: "Space", help: "播放 / 暂停" };
        assert_eq!(narrow.line(1).to_string(), "Space  播放 / 暂停");
    }
}

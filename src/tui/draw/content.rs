//! 内容区：按当前屏渲染。

use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph};

use crate::model::{Quality, Song};
use crate::player::Phase;

use super::super::state::{App, Menu};
use super::{highlight, panel_block, rows};

impl App {
    pub(super) fn draw_content(&mut self, frame: &mut Frame, area: Rect) {
        let title = self.content_title();
        // 筛选框开着时，第一行让给输入行（跟搜索屏的输入行一个样子）。
        let area = if self.filter_input {
            let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
            self.draw_filter_input(frame, rows[0]);
            rows[1]
        } else {
            area
        };
        // 名字输入行占顶上那一行（输入态进来就画，跟筛选输入行同一条道理）。
        let area = if self.local_input.is_some() {
            let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
            self.draw_local_input(frame, rows[0]);
            rows[1]
        } else {
            area
        };
        match self.nav.menu {
            Menu::Playlists if self.nav.shown.is_none() => {
                let items: Vec<ListItem> = self
                    .nav
                    .playlists
                    .visible_rows()
                    .map(|(playlist, _)| {
                        ListItem::new(super::playlist_row(Some(playlist.song_count), &playlist.name))
                    })
                    .collect();
                let list = List::new(items)
                    .block(panel_block(&title))
                    .highlight_style(highlight())
                    .highlight_symbol("▶ ");
                frame.render_stateful_widget(list, area, &mut self.nav.playlists.state);
            }
            Menu::LocalPlaylists if self.nav.local_shown.is_none() => {
                if self.nav.local_playlists.items().is_empty() {
                    frame.render_widget(super::empty_panel("还没有本地歌单 · N 新建一张", &title), area);
                } else {
                    let items: Vec<ListItem> = self
                        .nav
                        .local_playlists
                        .visible_rows()
                        .map(|(playlist, _)| {
                            ListItem::new(super::playlist_row(Some(playlist.songs.len() as u64), &playlist.name))
                        })
                        .collect();
                    let list = List::new(items)
                        .block(panel_block(&title))
                        .highlight_style(highlight())
                        .highlight_symbol("▶ ");
                    frame.render_stateful_widget(list, area, &mut self.nav.local_playlists.state);
                }
            }
            // 空歌单：进去之后列表是空的，得说一句「怎么往里加」，别只留一块空白。
            Menu::LocalPlaylists
                if self.nav.local_shown.is_some() && self.nav.local_tracks.items().is_empty() =>
            {
                frame.render_widget(super::empty_panel("空歌单 · 曲库里按 P 把歌加进来", &title), area);
            }
            Menu::Search => {
                // 输入行占第一行（Task 7 把它做成常驻），结果在下面。
                let rows =
                    Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
                let keyword = self.nav.search_text.clone();
                // 三种状态都是同一句标签（正在输入带光标 / 输入完了 / 还没输入）。
                // 「按 3 回到输入框」这类操作提示不写在这儿——统一放状态行与帮助浮层。
                let prompt = if self.nav.search_input {
                    Line::from(vec![
                        Span::styled("搜索：", Style::new().fg(Color::Yellow)),
                        Span::raw(keyword),
                        Span::styled("▏", Style::new().fg(Color::Yellow)),
                    ])
                } else {
                    Line::from(vec![
                        Span::styled("搜索：", Style::new().fg(Color::DarkGray)),
                        Span::raw(keyword),
                    ])
                };
                frame.render_widget(Paragraph::new(prompt), rows[0]);
                self.draw_songs(frame, rows[1], &title);
            }
            Menu::History => self.draw_history(frame, area, &title),
            _ => self.draw_songs(frame, area, &title),
        }
    }

    /// 内容区标题：面包屑 + 「这一屏有多少条」，外加「筛选」徽标。
    ///
    /// 计数挂在这里而不是状态行：它是**这个视图的属性**，不像通知那样说完就走；
    /// 位置也是现成的（这块标题本来就挂筛选徽标）。筛过之后只报命中/总数。
    fn content_title(&self) -> String {
        let (title, count, unit) = self.title_and_count();
        let keyword = self.nav.filter_text();
        if keyword.is_empty() {
            return format!(" {title} {count} {unit} ");
        }
        let (hits, total) = self.nav.filter_counts();
        format!(" {title} 筛选「{keyword}」{hits}/{total} ")
    }

    /// 面包屑（带层级）与这一屏的条数。
    ///
    /// 历史那屏的条数是现算的派生信息，不进导航状态——以前它存在标题里，
    /// 记录一条播放历史就会把**别的屏**的标题改写成「历史（N 条）」。
    fn title_and_count(&self) -> (String, usize, &'static str) {
        // 第二项是「有没有进到下一层」：两个屏各有各的那一层，别拿串了（在 `6` 上看到
        // 「本地歌单 0 首」而下面画着 N 张就是这么来的）。
        let nested = match self.nav.menu {
            Menu::LocalPlaylists => self.nav.local_shown.is_some(),
            _ => self.nav.shown.is_some(),
        };
        match (self.nav.menu, nested) {
            (Menu::Playlists, true) => (
                self.nav.title_chain().join(" › "),
                self.nav.tracks.items().len(),
                "首",
            ),
            (Menu::LocalLibrary, _) => (
                self.nav.menu.title().to_string(),
                self.nav.local_view.items().len(),
                "首",
            ),
            (Menu::LocalPlaylists, true) => (
                self.nav.title_chain().join(" › "),
                self.nav.local_tracks.items().len(),
                "首",
            ),
            (Menu::LocalPlaylists, false) => (
                self.nav.menu.title().to_string(),
                self.nav.local_playlists.items().len(),
                "张",
            ),
            (Menu::Playlists, false) => (
                self.nav.menu.title().to_string(),
                self.nav.playlists.items().len(),
                "个",
            ),
            (Menu::Favorites, _) => (
                self.nav.menu.title().to_string(),
                self.nav.favorites.items().len(),
                "首",
            ),
            (Menu::Search, _) => (
                self.nav.menu.title().to_string(),
                self.nav.search_results.items().len(),
                "首",
            ),
            (Menu::History, _) => (self.nav.menu.title().to_string(), self.history.len(), "条"),
        }
    }

    /// `/` 打开的筛选输入行。
    pub(super) fn draw_filter_input(&self, frame: &mut Frame, area: Rect) {
        let line = Line::from(vec![
            Span::styled("筛选：", Style::new().fg(Color::Yellow)),
            Span::raw(self.nav.filter_text().to_string()),
            Span::styled("▏", Style::new().fg(Color::Yellow)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    }

    /// 本地歌单的名字输入行（`N` 新建 / `R` 改名）。跟筛选输入行一样的道理：
    /// 不画出来的话，打进去的字看不见。
    pub(super) fn draw_local_input(&self, frame: &mut Frame, area: Rect) {
        let Some(input) = self.local_input.as_ref() else {
            return;
        };
        let what = input.label();
        let line = Line::from(vec![
            Span::styled(what, Style::new().fg(Color::Yellow)),
            Span::raw(input.text.clone()),
            Span::styled("▏", Style::new().fg(Color::Yellow)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    }

    /// 历史屏：跟别的曲目列表同一副表；备注列写「时间 · 来自《来源》」。
    ///
    /// 时长/体积来自记录里存的那份（Task 8），所以这一屏也是有话可说才画；
    /// 老记录没有就是 `--:--` / `—`，不写假的 0。
    fn draw_history(&mut self, frame: &mut Frame, area: Rect, title: &str) {
        let block = panel_block(title);
        if self.history.is_empty() {
            frame.render_widget(super::empty_panel("暂无播放历史", title), area);
            return;
        }
        let want = self.config.playback.quality;
        let playing = self.now_playing_mid();
        let library: HashMap<&str, &Song> =
            self.library.iter().map(|song| (song.mid.as_str(), song)).collect();

        let inner = block.inner(area);
        let (head, list_area) = rows::group(inner, rows::MIN_FOR_HEADER);
        let columns = rows::fit(&rows::history_columns(), rows::row_width(list_area.width));

        let items: Vec<ListItem> = self
            .nav
            .song_view()
            .visible_rows()
            .map(|(song, label)| {
                let row = self.song_row(song, &library, playing.as_deref(), want);
                // 历史自己的备注：`visible_rows()` 的第二项就是拼好的「时间 · 来自《来源》」。
                let row = rows::Row { note: Some(label), ..row };
                ListItem::new(rows::line(&columns, &row))
            })
            .collect();

        frame.render_widget(block, area);
        if let Some(head) = head {
            frame.render_widget(Paragraph::new(rows::header(&columns)), head);
        }
        let mut state = std::mem::take(&mut self.nav.song_view_mut().state);
        let list = List::new(items).highlight_style(highlight()).highlight_symbol("▶ ");
        frame.render_stateful_widget(list, list_area, &mut state);
        self.nav.song_view_mut().state = state;
    }

    /// 空曲库：说清去哪儿加目录（不是一片空白）。
    pub(super) fn draw_empty_library(&self, frame: &mut Frame, area: Rect) {
        let text = Line::from(vec![
            Span::styled("曲库是空的", Style::new().fg(Color::Yellow)),
            Span::raw("：在 config.toml 的 [library] dirs 里加目录，再按 r 重扫"),
        ]);
        frame.render_widget(Paragraph::new(text).block(panel_block(" 本地曲库 ")), area);
    }

    /// 曲目列表：所有「装着一堆歌」的屏共用（收藏 / 歌单曲目 / 搜索 / 本地曲库 / 本地歌单曲目）。
    ///
    /// 列集按屏声明（本地曲库没有缓存/下载两列），宽度按窗口让位（见 `rows::fit`）。表头画在
    /// 列表块**内部**（先 `Block::inner` 再 `rows::group`），所以 `draw::layout` 不用动。
    pub(super) fn draw_songs(&mut self, frame: &mut Frame, area: Rect, title: &str) {
        let columns = match self.nav.menu {
            Menu::LocalLibrary => rows::local_columns(),
            Menu::LocalPlaylists if self.nav.local_shown.is_some() => rows::local_playlist_columns(),
            _ => rows::track_columns(),
        };
        let want = self.config.playback.quality;
        let playing = self.now_playing_mid();
        // 本地条目要按 mid 查当前曲库：补齐时长/容器/大小，顺带判「已丢失」。每行
        // `iter().any()` 是 O(行×曲库)，这里一次建表。
        let library: HashMap<&str, &Song> =
            self.library.iter().map(|song| (song.mid.as_str(), song)).collect();

        let block = panel_block(title);
        let inner = block.inner(area);
        let (head, list_area) = rows::group(inner, rows::MIN_FOR_HEADER);
        let columns = rows::fit(&columns, rows::row_width(list_area.width));

        let items: Vec<ListItem> = self
            .nav
            .song_view()
            .visible_rows()
            .map(|(song, _)| {
                let row = self.song_row(song, &library, playing.as_deref(), want);
                ListItem::new(rows::line(&columns, &row))
            })
            .collect();

        frame.render_widget(block, area);
        if let Some(head) = head {
            frame.render_widget(Paragraph::new(rows::header(&columns)), head);
        }
        let mut state = std::mem::take(&mut self.nav.song_view_mut().state);
        let list = List::new(items).highlight_style(highlight()).highlight_symbol("▶ ");
        frame.render_stateful_widget(list, list_area, &mut state);
        self.nav.song_view_mut().state = state;
    }

    /// 组一行：三处列表（`draw_songs` / `draw_history` / `draw_queue`）共用同一份口径。
    ///
    /// `library` 是「按 mid 查当前曲库」的表（本地条目用得上，查不到就是已丢失）。
    /// 备注那一列按屏分岔（本地歌单写「已丢失」；历史写「时间 · 来源」，由调用方覆盖）。
    pub(super) fn song_row<'a>(
        &'a self,
        song: &'a Song,
        library: &HashMap<&'a str, &'a Song>,
        playing: Option<&str>,
        want: Quality,
    ) -> rows::Row<'a> {
        // 本地条目（本地曲库 / 本地歌单 / 历史里的本地条目）：曲库那份比快照新，优先用它。
        let fresh = library.get(song.mid.as_str()).copied();
        let source = fresh.unwrap_or(song);
        let copy = rows::copy_of(
            source,
            &self.cached,
            &self.downloads,
            &self.config.paths.download_dir,
            want,
        );
        let note = match self.nav.menu {
            Menu::LocalPlaylists
                if self.nav.local_shown.is_some() && !self.library_scanning && fresh.is_none() =>
            {
                Some("（已丢失）")
            }
            _ => None,
        };
        rows::Row {
            playing: playing == Some(song.mid.as_str()),
            title: &song.title,
            singers: &song.singers,
            // 本地条目取曲库那份的专辑（tag 是扫描时读的）；空串由 `rows` 写成 `—`。
            album: &source.album_name,
            duration: (source.duration_secs > 0).then_some(source.duration_secs),
            tier: Some(copy.tier),
            size: copy.size,
            cached: copy.cached,
            downloaded: copy.downloaded,
            note,
        }
    }

    /// 正在播的那首的 mid（没在播/播完就是 `None`）。
    ///
    /// 按 mid 比对而不是「标题 — 歌手」字符串：QQ 的 mid 全局唯一，同歌手的歌
    /// 不再互相误标。
    fn now_playing_mid(&self) -> Option<String> {
        let snapshot = self.engine.snapshot();
        if snapshot.phase == Phase::Idle {
            return None;
        }
        snapshot.mid
    }
}

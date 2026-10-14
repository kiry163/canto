//! 曲目列表的列模型：一处声明列，一处画表头，一处画行。
//!
//! 行级抽象：各屏只填 [`Row`]（填不出的字段留 `None`），列集与顺序由这里定。三套歌曲行
//! （收藏/歌单曲目/搜索、历史、队列浮层）都走这一份；`List` 的壳、空态、选中态仍各屏
//! 自己写（见 `docs/superpowers/specs/2026-10-13-track-table-design.md` 的决策记录）。
//!
//! 宽度：`Fixed` 列永不让位；`Fill` 列吃剩余、分不到 `min` 就按 [`DROP_ORDER`] 出局。

use std::borrow::Cow;
use std::collections::HashMap;

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::model::{Quality, Song};

use super::truncate_to_cols;

/// 一列读哪个字段。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Field {
    Marker,
    Title,
    Singers,
    Album,
    Duration,
    Tier,
    Size,
    Cached,
    Downloaded,
    /// 本地那份是哪儿来的：`下载` / `目录`（只有本地屏有这一列）。
    Source,
    Note,
}

/// 宽度：定宽，或「吃剩余」。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Width {
    Fixed(u16),
    /// 分不到 `min` 就按让位顺序出局（`备注` 的 `min` 就是为这个设的）。
    Fill { min: u16 },
}

/// 格子在列里怎么摆。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Align {
    Left,
    Right,
    Center,
}

/// 一列：读哪个字段、表头写什么、多宽、怎么对齐。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Column {
    pub field: Field,
    pub title: &'static str,
    pub width: Width,
    pub align: Align,
}

/// 定宽列的宽度（spec §1）。
pub(super) const MARKER: u16 = 1;
pub(super) const DURATION: u16 = 5;
pub(super) const TIER: u16 = 8;
pub(super) const SIZE: u16 = 7;
pub(super) const MARK: u16 = 4;
/// `标题`/`歌手` 的宽度下限。
pub(super) const FILL_MIN: u16 = 8;
/// `备注` 的宽度下限：分不到这些列时一句「3 小时前 · 来自《…》」只会被截成半句，
/// 不如按让位顺序出局。
pub(super) const NOTE_MIN: u16 = 12;
/// 列之间的空格（每格都补齐到列宽，所以 1 列够）。
pub(super) const GAP: u16 = 1;
/// `List` 的 `highlight_symbol("▶ ")` 会给每一行留两列：表头也要缩进这么多才对齐。
pub(super) const INDENT: u16 = 2;
/// 可用高 ≥ 这个值才画表头（矮屏里一行数据比一行表头值钱）。
pub(super) const MIN_FOR_HEADER: u16 = 6;

/// 让位顺序：装不下时从前往后撤。`标题`/`标记`/`缓存`/`下载` 永不撤。
///
/// `专辑` 排第二：60 列（最小窗口）下它跟 `备注` 一起出局，于是加这一列**没有**动
/// 最小窗口的观感（历史屏撤完这两列正好 60，其余 8 列一个不少）。`来源` 紧跟其后：
/// 它是本地屏独有的 4 列，撤了也只是少知道「这份是下载来的还是目录里扫到的」。
const DROP_ORDER: [Field; 7] = [
    Field::Note,
    Field::Album,
    Field::Source,
    Field::Size,
    Field::Tier,
    Field::Duration,
    Field::Singers,
];

/// 这批列（宽度已定）要占多少列，含列间空格。
pub(super) fn total(columns: &[Column]) -> u16 {
    let cells: u16 = columns
        .iter()
        .map(|column| match column.width {
            Width::Fixed(width) => width,
            Width::Fill { min } => min,
        })
        .sum();
    cells + GAP * columns.len().saturating_sub(1) as u16
}

/// 按可用宽度定列：① 先按 `min` 记着 `Fill` 列；② 装不下就按 [`DROP_ORDER`] 撤列；
/// ③ 余量等分给 `Fill` 列（除不尽的余数从前面的列开始 +1）。
pub(super) fn fit(columns: &[Column], width: u16) -> Vec<Column> {
    let mut kept: Vec<Column> = columns.to_vec();
    while total(&kept) > width {
        let Some(victim) = DROP_ORDER
            .iter()
            .find_map(|field| kept.iter().position(|column| column.field == *field))
        else {
            break;
        };
        kept.remove(victim);
    }
    let fills: Vec<usize> = kept
        .iter()
        .enumerate()
        .filter(|(_, column)| matches!(column.width, Width::Fill { .. }))
        .map(|(index, _)| index)
        .collect();
    if !fills.is_empty() {
        let slack = width.saturating_sub(total(&kept));
        let each = slack / fills.len() as u16;
        let extra = slack % fills.len() as u16;
        for (order, index) in fills.iter().enumerate() {
            if let Width::Fill { min } = kept[*index].width {
                kept[*index].width = Width::Fixed(min + each + u16::from((order as u16) < extra));
            }
        }
    }
    kept
}

/// 定宽的列。
fn column(field: Field, title: &'static str, width: u16, align: Align) -> Column {
    Column { field, title, width: Width::Fixed(width), align }
}

/// 吃剩余的列。
fn fill(field: Field, title: &'static str, min: u16, align: Align) -> Column {
    Column { field, title, width: Width::Fill { min }, align }
}

/// 收藏 / 歌单曲目 / 搜索。
pub(super) fn track_columns() -> Vec<Column> {
    vec![
        column(Field::Marker, "", MARKER, Align::Left),
        fill(Field::Title, "标题", FILL_MIN, Align::Left),
        fill(Field::Singers, "歌手", FILL_MIN, Align::Left),
        fill(Field::Album, "专辑", FILL_MIN, Align::Left),
        column(Field::Duration, "时长", DURATION, Align::Right),
        column(Field::Tier, "音质", TIER, Align::Left),
        column(Field::Size, "体积", SIZE, Align::Right),
        column(Field::Cached, "缓存", MARK, Align::Center),
        column(Field::Downloaded, "下载", MARK, Align::Center),
    ]
}

/// 本地曲库：本地歌不经过缓存，也不叫「下载」——那两列整列恒空，不如不画；
/// 换成 `来源`（这份是下载来的还是在目录里扫到的）——第 5 屏是两拨合起来的，得看得出区别。
pub(super) fn local_columns() -> Vec<Column> {
    vec![
        column(Field::Marker, "", MARKER, Align::Left),
        fill(Field::Title, "标题", FILL_MIN, Align::Left),
        fill(Field::Singers, "歌手", FILL_MIN, Align::Left),
        fill(Field::Album, "专辑", FILL_MIN, Align::Left),
        column(Field::Duration, "时长", DURATION, Align::Right),
        column(Field::Tier, "音质", TIER, Align::Left),
        column(Field::Size, "体积", SIZE, Align::Right),
        column(Field::Source, "来源", MARK, Align::Center),
    ]
}

/// 本地歌单 › 曲目：多一个「已丢失」的备注位。
pub(super) fn local_playlist_columns() -> Vec<Column> {
    let mut columns = local_columns();
    columns.push(fill(Field::Note, "备注", NOTE_MIN, Align::Left));
    columns
}

/// 历史：它存的也是 `Song`（有 mid），所以缓存/下载两列有意义。
pub(super) fn history_columns() -> Vec<Column> {
    let mut columns = track_columns();
    columns.push(fill(Field::Note, "备注", NOTE_MIN, Align::Left));
    columns
}

/// 队列浮层：最宽 72 列，且没有「来源」那一栏。
pub(super) fn queue_columns() -> Vec<Column> {
    vec![
        column(Field::Marker, "", MARKER, Align::Left),
        fill(Field::Title, "标题", FILL_MIN, Align::Left),
        fill(Field::Singers, "歌手", FILL_MIN, Align::Left),
        column(Field::Duration, "时长", DURATION, Align::Right),
        column(Field::Cached, "缓存", MARK, Align::Center),
        column(Field::Downloaded, "下载", MARK, Align::Center),
    ]
}

/// 一行的全部可用数据：屏各自填能填的，填不出的留 `None`（那一格写占位符）。
pub(super) struct Row<'a> {
    pub playing: bool,
    pub title: &'a str,
    pub singers: &'a str,
    /// 专辑名；空串 = 不知道（老历史条目、没打 album tag 的文件）→ 那一格写 `—`。
    pub album: &'a str,
    /// 秒；`None` = 不知道 → `--:--`。
    pub duration: Option<u64>,
    /// 音质或容器；`None` → `—`。
    pub tier: Option<Cow<'a, str>>,
    /// 字节；`None` → `—`。
    pub size: Option<u64>,
    pub cached: bool,
    pub downloaded: bool,
    /// 本地那份从哪儿来（`下载` / `目录`）；`None` = 不是本地歌 → 那一格写空串。
    pub source: Option<&'a str>,
    pub note: Option<&'a str>,
}

/// 表头那一行（`DarkGray`；对齐规则跟数据一致，所以「时长」这些标题也在它对的那一列上）。
///
/// **自带 [`INDENT`] 缩进**：`List` 渲染数据行时会先留出选中符号那两列，表头是普通
/// `Paragraph`，得自己补上同样的两列才对得齐。
pub(super) fn header(columns: &[Column]) -> Line<'static> {
    let mut spans = vec![Span::raw(" ".repeat(INDENT as usize))];
    for (order, column) in columns.iter().enumerate() {
        if order > 0 {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::raw(pad(column.title, cell_width(column), column.align)));
    }
    Line::from(spans).style(Style::new().fg(Color::DarkGray))
}

/// 一行数据。
///
/// 返回**拥有数据**的 `Line<'static>`：三处绘制函数是「建 items → `mem::take(&mut …state)`
/// → 渲染」的顺序，行若借着 `self.nav` 里的歌单就会与 `&mut` 撞上。
///
/// **不加缩进**：选中符号那两列由 `List` 自己留（再补一次就会比表头右移两列）。
pub(super) fn line<'a>(columns: &[Column], row: &Row<'a>) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (order, column) in columns.iter().enumerate() {
        if order > 0 {
            spans.push(Span::raw(" "));
        }
        spans.push(cell(column, row, cell_width(column)));
    }
    Line::from(spans)
}

/// 一格：取值 → 截断/补齐 → 上色。每格自己拥有那几个字符（就几列宽，拷贝很便宜）。
fn cell(column: &Column, row: &Row<'_>, width: usize) -> Span<'static> {
    match column.field {
        Field::Marker => Span::raw(if row.playing { "♪" } else { " " }.to_string()),
        Field::Title => Span::raw(clip(row.title, width)),
        Field::Singers => Span::raw(clip(row.singers, width)),
        Field::Album => Span::raw(clip(
            if row.album.is_empty() { "—" } else { row.album },
            width,
        )),
        Field::Duration => {
            let text = row.duration.map_or_else(
                || "--:--".to_string(),
                |secs| super::format_duration(std::time::Duration::from_secs(secs)),
            );
            Span::raw(pad_owned(text, width, Align::Right))
        }
        Field::Tier => Span::raw(
            row.tier
                .as_deref()
                .map_or_else(|| "—".to_string(), |tier| clip(tier, width)),
        ),
        Field::Size => {
            let text = row.size.map_or_else(|| "—".to_string(), super::format_size);
            Span::raw(pad_owned(text, width, Align::Right))
        }
        Field::Cached => Span::styled(
            pad(if row.cached { "●" } else { " " }, width, Align::Center),
            Style::new().fg(Color::Green),
        ),
        Field::Downloaded => Span::styled(
            pad(if row.downloaded { "↓" } else { " " }, width, Align::Center),
            Style::new().fg(Color::Cyan),
        ),
        // 来源也是「补齐到列宽」的一员：退回空串的话这一行会比表头短一截。
        Field::Source => Span::raw(pad(
            row.source.unwrap_or(""),
            width,
            Align::Center,
        )),
        // 空备注也补齐：别的每一列都会补到列宽，只有它退回空串的话，这一行会比表头短一截。
        Field::Note => Span::styled(
            clip(row.note.unwrap_or(""), width),
            Style::new().fg(Color::DarkGray),
        ),
    }
}

/// 一列在 `fit` 之后是多宽（`Fill` 已经变成 `Fixed`，这里兜住还没 fit 的调用）。
fn cell_width(column: &Column) -> usize {
    let width = match column.width {
        Width::Fixed(width) => width,
        Width::Fill { min } => min,
    };
    width as usize
}

/// 截断到 `width` 个显示列（超长补 `…`），再补齐到 `width`。
fn clip(text: &str, width: usize) -> String {
    pad_owned(truncate_to_cols(text, width as u16).into_owned(), width, Align::Left)
}

/// 补齐到 `width` 个显示列（中文双宽）。超宽先用 `…` 截断。
fn pad(text: &str, width: usize, align: Align) -> String {
    pad_owned(truncate_to_cols(text, width as u16).into_owned(), width, align)
}

/// 按显示宽度补齐（左边/右边补空格）。
fn pad_owned(text: String, width: usize, align: Align) -> String {
    let used = text.width();
    let slack = width.saturating_sub(used);
    let (left, right) = match align {
        Align::Left => (0, slack),
        Align::Right => (slack, 0),
        Align::Center => (slack / 2, slack - slack / 2),
    };
    format!("{}{}{}", " ".repeat(left), text, " ".repeat(right))
}

/// 在列表块**内部**切出表头那一行：可用高 `>= min_height` 才切。
///
/// 块的边框由调用方先画（拿 `Block::inner(area)` 之后再调这里），所以表头落在框里、不是框外。
///
/// - 全屏列表传 [`MIN_FOR_HEADER`]：矮屏里一行数据比一行表头值钱。
/// - 队列浮层传 `1`（等于「总是画」）：它的高度是**照着内容算的**，表头那一行不多占谁的位置。
pub(super) fn group(inner: Rect, min_height: u16) -> (Option<Rect>, Rect) {
    if inner.height >= min_height {
        let head = Rect { height: 1, ..inner };
        let list = Rect { y: inner.y + 1, height: inner.height - 1, ..inner };
        (Some(head), list)
    } else {
        (None, inner)
    }
}

/// 行内容能用的宽度：`List` 会给每一行留 [`INDENT`] 列给选中符号（`▶ `），
/// 所以按列表区宽度分配列宽时必须把这 2 列扣掉——不然整行超宽，尾巴会被 ratatui 裁掉。
pub(super) fn row_width(list_area_width: u16) -> u16 {
    list_area_width.saturating_sub(INDENT)
}

/// 行上「本地已有那份」的解析结果（spec §3：**已有那份就报那一份**）。
pub(super) struct LocalCopy<'a> {
    pub tier: Cow<'a, str>,
    pub size: Option<u64>,
    pub cached: bool,
    pub downloaded: bool,
    /// 本地那份是哪儿来的：`下载`（路径在下载索引里）/ `目录`（只被曲库扫到）。
    /// 非本地歌是 `None`——那一列只有本地屏有。
    pub origin: Option<&'static str>,
}

/// 口径：缓存 > 下载索引 > 曲库文件 > 按配置下限预估。
///
/// - 缓存那份的档位能从文件名精确读出（`<mid>.<档位>.<ext>`），体积取真实字节；
/// - 下载索引里那份的档位是**下载那一刻记下来的**（`Track.quality`），所以在线列表与第 5 屏
///   写的是同一个答案；
/// - 只是被扫到的本地文件读不出码率，写**容器**（扩展名）与 `local_bytes`——写容器是诚实的；
/// - 都没有才按配置下限算「新取流会取哪一档」（今天的行为）。
///
/// 本地有缓存与下载两份时两个标记都亮，但档位/体积报缓存那份——播放也是先命中缓存。
pub(super) fn copy_of<'a>(
    song: &'a Song,
    cached: &'a HashMap<String, (Quality, u64)>,
    downloads: &'a crate::download::Index,
    download_dir: &std::path::Path,
    want: Quality,
) -> LocalCopy<'a> {
    if song.is_local() {
        let path = song.path();
        // 本地那条是不是「下载来的」也按索引判（不是按目录位置猜）：历史屏里重播下载歌
        // 同样该亮 `↓`，「来源」列与档位也写同一份答案。
        let registered = path
            .and_then(|path| downloads.mid_of_path(download_dir, path))
            .and_then(|mid| downloads.by_mid(mid));
        let tier: Cow<'a, str> = match registered {
            Some(track) => track.quality.label().into(),
            None => path
                .and_then(|path| path.extension())
                .map_or_else(
                    || "—".to_string(),
                    |ext| crate::download::container_label(&ext.to_string_lossy()),
                )
                .into(),
        };
        return LocalCopy {
            tier,
            size: (song.local_bytes > 0).then_some(song.local_bytes),
            cached: false,
            downloaded: registered.is_some(),
            origin: Some(if registered.is_some() { "下载" } else { "目录" }),
        };
    }
    if let Some((quality, bytes)) = cached.get(&song.mid) {
        return LocalCopy {
            tier: quality.label().into(),
            size: Some(*bytes),
            cached: true,
            // 两个标记都按 mid 查：同一份文件在哪一屏都是同一个答案。
            downloaded: downloads.by_mid(&song.mid).is_some(),
            origin: None,
        };
    }
    if let Some(found) = downloads.by_mid(&song.mid) {
        return LocalCopy {
            tier: found.quality.label().into(),
            size: Some(found.bytes),
            cached: false,
            downloaded: true,
            origin: None,
        };
    }
    let (quality, size) = song.sizes.at_or_below(want);
    LocalCopy {
        tier: quality.label().into(),
        size,
        cached: false,
        downloaded: false,
        origin: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(columns: &[Column]) -> Vec<Field> {
        columns.iter().map(|column| column.field).collect()
    }

    fn width_of(columns: &[Column], field: Field) -> u16 {
        match columns
            .iter()
            .find(|column| column.field == field)
            .expect("这一列应该在")
            .width
        {
            Width::Fixed(width) => width,
            Width::Fill { min } => min,
        }
    }

    /// 60 列（最小窗口）：撤掉「备注」与「专辑」，`标题`/`歌手` 各 12，正好铺满。
    ///
    /// 这正是 spec 里那句「最紧那一档仍然画」的现状：加专辑列**没有**动 60 列的观感。
    #[test]
    fn sixty_columns_drop_the_note_and_the_album() {
        let fitted = fit(&history_columns(), 60);
        assert_eq!(
            fields(&fitted),
            vec![
                Field::Marker,
                Field::Title,
                Field::Singers,
                Field::Duration,
                Field::Tier,
                Field::Size,
                Field::Cached,
                Field::Downloaded,
            ],
            "60 列下该撤「备注」与「专辑」"
        );
        assert_eq!(width_of(&fitted, Field::Title), 12);
        assert_eq!(width_of(&fitted, Field::Singers), 12);
        assert_eq!(total(&fitted), 60, "正好铺满，不多不少");
    }

    /// 宽屏：9 列全在，定宽列一个都不许动。
    /// spec §6：60 / 80 / 120 / 200 列各钉一次「哪些列在、`标题`/`歌手`/`专辑` 拿到多少列、
    /// 正好铺满」。
    ///
    /// 60 列那两行是加专辑列之前**一模一样**的数字：专辑列自己先出局，所以最小窗口不受影响。
    #[test]
    fn the_fit_shapes_at_the_four_window_widths() {
        // (宽度, 哪张屏, 列数, 标题, 歌手, 专辑)
        let cases: [(u16, &str, usize, u16, u16, u16); 12] = [
            (60, "曲目", 8, 12, 12, 0), // 装不下「专辑」，正好铺满
            (80, "曲目", 9, 15, 14, 14),
            (120, "曲目", 9, 28, 28, 27), // 除不尽时多出来那一列给前面的
            (200, "曲目", 9, 55, 54, 54),
            (60, "历史", 8, 12, 12, 0), // 历史屏 60 列撤「备注」+「专辑」
            (80, "历史", 10, 10, 10, 9),
            (120, "历史", 10, 20, 20, 19),
            (200, "历史", 10, 40, 40, 39),
            (60, "本地", 8, 10, 9, 9), // 多一个「来源」（4 列）：60 列装得下八列，代价是三列各窄一列
            (80, "本地", 8, 16, 16, 16),
            (120, "本地", 8, 30, 29, 29),
            (200, "本地", 8, 56, 56, 56),
        ];
        for (width, screen, count, title, singers, album) in cases {
            let columns = match screen {
                "曲目" => track_columns(),
                "历史" => history_columns(),
                _ => local_columns(),
            };
            let fitted = fit(&columns, width);
            let where_ = format!("{width} 列（{screen}）");
            assert_eq!(fitted.len(), count, "{where_}的列数：{fitted:?}");
            assert_eq!(width_of(&fitted, Field::Title), title, "{where_}的标题宽");
            assert_eq!(width_of(&fitted, Field::Singers), singers, "{where_}的歌手宽");
            if album == 0 {
                assert!(
                    !fitted.iter().any(|column| column.field == Field::Album),
                    "{where_}不该有专辑列"
                );
            } else {
                assert_eq!(width_of(&fitted, Field::Album), album, "{where_}的专辑宽");
            }
            assert_eq!(total(&fitted), width, "{where_}要正好铺满");
        }
    }

    #[test]
    fn a_wide_window_keeps_every_column() {
        let fitted = fit(&history_columns(), 160);
        assert_eq!(fitted.len(), 10, "160 列下 10 列全在：{fitted:?}");
        assert_eq!(total(&fitted), 160);
        assert_eq!(width_of(&fitted, Field::Duration), DURATION);
        assert_eq!(width_of(&fitted, Field::Tier), TIER);
        assert_eq!(width_of(&fitted, Field::Size), SIZE);
        assert_eq!(width_of(&fitted, Field::Cached), MARK);
        assert_eq!(width_of(&fitted, Field::Downloaded), MARK);
        assert_eq!(width_of(&fitted, Field::Marker), MARKER);
    }

    /// 窄到极限：先撤备注、再撤专辑、然后才是体积，`标题`/`歌手` 压到下限就停
    /// （标记列与标题永不撤）。
    #[test]
    fn the_drop_order_goes_note_then_album_then_size() {
        // 45 列：撤「备注」（61）还不够，撤「专辑」（52）还不够，再撤「体积」→ 44 装得下，
        // 余 1 列给第一个 Fill。
        let fitted = fit(&history_columns(), 45);
        assert_eq!(
            fields(&fitted),
            vec![
                Field::Marker,
                Field::Title,
                Field::Singers,
                Field::Duration,
                Field::Tier,
                Field::Cached,
                Field::Downloaded,
            ],
            "45 列下该撤掉「备注」与「体积」"
        );
        assert_eq!(width_of(&fitted, Field::Title), FILL_MIN + 1);
        assert_eq!(width_of(&fitted, Field::Singers), FILL_MIN);
        assert_eq!(total(&fitted), 45);
    }

    /// 每张屏的列集就是 spec §2 那张表。
    #[test]
    fn each_screen_declares_the_columns_from_the_spec() {
        assert_eq!(
            fields(&track_columns()),
            vec![
                Field::Marker,
                Field::Title,
                Field::Singers,
                Field::Album,
                Field::Duration,
                Field::Tier,
                Field::Size,
                Field::Cached,
                Field::Downloaded
            ]
        );
        assert_eq!(
            fields(&local_columns()),
            vec![
                Field::Marker,
                Field::Title,
                Field::Singers,
                Field::Album,
                Field::Duration,
                Field::Tier,
                Field::Size,
                Field::Source
            ]
        );
        assert_eq!(
            fields(&local_playlist_columns()),
            vec![
                Field::Marker,
                Field::Title,
                Field::Singers,
                Field::Album,
                Field::Duration,
                Field::Tier,
                Field::Size,
                Field::Source,
                Field::Note
            ]
        );
        assert_eq!(
            fields(&history_columns()),
            vec![
                Field::Marker,
                Field::Title,
                Field::Singers,
                Field::Album,
                Field::Duration,
                Field::Tier,
                Field::Size,
                Field::Cached,
                Field::Downloaded,
                Field::Note
            ]
        );
        assert_eq!(
            fields(&queue_columns()),
            vec![
                Field::Marker,
                Field::Title,
                Field::Singers,
                Field::Duration,
                Field::Cached,
                Field::Downloaded
            ]
        );
    }

    /// 把一行拼成字符串（这些断言只看内容与列位，不看样式）。
    fn render(line: &Line<'_>) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    /// 拼好的一行里，`needle` 从第几**显示列**开始（宽字符按两列算）。
    ///
    /// 不能拿 `str::find` 的字节偏移直接比两行：中文一个字三字节，两行前缀宽度不同时
    /// 字节偏移会骗人，而显示列才是眼睛看到的那个。
    fn at_column(text: &str, needle: &str) -> usize {
        let at = text.find(needle).expect("这一格应该在");
        UnicodeWidthStr::width(&text[..at])
    }

    /// 表头与数据落在同一批列区间里：两行显示宽度一样，左对齐比起点、右对齐比终点、
    /// 居中允许一个字符的居中余数差。
    ///
    /// `来源` 列的让位：60 列（最小窗口）装得下（八列 56 格），再窄才跟着 `专辑` 撤。
    #[test]
    fn the_local_columns_have_a_source_column_that_drops_on_narrow_screens() {
        let wide: Vec<Field> = fit(&local_columns(), 60).iter().map(|c| c.field).collect();
        assert!(wide.contains(&Field::Source), "{wide:?}");

        // 46 列：`专辑` 先出局，还装不下就轮到 `来源`。
        let narrow: Vec<Field> = fit(&local_columns(), 46).iter().map(|c| c.field).collect();
        assert!(!narrow.contains(&Field::Source), "窄屏先撤：{narrow:?}");
        assert!(narrow.contains(&Field::Title) && narrow.contains(&Field::Duration));
    }

    /// 本地那份的「来源」：下载索引里登记过 → `下载`，只在曲库目录里 → `目录`；
    /// QQ 歌没这一格（那是另一个屏的列）。
    #[test]
    fn a_row_says_where_the_bytes_came_from() {
        let dir = std::env::temp_dir().join(format!("canto-rows-source-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("建目录");
        let downloaded = dir.join("歌手 - 下载的.mp3");
        std::fs::write(&downloaded, vec![0_u8; 4]).expect("写文件");
        let downloads = download_index(&[("m1", "歌手 - 下载的.mp3", Quality::Low128, 4)]);
        let nothing = cached_index(&[]);

        let mine = crate::tui::test_support::local_song(&downloaded.to_string_lossy());
        let other =
            crate::tui::test_support::local_song(&dir.join("自己的.flac").to_string_lossy());
        let copy = copy_of(&mine, &nothing, &downloads, &dir, Quality::Low128);
        assert_eq!(copy.origin, Some("下载"));
        assert_eq!(
            copy.tier, "MP3 128k",
            "登记过的那份写**下载时记下的档位**（跟在线列表同一个答案），不是容器名"
        );
        let copy = copy_of(&other, &nothing, &downloads, &dir, Quality::Low128);
        assert_eq!(copy.origin, Some("目录"));
        assert_eq!(copy.tier, "FLAC", "没登记的那份只读得出容器（扩展名）");
        assert_eq!(
            copy_of(
                &crate::tui::test_support::test_song("m9"),
                &nothing,
                &downloads,
                &dir,
                Quality::Low128
            )
            .origin,
            None,
            "QQ 歌没这一格"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 数据行按**列表渲染的样子**补上 [`INDENT`]（`List` 给每行留的选中符号那两列），
    /// 所以这里比的是绝对显示列。
    #[test]
    fn the_header_lines_up_with_the_row() {
        // 120 列：专辑列在这一档是有的（60 列下它自己就出局了）。
        let columns = fit(&history_columns(), 120);
        let head = render(&header(&columns));
        let row = format!(
            "{}{}",
            " ".repeat(INDENT as usize),
            render(&line(
                &columns,
                &Row {
                    playing: true,
                    title: "成全",
                    singers: "刘若英",
                    album: "年华",
                    duration: Some(276),
                    tier: Some("MP3 128k".into()),
                    size: Some(4_400_000),
                    cached: true,
                    downloaded: true,
                    source: None,
                    note: None,
                },
            ))
        );
        // 两行一样宽，且正好是「缩进 + 这批列要占的列数」。
        let expected = INDENT as usize + total(&columns) as usize;
        assert_eq!(UnicodeWidthStr::width(head.as_str()), expected, "{head:?}");
        assert_eq!(UnicodeWidthStr::width(row.as_str()), expected, "{row:?}");

        let width_of_text = |text: &str| UnicodeWidthStr::width(text);
        for (field, needle, value) in [
            (Field::Title, "标题", "成全"),
            (Field::Singers, "歌手", "刘若英"),
            (Field::Album, "专辑", "年华"),
            (Field::Duration, "时长", "04:36"),
            (Field::Tier, "音质", "MP3 128k"),
            (Field::Size, "体积", "4.2MB"),
            (Field::Cached, "缓存", "●"),
            (Field::Downloaded, "下载", "↓"),
        ] {
            let column = *columns
                .iter()
                .find(|column| column.field == field)
                .expect("这一列应该在");
            let (head_at, value_at) = (at_column(&head, needle), at_column(&row, value));
            match column.align {
                Align::Left => assert_eq!(
                    head_at, value_at,
                    "左对齐的列起点要一样（{needle}）：\n{head:?}\n{row:?}"
                ),
                Align::Right => assert_eq!(
                    head_at + width_of_text(needle),
                    value_at + width_of_text(value),
                    "右对齐的列终点要一样（{needle}）：\n{head:?}\n{row:?}"
                ),
                // 居中：表头是两字标题、数据是一个符号，居中余数可以差 1 列。
                Align::Center => assert!(
                    head_at.abs_diff(value_at) <= 1,
                    "居中列不该偏出 1 列（{needle}）：\n{head:?}\n{row:?}"
                ),
            }
        }
    }

    /// 超长按**显示宽度**截断 + `…`：标题长短不许把后面的列挤动。
    #[test]
    fn a_long_title_is_cut_by_display_width() {
        let columns = fit(&track_columns(), 60);
        let build = |title: &str| {
            render(&line(
                &columns,
                &Row {
                    playing: false,
                    title,
                    singers: "周杰伦",
                    album: "叶惠美",
                    duration: Some(61),
                    tier: Some("FLAC".into()),
                    size: None,
                    cached: false,
                    downloaded: false,
                    source: None,
                    note: None,
                },
            ))
        };
        let long = build("一二三四五六七八九十十一十二十三");
        let short = build("短");
        assert!(long.contains('…'), "超长要补省略号：{long:?}");
        assert_eq!(
            at_column(&long, "01:01"),
            at_column(&short, "01:01"),
            "标题长短不许把后面的列挤动：\n{long:?}\n{short:?}"
        );
        assert_eq!(
            UnicodeWidthStr::width(long.as_str()),
            UnicodeWidthStr::width(short.as_str()),
            "两行的显示宽度要一样"
        );
        assert!(short.contains('—'), "体积未知写 `—`");
    }

    /// 不知道的字段写占位符，不写假的 0。
    #[test]
    fn unknown_values_use_placeholders() {
        let columns = fit(&local_columns(), 80);
        let text = render(&line(
            &columns,
            &Row {
                playing: false,
                title: "没时长的",
                singers: "",
                album: "",
                duration: None,
                tier: None,
                size: None,
                cached: false,
                downloaded: false,
                source: None,
                note: None,
            },
        ));
        assert!(text.contains("--:--"), "时长未知：{text:?}");
        assert!(text.contains('—'), "音质/体积/专辑未知：{text:?}");
        assert!(!text.contains("00:00"), "不许写假的 00:00：{text:?}");
    }

    /// 矮屏不切表头，够高才切一行。
    #[test]
    fn the_header_is_only_cut_when_there_is_room() {
        let tall = Rect::new(0, 0, 60, 10);
        let (head, list) = group(tall, MIN_FOR_HEADER);
        assert_eq!(head, Some(Rect::new(0, 0, 60, 1)));
        assert_eq!(list, Rect::new(0, 1, 60, 9));

        let short = Rect::new(0, 0, 60, MIN_FOR_HEADER - 1);
        assert_eq!(group(short, MIN_FOR_HEADER), (None, short), "矮屏把那一行还给数据");
    }

    /// 队列浮层那种「高度照着内容算」的写法：门槛给 1，等于总是切表头。
    #[test]
    fn a_floor_of_one_always_cuts_the_header() {
        let tight = Rect::new(0, 0, 40, 3);
        let (head, list) = group(tight, 1);
        assert_eq!(head, Some(Rect::new(0, 0, 40, 1)));
        assert_eq!(list, Rect::new(0, 1, 40, 2));
    }

    fn cached_index(entries: &[(&str, Quality, u64)]) -> HashMap<String, (Quality, u64)> {
        entries
            .iter()
            .map(|(mid, quality, bytes)| ((*mid).to_string(), (*quality, *bytes)))
            .collect()
    }

    /// 一份下载索引（测试用）：`(mid, 文件名, 档位, 字节数)`。
    fn download_index(entries: &[(&str, &str, Quality, u64)]) -> crate::download::Index {
        let mut index = crate::download::Index::default();
        for (mid, file, quality, bytes) in entries {
            index.upsert(crate::download::Track {
                mid: (*mid).to_string(),
                file: (*file).to_string(),
                quality: *quality,
                container: crate::download::container_label(file.rsplit('.').next().unwrap_or("")),
                bytes: *bytes,
                title: String::new(),
                singers: String::new(),
                album: String::new(),
                duration_secs: 0,
                at: 0,
            });
        }
        index
    }

    /// 下载目录（测试里只用来把 `file` 拼成路径，没在磁盘上建）。
    const DOWNLOADS: &str = "D:/canto";

    /// 四种状态：缓存命中 > 下载索引命中 > 都没有（按配置下限预估）；本地歌报自己的容器与大小。
    #[test]
    fn the_tier_and_size_report_whatever_is_already_on_disk() {
        let mut song = crate::tui::test_support::titled_song("a", "成全", "刘若英");
        song.sizes.mp3_128 = 4_400_000;
        let cached = cached_index(&[("a", Quality::High320, 9_800_000)]);
        let downloads = download_index(&[("a", "刘若英 - 成全.mp3", Quality::High320, 9_900_000)]);
        // 空的索引要先绑成变量：`copy` 借着它们，临时值活不到断言那里。
        let nothing_cached = cached_index(&[]);
        let nothing_downloaded = download_index(&[]);
        let dir = std::path::Path::new(DOWNLOADS);

        // ① 有缓存：报缓存那份（播放也是先命中缓存），两个标记都亮。
        let copy = copy_of(&song, &cached, &downloads, dir, Quality::Low128);
        assert_eq!(copy.tier, "MP3 320k");
        assert_eq!(copy.size, Some(9_800_000));
        assert!(copy.cached && copy.downloaded);

        // ② 只下载过：档位与真实大小都来自索引（档位不再只到容器）。
        let copy = copy_of(&song, &nothing_cached, &downloads, dir, Quality::Low128);
        assert_eq!(copy.tier, "MP3 320k");
        assert_eq!(copy.size, Some(9_900_000));
        assert!(!copy.cached && copy.downloaded);

        // ③ 都没有：按配置下限预估（今天的行为）。
        let copy = copy_of(&song, &nothing_cached, &nothing_downloaded, dir, Quality::Low128);
        assert_eq!(copy.tier, "MP3 128k");
        assert_eq!(copy.size, Some(4_400_000));
        assert!(!copy.cached && !copy.downloaded);

        // ④ 本地歌：容器取自扩展名，大小来自曲库扫描；缓存/下载两列按「这份路径归谁」判。
        let mut local = crate::tui::test_support::local_song("C:/Music/canto/曲终人散.flac");
        local.local_bytes = 31_457_280;
        let copy = copy_of(&local, &cached, &downloads, dir, Quality::Low128);
        assert_eq!(copy.tier, "FLAC");
        assert_eq!(copy.size, Some(31_457_280));
        assert!(!copy.cached && !copy.downloaded, "没在下载索引里");

        // ⑤ 本地歌读不出大小：体积未知，仍是 `—`。
        let mut unknown = crate::tui::test_support::local_song("C:/Music/canto/没时长的.wav");
        unknown.local_bytes = 0;
        assert_eq!(copy_of(&unknown, &cached, &downloads, dir, Quality::Low128).size, None);
    }

    /// 本地歌那份**就是下载来的**（路径在下载索引里）：历史屏里重播也该亮 `↓`。
    #[test]
    fn a_local_file_that_is_in_the_download_index_still_lights_the_marker() {
        let dir = std::path::Path::new(DOWNLOADS);
        let mut local = crate::tui::test_support::local_song("D:/canto/刘若英 - 成全.flac");
        local.local_bytes = 4_400_000;
        let downloads = download_index(&[("a", "刘若英 - 成全.flac", Quality::Flac, 4_400_000)]);
        let nothing_cached = cached_index(&[]);
        let copy = copy_of(&local, &nothing_cached, &downloads, dir, Quality::Low128);
        assert!(copy.downloaded, "路径在下载索引里");
        assert!(!copy.cached);
    }
}

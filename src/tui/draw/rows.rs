//! 曲目列表的列模型：一处声明列，一处画表头，一处画行。
//!
//! 行级抽象：各屏只填 [`Row`]（填不出的字段留 `None`），列集与顺序由这里定。三套歌曲行
//! （收藏/歌单曲目/搜索、历史、队列浮层）都走这一份；`List` 的壳、空态、选中态仍各屏
//! 自己写（见 `docs/superpowers/specs/2026-10-13-track-table-design.md` 的决策记录）。
//!
//! 宽度：`Fixed` 列永不让位；`Fill` 列吃剩余、分不到 `min` 就按 [`DROP_ORDER`] 出局。

// 三处绘制接线在后面的任务里（Task 7/9/10 才用上）：在那之前只有单测在用这一堆类型，
// 先压住 dead_code，Task 10 落地时摘掉这一行。
#![allow(dead_code)]

/// 一列读哪个字段。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Field {
    Marker,
    Title,
    Singers,
    Duration,
    Tier,
    Size,
    Cached,
    Downloaded,
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
const DROP_ORDER: [Field; 5] =
    [Field::Note, Field::Size, Field::Tier, Field::Duration, Field::Singers];

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
        column(Field::Duration, "时长", DURATION, Align::Right),
        column(Field::Tier, "音质", TIER, Align::Left),
        column(Field::Size, "体积", SIZE, Align::Right),
        column(Field::Cached, "缓存", MARK, Align::Center),
        column(Field::Downloaded, "下载", MARK, Align::Center),
    ]
}

/// 本地曲库：本地歌不经过缓存，也不叫「下载」——那两列整列恒空，不如不画。
pub(super) fn local_columns() -> Vec<Column> {
    vec![
        column(Field::Marker, "", MARKER, Align::Left),
        fill(Field::Title, "标题", FILL_MIN, Align::Left),
        fill(Field::Singers, "歌手", FILL_MIN, Align::Left),
        column(Field::Duration, "时长", DURATION, Align::Right),
        column(Field::Tier, "音质", TIER, Align::Left),
        column(Field::Size, "体积", SIZE, Align::Right),
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

    /// 60 列（最小窗口）：撤掉「备注」，`标题`/`歌手` 各 12，正好铺满。
    #[test]
    fn sixty_columns_drop_the_note_and_split_the_rest_evenly() {
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
            "60 列下只该撤「备注」"
        );
        assert_eq!(width_of(&fitted, Field::Title), 12);
        assert_eq!(width_of(&fitted, Field::Singers), 12);
        assert_eq!(total(&fitted), 60, "正好铺满，不多不少");
    }

    /// 宽屏：9 列全在，定宽列一个都不许动。
    #[test]
    fn a_wide_window_keeps_every_column() {
        let fitted = fit(&history_columns(), 160);
        assert_eq!(fitted.len(), 9, "160 列下 9 列全在：{fitted:?}");
        assert_eq!(total(&fitted), 160);
        assert_eq!(width_of(&fitted, Field::Duration), DURATION);
        assert_eq!(width_of(&fitted, Field::Tier), TIER);
        assert_eq!(width_of(&fitted, Field::Size), SIZE);
        assert_eq!(width_of(&fitted, Field::Cached), MARK);
        assert_eq!(width_of(&fitted, Field::Downloaded), MARK);
        assert_eq!(width_of(&fitted, Field::Marker), MARKER);
    }

    /// 窄到极限：先撤备注、再撤体积，`标题`/`歌手` 压到下限就停（标记列与标题永不撤）。
    #[test]
    fn the_drop_order_goes_note_then_size_then_tier() {
        // 45 列：先撤「备注」还是不够（52），再撤「体积」→ 44 装得下，余 1 列给第一个 Fill。
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
                Field::Duration,
                Field::Tier,
                Field::Size
            ]
        );
        assert_eq!(
            fields(&local_playlist_columns()),
            vec![
                Field::Marker,
                Field::Title,
                Field::Singers,
                Field::Duration,
                Field::Tier,
                Field::Size,
                Field::Note
            ]
        );
        assert_eq!(
            fields(&history_columns()),
            vec![
                Field::Marker,
                Field::Title,
                Field::Singers,
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
}

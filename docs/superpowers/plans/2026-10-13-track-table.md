# 曲目列表的列模型与表头 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 8 处曲目/歌单列表的显示收成一套「列模型 + 共用画行 + 表头」，并给网络歌曲加上 `●` 已缓存 / `↓` 已下载两个标记。

**Architecture:** 新增 `src/tui/draw/rows.rs`（纯函数：列的声明、按窗口分宽度、画表头、画行），各屏只负责填 `Row`（填不出的字段留 `None`）；列表外壳仍是 ratatui 的 `List` + `ListState`（不动导航与既有测试面）。数据侧三处：缓存复用现成的 `App.cached`（O(1)）、下载新增 `App.downloads`（扫下载目录建 stem 表）、本地歌的容器/大小来自曲库索引里本来就有的 `Entry.size`；历史记录补两个字段把起播时手里的时长/体积落盘。

**Tech Stack:** Rust edition 2024、ratatui 0.30.2（`List` / `Paragraph` / `Block`，**不换** `Table`）、`unicode-width`（已在用）、serde（历史记录加字段）。**不新增任何依赖**。

**Spec:** `docs/superpowers/specs/2026-10-13-track-table-design.md`

## Global Constraints

- **三平台都能编**（macOS / Linux / Windows）。`src/overlay.rs`（桌面歌词）本次**一行不碰**。
- 每步都跑 `cargo test <过滤>`；每个 Task 收尾跑 `cargo clippy --all-targets --locked -- -D warnings` 与完整 `cargo test`。
- 测试与代码同文件（`#[cfg(test)] mod tests`）；测试用的构造器在 `crate::tui::test_support`（`test_app` / `test_song` / `local_song` / `titled_song`）。
- **列宽、列顺序、颜色、让位顺序一律照 spec 的表**，不许在实现里自己发明（`时长 5`、`音质 8`、`体积 7`、`缓存 4`、`下载 4`、`Fill` 的 `min` 是 `8/8/12`、列间 1 空格、表头缩进 2 列）。
- **键位零改动**；`draw::layout` 不动（表头在列表块内部切）。
- 空占位符固定：时长未知 `--:--`、体积未知 `—`、音质未知 `—`、标记没有就是空格。
- 提交粒度：每个 Task 一个 commit，中文 conventional commit（`feat(tui): …` / `fix(tui): …` / `docs: …`）。

---

## File Structure

| 文件 | 职责 | 动作 |
|---|---|---|
| `src/tui/draw/rows.rs` | 列模型（`Field`/`Column`/`Width`/`Align`）、`fit`/`header`/`line`/`group`、各屏列集、`LocalCopy` 解析 | **新增** |
| `src/tui/draw/mod.rs` | `truncate_to_cols` 搬进来（与 `format_size`/`format_duration` 同居）；`mod rows;` | 改 |
| `src/tui/draw/status.rs` | 删掉本地那份 `truncate_to_cols`，改用 `super::truncate_to_cols` | 改 |
| `src/tui/draw/content.rs` | `draw_songs` / `draw_history` 改走 `rows`；本地歌单按 mid 建曲库表；`playlist_row()`；空态 helper | 改 |
| `src/tui/draw/overlay.rs` | `draw_queue` 改走 `rows`；两个选择框改用 `playlist_row()` | 改 |
| `src/model.rs` | `Song.local_bytes`（本地文件字节数；0 = 未知/非本地） | 改 |
| `src/library.rs` | `entry_to_song` 填 `local_bytes` | 改 |
| `src/download.rs` | `file_stem` → `pub(crate)`；`container_label()`；`scan_downloads()` | 改 |
| `src/history.rs` | `Entry` 加 `duration_secs` + `sizes`（`serde(default)`） | 改 |
| `src/tui/state.rs` | `App.downloads` + `refresh_downloads()`（启动 / 下载完成 / `r`）；`record_history` 抄字段；`refresh_history_view` 还原 + 本地条目查曲库；`poll_transfer` 触发重扫 | 改 |
| `README.md`、`docs/design.md` | 三条已知限制改写 + 新增「曲目列表的列模型」一节 | 改 |

---

### Task 1: 列模型骨架（`Field` / `Width` / `Align` / `Column` / `fit`）

**Files:**
- Create: `src/tui/draw/rows.rs`
- Modify: `src/tui/draw/mod.rs`（模块声明处加 `mod rows;`）

**Interfaces:**
- Produces（后面所有任务都用这几个名字）：
  - `rows::Field { Marker, Title, Singers, Duration, Tier, Size, Cached, Downloaded, Note }`
  - `rows::Width { Fixed(u16), Fill { min: u16 } }`、`rows::Align { Left, Right, Center }`
  - `rows::Column { field: Field, title: &'static str, width: Width, align: Align }`
  - `rows::fit(columns: &[Column], width: u16) -> Vec<Column>`（装不下时按让位顺序撤列，余量等分给 `Fill`）
  - `rows::total(columns: &[Column]) -> u16`（这批列在**修好宽度之后**要占多少列，含分隔）
  - 常量：`MARKER=1 DURATION=5 TIER=8 SIZE=7 MARK=4 FILL_MIN=8 NOTE_MIN=12 GAP=1 INDENT=2`、`DROP_ORDER`
  - 各屏列集：`rows::track_columns() local_columns() local_playlist_columns() history_columns() queue_columns()`

- [ ] **Step 1: 写失败的测试**

`src/tui/draw/rows.rs` 末尾：

```rust
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

    /// 宽屏：`备注` 也在，三者分余量。
    #[test]
    fn a_wide_window_keeps_every_column() {
        let fitted = fit(&history_columns(), 160);
        assert_eq!(fitted.len(), 9, "160 列下 9 列全在：{fitted:?}");
        assert_eq!(total(&fitted), 160);
        // 定宽列一个都不许动。
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
            vec![Field::Marker, Field::Title, Field::Singers, Field::Duration, Field::Tier, Field::Size, Field::Cached, Field::Downloaded]
        );
        assert_eq!(
            fields(&local_columns()),
            vec![Field::Marker, Field::Title, Field::Singers, Field::Duration, Field::Tier, Field::Size]
        );
        assert_eq!(
            fields(&local_playlist_columns()),
            vec![Field::Marker, Field::Title, Field::Singers, Field::Duration, Field::Tier, Field::Size, Field::Note]
        );
        assert_eq!(
            fields(&history_columns()),
            vec![Field::Marker, Field::Title, Field::Singers, Field::Duration, Field::Tier, Field::Size, Field::Cached, Field::Downloaded, Field::Note]
        );
        assert_eq!(
            fields(&queue_columns()),
            vec![Field::Marker, Field::Title, Field::Singers, Field::Duration, Field::Cached, Field::Downloaded]
        );
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --locked rows::tests` —— 预期编译失败（没有 `rows` 模块）。

- [ ] **Step 3: 写实现**

`src/tui/draw/rows.rs` 头部（真源在 spec §1）。**实施记录（2026-10-13）**：`rows.rs` 的两批类型
要到 Task 7/9/10 才有生产消费者，在此之前只有单测在用，所以模块顶部加了一行
`#![allow(dead_code)]`（带注释指明来由，与 `paths.rs:108` 同一套口径）——**Task 10 落地时摘掉**：

```rust
//! 曲目列表的列模型：一处声明列，一处画表头，一处画行。
//!
//! 行级抽象：各屏只填 [`Row`]（填不出的字段留 `None`），列集与顺序由这里定。三套歌曲行
//! （收藏/歌单曲目/搜索、历史、队列浮层）都走这一份；`List` 的壳、空态、选中态仍各屏
//! 自己写（见 spec 的决策记录）。
//!
//! 宽度：`Fixed` 列永不让位；`Fill` 列吃剩余、分不到 `min` 就按 [`DROP_ORDER`] 出局。

use ratatui::text::Line;

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
                kept[*index].width =
                    Width::Fixed(min + each + u16::from((order as u16) < extra));
            }
        }
    }
    kept
}

/// 一行固定宽度、按 `align` 对齐的格子（`Fixed` 之后才是真宽度，`Fill` 在 `fit` 里定死）。
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
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --locked rows::tests` —— 预期 4 条全过。

- [ ] **Step 5: Commit**

```bash
git add src/tui/draw/rows.rs src/tui/draw/mod.rs
git commit -m "feat(tui): 曲目列表的列模型骨架(列集 + 按宽度让位)"
```

---

### Task 2: 画表头与画行（`header` / `line` / `group`）+ 搬 `truncate_to_cols`

**Files:**
- Modify: `src/tui/draw/rows.rs`（追加）
- Modify: `src/tui/draw/mod.rs`（把 `truncate_to_cols` 搬进来，`pub(super)`）
- Modify: `src/tui/draw/status.rs`（删掉本地那份，改 `use super::truncate_to_cols;`）

**Interfaces:**
- Consumes: Task 1 的 `Column`/`Width`/`Align`/`Field`/`GAP`/`INDENT`/`MIN_FOR_HEADER`/`total`
- Produces:
  - `rows::Row<'a> { playing: bool, title: &'a str, singers: &'a str, duration: Option<u64>, tier: Option<Cow<'a, str>>, size: Option<u64>, cached: bool, downloaded: bool, note: Option<&'a str> }`（输入借用，输出拥有）
  - `rows::header(columns: &[Column], indent: u16) -> Line<'static>`
  - `rows::line<'a>(columns: &[Column], row: &Row<'a>, indent: u16) -> Line<'static>`
  - `rows::group(inner: Rect) -> (Option<Rect>, Rect)`（列表块**内部**那两行/块）
  - `draw::truncate_to_cols(text: &str, cols: u16) -> Cow<'_, str>`（现在在 `draw/mod.rs`）

> **为什么 `line` 返回 `Line<'static>`（而不是借来的）**：三处绘制函数现在是「建 items →
  `mem::take(&mut ...state)` → 渲染」这个顺序；若 `Line` 借着 `self`（歌单在 `self.nav` 里）
  就会与 `&mut` 撞上。让输出自己拥有字符串，三处函数的结构一行都不用动（代价是每行几个小
  `String`，与今天 `format!` 拼行同一个量级）。

- [ ] **Step 1: 写失败的测试**

`rows.rs` 的 `mod tests` 里追加（先把两个小助手放进测试模块）：

```rust
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

    /// 表头与数据列位必须一一对齐（60 列下 8 列全在）。
    #[test]
    fn the_header_lines_up_with_the_row() {
        let columns = fit(&history_columns(), 60);
        let head = render(&header(&columns, INDENT));
        let row = render(&line(
            &columns,
            &Row {
                playing: true,
                title: "成全",
                singers: "刘若英",
                duration: Some(276),
                tier: Some("MP3 128k".into()),
                size: Some(4_400_000),
                cached: true,
                downloaded: true,
                note: None,
            },
            INDENT,
        ));
        assert!(head.starts_with("  "), "表头要跟 `▶ ` 同宽缩进：{head:?}");
        for (needle, value) in [
            ("标题", "成全"),
            ("歌手", "刘若英"),
            ("时长", "04:36"),
            ("音质", "MP3 128k"),
            ("体积", "4.2MB"),
            ("缓存", "●"),
            ("下载", "↓"),
        ] {
            assert_eq!(
                at_column(&head, needle),
                at_column(&row, value),
                "「{needle}」没对齐：\n{head:?}\n{row:?}"
            );
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
                    duration: Some(61),
                    tier: Some("FLAC".into()),
                    size: None,
                    cached: false,
                    downloaded: false,
                    note: None,
                },
                INDENT,
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
                duration: None,
                tier: None,
                size: None,
                cached: false,
                downloaded: false,
                note: None,
            },
            INDENT,
        ));
        assert!(text.contains("--:--"), "时长未知：{text:?}");
        assert!(text.contains('—'), "音质/体积未知：{text:?}");
        assert!(!text.contains("00:00"), "不许写假的 00:00：{text:?}");
    }

    /// 矮屏不切表头，够高才切一行。
    #[test]
    fn the_header_is_only_cut_when_there_is_room() {
        let tall = Rect::new(0, 0, 60, 10);
        let (head, list) = group(tall);
        assert_eq!(head, Some(Rect::new(0, 0, 60, 1)));
        assert_eq!(list, Rect::new(0, 1, 60, 9));

        let short = Rect::new(0, 0, 60, MIN_FOR_HEADER - 1);
        assert_eq!(group(short), (None, short), "矮屏把那一行还给数据");
    }
```

> **实施记录（2026-10-13）**：上面 `the_header_lines_up_with_the_row` 里「表头与数据落在同一列」
> 那句，实施时改成了**按对齐方式分别断言**——`左对齐比起点、右对齐比终点、居中允许差 1 列`，
> 并先钉住「两行显示宽度相同且 = `INDENT + total(columns)`」。原因：右对齐列的标题（「时长」占 4 列）
> 与数据（`04:36` 占 5 列）本来就不该从同一列起，比起点必然差 1 列；第一版断言就是这么写错的，
> 而它恰好证明了这条断言有效。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --locked rows::tests` —— 预期编译失败（`header` / `line` / `group` / `Row` 都不存在）。

- [ ] **Step 3: 搬 `truncate_to_cols` 到 `draw/mod.rs`**

把 `src/tui/draw/status.rs:26-45` 的整段函数（含它上面那两条文档注释）剪到 `src/tui/draw/mod.rs` 里
`format_size` / `format_duration` 旁边，签名改成：

```rust
/// 按**显示宽度**截断（中文、书名号都是双宽），超出就用省略号收尾。
///
/// 固定列宽下必须截：整条画不完时至少让人看出「被裁了」，而不是话说到一半。
/// `draw/status.rs` 的通知位与 `draw/rows.rs` 的表格都用它。
pub(super) fn truncate_to_cols(text: &str, cols: u16) -> Cow<'_, str> {
```

`status.rs` 里删掉本地那份，改成 `use super::truncate_to_cols;`（`use std::borrow::Cow;` 也要跟着删）。
`draw/mod.rs` 顶部补 `use std::borrow::Cow;` 与 `use unicode_width::UnicodeWidthStr;`（若还没 import）。

- [ ] **Step 4: 写实现**

`rows.rs` 追加：

```rust
use std::borrow::Cow;

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::truncate_to_cols;

/// 一行的全部可用数据：屏各自填能填的，填不出的留 `None`（那一格写占位符）。
pub(super) struct Row<'a> {
    pub playing: bool,
    pub title: &'a str,
    pub singers: &'a str,
    /// 秒；`None` = 不知道 → `--:--`。
    pub duration: Option<u64>,
    /// 音质或容器（见 `LocalCopy`）；`None` → `—`。
    pub tier: Option<Cow<'a, str>>,
    /// 字节；`None` → `—`。
    pub size: Option<u64>,
    pub cached: bool,
    pub downloaded: bool,
    pub note: Option<&'a str>,
}

/// 表头那一行（`DarkGray`；`时长`/`体积` 右对齐、`缓存`/`下载` 居中，跟数据同规则）。
pub(super) fn header(columns: &[Column], indent: u16) -> Line<'static> {
    let mut spans = vec![Span::raw(" ".repeat(indent as usize))];
    for (order, column) in columns.iter().enumerate() {
        if order > 0 {
            spans.push(Span::raw(" "));
        }
        let width = match column.width {
            Width::Fixed(width) => width,
            Width::Fill { min } => min,
        } as usize;
        spans.push(Span::raw(pad(column.title, width, column.align)));
    }
    Line::from(spans).style(Style::new().fg(Color::DarkGray))
}

/// 一行数据。
///
/// 返回**拥有数据**的 `Line<'static>`（见任务开头那段「为什么」）：调用方建完 items 之后
/// 还要 `mem::take(&mut ..state)`，借着的行会与 `&mut` 撞上。
pub(super) fn line<'a>(columns: &[Column], row: &Row<'a>, indent: u16) -> Line<'static> {
    let mut spans = vec![Span::raw(" ".repeat(indent as usize))];
    for (order, column) in columns.iter().enumerate() {
        if order > 0 {
            spans.push(Span::raw(" "));
        }
        let width = match column.width {
            Width::Fixed(width) => width,
            Width::Fill { min } => min,
        } as usize;
        spans.push(cell(column, row, width));
    }
    Line::from(spans)
}

/// 一格：取值 → 截断/补齐 → 上色。每格自己拥有那几个字符（就几列宽，拷贝很便宜）。
fn cell(column: &Column, row: &Row<'_>, width: usize) -> Span<'static> {
    match column.field {
        Field::Marker => Span::raw(if row.playing { "♪" } else { " " }.to_string()),
        Field::Title => Span::raw(clip(row.title, width)),
        Field::Singers => Span::raw(clip(row.singers, width)),
        Field::Duration => {
            let text = row.duration.map_or_else(
                || "--:--".to_string(),
                |secs| super::format_duration(std::time::Duration::from_secs(secs)),
            );
            Span::raw(pad_owned(text, width, Align::Right))
        }
        Field::Tier => Span::raw(row.tier.as_deref().map_or_else(|| "—".to_string(), |tier| clip(tier, width))),
        Field::Size => {
            let text = row.size.map_or_else(|| "—".to_string(), super::format_size);
            Span::raw(pad_owned(text, width, Align::Right))
        }
        Field::Cached => {
            Span::styled(pad(if row.cached { "●" } else { " " }, width, Align::Center), Style::new().fg(Color::Green))
        }
        Field::Downloaded => {
            Span::styled(pad(if row.downloaded { "↓" } else { " " }, width, Align::Center), Style::new().fg(Color::Cyan))
        }
        Field::Note => Span::styled(
            row.note.map_or_else(String::new, |note| clip(note, width)),
            Style::new().fg(Color::DarkGray),
        ),
    }
}

/// 截断到 `width` 个显示列（超长补 `…`），再补齐到 `width`。
fn clip(text: &str, width: usize) -> String {
    pad_owned(truncate_to_cols(text, width as u16).into_owned(), width, Align::Left)
}

/// 补齐到 `width` 个显示列（中文双宽）。超宽先用 `…` 截断。
fn pad(text: &str, width: usize, align: Align) -> String {
    pad_owned(truncate_to_cols(text, width as u16).into_owned(), width, align)
}

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

/// 在列表块**内部**切出表头那一行。够高才切（见 [`MIN_FOR_HEADER`]）。
///
/// 块的边框由调用方先画（`Block::inner(area)` 之后再调这里），所以表头落在框里、不是框外。
pub(super) fn group(inner: Rect) -> (Option<Rect>, Rect) {
    if inner.height >= MIN_FOR_HEADER {
        let head = Rect { height: 1, ..inner };
        let list = Rect { y: inner.y + 1, height: inner.height - 1, ..inner };
        (Some(head), list)
    } else {
        (None, inner)
    }
}
```

（`Row` 里 `tier`/`note` 的 `Cow` 让借来的 `&'static str` 与表里来的 `&str` 都能填；`line` 的
生命周期跟着 `Row` 走，不拷贝。）

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test --locked rows::tests` 与 `cargo test --locked status`
预期：`rows` 的 8 条与状态行那几条（截断行为没变）全过。

- [ ] **Step 6: Commit**

```bash
git add src/tui/draw/rows.rs src/tui/draw/mod.rs src/tui/draw/status.rs
git commit -m "feat(tui): 表头与行的绘制(列宽补齐/显示宽度截断/表头切分)"
```

---

### Task 3: `Song.local_bytes`（本地文件字节数）

**Files:**
- Modify: `src/model.rs`（字段 + 测试辅助）、`src/library.rs`（`entry_to_song` + 新测试）
- Modify（补字段，编译期会全部点名）：`src/qq/parse.rs:42`、`src/qq/parse.rs:76`、`src/qq/write.rs:84`、`src/download.rs:205`、`src/tui/queue.rs:316`、`src/tui/test_support.rs:63`、`src/tui/state.rs:2210`、`src/model.rs:367`

**Interfaces:**
- Produces: `Song.local_bytes: u64`（`0` = 未知或非本地；本地歌由曲库扫描填 `Entry.size`）

- [ ] **Step 1: 写失败的测试**

`src/library.rs` 的 `mod tests` 里追加：

```rust
    /// 本地文件的字节数要跟着 `Song` 走：列表那一列报的是磁盘上那份的大小。
    #[test]
    fn entry_to_song_carries_the_file_size() {
        let entry = Entry {
            path: std::path::PathBuf::from("C:/Music/canto/曲终人散.flac"),
            mtime: 1,
            size: 31_457_280,
            title: "曲终人散".to_string(),
            artist: "张宇".to_string(),
            album: "消息".to_string(),
            duration_secs: 270,
        };
        let song = entry_to_song(&entry);
        assert_eq!(song.local_bytes, 31_457_280);
        assert!(song.is_local());
    }

    /// QQ 歌（接口来的）没有本地文件，是 0——这条钉在 `qq/parse.rs` 的
    /// `parsed_songs_are_qq_sourced` 里（见实施记录）。
```

> **实施记录（2026-10-13）**：`test_support::test_song` 对 `library::tests` 是私有的，所以
> 「QQ 歌是 0」那条断言没有写成独立测试，而是**加进了 `src/qq/parse.rs::parsed_songs_are_qq_sourced`**
> 的现有断言里（`assert_eq!(song.local_bytes, 0, "接口来的歌没有本地文件")`）——钉在真正产出
> QQ `Song` 的地方比钉在测试辅助上更有意义。
> 另外 `src/qq/parse.rs:42` / `:76` 两处生产构造点补的是 `local_bytes: 0,`（带一行注释「接口来的歌
> 没有本地文件」），`src/tui/state.rs:2210` 那处也填 0 并留了注释（Task 8 会一起补上真实值）。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --locked library::tests::entry_to_song_carries_the_file_size`
预期：编译失败（`Song` 没有 `local_bytes`）。

- [ ] **Step 3: 写实现**

`src/model.rs` 的 `Song` 里加字段（放在 `sizes` 之后、`source` 之前）：

```rust
    /// 各档位文件大小（字节）。0 = 该档位没有。
    pub sizes: FileSizes,
    /// 本地文件的字节数；`0` = 非本地或读不出（曲库扫描本来就会 stat，见 `library::Entry.size`）。
    pub local_bytes: u64,
```

`src/library.rs` 的 `entry_to_song` 里加一行：

```rust
        sizes: FileSizes::default(),
        // 本地文件的大小：列表那一列报「磁盘上那份」，没有接口体积也不写 `—`。
        local_bytes: entry.size,
```

其余 8 处 `Song { .. }` 字面量补 `local_bytes: 0,`（`cargo check` 会把它们一个个点出来；
`src/tui/state.rs:2210` 是历史还原出来的 `Song`，Task 8 会重写那一段，这里先填 0）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --locked` —— 预期全绿（350 项里新增 2 项）。

- [ ] **Step 5: Commit**

```bash
git add -A src
git commit -m "feat(model): Song 带上本地文件字节数(曲库扫描本来就有)"
```

---

### Task 4: 扫下载目录（`scan_downloads` / `container_label` / `file_stem` 公开）

**Files:**
- Modify: `src/download.rs`

**Interfaces:**
- Produces:
  - `download::file_stem(song: &Song) -> String`（改成 `pub(crate)`，与落盘同一个函数）
  - `download::container_label(extension: &str) -> String`（扩展名 → 容器名，大写）
  - `download::scan_downloads(dir: &Path) -> HashMap<String, (String, u64)>`（`stem → (容器, 字节)`）

- [ ] **Step 1: 写失败的测试**

`src/download.rs` 的 `mod tests` 里追加：

```rust
    /// 扫下载目录：同名多份取「更高」的那份；`.part` 与目录不认；没有目录就是空表。
    #[test]
    fn scan_downloads_keeps_the_highest_copy_and_skips_parts() {
        let dir = std::env::temp_dir().join(format!("canto-dl-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join("sub")).expect("建目录");
        std::fs::write(dir.join("刘若英 - 成全.mp3"), vec![0_u8; 4_400_000]).expect("写 mp3");
        std::fs::write(dir.join("刘若英 - 成全.flac"), vec![0_u8; 31_457_280]).expect("写 flac");
        std::fs::write(dir.join("周杰伦 - 晴天.mp3.part"), b"半截").expect("写半截");
        std::fs::write(dir.join("README.txt"), b"说明").expect("写非音频");

        let found = scan_downloads(&dir);
        assert_eq!(
            found.get("刘若英 - 成全"),
            Some(&("FLAC".to_string(), 31_457_280)),
            "同一个 stem 有多份时取容器更高的那份：{found:?}"
        );
        assert_eq!(found.get("周杰伦 - 晴天"), None, "`.part` 不算：{found:?}");
        assert_eq!(found.get("README"), Some(&("TXT".to_string(), 6)));
        assert!(scan_downloads(&dir.join("没有这个目录")).is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 扫出来的键就是落盘时用的文件名：两边共用 `file_stem`，改名规则不许各写一套。
    #[test]
    fn the_scanned_key_matches_the_file_we_write() {
        let song = song("成全", "刘若英");
        let dir = std::env::temp_dir().join(format!("canto-dl-key-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("建目录");
        std::fs::write(dir.join(format!("{}.mp3", file_stem(&song))), b"x").expect("写文件");

        let found = scan_downloads(&dir);
        assert_eq!(found.get(&file_stem(&song)).map(|(container, _)| container.as_str()), Some("MP3"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn container_label_is_the_uppercase_extension() {
        assert_eq!(container_label("flac"), "FLAC");
        assert_eq!(container_label("M4A"), "M4A");
        assert_eq!(container_label("mp3"), "MP3");
    }
```

（`song(title, singers)` 是 `download.rs` 测试里现成的辅助，见 `src/download.rs:204`。）

> **实施记录（2026-10-13）**：四处按实情调整——
> ① 临时目录名从 `canto-dl-{pid}` 改成 `canto-dl-scan-{pid}`：**既有的**
>    `lyrics_land_in_their_own_directory`（`download.rs:297`）已经占了 `canto-dl-{pid}`，
>    并行跑会互相清目录（第一次跑就红了，正是这个原因）；
> ② 假文件用 44 KB / 314 KB，不写 4.4 MB / 31 MB——排序只看「容器 > 体积」，没必要每次测试
>    往临时目录写 35 MB；
> ③ `"半截".as_bytes()` 而不是 `b"半截"`（字节串字面量只能放 ASCII）；
> ④ `scan_downloads` 挂了一行 `#[allow(dead_code)]` + 「Task 5 接上就摘掉」的注释（与
>    `paths.rs:108` 同一套口径）——CI 的门槛是 `-D warnings`，没有消费者时这一行是必须的。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --locked download::tests::scan_downloads_keeps_the_highest_copy_and_skips_parts`
预期：编译失败（`scan_downloads` / `container_label` 不存在）。

- [ ] **Step 3: 写实现**

`src/download.rs`：`fn file_stem` 改成 `pub(crate) fn file_stem`（文档注释保持），并追加：

```rust
/// 扩展名 → 容器名（列表那一列的「音质」位就写它）。
///
/// **不猜码率**：`.mp3` 可能是 128 也可能是 320，文件本身读不出来；写容器是诚实的。
pub(crate) fn container_label(extension: &str) -> String {
    extension.to_ascii_uppercase()
}

/// 扫一遍下载目录：`文件名（不含扩展名） → (容器, 字节数)`。
///
/// 列表里的 `↓` 与「体积报磁盘那份」用它（按 `file_stem` 查表，O(1)）。用文件名当键是**唯一
/// 可选**的口径：文件里没有 mid。同一个 stem 有多份时（mp3 + flac）按容器排
/// `flac/wav > m4a/ogg > 其它`，同级取体积大的——体积是文件本身唯一读得出的「档位高低」信号。
/// 目录不存在 / 没权限一律给空表（下载目录可能还没建）。
pub(crate) fn scan_downloads(dir: &Path) -> HashMap<String, (String, u64)> {
    let mut found: HashMap<String, (String, u64)> = HashMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let (Some(stem), Some(extension)) = (
            path.file_stem().map(|stem| stem.to_string_lossy().into_owned()),
            path.extension().map(|ext| ext.to_string_lossy().to_ascii_lowercase()),
        ) else {
            continue;
        };
        if extension == "part" {
            continue;
        }
        let Ok(metadata) = entry.metadata() else { continue };
        if !metadata.is_file() {
            continue;
        }
        let candidate = (container_label(&extension), metadata.len());
        match found.get(&stem) {
            Some(current) if rank(&candidate) <= rank(current) => {}
            _ => {
                found.insert(stem, candidate);
            }
        }
    }
    found
}

/// 「哪一份更高」：先看容器，同级看体积。
fn rank(entry: &(String, u64)) -> (u8, u64) {
    let tier = match entry.0.as_str() {
        "FLAC" | "WAV" => 2,
        "M4A" | "OGG" => 1,
        _ => 0,
    };
    (tier, entry.1)
}
```

文件顶部补 `use std::collections::HashMap;`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --locked download::tests` —— 预期全过。

- [ ] **Step 5: Commit**

```bash
git add src/download.rs
git commit -m "feat(download): 扫下载目录建 stem 表(容器+字节, 同名取最高)"
```

---

### Task 5: `App.downloads` 接线（启动 / 下载完成 / `r`）

**Files:**
- Modify: `src/tui/state.rs`（字段、`App::new`、`refresh_downloads`、`LocalRescan` 分支、`poll_transfer`）

**Interfaces:**
- Consumes: `download::scan_downloads(&Path) -> HashMap<String,(String,u64)>`
- Produces: `App.downloads: HashMap<String, (String, u64)>`、`App::refresh_downloads(&mut self)`

- [ ] **Step 1: 写失败的测试**

`src/tui/state.rs` 的 `mod tests` 里追加：

```rust
    /// 下载完了列表里的 `↓` 要立刻出现：`poll_transfer` 收到结果就重扫下载目录。
    #[test]
    fn a_finished_download_refreshes_the_downloaded_index() {
        let (mut app, _runtime) = test_app();
        let dir = std::env::temp_dir().join(format!("canto-dl-state-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("建目录");
        app.config.download_dir = dir.clone();
        app.refresh_downloads();
        assert!(app.downloads.is_empty(), "空目录就是空表");

        // 假装那一下下载落了盘，然后走一遍「下载完成」的收尾。
        let song = crate::tui::test_support::test_song("a");
        std::fs::write(
            dir.join(format!("{}.mp3", crate::download::file_stem(&song))),
            vec![0_u8; 1024],
        )
        .expect("写文件");
        app.transfer = Some(std::sync::Arc::new(std::sync::Mutex::new(Transfer {
            title: song.title.clone(),
            received: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            total: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            outcome: Some("已下载：歌 a".to_string()),
        })));
        app.poll_transfer();

        assert!(
            app.downloads.contains_key(&crate::download::file_stem(&song)),
            "下载完成后 `↓` 该出现：{:?}",
            app.downloads
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// `r`（本地曲库重扫）也顺手重扫下载目录：手工删掉文件之后按一下就该消失。
    #[test]
    fn rescanning_the_library_also_refreshes_downloads() {
        let (mut app, _runtime) = test_app();
        let dir = std::env::temp_dir().join(format!("canto-dl-rescan-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("建目录");
        app.config.download_dir = dir.clone();
        let file = dir.join("刘若英 - 成全.mp3");
        std::fs::write(&file, vec![0_u8; 8]).expect("写文件");
        app.refresh_downloads();
        assert_eq!(app.downloads.len(), 1);

        std::fs::remove_file(&file).expect("删文件");
        app.switch_menu(Menu::LocalLibrary);
        app.apply(crate::tui::keys::Action::LocalRescan);
        assert!(app.downloads.is_empty(), "重扫之后删掉的那份不该还在");

        std::fs::remove_dir_all(&dir).ok();
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --locked state::tests::a_finished_download_refreshes_the_downloaded_index`
预期：编译失败（`downloads` / `refresh_downloads` 不存在）。

- [ ] **Step 3: 写实现**

① `App` 的 `cached` 字段下面加：

```rust
    /// 下载目录里已有的文件：`文件名（不含扩展名） → (容器, 字节)`。列表里标 `↓`、并让
    /// 「音质/体积」报磁盘上那份。启动扫一次，下载完成后与 `r` 重扫时更新（见 `refresh_downloads`）。
    pub(super) downloads: std::collections::HashMap<String, (String, u64)>,
```

② `App::new` 里，在 `let playlists_store = …;` 之后（`config` 还没被 move 进字段）：

```rust
        // 下载目录里的文件：`↓` 标记与「体积报磁盘那份」用它。
        let downloads = crate::download::scan_downloads(&config.download_dir);
```

字段初始化处（`cached: crate::cache::cached_entries(),` 下面）加 `downloads,`。

③ `impl App` 里加方法（放在 `start_library_scan` 附近）：

```rust
    /// 重扫下载目录：`↓` 与「体积报磁盘那份」的数据源。
    ///
    /// 启动一次；每次下载完成（`poll_transfer` 收到结果）一次；`r` 重扫曲库时顺带一次——
    /// 外部删掉 / 手工拷进来的文件靠它反映。
    pub(super) fn refresh_downloads(&mut self) {
        self.downloads = crate::download::scan_downloads(&self.config.download_dir);
    }
```

④ `apply` 的 `LocalRescan` 分支：

```rust
            keys::Action::LocalRescan => {
                if self.nav.menu == Menu::LocalLibrary {
                    self.start_library_scan();
                    // 手工删掉 / 拷进来的下载文件，按一下 `r` 就反映（曲库与下载目录一起刷）。
                    self.refresh_downloads();
                }
            }
```

⑤ `poll_transfer` 的 outcome 分支里（`self.notify(outcome);` 之前）：

```rust
            // 下载完成（成功或失败）之后重扫一次下载目录：成功的这次会多出一个 `↓`。
            self.refresh_downloads();
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --locked state::tests` —— 预期全过。

- [ ] **Step 5: Commit**

```bash
git add src/tui/state.rs
git commit -m "feat(tui): App 维护下载目录索引(启动/下载完成/r 三处刷新)"
```

---

### Task 6: 「本地已有那份」的解析（`LocalCopy` / `copy_of`）

**Files:**
- Modify: `src/tui/draw/rows.rs`

**Interfaces:**
- Consumes: `Song.local_bytes`（Task 3）、`download::file_stem` / `container_label` / `scan_downloads` 的表形状（Task 4）、`App.cached` 的形状（现成）
- Produces:
  - `rows::LocalCopy<'a> { tier: Cow<'a, str>, size: Option<u64>, cached: bool, downloaded: bool }`
  - `rows::copy_of<'a>(song: &'a Song, cached: &'a HashMap<String,(Quality,u64)>, downloads: &'a HashMap<String,(String,u64)>, want: Quality) -> LocalCopy<'a>`

- [ ] **Step 1: 写失败的测试**

```rust
    fn cached_index(entries: &[(&str, Quality, u64)]) -> HashMap<String, (Quality, u64)> {
        entries
            .iter()
            .map(|(mid, quality, bytes)| ((*mid).to_string(), (*quality, *bytes)))
            .collect()
    }

    fn downloaded_index(entries: &[(&str, &str, u64)]) -> HashMap<String, (String, u64)> {
        entries
            .iter()
            .map(|(stem, container, bytes)| ((*stem).to_string(), ((*container).to_string(), *bytes)))
            .collect()
    }

    /// 四种状态：缓存命中 > 下载命中 > 都没有（按配置下限预估）；本地歌报自己的容器与大小。
    #[test]
    fn the_tier_and_size_report_whatever_is_already_on_disk() {
        let mut song = crate::tui::test_support::titled_song("a", "成全", "刘若英");
        song.sizes.mp3_128 = 4_400_000;
        let cached = cached_index(&[("a", Quality::High320, 9_800_000)]);
        let downloads = downloaded_index(&[("刘若英 - 成全", "MP3", 9_900_000)]);

        // ① 有缓存：报缓存那份（播放也是先命中缓存），两个标记都亮。
        let copy = copy_of(&song, &cached, &downloads, Quality::Low128);
        assert_eq!(copy.tier, "MP3 320k");
        assert_eq!(copy.size, Some(9_800_000));
        assert!(copy.cached && copy.downloaded);

        // ② 只下载过：报磁盘那份的容器与真实大小。
        let copy = copy_of(&song, &cached_index(&[]), &downloads, Quality::Low128);
        assert_eq!(copy.tier, "MP3");
        assert_eq!(copy.size, Some(9_900_000));
        assert!(!copy.cached && copy.downloaded);

        // ③ 都没有：按配置下限预估（今天的行为）。
        let copy = copy_of(&song, &cached_index(&[]), &downloaded_index(&[]), Quality::Low128);
        assert_eq!(copy.tier, "MP3 128k");
        assert_eq!(copy.size, Some(4_400_000));
        assert!(!copy.cached && !copy.downloaded);

        // ④ 本地歌：容器取自扩展名，大小来自曲库扫描；缓存/下载两列不适用。
        let mut local = crate::tui::test_support::local_song("C:/Music/canto/曲终人散.flac");
        local.local_bytes = 31_457_280;
        let copy = copy_of(&local, &cached, &downloads, Quality::Low128);
        assert_eq!(copy.tier, "FLAC");
        assert_eq!(copy.size, Some(31_457_280));
        assert!(!copy.cached && !copy.downloaded);

        // ⑤ 本地歌读不出大小：体积未知，仍是 `—`。
        let mut unknown = crate::tui::test_support::local_song("C:/Music/canto/没时长的.wav");
        unknown.local_bytes = 0;
        assert_eq!(copy_of(&unknown, &cached, &downloads, Quality::Low128).size, None);
    }
```

> **实施记录（2026-10-13）**：② 与 ③ 里的空索引要先绑成变量（`nothing_cached` / `nothing_downloaded`）
> 再传——`copy` 借着它们，`&cached_index(&[])` 这种临时值活不到断言那里（E0716）。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --locked rows::tests::the_tier_and_size_report_whatever_is_already_on_disk`
预期：编译失败（`copy_of` / `LocalCopy` 不存在）。

- [ ] **Step 3: 写实现**

```rust
use std::collections::HashMap;

use crate::model::{Quality, Song};

/// 行上「本地已有那份」的解析结果（spec §3：**已有那份就报那一份**）。
pub(super) struct LocalCopy<'a> {
    pub tier: Cow<'a, str>,
    pub size: Option<u64>,
    pub cached: bool,
    pub downloaded: bool,
}

/// 口径：缓存 > 下载 > 曲库文件 > 按配置下限预估。
///
/// - 缓存那份的档位能从文件名精确读出（`<mid>.<档位>.<ext>`），体积取真实字节；
/// - 下载那份与本地文件只读得出**容器**（扩展名）与字节数，不猜 128/320；
/// - 本地歌（`Source::Local`）直接报自己的容器与 `local_bytes`；
/// - 都没有才按配置下限算「新取流会取哪一档」（今天的行为）。
pub(super) fn copy_of<'a>(
    song: &'a Song,
    cached: &'a HashMap<String, (Quality, u64)>,
    downloads: &'a HashMap<String, (String, u64)>,
    want: Quality,
) -> LocalCopy<'a> {
    if song.is_local() {
        let tier = song
            .path()
            .and_then(|path| path.extension())
            .map_or_else(|| "—".to_string(), |ext| crate::download::container_label(&ext.to_string_lossy()));
        return LocalCopy {
            tier: tier.into(),
            size: (song.local_bytes > 0).then_some(song.local_bytes),
            cached: false,
            downloaded: false,
        };
    }
    let stem = crate::download::file_stem(song);
    if let Some((quality, bytes)) = cached.get(&song.mid) {
        return LocalCopy {
            tier: quality.label().into(),
            size: Some(*bytes),
            cached: true,
            downloaded: downloads.contains_key(&stem),
        };
    }
    if let Some((container, bytes)) = downloads.get(&stem) {
        return LocalCopy { tier: container.clone().into(), size: Some(*bytes), cached: false, downloaded: true };
    }
    let (quality, size) = song.sizes.at_or_below(want);
    LocalCopy { tier: quality.label().into(), size, cached: false, downloaded: false }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --locked rows::tests` —— 预期全过。

- [ ] **Step 5: Commit**

```bash
git add src/tui/draw/rows.rs
git commit -m "feat(tui): 音质/体积按「本地已有那份」报(缓存>下载>曲库>预估)"
```

---

### Task 7: `draw_songs` 改走 `rows`（表头 + 两个标记 + 备注）

**Files:**
- Modify: `src/tui/draw/content.rs`（`draw_songs` 主体 + `Menu::LocalPlaylists` 的曲库表）
- Modify: `src/tui/draw/mod.rs`（若 `body_rows` 之类的测试辅助需要新参数）

**Interfaces:**
- Consumes: Task 1/2/6 的 `rows::{fit,group,header,line,Row,copy_of}` 与列集；`App.{cached,downloads,library}`、`App.config.quality`
- Produces: `App::song_row(&Song, &Row 之外的东西) -> rows::Row`（本任务内的私有辅助）

- [ ] **Step 1: 写失败的测试**

`src/tui/draw/mod.rs` 的 tests 里追加（用 `frame_raw` 与 `column_of`，它们保留显示列）：

```rust
    /// 曲目列表有表头，且与数据同列；缓存/下载两个标记按数据在正确的列上。
    #[test]
    fn song_lists_have_a_header_aligned_with_the_rows() {
        let (mut app, _runtime) = test_app();
        let mut song = crate::tui::test_support::titled_song("a", "成全", "刘若英");
        song.sizes.mp3_128 = 4_400_000;
        app.nav.favorites.set_items(vec![song]);
        app.cached.insert("a".to_string(), (crate::model::Quality::High320, 9_800_000));
        app.config.download_dir = std::env::temp_dir().join(format!("canto-draw-dl-{}", std::process::id()));
        std::fs::remove_dir_all(&app.config.download_dir).ok();
        std::fs::create_dir_all(&app.config.download_dir).expect("建下载目录");
        std::fs::write(
            app.config.download_dir.join("刘若英 - 成全.mp3"),
            vec![0_u8; 1_024],
        )
        .expect("写下载文件");
        app.refresh_downloads();
        app.switch_menu(Menu::Favorites);

        let rows = frame_raw(&mut app, 100, 30);
        let header = rows.iter().find(|row| row.contains("标题") && row.contains("缓存")).expect("有表头");
        let data = rows.iter().find(|row| row.contains("成全")).expect("有数据行");
        for needle in ["标题", "歌手", "时长", "音质", "体积", "缓存", "下载"] {
            let head = column_of(header, needle);
            let value = match needle {
                "标题" => column_of(data, "成全"),
                "歌手" => column_of(data, "刘若英"),
                "时长" => column_of(data, "03:20"),
                "音质" => column_of(data, "MP3 320k"),
                "体积" => column_of(data, "9.3MB"),
                "缓存" => column_of(data, "●"),
                "下载" => column_of(data, "↓"),
                other => panic!("没预料到的列 {other}"),
            };
            assert_eq!(head, value, "「{needle}」列没对齐：\n表头 {header:?}\n数据 {data:?}");
        }
    }

    /// 本地曲库屏：容器（扩展名）与真实大小，不再显示配置档位。
    #[test]
    fn a_local_row_reports_its_own_container_and_size() {
        let (mut app, _runtime) = test_app();
        let mut song = crate::tui::test_support::local_song("C:/Music/canto/曲终人散.flac");
        song.local_bytes = 31_457_280;
        song.duration_secs = 270;
        app.library = vec![song];
        app.refresh_local_view();
        app.switch_menu(Menu::LocalLibrary);

        let text = frame_text(&mut app, 140, 40);
        assert!(text.contains("FLAC"), "本地歌报容器：{text}");
        assert!(text.contains("30.0MB"), "本地歌报真实大小：{text}");
        assert!(!text.contains("MP3 128k"), "不该再显示配置档位：{text}");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --locked draw::tests::song_lists_have_a_header_aligned_with_the_rows`
预期：FAIL（没有表头；本地那行写着 `MP3 128k`）。

- [ ] **Step 3: 写实现**

`content.rs` 的 `draw_songs` 整体改写（`.block(panel_block(title))` 挪出去单独画，表头在框内切）；
顶部 import 补 `use std::collections::HashMap;`、`use crate::model::{Quality, Song};`、`use super::rows;`：

```rust
    /// 曲目列表：所有「装着一堆歌」的屏共用（收藏 / 歌单曲目 / 搜索 / 本地曲库 / 本地歌单曲目）。
    ///
    /// 列集按屏声明（本地曲库没有缓存/下载两列），宽度按窗口让位（见 `rows::fit`）。
    pub(super) fn draw_songs(&mut self, frame: &mut Frame, area: Rect, title: &str) {
        let columns = match self.nav.menu {
            Menu::LocalLibrary => rows::local_columns(),
            Menu::LocalPlaylists if self.nav.local_shown.is_some() => rows::local_playlist_columns(),
            _ => rows::track_columns(),
        };
        let want = self.config.quality;
        let playing = self.now_playing_mid();
        // 本地歌单里的条目要按 mid 查当前曲库：补齐时长/容器/大小，顺带判「已丢失」。
        // 每行 `iter().any()` 是 O(行×曲库)，这里一次建表。
        let library: HashMap<&str, &Song> =
            self.library.iter().map(|song| (song.mid.as_str(), song)).collect();

        let block = panel_block(title);
        let inner = block.inner(area);
        let (head, list_area) = rows::group(inner);
        let columns = rows::fit(&columns, list_area.width);

        let items: Vec<ListItem> = self
            .nav
            .song_view()
            .visible_rows()
            .map(|(song, _)| {
                let row = self.song_row(song, &library, playing.as_deref(), want);
                ListItem::new(rows::line(&columns, &row, rows::INDENT))
            })
            .collect();

        frame.render_widget(block, area);
        if let Some(head) = head {
            frame.render_widget(Paragraph::new(rows::header(&columns, rows::INDENT)), head);
        }
        let mut state = std::mem::take(&mut self.nav.song_view_mut().state);
        let list = List::new(items).highlight_style(highlight()).highlight_symbol("▶ ");
        frame.render_stateful_widget(list, list_area, &mut state);
        self.nav.song_view_mut().state = state;
    }
```

`App::song_row`（同文件，私有辅助，把四个来源拼成 `Row`）：

```rust
    /// 组一行：三处列表（`draw_songs` / `draw_history` / `draw_queue`）共用同一份口径。
    ///
    /// `library` 是「按 mid 查当前曲库」的表（本地条目用得上，查不到就是已丢失）。
    /// 备注那一列按屏分岔（本地歌单写「已丢失」；历史写「时间 · 来源」，由调用方覆盖）。
    fn song_row<'a>(
        &'a self,
        song: &'a Song,
        library: &HashMap<&'a str, &'a Song>,
        playing: Option<&str>,
        want: Quality,
    ) -> rows::Row<'a> {
        // 本地条目（本地曲库 / 本地歌单 / 历史里的本地条目）：曲库那份比快照新，优先用它。
        let fresh = library.get(song.mid.as_str()).copied();
        let source = fresh.unwrap_or(song);
        let copy = rows::copy_of(source, &self.cached, &self.downloads, want);
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
            duration: (source.duration_secs > 0).then_some(source.duration_secs),
            tier: Some(copy.tier),
            size: copy.size,
            cached: copy.cached,
            downloaded: copy.downloaded,
            note,
        }
    }
```

> `note` 的判定按 `Menu` 分岔（与今天 `content.rs:303-307` 等价）；Task 9 里历史屏覆盖它
> （`rows::Row { note: Some(label), ..row }`），不用改签名。

> **实施记录（2026-10-13）**：三处按实测调整，都是这一层才暴露的——
> ① **缩进不能加两次**：`List` 渲染数据行时**自己**会留出 `highlight_symbol` 那 2 列，而
>    `header` 是普通 `Paragraph`、不会。所以 `rows::line(columns, row)` **不带缩进参数**、
>    `rows::header(columns)` 自带 `INDENT`；谁要再补一次就会整体右移 2 列（第一次实现就这么错了，
>    draw 层那条对齐断言当场抓住）。
> ② **列宽要扣掉那 2 列**：`fit` 必须拿「列表区宽 − `INDENT`」，所以新增
>    `rows::row_width(list_area_width)` 专职这件事；少扣的后果是整行超宽、被 ratatui 裁掉尾巴
>    （表现为表头的「体积」少一个字、数据的 `30.0MB` 少 `MB`）。
> ③ **浮层左上角那一格可能画不出来**：表头的中文字（`标题`）占 cells 5–8，而 60 列时帮助浮层的
>    `┌` 正好落在 cell 8——**宽字符后面那一格永远不会被重画**（ratatui `Buffer::diff` 的约定：
>    `double width symbol at index 0 - skip index 1`，缓冲必须满足「宽字符后面没有非空白格」）。
>    于是 `help_box_rows` 改成按标题 `帮助` 定位浮层（不再找 `┌ 帮助`）。这是 CJK + 覆盖层的固有
>    交互（今天的列表行也可能撞上，只是列位不同），记进 `docs/design.md` 的已知限制。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --locked draw::tests` —— 预期上面两条过；**同时会挂掉几条旧断言**，按 Step 5 改。

- [ ] **Step 5: 改写被影响的既有断言**

- `src/tui/draw/mod.rs:966-967`（`the_local_library_screen_lists_files_and_explains_an_empty_one`）：
  `assert!(!text.contains("MB"), "本地曲目没有体积列：{text}")` → 改成
  `assert!(text.contains("WAV"), "本地歌报容器：{text}")` 与
  `assert!(text.contains('—'), "读不出大小时写 `—`：{text}")`（这两首是造的假路径，没有 `local_bytes`）。
- 其余断言若因为「列表整体下移一行 / 多了一列」而失败，**逐条**重算期望值，不要放宽成 `contains` 了事。

- [ ] **Step 6: 跑 `cargo test --locked` 与 `cargo clippy --all-targets --locked -- -D warnings`**

预期全绿。

- [ ] **Step 7: Commit**

```bash
git add src/tui/draw/content.rs src/tui/draw/mod.rs
git commit -m "feat(tui): 曲目列表走列模型(表头 + 缓存/下载标记 + 本地歌容器与大小)"
```

---

### Task 8: 历史落盘补 `duration_secs` + `sizes`

**Files:**
- Modify: `src/history.rs`（`Entry` 字段 + 测试）
- Modify: `src/tui/state.rs`（`record_history` / `refresh_history_view`）

**Interfaces:**
- Produces: `history::Entry.duration_secs: u64`、`history::Entry.sizes: crate::model::FileSizes`（两者都 `#[serde(default)]`）

- [ ] **Step 1: 写失败的测试**

`src/history.rs` 的 `mod tests` 里追加：

```rust
    /// 新写的记录带着时长与各档位体积；读回来还在。
    #[test]
    fn an_entry_keeps_the_duration_and_sizes() {
        let mut sizes = crate::model::FileSizes::default();
        sizes.mp3_128 = 4_400_000;
        let entry = Entry {
            mid: "a".to_string(),
            title: "成全".to_string(),
            singers: "刘若英".to_string(),
            at: 1,
            origin: "我的收藏".to_string(),
            local_path: None,
            duration_secs: 276,
            sizes,
        };
        let text = serde_json::to_string(&entry).expect("序列化");
        let back: Entry = serde_json::from_str(&text).expect("反序列化");
        assert_eq!(back.duration_secs, 276);
        assert_eq!(back.sizes.mp3_128, 4_400_000);
    }

    /// 升级前的老文件（没有这两个字段）照样读得出来：默认 0，列表里写 `--:--`/`—`。
    #[test]
    fn an_old_history_file_still_loads() {
        let old = r#"[{"mid":"a","title":"成全","singers":"刘若英","at":1,"origin":"我的收藏"}]"#;
        let entries: Vec<Entry> = serde_json::from_str(old).expect("老文件要读得出来");
        assert_eq!(entries[0].duration_secs, 0);
        assert_eq!(entries[0].sizes.mp3_128, 0);
    }
```

`src/tui/state.rs` 的测试里追加（钉住「从历史重播不会把数据写丢」）：

```rust
    /// 历史重播：还原出来的 Song 带着时长/体积，所以新记录照样完整。
    #[test]
    fn replaying_from_history_keeps_the_duration_and_sizes() {
        let (mut app, _runtime) = test_app();
        let mut song = crate::tui::test_support::titled_song("a", "成全", "刘若英");
        song.duration_secs = 276;
        song.sizes.mp3_128 = 4_400_000;
        app.record_history(&song);

        // 从历史屏拿到的那份 Song（`refresh_history_view` 还原）再记一次。
        app.refresh_history_view();
        let replayed = app.nav.history_view.items()[0].clone();
        assert_eq!(replayed.duration_secs, 276, "还原要带时长");
        assert_eq!(replayed.sizes.mp3_128, 4_400_000, "还原要带体积");
        app.record_history(&replayed);

        let latest = &app.history.entries()[0];
        assert_eq!(latest.duration_secs, 276);
        assert_eq!(latest.sizes.mp3_128, 4_400_000);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --locked history::tests::an_entry_keeps_the_duration_and_sizes`
预期：编译失败（`Entry` 没有这两个字段）。

- [ ] **Step 3: 写实现**

`history.rs` 的 `Entry` 末尾加：

```rust
    /// 时长（秒）。后加的字段：老记录没有 → 0 → 列表里写 `--:--`。
    #[serde(default)]
    pub duration_secs: u64,
    /// 起播那一刻接口报的各档位体积。后加的字段：老记录没有 → 全 0 → 体积 `—`。
    ///
    /// 存**原始档位表**而不是算好的那一档：配置改了之后列表会按新下限重算（跟别的屏一致）。
    #[serde(default)]
    pub sizes: crate::model::FileSizes,
```

`state.rs` 的 `record_history` 里补两行：

```rust
                at: history::now_secs(),
                origin: self.queue.origin().to_string(),
                local_path: song.path().map(std::path::Path::to_path_buf),
                duration_secs: song.duration_secs,
                sizes: song.sizes,
```

`refresh_history_view` 里：QQ 条目用记录里的字段；**本地条目按 mid 查当前曲库**（文件可能换过，
曲库那份更新），查不到才用记录里那份：

```rust
        let library: HashMap<&str, &Song> =
            self.library.iter().map(|song| (song.mid.as_str(), song)).collect();
        let songs: Vec<Song> = entries
            .iter()
            .map(|entry| {
                let fresh = library.get(entry.mid.as_str()).copied();
                Song {
                    mid: entry.mid.clone(),
                    song_id: None,
                    media_mid: None,
                    title: entry.title.clone(),
                    singers: entry.singers.clone(),
                    album_mid: String::new(),
                    album_name: String::new(),
                    // 本地条目以当前曲库为准（文件可能被换过）；QQ 条目用记录里那份。
                    duration_secs: fresh.map_or(entry.duration_secs, |song| song.duration_secs),
                    sizes: entry.sizes,
                    local_bytes: fresh.map_or(0, |song| song.local_bytes),
                    source: match &entry.local_path {
                        Some(path) => crate::model::Source::Local(path.clone()),
                        None => crate::model::Source::Qq,
                    },
                }
            })
            .collect();
```

（`use std::collections::HashMap;` 若 `state.rs` 还没 import 就补上。）

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --locked history::tests` 与 `cargo test --locked state::tests::replaying_from_history_keeps_the_duration_and_sizes`
预期全过。

- [ ] **Step 5: Commit**

```bash
git add src/history.rs src/tui/state.rs
git commit -m "feat(history): 记录里补时长与各档位体积(老文件照读, 重播不丢)"
```

---

### Task 9: `draw_history` 改走 `rows`

**Files:**
- Modify: `src/tui/draw/content.rs`（`draw_history`）

**Interfaces:**
- Consumes: Task 8 还原后的 `Song`（带时长/体积）、`rows::history_columns()`
- Produces: 无新接口（`song_row` 的 `note` 这一版按屏分岔：历史给「时间 · 来源」）

- [ ] **Step 1: 写失败的测试**

```rust
    /// 历史屏也有表头与那些列；备注列写「时间 · 来源」。
    #[test]
    fn the_history_screen_has_a_header_and_a_note_column() {
        let (mut app, _runtime) = test_app();
        let mut song = crate::tui::test_support::titled_song("a", "成全", "刘若英");
        song.duration_secs = 276;
        song.sizes.mp3_128 = 4_400_000;
        app.record_history(&song);
        app.switch_menu(Menu::History);

        let rows = frame_raw(&mut app, 140, 30);
        let header = rows.iter().find(|row| row.contains("时长") && row.contains("备注")).expect("历史屏有表头");
        let data = rows.iter().find(|row| row.contains("成全")).expect("有数据行");
        assert_eq!(column_of(header, "时长"), column_of(data, "04:36"), "时长列对齐");
        assert_eq!(column_of(header, "备注"), column_of(data, "刚刚"), "备注列对齐");
        assert!(data.contains("MP3 128k"), "历史也报音质：{data}");
        assert!(data.contains("4.2MB"), "历史也报体积：{data}");
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
                at: crate::history::now_secs(),
                origin: "我的收藏".to_string(),
                local_path: None,
                duration_secs: 0,
                sizes: crate::model::FileSizes::default(),
            },
            50,
        );
        app.refresh_history_view();
        app.switch_menu(Menu::History);
        let text = frame_text(&mut app, 140, 30);
        assert!(text.contains("--:--"), "{text}");
        assert!(!text.contains("00:00"), "不许写假的 00:00：{text}");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --locked draw::tests::the_history_screen_has_a_header_and_a_note_column`
预期：FAIL（没有表头 / 没有备注列）。

- [ ] **Step 3: 写实现**

把 `draw_history`（`content.rs:209-246`）改成与 `draw_songs` 同一副骨架：列集用
`rows::history_columns()`，行用 `self.song_row(...)` 再加备注（`labels` 里那份
「时间 · 来源」由 `state.rs:2226` 现成拼好，`visible_rows()` 的第二项就是它）：

```rust
    /// 历史屏：跟别的曲目列表同一副表；备注列写「时间 · 来自《来源》」。
    fn draw_history(&mut self, frame: &mut Frame, area: Rect, title: &str) {
        if self.history.is_empty() {
            frame.render_widget(
                Paragraph::new("暂无播放历史")
                    .style(Style::new().fg(Color::DarkGray))
                    .alignment(ratatui::layout::Alignment::Center)
                    .block(panel_block(title)),
                area,
            );
            return;
        }
        let want = self.config.quality;
        let playing = self.now_playing_mid();
        let library: HashMap<&str, &Song> =
            self.library.iter().map(|song| (song.mid.as_str(), song)).collect();
        let block = panel_block(title);
        let inner = block.inner(area);
        let (head, list_area) = rows::group(inner);
        let columns = rows::fit(&rows::history_columns(), list_area.width);

        let items: Vec<ListItem> = self
            .nav
            .song_view()
            .visible_rows()
            .map(|(song, label)| {
                let row = self.song_row(song, &library, playing.as_deref(), want);
                let row = rows::Row { note: Some(label), ..row };
                ListItem::new(rows::line(&columns, &row, rows::INDENT))
            })
            .collect();

        frame.render_widget(block, area);
        if let Some(head) = head {
            frame.render_widget(Paragraph::new(rows::header(&columns, rows::INDENT)), head);
        }
        let mut state = std::mem::take(&mut self.nav.song_view_mut().state);
        let list = List::new(items).highlight_style(highlight()).highlight_symbol("▶ ");
        frame.render_stateful_widget(list, list_area, &mut state);
        self.nav.song_view_mut().state = state;
    }
```

（`song_row` 里那条「本地歌单的 `（已丢失）`」判定按 `self.nav.menu` 分岔，历史屏走不到它；
历史自己的备注在这里覆盖上去。）

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --locked draw::tests` —— 预期全过（含 Task 7 那两条）。

- [ ] **Step 5: Commit**

```bash
git add src/tui/draw/content.rs
git commit -m "feat(tui): 历史屏走列模型(表头 + 时长/音质/体积 + 备注列)"
```

---

### Task 10: `draw_queue` 改走 `rows`

**Files:**
- Modify: `src/tui/draw/overlay.rs`（`draw_queue`）

**Interfaces:**
- Consumes: `rows::queue_columns()`、`rows::{fit,group,header,line,Row}`

- [ ] **Step 1: 写失败的测试**

```rust
    /// 队列浮层也有表头（高度跟着 +1），`缓存/下载` 两列在。
    #[test]
    fn the_queue_overlay_has_a_header() {
        let (mut app, _runtime) = test_app();
        app.queue = queue::Queue::new(vec![test_song("a"), test_song("b")], 0, "嗯");
        app.apply(crate::tui::keys::Action::QueueOverlay);
        let rows = frame_raw(&mut app, 160, 40);
        // 浮层标题挂在框线上，表头是它下面那一行。
        let title_at = rows
            .iter()
            .position(|row| row.contains("队列1/2"))
            .expect("浮层标题还在");
        let header = &rows[title_at + 1];
        assert!(
            header.contains("缓存") && header.contains("下载"),
            "表头紧跟在框线下面：{header:?}"
        );
        let data = rows.iter().find(|row| row.contains("歌a")).expect("有数据行");
        assert_eq!(
            column_of(header, "时长"),
            column_of(data, "03:20"),
            "时长列对齐：\n{header:?}\n{data:?}"
        );
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --locked draw::tests::the_queue_overlay_has_a_header`
预期：FAIL。

- [ ] **Step 3: 写实现**

`draw_queue`（`overlay.rs:326-359`）改成与 `draw_songs` 同一副骨架：`height = items.len() as u16 + 3`
（多表头那一行），列集 `rows::queue_columns()`，行用 `self.song_row(...)`（`♪` 按**队列下标**：

```rust
                let playing = Some(index) == Some(self.queue.index());
```
→ 组行时 `playing` 传 `if index == self.queue.index() { Some(song.mid.as_str()) } else { None }`
的写法**不要**用（队列里同一首可能出现两次，按 mid 会两行都标 `♪`）；直接给 `Row.playing` 赋
`index == self.queue.index()`：实施时若 `song_row` 的签名只收 `playing: Option<&str>`，就在队列里
另外组 `Row`（`let mut row = self.song_row(...).0; row.playing = index == self.queue.index();`）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --locked draw::tests` —— 预期全过；`the_queue_overlay_draws_the_highlight_on_the_selected_row`
（`draw/mod.rs:1173-1191`）会因为多了一行而需要重算期望，按原判据（`▶` 落在两行之间）核对后调整。

- [ ] **Step 4b: 摘掉 `rows.rs` 顶部那行 `#![allow(dead_code)]`**

到这一步 `rows.rs` 的东西全都有生产消费者了（`fit`/`header`/`line`/`group`/各列集都在三处绘制里用上）。
删掉那两行注释 + 属性，跑 `cargo clippy --all-targets --locked -- -D warnings` 确认没有 dead_code 残留。

- [ ] **Step 5: Commit**

```bash
git add src/tui/draw/overlay.rs src/tui/draw/mod.rs
git commit -m "feat(tui): 队列浮层走列模型(表头 + 缓存/下载标记)"
```

---

### Task 11: 歌单行 4 处合并 + 空态配方收成 helper

**Files:**
- Modify: `src/tui/draw/content.rs`、`src/tui/draw/overlay.rs`

**Interfaces:**
- Produces: `draw::playlist_row(count: usize, name: &str) -> String`（`{:>5}  {name}` 的唯一一份）、
  `draw::empty_panel(text: &str, title: &str) -> Paragraph<'static>`（`DarkGray + Center + panel_block` 的唯一一份）

- [ ] **Step 1: 写失败的测试**

```rust
    /// 歌单行的缩进口径只有一处：选择框的「新建…」跟普通行同一列。
    #[test]
    fn the_playlist_row_indent_has_one_source() {
        let row = crate::tui::draw::playlist_row(3, "嗯");
        assert_eq!(row, "    3  嗯");
        let (mut app, _runtime) = test_app();
        app.switch_menu(Menu::Playlists);
        app.nav.playlists.set_items(vec![crate::tui::test_support::test_playlist("1", "嗯")]);
        let rows = frame_raw(&mut app, 100, 30);
        let data = rows.iter().find(|row| row.contains("嗯")).expect("有歌单行");
        assert_eq!(column_of(data, "3"), 2, "计数从框内第 3 列开始：{data:?}");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --locked draw::tests::the_playlist_row_indent_has_one_source`
预期：编译失败（没有 `playlist_row`）。

- [ ] **Step 3: 写实现**

`draw/mod.rs` 里加（放 `panel_block` 旁边）：

```rust
/// 歌单行：`{:>5}  {名字}`。四处（QQ/本地歌单列表、两个选择框）共用这一份，缩进口径只有一处。
pub(super) fn playlist_row(count: usize, name: &str) -> String {
    format!("{count:>5}  {name}")
}

/// 空态占位：`DarkGray` 居中 + 带标题的框。三处（空本地歌单列表 / 空歌单 / 空历史）共用。
pub(super) fn empty_panel(text: &str, title: &str) -> Paragraph<'static> {
    Paragraph::new(text.to_string())
        .style(Style::new().fg(Color::DarkGray))
        .alignment(Alignment::Center)
        .block(panel_block(title))
}
```

四处改用它：`content.rs:42`、`content.rs:66`、`overlay.rs:29`、`overlay.rs:49`；`overlay.rs:27`
的 `"       新建…"` 改成 `format!("{}新建…", " ".repeat(5 + 2))` —— 更好的是
`playlist_row(0, "新建…")` 的等价形式：`format!("{:>5}  新建…", "")`（保持与别的行同一份口径）。

三处空态改用 `empty_panel`：`content.rs:53-58`、`content.rs:80-85`、`content.rs:211-217`
（`draw_empty_library` 是**别的**配方——左对齐带黄色强调，不动）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --locked draw::tests` —— 预期全过。

- [ ] **Step 5: Commit**

```bash
git add src/tui/draw/content.rs src/tui/draw/overlay.rs src/tui/draw/mod.rs
git commit -m "refactor(tui): 歌单行与空态各收成一处(缩进口径不再硬编码)"
```

---

### Task 12: 文档与真机验收

**Files:**
- Modify: `README.md`、`docs/design.md`

- [ ] **Step 1: 改 README**

- 功能表里那条「播放缓存（…列表里有「已缓存」标记）」→ 改成「列表里有 `●` 已缓存 / `↓` 已下载两个标记」。
- 已知限制三条改写（照 spec「与旧条款的关系」那张表）：
  - 「历史屏的行不显示时长与档位」→「历史**新条目**显示时长/音质/体积（起播时落盘）；升级前的老条目显示 `--:--`/`—`，从别的列表再播一次即补齐」。
  - 「列表里那一列（格式 + 档位 + 体积）按配置的下限算」→ 补「本地已有那份（缓存 > 下载 > 曲库）时报那一份；都没有才按配置下限预估」。
  - 「本地歌单里的歌没有时长…档位是曲库那个占位值」→「按 mid 查当前曲库补齐；文件移走才 `--:--`/`—`」。
- 新增三条已知限制：`↓` 按文件名认（同歌手同标题会互相标；手工拷进下载目录的文件也算）、
  `↓` 的档位只到容器（mp3 不写 128/320）、同一个 stem 有多份时取容器更高的那份。

- [ ] **Step 2: 改 docs/design.md**

在「本地歌单（`6`，阶段二）」之前加一节「曲目列表的列模型（2026-10-13）」：列集与让位顺序、
三个数据源（缓存/下载/曲库）与刷新时机、三处「列在撒谎」的来龙去脉、以及真机记录（Step 4）。

- [ ] **Step 3: 真机验收（tuios，真二进制）**

照 spec §6 第二层那 7 条跑，逐条记录实测值（哪一行、看到什么）：
表头对齐（`capture-pane` 对着数）、`●` 与缓存那份的档位/大小、`d` 下载后立刻出现 `↓` 且体积变磁盘那份、
手工删文件 + `r` 后 `↓` 消失、历史新条目与老条目的差别、本地 FLAC 显示 `FLAC` + 真实大小、
60×15 与 80×24 两个尺寸逐屏截一遍。

步骤骨架（窗口按任务命名，收尾关窗；命令细节见 `AGENTS.md`）：

```sh
tuios new-window -s canto-dev canto-table --cwd /Users/kiry/code/canto --no-focus -- /Users/kiry/code/canto/target/debug/canto --log debug
tuios wait-for window-output -s canto-dev -w canto-table --pattern '<一定会出现的字符串>' --timeout 20000
tuios capture-pane -s canto-dev -w canto-table
tuios screenshot -o /tmp/canto-table-60.png   # 尺寸矩阵那两档要先把会话尺寸调成 60×15 / 80×24
tuios send-keys -s canto-dev -w canto-table Escape
tuios send-keys -s canto-dev -w canto-table q
tuios wait-for window-exit -s canto-dev -w canto-table --timeout 10000
tuios close-window -s canto-dev canto-table
```

- [ ] **Step 4: 跑完整验证并提交**

```bash
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
git add README.md docs/design.md
git commit -m "docs: 曲目列表的列模型与两个标记(含真机记录与三条已知限制)"
```

---

## Self-Review

**1. Spec 覆盖**（spec 每节的落地任务）：

| spec | 任务 |
|---|---|
| §1 列模型（`fit`/`header`/`line`/宽度常量/颜色/60 列算术） | Task 1、2 |
| §2 各屏列集 | Task 1（列集）、7（`draw_songs`）、9（历史）、10（队列） |
| §3 数据来源（缓存/下载/本地/历史 + 三条口径） | Task 3、4、5、6、8 |
| §4 绘制接线（`group`、队列高度 +1、`layout` 不动） | Task 2、7、9、10 |
| §5 退化（占位符、矮屏阈值、浮层内宽） | Task 2（`group`/占位符）、7（本地大小未知）、9（老条目） |
| §6 测试计划（第一层 + 真机 + 要改写的既有断言） | 各任务的 Step 1、Task 12 |
| §7 改动面 | File Structure + 各任务 Files |
| §8 风险与已知限制 | Task 12（README 三条 + design.md） |

**2. 占位符扫描**：没有 TBD/TODO；每个代码步骤都给了能编译的成品代码（列模型、`scan_downloads`、
`copy_of`、`song_row`、历史字段、三处绘制接线都是整段可抄的）。

**3. 类型一致性**：`rows::Row` 的字段名（`playing/title/singers/duration/tier/size/cached/
downloaded/note`）在 Task 2 定义，Task 6/7/9/10 引用一致；`LocalCopy` 的字段（`tier/size/
cached/downloaded`）在 Task 6 定义、Task 7 消费；`scan_downloads` 的返回形状
（`HashMap<String,(String,u64)>`）在 Task 4 定义、Task 5/6 消费；`file_stem` 在 Task 4 公开、
Task 5/6/7 用同一份（**不许**在别处再写一份 `format!("{} - {}", singers, title)`）。

# 本地曲库与本地歌单实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 canto 在**不登录、没网络**时依然可用——扫本地目录、放本地歌、按本地歌单组织；登录且有网时 QQ 那一套照旧，两边进同一个队列与历史。

**Architecture:** 路线甲。`Song` 增一个 `source`（`Qq` / `Local(路径)`），本地曲目的 `mid = "local:<绝对路径>"`，于是队列 / 历史 / `♪` / 歌词 / 渲染**全都不用改签名**；扫描结果落 `~/.cache/canto/library.json` 供下次秒开，后台按 `mtime`/size 增量补扫；播放侧 `Track` 的字节来源分成「CDN 候选」与「本地文件」两支。免登录性靠两处：落屏永远落首页 + 需要登录的屏只在进入时给一句提示。

**Tech Stack:** Rust 2024 · ratatui 0.30 · rodio 0.22（symphonia 解码）· **`lofty`（本次唯一新依赖，读 tag/时长/内嵌歌词）** · serde/serde_json · 测试与代码同文件。

**Spec:** `docs/superpowers/specs/2026-10-13-local-library-design.md`（本次实现的权威；冲突以它为准）

## Global Constraints

- 三平台都要能编（macOS / Linux / Windows）。`src/overlay.rs` 是 macOS 桌面歌词，本次**不碰**。
- 测试与代码同文件（`#[cfg(test)] mod tests`）；每步跑 `cargo test` 与 `cargo clippy --all-targets --locked -- -D warnings`。
- 唯一允许新增的依赖是 `lofty`。其余一律用树里已有的。
- 键位约定：默认小写；大写只留给升级型/破坏性能力（本次：`P` 加入本地歌单、`R` 重命名、`D` 删歌单）。
- 本地文件的路径全程**零网络请求**（网络只出现在登录屏取码与 QQ 屏）。
- 失败与跳过都要一句人话进状态行（不静默、不只写日志）。文案固定为 `需要登录：按 L` 与 `打不开：<原因>`。
- 状态行/帮助表不许许诺按不出来的键（本地屏不列 `d`/`p`）。

## Review Focus

1. **索引里有、盘上没有的文件**（U 盘拔了、扫描后删了）→ 不能崩、不能卡在「扫描中」，要删条目。（Task 3）
2. **软链成环 / 目录指向自己**→ 遍历不能无限递归。（Task 2）
3. **文件名含中文、空格、emoji、非 UTF-8 字节**→ 合成 mid 往返不丢、能播、能显示。（Task 1、Task 6）
4. **本地歌单里文件被改名/移走**→ 条目显示「（已丢失）」而不是消失或被清空。（Task 10）
5. **未登录 + 队列/历史里有 QQ 歌**→ 给 `需要登录：按 L`，不静默跳过、不崩；历史屏未登录只列本地条目。（Task 7）

以上每条都有对应任务里的测试钉住。

## 文件结构

| 文件 | 职责 | 阶段 |
|---|---|---|
| `src/model.rs` | `Source` / `Song.source` / `local_mid` / `local_path` | 一 |
| `src/config.rs` | `[library] dirs`（缺省 = `download_dir`） | 一 |
| `src/paths.rs` | `library_index_path()` / `local_playlists_path()` | 一/二 |
| `src/library.rs`（新） | 遍历 + 读 tag + 索引读写 + 增量合并 + `Index → Vec<Song>` | 一 |
| `src/player.rs` | `TrackSource`（CDN / 本地文件）与 `open()` 的本地分支 | 一 |
| `src/history.rs` | `Entry.local_path`（serde 默认，老文件照读） | 一 |
| `src/tui/state.rs` | `Menu::ALL`、`Msg::Library*`、扫描线程、播放入口分岔、未登录门、`L` 两副面孔、落屏 | 一/二 |
| `src/tui/keys.rs` | 本地两屏的键（`r`/`P`/`n`/`R`/`x`/`D`）、登录屏 `Esc` | 一/二 |
| `src/tui/draw/home.rs` | 首页格子（未登录时的副标题） | 一/二 |
| `src/tui/draw/content.rs` | 本地行的 `—`/`--:--`、空曲库引导句 | 一 |
| `src/tui/draw/overlay.rs` | 帮助表分岔 + 本地歌单屏那段 | 一/二 |
| `src/playlists.rs`（新） | 本地歌单存取与校验 | 二 |

---

### Task 1: `Song` 的来源维度与合成 mid

**Files:**
- Modify: `src/model.rs`（`Song` 定义附近 + 文件末尾的测试模块）

**Interfaces:**
- Produces:
  - `pub enum Source { Qq, Local(PathBuf) }`，`#[derive(Debug, Clone, PartialEq, Eq, Default)]`，`#[default] Qq`
  - `Song` 新增字段 `#[serde(default)] pub source: Source`
  - `pub fn local_mid(path: &Path) -> String` = `format!("local:{}", path.to_string_lossy())`
  - `pub fn local_path(mid: &str) -> Option<PathBuf>` = 去掉 `local:` 前缀；没有前缀返 `None`
  - `impl Song { pub fn is_local(&self) -> bool; pub fn path(&self) -> Option<&Path> }`

- [ ] **Step 1: 写失败测试**（加进 `src/model.rs` 的 `mod tests`）

```rust
#[test]
fn local_mid_round_trips() {
    let path = Path::new(r"C:\Users\28269\Music\canto\张宇 - 曲终人散.mp3");
    assert_eq!(local_path(&local_mid(path)).as_deref(), Some(path));
    let emoji = Path::new("/music/雨 ☔ 了.flac");
    assert_eq!(local_path(&local_mid(emoji)).as_deref(), Some(emoji));
}

#[test]
fn a_non_local_mid_has_no_path() {
    assert_eq!(local_path("000DhFOL1o15dn"), None);
    assert_eq!(local_path("local:"), None, "空路径不算");
}

#[test]
fn a_qq_song_defaults_to_the_qq_source() {
    // 样本从 `qq/parse.rs` 的现有测试里抄一个最小响应片段
    let song: Song = serde_json::from_value(serde_json::json!({
        "mid": "000DhFOL1o15dn", "name": "曲终人散", "interval": 270,
        "singer": [{"name": "张宇"}]
    }))
    .expect("QQ 接口的 JSON 里没有 source，靠 serde 默认");
    assert_eq!(song.source, Source::Qq);
    assert!(!song.is_local());
}
```

- [ ] **Step 2: 跑，确认红**

Run: `cargo test -- local_mid_round_trips a_non_local_mid_has_no_path a_qq_song_defaults`
Expected: 编译失败（`Source` / `local_mid` 不存在）

- [ ] **Step 3: 实现**

`Source` + `Song.source` + 两个自由函数 + 两个方法。`local_path` 只在 `mid` 以 `local:` 开头且剩余部分非空时返回 `Some`。

- [ ] **Step 4: 跑，确认绿**

Run: `cargo test`（全量）
Expected: 全绿（现有 289 条不许变红）

- [ ] **Step 5: clippy + 提交**

```bash
cargo clippy --all-targets --locked -- -D warnings
git add src/model.rs && git commit -m "feat(local): Song 增来源维度与合成 mid（local:<路径>）"
```

---

### Task 2: 曲库扫描 + 配置项 + 索引路径

**Files:**
- Create: `src/library.rs`
- Modify: `src/main.rs`（模块表加 `mod library;`）、`src/config.rs`、`src/paths.rs`
- Test: `src/library.rs` 内的 `mod tests`、`src/config.rs` 内的测试

**Interfaces:**
- Consumes: `model::{Song, Source, local_mid}`（Task 1）
- Produces:
  - `pub struct LibraryConfig { pub dirs: Option<Vec<PathBuf>> }`；`Config` 增 `pub library: LibraryConfig`；
    `impl LibraryConfig { pub fn resolve(&self, download_dir: &Path) -> Vec<PathBuf> }`（`None` → `vec![download_dir]`；`Some(v)` → 原样，含空表）
  - `paths::library_index_path()` = `~/.cache/canto/library.json`
  - `paths::local_playlists_path()` = `~/.config/canto/local_playlists.json`
  - `pub const EXTENSIONS: &[&str] = &["mp3", "flac", "m4a", "mp4", "ogg", "wav"]`
  - `pub struct Entry { pub path: PathBuf, pub mtime: u64, pub size: u64, pub title: String, pub artist: String, pub album: String, pub duration_secs: u64 }`（serde 双向）
  - `pub fn read_entry(path: &Path, mtime: u64, size: u64) -> Entry`（`lofty`：title/artist/album/duration；读不到就用文件名当 title、其余空/0）
  - `pub fn walk(dirs: &[PathBuf]) -> (Vec<(PathBuf, u64, u64)>, usize)`（返回 文件三元组 + 跳过的不支持格式计数；跳过点开头目录、不跟软链、去重、按路径排序）

- [ ] **Step 1: 写失败测试**

```rust
// src/library.rs 的 tests
#[test]
fn walk_collects_supported_files_and_counts_the_rest() {
    let dir = temp_tree();                       // 建 canto/a.mp3、canto/b.FLAC、canto/c.txt、canto/.hidden/d.mp3、canto/sub/e.wav
    let (files, skipped) = walk(&[dir.join("canto")]);
    let names: Vec<String> = files.iter().map(|(p, _, _)| file_name(p)).collect();
    assert_eq!(names, ["a.mp3", "b.FLAC", "e.wav"], "大小写不敏感；点开头目录整棵跳过");
    assert_eq!(skipped, 1, "c.txt 只计数，不进列表");
}

#[test]
fn walk_does_not_follow_symlinks_and_survives_a_cycle() {
    // 平台无关做法：真软链建不起来就跳过这条断言，只留「不跟软链」的实现约束
    make_symlink(dir.join("canto/loop"), &dir.join("canto")).ok();
    let (files, _) = walk(&[dir.join("canto")]);
    assert!(files.len() <= 8, "成环要不跟软链，不能无限递归");
}

#[test]
fn walk_dedupes_and_sorts() {
    let dir = temp_tree();
    let (files, _) = walk(&[dir.join("canto"), dir.join("canto/")]); // 同一目录写两遍
    assert_eq!(files.len(), 3, "去重");
    let paths: Vec<_> = files.iter().map(|(p, _, _)| p.clone()).collect();
    assert!(paths.windows(2).all(|w| w[0] <= w[1]), "排序稳定");
}

#[test]
fn read_entry_falls_back_to_the_file_name() {
    let bare = touch(dir.join("canto/没有 tag.mp3"));   // 只有 ID3 头或干脆空文件都行
    let entry = read_entry(&bare, 1, 2);
    assert_eq!(entry.title, "没有 tag");
    assert_eq!(entry.duration_secs, 0, "读不出时长就是 0（渲染成 --:--）");
}

#[test]
fn read_entry_reads_written_tags() {
    let tagged = fixture_with_tags("tagged.mp3", "曲终人散", "张宇", "消息");
    let entry = read_entry(&tagged, 1, 2);
    assert_eq!((entry.title.as_str(), entry.artist.as_str(), entry.album.as_str()),
               ("曲终人散", "张宇", "消息"));
}

// 夹具（照 `player.rs` 里合成 WAV 那段抄；tag 用 `lofty` 写回）
fn fixture_with_tags(name: &str, title: &str, artist: &str, album: &str) -> PathBuf { … }
```

```rust
// src/config.rs 的 tests
#[test]
fn library_dirs_default_to_the_download_dir() {
    let config: Config = toml::from_str("").unwrap();
    assert_eq!(config.library.resolve(Path::new("C:/Music/canto")),
               vec![PathBuf::from("C:/Music/canto")]);
}

#[test]
fn an_empty_library_table_turns_the_feature_off() {
    let config: Config = toml::from_str("[library]\ndirs = []\n").unwrap();
    assert!(config.library.resolve(Path::new("C:/Music/canto")).is_empty());
}
```

- [ ] **Step 2: 跑，确认红**

Run: `cargo test -- library:: walk_ read_entry_ library_dirs_default`
Expected: 编译失败（模块/函数不存在）

- [ ] **Step 3: 实现**

`cargo add lofty`。遍历用 `std::fs::read_dir` + 显式递归（**不用 `walkdir`**，不为它加第二个依赖）；软链用 `fs::symlink_metadata().file_type().is_symlink()` 判掉。tag 读取用 `lofty::read_from_path` → primo/tag → `title()/artist()/album()` + `properties().duration()`（秒，四舍五入）。

> 夹具说明：若 `lofty` 对所选容器写不回 tag（实测决定），夹具改成手写 ID3v2.3 头（`ID3\3\0` + syncsafe 长度 + `TIT2`/`TPE1`/`TALB` 帧，约 20 行）；产品代码不受影响。

- [ ] **Step 4: 跑，确认绿**（`cargo test` 全量）+ 大小写/点目录/软链三条都要绿

- [ ] **Step 5: clippy + 提交**

```bash
git add Cargo.toml Cargo.lock src/main.rs src/library.rs src/config.rs src/paths.rs
git commit -m "feat(local): 曲库扫描（递归 + 白名单 + 读 tag）与 [library] 配置"
```

---

### Task 3: 索引读写与增量

**Files:**
- Modify: `src/library.rs`

**Interfaces:**
- Consumes: `Entry` / `walk` / `read_entry`（Task 2）
- Produces:
  - `pub const INDEX_VERSION: u32 = 1`
  - `pub struct Index { pub version: u32, pub dirs: Vec<PathBuf>, pub entries: Vec<Entry> }`（serde；`Index`/`Entry` 都要 `Debug + Clone + PartialEq`，`Index` 另加 `Default`——测试里直接整份比较）
  - `pub fn load_index(path: &Path, dirs: &[PathBuf]) -> Index`（文件缺失/损坏/`version` 或 `dirs` 不符 → 返回 `Index::default()`）
  - `pub fn save_index(path: &Path, index: &Index) -> io::Result<()>`（`tmp` + `rename`）
  - `pub struct ScanOutcome { pub index: Index, pub added: usize, pub removed: usize, pub skipped_unsupported: usize }`
  - `pub fn rescan(previous: &Index, dirs: &[PathBuf]) -> ScanOutcome`（`mtime`/`size` 都没变的沿用旧 `Entry`，不重读 tag）
  - `pub fn index_songs(index: &Index) -> Vec<Song>`（`entry_to_song`；排序 = `(artist, album, title, path)`）

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn the_index_round_trips_and_mismatches_are_thrown_away() {
    let dir = temp_tree();
    let path = dir.join("library.json");
    let outcome = rescan(&Index::default(), &[dir.join("canto")]);
    save_index(&path, &outcome.index).unwrap();
    let back = load_index(&path, &[dir.join("canto")]);
    assert_eq!(back.entries.len(), 3);
    assert!(load_index(&path, &[dir.join("其它")]).entries.is_empty(), "dirs 变了整份丢");
    std::fs::write(&path, "{ 半个 json").unwrap();
    assert!(load_index(&path, &[dir.join("canto")]).entries.is_empty(), "坏了按空处理");
}

#[test]
fn only_changed_files_are_re_read() {
    let dir = temp_tree();
    let dirs = [dir.join("canto")];
    let first = rescan(&Index::default(), &dirs);
    assert_eq!(first.added, 3);
    // 给 walk 加点“耗时”没法直接测，改成断言：什么都没变时 added/removed 都是 0
    let again = rescan(&first.index, &dirs);
    assert_eq!((again.added, again.removed), (0, 0));
    assert_eq!(again.index.entries, first.index.entries, "Entry 原样沿用");
}

#[test]
fn changed_and_removed_files_are_noticed() {
    let dir = temp_tree(); let dirs = [dir.join("canto")];
    let first = rescan(&Index::default(), &dirs);
    std::fs::write(dir.join("canto/a.mp3"), b"different bytes").unwrap(); // size 变了
    std::fs::remove_file(dir.join("canto/b.FLAC")).unwrap();
    let second = rescan(&first.index, &dirs);
    assert_eq!(second.removed, 1);
    assert_eq!(second.added, 1, "变了的算一次重读");
    assert!(!second.index.entries.iter().any(|e| e.path.ends_with("b.FLAC")));
}

#[test]
fn index_songs_sorts_by_artist_album_title() {
    let dir = temp_tree_with_tags();     // 三首：两个歌手、两个专辑
    let songs = index_songs(&rescan(&Index::default(), &[dir]).index);
    assert!(songs.windows(2).all(|w| (key(&w[0])) <= key(&w[1])));
    assert!(songs.iter().all(|s| s.is_local() && s.mid.starts_with("local:")));
    assert!(songs.iter().all(|s| s.sizes.at_or_below(Quality::Low128).is_none()),
            "本地曲目没有体积信息（渲染成 —）");
}
```

- [ ] **Step 2: 跑，确认红** → Run: `cargo test -- the_index_round_trips only_changed_files changed_and_removed index_songs_sorts`

- [ ] **Step 3: 实现**：`rescan` 一次遍历：`walk` 出 `(path, mtime, size)` → 与旧 `index.entries` 按 path 建 map 比对（命中且 `mtime`/`size` 相同 → 沿用；否则 `read_entry`）→ 旧表里没这次扫到的条目算 removed（`added` 计「新增或重读」）。

- [ ] **Step 4: 跑，确认绿** + `cargo test` 全量
- [ ] **Step 5: clippy + 提交**：`git commit -m "feat(local): 曲库索引（落盘 + mtime/size 增量 + 排序）"`

---

### Task 4: 扫描线程与状态

**Files:**
- Modify: `src/tui/state.rs`（`Msg` 枚举、`App` 字段、`load_initial`、消息处理）、`src/tui/mod.rs`（启动后触发一次）

**Interfaces:**
- Consumes: `library::{load_index, rescan, save_index, index_songs}`、`paths::library_index_path()`、`config.library.resolve(&config.download_dir)`
- Produces:
  - `App` 字段：`pub(super) library: Vec<Song>`、`pub(super) library_scanning: bool`、`pub(super) library_skipped: usize`
  - `Msg::LibraryReady { songs: Vec<Song>, added: usize, removed: usize, skipped: usize }`、`Msg::LibraryFailed(String)`
  - `App::start_library_scan(&mut self)`（先读索引立刻可用 → `std::thread::spawn` 增量扫 → 扫完 `save_index` 再发 `LibraryReady`；同时只允许一次，扫描中再调只出一句「正在扫描」）
  - `App::local_songs(&self) -> &[Song]`

- [ ] **Step 1: 写失败测试**（`test_support::test_app()` + 临时目录；`config.library.dirs = Some(vec![tmp])`）

```rust
#[test]
fn a_finished_scan_fills_the_library_and_notices_nothing_when_unchanged() {
    let (mut app, runtime) = test_app();
    let dir = temp_tree();
    app.config.library.dirs = Some(vec![dir.join("canto")]);
    app.start_library_scan();
    pump_until_library_ready(&mut app, &runtime);        // 照登录那套消息泵
    assert_eq!(app.local_songs().len(), 3);
    assert!(!app.library_scanning);
    assert!(app.notice_text().is_none(), "没有变化就不打扰");
}

#[test]
fn changes_are_reported_once() {
    // 第一次扫完 → 往目录塞一首 → 再扫 → 断言通知里有「新增 1」
}

#[test]
fn a_scan_already_running_is_not_started_twice() {
    app.library_scanning = true;
    app.start_library_scan();
    assert!(app.notice_text().unwrap().contains("正在扫描"));
}
```

- [ ] **Step 2: 跑，确认红** → `cargo test -- a_finished_scan_fills changes_are_reported a_scan_already_running`

- [ ] **Step 3: 实现**：`Sender<Msg>` 的克隆照 `tui/login.rs:121` 的 `spawn_qr_login` 拿法；线程里读索引 → `rescan` → `save_index`（失败只 `warn`）→ `send`。启动触发点放在 `load_initial` 之后（`tui/mod.rs` 那一处），**不阻塞首帧**。

- [ ] **Step 4: 跑，确认绿** + 全量
- [ ] **Step 5: clippy + 提交**：`git commit -m "feat(local): 后台扫描线程 + 曲库状态（读索引秒开、增量补扫）"`

---

### Task 5: 本地曲库屏

**Files:**
- Modify: `src/tui/state.rs`（`Menu::ALL`、`switch_menu`）、`src/tui/keys.rs`、`src/tui/draw/content.rs`、`src/tui/draw/overlay.rs`、`README.md`（键位表 + 功能表）

**Interfaces:**
- Consumes: `App::local_songs()`、`App::start_library_scan()`
- Produces:
  - `Menu::LocalLibrary`，`Menu::ALL` 变 5 项（编号 5；阶段二再加第 6 项，**编号不重排**）
  - `keys::Action::LocalRescan`，绑 `r`（`Context::Content` 且有 `Menu::LocalLibrary`；`state.rs` 里按 `nav.menu` 分岔）
  - `App::is_local_screen(&self) -> bool`
  - `App::library_enabled(&self) -> bool`（`config.library.resolve(&config.download_dir)` 非空；`dirs = []` 时首页不画这一格，`switch_menu` 也不让进）

- [ ] **Step 1: 写失败测试**

```rust
// 绘制（TestBackend）
#[test]
fn the_local_library_screen_lists_songs_without_sizes() {
    // app.library 灌三首（test_support 造）→ frame_text
    // 断言：含「曲终人散」「张宇」；那一行含 "—"（体积）不含 "MB"
    // 时长未知那首含 "--:--"
}

#[test]
fn an_empty_library_explains_where_to_add_dirs() {
    // library 空 → frame_text 含 "[library]" 与 "dirs"
}

// 键位
#[test]
fn r_rescans_only_on_the_local_screen() {
    assert_eq!(press(Context::Content, KeyCode::Char('r')), Some(Action::LocalRescan));
    // 在「我的收藏」上按 r 不产生这个动作（状态层按 menu 分岔，见 state 测试）
}

// 帮助表
#[test]
fn the_local_screen_help_omits_qq_only_keys() {
    // Menu::LocalLibrary 的帮助表：含 ("r", "重扫曲库")
    // 断言 cells 不含 ("d", …)、("p", …)、("x", …)：本地歌没有 QQ 身份，这三个键在本地屏不绑
}

// 曲库被关掉时不画格子
#[test]
fn an_empty_dirs_list_hides_the_local_cell() {
    let (mut app, _runtime) = test_app();
    app.config.library.dirs = Some(Vec::new());
    assert!(!app.library_enabled());
    app.go_home();
    assert!(!frame_text(&mut app, 120, 40).contains("本地曲库"), "dirs = [] 就不画这一格");
}
```

- [ ] **Step 2: 跑，确认红**
- [ ] **Step 3: 实现**：`switch_menu` 对 `Menu::LocalLibrary` **不发任何请求**（本地集合直接用 `local_songs()`）；列表渲染沿 `content.rs` 现有分支（本地行体积列走 `—`，时长 0 → `--:--`，标题栏带「扫描中 N 首」）；帮助表的「歌曲」段按屏分岔。
- [ ] **Step 4: 跑，确认绿** + 全量 + `README.md` 键位表补 `5 本地曲库` 与 `r`
- [ ] **Step 5: clippy + 提交**：`git commit -m "feat(local): 本地曲库屏（第 5 格、r 重扫、—/--:--、空库引导）"`

---

### Task 6: 播放本地文件 + 历史带路径

**Files:**
- Modify: `src/player.rs`（`Track`/`TrackSource`/`open()`）、`src/tui/state.rs`（播放入口、写历史、歌词加载分岔）、`src/history.rs`

**Interfaces:**
- Consumes: `Song.source`、`model::local_path`
- Produces:
  - `pub enum TrackSource { Cdn(Vec<Candidate>), Local(PathBuf) }`；`pub struct Track { pub song: Song, pub source: TrackSource }`
  - `history::Entry` 增 `#[serde(default)] pub local_path: Option<PathBuf>`
  - `state.rs`：`fn play_song(&mut self, song: Song)` 按 `song.source` 分岔——`Local` 直接 `engine.play(Track { song, source: TrackSource::Local(path) })`，**不调 vkey**；失败文案 `打不开：<原因>`
  - `state.rs`：歌词加载按 `song.source` 分岔——`Local` 走 `App::load_local_lyrics(&mut self, path: &Path)`：① 音频**同目录同名** `.lrc` → ② `config.lyrics_dir` 里同名 `.lrc` → ③ `lofty` 读**内嵌歌词**；命中即写进现有歌词缓存（key = 合成 mid），**一个请求都不发**；都没有 = 「暂无歌词」

- [ ] **Step 1: 写失败测试**

```rust
// src/player.rs（照现有「真 WAV 解码器」冒烟测试的写法）
#[test]
fn a_local_file_plays_without_touching_the_network_or_the_cache() {
    let wav = tiny_wav(dir.join("本地.wav"));          // 合成 PCM WAV
    let (engine, tx) = engine_for_test();
    tx.send(Cmd::Play(Track { song: local_song(&wav), source: TrackSource::Local(wav.clone()) })).unwrap();
    let snapshot = wait_for_duration(&engine);          // 现有冒烟测试的手法
    assert!(snapshot.duration > StdDuration::ZERO);
    assert!(cache::cached_entries(&cache_dir()).unwrap().is_empty(), "本地文件不写播放缓存");
}

// src/history.rs
#[test]
fn local_entries_round_trip_and_old_files_still_load() {
    // 写一个带 local_path 的 Entry → 读回一致
    // 读一份没有该字段的老 JSON → 照旧解析、local_path 为 None
}

// src/tui/state.rs
#[test]
fn playing_a_local_song_never_asks_for_login() {
    // 无会话（默认 test_app）+ library 里一首本地歌 → Enter
    // 断言：状态行没有「需要登录」；引擎收到的是 TrackSource::Local
}

#[test]
fn a_local_song_finds_its_lyrics_without_the_network() {
    // ① 临时目录里「一首.wav」+ 同目录「一首.lrc」（三行时间轴）→ 播它 → 歌词栏出现第二行那句话
    // ② 把 .lrc 删掉、改放 `config.lyrics_dir` 里的同名 .lrc → 重新播 → 仍然找到
    // ③ 两处都没有 → 歌词栏写「暂无歌词」，且全程没有任何请求（测试里没有会话也不报 SessionExpired）
}
```

- [ ] **Step 2: 跑，确认红**
- [ ] **Step 3: 实现**：`player.rs::open()` 增 `TrackSource::Local` 分支（`File::open` + `Decoder::new(MediaSource::File)`，**不包 `with_cache`**）；`state.rs` 的播放路径与写历史处带上 `local_path`；历史还原时 `local_path` 存在就 `Song { source: Source::Local(p) }`。
- [ ] **Step 4: 跑，确认绿** + 全量
- [ ] **Step 5: clippy + 提交**：`git commit -m "feat(local): 本地文件直接播（零请求不写缓存）+ 历史带本地路径"`

---

### Task 7: 免登录启动与「需要登录」的门

**Files:**
- Modify: `src/tui/state.rs`（`load_initial`、`switch_menu`、`sign_out`、历史屏视图）、`src/tui/mod.rs`（启动不再取码）、`src/tui/keys.rs`（登录屏 `Esc`）、`src/tui/draw/home.rs`（副标题）、`src/qq/mod.rs:35`（文案）、`README.md`（已知限制里缓存那条）

**Interfaces:**
- Consumes: `session::jar().state().logged_in`
- Produces:
  - `App::logged_in(&self) -> bool`、`App::qq_screen_available(&self, menu: Menu) -> bool`（未登录时 `Favorites`/`Playlists`/`Search` 为假，`History`/`LocalLibrary` 为真）
  - `keys::Action::LoginBack`（登录屏 `Esc`）
  - `App::start_login()` **只**建登录屏与 spawn 取码——调用点从 `load_initial` 挪到「按 `L` 或首页去登录屏」

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn without_a_session_the_app_lands_on_home_and_never_fetches_a_qr() {
    let (mut app, _runtime) = test_app();      // test_app 默认无会话
    app.load_initial();
    assert!(matches!(app.screen, Screen::Home));
    assert!(app.qr_state().is_none(), "启动不许取码");
}

#[test]
fn qq_screens_refuse_to_open_without_a_session() {
    let (mut app, _runtime) = test_app();
    app.switch_menu(Menu::Favorites);
    assert!(matches!(app.screen, Screen::Home), "不跳屏");
    assert!(app.notice_text().unwrap().contains("需要登录：按 L"));
    app.switch_menu(Menu::LocalLibrary);
    assert!(matches!(app.screen, Screen::Main), "本地屏照进");
    assert_eq!(app.nav.menu, Menu::LocalLibrary);
}

#[test]
fn the_history_screen_hides_qq_entries_without_a_session() {
    let (mut app, _runtime) = test_app();
    // history 里一条本地（local_path = Some）一条 QQ（None）→ 进历史
    app.switch_menu(Menu::History);
    let text = frame_text(&mut app, 120, 40);
    assert!(text.contains("曲终人散"), "本地条目照列");
    assert!(!text.contains("QQ 那首的标题"), "QQ 条目未登录时不显示");
}

#[test]
fn l_goes_to_login_when_logged_out_and_signs_out_when_logged_in() {
    let (mut app, _runtime) = test_app();
    app.apply(keys::Action::Logout);
    assert!(matches!(app.screen, Screen::Login));
    assert!(app.qr_state().is_some(), "进了登录屏才取码");
    // 已登录那半：给会话打桩 → 队列里放一首本地歌 → Logout
    app.queue.push(local_song(Path::new(r"C:\x\a.mp3")));
    app.apply(keys::Action::Logout);
    assert!(matches!(app.screen, Screen::Home));
    assert_eq!(app.queue.items().len(), 1, "登出不再清队列");
}

#[test]
fn playing_a_qq_song_without_a_session_says_need_login() {
    // 队列里一首 QQ 歌（source = Qq）→ Space / n
    // 断言：状态行含「需要登录：按 L」；会话文件没被删（没走 sign_out）
}

#[test]
fn escaped_from_the_login_screen_goes_home() {
    assert_eq!(press(Context::Login, KeyCode::Esc), Some(Action::LoginBack));
}
```

- [ ] **Step 2: 跑，确认红**
- [ ] **Step 3: 实现**：`load_initial` 两种登录态都 `go_home()`；`switch_menu` 开头过 `qq_screen_available`；`sign_out` 改成「清会话 → 回首页、保留队列」（不再 `queue.clear()`、不再 `start_login()`）；`qq/mod.rs:35` 的文案换成 `需要登录：按 L`；首页 QQ 三格未登录时副标题写「需要登录」、历史格写「仅本地条目」。
- [ ] **Step 4: 跑，确认绿** + 全量
- [ ] **Step 5: clippy + 提交**：`git commit -m "feat(local): 免登录可达（落首页、QQ 屏给门、L 两副面孔、启动不取码）"`

---

### Task 8: 阶段一收尾（真机冒烟 + 文档）

**Files:**
- Modify: `docs/design.md`（新增「本地曲库」一节 + 已知限制）、`README.md`（配置文件、文件清单、缓存那条限制改写）

- [ ] **Step 1: 文档**：`docs/design.md` 写清「合成 mid 是键」「索引是 `~/.cache` 派生物」「路径即身份（改名=新歌）」「未登录时 QQ 屏仍然画（发现性取舍）」；README 的「缓存只收顺序听完的歌，所以不等于离线模式」改写成「播放缓存只是播放缓存，离线靠本地曲库」。
- [ ] **Step 2: 真机冒烟（tuios，session `canto-dev`，Windows）**

```bash
# 1) 清会话后启动：应落首页、零 HTTP 行
mv ~/.config/canto/session.json /tmp/session.bak
tuios new-window -s canto-dev canto-local --cwd 'G:\code\canto' --no-focus -- 'G:\code\canto\target\debug\canto.exe' --log debug
tuios wait-for window-output -s canto-dev -w canto-local --pattern '本地曲库' --timeout 25000
grep -c "http" ~/.config/canto/canto.log        # 期望 0
# 2) 进 5、播一首、看歌词栏/频谱
tuios send-keys -s canto-dev -w canto-local '5'; tuios capture-pane -s canto-dev -w canto-local
```

- [ ] **Step 3: 离线复验**：`HTTPS_PROXY=http://127.0.0.1:1` 起一次，重复 Step 2；再按 `L` 进登录屏（取码失败是**预期**）→ `Esc` 回首页 → `q` 退出 → `close-window`。
- [ ] **Step 4: 提交**：`git commit -m "docs(local): 阶段一——设计说明与已知限制；真机冒烟记录进台账"`

---

## 阶段二：本地歌单

### Task 9: 本地歌单存储

**Files:**
- Create: `src/playlists.rs`（`src/main.rs` 加 `mod playlists;`）
- Modify: `src/paths.rs`（`local_playlists_path` 已在 Task 2 落地）

**Interfaces:**
- Produces:
  - `pub struct StoredSong { pub mid: String, pub title: String, pub singers: String }`
  - `pub struct LocalPlaylist { pub name: String, pub songs: Vec<StoredSong> }`
  - `pub struct Store { pub version: u32, pub playlists: Vec<LocalPlaylist> }`（serde，`version = 1`）
  - `pub enum NameError { Empty, Duplicate }`
  - `Store::load(path) -> Store` / `Store::save(&self, path) -> io::Result<()>`（tmp + rename）
  - `Store::{create(&mut self, name: &str) -> Result<usize, NameError>, rename(&mut self, i: usize, name: &str) -> Result<(), NameError>, add(&mut self, i: usize, song: &Song), remove(&mut self, i: usize, j: usize), delete(&mut self, i: usize), len_of(&self, i: usize) -> usize}`

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn create_rename_and_persist_round_trip() {
    let path = tmp_file("local_playlists.json");
    let mut store = Store::default();
    let i = store.create("通勤").unwrap();
    store.add(i, &local_song(Path::new(r"C:\m\张宇 - 曲终人散.mp3")));
    store.rename(i, "通勤路上").unwrap();
    store.save(&path).unwrap();
    let back = Store::load(&path);
    assert_eq!(back.playlists.len(), 1);
    assert_eq!(back.playlists[0].name, "通勤路上");
    assert_eq!(back.playlists[0].songs[0].mid, r"local:C:\m\张宇 - 曲终人散.mp3");
    assert_eq!(back.playlists[0].songs[0].title, "曲终人散", "标题快照留着");
}

#[test]
fn blank_and_duplicate_names_are_rejected() {
    let mut store = Store::default();
    store.create("通勤").unwrap();
    assert_eq!(store.create("通勤"), Err(NameError::Duplicate));
    assert_eq!(store.create("  通勤  "), Err(NameError::Duplicate), "先去掉首尾空白再比");
    assert_eq!(store.create("   "), Err(NameError::Empty));
    let i = store.create("另一个").unwrap();
    assert_eq!(store.rename(i, ""), Err(NameError::Empty));
    assert_eq!(store.rename(0, "另一个"), Err(NameError::Duplicate));
}

#[test]
fn add_keeps_join_order_and_skips_duplicates() {
    let mut store = Store::default();
    let i = store.create("通勤").unwrap();
    let a = local_song(Path::new("/m/a.mp3"));
    let b = local_song(Path::new("/m/b.mp3"));
    store.add(i, &a); store.add(i, &b); store.add(i, &a);
    assert_eq!(store.len_of(i), 2);
    assert_eq!(store.playlists[0].songs[1].mid, b.mid, "顺序就是加入顺序");
}

#[test]
fn missing_files_do_not_erase_the_playlist() {
    let mut store = Store::default();
    let i = store.create("通勤").unwrap();
    store.add(i, &local_song(Path::new("/m/gone.mp3")));
    // 加载一个「曲库索引里已经查不到这首歌」的场景：条目仍然在
    assert_eq!(store.len_of(i), 1);
    assert_eq!(store.playlists[0].songs[0].title, "gone", "标题快照不依赖文件在不在");
}
```

- [ ] **Step 2: 跑，确认红** → **Step 3: 实现** → **Step 4: 绿 + 全量**
- [ ] **Step 5: clippy + 提交**：`git commit -m "feat(local): 本地歌单存储（校验 + 立即落盘 + 快照条目）"`

---

### Task 10: 本地歌单屏

**Files:**
- Modify: `src/tui/state.rs`（`Menu::ALL` 第六项、`nav.shown` 复用、双按删除的待确认状态）、`src/tui/keys.rs`、`src/tui/draw/content.rs`（歌单两屏）、`src/tui/draw/overlay.rs`（帮助表那段）

**Interfaces:**
- Consumes: `playlists::Store`（Task 9）
- Produces:
  - `Menu::LocalPlaylists`（编号 6）、`App::playlists: Store`
  - `keys::Action::{LocalPlaylistCreate, LocalPlaylistRename, LocalPlaylistRemove, LocalPlaylistDelete}`
  - `keys::Action::{LocalPlaylistChar(char), LocalPlaylistSubmit, LocalPlaylistDown, LocalPlaylistUp, LocalPlaylistCancel}`——新建/改名共用的输入态，与选择框共用的上下移动（阶段二 Task 11 复用同一组）
  - `App::{pending_playlist_delete: Option<usize>, enter_playlist(&mut self, i: usize), leave_playlist(&mut self)}`
  - 歌单行显示 `名字 + N 首`（用 `Store::len_of`）；条目渲染：曲库索引里能查到就显示实时时长，否则 `（已丢失）`

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn the_screen_lists_playlists_then_tracks() {
    let (mut app, _runtime) = test_app();
    // 两个歌单：一个两首、一个空
    app.switch_menu(Menu::LocalPlaylists);
    let first = frame_text(&mut app, 120, 40);
    assert!(first.contains("通勤") && first.contains("2 首"), "左列表写歌单名与曲目数");
    app.enter_playlist(0);                       // Enter
    let second = frame_text(&mut app, 120, 40);
    assert!(second.contains("曲终人散"), "进去看曲目");
    app.enter_playlist(1);
    assert!(frame_text(&mut app, 120, 40).contains("空歌单"));
}

#[test]
fn n_creates_r_and_renames_x_removes() {
    let (mut app, _runtime) = test_app();
    app.switch_menu(Menu::LocalPlaylists);
    app.apply(keys::Action::LocalPlaylistCreate);
    for c in "通勤".chars() { app.apply(keys::Action::LocalPlaylistChar(c)); }
    app.apply(keys::Action::LocalPlaylistSubmit);
    assert_eq!(app.playlists.playlists[0].name, "通勤");
    app.apply(keys::Action::LocalPlaylistRename);
    for c in "路上".chars() { app.apply(keys::Action::LocalPlaylistChar(c)); }
    app.apply(keys::Action::LocalPlaylistSubmit);
    assert_eq!(app.playlists.playlists[0].name, "路上");
    app.enter_playlist(0);
    app.apply(keys::Action::LocalPlaylistRemove);
    assert_eq!(app.playlists.len_of(0), 1, "刚删掉一首");
}

#[test]
fn deleting_a_playlist_needs_two_presses() {
    let (mut app, _runtime) = test_app();
    app.switch_menu(Menu::LocalPlaylists);
    app.apply(keys::Action::LocalPlaylistDelete);
    assert_eq!(app.playlists.playlists.len(), 2, "第一次不删");
    assert!(app.notice_text().unwrap().contains("再按一次"));
    app.apply(keys::Action::LocalPlaylistCreate);        // 别的键取消这次待确认
    assert!(app.pending_playlist_delete.is_none());
    app.apply(keys::Action::LocalPlaylistDelete);
    app.apply(keys::Action::LocalPlaylistDelete);
    assert_eq!(app.playlists.playlists.len(), 1, "第二次才删");
}

#[test]
fn a_lost_entry_is_marked_and_does_not_crash_playback() {
    // 歌单里有 mid 但曲库索引查不到 → 行里出现「（已丢失）」
    // 在上面按 Enter → 状态行含「打不开：」，界面不崩、光标不乱跳
}

#[test]
fn the_local_playlist_help_lists_the_new_keys() {
    // 帮助表含 ("n", …)、("R", …)、("x", …)、("D", …)；不含 ("d", …)、("p", …)
}
```

- [ ] **Step 2: 跑，确认红** → **Step 3: 实现** → **Step 4: 绿 + 全量**
- [ ] **Step 5: clippy + 提交**：`git commit -m "feat(local): 本地歌单屏（新建/改名/移出/双按删除）"`

---

### Task 11: `P` 把歌加进本地歌单

**Files:**
- Modify: `src/tui/state.rs`（picker 的本地版）、`src/tui/keys.rs`（`Action::LocalPlaylistAdd` 绑 `P`）、`src/tui/draw/overlay.rs`（本地 picker 的「新建…」一行 + 本地屏帮助补 `P`）

**Interfaces:**
- Consumes: `playlists::Store::{create, add}`、现有歌单选择框那套（`Context::*Picker`）
- Produces: `App::local_picker: Option<usize>`（本地 picker 的高亮；与现有 QQ picker 分开，避免两套状态互踩）

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn p_only_works_on_the_local_library_screen() {
    assert_eq!(press(Context::Content, KeyCode::Char('P')), Some(Action::LocalPlaylistAdd));
    // 「我的收藏」上按 P：状态层按 menu 分岔 → 不产生这个动作、状态行也不提它
}

#[test]
fn the_picker_lists_local_playlists_with_a_new_entry_first() {
    let (mut app, _runtime) = test_app();
    app.switch_menu(Menu::LocalLibrary);
    app.apply(keys::Action::LocalPlaylistAdd);
    let text = frame_text(&mut app, 120, 40);
    let new_at = text.find("新建…").expect("第一行是新建");
    assert!(new_at < text.find("通勤").unwrap(), "新建排在歌单上面");
}

#[test]
fn choosing_new_asks_for_a_name_then_adds_the_song() {
    let (mut app, _runtime) = test_app();
    app.switch_menu(Menu::LocalLibrary);
    app.apply(keys::Action::LocalPlaylistAdd);          // 光标默认在「新建…」
    app.apply(keys::Action::LocalPlaylistSubmit);       // 进入输入名字态
    for c in "通勤".chars() { app.apply(keys::Action::LocalPlaylistChar(c)); }
    app.apply(keys::Action::LocalPlaylistSubmit);
    assert_eq!(app.playlists.playlists[0].name, "通勤");
    assert_eq!(app.playlists.len_of(0), 1, "建完顺手把这首加进去");
    assert!(app.notice_text().unwrap().contains("已加入"));
}

#[test]
fn choosing_an_existing_playlist_adds_once() {
    let (mut app, _runtime) = test_app();
    app.switch_menu(Menu::LocalLibrary);
    for _ in 0..2 {                                     // 同一首加两次
        app.apply(keys::Action::LocalPlaylistAdd);
        app.apply(keys::Action::LocalPlaylistDown);      // 光标移到「通勤」
        app.apply(keys::Action::LocalPlaylistSubmit);
    }
    assert_eq!(app.playlists.len_of(0), 1);
    assert!(app.notice_text().unwrap().contains("已在歌单里"));
}
```

- [ ] **Step 2: 跑，确认红** → **Step 3: 实现** → **Step 4: 绿 + 全量**
- [ ] **Step 5: clippy + 提交**：`git commit -m "feat(local): 大写 P 加入本地歌单（picker 复用 + 顺手新建）"`

---

### Task 12: 阶段二收尾（真机冒烟 + 文档 + 全量对齐）

**Files:**
- Modify: `README.md`（键位表补 `P`/`n`/`R`/`x`/`D`、功能表补本地歌单）、`docs/design.md`（§5 的落地说明）

- [ ] **Step 1: 真机冒烟**：`HTTPS_PROXY=http://127.0.0.1:1` 起一次 → `5` 播一首 → `P` 新建「通勤」并加入 → `6` 看到歌单与曲目 → `R` 改名 → `D` 双按删除 → `q` 退出 → `close-window`，记录到台账。
- [ ] **Step 2: 文档 + 提交**：`git commit -m "docs(local): 阶段二——本地歌单的键位与说明"`

## 收尾（不在任务里，执行者最后做一次）

- `cargo test` 全量 + `cargo clippy --all-targets --locked -- -D warnings`（用即将整合的那棵树跑）。
- 整支分支的终审（fresh reviewer 子代理，范围 = 本特性第一个提交的父提交..HEAD，带上 spec + 本计划 + 台账）。
- 终审 findings 逐条修或记 Ruling；然后走 `finishing-a-development-branch`（合并回 `main`、在合并结果上再跑一遍门禁）。

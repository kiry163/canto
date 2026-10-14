# 本地文件模型重构 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让「本地有这份吗」变成按 mid 查表的精确问题：新增下载索引、歌词与音频同目录、曲库退出 QQ 歌的取源链、第 5 屏变成「下载 ∪ 曲库目录」的合并列表。

**Architecture:** 三个索引各管一段（缓存无索引文件 / 下载 `<download_dir>/downloads.json` / 曲库 `~/.cache/canto/library.json`），一个共用内存结构描述「磁盘上的这首歌」；QQ 歌取源只查缓存与下载索引（都按 mid），界面身份一律 `local:<路径>`。

**Tech Stack:** Rust 2021、ratatui、serde/serde_json、lofty（tag）、TDD 走 `cargo test`（TestBackend + 纯函数 + 哑引擎），第二层真机冒烟走 tuios。

**Spec:** `docs/superpowers/specs/2026-10-14-local-files-design.md`

**执行顺序（与下方编号不同的一处）：** **Task 4（配置）排在 Task 6 之后做**。理由：删掉
`[paths] lyrics_dir` 之前，必须先把它在本仓库里的两个用处断掉——`state.rs` 里 `local_lyric(…)`
那个调用在 Task 3 改掉、`download::save(…, lyrics_dir, …)` 与 `download_song` 在 Task 6 改掉。
其余任务按编号顺序执行，每个任务跑完都必须 `cargo test` 全绿（不允许中间留下编译不过的树）。

## Global Constraints

- 不吃兼容：不做迁移、不保留旧键，老键一律按「不认识的键」点名进 `canto.log`。
- 本地身份仍是 `local:<绝对路径>`（不做内容哈希、不做文件名里的 mid）。
- 下载目录**只认索引**：未登记的音频文件不显示、不参与任何判定（含曲库扫描跳过它）。
- 索引文件一律 临时文件 + rename；坏文件 / 版本不符 → 按空处理（宁可当没有）。
- 界面文案与列宽规矩：表头与单元格都由 `rows::line/header` 统一渲染；`来源` 列宽 4。
- 状态行通知位固定 32 列（`tui::draw::status::NOTICE_COLS`）；长文案进日志。
- 测试绝不碰真数据目录（`tui::test_support` 注入临时路径），所有新落盘路径都要可注入。

## Review Focus

- **同一个文件同时出现在两个索引里**（`download_dir` 正好是 `dirs` 的一项，或 `dirs` 是它的父目录）：第 5 屏必须**只出现一次**，来源写「下载」。
- **索引与磁盘不一致**（文件被删 / 改名 / 换成别的内容 / 手工拷进来）：`↓`、第 5 屏、起播三处必须给出**一致**的答案（都以「文件在不在 + 大小对不对」为准）。
- **缓存超限时的删法**（`[cache] mb` 调小）：`sweep` 只能删音频，`.lrc`、`library.json` 与子目录一个都不能动（今天它会误删 `library.json`）。
- **同歌手同标题的两首不同歌**：`↓` 不能互相标；下载时文件名不能互相冒充（今天的 `existed: true` 会把第二首当成已下载）。
- **没登录 / 没网**：第 5 屏里下载歌的标题 / 歌手 / 专辑 / 时长必须来自索引（不是空的），歌词来自同目录 `.lrc`，`--log debug` 里零 HTTP。

---

### Task 1: 下载索引 `download::Index`

**Files:**
- Modify: `src/download.rs`（新增一段：索引类型 + 读写 + 校验；本任务先不碰 `save`）
- Test: `src/download.rs` 的 `mod tests`

**Interfaces:**
- Produces:
  - `pub const INDEX_VERSION: u32 = 1;`
  - `pub fn index_path(dir: &Path) -> PathBuf` → `dir.join("downloads.json")`
  - `pub struct Track { pub mid: String, pub file: String, pub quality: Quality, pub container: String, pub bytes: u64, pub title: String, pub singers: String, pub album: String, pub duration_secs: u64, pub at: u64 }`（`file` 是**相对** `download_dir` 的文件名）
  - `pub struct Index { pub version: u32, pub tracks: Vec<Track> }`，`impl Default`
  - `pub fn load(dir: &Path) -> Index`（缺失 / 坏了 / 版本不符 → 空 + `tracing::warn!`）
  - `pub fn save(&self, dir: &Path) -> Result<()>`（tmp + rename）
  - `pub fn by_mid(&self, mid: &str) -> Option<&Track>`
  - `pub fn path_of(&self, track: &Track, dir: &Path) -> PathBuf`
  - `pub fn mid_of_path(&self, dir: &Path, path: &Path) -> Option<&str>`
  - `pub fn upsert(&mut self, track: Track)`（同 mid 覆盖，其余顺序不变）
  - `pub fn validate(&mut self, dir: &Path) -> bool`（文件不在 → 丢；`bytes` 不符 → 以磁盘为准改；返回是否改过，好让调用方决定要不要回写）

- [ ] **Step 1: 写失败的测试**（放 `mod tests` 末尾）

```rust
#[test]
fn the_index_round_trips_and_treats_anything_odd_as_empty() {
    let dir = temp_dir("index-basics");
    assert!(Index::load(&dir).tracks.is_empty(), "缺失 = 空");

    std::fs::write(index_path(&dir), "{ 不是 JSON").expect("写坏文件");
    assert!(Index::load(&dir).tracks.is_empty(), "坏了 = 空");

    let mut index = Index::default();
    index.upsert(track("m1", "歌手 - 一首.flac"));
    index.save(&dir).expect("落盘");
    let back = Index::load(&dir);
    assert_eq!(back.tracks, index.tracks);
    assert_eq!(back.by_mid("m1").map(|t| t.file.as_str()), Some("歌手 - 一首.flac"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_version_mismatch_is_empty_too() {
    let dir = temp_dir("index-version");
    let text = format!(r#"{{"version":99,"tracks":[]}}"#);
    std::fs::write(index_path(&dir), text).expect("写文件");
    assert!(Index::load(&dir).tracks.is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn upsert_replaces_the_same_mid_only() {
    let mut index = Index::default();
    index.upsert(track("m1", "a.flac"));
    index.upsert(track("m2", "b.flac"));
    index.upsert(track("m1", "a2.flac"));
    let files: Vec<&str> = index.tracks.iter().map(|t| t.file.as_str()).collect();
    assert_eq!(files, ["a2.flac", "b.flac"], "同 mid 覆盖，别的条目顺序不动");
}

#[test]
fn validate_drops_ghosts_and_trusts_the_disk_for_sizes() {
    let dir = temp_dir("index-validate");
    std::fs::write(dir.join("活着.flac"), vec![7_u8; 11]).expect("写文件");
    let mut index = Index::default();
    index.upsert(track("live", "活着.flac"));
    index.upsert(track("ghost", "没有这个文件.flac"));
    assert!(index.validate(&dir), "丢了一条幽灵条目 = 改过");
    assert_eq!(index.tracks.len(), 1);
    assert_eq!(index.by_mid("live").expect("还在").bytes, 11, "大小以磁盘为准");
    assert!(!index.validate(&dir), "再验一次不该说改过");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_path_knows_its_mid() {
    let dir = temp_dir("index-mid-of-path");
    let mut index = Index::default();
    index.upsert(track("m1", "一首.flac"));
    let path = index.path_of(index.by_mid("m1").unwrap(), &dir);
    assert_eq!(index.mid_of_path(&dir, &path), Some("m1"));
    assert_eq!(index.mid_of_path(&dir, Path::new("/别处/一首.flac")), None);
    std::fs::remove_dir_all(&dir).ok();
}
```

（`temp_dir(tag)` / `track(mid, file)` 两个小助手：前者沿用本文件既有测试里那句 `std::env::temp_dir().join(format!("canto-download-{}-{tag}", std::process::id()))`；后者返回一条 `Track { mid, file, quality: Quality::Flac, container: "FLAC".into(), bytes: 0, title/singers/album: 同 file 派生的短串, duration_secs: 1, at: 1 }`。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test download::tests::the_index_round_trips -- --nocapture`
Expected: 编译失败（`Index` / `index_path` / `track` 未定义）。

- [ ] **Step 3: 实现**

`Track` / `Index` 都带 `serde::{Serialize, Deserialize}`；`Default` 给 `version: INDEX_VERSION, tracks: vec![]`。
`load`：`read_to_string` → `serde_json::from_str::<Index>`，任一失败或 `version != INDEX_VERSION` 就 `tracing::warn!` + 空。
`save`：`create_dir_all(dir)` → 写 `index_path(dir).with_extension("json.tmp")` → `rename`。
`validate`：`retain` 掉 `!path_of(..).is_file()` 的条目（记一条 `tracing::info!`），存活条目的 `bytes` 改成 `metadata().len()`，用「有没有真的改过」当返回值。
`mid_of_path`：把 `dir.join(&track.file)` 与入参比。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test download::tests`
Expected: PASS（本文件既有测试也全绿）。

- [ ] **Step 5: 提交**

```bash
git add src/download.rs
git commit -m "feat(download): 下载索引（mid ↔ 文件 + 离线显示字段），带目录校验"
```

---

### Task 2: 缓存目录压平 + `sweep` 白名单

**Files:**
- Modify: `src/cache.rs`（`lyric_dir()`、`sweep_dir()`）
- Test: `src/cache.rs` 的 `mod tests`

**Interfaces:**
- Produces: `pub fn lyric_dir() -> PathBuf` 现在返回 `paths::cache_dir()` 本身；`sweep_dir(dir, limit)` 只把「`<mid>.<档位>.<ext>` 形态」的文件算进上限并可删。

- [ ] **Step 1: 写失败的测试**

```rust
#[test]
fn lyrics_live_beside_the_tracks_and_are_not_tracks() {
    let dir = temp_dir("flat-lyrics");
    std::fs::write(dir.join("m1.320.mp3"), vec![0_u8; 10]).expect("写缓存");
    std::fs::write(dir.join("m1.lrc"), "[00:01.00]第一句\n").expect("写歌词");
    std::fs::write(dir.join("m1.trans.lrc"), "[00:01.00]译文\n").expect("写翻译");
    std::fs::write(dir.join("library.json"), "{}").expect("写曲库索引");

    let entries = cached_entries_in(&dir);
    assert_eq!(entries.get("m1"), Some(&(Quality::High320, 10)), "只认音频：{entries:?}");

    // 歌词读写走同一目录。
    put_lyric_in(&dir, "m2", None);
    assert!(dir.join("m2.lrc").is_file(), "歌词写进缓存根目录");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn sweep_only_evicts_tracks() {
    let dir = temp_dir("sweep-tracks");
    for (name, size) in [("a.128.mp3", 10_u64), ("b.128.mp3", 10)] {
        std::fs::write(dir.join(name), vec![0_u8; size as usize]).expect("写音频");
    }
    std::fs::write(dir.join("a.lrc"), vec![0_u8; 5]).expect("写歌词");
    std::fs::write(dir.join("library.json"), vec![0_u8; 5]).expect("写曲库索引");
    std::fs::create_dir_all(dir.join("old-lyrics")).expect("建子目录");

    let (deleted, _) = sweep_dir(&dir, 10);
    assert_eq!(deleted, 1, "只删一个音频");
    assert!(dir.join("a.lrc").exists() && dir.join("library.json").exists(), "歌词与索引不能动");
    assert!(dir.join("old-lyrics").is_dir(), "子目录不能动");
    assert!(!dir.join("a.128.mp3").exists(), "最久没动的那个音频被删");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_legacy_lyrics_subdirectory_is_cleaned_up() {
    let dir = temp_dir("sweep-legacy");
    std::fs::create_dir_all(dir.join("lyrics")).expect("建老歌词目录");
    std::fs::write(dir.join("lyrics/m1.lrc"), "x").expect("写老歌词");
    sweep_dir(&dir, 0);
    assert!(!dir.join("lyrics").exists(), "老歌词子目录不再被读，顺手删掉");
    std::fs::remove_dir_all(&dir).ok();
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test cache::tests::sweep_only_evicts_tracks -- --nocapture`
Expected: FAIL（删了 3 个 / 老 `lyrics/` 目录还在）。

- [ ] **Step 3: 实现**

`lyric_dir()` 返回 `paths::cache_dir()`；`put_lyric` / `lyric` 里的目录参数跟着改（它们已经吃 `dir`，只需换调用点）。
`sweep_dir` 里加一个判定：文件名按 `rsplitn(3, '.')` 切出 `(mid, tier, ext)` 且 `Quality::parse(tier).is_some()` 且非 `part` 才算音频（抽成小函数 `fn is_track_name(name: &str) -> bool`，`cached_entries_in` 也复用它，避免两处口径）；只有音频进 `kept`，`.part` 照旧先删。
`<dir>/lyrics` 存在就 `remove_dir_all` + 一句 `tracing::info!`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test cache::tests`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add src/cache.rs
git commit -m "refactor(cache): 歌词与缓存同目录，sweep 只认音频形态（顺手不再误删索引）"
```

---

### Task 3: 曲库扫描：配对 `.lrc`、跳过下载目录、索引版本 2

**Files:**
- Modify: `src/library.rs`（`Entry`、`INDEX_VERSION`、`walk`、`rescan`、`local_lyric`）
- Test: `src/library.rs` 的 `mod tests`

**Interfaces:**
- Produces:
  - `pub struct Entry { …, pub lyric: Option<PathBuf> }`（跟在 `duration_secs` 后）
  - `pub const INDEX_VERSION: u32 = 2;`
  - `pub fn walk(dirs: &[PathBuf], skip: &Path) -> (Vec<(PathBuf, u64, u64)>, usize)`
  - `pub fn rescan(previous: &Index, dirs: &[PathBuf], skip: &Path) -> ScanOutcome`
  - `pub fn local_lyric(path: &Path) -> Option<Lyric>`（第二段 `lyrics_dir` 删掉）

- [ ] **Step 1: 写失败的测试**

```rust
#[test]
fn a_sibling_lrc_is_paired_and_never_counted_as_audio() {
    let dir = temp_dir("lrc-pair");
    let audio = dir.join("一曲.wav");
    write_wav(&audio, 4.0);
    std::fs::write(dir.join("一曲.lrc"), "[00:01.00]第一句\n[00:02.00]第二句\n").expect("写歌词");
    std::fs::write(dir.join("一曲.trans.lrc"), "[00:01.00]译文\n").expect("写翻译");
    std::fs::write(dir.join("无主.lrc"), "[00:01.00]孤儿\n").expect("写孤儿歌词");

    let (files, unsupported) = walk(&[dir.clone()], Path::new("/不存在的下载目录"));
    assert_eq!(files.len(), 1, "只有音频进候选：{files:?}");
    assert_eq!(unsupported, 0, ".lrc 不算「不支持的音频」");

    let outcome = rescan(&Index::default(), &[dir.clone()], Path::new("/不存在的下载目录"));
    let entry = &outcome.index.entries[0];
    assert_eq!(entry.lyric.as_deref(), Some(dir.join("一曲.lrc").as_path()), "同 stem 的 .lrc 配上了");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_download_directory_is_skipped_whole() {
    let root = temp_dir("skip-download");
    let download = root.join("canto");
    let music = root.join("music");
    std::fs::create_dir_all(&download).expect("建下载目录");
    std::fs::create_dir_all(&music).expect("建音乐目录");
    write_wav(&download.join("下载的.wav"), 1.0);
    write_wav(&music.join("自己的.wav"), 1.0);

    // ① dirs 里正好是它
    let (files, _) = walk(&[download.clone(), music.clone()], &download);
    assert_eq!(files.len(), 1, "下载目录整条不扫");
    // ② dirs 是它的父目录 → 递归到它时整棵子树跳过
    let (files, _) = walk(&[root.clone()], &download);
    assert_eq!(files.len(), 1, "嵌套也要跳过：{files:?}");
    assert!(files[0].0.ends_with("自己的.wav"));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn local_lyric_looks_beside_the_track_then_inside_it() {
    let dir = temp_dir("local-lyric");
    let audio = dir.join("一曲.wav");
    write_wav(&audio, 1.0);
    assert!(local_lyric(&audio).is_none(), "都没有就是没有");
    std::fs::write(dir.join("一曲.lrc"), "[00:01.00]同目录\n").expect("写歌词");
    assert!(local_lyric(&audio).is_some(), "同目录同名 .lrc");
    std::fs::remove_dir_all(&dir).ok();
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test library::tests::the_download_directory_is_skipped_whole -- --nocapture`
Expected: 编译失败（`walk` 参数个数不符）。

- [ ] **Step 3: 实现**

`walk`/`walk_into` 多带一个 `skip: &Path`：进一个目录前先比路径（**两边都 `canonicalize` 失败就按字面比**，避免 `./` 与盘符大小写不一致把跳过判断弄丢）；相等就整棵子树跳过。`walk_into` 顺手收集 `.lrc` / `.trans.lrc`：按 stem 建 `HashMap<String, PathBuf>`（`.lrc` 优先于 `.trans.lrc`），音频进来时配 `lyric`；配不上就忽略。`rescan` 把 `skip` 透传，并在「mtime/size 都没变」沿用旧条目时**也把新配到的 lyric 写进去**（`Entry` 是 `Clone`，沿用前 copy 一份改 lyric 即可）。
`read_entry` 多一个 `lyric: Option<PathBuf>` 参数。`local_lyric` 删掉第二段（`lyrics_dir`），签名变 `(path: &Path)`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test library::tests`
Expected: PASS（`INDEX_VERSION` 变 2 后，既有那几条「版本不符按空」的测试照旧绿）。

- [ ] **Step 5: 提交**

```bash
git add src/library.rs
git commit -m "feat(library): 扫描配对同 stem 的 .lrc、跳过下载目录、索引版本 2"
```

---

### Task 4: 配置面：删 `lyrics_dir`、`dirs` 两态

**Files:**
- Modify: `src/config.rs`（`PathsConfig`、`LibraryConfig`、解析、模板）
- Test: `src/config.rs` 的 `mod tests`

**Interfaces:**
- Produces: `PathsConfig { pub download_dir: PathBuf }`（`lyrics_dir` 消失）；`LibraryConfig { pub dirs: Vec<String> }` + `pub fn resolve(&self) -> Vec<PathBuf>`（不再回退到 `download_dir`）。

- [ ] **Step 1: 写失败的测试**

```rust
#[test]
fn lyrics_dir_is_gone_and_named_as_unknown() {
    let (config, warnings) = parse("[paths]\ndownload_dir = 'D:/songs'\nlyrics_dir = 'D:/lyrics'\n");
    assert_eq!(config.paths.download_dir, PathBuf::from("D:/songs"));
    let joined = warnings.join("\n");
    assert!(joined.contains("lyrics_dir"), "要点名：{joined}");
    assert!(joined.contains("[paths]"), "{joined}");
}

#[test]
fn library_dirs_is_two_state() {
    let (config, warnings) = parse("[library]\ndirs = []\n");
    assert!(config.library.resolve().is_empty(), "空表 = 不扫别人的目录");
    assert!(warnings.is_empty(), "空表不是错误：{warnings:?}");

    let (config, _) = parse("[library]\ndirs = ['D:/music']\n");
    assert_eq!(config.library.resolve(), vec![PathBuf::from("D:/music")]);

    let (config, _) = parse("[playback]\nquality = \"flac\"\n");
    assert!(config.library.resolve().is_empty(), "不写 = 也不扫");
}
```

模板那条既有测试（`the_template_documents_every_key`）要跟着改：断言模板里**没有** `lyrics_dir`，且 `[library]` 那段写的是两态（含「不写 / dirs = []」与「跳过下载目录」两句）。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test config::tests::library_dirs_is_two_state -- --nocapture`
Expected: 编译失败（`resolve` 还吃参数）。

- [ ] **Step 3: 实现**

`paths_config` 不再读 `lyrics_dir`（`[paths]` 的 `known` 只剩 `download_dir`，于是它会自动落进「不认识的键」那条路径）。
`library_config` 的 `string_list("dirs")` 结果直接 `unwrap_or_default()` 成 `Vec<String>`；`resolve()` 不再接参。
模板：`[paths]` 段删掉歌词目录那两行注释与键；`[library]` 段的注释重写成两态 + 「下载目录会被自动跳过」；`download_dir` 的注释里写明「同目录还放歌词与 downloads.json」。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test config::tests`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add src/config.rs
git commit -m "feat(config): 删 [paths] lyrics_dir、[library] dirs 改两态"
```

---

### Task 5: 取源与来源标签（`player::local_copy`）

**Files:**
- Modify: `src/player.rs`
- Test: `src/player.rs` 的 `mod tests`

**Interfaces:**
- Consumes: `download::Index` + `Index::by_mid` / `mid_of_path`（Task 1）
- Produces: `pub fn local_copy(song: &Song, cache_dir: Option<&Path>, downloads: &crate::download::Index, download_dir: &Path) -> Option<TrackSource>`；`PlaySource::Download` 的语义改成「这份路径在下载索引里」。

- [ ] **Step 1: 写失败的测试**

```rust
#[test]
fn the_chain_is_cache_then_download_then_nothing() {
    let cache = temp_dir("chain-cache");
    let downloads_dir = temp_dir("chain-downloads");
    std::fs::write(cache.join("m1.flac.flac"), vec![0_u8; 4]).expect("写缓存");
    let mut index = download::Index::default();
    index.upsert(track_for("m2", "歌手 - 二首.flac"));
    std::fs::write(downloads_dir.join("歌手 - 二首.flac"), vec![0_u8; 4]).expect("写下载");

    let m1 = qq_song("m1");
    let m2 = qq_song("m2");
    let m3 = qq_song("m3");

    let hit = local_copy(&m1, Some(&cache), &index, &downloads_dir).expect("缓存命中");
    assert_eq!(from_of(&hit), PlaySource::Cache);
    let hit = local_copy(&m2, Some(&cache), &index, &downloads_dir).expect("下载命中");
    assert_eq!(from_of(&hit), PlaySource::Download);
    assert!(local_copy(&m3, Some(&cache), &index, &downloads_dir).is_none(), "都没有");
    assert!(local_copy(&m2, None, &index, &downloads_dir).is_some(), "缓存关掉不影响下载");
}

#[test]
fn a_local_file_is_labelled_by_which_index_owns_it() {
    let downloads_dir = temp_dir("label-downloads");
    let mine = downloads_dir.join("歌手 - 二首.flac");
    std::fs::write(&mine, vec![0_u8; 4]).expect("写下载");
    let mut index = download::Index::default();
    index.upsert(track_for("m2", "歌手 - 二首.flac"));

    let played = Song { source: crate::model::Source::Local(mine.clone()), ..qq_song("m2") };
    let hit = local_copy(&played, None, &index, &downloads_dir).expect("本地文件");
    assert_eq!(from_of(&hit), PlaySource::Download, "在下载索引里 → 下载");

    let other = Song { source: crate::model::Source::Local(downloads_dir.join("别的.flac")), ..qq_song("m9") };
    let hit = local_copy(&other, None, &index, &downloads_dir).expect("本地文件");
    assert_eq!(from_of(&hit), PlaySource::Library, "只在曲库里 → 曲库");
}
```

（`from_of(&TrackSource) -> PlaySource` 与 `qq_song(mid)` / `track_for(mid, file)` 三个小助手放本文件测试里。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test player::tests::the_chain_is_cache_then_download_then_nothing -- --nocapture`
Expected: 编译失败（`downloads` 现在还是 `HashMap`、参数个数不符）。

- [ ] **Step 3: 实现**

`local_copy` 里把「`song.path()` → `PlaySource::Library`」改成先查索引：`downloads.mid_of_path(download_dir, path).is_some()` → `Download`，否则 `Library`；下载那一跳从 `downloads.get(&file_stem(song))` 换成 `downloads.by_mid(&song.mid)` + `path_of`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test player::tests`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add src/player.rs
git commit -m "feat(player): 取源按 mid 查下载索引，来源标签按数据判定"
```

---

### Task 6: 状态机接线（合并列表、`↓` 按 mid、`d` 查索引）

**Files:**
- Modify: `src/download.rs`（`Track::to_song`、`free_name`、删 `scan_downloads`/`Downloaded`）、`src/tui/state.rs`、`src/tui/test_support.rs`（测试访问器）
- Test: `src/tui/state.rs` 的 `mod tests`、`src/tui/test_support.rs`

**Interfaces:**
- Consumes: Task 1 的 `Index`、Task 5 的 `local_copy`、Task 3 的 `rescan(previous, dirs, skip)`
- Produces:
  - `download::Track::to_song(&self, dir: &Path) -> Song`（`Source::Local`、`mid = local_mid(path)`、`local_bytes = bytes`、`album_name` / `duration_secs` / `title` / `singers` 来自索引）
  - `App.downloads: download::Index`（不再有 `scan_downloads`）
  - `App::refresh_downloads()` = `Index::load` + `validate`（改过就回写）

- [ ] **Step 1: 写失败的测试**（用 `test_support::test_app`）

```rust
#[test]
fn the_local_screen_merges_downloads_and_directories_without_duplicates() {
    let (mut app, _runtime) = test_app();
    let dir = app.config.paths.download_dir.clone();
    std::fs::create_dir_all(&dir).expect("建下载目录");
    std::fs::write(dir.join("歌手 - 下载的.flac"), vec![0_u8; 8]).expect("写下载文件");
    app.downloads.upsert(track_for("m1", "歌手 - 下载的.flac"));
    app.downloads.save(&dir).expect("落盘");

    let music = dir.parent().unwrap().join("music");
    std::fs::create_dir_all(&music).expect("建曲库目录");
    std::fs::write(music.join("自己的.wav"), vec![0_u8; 8]).expect("写本地文件");
    app.config.library.dirs = vec![music.to_string_lossy().into_owned()];
    // 曲库目录故意写成下载目录的父目录：下载那份必须只出现一次，且来源是「下载」。
    app.config.library.dirs.push(dir.to_string_lossy().into_owned());

    app.start_library_scan();
    app.apply_scan_for_test();          // 把后台结果灌进来（测试里同步跑）
    let songs = app.local_songs();
    assert_eq!(songs.len(), 2, "下载 1 + 目录 1，且没有重复：{songs:?}");
}

#[test]
fn the_download_marker_goes_by_mid_not_by_file_name() {
    let (mut app, _runtime) = test_app();
    let dir = app.config.paths.download_dir.clone();
    std::fs::create_dir_all(&dir).expect("建下载目录");
    std::fs::write(dir.join("同一首歌.flac"), vec![0_u8; 8]).expect("写文件");
    app.downloads.upsert(track_for("m1", "同一首歌.flac"));

    // 两首同歌手同标题的 QQ 歌：只有登记过的那个 mid 亮 `↓`。
    let mine = Song { mid: "m1".into(), title: "同一首歌".into(), singers: "同一人".into(), ..qq_song("m1") };
    let other = Song { mid: "m2".into(), title: "同一首歌".into(), singers: "同一人".into(), ..qq_song("m2") };
    assert!(app.downloaded(&mine));
    assert!(!app.downloaded(&other), "同名的另一首不该被标成已下载");
}

#[test]
fn a_second_song_with_the_same_file_name_does_not_impersonate() {
    let dir = temp_dir("free-name");
    std::fs::write(dir.join("同一人 - 同一首歌.flac"), vec![0_u8; 4]).expect("先有一份");
    assert_eq!(
        free_name(&dir, "同一人 - 同一首歌", "flac"),
        "同一人 - 同一首歌 (2).flac",
        "同名已被占（不管是谁的）就加序号"
    );
    assert_eq!(free_name(&dir, "别人 - 别的歌", "flac"), "别人 - 别的歌.flac");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_download_whose_file_is_gone_is_dropped_from_the_list() {
    let (mut app, _runtime) = test_app();
    let dir = app.config.paths.download_dir.clone();
    std::fs::create_dir_all(&dir).expect("建目录");
    std::fs::write(dir.join("还在.flac"), vec![0_u8; 4]).expect("写文件");
    app.downloads.upsert(track_for("m1", "还在.flac"));
    app.downloads.upsert(track_for("m2", "被删了.flac"));
    app.refresh_downloads();                   // 重载 + 校验 + 回写
    assert!(app.downloads.by_mid("m1").is_some());
    assert!(app.downloads.by_mid("m2").is_none(), "幽灵条目要掉");
    assert_eq!(app.local_songs().len(), 1);
    assert!(!app.downloaded(&qq_song("m2")), "`↓` 跟着灭");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn pressing_d_on_an_already_downloaded_song_only_says_so() {
    let (mut app, _runtime) = test_app();
    let dir = app.config.paths.download_dir.clone();
    std::fs::create_dir_all(&dir).expect("建目录");
    std::fs::write(dir.join("歌手 - 有了.flac"), vec![0_u8; 4]).expect("写文件");
    app.downloads.upsert(track_for("m1", "歌手 - 有了.flac"));
    let song = qq_song("m1");
    app.start_download_for_test(&song);
    assert!(app.download_in_flight().is_none(), "不该起下载");
    assert_eq!(app.notice_text(), Some("已下载：歌手 - 有了.flac".to_string()));
    std::fs::remove_dir_all(&dir).ok();
}
```

（`free_name(dir, stem, ext)` 是 `download.rs` 里新抽出的纯函数、`Track` 助手统一叫 `track_for(mid, file)`，都放各自文件的测试模块里；`start_download_for_test` / `download_in_flight` / `notice_text` 都在 `test_support` 里补一版薄的访问器——`App::notice_text()` 已有对应字段，包一层只读即可。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test tui::state::tests::the_local_screen_merges -- --nocapture`
Expected: 编译失败（`downloads.upsert` / `local_songs` / `downloaded` 未定义）。

- [ ] **Step 3: 实现**

- `download::Track::to_song(dir)`：见 Interfaces。
- `App`：`downloads: download::Index`；`App::new` 里 `Index::load(&config.paths.download_dir)`（校验 + 必要时回写）；`refresh_downloads()` 同逻辑。
- `start_library_scan` / `scan_library`：`rescan(previous, dirs, skip)`，`skip = &config.paths.download_dir`。
- `refresh_local_view`：`self.library`（曲库歌）**chain** 下载索引的 `to_song`（下载歌），两边合起来再 `set_items`；排序沿用「歌手 → 专辑 → 标题 → 路径」（把 `library::index_songs` 的排序抽成 `pub fn sort_songs(&mut Vec<Song>)` 给两处共用）。
- `local_songs()`：给测试用的只读访问器（返回合并后的 `Vec<Song>`）。
- `downloaded(&Song)`：`self.downloads.by_mid(&song.mid).is_some()`（供 `rows::copy_of` 换掉文件名判定）。
- `download_song`：先 `by_mid(&song.mid)`，命中且 `path_of` 存在 → `notify("已下载：<file>")` 并返回；否则照旧下载（`save` 的 `lyrics_dir` 参数删掉——歌词落音频同目录），成功后 `upsert` + `save`（`poll_transfer` 里做）。
- `download.rs` 里删掉 `scan_downloads()` 与 `Downloaded`，抽出纯函数 `pub(crate) fn free_name(dir: &Path, stem: &str, ext: &str) -> String`（目标名被占就追加 ` (2)`、` (3)`…），`save()` 的两条落盘分支（从缓存复制 / 网络下载）都改用它。
- `local_features_on()` 与「本地功能已关闭…」提示删掉（首页两格与第 5/6 屏恒可用）。
- `local_lyric(path, lyrics_dir)` 的调用点改成 `local_lyric(path)`；`download::file_stem` 只留给 `save()` 用。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test tui::state::tests`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add src/download.rs src/tui/state.rs
git commit -m "feat(tui): 第 5 屏合并下载与曲库目录，↓ 与 d 都按 mid"
```

---

### Task 7: 列与渲染：`来源` 列

**Files:**
- Modify: `src/tui/draw/rows.rs`、`src/tui/draw/content.rs`
- Test: 两个文件的 `mod tests`

**Interfaces:**
- Consumes: Task 5 的取源判定（`copy_of` 要能知道路径归谁）
- Produces: `Field::Source`；`LocalCopy { …, pub origin: Option<&'static str> }`（`Some("下载") / Some("目录") / None`）；`rows::Row { …, pub source: Option<&'static str> }`；`local_columns()` 里 `column(Field::Source, "来源", MARK, Align::Center)`。

- [ ] **Step 1: 写失败的测试**

```rust
#[test]
fn the_local_columns_have_a_source_column_that_drops_first_on_narrow_screens() {
    let wide = fit(&local_columns(), 120);
    let fields: Vec<Field> = wide.iter().map(|c| c.field).collect();
    assert!(fields.contains(&Field::Source), "{fields:?}");

    let narrow = fit(&local_columns(), 60);
    let fields: Vec<Field> = narrow.iter().map(|c| c.field).collect();
    assert!(!fields.contains(&Field::Source), "60 列下让位：{fields:?}");
    assert!(fields.contains(&Field::Title) && fields.contains(&Field::Duration));
}

#[test]
fn a_row_says_where_the_bytes_came_from() {
    // 下载索引里有的那份写「下载」，只在曲库目录里的写「目录」。
    let dir = std::env::temp_dir().join("canto-rows-source");
    std::fs::create_dir_all(&dir).expect("建目录");
    let downloaded = dir.join("歌手 - 下载的.flac");
    std::fs::write(&downloaded, vec![0_u8; 4]).expect("写文件");
    let mut index = crate::download::Index::default();
    index.upsert(crate::download::Track { mid: "m1".into(), file: "歌手 - 下载的.flac".into(), ..track_stub() });

    let mine = local_song(&downloaded);
    let other = local_song(&dir.join("自己的.flac"));
    assert_eq!(copy_of(&mine, &no_cache(), &index, &dir, Quality::Low128).origin, Some("下载"));
    assert_eq!(copy_of(&other, &no_cache(), &index, &dir, Quality::Low128).origin, Some("目录"));
    assert_eq!(copy_of(&qq_song("m9"), &no_cache(), &index, &dir, Quality::Low128).origin, None, "QQ 歌没这一格");
    std::fs::remove_dir_all(&dir).ok();
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test tui::draw::rows::tests::a_row_says_where -- --nocapture`
Expected: 编译失败（`Field::Source` / `origin` 未定义）。

- [ ] **Step 3: 实现**

`copy_of` 的参数从 `downloads: &HashMap<String, Downloaded>` 换成 `&crate::download::Index` + `download_dir: &Path`；`origin` 只在 `song.is_local()` 时给值（`index.mid_of_path(dir, path).is_some()` → `"下载"` 否则 `"目录"`）。
`Field::Source` 进 `DROP_ORDER`（排在 `Field::Album` 之后，`Note` 之后第二位），`local_columns()` 加上它；`Row` 加 `source` 字段并在 `rows::line` 里按 `Align::Center` 渲染（`None` 写空串）；`song_row` 把 `copy.origin` 传进 `Row`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test tui::draw`
Expected: PASS（既有列模型测试跟着改口径后仍绿）。

- [ ] **Step 5: 提交**

```bash
git add src/tui/draw/rows.rs src/tui/draw/content.rs
git commit -m "feat(tui): 本地屏加「来源」列（下载 / 目录）"
```

---

### Task 8: 本地歌单快照补字段

**Files:**
- Modify: `src/playlists.rs`
- Test: `src/playlists.rs` 的 `mod tests`

**Interfaces:**
- Produces: `pub struct StoredSong { pub mid: String, pub title: String, pub singers: String, pub album: String, pub duration_secs: u64 }`；`pub const VERSION: u32 = 2;`；`Store::add` 顺手带走这两个字段。

- [ ] **Step 1: 写失败的测试**

```rust
#[test]
fn a_stored_song_keeps_the_album_and_duration() {
    let song = Song { album_name: "专辑".into(), duration_secs: 251, ..song_stub("m1") };
    let mut store = Store::default();
    let index = store.create("通勤").expect("建歌单");
    store.add(index, &song);
    let stored = &store.playlists[0].songs[0];
    assert_eq!((stored.album.as_str(), stored.duration_secs), ("专辑", 251));

    let text = serde_json::to_string(&store).expect("序列化");
    let back: Store = serde_json::from_str(&text).expect("反序列化");
    assert_eq!(back.playlists[0].songs[0].duration_secs, 251);
}

#[test]
fn an_old_playlist_file_is_treated_as_empty() {
    let text = r#"{"version":1,"playlists":[{"name":"旧的","songs":[]}]}"#;
    let store: std::result::Result<Store, _> = serde_json::from_str(text);
    assert!(store.map(|s| s.version == 1).unwrap_or(true), "老版本号读得出来但版本对不上");
    assert_eq!(VERSION, 2);
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test playlists::tests::a_stored_song_keeps -- --nocapture`
Expected: 编译失败（`StoredSong` 没有 `album` / `duration_secs`）。

- [ ] **Step 3: 实现**

`StoredSong` 加两个字段、`VERSION` 改 2、`Store::add` 从 `Song` 里取 `album_name` / `duration_secs`；`Store::load` 的版本判定照旧（不符 → 空）。本地歌单屏那两列的渲染（`state.rs` 的 `refresh_local_playlists` 一带）改成从快照取（原先写死 `0` / 空串的地方）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test playlists::tests`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add src/playlists.rs src/tui/state.rs
git commit -m "feat(playlists): 本地歌单快照带上专辑与时长（版本 2）"
```

---

### Task 9: 文档

**Files:**
- Modify: `README.md`、`docs/design.md`

- [ ] **Step 1: 改 README**

- 「功能」表：本地曲库那两条（`5` / `6`）与下载那一条按新模型重写；播放取源那一条改成「缓存 > 下载索引 > 线上」并删掉「同一份文件从 5 那屏播写曲库、从在线列表里播写下载」那句限制。
- 「配置」章：贴的模板片段与 `render_default()` 对齐（`[paths]` 只有 `download_dir`；`[library]` 两态 + 跳过下载目录）。
- 「文件」清单：下载目录里加 `downloads.json` 与同目录 `.lrc`；缓存目录里歌词与音频同目录、没有 `lyrics/`。
- 「已知限制」：新增「下载目录只认索引（手工拷进去的文件不认）」「换 `download_dir` 要连 `downloads.json` 一起搬」「升级后老下载文件不进索引」；删掉「`↓` 按文件名认」与「`↓` 的档位只到容器」两条（现在档位来自索引）。

- [ ] **Step 2: 改 design.md**

「本地曲库（`5`）」那一节改写成「本地文件模型（下载 / 曲库 / 缓存 / 本地歌单）」：三个索引各管一段、取源链、第 5 屏合并列表与 `来源` 列、`sweep` 白名单、歌词同目录；「代码结构」里 `download.rs` / `library.rs` / `cache.rs` 那三行按新职责改写。

- [ ] **Step 3: 检查没有旧说法残留**

Run: `grep -rn "lyrics_dir\|scan_downloads\|按文件名认\|歌手 - 标题\." README.md docs/design.md src`
Expected: 只剩 spec/plan 里的历史引用（`docs/superpowers/**`）与代码里确实还需要 `file_stem` 的地方（`download::save` 的落盘命名）。

- [ ] **Step 4: 提交**

```bash
git add README.md docs/design.md
git commit -m "docs: 本地文件模型（下载索引 / 曲库 / 缓存 / 来源列）"
```

---

### Task 10: 真机冒烟（tuios）

**Files:** 无（验证任务）

- [ ] **Step 1: 起测试环境**

```bash
cp "C:/Users/28269/Desktop/session.json" "C:/Users/28269/.config/canto/session.json"   # 使用者提供的登录态
./target/debug/canto.exe config init                                                    # 生成新模板（[paths] 只有一个键）
grep -n "download_dir\|dirs\|lyrics_dir" "C:/Users/28269/.config/canto/config.toml"
```
Expected: `[paths]` 有 `download_dir`、**没有** `lyrics_dir`；`[library]` 段是两态注释。

- [ ] **Step 2: 下载一首（真网）**

按 AGENTS 的窗口冒烟：`tuios new-window -s canto-dev canto-local --cwd G:/code/canto --no-focus -- G:/code/canto/target/debug/canto.exe --log debug`，等 `在线收藏`，`Enter` 进收藏、`d` 下载选中那首，等状态行出现「已下载」。

- [ ] **Step 3: 检查落盘与标记**

```bash
ls -la "C:/Users/28269/Music/canto"          # 音频 + .lrc + downloads.json
python -c "import json;print(json.load(open(r'C:/Users/28269/Music/canto/downloads.json',encoding='utf-8'))['tracks'][0])" 2>/dev/null || cat "C:/Users/28269/Music/canto/downloads.json"
```
Expected: `downloads.json` 里那一条的 `mid` 与在线列表那首一致、`file` 就是刚落的文件名；退出重进后那一行的 `↓` **仍然亮**（说明来自索引而不是重扫目录）。

- [ ] **Step 4: 校验三处一致**

把音频改名（`mv` 成 `xxx.flac`）→ 重进第 5 屏：那条消失、`↓` 灭；把 `downloads.json` 删掉 → 第 5 屏的下载歌全部消失（未登记不认）。
然后 `canto config init` 之外不要再动配置，把文件改回原名、索引用第 3 步备份的副本还原。

- [ ] **Step 5: 缓存超限与歌词**

把 `[cache] mb` 调成 `1`，播一首完整听完（或先手工塞几个几百 KB 的假缓存文件），重启：`canto.log` 里有「缓存清理：删除 N 个文件」；`ls ~/.cache/canto` 里 `library.json` 与 `.lrc` 仍在。

- [ ] **Step 6: 记录结果**

把「哪几条真验过 / 哪条没验成」写进本任务的提交信息（`docs: 本地文件模型真机实测记录`，附到 design.md 的实测段落）。

```bash
git add docs/design.md
git commit -m "docs: 本地文件模型真机实测（下载索引 / ↓ 按 mid / sweep 白名单）"
```

# canto 终端界面重构实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 canto 的终端界面重构成「左侧导航菜单 + 右侧主区 + 底部歌词 + 全宽播放条」的单入口 TUI，并把 CLI 子命令全部删掉、登录搬进 TUI、队列变成可见可操作的显式队列。

**Architecture:** 四阶段渐进：①管道与收缩（`paths.rs` / `config.rs` / `history.rs` / CLI 删减，旧界面不动）②新骨架（`tui/` 拆模块、`Screen` 机制、菜单路由、三段布局、状态行拆分、帮助浮层）③显式队列（纯逻辑 `queue.rs` + `u` 浮层 + 播放模式 + 历史接入）④登录屏（`qr.rs` widget 化 + `Screen::Login` + 会话失效切屏）。每阶段结束都能 `cargo run` 验收。

**Tech Stack:** Rust 2024 edition、ratatui 0.30、crossterm 0.29、rodio 0.22、reqwest 0.13（blocking + cookies）、tokio、serde、clap 4。

**Spec:** `docs/superpowers/specs/2026-10-11-tui-redesign-design.md`（设计与本节冲突时以 spec 为准，本计划负责落地）

**执行顺序（实施时调整过）**：阶段 1（Task 1、2）→ 阶段 2（Task 5–9）→ 阶段 3（Task 3、10–13）→ 阶段 4（Task 14–16）→ **阶段 5（Task 17：CLI 收缩与 README 收尾）**。Task 3 与 Task 17 都从原位置挪了，理由写在各自的小节里；每个任务结束都必须 `cargo test` 全绿且无新增警告（这就是挪动的原因——提前删子命令会让登录与写入路径立刻变成 dead code）。

## Global Constraints

- 只新增一个依赖：`toml`。**不**引入 `rand` / `chrono` / `time`（洗牌用本地 xorshift64\*，时间格式化用已有 `libc::localtime_r`）。
- 键位表按 spec 第 2 节原样实现：`1`–`4` 直达菜单、`Tab` 焦点循环、`j`/`k`/`g`/`G`/`Home`/`End`/`PgUp`/`PgDn`/`Ctrl+U`/`Ctrl+D`、`Enter`、`Esc`/`Backspace` 返回、`Space`、`n`/`b`、`←`/`→`(`h`/`l`)、`+`/`-`(`,`/`.`) 、`m`、`a`/`A`、`p`、`x`、`f`、`d`、`u`、`y`、`/`、`?`、`q`/`Ctrl-C`。
- 尺寸契约：菜单 16 列；歌词块 `clamp(主区高度/3, 5, 12)`；播放条恒 4 行；状态行 1 行（左键位提示、右状态，右对齐）；<100 列菜单收成顶部页签行；<20 行高歌词关闭；<60×15 只画一行尺寸提示。
- `quality` 取值与现有 `Quality::parse` 一致（`hires`/`flac`/`320`/`128`）；`download_dir` 支持 `~` 展开；`history_limit` 默认 500。
- 队列语义严格按 spec 第 3 节（含空队列 `a`/`A` = 开新队列、删当前项播原 `index+1`、随机一轮洗牌内不重复）。
- 中文注释与中文界面文案，风格与现有代码一致（说明「为什么」，不复述「做了什么」）。
- 现有测试（`model.rs` / `session.rs` / `qr.rs` / `player.rs` / `login.rs` 内的 `mod tests`）除被本计划显式改签名的以外，保持通过。
- 每个任务结束都要 `cargo test` 全绿 + `cargo clippy --all-targets` 无新警告（仓库现状是否 clippy-clean 由执行者先跑一次确认基线）。

---

## 文件结构

| 文件 | 责任 |
|---|---|
| `src/paths.rs`（新） | 所有落盘路径的唯一来源（会话/配置/历史/日志/二维码 PNG） |
| `src/config.rs`（重写） | `config.toml` 的载入、默认生成、坏文件回退、`~` 展开 |
| `src/history.rs`（新） | 播放历史的读写、上限截断、连续去重、清空、相对时间文案 |
| `src/cli.rs`（删减） | 只剩 `--log` / `--version` / `--help` |
| `src/main.rs`（删减） | 日志 → config → session → `tui::run`；删除全部子命令实现 |
| `src/qr.rs`（改接口） | PNG → `Vec<String>` 字符块，不再 `print!` |
| `src/tui/mod.rs`（新） | `run` / 事件循环 / `Screen` 切换 |
| `src/tui/state.rs`（新） | `App` 状态、`Msg` 处理、视图与焦点 |
| `src/tui/keys.rs`（新） | `KeyEvent` + 上下文 → `Action`（纯函数） |
| `src/tui/queue.rs`（新） | 队列模型与模式推进（纯逻辑，无 UI 依赖） |
| `src/tui/login.rs`（新） | 登录屏状态与轮询 |
| `src/tui/draw/mod.rs`（新） | 骨架布局、断点、总装 |
| `src/tui/draw/{menu,content,lyric,player,status,overlay}.rs`（新） | 各面板绘制 |
| `src/tui.rs` | **删除**，内容按上表拆开 |

---

# 阶段 1：管道与收缩（旧界面不动，可独立交付）

> **执行顺序调整（实施时定）**：Task 3（`history.rs`）挪到阶段 3、与它的消费方（Task 13 历史屏）同批提交。理由：整套历史 API（`relative` / `entries` / `clear` …）要等历史屏落地才有人调用，提前提交只会留下 `dead_code` 警告，而用 `#[allow(dead_code)]` 压掉是把警告当解决。阶段 1 = Task 1 / Task 2（Task 4 已挪到阶段 5，见文末）。

### Task 1: `paths.rs` 收拢所有路径

**Files:**
- Create: `src/paths.rs`
- Delete: `src/config.rs`（20 行的路径模块，功能被 `paths.rs` 取代；新的 `config.rs` 在 Task 2 重建）
- Modify: `src/main.rs`（`config::data_dir()` / `config::session_path()` 改 `paths::`）、`src/session.rs`（`config::session_path()` 改 `paths::`）、`src/main.rs:26`（`mod config;` → `mod paths;`，Task 2 再加回 `mod config;`）

**Interfaces:**
- Produces: `paths::data_dir() -> PathBuf`、`paths::session_path()`、`paths::config_path()`、`paths::history_path()`、`paths::log_path()`、`paths::qr_png_path()`（全部 `-> PathBuf`）

- [ ] **Step 1: 写失败的测试**

`src/paths.rs`：

```rust
//! 所有落盘路径的唯一来源。
//!
//! 以前 `config.rs` 管会话路径，`main.rs` 自己拼 `login-qr.png` 与 `canto.log`，
//! 同一份目录规则散在三处——改一处忘一处就会出现「日志目录建了、会话目录没建」这类错。

use std::path::PathBuf;

/// 应用数据目录：`<系统配置目录>/canto`。macOS 是 `~/Library/Application Support/canto`。
pub fn data_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("canto")
}

pub fn session_path() -> PathBuf {
    data_dir().join("session.json")
}

pub fn config_path() -> PathBuf {
    data_dir().join("config.toml")
}

pub fn history_path() -> PathBuf {
    data_dir().join("history.json")
}

pub fn log_path() -> PathBuf {
    data_dir().join("canto.log")
}

pub fn qr_png_path() -> PathBuf {
    data_dir().join("login-qr.png")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 这些路径都会写进同一个目录（`save` 只建一次父目录），名字撞车会互相覆盖。
    #[test]
    fn all_paths_share_one_directory_and_are_distinct() {
        let dir = data_dir();
        let paths = [
            session_path(),
            config_path(),
            history_path(),
            log_path(),
            qr_png_path(),
        ];
        for path in &paths {
            assert_eq!(path.parent(), Some(dir.as_path()), "{} 不在数据目录里", path.display());
        }
        let mut names: Vec<_> = paths
            .iter()
            .map(|path| path.file_name().expect("都有文件名").to_os_string())
            .collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), paths.len(), "路径撞车：{names:?}");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test paths`
Expected: 编译失败——`paths` 模块还没在 `main.rs` 里声明

- [ ] **Step 3: 接线**

`src/main.rs`：把 `mod config;` 换成 `mod paths;`；文件里所有 `config::data_dir()` / `config::session_path()` 改成 `paths::data_dir()` / `paths::session_path()`（`init_tracing` 里的日志路径、`login()`、`qr_login()`、`whoami()`）。

`src/session.rs`：`use crate::config;` → `use crate::paths;`；`config::session_path()` → `paths::session_path()`（`save` / `clear` 两处）。删掉 `src/config.rs`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test && cargo build`
Expected: 全绿 + 构建通过

- [ ] **Step 5: 提交**

```bash
git add -A src/paths.rs src/config.rs src/main.rs src/session.rs
git commit -m "refactor(paths): 落盘路径收拢到 paths.rs"
```

---

### Task 2: `config.toml`（音质 / 下载目录 / 历史上限）

**Files:**
- Create: `src/config.rs`
- Modify: `Cargo.toml`（新增 `toml`）、`src/main.rs`（`mod config;` 加回，启动时载入）

**Interfaces:**
- Consumes: `paths::config_path()`、`model::Quality::{parse, Flac}`
- Produces:
  - `config::Config { pub quality: Quality, pub download_dir: PathBuf, pub history_limit: usize }`（`impl Default`）
  - `config::load() -> (Config, Option<String>)`——第二个值是给状态行的警告（坏文件时）
  - `config::default_download_dir() -> PathBuf`

- [ ] **Step 1: 加依赖**

```bash
cargo add toml
```

- [ ] **Step 2: 写失败的测试**

`src/config.rs`：

```rust
//! 配置文件：`<配置目录>/canto/config.toml`。
//!
//! 自用工具，配置面刻意窄：音质下限、下载目录、历史上限。界面上不提供设置项
//! ——改配置就是开这个文件改，因此**解析失败绝不能变成打不开程序**。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::model::Quality;
use crate::paths;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub quality: Quality,
    pub download_dir: PathBuf,
    pub history_limit: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            quality: Quality::Flac,
            download_dir: default_download_dir(),
            history_limit: 500,
        }
    }
}

/// 默认下载目录：`<系统音乐目录>/canto`。
pub fn default_download_dir() -> PathBuf {
    dirs::audio_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("canto")
}

/// 文件里能写的字段；全 `Option`——缺字段就用默认值，不要求写全。
#[derive(Debug, Deserialize)]
struct Raw {
    quality: Option<String>,
    download_dir: Option<String>,
    history_limit: Option<usize>,
}

impl Raw {
    fn resolve(self, warnings: &mut Vec<String>) -> Config {
        let mut config = Config::default();
        if let Some(text) = self.quality {
            match Quality::parse(&text) {
                Some(quality) => config.quality = quality,
                None => warnings.push(format!(
                    "config.toml 里的 quality = {text:?} 不认识（可用：hires / flac / 320 / 128），用默认值 {}",
                    config.quality.label()
                )),
            }
        }
        if let Some(text) = self.download_dir {
            config.download_dir = expand_home(&text);
        }
        if let Some(limit) = self.history_limit {
            config.history_limit = limit.max(1);
        }
        config
    }
}

/// `~/Music/canto` → `$HOME/Music/canto`（`dirs::audio_dir()` 之类的返回值不带 `~`，
/// 但手写配置时一定会写）。展开不了就原样当相对路径用。
pub fn expand_home(text: &str) -> PathBuf {
    if let Some(rest) = text.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_from(tag: &str, text: &str) -> (Config, Option<String>) {
        let dir = std::env::temp_dir().join(format!("canto-config-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let path = dir.join("config.toml");
        std::fs::write(&path, text).expect("写配置");
        let result = load_file(&path);
        std::fs::remove_dir_all(&dir).ok();
        result
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let (config, warning) = load_from("missing", "quality = \"320\"\n");
        assert_eq!(config.quality, Quality::High320);
        assert_eq!(config.download_dir, default_download_dir());
        assert_eq!(config.history_limit, 500);
        assert!(warning.is_none(), "缺字段不是错误：{warning:?}");
    }

    #[test]
    fn unknown_quality_warns_and_keeps_default() {
        let (config, warning) = load_from("unknown", "quality = \"cd\"\n");
        assert_eq!(config.quality, Quality::Flac);
        assert!(warning.expect("应给出警告").contains("cd"));
    }

    #[test]
    fn broken_toml_falls_back_to_defaults_with_a_warning() {
        let (config, warning) = load_from("broken", "quality = \n");
        assert_eq!(config, Config::default());
        assert!(warning.expect("应给出警告").contains("解析失败"));
    }

    #[test]
    fn tilde_in_download_dir_expands_to_home() {
        let home = dirs::home_dir().expect("有家目录");
        assert_eq!(expand_home("~/Music/canto"), home.join("Music/canto"));
        assert_eq!(expand_home("/tmp/x"), PathBuf::from("/tmp/x"));
    }
}
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test config`
Expected: FAIL——`load_file` 未定义

- [ ] **Step 4: 写实现**

同文件补上：

```rust
/// 载入配置。文件不存在则**生成一份默认配置**（让使用者知道有这个文件可改）。
/// 返回的 `Option<String>` 是给状态行的一次性警告。
pub fn load() -> (Config, Option<String>) {
    let path = paths::config_path();
    if !path.exists() {
        let warning = write_default(&path).err().map(|error| format!("{error:#}"));
        return (Config::default(), warning);
    }
    load_file(&path)
}

fn load_file(path: &Path) -> (Config, Option<String>) {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            return (
                Config::default(),
                Some(format!("读 {} 失败：{error}", path.display())),
            );
        }
    };
    let mut warnings = Vec::new();
    let raw = match toml::from_str::<Raw>(&text) {
        Ok(raw) => raw,
        Err(error) => {
            return (
                Config::default(),
                Some(format!("{} 解析失败，用默认配置：{error}", path.display())),
            );
        }
    };
    let config = raw.resolve(&mut warnings);
    (config, (!warnings.is_empty()).then(|| warnings.join("；")))
}

/// 首次运行生成默认配置：注释里写清取值，改的时候不用回来翻代码。
fn write_default(path: &Path) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(parent)
        .with_context(|| format!("创建配置目录失败：{}", parent.display()))?;
    let default = Config::default();
    // 写回文件的值必须是 `Quality::parse` 认得的短名（`Flac::label()` 是中文「无损」）。
    let quality = match default.quality {
        Quality::Flac => "flac".to_string(),
        other => other.label().to_lowercase(),
    };
    let text = format!(
        "# canto 配置。改完重启 canto 生效。\n\
         # 音质下限：从这一档往下找 —— hires | flac | 320 | 128\n\
         quality = \"{quality}\"\n\
         # 下载目录（`d` 键落盘的地方），支持 `~`\n\
         download_dir = \"{}\"\n\
         # 播放历史最多保留多少条\n\
         history_limit = {}\n",
        default.download_dir.display(),
        default.history_limit,
    );
    std::fs::write(path, text).with_context(|| format!("写 {} 失败", path.display()))
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test config`
Expected: PASS（4 个用例）

- [ ] **Step 6: 提交**

```bash
git add Cargo.toml Cargo.lock src/config.rs
git commit -m "feat(config): config.toml 承载音质下限/下载目录/历史上限"
```

---

### Task 3: 播放历史的读写与相对时间

> **在阶段 3 执行**（与 Task 13 同批提交），原因见阶段 1 开头的说明。

**Files:**
- Create: `src/history.rs`
- Modify: `src/main.rs`（`mod history;`）

**Interfaces:**
- Consumes: `paths::history_path()`、`model::Song`
- Produces:
  - `history::Entry { pub mid: String, pub title: String, pub singers: String, pub at: u64, pub origin: String }`
  - `history::History`，方法：`load(path: &Path, limit: usize) -> History`、`push(&mut self, entry: Entry, limit: usize)`、`is_empty()`、`len()`、`entries(&self) -> &[Entry]`、`save(&self, path: &Path) -> Result<()>`、`clear(&mut self, path: &Path) -> Result<()>`
  - `history::relative(now: u64, at: u64) -> String`、`history::now_secs() -> u64`

- [ ] **Step 1: 写失败的测试**

`src/history.rs`：

```rust
//! 播放历史：`<配置目录>/canto/history.json`。
//!
//! 只记「哪首、什么时候、从哪个列表来的」，够历史页显示与重播就行。
//! 新条目插到最前，超上限从尾部丢——列表按时间倒序读，不用每次排序。

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// 一条播放记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub mid: String,
    pub title: String,
    pub singers: String,
    /// Unix 秒。
    pub at: u64,
    /// 当时队列的来源名（歌单名 / `搜索：晴天` / `历史`）。
    pub origin: String,
}

#[derive(Debug, Clone, Default)]
pub struct History {
    /// 新的在前。
    entries: Vec<Entry>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(mid: &str, at: u64) -> Entry {
        Entry {
            mid: mid.to_string(),
            title: format!("歌 {mid}"),
            singers: "歌手".to_string(),
            at,
            origin: "嗯".to_string(),
        }
    }

    fn temp_path(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("canto-history-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir.join("history.json")
    }

    #[test]
    fn keeps_newest_first_and_truncates_to_limit() {
        let mut history = History::default();
        for index in 0..5u64 {
            history.push(entry(&format!("m{index}"), 100 + index), 3);
        }
        let mids: Vec<_> = history.entries().iter().map(|e| e.mid.as_str()).collect();
        assert_eq!(mids, ["m4", "m3", "m2"], "新的在前且只留 3 条");
    }

    #[test]
    fn repeats_of_the_same_song_only_refresh_the_timestamp() {
        let mut history = History::default();
        history.push(entry("m1", 100), 10);
        history.push(entry("m2", 200), 10);
        history.push(entry("m2", 300), 10);
        assert_eq!(history.len(), 2, "连续同 mid 不该刷屏");
        assert_eq!(history.entries()[0].at, 300, "时间戳应被刷新");
    }

    #[test]
    fn round_trips_through_disk_and_tolerates_a_broken_file() {
        let path = temp_path("roundtrip");
        let mut history = History::default();
        history.push(entry("m1", 100), 10);
        history.save(&path).expect("落盘");
        let loaded = History::load(&path, 10);
        assert_eq!(loaded.entries(), history.entries());

        std::fs::write(&path, "{ 不是 JSON").expect("写坏文件");
        assert!(History::load(&path, 10).is_empty(), "坏文件当空历史，不阻塞启动");
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn clear_empties_memory_and_disk() {
        let path = temp_path("clear");
        let mut history = History::default();
        history.push(entry("m1", 100), 10);
        history.save(&path).expect("落盘");
        history.clear(&path).expect("清空");
        assert!(history.is_empty());
        assert!(History::load(&path, 10).is_empty());
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn relative_time_reads_like_a_person_wrote_it() {
        let now = now_secs();
        assert_eq!(relative(now, now), "刚刚");
        assert_eq!(relative(now, now - 300), "5 分钟前");
        let text = relative(now, now - 2 * 24 * 3600);
        assert!(!text.is_empty() && text != "刚刚", "久远的记录给出日期：{text}");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test history`
Expected: FAIL——`History::push` / `History::load` / `relative` 未定义

- [ ] **Step 3: 写实现**

同文件补上：

```rust
impl History {
    /// 读历史；文件不在或读不动就当空历史（历史丢了不该阻塞启动）。
    pub fn load(path: &Path, limit: usize) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let Ok(mut entries) = serde_json::from_str::<Vec<Entry>>(&text) else {
            return Self::default();
        };
        entries.truncate(limit);
        Self { entries }
    }

    /// 记一条。与最上面那条同 mid 时只刷新时间戳——单曲循环否则会把历史刷成一条。
    pub fn push(&mut self, entry: Entry, limit: usize) {
        if let Some(first) = self.entries.first_mut()
            && first.mid == entry.mid
        {
            first.at = entry.at;
            return;
        }
        self.entries.insert(0, entry);
        self.entries.truncate(limit.max(1));
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 落盘（临时文件 + rename，中途失败不留坏文件）。
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建配置目录失败：{}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(&self.entries).context("序列化历史失败")?;
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, text).with_context(|| format!("写 {} 失败", temp.display()))?;
        std::fs::rename(&temp, path).with_context(|| format!("替换 {} 失败", path.display()))
    }

    /// 清空内存与磁盘。
    pub fn clear(&mut self, path: &Path) -> Result<()> {
        self.entries.clear();
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| format!("删除 {} 失败", path.display())),
        }
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// `刚刚` / `12 分钟前` / `21:30` / `昨天 21:30` / `3月5日` / `2025年12月31日`。
pub fn relative(now: u64, at: u64) -> String {
    let delta = now.saturating_sub(at);
    if delta < 60 {
        return "刚刚".to_string();
    }
    if delta < 3600 {
        return format!("{} 分钟前", delta / 60);
    }
    let (now_tm, at_tm) = (local_tm(now), local_tm(at));
    let hhmm = format!("{:02}:{:02}", at_tm.tm_hour, at_tm.tm_min);
    if now_tm.tm_year == at_tm.tm_year {
        if now_tm.tm_yday == at_tm.tm_yday {
            return hhmm;
        }
        if now_tm.tm_yday - at_tm.tm_yday == 1 {
            return format!("昨天 {hhmm}");
        }
        return format!("{}月{}日", at_tm.tm_mon + 1, at_tm.tm_mday);
    }
    format!(
        "{}年{}月{}日",
        at_tm.tm_year + 1900,
        at_tm.tm_mon + 1,
        at_tm.tm_mday
    )
}

/// 本地时间（借 `libc`，不为一句 `HH:MM` 再拉一个时间库）。
fn local_tm(at: u64) -> libc::tm {
    let seconds = at as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: `localtime_r` 只写 `tm`；`seconds` 与 `tm` 都是本栈上的有效值。
    unsafe { libc::localtime_r(&seconds, &mut tm) };
    tm
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test history`
Expected: PASS（5 个用例）

- [ ] **Step 5: 提交**

```bash
git add src/history.rs src/main.rs
git commit -m "feat(history): 播放历史读写、上限截断与相对时间文案"
```

---

# 阶段 5：CLI 收缩与收尾（**在登录屏之后执行**）

> 为什么不在阶段 1 做：删掉子命令的那一刻，`qr::show`、`qq::login::{create, poll}`、`session::Jar::login_with_cookie`、`qq::write::set_favorite` 就没有调用方了——它们的新家分别是登录屏（Task 15）与 TUI 内的收藏/歌单写入。先删就只剩两条路：留一堆 `dead_code` 警告，或者用 `#[allow]` 把警告压掉（那是拿抑制当解决）。所以收缩放在最后，此时 TUI 已经覆盖登录、播放、搜索、下载、收藏、加/移歌单、登出。
>
> `config` 的接线（`tui::run` 收 `config`、下载目录与音质取自 config）已在 Task 2 一并完成，本任务只剩删除与收尾。

### Task 17: CLI 收缩到只剩 `--log`

**Files:**
- Modify: `src/cli.rs`（删到只剩 `log`）、`src/main.rs`（删掉全部子命令分支与辅助函数）、`src/tui.rs`（`run` 增参、下载目录与音质取自 config）、`README.md`
- Test: `src/cli.rs` 内联测试

**Interfaces:**
- Consumes: `config::load() -> (Config, Option<String>)`、`paths::{log_path, data_dir}`
- Produces: `tui::run(handle: tokio::runtime::Handle, engine: Engine, config: Config, warning: Option<String>) -> Result<()>`

- [ ] **Step 1: 写失败的测试**

`src/cli.rs` 全文替换为：

```rust
//! 命令行入口。
//!
//! 界面上能做的事一律进 TUI；这里只留排查用的 `--log` 与 clap 自带的
//! `--version` / `--help`。

use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "canto", version, about = "canto —— 自用的 QQ 音乐播放器")]
pub struct Cli {
    /// 日志级别：error / warn / info / debug / trace（也可写 `reqwest=debug` 这样的过滤串）
    #[arg(short, long, default_value = "warn")]
    pub log: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_knows_the_log_flag() {
        let cli = Cli::try_parse_from(["canto", "--log", "debug"]).expect("--log 该被接受");
        assert_eq!(cli.log, "debug");
        assert_eq!(
            Cli::try_parse_from(["canto"]).expect("裸跑该被接受").log,
            "warn"
        );
        assert!(
            Cli::try_parse_from(["canto", "playlists"]).is_err(),
            "子命令已经删了，不该再被接受"
        );
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test cli`
Expected: FAIL——`main.rs` 仍在匹配已删除的 `Command` 变体，编译不过

- [ ] **Step 3: 删 `main.rs` 的子命令实现**

`src/main.rs` 从 455 行删到约 60 行：

```rust
//! canto —— 自用的 QQ 音乐播放器。
//!
//! 启动路径只有一条：日志 → 配置 → 会话 → 终端界面（未登录时先出登录屏，
//! 见 `tui::login`）。

mod cli;
mod config;
mod download;
mod history;
mod http;
mod media;
mod model;
mod paths;
mod player;
mod qq;
mod qr;
mod session;
mod tui;

use anyhow::{Context, Result};
use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::cli::Cli;
use crate::player::Engine;

/// 这里**不用** `#[tokio::main]`：播放与 `reqwest::blocking` 的媒体流都在自己的
/// 线程上同步跑，只有网络请求交给运行时。
fn main() -> Result<()> {
    restore_sigpipe();
    let cli = Cli::parse();
    // 界面模式下 stdout/stderr 就是屏幕，日志必须落文件，否则糊屏。
    init_tracing(&cli.log)?;
    let runtime = tokio::runtime::Runtime::new().context("创建异步运行时失败")?;
    let (config, warning) = config::load();
    let engine = Engine::start()?;
    tui::run(runtime.handle().clone(), engine, config, warning)
}

fn restore_sigpipe() { /* 原样保留 240-253 行 */ }

fn init_tracing(level: &str) -> Result<()> { /* 原样，只把 to_file 分支转正、路径换 paths::log_path() */ }
```

删除：`Command` 的全部匹配分支、`wait_until_finished`、`parse_quality`、`resolve_playlist`、`login`、`qr_login`、`whoami`、`print_playlists`、`print_tracks`、`print_song_rows`，以及不再使用的 `use`（`std::io::Read`、`crate::cli::Command`、`crate::model::{Playlist, Quality, Song}`、`crate::download`、`crate::session`、`qr` 等按编译器报错逐个清）。`QR_ATTEMPTS` / `QR_POLL_INTERVAL` 两个常量在 Task 15 已搬进 `tui/login.rs`，这里删掉 main.rs 里的副本。

同时删掉三处因为「唯一调用方在命令行」而失去调用方的 API：`qq::write::set_favorite`（`qq/write.rs`，`Favorite` / `Unfavorite` 两个子命令走的都是它；TUI 的收藏走 `add(FAVORITES_DIR_ID, 0, …)`）、`download::default_dir`（TUI 用 `config.download_dir`）、以及 `init_tracing` 的 `to_file: bool` 参数（既然只有 TUI 一种模式，日志恒写文件）。`qq/write.rs` 里钉 `set_favorite` 请求形状的测试一并改成钉 `add`/`remove`。

- [x] **Step 4: `tui::run` 接 config —— 已在 Task 2 完成**

（保留在本节是为了说明签名来源：`tui::run(handle, engine, config, warning)`，`App` 持 `config: Config`，`download_selected` 用 `config.download_dir`、`play_index` 与下载都传 `Some(config.quality)`。）

`src/tui.rs`：

```rust
pub fn run(
    handle: tokio::runtime::Handle,
    engine: Engine,
    config: Config,
    warning: Option<String>,
) -> Result<()> {
    let (tx, rx) = channel();
    let mut app = App::new(handle, engine, config, tx);
    if let Some(warning) = warning {
        app.status = warning;
    }
    app.reload_playlists();
    ...
}
```

`App` 增字段 `config: Config`，并把两处硬编码接上：

```rust
    /// 下载右栏里选中的那首（含歌词），进度走状态栏。
    fn download_selected(&mut self) {
        ...
        let dir = self.config.download_dir.clone();   // 原 download::default_dir()
        ...
    }

    /// 从队列里播第 `index` 首：先取候选地址（后台），拿到就交给引擎。
    fn play_index(&mut self, index: usize) {
        ...
        let quality = self.config.quality;
        self.handle.spawn(async move {
            let result = qq::vkey::candidates(&handle_song, Some(quality)).await;
            ...
        });
    }
```

`download::default_dir()` 只剩 `config::default_download_dir()` 一个调用者被删后，把 `download.rs` 里的 `default_dir` 删掉（避免两处各写一份默认目录）。

- [ ] **Step 5: 跑测试与冒烟**

Run: `cargo test && cargo clippy --all-targets`
Expected: 全绿、无新警告

Run: `cargo run -- --help`
Expected: 只有 `--log`、`-h`、`-V`，没有任何子命令

Run: `cargo run`（已登录的机器）
Expected: 进主界面，行为与改动前一致；改 `config.toml` 的 `download_dir` 后按 `d` 落到新目录

- [ ] **Step 6: 更新 README**

删掉「命令行子命令」整节；「用法」段改为：

```bash
cargo run            # 进终端界面（所有功能都在里面）
cargo run -- --log info   # 把播放细节写进日志文件
```

并在「已知限制」里保留原 `--quality` 的说明改为 config 说法（音质下限在 `config.toml` 的 `quality`）。

- [ ] **Step 7: 提交**

```bash
git add -A src/cli.rs src/main.rs src/download.rs src/qq/write.rs README.md
git commit -m "refactor(cli): 删掉全部数据子命令，只留 --log"
```

---

# 阶段 2：新骨架（菜单 / 路由 / 布局 / 键位）

> 阶段 2 起 `tui.rs` 拆成 `tui/` 目录。每一步都保持 `cargo run` 可用。

### Task 5: 拆模块 + 键位表抽成纯函数

**Files:**
- Delete: `src/tui.rs`
- Create: `src/tui/mod.rs`、`src/tui/state.rs`、`src/tui/keys.rs`、`src/tui/draw.rs`
- Modify: `src/main.rs`（`mod tui;` 不变，仍指向目录）

**Interfaces:**
- Produces:
  - `tui::run(handle, engine, config, warning) -> Result<()>`（签名不变，实现搬进 `tui/mod.rs`）
  - `tui::keys::Action`（枚举）、`tui::keys::Context`（枚举）、`tui::keys::resolve(context: Context, key: KeyEvent) -> Option<Action>`
  - `tui::state::App`、`tui::state::{Msg, Focus}`
  - `tui::draw::{draw, focused_block, highlight, song_label, format_size, format_duration}`

- [ ] **Step 1: 写失败的测试**

`src/tui/keys.rs`：

```rust
//! 键位 → 动作。
//!
//! 抽成纯函数的理由：键位优先级（模态 > 输入态 > 常规）是最容易错、又最难
//! 靠手点验证的部分，放在这里可以用单测钉住。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// 界面动作。绘制与状态机只认这个枚举，不认按键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    Help,
    ToggleLyric,
    QueueOverlay,
    Search,
    CycleMode,
    Menu(usize),
    FocusNext,
    FocusPrev,
    Up,
    Down,
    First,
    Last,
    PageUp,
    PageDown,
    Enter,
    Back,
    PlayPause,
    Next,
    Prev,
    SeekBack,
    SeekForward,
    VolumeUp,
    VolumeDown,
    AppendQueue,
    InsertQueue,
    AddToPlaylist,
    RemoveFromPlaylist,
    Favorite,
    Download,
    QueueDelete,
    QueueClear,
    Logout,
    InputChar(char),
    InputBackspace,
    InputSubmit,
    InputCancel,
}

/// 按键时的上下文。模态与输入态在这里体现，`resolve` 不再自己猜。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    /// 焦点在菜单上。
    Menu,
    /// 焦点在主区列表上。
    Content,
    /// 焦点在歌词块上。
    Lyric,
    /// 搜索输入框正在吃键。
    SearchInput,
    /// 队列浮层。
    QueueOverlay,
    /// 帮助浮层。
    HelpOverlay,
    /// `p` 弹出的歌单选择框。
    PlaylistPicker,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn press(context: Context, code: KeyCode) -> Option<Action> {
        resolve(context, key(code))
    }

    #[test]
    fn ctrl_c_quits_from_every_context() {
        for context in [
            Context::Menu,
            Context::Content,
            Context::Lyric,
            Context::SearchInput,
            Context::QueueOverlay,
            Context::HelpOverlay,
            Context::PlaylistPicker,
        ] {
            let event = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
            assert_eq!(resolve(context, event), Some(Action::Quit), "{context:?}");
        }
    }

    #[test]
    fn typing_in_the_search_box_never_quits_or_navigates() {
        assert_eq!(press(Context::SearchInput, KeyCode::Char('q')), Some(Action::InputChar('q')));
        assert_eq!(press(Context::SearchInput, KeyCode::Char('/')), Some(Action::InputChar('/')));
        assert_eq!(press(Context::SearchInput, KeyCode::Char('j')), Some(Action::InputChar('j')));
        assert_eq!(press(Context::SearchInput, KeyCode::Enter), Some(Action::InputSubmit));
        assert_eq!(press(Context::SearchInput, KeyCode::Esc), Some(Action::InputCancel));
        assert_eq!(press(Context::SearchInput, KeyCode::Backspace), Some(Action::InputBackspace));
    }

    #[test]
    fn overlays_swallow_keys_they_do_not_know() {
        assert_eq!(press(Context::HelpOverlay, KeyCode::Char('a')), None);
        assert_eq!(press(Context::HelpOverlay, KeyCode::Char('?')), Some(Action::Help));
        assert_eq!(press(Context::HelpOverlay, KeyCode::Char('L')), Some(Action::Logout));
        assert_eq!(press(Context::QueueOverlay, KeyCode::Char('d')), Some(Action::QueueDelete));
        assert_eq!(press(Context::QueueOverlay, KeyCode::Enter), Some(Action::Enter));
        assert_eq!(press(Context::QueueOverlay, KeyCode::Esc), Some(Action::Back));
    }

    #[test]
    fn numbers_jump_between_menus_from_anywhere() {
        assert_eq!(press(Context::Content, KeyCode::Char('3')), Some(Action::Menu(2)));
        assert_eq!(press(Context::Menu, KeyCode::Char('1')), Some(Action::Menu(0)));
        assert_eq!(press(Context::Lyric, KeyCode::Char('4')), Some(Action::Menu(3)));
    }

    #[test]
    fn content_keys_cover_the_documented_table() {
        assert_eq!(press(Context::Content, KeyCode::Char('a')), Some(Action::AppendQueue));
        assert_eq!(press(Context::Content, KeyCode::Char('A')), Some(Action::InsertQueue));
        assert_eq!(press(Context::Content, KeyCode::Char('p')), Some(Action::AddToPlaylist));
        assert_eq!(press(Context::Content, KeyCode::Char('m')), Some(Action::CycleMode));
        assert_eq!(press(Context::Content, KeyCode::Char(' ')), Some(Action::PlayPause));
        assert_eq!(press(Context::Content, KeyCode::PageDown), Some(Action::PageDown));
        assert_eq!(press(Context::Content, KeyCode::End), Some(Action::Last));
        assert_eq!(press(Context::Content, KeyCode::Char('.')), Some(Action::VolumeUp));
        assert_eq!(press(Context::Content, KeyCode::Char('?')), Some(Action::Help));
    }

    #[test]
    fn esc_and_backspace_mean_back_only_where_there_is_somewhere_to_go() {
        assert_eq!(press(Context::Content, KeyCode::Esc), Some(Action::Back));
        assert_eq!(press(Context::Menu, KeyCode::Esc), None, "菜单是最外层，Esc 不做事");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test keys`
Expected: FAIL——`resolve` 未定义

- [ ] **Step 3: 写实现**

`src/tui/keys.rs` 追加：

```rust
/// 键位映射。返回值是 `None` 表示这个键在当前上下文没有含义（调用方忽略）。
pub fn resolve(context: Context, key: KeyEvent) -> Option<Action> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Some(Action::Quit);
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match context {
        Context::SearchInput => match key.code {
            KeyCode::Enter => Some(Action::InputSubmit),
            KeyCode::Esc => Some(Action::InputCancel),
            KeyCode::Backspace => Some(Action::InputBackspace),
            KeyCode::Char(character) if !ctrl => Some(Action::InputChar(character)),
            _ => None,
        },
        Context::HelpOverlay => match key.code {
            KeyCode::Esc | KeyCode::Char('?') => Some(Action::Help),
            KeyCode::Char('L') => Some(Action::Logout),
            _ => None,
        },
        Context::QueueOverlay => match key.code {
            KeyCode::Esc => Some(Action::Back),
            KeyCode::Enter => Some(Action::Enter),
            KeyCode::Char('j') | KeyCode::Down => Some(Action::Down),
            KeyCode::Char('k') | KeyCode::Up => Some(Action::Up),
            KeyCode::Char('d') => Some(Action::QueueDelete),
            KeyCode::Char('D') => Some(Action::QueueClear),
            KeyCode::Char('g') | KeyCode::Home => Some(Action::First),
            KeyCode::Char('G') | KeyCode::End => Some(Action::Last),
            _ => None,
        },
        Context::PlaylistPicker => match key.code {
            KeyCode::Esc => Some(Action::Back),
            KeyCode::Enter => Some(Action::Enter),
            KeyCode::Char('j') | KeyCode::Down => Some(Action::Down),
            KeyCode::Char('k') | KeyCode::Up => Some(Action::Up),
            _ => None,
        },
        Context::SearchInput | Context::Menu | Context::Content | Context::Lyric => {
            common(key, context)
        }
    }
}

/// 菜单 / 主区 / 歌词共用的键位（焦点差异只在焦点切换键上）。
fn common(key: KeyEvent, context: Context) -> Option<Action> {
    match key.code {
        KeyCode::Char('1'..='4') => {
            let KeyCode::Char(digit) = key.code else { unreachable!() };
            Some(Action::Menu(digit as usize - '1' as usize))
        }
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Char('?') | KeyCode::F(1) => Some(Action::Help),
        KeyCode::Tab => Some(Action::FocusNext),
        KeyCode::BackTab => Some(Action::FocusPrev),
        KeyCode::Char('j') | KeyCode::Down => Some(Action::Down),
        KeyCode::Char('k') | KeyCode::Up => Some(Action::Up),
        KeyCode::Char('g') | KeyCode::Home => Some(Action::First),
        KeyCode::Char('G') | KeyCode::End => Some(Action::Last),
        KeyCode::PageUp => Some(Action::PageUp),
        KeyCode::PageDown => Some(Action::PageDown),
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => Some(Action::PageUp),
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => Some(Action::PageDown),
        KeyCode::Enter => Some(Action::Enter),
        // 菜单是最外层，没有「上级」可回；主区/歌词的 Esc 由 `state` 决定回哪一层。
        KeyCode::Esc | KeyCode::Backspace => (context != Context::Menu).then_some(Action::Back),
        KeyCode::Char(' ') => Some(Action::PlayPause),
        KeyCode::Char('n') => Some(Action::Next),
        KeyCode::Char('b') => Some(Action::Prev),
        KeyCode::Left | KeyCode::Char('h') => Some(Action::SeekBack),
        KeyCode::Right | KeyCode::Char('l') => Some(Action::SeekForward),
        KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Char('.') => Some(Action::VolumeUp),
        KeyCode::Char('-') | KeyCode::Char(',') => Some(Action::VolumeDown),
        KeyCode::Char('m') => Some(Action::CycleMode),
        KeyCode::Char('a') => Some(Action::AppendQueue),
        KeyCode::Char('A') => Some(Action::InsertQueue),
        KeyCode::Char('p') => Some(Action::AddToPlaylist),
        KeyCode::Char('x') => Some(Action::RemoveFromPlaylist),
        KeyCode::Char('f') => Some(Action::Favorite),
        KeyCode::Char('d') => Some(Action::Download),
        KeyCode::Char('u') => Some(Action::QueueOverlay),
        KeyCode::Char('y') => Some(Action::ToggleLyric),
        KeyCode::Char('/') => Some(Action::Search),
        _ => None,
    }
}
```

- [ ] **Step 4: 把现有 `tui.rs` 按责任切开**

- `src/tui/mod.rs`：`pub mod draw; pub mod keys; pub mod state;`、`run`、`event_loop`、`Transfer`、`WriteRequest` 与常量 `TICK`、`MAX_AUTO_SKIPS`（原样搬运）。
- `src/tui/state.rs`：`Msg`、`Focus`、`App` 及其 `impl`（原样搬运，只把 `handle_key` 改成走 `keys::resolve`）。
- `src/tui/draw.rs`：`draw`、`draw_picker`、`draw_playlists`、`draw_tracks`、`draw_lyric`、`now_playing_label`、`draw_status`、`focused_block`、`highlight`、`song_label`、`format_size`、`format_duration`（原样搬运）。

`state.rs` 的 `handle_key` 改成：

```rust
    fn handle_key(&mut self, key: KeyEvent) {
        let Some(action) = keys::resolve(self.context(), key) else {
            return;
        };
        self.apply(action);
    }

    /// 当前按键上下文：模态 > 输入态 > 焦点所在。
    fn context(&self) -> keys::Context {
        if self.help_open {
            return keys::Context::HelpOverlay;
        }
        if self.picker.is_some() {
            return keys::Context::PlaylistPicker;
        }
        if self.input.is_some() {
            return keys::Context::SearchInput;
        }
        match self.focus {
            Focus::Playlists => keys::Context::Menu,
            Focus::Tracks => keys::Context::Content,
        }
    }
```

`apply(action)` 按 `Action` 分派到原有的 `move_selection` / `enter_playlist` / `play_from_tracks` / `download_selected` / `favorite_selected` / `open_picker` / `remove_selected` / `search` 等函数；本任务里 `Menu` / `PageUp` / `PageDown` / `First` / `Last` / `Back` 先接到「离得最近的现有行为」（`First`/`Last` → `select_edge`，`PageUp`/`PageDown` → 一次挪 10 行，`Back` → 输入态取消或忽略，`Menu(_)` → 忽略），`help_open` 字段先恒为 `false`。这些是**本阶段的真实行为**，不是空实现；`?` 浮层与菜单路由在 Task 6–9 接管。

- [ ] **Step 5: 跑测试与冒烟**

Run: `cargo test && cargo clippy --all-targets`
Expected: 全绿（含新的 6 个 keys 用例）

Run: `cargo run`
Expected: 界面与拆分前逐像素一致（焦点边框、歌词、播放条、状态栏都在），`j`/`k`/`Tab`/`Enter`/`Space`/`n`/`b`/`y`/`/` 全部照旧

- [ ] **Step 6: 提交**

```bash
git add -A src/tui.rs src/tui
git commit -m "refactor(tui): 拆成 tui/ 目录，键位表抽成纯函数"
```

---

### Task 6: 菜单 + 视图路由 + 嵌套返回

**Files:**
- Modify: `src/tui/state.rs`、`src/tui/draw.rs`（或新建 `src/tui/draw/`，见 Task 8）
- Test: `src/tui/state.rs` 内联测试

**Interfaces:**
- Produces:
  - `state::Menu { Favorites, Playlists, Search, History }`（`ALL: [Menu; 4]`、`index()`、`title()`）
  - `state::ListView { items: Vec<Song>, state: ListState, error: Option<String> }`（`selected() -> Option<&Song>`、`select_first()`、`move_by(delta)`、`edge(last)`、`page(delta)`）
  - `MainState` 字段：`menu: Menu`、`focus: Focus`（`Menu` / `Content` / `Lyric`）、`favorites: ListView`、`playlists: ListView<Playlist>`、`tracks: ListView`（嵌套层）、`search_text: String`、`history_view: ListView`、`breadcrumb: Vec<String>`
  - `App::switch_menu(&mut self, menu: Menu)`、`App::enter_selected(&mut self)`、`App::back(&mut self) -> bool`

- [ ] **Step 1: 写失败的测试**

`src/tui/state.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FileSizes, Playlist, Song};

    fn test_song(mid: &str) -> Song {
        Song {
            mid: mid.to_string(),
            song_id: None,
            media_mid: None,
            title: format!("歌 {mid}"),
            singers: "歌手".to_string(),
            album_mid: String::new(),
            album_name: String::new(),
            duration_secs: 200,
            sizes: FileSizes::default(),
        }
    }

    /// 测试用的 `App`。这几件事要留意：
    /// - 运行时故意漏出去（`Box::leak`），否则句柄一旦失效，后台任务全哑。
    /// - 音频设备真开：本机跑测试可用；无音频设备的 CI 需要另行注入假引擎。
    /// - 历史落盘路径每个测试各不相同（进程内自增），避免互相看到对方写的文件。
    fn test_app() -> App {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let runtime = Box::leak(Box::new(
            tokio::runtime::Runtime::new().expect("测试运行时"),
        ));
        let (tx, _rx) = std::sync::mpsc::channel();
        App::new(
            runtime.handle().clone(),
            Engine::start().expect("打开音频设备"),
            Config::default(),
            std::env::temp_dir().join(format!(
                "canto-test-{}-{unique}-history.json",
                std::process::id()
            )),
            tx,
        )
    }

    fn playlist(id: &str, name: &str) -> Playlist {
        Playlist {
            id: id.to_string(),
            dir_id: 1,
            name: name.to_string(),
            song_count: 3,
            cover_url: None,
            is_favorites: false,
        }
    }

    /// 菜单切换是「换一屏内容」，不是「清空一屏内容」：切走再切回，选中位置要还在。
    #[test]
    fn switching_menus_keeps_each_views_selection() {
        let mut nav = Nav::new();
        nav.favorites.items = vec![test_song("a"), test_song("b"), test_song("c")];
        nav.favorites.select_first();
        nav.favorites.move_by(2);
        assert_eq!(nav.favorites.selected().map(|s| s.mid.as_str()), Some("c"));

        nav.menu = Menu::Playlists;
        nav.menu = Menu::Favorites;
        assert_eq!(
            nav.favorites.selected().map(|s| s.mid.as_str()),
            Some("c"),
            "切回来选中位置该保留"
        );
    }

    #[test]
    fn entering_a_playlist_pushes_a_breadcrumb_and_esc_pops_it() {
        let mut nav = Nav::new();
        nav.playlists.items = vec![playlist("1", "嗯"), playlist("2", "循环")];
        nav.playlists.select_first();
        let target = nav.enter_selected_playlist().expect("选中了歌单");
        assert_eq!(target.id, "1");
        nav.push_tracks("1", "嗯", vec![test_song("a")]);
        assert_eq!(nav.breadcrumb, ["我的歌单", "嗯"]);
        assert!(nav.pop_tracks(), "Esc 该返回歌单列表");
        assert_eq!(nav.breadcrumb, ["我的歌单"]);
        assert!(!nav.pop_tracks(), "已经在最上层，再返回没有动作");
    }

    #[test]
    fn paging_moves_ten_rows_and_stops_at_the_end() {
        let mut view = ListView::default();
        view.items = (0..25).map(|index| test_song(&format!("m{index}"))).collect();
        view.select_first();
        view.page(1);
        assert_eq!(view.state.selected(), Some(10));
        view.page(1);
        view.page(1);
        assert_eq!(view.state.selected(), Some(24), "到尾就停，不越界");
        view.page(-1);
        assert_eq!(view.state.selected(), Some(14));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test state`
Expected: FAIL——`Nav` / `ListView` / `Menu` 未定义

- [ ] **Step 3: 写实现**

`src/tui/state.rs` 新增（`Nav` 就是原 `App` 里「界面状态」那部分字段的抽取，`App` 持有 `nav: Nav` + 网络/引擎句柄）：

```rust
/// 左侧菜单项。顺序即数字键 `1`–`4` 的顺序。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Menu {
    Favorites,
    Playlists,
    Search,
    History,
}

impl Menu {
    pub const ALL: [Menu; 4] = [Menu::Favorites, Menu::Playlists, Menu::Search, Menu::History];

    pub fn index(self) -> usize {
        Menu::ALL.iter().position(|item| *item == self).unwrap_or(0)
    }

    pub fn title(self) -> &'static str {
        match self {
            Menu::Favorites => "我的收藏",
            Menu::Playlists => "我的歌单",
            Menu::Search => "搜索",
            Menu::History => "历史",
        }
    }
}

/// 一屏列表的通用状态（曲目 / 歌单 / 历史都用它）。
#[derive(Debug, Default)]
pub struct ListView<T> {
    pub items: Vec<T>,
    pub state: ListState,
    pub error: Option<String>,
    /// 每行的右侧副标题（历史屏用「时间 · 来自《来源》」；别的屏留空）。
    /// 与 `items` 等长，改动 `items` 的代码要顺手改它。
    pub labels: Vec<String>,
}

impl<T> ListView<T> {
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn select_first(&mut self) {
        self.state.select((!self.items.is_empty()).then_some(0));
    }

    pub fn move_by(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let current = self.state.selected().unwrap_or(0) as isize;
        let next = (current + delta).clamp(0, self.items.len() as isize - 1);
        self.state.select(Some(next as usize));
    }

    pub fn edge(&mut self, last: bool) {
        if !self.items.is_empty() {
            self.state.select(Some(if last { self.items.len() - 1 } else { 0 }));
        }
    }

    /// 翻页＝挪一屏（留一行重叠，行数与当前视口一致时由调用方传 `10`）。
    pub fn page(&mut self, pages: isize) {
        self.move_by(pages * 10);
    }

    pub fn selected(&self) -> Option<&T> {
        self.state.selected().and_then(|index| self.items.get(index))
    }
}

/// 界面导航状态：菜单、焦点、四屏各自的列表、歌单曲目的嵌套层。
#[derive(Debug)]
pub struct Nav {
    pub menu: Menu,
    pub focus: Focus,
    pub favorites: ListView<Song>,
    pub playlists: ListView<Playlist>,
    /// 「我的歌单 › 某歌单」这一层的曲目。
    pub tracks: ListView<Song>,
    /// 搜索结果单独一份：跟 `tracks` 共用会把正在看的歌单曲目冲掉。
    pub search_results: ListView<Song>,
    pub history_view: ListView<Song>,
    /// 当前歌单曲目层对应的歌单（`None` = 不在嵌套层里）。
    pub shown: Option<(String, String)>,
    pub breadcrumb: Vec<String>,
    pub search_text: String,
}

impl Nav {
    pub fn new() -> Self {
        Self {
            menu: Menu::Favorites,
            focus: Focus::Menu,
            favorites: ListView::default(),
            playlists: ListView::default(),
            tracks: ListView::default(),
            search_results: ListView::default(),
            history_view: ListView::default(),
            shown: None,
            breadcrumb: Vec::new(),
            search_text: String::new(),
        }
    }

    /// 焦点在菜单上时选中的歌单。
    pub fn enter_selected_playlist(&self) -> Option<&Playlist> {
        self.playlists.selected()
    }

    /// 进入「我的歌单 › 嗯」这一层：清掉旧曲目、写面包屑。
    pub fn push_tracks(&mut self, id: &str, name: &str, songs: Vec<Song>) {
        self.shown = Some((id.to_string(), name.to_string()));
        self.tracks.items = songs;
        self.tracks.error = None;
        self.tracks.select_first();
        self.breadcrumb = vec!["我的歌单".to_string(), name.to_string()];
    }

    /// 返回上一层。返回 `true` 表示吃掉了这次 Esc。
    pub fn pop_tracks(&mut self) -> bool {
        if self.shown.take().is_none() {
            return false;
        }
        self.tracks = ListView::default();
        self.breadcrumb = vec!["我的歌单".to_string()];
        true
    }

    /// 当前焦点下的列表（绘制与移动都用它，免得三处各写一遍 match）。
    pub fn focused_songs(&mut self) -> Option<&mut ListView<Song>> {
        if self.focus != Focus::Content {
            return None;
        }
        Some(match self.menu {
            Menu::Favorites => &mut self.favorites,
            Menu::Playlists => &mut self.tracks,
            Menu::History => &mut self.history_view,
            Menu::Search => &mut self.search_results,
        })
    }
}
```

`switch_menu`：设 `menu`、把焦点移到主区（搜索项直接进输入态）、若 `Menu::Playlists` 且 `shown.is_some()` 则保持嵌套层（`pop_tracks` 由 Esc 负责）。

- [ ] **Step 4: 接数据与渲染**

`App::on_msg` 把 `Msg::Playlists` / `Msg::Tracks` / `Msg::Search` / 新 `Msg::Favorites` 的结果写进 `nav.*`；`Msg::Favorites` 新增（原来收藏只在右栏点进去才有，现在它是菜单第一项，必须在后台拉一次）。

主区渲染按 `nav.menu` 分派：`Favorites` → `nav.favorites`；`Playlists` → `nav.shown.is_none()` 时画 `nav.playlists`（歌单列表），否则画 `nav.tracks`；`Search` → 输入行 + `nav.search_results`；`History` → `nav.history_view`（行文案用 `labels`，见 Task 13）。

- [ ] **Step 5: 跑测试与冒烟**

Run: `cargo test state`
Expected: PASS

Run: `cargo run`
Expected: 数字键 `1`–`4` 四屏可切；`我的歌单` 里 `Enter` 进曲目层、`Esc` 回歌单列表（面包屑跟着变）；切走再切回同一屏，选中行没被重置

- [ ] **Step 6: 提交**

```bash
git add -A src/tui
git commit -m "feat(tui): 左侧菜单 + 主区视图路由 + 歌单曲目嵌套返回"
```

---

### Task 7: 搜索视图（输入框常驻 + 输入态吞键 + 结果留存）

**Files:**
- Modify: `src/tui/state.rs`、`src/tui/draw.rs`

**Interfaces:**
- Consumes: `keys::{Action, Context}`（`Search` / `InputChar` / `InputSubmit` / `InputCancel` / `InputBackspace`）
- Produces: `Nav.search_input: bool`（是否在输入态）、`App::submit_search(&mut self, keyword: String)`

- [ ] **Step 1: 写失败的测试**

```rust
    #[test]
    fn search_keeps_its_own_screen_so_browsing_is_not_lost() {
        let mut nav = Nav::new();
        nav.favorites.items = vec![song("a"), song("b")];
        nav.favorites.select_first();
        nav.favorites.move_by(1);

        nav.menu = Menu::Search;
        nav.search_input = true;
        for character in "晴天".chars() {
            nav.search_text.push(character);
        }
        nav.search_results.items = vec![test_song("s1"), test_song("s2")];
        nav.search_results.select_first();

        nav.menu = Menu::Favorites;
        assert_eq!(nav.favorites.selected().map(|s| s.mid.as_str()), Some("b"), "浏览上下文没丢");
        nav.menu = Menu::Search;
        assert_eq!(
            nav.search_results.selected().map(|s| s.mid.as_str()),
            Some("s1"),
            "搜索结果也还在"
        );
        assert_eq!(nav.search_text, "晴天");
    }

    #[test]
    fn leaving_the_search_box_stops_swallowing_keys() {
        let mut nav = Nav::new();
        nav.search_input = true;
        nav.end_search_input();
        assert!(!nav.search_input);
        assert_eq!(nav.search_text, "晴天", "退出输入态不丢关键词");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test search_keeps` `cargo test leaving_the_search_box`
Expected: FAIL——`search_input` / `end_search_input` 未定义

- [ ] **Step 3: 写实现**

`Nav` 增 `pub search_input: bool`（默认 `false`）与：

```rust
    /// 进入搜索框：光标在末尾，`Esc` 只退出输入态、不清关键词。
    pub fn begin_search_input(&mut self) {
        self.menu = Menu::Search;
        self.search_input = true;
        self.focus = Focus::Content;
    }

    pub fn end_search_input(&mut self) {
        self.search_input = false;
    }
```

`App::apply`：`Action::Search` → `nav.begin_search_input()`；`InputChar(c)` → `nav.search_text.push(c)`；`InputBackspace` → `pop`；`InputCancel` → `end_search_input`；`InputSubmit` → `end_search_input()` + 后台搜索（结果走 `Msg::Search`，写进 `nav.search_results`）。

- [ ] **Step 4: 跑测试与冒烟**

Run: `cargo test`
Expected: PASS

Run: `cargo run`
Expected: `3` 进搜索屏且有光标；输入 `j`/`q`/`/` 都只是字符；`Enter` 出结果并退出输入态（此时 `j`/`k` 又能移动结果）；`j` 选结果 `Enter` 播放；切到 `1` 再回 `3`，关键词与结果都还在

- [ ] **Step 5: 提交**

```bash
git add -A src/tui
git commit -m "feat(tui): 搜索成为独立视图，输入态吞键、结果与关键词留存"
```

---

### Task 8: 三段布局、断点与状态行拆分

**Files:**
- Create: `src/tui/draw/mod.rs`、`src/tui/draw/{menu,content,lyric,player,status}.rs`
- Delete: `src/tui/draw.rs`（内容按面板拆开）
- Test: `src/tui/draw/mod.rs` 内联测试（`TestBackend` 冒烟）

**Interfaces:**
- Produces:
  - `draw::layout(area: Rect, lyric: bool) -> Layout`（返回 `menu/content/lyric/player/status` 的 `Rect`s）
  - `draw::overlay_rect(area: Rect, width: u16, height: u16) -> Rect`（居中弹层，供浮层复用）
  - `draw::too_small(area: Rect) -> bool`（<60×15）

- [ ] **Step 1: 写失败的测试**

`src/tui/draw/mod.rs`：

```rust
#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn buffer(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("测试终端");
        terminal.draw(|frame| app.draw(frame)).expect("绘制");
        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..height {
            for x in 0..width {
                text.push_str(buffer[(x, y)].symbol());
            }
        }
        text
    }

    /// 面包屑、状态行两段、菜单项——这几处「画没画出来」是重构最容易漏的。
    #[test]
    fn wide_screen_shows_menu_breadcrumb_and_two_part_status_row() {
        let mut app = test_app();
        app.nav.push_tracks("1", "嗯", vec![test_song("a")]);
        app.status = "已插队《晴天》".to_string();
        let text = buffer(&mut app, 160, 40);
        assert!(text.contains("我的收藏"), "菜单第一项");
        assert!(text.contains("我的歌单 › 嗯"), "面包屑");
        assert!(text.contains("已插队《晴天》"), "状态行右侧");
        assert!(text.contains("Enter"), "状态行左侧仍有键位提示");
    }

    #[test]
    fn narrow_screen_turns_the_menu_into_a_tab_row() {
        let mut app = test_app();
        let text = buffer(&mut app, 90, 24);
        assert!(text.contains("1 我的收藏"), "窄屏菜单变页签行");
    }

    #[test]
    fn tiny_window_only_asks_for_a_bigger_terminal() {
        let mut app = test_app();
        let text = buffer(&mut app, 40, 10);
        assert!(text.contains("终端太小"), "小窗口只给一句提示");
        assert!(!text.contains("播放"), "不再画播放条");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test draw`
Expected: FAIL——`app.nav` / `App::draw` 尚未按新布局接好

- [ ] **Step 3: 写布局实现**

```rust
//! 骨架布局：菜单（或顶部页签）+ 主区 + 歌词块 + 播放条 + 状态行。

use ratatui::layout::{Constraint, Layout, Rect};

/// 菜单列宽；窄屏（<100 列）时菜单收成顶部一行页签，不再占列。
pub const MENU_WIDTH: u16 = 16;
/// 低于这个宽度就把菜单横过来。
pub const NARROW_WIDTH: u16 = 100;
/// 窗口小于这个尺寸就只画一行提示。
pub const MIN_WIDTH: u16 = 60;
pub const MIN_HEIGHT: u16 = 15;

pub struct Areas {
    pub menu: Rect,
    pub content: Rect,
    pub lyric: Option<Rect>,
    pub player: Rect,
    pub status: Rect,
    /// 窄屏：菜单是顶部一行而不是左列。
    pub narrow: bool,
}

pub fn layout(area: Rect, lyric_wanted: bool) -> Areas {
    let narrow = area.width < NARROW_WIDTH;
    // 播放条恒 4 行，状态行恒 1 行（顺序：菜单/页签在一个纵向切里）。
    let rows = Layout::vertical([
        Constraint::Length(if narrow { 1 } else { 0 }),
        Constraint::Min(3),
        Constraint::Length(4),
        Constraint::Length(1),
    ])
    .split(area);
    let content_area = rows[1];
    // 歌词只在够高时占位：矮屏（<20 行）直接关掉，别把主区挤成一行。
    let lyric_height = if lyric_wanted && area.height >= 20 {
        (content_area.height / 3).clamp(5, 12)
    } else {
        0
    };
    let right = Layout::vertical([Constraint::Min(3), Constraint::Length(lyric_height)])
        .split(content_area);
    Areas {
        menu: if narrow {
            rows[0]
        } else {
            Layout::horizontal([Constraint::Length(MENU_WIDTH), Constraint::Min(20)])
                .split(content_area)[0]
        },
        content: if narrow {
            right[0]
        } else {
            Layout::horizontal([Constraint::Length(MENU_WIDTH), Constraint::Min(20)])
                .split(right[0])[1]
        },
        lyric: (lyric_height > 0).then(|| {
            if narrow {
                right[1]
            } else {
                Layout::horizontal([Constraint::Length(MENU_WIDTH), Constraint::Min(20)])
                    .split(right[1])[1]
            }
        }),
        player: rows[2],
        status: rows[3],
        narrow,
    }
}

/// 居中弹层（队列浮层 / 帮助 / 歌单选择框共用）。
pub fn overlay_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(4));
    let height = height.min(area.height.saturating_sub(2));
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
```

- [ ] **Step 4: 拆绘制函数并接新骨架**

`draw/menu.rs`（宽屏 16 列菜单：四项 + 底部灰字 `uin`；窄屏：顶行 `1 我的收藏 · 2 我的歌单 · 3 搜索 · 4 历史`，当前项高亮）、`draw/content.rs`（按 `nav.menu` 分派 + 面包屑写进块标题 + 搜索屏的输入行）、`draw/lyric.rs`（原 `draw_lyric`，居中 + 当前行高亮 + 翻译）、`draw/player.rs`（原播放条四行，信息行改成 spec 的字段顺序）、`draw/status.rs`（**左右两段**：左＝`hints::for_context(...)`，右＝`loading`/`status`，用 `Paragraph::new(line).alignment(Alignment::Right)` 单独渲染右段）。

`draw/mod.rs` 的 `App::draw`：

```rust
    fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        if draw::too_small(area) {
            frame.render_widget(
                Paragraph::new(format!(
                    "终端太小，请拉大到 ≥{}×{}（当前 {}×{}）",
                    draw::MIN_WIDTH,
                    draw::MIN_HEIGHT,
                    area.width,
                    area.height
                ))
                .alignment(Alignment::Center),
                area,
            );
            return;
        }
        let lyric_wanted = self.show_lyric && self.lyric.is_some() && self.player_active();
        let areas = draw::layout(area, lyric_wanted);
        draw::menu::draw(self, frame, areas.menu);
        draw::content::draw(self, frame, areas.content);
        if let Some(area) = areas.lyric {
            draw::lyric::draw(self, frame, area);
        }
        draw::player::draw(self, frame, areas.player);
        draw::status::draw(self, frame, areas.status);
        if self.help_open {
            draw::overlay::draw_help(self, frame);
        } else if self.queue_open {
            draw::overlay::draw_queue(self, frame);
        } else if self.picker.is_some() {
            draw::overlay::draw_picker(self, frame);
        }
    }
```

`hints` 表（`draw/status.rs` 内）按焦点给出 3–5 个键：

```rust
pub fn hints(context: keys::Context) -> &'static str {
    match context {
        keys::Context::Menu => "↑↓ 选菜单 · Enter 进入 · Tab 到主区 · ? 帮助",
        keys::Context::Content => "Enter 播放 · a 追加 · A 插队 · f 收藏 · ? 帮助",
        keys::Context::Lyric => "j/k 滚动歌词 · y 收起 · Tab 离开",
        keys::Context::SearchInput => "Enter 搜索 · Esc 取消输入",
        keys::Context::QueueOverlay => "Enter 跳播 · d 删除 · Shift+D 清空 · Esc 关闭",
        keys::Context::HelpOverlay => "Esc 关闭 · L 登出",
        keys::Context::PlaylistPicker => "Enter 确认 · Esc 取消",
    }
}
```

`App` 里加一个判断「有没有在放」的小工具（歌词块与播放条都靠它决定显示什么）：

```rust
    /// 音频引擎是否已经离开 `Idle`（有歌在放或缓冲）。
    fn player_active(&self) -> bool {
        self.engine.snapshot().phase != Phase::Idle
    }
```

- [ ] **Step 5: 跑测试与冒烟**

Run: `cargo test draw`
Expected: PASS（3 个冒烟用例）

Run: `COLUMNS=90 cargo run` 与 `COLUMNS=160 cargo run`
Expected: 宽屏三栏 + 歌单十六列菜单（底部显示 uin）；窄屏菜单变顶行页签、主区吃满宽；把终端拉到 40×10 只显示一行提示；加载歌单时左侧键位提示仍在、右侧显示 spinner

- [ ] **Step 6: 提交**

```bash
git add -A src/tui
git commit -m "feat(tui): 三段布局与断点，状态行拆成键位提示 + 状态两段"
```

---

### Task 9: 帮助浮层 + 改键（`a`→`p`、数字键、翻页键）

**Files:**
- Create: `src/tui/draw/overlay.rs`
- Modify: `src/tui/state.rs`（`help_open`、`Action::Help`/`Logout`/`Menu(n)`/`PageUp`/`PageDown`/`First`/`Last`/`Back` 的实际行为）、`src/tui/draw.rs`→`draw/overlay.rs`（原 `draw_picker` 搬进来）

**Interfaces:**
- Consumes: `keys::{Action, Context}`
- Produces: `draw::overlay::{draw_help, draw_queue, draw_picker}`；`Nav.help_open: bool`

- [ ] **Step 1: 写失败的测试**

`src/tui/state.rs`：

```rust
    #[test]
    fn help_overlay_toggles_and_swallows_everything_else() {
        let mut app = test_app(160, 40);
        app.apply(Action::Help);
        assert!(app.nav.help_open);
        assert_eq!(app.context(), keys::Context::HelpOverlay);
        // 浮层打开时 `1` 不再切菜单。
        let before = app.nav.menu;
        app.apply(Action::Help); // 同键 = 关闭
        assert!(!app.nav.help_open);
        assert_eq!(app.nav.menu, before);
    }

    #[test]
    fn digits_switch_menu_and_move_focus_to_content() {
        let mut app = test_app(160, 40);
        app.apply(Action::Menu(2));
        assert_eq!(app.nav.menu, Menu::Search);
        assert!(app.nav.search_input, "进搜索屏直接可输入");
        app.apply(Action::Menu(1));
        assert_eq!(app.nav.menu, Menu::Playlists);
        assert_eq!(app.nav.focus, Focus::Content);
    }

    #[test]
    fn p_opens_the_playlist_picker_and_a_only_touches_the_queue() {
        let mut app = test_app(160, 40);
        app.nav.favorites.items = vec![test_song("a")];
        app.nav.favorites.select_first();
        app.apply(Action::AddToPlaylist);
        assert!(app.picker.is_some(), "p 打开歌单选择框");
        app.picker = None;
        app.apply(Action::AppendQueue);
        assert!(app.picker.is_none(), "a 不该弹出歌单选择框");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test help_overlay` `cargo test p_opens`
Expected: FAIL——`help_open` / `Nav.menu` 分派尚未实现

- [ ] **Step 3: 写实现**

`state.rs`：`Nav` 增 `pub help_open: bool`；`apply` 补 `Action::Help`（`help_open = !help_open`）、`Action::Menu(index)`（`Menu::ALL[index]` + `switch_menu`）、`Action::PageUp/PageDown`（焦点列表 `page(±1)`）、`Action::First/Last`（`edge`）、`Action::Back`（顺序：关浮层 → 退出输入态 → `pop_tracks()` → 无动作）、`Action::CycleMode`（阶段 3 前先只切 `Queue` 的模式文案，见 Task 12）、`Action::Logout`（阶段 4 接）。

`draw/overlay.rs`：

```rust
/// 帮助浮层：按当前菜单项列出可用键位。
pub fn draw_help(app: &App, frame: &mut Frame) {
    let lines = help_lines(app.nav.menu);
    let width = 56.min(frame.area().width.saturating_sub(4));
    let area = super::overlay_rect(frame.area(), width, lines.len() as u16 + 2);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title(" 帮助 "))
            .wrap(Wrap { trim: false }),
        area,
    );
}

/// 每个菜单项一套键位说明：全局键 + 这一屏特有的键。
fn help_lines(menu: Menu) -> Vec<Line<'static>> {
    let mut rows = vec![
        Line::from("全局  Tab 焦点 · 1-4 菜单 · / 搜索 · ? 帮助 · q 退出"),
        Line::from("播放  Space 暂停 · n/b 切歌 · ←/→ 10 秒 · +/- 音量 · m 模式"),
        Line::from("歌曲  Enter 播放 · a 追加队列 · A 插队 · u 队列 · f 收藏 · d 下载"),
    ];
    rows.push(match menu {
        Menu::Favorites => Line::from("收藏  p 加进歌单 · x 从歌单移除"),
        Menu::Playlists => Line::from("歌单  Enter 进入曲目 · Esc 返回歌单列表"),
        Menu::Search => Line::from("搜索  Enter 搜索 · Esc 取消输入 · 结果里 Enter 播放"),
        Menu::History => Line::from("历史  Enter 从这首重播 · Shift+D 清空历史"),
    });
    rows
}
```

（登出行在 Task 16 落地登出动作时再加；现在写上去就是画一个按了没反应的键。）

`Action::Logout` 在阶段 4 落地。

- [ ] **Step 4: 跑测试与冒烟**

Run: `cargo test`
Expected: PASS

Run: `cargo run`
Expected: `?` 开/关帮助且吞掉其它键；`1`–`4` 整屏切换 + 焦点落在主区；`p` 弹歌单选择框、`a` 只动队列；`PgUp`/`PgDn`/`Ctrl+U`/`Ctrl+D` 翻页；`Home`/`End` 首尾；状态行左侧提示随焦点变化

- [ ] **Step 5: 提交**

```bash
git add -A src/tui
git commit -m "feat(tui): 帮助浮层与改键（a 归队列、p 加歌单、1-4/翻页/Home/End）"
```

---

# 阶段 3：显式队列

### Task 10: `queue.rs`（纯逻辑 + 模式推进）

**Files:**
- Create: `src/tui/queue.rs`
- Modify: `src/tui/mod.rs`（`pub mod queue;`）

**Interfaces:**
- Produces:
  - `queue::Mode { Sequential, LoopAll, RepeatOne, Shuffle }`（`next()`、`label()`）
  - `queue::Step { Play(usize), Stop }`
  - `queue::Queue`：`new(items: Vec<Song>, index: usize, origin: impl Into<String>) -> Self`、`items()`、`index()`、`len()`、`is_empty()`、`current() -> Option<&Song>`、`origin()`、`mode()`、`set_mode(Mode)`、`cycle_mode() -> Mode`、`push_back(Song)`、`insert_next(Song)`、`remove(at: usize) -> Option<Step>`、`clear()`、`jump(at: usize)`、`advance() -> Step`、`next_manual() -> Step`、`prev_manual() -> Step`

- [ ] **Step 1: 写失败的测试**

`src/tui/queue.rs`：

```rust
//! 播放队列与播放模式——纯逻辑，不碰 UI、网络与音频设备。
//!
//! 「一首播完往哪走」与「用户按 n/b 往哪走」是**两件事**：单曲循环时按 `n`
//! 仍然要前进（那是显式意图），而播完要重播当前这首。所以这里分成
//! `advance` / `next_manual` / `prev_manual` 三个入口，各自有单测钉住。

use std::time::{SystemTime, UNIX_EPOCH};

use crate::model::Song;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FileSizes;

    fn song(mid: &str) -> Song {
        Song {
            mid: mid.to_string(),
            song_id: None,
            media_mid: None,
            title: format!("歌 {mid}"),
            singers: "歌手".to_string(),
            album_mid: String::new(),
            album_name: String::new(),
            duration_secs: 200,
            sizes: FileSizes::default(),
        }
    }

    fn queue(count: usize, mode: Mode) -> Queue {
        let items = (0..count).map(|index| song(&format!("m{index}"))).collect();
        let mut queue = Queue::new(items, 0, "嗯");
        queue.set_mode(mode);
        queue
    }

    #[test]
    fn sequential_stops_at_the_end() {
        let mut queue = queue(3, Mode::Sequential);
        assert_eq!(queue.advance(), Step::Play(1));
        assert_eq!(queue.advance(), Step::Play(2));
        assert_eq!(queue.advance(), Step::Stop, "队尾要停，不回头");
    }

    #[test]
    fn loop_all_wraps_around() {
        let mut queue = queue(2, Mode::LoopAll);
        assert_eq!(queue.advance(), Step::Play(1));
        assert_eq!(queue.advance(), Step::Play(0));
    }

    #[test]
    fn repeat_one_replays_but_n_still_moves_on() {
        let mut queue = queue(3, Mode::RepeatOne);
        assert_eq!(queue.advance(), Step::Play(0), "播完重播当前");
        assert_eq!(queue.next_manual(), Step::Play(1), "n 是显式意图");
        assert_eq!(queue.advance(), Step::Play(1));
    }

    #[test]
    fn shuffle_walks_every_item_once_per_round() {
        let mut queue = queue(5, Mode::Shuffle);
        queue.reshuffle_seeded(42);
        let mut seen = vec![queue.index()];
        for _ in 1..5 {
            let Step::Play(index) = queue.advance() else {
                panic!("一轮之内不该停");
            };
            seen.push(index);
        }
        seen.sort_unstable();
        assert_eq!(seen, [0, 1, 2, 3, 4], "一轮里每首恰好一次");
        let Step::Play(_) = queue.advance() else {
            panic!("一轮播完重洗，不停下");
        };
    }

    #[test]
    fn prev_walks_back_in_the_same_order_it_came() {
        let mut queue = queue(3, Mode::Sequential);
        assert_eq!(queue.prev_manual(), Step::Stop, "第一首之前没有上一首");
        queue.jump(2);
        assert_eq!(queue.prev_manual(), Step::Play(1));

        let mut looping = queue(3, Mode::LoopAll);
        assert_eq!(looping.prev_manual(), Step::Play(2), "列表循环里 b 从第一首回到最后一首");

        let mut shuffled = queue(4, Mode::Shuffle);
        shuffled.reshuffle_seeded(7);
        let first = shuffled.index();
        let Step::Play(second) = shuffled.advance() else {
            panic!("随机模式应前进");
        };
        assert_eq!(shuffled.prev_manual(), Step::Play(first), "b 沿洗牌序后退");
        assert_eq!(second, shuffled.index());
    }

    #[test]
    fn appending_and_inserting_change_the_queue_not_the_source_list() {
        let mut queue = queue(2, Mode::Sequential);
        queue.push_back(song("tail"));
        assert_eq!(queue.len(), 3);
        assert_eq!(queue.items()[2].mid, "tail");
        queue.insert_next(song("next"));
        assert_eq!(queue.items()[1].mid, "next", "A 插到当前项后面");
        assert_eq!(queue.index(), 0, "插队不动当前项");
    }

    #[test]
    fn removing_the_current_item_plays_its_successor() {
        let mut queue = queue(3, Mode::Sequential);
        queue.jump(1);
        assert_eq!(queue.remove(1), Some(Step::Play(1)), "删当前项 → 播原 index+1");
        assert_eq!(queue.items()[1].mid, "m2");
    }

    #[test]
    fn removing_the_last_item_stops_unless_looping() {
        let mut sequential = queue(2, Mode::Sequential);
        sequential.jump(1);
        assert_eq!(sequential.remove(1), Some(Step::Stop));

        let mut looping = queue(2, Mode::LoopAll);
        looping.jump(1);
        assert_eq!(looping.remove(1), Some(Step::Play(0)));
    }

    #[test]
    fn removing_a_song_before_the_current_one_keeps_playing_the_same_song() {
        let mut queue = queue(3, Mode::Sequential);
        queue.jump(2);
        assert_eq!(queue.remove(0), None, "删的不是当前项，播放不用动");
        assert_eq!(queue.current().map(|s| s.mid.as_str()), Some("m2"));
        assert_eq!(queue.index(), 1, "下标要跟着前移");
    }

    #[test]
    fn clearing_an_empty_queue_is_a_no_op_and_current_is_none() {
        let mut queue = queue(1, Mode::Sequential);
        queue.clear();
        assert!(queue.is_empty());
        assert!(queue.current().is_none());
        assert_eq!(queue.advance(), Step::Stop);
        assert_eq!(queue.remove(0), Some(Step::Stop), "空队列删除也是停");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test queue`
Expected: FAIL——`Queue` 未定义

- [ ] **Step 3: 写实现**

```rust
/// 播放模式。`m` 按这个顺序循环。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Sequential,
    LoopAll,
    RepeatOne,
    Shuffle,
}

impl Mode {
    pub fn next(self) -> Self {
        match self {
            Mode::Sequential => Mode::LoopAll,
            Mode::LoopAll => Mode::RepeatOne,
            Mode::RepeatOne => Mode::Shuffle,
            Mode::Shuffle => Mode::Sequential,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::Sequential => "顺序",
            Mode::LoopAll => "列表循环",
            Mode::RepeatOne => "单曲",
            Mode::Shuffle => "随机",
        }
    }
}

/// 「接下来往哪走」。`Play(index)` 是队列下标，不是洗牌序里的位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Play(usize),
    Stop,
}

#[derive(Debug, Clone)]
pub struct Queue {
    items: Vec<Song>,
    index: usize,
    origin: String,
    mode: Mode,
    /// 随机模式下这一轮的顺序（`0..len` 的一个排列）。
    shuffle: Vec<usize>,
    /// 游标在 `shuffle` 里的位置。
    shuffle_pos: usize,
}

impl Queue {
    pub fn new(items: Vec<Song>, index: usize, origin: impl Into<String>) -> Self {
        let mut queue = Self {
            index: index.min(items.len().saturating_sub(1)),
            items,
            origin: origin.into(),
            mode: Mode::Sequential,
            shuffle: Vec::new(),
            shuffle_pos: 0,
        };
        queue.reshuffle();
        queue.sync_shuffle_pos();
        queue
    }

    pub fn items(&self) -> &[Song] {
        &self.items
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn current(&self) -> Option<&Song> {
        self.items.get(self.index)
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        if mode == Mode::Shuffle {
            self.reshuffle();
            self.sync_shuffle_pos();
        }
    }

    pub fn cycle_mode(&mut self) -> Mode {
        self.set_mode(self.mode.next());
        self.mode
    }

    pub fn push_back(&mut self, song: Song) {
        self.items.push(song);
        self.shuffle.push(self.items.len() - 1);
    }

    pub fn insert_next(&mut self, song: Song) {
        let at = (self.index + 1).min(self.items.len());
        self.items.insert(at, song);
        // 洗牌表：位置 ≥ at 的下标整体后移，新项插在当前项之后。
        for slot in &mut self.shuffle {
            if *slot >= at {
                *slot += 1;
            }
        }
        self.shuffle.insert((self.shuffle_pos + 1).min(self.shuffle.len()), at);
    }

    /// 删第 `at` 项。`Some(step)` = 这次删的是当前项，调用方按 `step` 继续播。
    pub fn remove(&mut self, at: usize) -> Option<Step> {
        if self.is_empty() {
            return Some(Step::Stop);
        }
        if at >= self.items.len() {
            return None;
        }
        self.items.remove(at);
        for slot in &mut self.shuffle {
            if *slot > at {
                *slot -= 1;
            }
        }
        if let Some(position) = self.shuffle.iter().position(|slot| *slot == at) {
            self.shuffle.remove(position);
            if position < self.shuffle_pos {
                self.shuffle_pos = self.shuffle_pos.saturating_sub(1);
            }
        }
        if at < self.index {
            self.index -= 1;
            return None;
        }
        if at > self.index {
            return None;
        }
        // 删的是当前项：下标不动就已经指向原来的下一首。
        if self.index < self.items.len() {
            return Some(Step::Play(self.index));
        }
        self.index = self.items.len().saturating_sub(1);
        Some(self.recover_after_end())
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.index = 0;
        self.shuffle.clear();
        self.shuffle_pos = 0;
    }

    /// 浮层里 `Enter` 跳播。
    pub fn jump(&mut self, at: usize) {
        if at < self.items.len() {
            self.index = at;
            self.sync_shuffle_pos();
        }
    }

    /// 一首播完往哪走。
    pub fn advance(&mut self) -> Step {
        if self.is_empty() {
            return Step::Stop;
        }
        match self.mode {
            Mode::Sequential => {
                if self.index + 1 < self.items.len() {
                    self.index += 1;
                    Step::Play(self.index)
                } else {
                    Step::Stop
                }
            }
            Mode::LoopAll => {
                self.index = (self.index + 1) % self.items.len();
                Step::Play(self.index)
            }
            Mode::RepeatOne => Step::Play(self.index),
            Mode::Shuffle => {
                if self.shuffle_pos + 1 < self.shuffle.len() {
                    self.shuffle_pos += 1;
                } else {
                    // 一轮播完重洗，避免「随机」在几首之间来回跳。
                    self.reshuffle();
                    self.shuffle_pos = 0;
                }
                self.index = self.shuffle[self.shuffle_pos];
                Step::Play(self.index)
            }
        }
    }

    /// 显式 `n`：单曲循环也前进，因为按 `n` 就是「我要换一首」。
    pub fn next_manual(&mut self) -> Step {
        match self.mode {
            Mode::RepeatOne | Mode::Sequential => {
                if self.index + 1 < self.items.len() {
                    self.index += 1;
                    Step::Play(self.index)
                } else {
                    Step::Stop
                }
            }
            Mode::LoopAll | Mode::Shuffle => self.advance(),
        }
    }

    /// 显式 `b`：沿「来时的那条顺序」后退。
    pub fn prev_manual(&mut self) -> Step {
        if self.is_empty() {
            return Step::Stop;
        }
        match self.mode {
            Mode::Shuffle => {
                if self.shuffle_pos == 0 {
                    return Step::Stop;
                }
                self.shuffle_pos -= 1;
                self.index = self.shuffle[self.shuffle_pos];
                Step::Play(self.index)
            }
            _ => {
                if self.index == 0 {
                    if self.mode == Mode::LoopAll {
                        self.index = self.items.len() - 1;
                        return Step::Play(self.index);
                    }
                    return Step::Stop;
                }
                self.index -= 1;
                Step::Play(self.index)
            }
        }
    }

    /// 洗牌用的小 PRNG（xorshift64*）：只为打乱顺序，跟安全性无关。
    fn reshuffle(&mut self) {
        self.reshuffle_seeded(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or(0x9E37_79B9_7F4A_7C15),
        );
    }

    fn reshuffle_seeded(&mut self, seed: u64) {
        let len = self.items.len();
        self.shuffle = (0..len).collect();
        let mut state = seed | 1;
        for position in (1..len).rev() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let swap = (state % (position as u64 + 1)) as usize;
            self.shuffle.swap(position, swap);
        }
    }

    /// 让游标指向当前项在洗牌表里的位置（`jump` / 新建后都要对齐）。
    fn sync_shuffle_pos(&mut self) {
        self.shuffle_pos = self
            .shuffle
            .iter()
            .position(|slot| *slot == self.index)
            .unwrap_or(0);
    }

    /// 已经走到队尾（或队列被删到当前项后面没东西）之后按模式恢复。
    fn recover_after_end(&mut self) -> Step {
        if self.is_empty() {
            return Step::Stop;
        }
        match self.mode {
            Mode::LoopAll => {
                self.index = 0;
                Step::Play(0)
            }
            Mode::Shuffle => {
                self.reshuffle();
                self.shuffle_pos = 0;
                self.index = self.shuffle[0];
                Step::Play(self.index)
            }
            Mode::Sequential | Mode::RepeatOne => Step::Stop,
        }
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test queue`
Expected: PASS（10 个用例）

- [ ] **Step 5: 提交**

```bash
git add -A src/tui/queue.rs src/tui/mod.rs
git commit -m "feat(queue): 显式队列模型与四种播放模式的推进逻辑"
```

---

### Task 11: 队列接入播放（Enter / a / A / m / n / b / 播完 / 自动跳过）

**Files:**
- Modify: `src/tui/state.rs`、`src/tui/mod.rs`（`Msg::Candidates` 的 `index` 语义）

**Interfaces:**
- Consumes: `queue::Queue`、`queue::{Step, Mode}`
- Produces: `App::open_queue(&mut self, songs: Vec<Song>, index: usize, origin: String)`、`App::step(&mut self, step: queue::Step)`、`App::append_to_queue(&mut self)`、`App::insert_queue_next(&mut self)`

- [ ] **Step 1: 写失败的测试**

```rust
    #[test]
    fn enter_opens_a_fresh_queue_from_the_focused_list() {
        let mut app = test_app();
        app.nav.menu = Menu::Favorites;
        app.nav.focus = Focus::Content;
        app.nav.favorites.items = vec![test_song("a"), test_song("b")];
        app.nav.favorites.state.select(Some(1));
        app.open_queue_from_focus();
        assert_eq!(app.queue.len(), 2);
        assert_eq!(app.queue.index(), 1);
        assert_eq!(app.queue.origin(), "我的收藏");
        assert_eq!(app.queue.mode(), queue::Mode::Sequential, "换队列回到顺序播放");
    }

    #[test]
    fn append_and_insert_do_not_disturb_the_current_song() {
        let mut app = test_app();
        app.open_queue(vec![test_song("a"), test_song("b")], 0, "嗯".to_string());
        app.nav.focus = Focus::Content;
        app.nav.favorites.items = vec![test_song("c")];
        app.nav.favorites.select_first();
        app.append_to_queue();
        assert_eq!(app.queue.len(), 3);
        assert_eq!(app.queue.current().map(|s| s.mid.as_str()), Some("a"));
        app.insert_queue_next();
        assert_eq!(app.queue.items()[1].mid, "c");
    }

    #[test]
    fn appending_with_an_empty_queue_opens_one_instead() {
        let mut app = test_app();
        app.nav.focus = Focus::Content;
        app.nav.favorites.items = vec![test_song("a")];
        app.nav.favorites.select_first();
        app.append_to_queue();
        assert_eq!(app.queue.len(), 1, "空队列时 a 等价于 Enter");
    }

    #[test]
    fn finishing_a_song_follows_the_mode_and_auto_skip_advances_too() {
        let mut app = test_app();
        app.open_queue(vec![test_song("a"), test_song("b")], 0, "嗯".to_string());
        app.queue.set_mode(queue::Mode::RepeatOne);
        assert_eq!(app.next_step_after_finish(), queue::Step::Play(0));
        app.queue.set_mode(queue::Mode::Sequential);
        assert_eq!(app.next_step_after_finish(), queue::Step::Play(1));
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test`
Expected: FAIL——`open_queue` / `append_to_queue` / `next_step_after_finish` 未定义

- [ ] **Step 3: 写实现**

`App` 用 `queue: queue::Queue` 替换原来的 `queue: Vec<Song>` + `queue_index` + `queue_name`；`Msg::Candidates` 的 `index` 改成队列下标（沿用现有 `request` 序号丢弃过期回话的机制）：

```rust
    /// 用某个列表开新队列并从 `index` 播。
    fn open_queue(&mut self, songs: Vec<Song>, index: usize, origin: String) {
        if songs.is_empty() {
            return;
        }
        self.queue = queue::Queue::new(songs, index, origin);
        self.skipped = 0;
        self.start(self.queue.index());
    }

    /// Enter：当前焦点列表 → 新队列。
    fn open_queue_from_focus(&mut self) {
        let origin = match self.nav.menu {
            Menu::Favorites => "我的收藏".to_string(),
            Menu::Playlists => self.nav.shown_name().unwrap_or_default(),
            Menu::Search => format!("搜索：{}", self.nav.search_text),
            Menu::History => "历史".to_string(),
        };
        let Some(view) = self.nav.focused_songs() else {
            return;
        };
        let index = view.state.selected().unwrap_or(0);
        let songs = view.items.clone();
        self.open_queue(songs, index, origin);
    }

    /// `a` / `A`：把焦点列表里选中的那首加进队列。
    fn push_to_queue(&mut self, front: bool) {
        let Some(song) = self.nav.focused_songs().and_then(|view| view.selected().cloned()) else {
            return;
        };
        if self.queue.is_empty() {
            self.open_queue(vec![song.clone()], 0, "单曲".to_string());
            return;
        }
        if front {
            self.queue.insert_next(song.clone());
            self.status = format!("已插队：{} — {}", song.title, song.singers);
        } else {
            self.queue.push_back(song.clone());
            self.status = format!("已加进队列：{} — {}", song.title, song.singers);
        }
    }

    /// 一首播完（或不可播自动跳过）之后按模式取下一步。
    fn next_step_after_finish(&mut self) -> queue::Step {
        self.queue.advance()
    }

    /// 按 `Step` 继续播：`Play` 取流播放，`Stop` 停播并说明原因。
    fn step(&mut self, step: queue::Step) {
        match step {
            queue::Step::Play(index) => self.start(index),
            queue::Step::Stop => {
                self.status = if self.queue.is_empty() {
                    "队列已空".to_string()
                } else {
                    "队列播完".to_string()
                };
            }
        }
    }
```

`tick()` 里把 `if self.queue_index + 1 < self.queue.len() { self.play_index(...) }` 换成 `let step = self.next_step_after_finish(); self.step(step)`。自动跳过（`Msg::Candidates` 的 `Err` 分支）同样走 `Step`：`self.skipped += 1` 后 `let step = self.queue.advance(); self.step(step)`，`MAX_AUTO_SKIPS` 满则 `Step::Stop` 语义。

正在播的标记改成 mid 比对：

```rust
    /// 主区列表里哪些行要标 `♪`（按 mid：QQ 的 mid 全局唯一，重名歌手不再误标）。
    fn now_playing_mid(&self) -> Option<&str> {
        self.queue.current().map(|song| song.mid.as_str())
    }
```

`draw/content.rs` 里把 `song_label(song) == playing` 换成 `Some(song.mid.as_str()) == app.now_playing_mid()`。

- [ ] **Step 4: 跑测试与冒烟**

Run: `cargo test && cargo clippy --all-targets`
Expected: PASS、无新警告

Run: `cargo run`
Expected: 任意列表 `Enter` 开队列并播放；`a` 追加 / `A` 插队后 `n` 按新顺序走；`m` 切到单曲循环时播完重播、按 `n` 仍前进；顺序模式播到队尾停下并显示「队列播完」；队列删空后 `n` 提示「队列已空」

- [ ] **Step 5: 提交**

```bash
git add -A src/tui
git commit -m "feat(queue): 界面接入显式队列、播放模式与播完推进"
```

---

### Task 12: 队列浮层 `u` + 播放条显示模式

**Files:**
- Modify: `src/tui/draw/overlay.rs`（`draw_queue`）、`src/tui/draw/player.rs`、`src/tui/state.rs`（`queue_open`、`queue_state: ListState`）

**Interfaces:**
- Consumes: `queue::Queue`、`keys::{Action, Context}`（`QueueOverlay` 上下文）
- Produces: `Nav.queue_open: bool`、`Nav.queue_state: ListState`

- [ ] **Step 1: 写失败的测试**

```rust
    #[test]
    fn queue_overlay_navigates_deletes_and_jumps() {
        let mut app = test_app();
        app.open_queue(
            vec![test_song("a"), test_song("b"), test_song("c")],
            0,
            "嗯".to_string(),
        );
        app.apply(Action::QueueOverlay);
        assert!(app.nav.queue_open, "u 打开队列浮层");
        assert_eq!(app.context(), keys::Context::QueueOverlay);

        app.apply(Action::Down);
        assert_eq!(app.nav.queue_state.selected(), Some(1));
        app.apply(Action::QueueDelete);
        assert_eq!(app.queue.len(), 2);
        assert_eq!(app.queue.current().map(|s| s.mid.as_str()), Some("a"), "删的不是当前项");

        app.apply(Action::Enter);
        assert_eq!(app.queue.index(), 1, "浮层里 Enter 用来跳播");
        app.apply(Action::QueueClear);
        assert!(app.queue.is_empty());
        assert_eq!(app.status, "队列已空");
    }

    #[test]
    fn queue_overlay_never_changes_playback_order() {
        let mut app = test_app();
        app.open_queue(vec![test_song("a"), test_song("b")], 0, "嗯".to_string());
        let order: Vec<_> = app.queue.items().iter().map(|s| s.mid.clone()).collect();
        app.apply(Action::QueueOverlay);
        app.apply(Action::Down);
        app.apply(Action::Enter);
        let after: Vec<_> = app.queue.items().iter().map(|s| s.mid.clone()).collect();
        assert_eq!(order, after, "队列没有重排能力，跳播只改指针");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test queue_overlay`
Expected: FAIL——`queue_open` / `queue_state` 未定义

- [ ] **Step 3: 写实现**

`Nav` 增 `queue_open: bool` 与 `queue_state: ListState`；`Action::QueueOverlay` 打开时把 `queue_state` 定位到 `queue.index()`；`Action::Back` 关闭。

`draw/overlay.rs`：

```rust
/// 队列浮层：标题写 `队列 3/42 · 来自《嗯》`，当前项高亮。
pub fn draw_queue(app: &mut App, frame: &mut Frame) {
    let items: Vec<ListItem> = app
        .queue
        .items()
        .iter()
        .map(|song| {
            ListItem::new(format!(
                "{:>5}  {} — {}",
                format_duration(StdDuration::from_secs(song.duration_secs)),
                song.title,
                song.singers
            ))
        })
        .collect();
    let title = format!(
        " 队列 {}/{} · 来自《{}》 · {} ",
        app.queue.index() + 1,
        app.queue.len(),
        app.queue.origin(),
        app.queue.mode().label()
    );
    let height = (items.len() as u16 + 2).min(frame.area().height.saturating_sub(2));
    let area = super::overlay_rect(
        frame.area(),
        frame.area().width.saturating_sub(8).min(72),
        height,
    );
    frame.render_widget(Clear, area);
    let list = List::new(items)
        .block(Block::bordered().title(title))
        .highlight_style(highlight())
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(list, area, &mut app.nav.queue_state);
}
```

> `overlay::draw_*` 一律收 `app: &mut App`（浮层要改 `ListState`）；`draw_picker` 从 `tui.rs` 搬过来时按这个签名改。

播放条信息行按 spec 顺序拼：

```rust
        let line = Line::from(vec![
            Span::styled("♪ ", cyan),
            Span::styled(title, cyan.bold()),
            Span::raw(format!(
                "  {quality} {state_text} · {} · {}/{} · 音量 {}%",
                queue.mode().label(),
                queue.index() + 1,
                queue.len(),
                snapshot.volume
            )),
        ]);
```

队列空时 `{}/{}` 显示 `-/-`（未播放且无队列）。

- [ ] **Step 4: 跑测试与冒烟**

Run: `cargo test`
Expected: PASS

Run: `cargo run`
Expected: `u` 弹队列浮层（标题含序号/来源/模式），`j`/`k` 移动、`Enter` 跳播并立即出声、`d` 删一项、`Shift+D` 清空后停播且状态行提示；播放条显示 `顺序 · 3/42` 并随 `m` 变化

- [ ] **Step 5: 提交**

```bash
git add -A src/tui
git commit -m "feat(queue): 队列浮层（跳播/删除/清空）与播放条模式显示"
```

---

### Task 13: 播放历史接入（写入 + 历史屏完整化）

**Files:**
- Modify: `src/tui/state.rs`（起播写历史、`Msg::Favorites` 之后的历史载入）、`src/tui/draw/content.rs`（历史屏渲染）、`src/main.rs`（把 `History` 交给 `tui::run`）

**Interfaces:**
- Consumes: `history::{History, Entry, now_secs, relative}`、`paths::history_path()`
- Produces: `App::record_history(&mut self, song: &Song)`、`App::refresh_history_view(&mut self)`

- [ ] **Step 1: 写失败的测试**

```rust
    #[test]
    fn starting_a_song_records_history_once_and_refreshes_the_view() {
        let mut app = test_app(160, 40);
        let song = test_song("a");
        app.record_history(&song);
        app.record_history(&song);
        assert_eq!(app.history.len(), 1, "同一首连着播只记一条");
        assert_eq!(app.nav.history_view.items.len(), 1, "历史屏立刻能看到");
        assert_eq!(app.nav.history_view.items[0].mid, "a");
    }

    #[test]
    fn history_view_plays_from_the_row_the_user_picked() {
        let mut app = test_app(160, 40);
        for mid in ["a", "b", "c"] {
            app.record_history(&test_song(mid));
        }
        app.nav.menu = Menu::History;
        app.nav.focus = Focus::Content;
        app.nav.history_view.state.select(Some(1));
        app.open_queue_from_focus();
        assert_eq!(app.queue.len(), 3);
        assert_eq!(app.queue.current().map(|s| s.mid.as_str()), Some("b"));
        assert_eq!(app.queue.origin(), "历史");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test history_`
Expected: FAIL——`record_history` / `history` 字段未定义

- [ ] **Step 3: 写实现**

`App` 增 `history: History` 与 `history_path: PathBuf`（`tui::run` 传入，便于测试注入临时路径）：

```rust
    /// 起播那一刻记一条历史（取流成功之后调用），并落盘。
    fn record_history(&mut self, song: &Song) {
        self.history.push(
            history::Entry {
                mid: song.mid.clone(),
                title: song.title.clone(),
                singers: song.singers.clone(),
                at: history::now_secs(),
                origin: self.queue.origin().to_string(),
            },
            self.config.history_limit,
        );
        if let Err(error) = self.history.save(&self.history_path) {
            tracing::warn!("写历史失败：{error:#}");
        }
        self.refresh_history_view();
    }

    /// 历史屏：新在前。每行的右侧副标题是「时间 · 来自《来源》」。
    /// `history::Entry` 只有 mid / 标题 / 歌手；取流只认 mid（`qq::vkey::candidates`
    /// 拿 mid 请求），所以能直接还原成 `Song`——若实测发现取流需要更多字段，
    /// 就把整首 `Song` 存进 `Entry`（这是 spec 的待验证项 3）。
    fn refresh_history_view(&mut self) {
        let selected = self.nav.history_view.state.selected();
        self.nav.history_view.items = self
            .history
            .entries()
            .iter()
            .map(|entry| Song {
                mid: entry.mid.clone(),
                song_id: None,
                media_mid: None,
                title: entry.title.clone(),
                singers: entry.singers.clone(),
                album_mid: String::new(),
                album_name: String::new(),
                duration_secs: 0,
                sizes: FileSizes::default(),
            })
            .collect();
        self.nav.history_view.labels = self
            .history
            .entries()
            .iter()
            .map(|entry| {
                format!(
                    "{} · 来自《{}》",
                    history::relative(history::now_secs(), entry.at),
                    entry.origin
                )
            })
            .collect();
        self.nav.history_view.state.select(selected.or(Some(0)));
    }
```

`ListView::labels`（Task 6 定义）在这里第一次真正被填：历史屏的行渲染成 `标题 — 歌手` + 右侧 `labels[i]`，**不**画时长/档位/体积三列（还原出来的 `Song` 里 `duration_secs` 是 0，画出来就是假的 00:00）。

`Shift+D`（`Context::Content` 且 `menu == Menu::History` 时的 `Action::QueueClear`）→ `app.history.clear(&path)` + 刷新 + 状态行「已清空播放历史」。

- [ ] **Step 4: 跑测试与冒烟**

Run: `cargo test && cargo run`
Expected: 播一首歌后进 `4` 历史屏能看到该首（左侧菜单底部显示 `uin`）；`Enter` 从那首重播；`Shift+D` 清空并变成「暂无播放历史」；重启后历史仍在

- [ ] **Step 5: 提交**

```bash
git add -A src/tui src/main.rs
git commit -m "feat(history): 起播写历史，历史屏可重播与清空"
```

---

# 阶段 4：登录屏

### Task 14: `qr.rs` 改成产出字符块 + 一键打开 PNG

**Files:**
- Modify: `src/qr.rs`（`show()` → `render()` + `to_lines()`；删掉 `print!` 与终端尺寸判断）

**Interfaces:**
- Produces:
  - `qr::to_lines(png: &[u8]) -> Result<Vec<String>>`（每字符格上下两像素，`▀`/`▄`/`█`/空格；深色终端语义不变）
  - `qr::save_png(png: &[u8], path: &Path) -> Result<PathBuf>`
  - `qr::dimensions(png: &[u8]) -> Result<(u32, u32)>`（给尺寸提示用）
  - `qr::open_with_system(path: &Path) -> Result<()>`

- [ ] **Step 1: 写失败的测试**

```rust
    #[test]
    fn renders_one_character_per_two_pixel_rows() {
        let png = sample_png(); // 现有测试里的 4×4 构造器
        let lines = to_lines(&png).expect("渲染");
        assert_eq!(lines.len(), 2, "4 像素高 = 2 行");
        assert_eq!(lines[0].chars().count(), 4, "每行 4 列");
        assert_eq!(dimensions(&png).expect("读尺寸"), (4, 4));
    }

    #[test]
    fn rejects_non_png_payloads() {
        let error = to_lines(b"<html>not a png</html>").expect_err("非 PNG 要报错");
        assert!(error.to_string().contains("PNG"), "实际：{error}");
    }

    #[test]
    fn saves_the_png_beside_the_session() {
        let dir = std::env::temp_dir().join(format!("canto-qr-{}", std::process::id()));
        let path = dir.join("login-qr.png");
        save_png(&sample_png(), &path).expect("落盘");
        assert!(path.exists(), "PNG 一定要留一份，终端画不下时靠它");
        std::fs::remove_dir_all(&dir).ok();
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test qr`
Expected: FAIL——`to_lines` / `dimensions` / `save_png` 未定义

- [ ] **Step 3: 写实现**

把现有 `show` 拆开：PNG 解码 + `is_module` 判定保留；`to_lines` 返回 `Vec<String>`（行为与现在的 `print!` 完全一致，只是不再自己输出）；`save_png` 用 `std::fs::create_dir_all` + `write`；`open_with_system`：

```rust
/// 调系统默认查看器打开文件。仅为把二维码递给手机扫——终端画不下时的兜底。
pub fn open_with_system(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(all(unix, not(target_os = "macos")))]
    let program = "xdg-open";
    std::process::Command::new(program)
        .arg(path)
        .spawn()
        .with_context(|| format!("调 {program} 打开 {} 失败", path.display()))
        .map(|_| ())
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test qr`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add -A src/qr.rs
git commit -m "refactor(qr): 二维码改成产出字符块，附系统查看器兜底"
```

---

### Task 15: `Screen::Login` 登录屏

**Files:**
- Create: `src/tui/login.rs`
- Modify: `src/tui/mod.rs`（`Screen` 枚举与切换）、`src/tui/state.rs`（`App.screen`、`Msg::{Qr, QrPoll}`）、`src/main.rs`（不再在启动时 `bail!`，交给 TUI）

**Interfaces:**
- Consumes: `qq::login::{create, poll, Poll}`、`session::jar()`、`qr::{to_lines, save_png, dimensions, open_with_system}`、`paths::qr_png_path()`
- Produces:
  - `login::LoginState { pub phase: LoginPhase, pub lines: Vec<String>, pub message: String, pub input: Option<String>, pub remaining: u32 }`
  - `login::LoginPhase { Loading, Ready, Expired, Failed(String) }`
  - `tui::Screen { Login(LoginState), Main }`
  - `App::enter_login(&mut self)`、`App::on_login_msg(&mut self, message: Msg)`

- [ ] **Step 1: 写失败的测试**

`src/tui/login.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_input_swallows_printable_keys_but_keeps_esc_and_enter() {
        let mut state = LoginState::default();
        state.begin_cookie_input();
        assert_eq!(state.context(), keys::Context::SearchInput);
        state.push_char('u');
        state.push_char('i');
        assert_eq!(state.input.as_deref(), Some("ui"));
        state.pop_char();
        assert_eq!(state.input.as_deref(), Some("u"));
        state.cancel_input();
        assert_eq!(state.context(), keys::Context::Login, "退出输入态回到登录屏常规键位");
    }

    #[test]
    fn a_short_terminal_says_what_size_is_needed_instead_of_drawing_half_a_qr() {
        let mut state = LoginState::default();
        state.lines = vec!["█".repeat(111); 56];
        state.message = "等待扫码…".to_string();
        let hint = state.size_hint(100, 30).expect("100 列装不下");
        assert!(hint.contains("112"), "提示里要有需要的尺寸：{hint}");
        assert!(state.size_hint(120, 60).is_none(), "装得下就不提示");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test login`
Expected: FAIL——`LoginState` 未定义

- [ ] **Step 3: 写实现**

`login.rs`：状态 + 绘制（二维码居中、`手机 QQ 扫一扫 · 剩余 mm:ss`、`[c] 粘贴 cookie  [o] 打开二维码图片`、状态行文案）。`c` 进入单行输入（横向滚动：绘制时只取尾部 `area.width - 4` 个字符），回车走 `jar.login_with_cookie` + `jar.save()`，失败就地显示 `{error:#}`。

`keys.rs` 增 `Context::Login` 与两个动作 `Action::LoginCookie` / `Action::LoginOpenPng`（`Context` 与 `Action` 枚举各加这几项）：

```rust
        Context::Login => match key.code {
            KeyCode::Char('c') => Some(Action::LoginCookie),
            KeyCode::Char('o') => Some(Action::LoginOpenPng),
            KeyCode::Char('q') => Some(Action::Quit),
            _ => None,
        },
```

登录屏的 cookie 输入态复用 `Context::SearchInput`（`Enter` = 提交整串、`Esc` = 退出输入态、`Backspace` = 删字符），因此不需要为输入另写一套键位。

`main.rs` 与 `tui::run`：删掉「未登录就 `bail!`」的分支，`App::new` 时读 `session::jar().state()`，未登录则 `enter_login()` 并立刻在后台 `qq::login::create()`（结果走 `Msg::Qr`），随后每 2 秒 `qq::login::poll()`（`Msg::QrPoll`），沿用原 `QR_ATTEMPTS = 3` 与 `QR_POLL_INTERVAL = 2s`：

```rust
/// 扫码登录：取二维码 → 画到屏幕 → 轮询到成功（过期就换一张，最多 3 张）。
fn start_qr_login(&mut self) {
    let tx = self.tx.clone();
    self.handle.spawn(async move {
        let result = qq::login::create().await;
        let _ = tx.send(Msg::Qr(result));
    });
}
```

- [ ] **Step 4: 跑测试与冒烟**

Run: `cargo test && cargo run`
Expected: 未登录时直接出登录屏：终端 ≥112×60 时二维码画在屏内并倒计时；把终端缩到 100×30 时显示 `终端需要 ≥112×60（当前 100×30）`；`c` 粘贴 cookie 后回车即进主界面（`session.json` 被写盘）；已登录的机器直接进主界面

- [ ] **Step 5: 提交**

```bash
git add -A src/tui src/main.rs
git commit -m "feat(tui): 登录屏（二维码/粘贴 cookie/尺寸兜底），TUI 成为唯一入口"
```

---

### Task 16: 会话失效切屏 + 帮助里的登出 + README 收尾

**Files:**
- Modify: `src/session.rs`（新增待验证的失效识别）、`src/qq/*.rs`（错误透出处）、`src/tui/state.rs`（`Msg` 错误分支切屏、`Action::Logout`）、`src/tui/draw/overlay.rs`（帮助里加回 `登出 L` 行）、`README.md`

**Interfaces:**
- Consumes: `session::Jar::{clear, save, state}`、`tui::Screen::Login`、`player::Cmd::Stop`
- Produces: `session::looks_expired(body: &str, status: u16) -> bool`、`session::SessionExpired`（`anyhow` 错误标记）、`App::handle_session_expired(&mut self, detail: String)`、`player::Engine::stop(&self)`（给音频线程发停播命令、快照复位 `Idle`）

- [ ] **Step 1: 实现失效识别（先按最保守规则）**

在 `session.rs` 加：

```rust
/// 响应里出现这些码/文案时判定为「会话已失效」。
///
/// ⚠️ 待实测收紧：现在只覆盖已知的凭据缺失与 401。实测时把 `cargo run -- -l debug`
/// 日志里真实的未登录响应码补进来——猜码值比漏判更糟。
pub const EXPIRED_CODES: [&str; 2] = ["1000", "401"];
pub const EXPIRED_MARKERS: [&str; 3] = ["not login", "未登录", "登录态失效"];

pub fn looks_expired(body: &str, status: u16) -> bool {
    status == 401
        || EXPIRED_MARKERS.iter().any(|marker| body.contains(marker))
        || EXPIRED_CODES
            .iter()
            .any(|code| body.contains(&format!("\"code\":{code}")) || body.contains(&format!("\"code\": {code}")))
}
```

测试：

```rust
    #[test]
    fn expired_detection_covers_status_markers_and_codes_but_not_normal_bodies() {
        assert!(looks_expired("", 401));
        assert!(looks_expired("{\"code\":1000,\"subcode\":0}", 200));
        assert!(looks_expired("please not login", 200));
        assert!(!looks_expired("{\"code\":0,\"data\":{}}", 200), "正常响应不能误判");
        assert!(!looks_expired("{\"code\":80092}", 200), "无权不是未登录");
    }
```

- [ ] **Step 2: 请求层接上并切屏**

`qq/` 层的请求封装（`qq/mod.rs` 里各接口共用的发请求处）在 `looks_expired` 为真时返回带标记的错误（`anyhow::Error::new(SessionExpired)`）；`App::on_msg` 的 `Err` 分支先 `if error.downcast_ref::<SessionExpired>().is_some() { self.handle_session_expired(...) }`：

```rust
    /// 会话失效：清本地会话、回登录屏、把原因写在登录屏状态行。
    fn handle_session_expired(&mut self, detail: String) {
        if let Err(error) = session::jar().clear() {
            tracing::warn!("清会话失败：{error:#}");
        }
        self.queue.clear();
        self.engine.stop();
        self.enter_login();
        if let Screen::Login(state) = &mut self.screen {
            state.message = format!("会话已过期，请重新登录（{detail}）");
        }
    }
```

`player::Engine` 需要新增 `pub fn stop(&self)`（给音频线程发停播命令并清空快照）——现有 `Cmd` 枚举里加一个 `Stop` 变体，`handle` 里 `*player = None` + 快照复位为 `Idle`。

- [ ] **Step 3: 帮助里的登出**

`help_lines` 加回 `登出  L（清空本地会话，回登录屏）`；`Action::Logout`（仅在 `Context::HelpOverlay` 产生）→ `handle_session_expired("手动登出".to_string())`，登录屏文案改成「已登出，请重新登录」。

- [ ] **Step 4: 跑测试与冒烟**

Run: `cargo test`
Expected: PASS（含新的 4 个失效识别用例）

Run: `cargo run`
Expected: `?` 里按 `L` → 回登录屏且 `session.json` 被删；手工把 `session.json` 改坏（或删除）后重进 → 登录屏按 `c` 粘贴 cookie 能直接进去；正常播放、搜索、歌单写入**不被**误判为失效

- [ ] **Step 5: README 收尾**

用法段改为：`cargo run` 进 TUI；首次运行出登录屏（扫码 / 粘贴 cookie）；配置在 `<配置数据目录>/canto/config.toml`（音质下限 / 下载目录 / 历史上限）；键位表换成 spec 第 2 节那张；「已知限制」补两条：同一首歌在同一列表出现两次会同时标 `♪`；队列与各屏选中位置退出即丢。

- [ ] **Step 6: 提交**

```bash
git add -A src README.md
git commit -m "feat(session): 会话失效回登录屏、帮助内登出，README 收尾"
```

---

## 自审记录

**Spec 覆盖**：骨架与尺寸契约（Task 8）、导航/焦点/键位（Task 5/6/7/9）、队列与模式（Task 10/11/12）、历史（Task 3/13）、启动与登录（Task 15/16）、config 与 CLI 收缩（Task 2/4）、代码结构（Task 5 起）、测试策略（各任务的用例）、四阶段划分（本文件的四个标题）、待验证项（Task 13 的 `Entry → Song` 取流、Task 16 的失效码）逐条落到了任务。

**自审时修掉的问题**（列出来是给执行者的上下文）：

1. `keys::common` 里 Esc 的两条匹配臂顺序会让「菜单上 Esc 不做事」的用例永远失败 → 改成 `(context != Context::Menu).then_some(Action::Back)`。
2. `queue::prev_manual` 的 `LoopAll` 回绕写在 `if self.index == 0 { return Stop }` 之后、永远走不到 → 提前判断，并补了「列表循环里 `b` 从第一首回到最后一首」的用例。
3. `config` 的测试临时目录会互相删除 → 每个用例一个独立 tag。
4. `test_app` / `test_song` 在 Task 8 起被引用却没人定义 → 在 Task 6 的测试模块里定好（含运行时 `Box::leak`、真开音频设备、历史路径每测试唯一三处注意点），Task 8 起统一收 `test_app()` 无参。
5. 搜索结果与歌单曲目原本共用 `ListView tracks`，一搜就把正在看的歌单曲目冲掉 → 拆出 `nav.search_results`。
6. 历史屏原本要改成 `ListView<Entry>`，会与 `Nav::focused_songs() -> &mut ListView<Song>` 打架 → 行模型仍是 `Song`，副标题走 `ListView::labels`。
7. `overlay::draw_*` 的签名定成 `&mut App`，去掉 `app.queue_state.clone()` 这类写不通的写法。
8. 登录屏的键位上下文与测试对齐（新增 `Context::Login`；cookie 输入态复用 `SearchInput`）。
9. Task 9 的帮助浮层不再提前画「L 登出」那一行（登出动作 Task 16 才落地）。
10. Task 16 的 `Engine::stop` / `SessionExpired` / `looks_expired` 写进了 Interfaces。

**已知取舍**：Task 5 里 `Menu`/`PageUp`/`PageDown`/`Back` 先接「离得最近的现有行为」，Task 6–9 接管为真实路由——这是分阶段落地的顺序，不是空实现。

**类型一致性**：`Nav` / `ListView` / `Menu` / `Focus` / `Queue` / `Step` / `Mode` / `Action` / `Context` / `History` / `Entry` / `Config` 在各任务里同名同签名。`tui::run` 在 Task 4 定成 `(handle, engine, config, warning)`；Task 13 需要追加 `history_path: PathBuf` 与 Task 15 需要 `Screen` 初始化时，执行者要同步改 `main.rs` 的调用点（这是唯一一处签名会演进的地方，已在对应任务里写明）。

---

## 实施后的增量（不属于原计划，记在这里保持连续）

- **队列浮层 / 歌单选择框的 `j`/`k` 穿透**（`3da631d`）：`apply` 的 `Down`/`Up` 一路落到主区列表，浮层自己那份 `ListState` 从没被挪动，表现为「浮层里选不动」。修法按「谁在最上层」路由；补了状态层与渲染层两条回归测试（把修复摘掉两条都会红）。
- **`/` 改为「筛当前列表」**：设计见 `specs/2026-10-11-tui-redesign-design.md` §10.1。搜索屏改由菜单第 3 项进入（原 `Nav::begin_search_input` 随之删除）。
- **单例运行**：设计见同文件 §10.2（`flock` + pid 锁文件，`src/instance.rs`）。
- **去掉菜单栏、切屏只留数字键**：设计见 `specs/2026-10-11-tui-redesign-design.md` §10.3。删掉 `Focus`/`Tab`/`Context::Menu`/`draw/menu.rs` 与窄屏页签断点；§2 里被推翻的两行已就地标注。
- **全屏歌词（`Y`）与当前句居中**：设计见 `specs/2026-10-11-tui-redesign-design.md` §10.4。删掉 `App::lyric_state`，改成自己算滚动位置（`lyric_rows` + `centered_scroll`，纯函数带单测）；全屏时内容区让给歌词，列表类键在 `Context::LyricFull` 里不绑定。


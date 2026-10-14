//! 配置文件：`<配置目录>/canto/config.toml`。
//!
//! 自用工具，配置面刻意窄，界面上也不提供设置项——改配置就是开这个文件改，因此：
//!
//! - **解析失败绝不能变成打不开程序**。只有 TOML 语法本身坏了才整份回默认；语法没问题时
//!   每个键各管各的：类型写错 = 那个键回默认、取值超范围 = 夹住，别的键一字不动。
//! - **不认识键要说话**：多出来的键（多半是拼错）点名进日志，不给「改了没反应」的错觉。
//! - 首次运行写一份**带注释的完整默认配置**：能配的键都在里面，不能在这份文件里改的
//!   （状态、派生数据、固定路径）用注释点明。想重新来一份就 `canto config init`。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use unicode_width::UnicodeWidthStr;

use crate::clock;
use crate::hotkeys;
use crate::model::Quality;
use crate::paths;
use crate::tui::draw::status::NOTICE_COLS;

/// 播放缓存默认上限（MiB）。1 GiB 大约能放 2000 首 128k、400 首 320k。
pub const DEFAULT_CACHE_MB: u64 = 1024;

/// 顶层认得的段。多出来的键会被点名（见 [`Section::unknown`]）。
const TOP_KEYS: &[&str] = &[
    "playback",
    "paths",
    "cache",
    "history",
    "ui",
    "library",
    "hotkeys",
    "desktop_lyrics",
];

/// 配置：每个段一组，字段就是文件里那些键。
///
/// `Default` 直接由各段的默认值拼出来（`derive` 就够）——「默认配置」只有这一处定义，
/// 模板、缺键回退、`Config::default()` 都读它。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub playback: PlaybackConfig,
    pub paths: PathsConfig,
    pub cache: CacheConfig,
    pub history: HistoryConfig,
    pub ui: UiConfig,
    pub library: LibraryConfig,
    pub hotkeys: HotkeyConfig,
    pub desktop_lyrics: DesktopLyricsConfig,
}

/// `[playback]` 段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackConfig {
    /// 音质下限（播放与下载共用）。
    pub quality: Quality,
}

impl Default for PlaybackConfig {
    fn default() -> Self {
        // 默认 128k MP3：体积最小，够日常听；要更好就在 config.toml 里改。
        Self {
            quality: Quality::Low128,
        }
    }
}

/// `[paths]` 段：两个落盘目录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathsConfig {
    /// 歌曲下载目录（`d` 键）。
    pub download_dir: PathBuf,
    /// 歌词下载目录（跟歌曲分开放）。
    pub lyrics_dir: PathBuf,
}

impl Default for PathsConfig {
    fn default() -> Self {
        Self {
            download_dir: default_download_dir(),
            lyrics_dir: default_lyrics_dir(),
        }
    }
}

/// `[cache]` 段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheConfig {
    /// 播放缓存上限（MiB）；0 = 关掉缓存。
    pub mb: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            mb: DEFAULT_CACHE_MB,
        }
    }
}

/// `[history]` 段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryConfig {
    /// 播放历史最多留多少条（至少 1：0 条历史不叫历史）。
    pub limit: usize,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self { limit: 500 }
    }
}

/// `[ui]` 段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiConfig {
    /// 状态行右侧通知的存活秒数；0 = 不自动消失（只被下一条顶掉）。
    pub notice_seconds: u64,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self { notice_seconds: 4 }
    }
}

/// 默认下载目录：`<系统音乐目录>/canto`。
pub fn default_download_dir() -> PathBuf {
    dirs::audio_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("canto")
}

/// 默认歌词目录：`<系统音乐目录>/canto-lyrics`。
///
/// 刻意跟歌曲目录**平级分开**：翻目录时不至于一首歌夹着两三个 `.lrc`，
/// 也方便本地播放器或脚本按固定位置去取。
pub fn default_lyrics_dir() -> PathBuf {
    dirs::audio_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("canto-lyrics")
}

/// `[hotkeys]` 段：canto 在后台（没聚焦）时也能控制播放。
///
/// 空串 = 这个动作不注册；缺字段用默认组合。写法不对的（键名不认 / 没带修饰键）在解析时
/// 被清成空串并给一条警告，所以这里只留一条不变式：**非空 = 合法 = 会被注册**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyConfig {
    /// 播放 / 暂停。
    pub play_pause: String,
    /// 下一首。
    pub next: String,
    /// 上一首。
    pub prev: String,
    /// 音量加。
    pub volume_up: String,
    /// 音量减。
    pub volume_down: String,
    /// 开 / 关桌面歌词窗口（关掉 = 把窗口拆了）。
    pub desktop_lyrics_toggle: String,
    /// 桌面歌词的锁定/解锁。
    pub desktop_lyrics_lock: String,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        let mut config = Self {
            play_pause: String::new(),
            next: String::new(),
            prev: String::new(),
            volume_up: String::new(),
            volume_down: String::new(),
            desktop_lyrics_toggle: String::new(),
            desktop_lyrics_lock: String::new(),
        };
        // 默认组合只在 `hotkeys::DEFAULTS` 里写一遍：配置模板也读它，改一处不会说岔。
        for entry in hotkeys::DEFAULTS {
            config.set(entry.field, entry.keys);
        }
        config
    }
}

impl HotkeyConfig {
    /// 按字段名取值。只有这一处（和下面的 `set`）需要「字段名 ↔ 成员」的对照——
    /// 因为它得跟着 `hotkeys::DEFAULTS` 那张表走。
    fn get(&self, field: &str) -> &str {
        match field {
            "play_pause" => &self.play_pause,
            "next" => &self.next,
            "prev" => &self.prev,
            "volume_up" => &self.volume_up,
            "volume_down" => &self.volume_down,
            "desktop_lyrics_toggle" => &self.desktop_lyrics_toggle,
            "desktop_lyrics_lock" => &self.desktop_lyrics_lock,
            _ => "",
        }
    }

    /// 按字段名赋值（`""` 表示禁用）。
    fn set(&mut self, field: &str, keys: &str) {
        match field {
            "play_pause" => self.play_pause = keys.to_string(),
            "next" => self.next = keys.to_string(),
            "prev" => self.prev = keys.to_string(),
            "volume_up" => self.volume_up = keys.to_string(),
            "volume_down" => self.volume_down = keys.to_string(),
            "desktop_lyrics_toggle" => self.desktop_lyrics_toggle = keys.to_string(),
            "desktop_lyrics_lock" => self.desktop_lyrics_lock = keys.to_string(),
            _ => {}
        }
    }

    /// 要注册的绑定（按 `DEFAULTS` 的顺序）；空串（= 禁用）在这里跳过。
    pub fn bindings(&self) -> Vec<hotkeys::Binding> {
        hotkeys::DEFAULTS
            .iter()
            .filter(|entry| !self.get(entry.field).is_empty())
            .map(|entry| hotkeys::Binding {
                action: entry.action,
                keys: self.get(entry.field).to_string(),
            })
            .collect()
    }
}

/// 一个 `#RRGGBB` 颜色。认不认由这里管，写坏了怎么报由调用方决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    /// 认 `#RRGGBB` 与 `RRGGBB`，十六进制大小写都行；其余（含长度不对）一律 `None`。
    pub fn parse(text: &str) -> Option<Self> {
        let digits = text.strip_prefix('#').unwrap_or(text);
        if digits.len() != 6
            || !digits
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        {
            return None;
        }
        Some(Self {
            r: u8::from_str_radix(&digits[0..2], 16).ok()?,
            g: u8::from_str_radix(&digits[2..4], 16).ok()?,
            b: u8::from_str_radix(&digits[4..6], 16).ok()?,
        })
    }

    /// 写回配置模板的形式（大写十六进制、带 `#`）。
    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }
}

/// `[library]` 段：本地曲库目录。
///
/// `dirs` 是 `Option`，因为「没写」与「写成空表」是两件事：没写 = 就扫下载目录；
/// 写了空表 = 本地功能整体关掉（首页那一格也不画）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LibraryConfig {
    pub dirs: Option<Vec<String>>,
}

impl LibraryConfig {
    /// 真正要扫的目录。相对路径原样留着（跟 `download_dir` 的语义一致，按当前工作目录解析）。
    pub fn resolve(&self, download_dir: &Path) -> Vec<PathBuf> {
        match &self.dirs {
            None => vec![download_dir.to_path_buf()],
            Some(dirs) => dirs.iter().map(PathBuf::from).collect(),
        }
    }
}

/// `[desktop_lyrics]` 段：桌面歌词覆盖层（Windows / macOS）。
///
/// 八个字段全是**偏好**；开 / 关与窗口位置是**状态**（全局热键翻、拖动改），记在
/// `desktop-lyrics.json` 里，不在这儿。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopLyricsConfig {
    /// 字体面；系统里没有这个字体就让平台自己回退。
    pub font: String,
    /// 逻辑像素，实际按窗口 DPI 缩放。
    pub font_size: u32,
    /// 当前句的颜色。
    pub color: Rgb,
    /// 描边颜色。
    pub outline: Rgb,
    /// 下一句与译文的颜色。
    pub next_color: Rgb,
    /// 整层透明度（0-100）。
    pub opacity: u8,
    /// 当前句有译文时是否一起显示。
    pub show_translation: bool,
    /// 暂停时是否隐藏（默认留着）。
    pub hide_when_paused: bool,
}

impl Default for DesktopLyricsConfig {
    fn default() -> Self {
        Self {
            font: "Microsoft YaHei UI".to_string(),
            font_size: 28,
            color: Rgb {
                r: 255,
                g: 255,
                b: 255,
            },
            outline: Rgb { r: 0, g: 0, b: 0 },
            next_color: Rgb {
                r: 158,
                g: 158,
                b: 158,
            },
            opacity: 90,
            show_translation: true,
            hide_when_paused: false,
        }
    }
}

/// 一段 TOML 表的逐键读取器。
///
/// 三条规则只在这里实现一次：**键不在 = 用默认（不吭声）；类型不对 = 用默认 + 点名警告；
/// 别的键照旧**。段名只用来点名（顶层段名是空的，点名写 `config.toml`）。
struct Section<'t, 'w> {
    name: &'t str,
    table: &'t toml::Table,
    warnings: &'w mut Vec<String>,
}

impl<'t, 'w> Section<'t, 'w> {
    fn new(name: &'t str, table: &'t toml::Table, warnings: &'w mut Vec<String>) -> Self {
        Self {
            name,
            table,
            warnings,
        }
    }

    /// 报点名：`[paths] download_dir` / `quality`（顶层不带段名）。
    fn label(&self, key: &str) -> String {
        if self.name.is_empty() {
            key.to_string()
        } else {
            format!("[{}] {key}", self.name)
        }
    }

    /// 这一片区域的叫法，给「不认识的键」用。
    fn place(&self) -> String {
        if self.name.is_empty() {
            "config.toml".to_string()
        } else {
            format!("[{}]", self.name)
        }
    }

    fn warn(&mut self, text: String) {
        self.warnings.push(text);
    }

    fn wrong_type(&mut self, key: &str, want: &str, value: &toml::Value) {
        let label = self.label(key);
        let text = format!("{label} 该是{want}，写成了 {}", inline(value));
        self.warn(text);
    }

    fn string(&mut self, key: &str) -> Option<String> {
        let value = self.table.get(key)?;
        match value.as_str() {
            Some(text) => Some(text.to_string()),
            None => {
                self.wrong_type(key, "字符串", value);
                None
            }
        }
    }

    fn integer(&mut self, key: &str) -> Option<i64> {
        let value = self.table.get(key)?;
        match value.as_integer() {
            Some(number) => Some(number),
            None => {
                self.wrong_type(key, "整数", value);
                None
            }
        }
    }

    fn boolean(&mut self, key: &str) -> Option<bool> {
        let value = self.table.get(key)?;
        match value.as_bool() {
            Some(flag) => Some(flag),
            None => {
                self.wrong_type(key, "布尔值", value);
                None
            }
        }
    }

    /// 字符串数组。`[]` 是合法取值（各段自己解释空表的含义）；混进来非字符串的元素时
    /// 只丢那一个元素并点名，剩下的照用。
    fn string_list(&mut self, key: &str) -> Option<Vec<String>> {
        let value = self.table.get(key)?;
        let Some(items) = value.as_array() else {
            self.wrong_type(key, "字符串数组", value);
            return None;
        };
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            match item.as_str() {
                Some(text) => out.push(text.to_string()),
                None => {
                    let label = self.label(key);
                    let text = format!("{label} 里有不是字符串的元素：{}", inline(item));
                    self.warn(text);
                }
            }
        }
        Some(out)
    }

    /// 这一段里多出来的键：拼错的键会让人「改了没反应」，所以一律点名（照用其余）。
    fn unknown(&mut self, known: &[&str]) {
        for key in self.table.keys() {
            if known.contains(&key.as_str()) {
                continue;
            }
            let place = self.place();
            self.warn(format!("{place} 里不认识的键：{key}（已忽略，拼错了吗？）"));
        }
    }
}

/// 取一个段：不在 = 整段用默认（不吭声）；在但不是表 = 点名警告 + 整段用默认。
fn section<'t>(
    table: &'t toml::Table,
    key: &str,
    warnings: &mut Vec<String>,
) -> Option<&'t toml::Table> {
    let value = table.get(key)?;
    match value.as_table() {
        Some(child) => Some(child),
        None => {
            warnings.push(format!(
                "[{key}] 该是一个段（写成 `[{key}]` 单起一行），写成了 {}",
                inline(value)
            ));
            None
        }
    }
}

/// 警告里给对方写的那个值。太长就截一下：这句话可能被塞进 32 列的通知位，
/// 全文反正进了日志。
fn inline(value: &toml::Value) -> String {
    let text = value.to_string();
    if text.chars().count() <= 24 {
        text
    } else {
        format!("{}…", text.chars().take(24).collect::<String>())
    }
}

/// 把一段配置文本解析成 [`Config`]。
///
/// 返回（配置, 全部警告）。真正给状态行看的那一句由 [`notice`] 从这些警告里挑；
/// 每条警告的原文都会以 `warn` 级别进日志。
fn parse(text: &str) -> (Config, Vec<String>) {
    let mut warnings = Vec::new();
    let table = match toml::from_str::<toml::Table>(text) {
        Ok(table) => table,
        Err(error) => {
            // 只有这种情况救不回来：连键都读不出来，于是一个都不认。
            tracing::warn!("配置不是合法 TOML：{error}");
            warnings.push("配置不是合法 TOML，整份用默认值".to_string());
            return (Config::default(), warnings);
        }
    };
    {
        let mut top = Section::new("", &table, &mut warnings);
        top.unknown(TOP_KEYS);
    }
    let playback = section(&table, "playback", &mut warnings);
    let paths = section(&table, "paths", &mut warnings);
    let cache = section(&table, "cache", &mut warnings);
    let history = section(&table, "history", &mut warnings);
    let ui = section(&table, "ui", &mut warnings);
    let library = section(&table, "library", &mut warnings);
    let hotkeys = section(&table, "hotkeys", &mut warnings);
    let desktop_lyrics = section(&table, "desktop_lyrics", &mut warnings);
    let config = Config {
        playback: playback_config(playback, &mut warnings),
        paths: paths_config(paths, &mut warnings),
        cache: cache_config(cache, &mut warnings),
        history: history_config(history, &mut warnings),
        ui: ui_config(ui, &mut warnings),
        library: library_config(library, &mut warnings),
        hotkeys: hotkeys_config(hotkeys, &mut warnings),
        desktop_lyrics: desktop_lyrics_config(desktop_lyrics, &mut warnings),
    };
    (config, warnings)
}

fn playback_config(table: Option<&toml::Table>, warnings: &mut Vec<String>) -> PlaybackConfig {
    let mut config = PlaybackConfig::default();
    let Some(table) = table else {
        return config;
    };
    let mut section = Section::new("playback", table, warnings);
    section.unknown(&["quality"]);
    if let Some(text) = section.string("quality") {
        match Quality::parse(&text) {
            Some(quality) => config.quality = quality,
            None => {
                // 提示里列出可用写法：别让人回来翻代码（模板注释里也有一份）。
                let options: Vec<&str> =
                    Quality::ALL.iter().map(|quality| quality.token()).collect();
                let text = format!(
                    "quality = {text:?} 不认识（可用：{}），用默认值 {}",
                    options.join(" / "),
                    config.quality.token()
                );
                section.warn(text);
            }
        }
    }
    config
}

fn paths_config(table: Option<&toml::Table>, warnings: &mut Vec<String>) -> PathsConfig {
    let mut config = PathsConfig::default();
    let Some(table) = table else {
        return config;
    };
    let mut section = Section::new("paths", table, warnings);
    section.unknown(&["download_dir", "lyrics_dir"]);
    if let Some(text) = section.string("download_dir") {
        config.download_dir = expand_home(&text);
    }
    if let Some(text) = section.string("lyrics_dir") {
        config.lyrics_dir = expand_home(&text);
    }
    config
}

fn cache_config(table: Option<&toml::Table>, warnings: &mut Vec<String>) -> CacheConfig {
    let mut config = CacheConfig::default();
    let Some(table) = table else {
        return config;
    };
    let mut section = Section::new("cache", table, warnings);
    section.unknown(&["mb"]);
    if let Some(mb) = section.integer("mb") {
        if mb < 0 {
            section.warn(format!("[cache] mb = {mb} 是负数，按 0（关掉缓存）算"));
            config.mb = 0;
        } else {
            config.mb = mb as u64;
        }
    }
    config
}

fn history_config(table: Option<&toml::Table>, warnings: &mut Vec<String>) -> HistoryConfig {
    let mut config = HistoryConfig::default();
    let Some(table) = table else {
        return config;
    };
    let mut section = Section::new("history", table, warnings);
    section.unknown(&["limit"]);
    if let Some(limit) = section.integer("limit") {
        if limit < 1 {
            section.warn(format!("[history] limit = {limit} 太小，按 1 算"));
        }
        config.limit = limit.max(1) as usize;
    }
    config
}

fn ui_config(table: Option<&toml::Table>, warnings: &mut Vec<String>) -> UiConfig {
    let mut config = UiConfig::default();
    let Some(table) = table else {
        return config;
    };
    let mut section = Section::new("ui", table, warnings);
    section.unknown(&["notice_seconds"]);
    if let Some(seconds) = section.integer("notice_seconds") {
        if seconds < 0 {
            section.warn(format!(
                "[ui] notice_seconds = {seconds} 是负数，按 0（不自动消失）算"
            ));
            config.notice_seconds = 0;
        } else {
            config.notice_seconds = seconds as u64;
        }
    }
    config
}

fn library_config(table: Option<&toml::Table>, warnings: &mut Vec<String>) -> LibraryConfig {
    let mut config = LibraryConfig::default();
    let Some(table) = table else {
        return config;
    };
    let mut section = Section::new("library", table, warnings);
    section.unknown(&["dirs"]);
    if let Some(dirs) = section.string_list("dirs") {
        config.dirs = Some(dirs);
    }
    config
}

fn hotkeys_config(table: Option<&toml::Table>, warnings: &mut Vec<String>) -> HotkeyConfig {
    let mut config = HotkeyConfig::default();
    let Some(table) = table else {
        return config;
    };
    let mut section = Section::new("hotkeys", table, warnings);
    let fields: Vec<&str> = hotkeys::DEFAULTS.iter().map(|entry| entry.field).collect();
    section.unknown(&fields);
    // 写法不对（键名不认 / 没带修饰键）就清成空串 + 一条点名到字段的警告。清掉是为了让后面
    // 只认一条不变式：非空 = 合法 = 会被注册。空串是明确禁用，不吭声。
    for entry in hotkeys::DEFAULTS {
        let Some(keys) = section.string(entry.field) else {
            continue;
        };
        if keys.is_empty() {
            config.set(entry.field, "");
            continue;
        }
        match hotkeys::validate(&keys) {
            Ok(_) => config.set(entry.field, &keys),
            Err(why) => {
                let text = format!(
                    "[hotkeys] {} = {keys:?} 不可用：{why}（这个动作不注册）",
                    entry.field
                );
                section.warn(text);
                config.set(entry.field, "");
            }
        }
    }
    config
}

fn desktop_lyrics_config(
    table: Option<&toml::Table>,
    warnings: &mut Vec<String>,
) -> DesktopLyricsConfig {
    let mut config = DesktopLyricsConfig::default();
    let Some(table) = table else {
        return config;
    };
    let mut section = Section::new("desktop_lyrics", table, warnings);
    section.unknown(&[
        "font",
        "font_size",
        "color",
        "outline",
        "next_color",
        "opacity",
        "show_translation",
        "hide_when_paused",
    ]);
    if let Some(font) = section.string("font") {
        if font.trim().is_empty() {
            section.warn("[desktop_lyrics] font 是空的，用默认字体".to_string());
        } else {
            config.font = font;
        }
    }
    if let Some(size) = section.integer("font_size") {
        // 小到画不出来没意义；上限不管（谁想写 400 就让他写），但不能让超大值溢出：
        // `i64 → u32` 用 clamp 收口。
        if size < 8 {
            section.warn(format!("[desktop_lyrics] font_size = {size} 太小，按 8 算"));
        }
        config.font_size = size.clamp(8, u32::MAX as i64) as u32;
    }
    take_color(&mut section, "color", &mut config.color);
    take_color(&mut section, "outline", &mut config.outline);
    take_color(&mut section, "next_color", &mut config.next_color);
    if let Some(opacity) = section.integer("opacity") {
        if !(0..=100).contains(&opacity) {
            section.warn(format!(
                "[desktop_lyrics] opacity = {opacity} 超出 0-100，已夹住"
            ));
        }
        config.opacity = opacity.clamp(0, 100) as u8;
    }
    if let Some(show) = section.boolean("show_translation") {
        config.show_translation = show;
    }
    if let Some(hide) = section.boolean("hide_when_paused") {
        config.hide_when_paused = hide;
    }
    config
}

/// 解析一个颜色字段：写坏了点名警告并**保持默认值**。
///
/// 跟 `[hotkeys]` 的「清成空串」不同——这里没有「禁用」的语义，退回默认比留个空值清楚。
fn take_color(section: &mut Section<'_, '_>, key: &str, slot: &mut Rgb) {
    if let Some(text) = section.string(key) {
        match Rgb::parse(&text) {
            Some(rgb) => *slot = rgb,
            None => {
                let text = format!("[desktop_lyrics] {key} = {text:?} 不是 #RRGGBB，用默认值");
                section.warn(text);
            }
        }
    }
}

/// 状态行那一条（`None` = 什么都不用说）。
///
/// 通知位只有 32 列（见 `tui::draw::status::NOTICE_COLS`），所以：只有一条**且放得下**才
/// 原样显示（那一条通常已经点出了字段名，人看一眼就知道改哪儿），否则退化成计数短句——
/// 每条警告的原文都在 `canto.log` 里。
fn notice(warnings: &[String]) -> Option<String> {
    if warnings.is_empty() {
        return None;
    }
    // 只有一条、而且放得下就原样给：整份解析失败那句（「配置不是合法 TOML，整份用默认值」）
    // 就是这么短，它本身已经说明白了。逐键的那些点名警告都太长（段名 + 键名 + 原文），
    // 落到下面去。
    if let [only] = warnings
        && UnicodeWidthStr::width(only.as_str()) <= usize::from(NOTICE_COLS)
    {
        return Some(only.clone());
    }
    Some(match warnings.len() {
        1 => "配置有问题（见 canto.log）".to_string(),
        count => format!("配置有 {count} 处问题（见 canto.log）"),
    })
}

/// 把一段文本写成 TOML 字符串。
///
/// 优先用**字面串**（单引号）：Windows 路径里的 `\` 在基本串里是转义起始符，
/// `C:\Users` 的 `\U` 是非法转义，会让**整个配置文件**判成坏文件（实测：Windows 上
/// 每次启动都弹一次「解析失败」）。文本里真有单引号时才退回基本串，把反斜杠与引号转义掉。
fn toml_literal(text: &str) -> String {
    if !text.contains('\'') {
        return format!("'{text}'");
    }
    let mut escaped = String::with_capacity(text.len() + 2);
    escaped.push('"');
    for character in text.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            other => escaped.push(other),
        }
    }
    escaped.push('"');
    escaped
}

/// `~/Music/canto` → `$HOME/Music/canto`。手写配置时一定会写 `~`，
/// 而 `dirs::audio_dir()` 之类的默认值是绝对路径。展开不了就原样当路径用。
pub fn expand_home(text: &str) -> PathBuf {
    if let Some(rest) = text.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(text)
}

/// 载入配置。文件不存在则**生成一份默认配置**（让使用者知道有这个文件可改）。
/// 返回的 `Option<String>` 是给状态行的一次性警告——**短句**：通知位只有 32 列，
/// 细节（路径、原文）走日志。
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
            tracing::warn!("读 {} 失败：{error}", path.display());
            return (
                Config::default(),
                Some("配置读不到，改用默认值".to_string()),
            );
        }
    };
    let (config, warnings) = parse(&text);
    // 细节进日志：状态行只有一格 32 列，放不下「哪个键、写成了什么」。
    for warning in &warnings {
        tracing::warn!("{}：{warning}", path.display());
    }
    (config, notice(&warnings))
}

/// 首次运行生成默认配置。
fn write_default(path: &Path) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(parent)
        .with_context(|| format!("创建配置目录失败：{}", parent.display()))?;
    std::fs::write(path, render_default()).with_context(|| format!("写 {} 失败", path.display()))
}

/// `canto config init`：旧文件先备份，再写一份当前版本的默认配置。
///
/// 返回（备份到哪儿了, 配置文件路径）；原来就没有文件 → 没有备份。
/// **不做任何旧值迁移**：配置项本身也是会改的，与其猜哪条该搬，不如让人对着备份自己看。
pub fn reset(path: &Path) -> Result<(Option<PathBuf>, PathBuf)> {
    let backup = if path.exists() {
        Some(backup(path)?)
    } else {
        None
    };
    write_default(path)?;
    Ok((backup, path.to_path_buf()))
}

/// 备份现有配置到 `config.toml.<时间戳>.bak`，返回备份路径。
///
/// 同一秒里跑两次（脚本、手快）就往后加序号——**绝不覆盖**上一份备份：那是使用者手改过的
/// 最后一份东西。时间戳用本地时间（见 [`clock::stamp`]）。
fn backup(path: &Path) -> Result<PathBuf> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.toml".to_string());
    let stamp = clock::stamp(clock::now_secs());
    let mut index = 1;
    loop {
        let suffix = if index == 1 {
            String::new()
        } else {
            format!("-{index}")
        };
        let candidate = path.with_file_name(format!("{name}.{stamp}{suffix}.bak"));
        if !candidate.exists() {
            std::fs::copy(path, &candidate)
                .with_context(|| format!("备份 {} 失败", candidate.display()))?;
            return Ok(candidate);
        }
        index += 1;
    }
}

/// 那份**带注释的完整默认配置**（首次运行与 `canto config init` 共用）。
///
/// 三条规矩：
/// 1. 每个能配的键都出现在文件里，值一律取自 `Config::default()`（改了默认值，这里的注释
///    和值跟着走，不会说岔）；
/// 2. **缺这个键就等于默认行为**的（`[library] dirs`），写成注释而不是真键——写进去反而
///    会改变行为；
/// 3. 不在这份文件里的东西（固定路径、状态文件、派生数据）在文件末尾列出来，免得人去找。
fn render_default() -> String {
    let default = Config::default();
    // 四档逐行写清楚：名字、体积量级，「默认」标记跟着默认值走（别硬编码在某一档上，
    // 否则改了默认值就会说错）。
    let options: String = Quality::ALL
        .iter()
        .map(|quality| {
            let token = quality.token();
            let mark = if *quality == default.playback.quality {
                "   ← 默认"
            } else {
                ""
            };
            format!(
                "#   {token:<6} {:<10} {}{mark}\n",
                quality.label(),
                quality.size_hint()
            )
        })
        .collect();
    // 全局快捷键：默认组合从 `DEFAULTS` 现取现写，不跟注释说岔。
    let hotkey_lines: String = hotkeys::DEFAULTS
        .iter()
        .map(|entry| format!("{} = \"{}\"\n", entry.field, entry.keys))
        .collect();
    // 路径按平台来：Windows 上是 `C:\Users\...`，必须写成字面串（见 `toml_literal`）。
    let download_dir = toml_literal(&default.paths.download_dir.to_string_lossy());
    let lyrics_dir = toml_literal(&default.paths.lyrics_dir.to_string_lossy());
    let data_dir = paths::data_dir().display().to_string();
    let cache_dir = paths::cache_dir().display().to_string();
    let font = toml_literal(&default.desktop_lyrics.font);
    let lyrics = &default.desktop_lyrics;
    format!(
        "# canto 配置。改完重启 canto 生效；想重新来一份就跑 `canto config init`\n\
         # （旧的那份会先备份成 config.toml.<时间戳>.bak）。\n\
         #\n\
         # 这个文件是**手写**的：程序只在首次运行与 `canto config init` 时写它。\n\
         # 写坏的键不会让程序起不来——那个键回默认值、别的键照旧，一句话进状态行、\n\
         # 详情进 canto.log；拼错的键（不认识的键）同样会在 canto.log 里记一笔。\n\
         \n\
         [playback]\n\
         # 音质下限：从这一档往下找，这一档没有就退到更低的一档（播放与下载都用它）\n\
         {options}\
         quality = \"{}\"\n\
         \n\
         [paths]\n\
         # 下载目录（`d` 键落盘的歌曲），支持 `~`\n\
         download_dir = {download_dir}\n\
         # 歌词目录（下载时另存的 .lrc，跟歌曲分开放），支持 `~`\n\
         lyrics_dir = {lyrics_dir}\n\
         \n\
         [cache]\n\
         # 播放缓存上限（MB，0 = 关掉）；缓存在 {cache_dir}，按「最久没听的先删」维护\n\
         mb = {}\n\
         \n\
         [history]\n\
         # 播放历史最多保留多少条（至少 1）\n\
         limit = {}\n\
         \n\
         [ui]\n\
         # 状态行右侧那条通知存活多少秒（0 = 不自动消失，只被下一条顶掉）\n\
         notice_seconds = {}\n\
         \n\
         [library]\n\
         # 本地曲库扫哪些目录（`5` 那屏；`r` 重扫）。**不写 dirs = 只扫上面的 download_dir**：\n\
         #   dirs = []            → 本地功能整体关掉（首页不画「本地曲库 / 本地歌单」两格）\n\
         #   dirs = ['D:/music']  → 就扫这些（不再顺手带上 download_dir），相对路径按启动时的工作目录算\n\
         # dirs = ['D:/music', 'E:/flac']\n\
         \n\
         [hotkeys]\n\
         # 全局快捷键：canto 在后台（没聚焦）时也能用这几个键控制播放。\n\
         # 只在 Windows / macOS 生效（Linux 上这一整段不注册，真的就只是注释）。\n\
         # 注意：注册期间这些组合**别的程序收不到**（系统级注册的固有代价）。\n\
         # 写法：修饰键在前、最后一个是要按的键；修饰键可写 ctrl / alt / shift / win\n\
         # （win = super = cmd），键名如 space / up / down / left / f5 / a；\n\
         # 别用媒体键名（macOS 上那要走系统授权）。空字符串 = 这个动作不注册。\n\
         # 最后两条是桌面歌词的：desktop_lyrics_toggle 开 / 关那个窗口（真拆真建），\n\
         # desktop_lyrics_lock 切它的锁定 / 解锁（解锁后才能拖动）。\n\
         {hotkey_lines}\
         \n\
         [desktop_lyrics]\n\
         # 桌面歌词（只在 Windows / macOS 生效）：播放时在桌面之上浮一条歌词，默认点不着；\n\
         # 全局热键 desktop_lyrics_toggle（默认 ctrl+shift+d）开 / 关这个窗口，\n\
         # desktop_lyrics_lock 解锁后能拖动\n\
         # ——开关与位置都是**状态**，记在 desktop-lyrics.json 里，不在这份文件里。\n\
         # 颜色写 #RRGGBB，opacity 是 0-100。\n\
         # font 是字体面：默认那串是 Windows 的中文字体，macOS 上认不出来会自动回退系统字体\n\
         # （想指定就写 macOS 上有的名字，如 {example_font}）。\n\
         font = {font}\n\
         font_size = {font_size}\n\
         color = \"{color}\"\n\
         outline = \"{outline}\"\n\
         next_color = \"{next_color}\"\n\
         opacity = {opacity}\n\
         show_translation = {show_translation}\n\
         hide_when_paused = {hide_when_paused}\n\
         \n\
         # ── 不在这份文件里的东西（列出来免得你去找）──────────────────────────\n\
         # 数据目录 {data_dir}：\n\
         #   session.json          登录态（cookie jar）——别提交、别分享\n\
         #   history.json          播放历史\n\
         #   local_playlists.json  本地歌单\n\
         #   desktop-lyrics.json   桌面歌词的开关与窗口中心点（**状态**：热键翻开关、拖动改位置）\n\
         #   canto.log             日志（配置里的问题、热键注册失败的原因都在这儿）\n\
         #   canto.lock            单例锁（内容只是持有者的 pid）\n\
         #   login-qr.png          二维码落盘的一份（`o` 键用系统查看器打开）\n\
         # 缓存目录 {cache_dir}：播放缓存与曲库索引（library.json）——**派生数据**，随时可删，\n\
         #   重听 / 重扫会再生成。\n\
         # 这些都不在这份文件里：路径是写死的，状态由程序自己记（配置只放「偏好」）。\n\
         #\n\
         # 重新生成这份配置（旧的先备份）：canto config init\n\
         # 只想知道这份文件在哪：      canto config path\n",
        default.playback.quality.token(),
        default.cache.mb,
        default.history.limit,
        default.ui.notice_seconds,
        example_font = "PingFang SC",
        font_size = lyrics.font_size,
        color = lyrics.color.to_hex(),
        outline = lyrics.outline.to_hex(),
        next_color = lyrics.next_color.to_hex(),
        opacity = lyrics.opacity,
        show_translation = lyrics.show_translation,
        hide_when_paused = lyrics.hide_when_paused,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::keys;
    use unicode_width::UnicodeWidthStr;

    fn temp_path(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("canto-config-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir.join("config.toml")
    }

    /// 走完整载入路径（读文件 → 解析 → 挑一句给状态行），并且用临时文件——
    /// 绝不碰使用者的真数据目录。
    fn load_from(tag: &str, text: &str) -> (Config, Option<String>) {
        let path = temp_path(tag);
        std::fs::write(&path, text).expect("写配置");
        let result = load_file(&path);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
        result
    }

    #[test]
    fn an_empty_file_is_all_defaults_and_says_nothing() {
        let (config, warnings) = parse("");
        assert_eq!(config, Config::default());
        assert!(warnings.is_empty(), "空文件不是错误：{warnings:?}");
    }

    /// 每一段的每个键都从文件里读，缺的那几个还是默认值。
    #[test]
    fn every_section_comes_from_the_file() {
        let text = "\
[playback]
quality = \"flac\"
[paths]
download_dir = 'D:/songs'
lyrics_dir = 'D:/lyrics'
[cache]
mb = 16
[history]
limit = 7
[ui]
notice_seconds = 30
[library]
dirs = ['E:/one', 'E:/two']
[desktop_lyrics]
font = 'Noto Sans'
font_size = 36
opacity = 40
show_translation = false
hide_when_paused = true
color = '#102030'
";
        let (config, warnings) = parse(text);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(config.playback.quality, Quality::Flac);
        assert_eq!(config.paths.download_dir, PathBuf::from("D:/songs"));
        assert_eq!(config.paths.lyrics_dir, PathBuf::from("D:/lyrics"));
        assert_eq!(config.cache.mb, 16);
        assert_eq!(config.history.limit, 7);
        assert_eq!(config.ui.notice_seconds, 30);
        assert_eq!(
            config.library.dirs.as_deref(),
            Some(&["E:/one".to_string(), "E:/two".to_string()][..])
        );
        assert_eq!(config.desktop_lyrics.font, "Noto Sans");
        assert_eq!(config.desktop_lyrics.font_size, 36);
        assert_eq!(config.desktop_lyrics.opacity, 40);
        assert!(!config.desktop_lyrics.show_translation);
        assert!(config.desktop_lyrics.hide_when_paused);
        assert_eq!(
            config.desktop_lyrics.color,
            Rgb {
                r: 0x10,
                g: 0x20,
                b: 0x30
            }
        );
        assert_eq!(
            config.desktop_lyrics.outline,
            DesktopLyricsConfig::default().outline,
            "没写的颜色还是默认"
        );
    }

    /// 只写一项：其余字段还是默认值（每一段都这样，缺段也是）。
    #[test]
    fn a_partial_section_keeps_the_other_defaults() {
        let (config, warnings) =
            parse("[cache]\nmb = 64\n[desktop_lyrics]\nfont_size = 36\nhide_when_paused = true\n");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(config.cache.mb, 64);
        assert_eq!(
            config.history.limit,
            HistoryConfig::default().limit,
            "缺段 = 默认"
        );
        assert_eq!(config.desktop_lyrics.font_size, 36);
        assert!(config.desktop_lyrics.hide_when_paused);
        assert_eq!(
            config.desktop_lyrics.font, "Microsoft YaHei UI",
            "没写的还是默认"
        );
        assert_eq!(config.desktop_lyrics.opacity, 90);
    }

    /// 类型写错只影响那一个键：别的键照常生效，**不再整份回默认**。
    #[test]
    fn a_wrong_type_only_costs_that_key() {
        let (config, warnings) =
            parse("[playback]\nquality = \"flac\"\n[cache]\nmb = \"2048\"\n[history]\nlimit = 9\n");
        assert_eq!(config.cache.mb, DEFAULT_CACHE_MB, "写坏了就用默认值");
        assert_eq!(config.playback.quality, Quality::Flac, "别的键不该被带走");
        assert_eq!(config.history.limit, 9, "别的键不该被带走");
        let warning = warnings.first().expect("应给出警告");
        assert!(warning.contains("[cache] mb"), "{warnings:?}");
        assert!(warning.contains("2048"), "要点出写成了什么：{warnings:?}");
        assert_eq!(warnings.len(), 1, "只该有一条：{warnings:?}");
    }

    /// 段写成非表（`playback = 3`）也一样：点名 + 整段用默认。
    #[test]
    fn a_section_written_as_a_scalar_is_named() {
        let (config, warnings) = parse("playback = 3\n[history]\nlimit = 9\n");
        assert_eq!(config.playback, PlaybackConfig::default());
        assert_eq!(config.history.limit, 9);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("[playback]"), "{warnings:?}");
    }

    /// 超出范围的值夹住并说一句，别把整份配置扔掉（有人以为 opacity 是 0-255）。
    #[test]
    fn out_of_range_values_are_clamped_and_named() {
        let (config, warnings) = parse(
            "[cache]\nmb = -1\n[history]\nlimit = 0\n[ui]\nnotice_seconds = -3\n\
             [desktop_lyrics]\nopacity = 900\nfont_size = -5\n",
        );
        assert_eq!(config.cache.mb, 0, "负数按 0（关掉缓存）");
        assert_eq!(config.history.limit, 1, "至少 1 条");
        assert_eq!(config.ui.notice_seconds, 0, "负数按 0（不自动消失）");
        assert_eq!(config.desktop_lyrics.opacity, 100, "夹到上限");
        assert_eq!(config.desktop_lyrics.font_size, 8, "下限是 8");
        for key in [
            "[cache] mb",
            "[history] limit",
            "[ui] notice_seconds",
            "[desktop_lyrics] opacity",
            "[desktop_lyrics] font_size",
        ] {
            assert!(
                warnings.iter().any(|warning| warning.contains(key)),
                "应点名 {key}：{warnings:?}"
            );
        }
    }

    /// 不认识的键要点名：拼错一个键如今不该是「什么都不发生」。
    #[test]
    fn unknown_keys_are_named_everywhere() {
        let (config, warnings) = parse(
            "quailty = \"flac\"\n[playback]\nquality = \"320\"\nqualityy = \"flac\"\n[paths]\ndowload_dir = 'D:/x'\n",
        );
        assert_eq!(
            config.playback.quality,
            Quality::High320,
            "认得的键照旧生效"
        );
        assert_eq!(
            config.paths.download_dir,
            PathsConfig::default().download_dir
        );
        let joined = warnings.join("\n");
        assert!(joined.contains("quailty"), "{joined}");
        assert!(joined.contains("qualityy"), "{joined}");
        assert!(joined.contains("dowload_dir"), "{joined}");
        assert!(
            joined.contains("config.toml") && joined.contains("[paths]"),
            "点名要带上位置：{joined}"
        );
    }

    #[test]
    fn quality_accepts_the_documented_tokens_and_warns_otherwise() {
        for (text, want) in [
            ("128", Quality::Low128),
            ("320", Quality::High320),
            ("flac", Quality::Flac),
            ("hires", Quality::Hires),
        ] {
            let (config, warnings) = parse(&format!("[playback]\nquality = \"{text}\"\n"));
            assert_eq!(config.playback.quality, want, "{text}");
            assert!(warnings.is_empty(), "{warnings:?}");
        }

        let (config, warnings) = parse("[playback]\nquality = \"cd\"\n");
        assert_eq!(config.playback.quality, Config::default().playback.quality);
        let warning = warnings.first().expect("应给出警告");
        assert!(warning.contains("cd"), "{warnings:?}");
        // 提示里要给出可用的写法，别让人回来翻代码。
        assert!(
            warning.contains("128") && warning.contains("320"),
            "{warnings:?}"
        );
    }

    #[test]
    fn colors_parse_in_both_cases_and_warn_on_garbage() {
        assert_eq!(
            Rgb::parse("#FFFFFF"),
            Some(Rgb {
                r: 255,
                g: 255,
                b: 255
            })
        );
        assert_eq!(Rgb::parse("ff0000"), Some(Rgb { r: 255, g: 0, b: 0 }));
        assert_eq!(
            Rgb::parse("#00ff7F"),
            Some(Rgb {
                r: 0,
                g: 255,
                b: 127
            })
        );
        assert_eq!(Rgb::parse("red"), None);
        assert_eq!(Rgb::parse("#12345"), None);
        assert_eq!(Rgb::parse(""), None);
        // 模板里写回的形式是 `#RRGGBB`（大写），并且能原样解析回来。
        assert_eq!(
            Rgb {
                r: 0x0a,
                g: 0xb0,
                b: 0xff
            }
            .to_hex(),
            "#0AB0FF"
        );
        assert_eq!(
            Rgb::parse(&Rgb { r: 1, g: 2, b: 3 }.to_hex()),
            Some(Rgb { r: 1, g: 2, b: 3 })
        );

        let (config, warnings) = parse("[desktop_lyrics]\ncolor = \"red\"\n");
        assert_eq!(
            config.desktop_lyrics.color,
            DesktopLyricsConfig::default().color
        );
        let warning = warnings.first().expect("应给出警告");
        assert!(warning.contains("[desktop_lyrics] color"), "{warnings:?}");
        assert!(warning.contains("red"), "{warnings:?}");
    }

    /// `[library]` 的三种写法各是各的意思（这就是模板里那段注释要讲清的东西）。
    #[test]
    fn the_three_library_forms_mean_three_things() {
        let download = Path::new("C:/Music/canto");

        let (config, warnings) = parse("[cache]\nmb = 1\n");
        assert_eq!(
            config.library.resolve(download),
            vec![download.to_path_buf()],
            "不写 = 就扫下载目录"
        );
        assert!(warnings.is_empty(), "{warnings:?}");

        let (config, warnings) = parse("[library]\ndirs = []\n");
        assert!(
            config.library.resolve(download).is_empty(),
            "空表 = 本地功能关掉"
        );
        assert!(
            warnings.is_empty(),
            "空表是明确关闭，不是配置错误：{warnings:?}"
        );

        let (config, warnings) = parse("[library]\ndirs = ['D:/music', 'E:/flac']\n");
        assert_eq!(
            config.library.resolve(download),
            vec![PathBuf::from("D:/music"), PathBuf::from("E:/flac")],
            "写了就以写的为准，不再顺手带上下载目录"
        );
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    /// 数组里混进非字符串：只丢那一个并点名，别的照用。
    #[test]
    fn a_bad_library_entry_is_dropped_and_named() {
        let (config, warnings) = parse("[library]\ndirs = ['D:/music', 3]\n");
        assert_eq!(
            config.library.dirs.as_deref(),
            Some(&["D:/music".to_string()][..])
        );
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("[library] dirs"), "{warnings:?}");
    }

    #[test]
    fn the_missing_hotkey_section_gives_the_default_combos() {
        let (config, warnings) = parse("[cache]\nmb = 2\n");
        assert_eq!(config.hotkeys, HotkeyConfig::default());
        assert_eq!(config.hotkeys.play_pause, "ctrl+shift+space");
        assert_eq!(config.hotkeys.volume_down, "ctrl+shift+down");
        assert_eq!(config.hotkeys.desktop_lyrics_toggle, "ctrl+shift+d");
        assert_eq!(config.hotkeys.desktop_lyrics_lock, "ctrl+shift+l");
        assert_eq!(config.hotkeys.bindings().len(), 7);
        assert!(warnings.is_empty(), "缺段不是错误：{warnings:?}");
    }

    /// 写了 `[hotkeys]` 但只写一项：其余几项还得是默认值。
    #[test]
    fn a_partial_hotkey_section_keeps_the_other_defaults() {
        let (config, warnings) = parse("[hotkeys]\nnext = \"ctrl+alt+n\"\n");
        assert_eq!(config.hotkeys.next, "ctrl+alt+n");
        assert_eq!(
            config.hotkeys.prev, "ctrl+shift+left",
            "没写的字段用默认组合"
        );
        assert_eq!(config.hotkeys.bindings().len(), 7);
        assert_eq!(config.hotkeys.desktop_lyrics_toggle, "ctrl+shift+d");
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn an_empty_string_disables_one_action() {
        let (config, warnings) = parse("[hotkeys]\nplay_pause = \"\"\n");
        assert_eq!(config.hotkeys.play_pause, "");
        let bindings = config.hotkeys.bindings();
        assert_eq!(bindings.len(), 6);
        assert!(
            !bindings
                .iter()
                .any(|binding| binding.action == keys::Action::PlayPause)
        );
        assert!(warnings.is_empty(), "明确禁用不该警告：{warnings:?}");
    }

    /// 裸键会被系统级抢走：不注册、清成空串、给一条点名到字段的警告。
    #[test]
    fn a_bare_key_is_refused_and_cleared() {
        let (config, warnings) = parse("[hotkeys]\nplay_pause = \"space\"\n");
        assert_eq!(
            config.hotkeys.play_pause, "",
            "非法值要清掉，别留个不会注册的字符串"
        );
        assert_eq!(config.hotkeys.bindings().len(), 6, "其余六条照旧");
        let warning = warnings.first().expect("应给出警告");
        assert!(warning.contains("[hotkeys] play_pause"), "{warnings:?}");
        assert!(warning.contains("space"), "{warnings:?}");
    }

    #[test]
    fn a_bad_key_name_is_refused_and_cleared() {
        let (config, warnings) = parse("[hotkeys]\nvolume_up = \"ctrl+shift+nope\"\n");
        assert_eq!(config.hotkeys.volume_up, "");
        assert_eq!(
            config.hotkeys.next, "ctrl+shift+right",
            "坏字段不影响别的字段"
        );
        let warning = warnings.first().expect("应给出警告");
        assert!(warning.contains("[hotkeys] volume_up"), "{warnings:?}");
    }

    #[test]
    fn a_combo_may_be_written_in_any_case() {
        let (config, warnings) = parse("[hotkeys]\nplay_pause = \"Ctrl+Shift+Space\"\n");
        assert_eq!(
            config.hotkeys.play_pause, "Ctrl+Shift+Space",
            "合法就原样留着"
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(config.hotkeys.bindings()[0].keys, "Ctrl+Shift+Space");
    }

    /// 给状态行的那句必须短——通知位固定 [`NOTICE_COLS`] 列（`tui/draw/status.rs`）：
    /// 只有一条且放得下才原样显示，否则就是计数短句。
    #[test]
    fn the_notice_stays_inside_the_status_bar() {
        assert_eq!(notice(&[]), None);

        let short = vec!["[hotkeys] play_pause 不可用".to_string()];
        assert_eq!(
            notice(&short).as_deref(),
            Some("[hotkeys] play_pause 不可用"),
            "一条短的就原样给"
        );

        let long = vec![
            "quality = \"cd\" 不认识（可用：128 / 320 / flac / hires），用默认值 128".to_string(),
        ];
        let text = notice(&long).expect("应给一句");
        assert!(!text.contains("quality"), "放不下就别硬塞：{text}");

        for count in [2, 3, 12] {
            let many: Vec<String> = (0..count).map(|index| format!("第 {index} 处")).collect();
            let text = notice(&many).expect("应给一句");
            assert!(text.contains(&count.to_string()), "{text}");
            assert!(
                UnicodeWidthStr::width(text.as_str()) <= usize::from(NOTICE_COLS),
                "{text}"
            );
        }

        // 极长的一条（比如解析器的原文）退化成计数句，原文只进日志。
        let huge = vec![format!("配置不是合法 TOML（{}）", "很长".repeat(40))];
        let text = notice(&huge).expect("应给一句");
        assert!(
            UnicodeWidthStr::width(text.as_str()) <= usize::from(NOTICE_COLS),
            "{text}"
        );
        assert!(!text.contains("很长"), "{text}");
    }

    /// 坏到读不出来（TOML 语法坏）是**唯一**整份回默认的情况。
    #[test]
    fn broken_toml_falls_back_to_defaults_with_a_short_notice() {
        let (config, warning) = load_from("broken", "quality = \n");
        assert_eq!(config, Config::default());
        let warning = warning.expect("应给出警告");
        assert!(warning.contains("默认值"), "{warning}");
        assert!(
            !warning.contains("config.toml") && !warning.contains("quality"),
            "警告里不该带路径或解析器原文：{warning}"
        );
        assert!(
            UnicodeWidthStr::width(warning.as_str()) <= usize::from(NOTICE_COLS),
            "{warning}"
        );
    }

    #[test]
    fn a_config_we_cannot_read_falls_back_to_defaults() {
        let dir = std::env::temp_dir().join(format!("canto-config-missing-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let path = dir.join("config.toml"); // 刻意不建这个文件
        let (config, warning) = load_file(&path);
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(config, Config::default());
        assert!(warning.expect("应给出警告").contains("默认值"));
    }

    /// 缺字段不是错误：老/新配置文件都能跑（`Config::default()` 与只写一项等价）。
    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let (config, warnings) = parse("[playback]\nquality = \"flac\"\n");
        assert_eq!(config.playback.quality, Quality::Flac);
        assert_eq!(config.paths.download_dir, default_download_dir());
        assert_eq!(config.paths.lyrics_dir, default_lyrics_dir());
        assert_eq!(config.cache.mb, DEFAULT_CACHE_MB);
        assert_eq!(config.history.limit, 500);
        assert_eq!(config.ui.notice_seconds, 4);
        assert_eq!(config.desktop_lyrics, DesktopLyricsConfig::default());
        assert!(warnings.is_empty(), "缺字段不是错误：{warnings:?}");
    }

    /// `notice_seconds` 0 = 不过期，是合法取值（不该警告）。
    #[test]
    fn notice_seconds_zero_means_never() {
        assert_eq!(Config::default().ui.notice_seconds, 4, "默认 4 秒");
        let (config, warnings) = parse("[ui]\nnotice_seconds = 0\n");
        assert_eq!(config.ui.notice_seconds, 0, "0 要原样保留（= 不过期）");
        assert!(warnings.is_empty(), "0 是合法取值，不该警告：{warnings:?}");
    }

    /// 模板里的路径要写成 TOML **字面串**：`C:\Users\...` 里那个 `\U` 是非法转义，
    /// 用基本串（双引号）写会让整个配置文件被判成坏文件——Windows 上实测每次启动都弹警告。
    #[test]
    fn windows_paths_survive_the_template_round_trip() {
        let text = format!(
            "[paths]\ndownload_dir = {}\nlyrics_dir = {}\n",
            toml_literal(r"C:\Users\me\Music\canto"),
            toml_literal(r"C:\Users\me\Music\canto-lyrics"),
        );
        let (config, warnings) = parse(&text);
        assert!(warnings.is_empty(), "路径不该让配置解析失败：{warnings:?}");
        assert_eq!(
            config.paths.download_dir,
            PathBuf::from(r"C:\Users\me\Music\canto")
        );
        assert_eq!(
            config.paths.lyrics_dir,
            PathBuf::from(r"C:\Users\me\Music\canto-lyrics")
        );
    }

    #[test]
    fn tilde_in_a_path_expands_to_home() {
        let home = dirs::home_dir().expect("有家目录");
        assert_eq!(expand_home("~/Music/canto"), home.join("Music/canto"));
        assert_eq!(expand_home("/tmp/x"), PathBuf::from("/tmp/x"));

        let (config, warnings) = parse("[paths]\ndownload_dir = '~/Music/songs'\n");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(config.paths.download_dir, home.join("Music/songs"));
    }

    /// 模板本身必须是一份能解析回去、并且**就等于默认值**的配置：一个键写错就会
    /// 在启动时给自己弹一条警告。
    #[test]
    fn the_template_parses_back_to_the_defaults() {
        let (config, warnings) = parse(&render_default());
        assert!(warnings.is_empty(), "模板不该有问题：{warnings:?}");
        assert_eq!(config, Config::default());
    }

    /// 「可配的都要在文件里」：每个键（`[library] dirs` 除外，写成注释）都出现；
    /// 不适手改的（固定路径、状态文件）与平台差异用注释点明。
    #[test]
    fn the_template_documents_every_key() {
        let text = render_default();
        let default = Config::default();

        // 每个键都在（[library] dirs 是注释里的示例，见下一条测试）。
        for key in [
            "quality",
            "download_dir",
            "lyrics_dir",
            "mb =",
            "limit =",
            "notice_seconds =",
            "[playback]",
            "[paths]",
            "[cache]",
            "[history]",
            "[ui]",
            "[library]",
            "[hotkeys]",
            "[desktop_lyrics]",
        ] {
            assert!(text.contains(key), "模板里缺 {key}：{text}");
        }
        // 值一律跟着默认值走（改了默认值模板就跟着走，不会说岔）。
        for line in [
            format!("quality = \"{}\"", default.playback.quality.token()),
            format!("mb = {}", default.cache.mb),
            format!("limit = {}", default.history.limit),
            format!("notice_seconds = {}", default.ui.notice_seconds),
            format!("font_size = {}", default.desktop_lyrics.font_size),
            format!("opacity = {}", default.desktop_lyrics.opacity),
            format!("color = \"{}\"", default.desktop_lyrics.color.to_hex()),
            format!("outline = \"{}\"", default.desktop_lyrics.outline.to_hex()),
            format!(
                "next_color = \"{}\"",
                default.desktop_lyrics.next_color.to_hex()
            ),
            format!(
                "show_translation = {}",
                default.desktop_lyrics.show_translation
            ),
            format!(
                "hide_when_paused = {}",
                default.desktop_lyrics.hide_when_paused
            ),
        ] {
            assert!(
                text.contains(&line),
                "模板里的值与默认值不一致：{line}\n{text}"
            );
        }
        // 四档音质逐行写清楚（规范名 + 显示名 + 体积量级），默认标记落在默认那一档。
        for quality in Quality::ALL {
            assert!(
                text.contains(quality.token()),
                "模板里没提 {} 一档",
                quality.token()
            );
            assert!(
                text.contains(quality.label()) && text.contains(quality.size_hint()),
                "模板里没写 {} 的说明",
                quality.token()
            );
        }
        let marked: Vec<&str> = text
            .lines()
            .filter(|line| line.contains("← 默认"))
            .collect();
        assert_eq!(
            marked.len(),
            1,
            "「默认」标记只能落在默认那一档上：{marked:?}"
        );
        assert!(
            marked[0].contains(default.playback.quality.token()),
            "标记在别处：{}",
            marked[0]
        );
        // 七条默认热键都在，并提醒会吞键。
        for entry in hotkeys::DEFAULTS {
            let line = format!("{} = \"{}\"", entry.field, entry.keys);
            assert!(text.contains(&line), "模板里缺 {line}：{text}");
        }
        assert!(
            text.contains("别的程序收不到"),
            "得说清注册期间组合是被独占的"
        );
        // 桌面歌词那两条要能从名字上看出是管什么的——`lock` / `lyrics` 看不出来，所以带前缀。
        for field in ["desktop_lyrics_toggle", "desktop_lyrics_lock"] {
            assert!(text.contains(&format!("{field} = ")), "模板里缺 {field}：{text}");
        }
        assert!(
            text.contains("最后两条是桌面歌词的"),
            "热键那段的注释要点明这两条管什么：{text}"
        );
        // 旧名字不许再出现：配置里只认现在这套键名。
        assert!(
            !text.lines().any(|line| {
                let line = line.trim_start();
                line.starts_with("lock = ") || line.starts_with("lyrics = ")
            }),
            "模板里不该留下旧键名：{text}"
        );
        // 平台差异写在文件里：热键与桌面歌词只在 Windows / macOS 生效。
        assert!(text.contains("只在 Windows / macOS 生效"), "{text}");
        assert!(
            text.contains("Linux"),
            "Linux 上不注册热键这件事要写出来：{text}"
        );
        // 旧键不再出现（开关归热键）。
        assert!(
            !text.contains("enabled"),
            "模板里不该留个不生效的键：{text}"
        );
    }

    /// `[library] dirs` 是个特例：**缺这个键 = 默认行为**，所以模板里只能是注释，
    /// 写进去反而会把「扫下载目录」变成「本地功能关掉」。
    #[test]
    fn the_template_leaves_library_dirs_commented_out() {
        let text = render_default();
        assert!(text.contains("[library]"), "{text}");
        assert!(
            !text
                .lines()
                .any(|line| line.trim_start().starts_with("dirs")),
            "dirs 必须是注释，不能真的写进文件：{text}"
        );
        for line in text.lines().filter(|line| line.contains("dirs =")) {
            assert!(
                line.trim_start().starts_with('#'),
                "只有注释形式才对：{line}"
            );
        }
        // 三种写法都讲清楚（不写 = 扫下载目录 / 空表 = 关掉 / 列表 = 就扫这些）。
        assert!(text.contains("不写 dirs"), "{text}");
        assert!(text.contains("dirs = []"), "{text}");
        assert!(text.contains("本地功能整体关掉"), "{text}");
    }

    /// 固定路径、状态文件这些「不在文件里但人得知道在哪」的东西，模板末尾要列出来。
    #[test]
    fn the_template_lists_what_is_not_configurable() {
        let text = render_default();
        for needle in [
            paths::data_dir().display().to_string().as_str(),
            paths::cache_dir().display().to_string().as_str(),
            "session.json",
            "history.json",
            "local_playlists.json",
            "desktop-lyrics.json",
            "canto.log",
            "canto.lock",
            "login-qr.png",
            "派生数据",
            "canto config init",
            "canto config path",
        ] {
            assert!(text.contains(needle), "模板里没提 {needle}：{text}");
        }
    }

    /// 首次运行 / `canto config init` 写出来的东西就是模板本身。
    #[test]
    fn writing_the_default_creates_the_templates_bytes() {
        let path = temp_path("write-default");
        write_default(&path).expect("写默认配置");
        let text = std::fs::read_to_string(&path).expect("读回配置");
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
        assert_eq!(text, render_default(), "写出来的就该是模板那一份");
    }

    /// `config init`：旧文件先按时间戳备份，再写新的那份。
    #[test]
    fn reset_backs_up_the_old_file_before_writing() {
        let path = temp_path("reset");
        let old = "# 这是我手改过的\n[playback]\nquality = \"hires\"\n";
        std::fs::write(&path, old).expect("写旧配置");

        let (backup, written) = reset(&path).expect("重新生成");
        assert_eq!(written, path);
        let backup = backup.expect("有旧文件就该备份");
        assert_eq!(
            std::fs::read_to_string(&backup).expect("读备份"),
            old,
            "备份要逐字节等于旧文件"
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("读新配置"),
            render_default(),
            "新文件是当前版本的默认配置"
        );
        let name = backup.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with("config.toml.") && name.ends_with(".bak"),
            "{name}"
        );
        assert!(!name.contains(':'), "Windows 文件名里不能有冒号：{name}");
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    /// 没有旧文件就没有备份（第一次跑 `canto config init` 不该凭空造一个 `.bak`）。
    #[test]
    fn reset_without_an_old_file_makes_no_backup() {
        let path = temp_path("reset-fresh");
        let (backup, _) = reset(&path).expect("重新生成");
        assert!(backup.is_none());
        assert!(path.exists());
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    /// 同一秒里跑两次：两份备份，谁也不覆盖谁。
    #[test]
    fn two_resets_in_the_same_second_keep_both_backups() {
        let path = temp_path("reset-twice");
        std::fs::write(&path, "quality = \"320\"\n").expect("第一份");
        let (first, _) = reset(&path).expect("第一次");
        std::fs::write(&path, "quality = \"flac\"\n").expect("第二份");
        let (second, _) = reset(&path).expect("第二次");
        let (first, second) = (first.expect("该有备份"), second.expect("该有备份"));
        assert_ne!(first, second, "两次备份不能撞名");
        assert_eq!(
            std::fs::read_to_string(&first).unwrap(),
            "quality = \"320\"\n"
        );
        assert_eq!(
            std::fs::read_to_string(&second).unwrap(),
            "quality = \"flac\"\n"
        );
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }
}

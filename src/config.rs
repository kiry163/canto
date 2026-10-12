//! 配置文件：`<配置目录>/canto/config.toml`。
//!
//! 自用工具，配置面刻意窄：音质下限、下载目录、历史上限。界面上不提供设置项
//! ——改配置就是开这个文件改，因此**解析失败绝不能变成打不开程序**。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::hotkeys;
use crate::model::Quality;
use crate::paths;

/// 播放缓存默认上限（MiB）。1 GiB 大约能放 2000 首 128k、400 首 320k。
pub const DEFAULT_CACHE_MB: u64 = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// 音质下限（播放与下载共用）。
    pub quality: Quality,
    /// 歌曲下载目录（`d` 键）。
    pub download_dir: PathBuf,
    /// 歌词下载目录（跟歌曲分开放）。
    pub lyrics_dir: PathBuf,
    /// 播放缓存上限（MiB）；0 = 关掉缓存。
    pub cache_mb: u64,
    pub history_limit: usize,
    /// 状态行右侧通知的存活秒数；0 = 不自动消失（只被下一条顶掉）。
    pub notice_seconds: u64,
    /// 系统级全局快捷键（`[hotkeys]` 段）；空串 = 那个动作不注册。
    pub hotkeys: HotkeyConfig,
    /// 桌面歌词（`[desktop_lyrics]` 段；只 Windows 生效）。
    pub desktop_lyrics: DesktopLyricsConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            // 默认 128k MP3：体积最小，够日常听；要更好就在 config.toml 里改。
            quality: Quality::Low128,
            download_dir: default_download_dir(),
            lyrics_dir: default_lyrics_dir(),
            cache_mb: DEFAULT_CACHE_MB,
            history_limit: 500,
            notice_seconds: 4,
            hotkeys: HotkeyConfig::default(),
            desktop_lyrics: DesktopLyricsConfig::default(),
        }
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
/// 空串 = 这个动作不注册；缺字段用默认组合。写法不对的（键名不认 / 没带修饰键）会在
/// `Raw::resolve` 里被清成空串并给一条警告，所以这里只留一条不变式：
/// **非空 = 合法 = 会被注册**。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
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
    /// 桌面歌词的锁定/解锁。
    pub lock: String,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        let mut config = Self {
            play_pause: String::new(),
            next: String::new(),
            prev: String::new(),
            volume_up: String::new(),
            volume_down: String::new(),
            lock: String::new(),
        };
        // 默认组合只在 `hotkeys::DEFAULTS` 里写一遍：配置模板也读它，改一处不会说岔。
        for (field, keys, _) in hotkeys::DEFAULTS {
            config.set(field, keys);
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
            "lock" => &self.lock,
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
            "lock" => self.lock = keys.to_string(),
            _ => {}
        }
    }

    /// 要注册的绑定（按 `DEFAULTS` 的顺序）；空串（= 禁用）在这里跳过。
    pub fn bindings(&self) -> Vec<hotkeys::Binding> {
        hotkeys::DEFAULTS
            .iter()
            .filter(|(field, _, _)| !self.get(field).is_empty())
            .map(|(field, _, action)| hotkeys::Binding {
                action: *action,
                keys: self.get(field).to_string(),
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
        if digits.len() != 6 || !digits.chars().all(|character| character.is_ascii_hexdigit()) {
            return None;
        }
        Some(Self {
            r: u8::from_str_radix(&digits[0..2], 16).ok()?,
            g: u8::from_str_radix(&digits[2..4], 16).ok()?,
            b: u8::from_str_radix(&digits[4..6], 16).ok()?,
        })
    }
}

/// `[desktop_lyrics]` 段：桌面歌词（Windows 覆盖层）。
///
/// 九个字段全是**偏好**；窗口位置是状态，写在 `desktop-lyrics.json` 里，不在这儿。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopLyricsConfig {
    /// 关掉 = 不建线程、不建窗口（零成本）。
    pub enabled: bool,
    /// 字体面；系统里没有这个字体就让 GDI 自己回退。
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
            enabled: true,
            font: "Microsoft YaHei UI".to_string(),
            font_size: 28,
            color: Rgb { r: 255, g: 255, b: 255 },
            outline: Rgb { r: 0, g: 0, b: 0 },
            next_color: Rgb { r: 158, g: 158, b: 158 },
            opacity: 90,
            show_translation: true,
            hide_when_paused: false,
        }
    }
}

/// 解析一个颜色字段：写坏了给一条点名警告并**保持默认值**。
///
/// 跟 `[hotkeys]` 的「清成空串」不同——这里没有「禁用」的语义，退回默认比留个空值清楚。
fn take_color(field: &str, text: Option<String>, slot: &mut Rgb, warnings: &mut Vec<String>) {
    let Some(text) = text else {
        return;
    };
    match Rgb::parse(&text) {
        Some(rgb) => *slot = rgb,
        None => warnings.push(format!("[desktop_lyrics] {field} 不是 #RRGGBB：{text:?}（用默认值）")),
    }
}

/// `[desktop_lyrics]` 的原始形状：全 `Option`，三个颜色先当字符串收（人会写错），
/// 两个数值用 `i64` 收——写成 `-5` 或 `900` 时得由我们**夹住**，不能让 serde 的
/// `u8`/`u32` 范围检查失败：那会把整份配置文件一起丢掉（用户只是多写了个数）。
#[derive(Debug, Default, Deserialize)]
struct RawDesktopLyrics {
    enabled: Option<bool>,
    font: Option<String>,
    font_size: Option<i64>,
    color: Option<String>,
    outline: Option<String>,
    next_color: Option<String>,
    opacity: Option<i64>,
    show_translation: Option<bool>,
    hide_when_paused: Option<bool>,
}

/// 文件里能写的字段；全 `Option`——缺字段就用默认值，不要求写全。
#[derive(Debug, Deserialize)]
struct Raw {
    quality: Option<String>,
    download_dir: Option<String>,
    lyrics_dir: Option<String>,
    /// 缓存上限（MiB）；0 = 关掉缓存。
    cache_mb: Option<u64>,
    history_limit: Option<usize>,
    notice_seconds: Option<u64>,
    /// 全局快捷键；缺这一整段就用默认组合。
    hotkeys: Option<HotkeyConfig>,
    /// 桌面歌词；缺这一整段就用默认值。
    desktop_lyrics: Option<RawDesktopLyrics>,
}

impl Raw {
    fn resolve(self, warnings: &mut Vec<String>) -> Config {
        let mut config = Config::default();
        if let Some(text) = self.quality {
            match Quality::parse(&text) {
                Some(quality) => config.quality = quality,
                None => warnings.push(format!(
                    "quality = {text:?} 不认识（可用：128 / 320 / flac / hires），用默认值 {}",
                    config.quality.token()
                )),
            }
        }
        if let Some(text) = self.download_dir {
            config.download_dir = expand_home(&text);
        }
        if let Some(text) = self.lyrics_dir {
            config.lyrics_dir = expand_home(&text);
        }
        if let Some(mb) = self.cache_mb {
            config.cache_mb = mb;
        }
        if let Some(limit) = self.history_limit {
            config.history_limit = limit.max(1);
        }
        if let Some(seconds) = self.notice_seconds {
            config.notice_seconds = seconds;
        }
        // 全局快捷键：写法不对（键名不认 / 没带修饰键）就清成空串 + 一条点名到字段的警告。
        // 清掉是为了让后面只认一条不变式：非空 = 合法 = 会被注册。
        let mut hotkeys = self.hotkeys.unwrap_or_default();
        for (field, _, _) in hotkeys::DEFAULTS {
            // 取值先 `to_string()`：紧接着要 `set` 它，不能再借着它。
            let keys = hotkeys.get(field).to_string();
            if keys.is_empty() {
                continue;
            }
            if let Err(why) = hotkeys::validate(&keys) {
                warnings.push(format!("[hotkeys] {field} 不可用:{why}"));
                hotkeys.set(field, "");
            }
        }
        config.hotkeys = hotkeys;
        // 桌面歌词：全 `Option` 的原始形状折成配置。颜色写坏了退回默认值 + 一条点名警告。
        let raw = self.desktop_lyrics.unwrap_or_default();
        let mut lyrics = DesktopLyricsConfig::default();
        if let Some(enabled) = raw.enabled {
            lyrics.enabled = enabled;
        }
        if let Some(font) = raw.font.filter(|font| !font.trim().is_empty()) {
            lyrics.font = font;
        }
        if let Some(size) = raw.font_size {
            // 小到画不出来没意义；上限不管（谁想写 400 就让他写），但**不能**让超大值
            // 溢出成小数：`i64 → u32` 用 clamp 收口。
            if size < 8 {
                warnings.push(format!("[desktop_lyrics] font_size = {size} 太小，用 8"));
            }
            lyrics.font_size = size.clamp(8, u32::MAX as i64) as u32;
        }
        take_color("color", raw.color, &mut lyrics.color, warnings);
        take_color("outline", raw.outline, &mut lyrics.outline, warnings);
        take_color("next_color", raw.next_color, &mut lyrics.next_color, warnings);
        if let Some(opacity) = raw.opacity {
            if !(0..=100).contains(&opacity) {
                warnings.push(format!("[desktop_lyrics] opacity = {opacity} 超出 0-100，已夹住"));
            }
            lyrics.opacity = opacity.clamp(0, 100) as u8;
        }
        if let Some(show) = raw.show_translation {
            lyrics.show_translation = show;
        }
        if let Some(hide) = raw.hide_when_paused {
            lyrics.hide_when_paused = hide;
        }
        config.desktop_lyrics = lyrics;
        config
    }
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
/// 细节（路径、解析器原文）走日志。
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
            return (Config::default(), Some("配置读不到，改用默认值".to_string()));
        }
    };
    let mut warnings = Vec::new();
    let raw = match toml::from_str::<Raw>(&text) {
        Ok(raw) => raw,
        Err(error) => {
            tracing::warn!("{} 解析失败：{error}", path.display());
            return (Config::default(), Some("配置坏了，改用默认值".to_string()));
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
    // 四档逐行写清楚：名字、体积量级，「默认」标记跟着默认值走（别硬编码在某一档上，
    // 否则改了默认值就会说错）。
    let options: String = Quality::ALL
        .iter()
        .map(|quality| {
            let token = quality.token();
            let mark = if *quality == default.quality { "   ← 默认" } else { "" };
            format!(
                "#   {token:<6} {:<10} {}{mark}\n",
                quality.label(),
                quality.size_hint()
            )
        })
        .collect();
    // 路径按平台来：Windows 上是 `C:\Users\...`，必须写成字面串（见 `toml_literal`）。
    let download_dir = toml_literal(&default.download_dir.to_string_lossy());
    let lyrics_dir = toml_literal(&default.lyrics_dir.to_string_lossy());
    // 全局快捷键：默认组合从 `DEFAULTS` 现取现写，不跟注释说岔。
    let hotkey_lines: String = hotkeys::DEFAULTS
        .iter()
        .map(|(field, keys, _)| format!("{field} = \"{keys}\"\n"))
        .collect();
    let text = format!(
        "# canto 配置。改完重启 canto 生效。\n\
         #\n\
         # 音质下限：从这一档往下找，这一档没有就退到更低的一档（播放与下载都用它）\n\
         {options}\
         quality = \"{}\"\n\
         #\n\
         # 下载目录（`d` 键落盘的歌曲），支持 `~`\n\
         download_dir = {}\n\
         # 歌词目录（下载时另存的 .lrc，跟歌曲分开放），支持 `~`\n\
         lyrics_dir = {}\n\
         # 播放缓存上限（MB，0 = 关掉）；缓存在 {}，按「最久没听的先删」维护\n\
         cache_mb = {}\n\
         # 播放历史最多保留多少条\n\
         history_limit = {}\n\
         # 状态行右侧那条通知存活多少秒（0 = 不自动消失，只被下一条顶掉）\n\
         notice_seconds = {}\n\
         #\n\
         # 全局快捷键：canto 在后台（没聚焦）时也能用这几个键控制播放。\n\
         # 注意：注册期间这些组合**别的程序收不到**（系统级注册的固有代价）。\n\
         # 写法：修饰键在前、最后一个是要按的键；修饰键可写 ctrl / alt / shift / win\n\
         # （win = super = cmd），键名如 space / up / down / left / f5 / a；\n\
         # 别用媒体键名（macOS 上那要走系统授权）。空字符串 = 这个动作不注册。\n\
         [hotkeys]\n\
         {hotkey_lines}\
         #\n\
         # 桌面歌词（Windows / macOS）：播放时在桌面之上浮一条歌词，默认点不着；\n\
         # 全局热键 lock 解锁后能拖动（位置记在 desktop-lyrics.json）。\n\
         # enabled = false 就完全不起这个窗口；颜色写 #RRGGBB，opacity 是 0-100。\n\
         # font 是字体面：默认那串是 Windows 的中文字体，macOS 上认不出来会自动回退系统字体\n\
         # （想指定就写 macOS 上有的名字，如 \"PingFang SC\"）。\n\
         [desktop_lyrics]\n\
         enabled = true\n\
         font = \"Microsoft YaHei UI\"\n\
         font_size = 28\n\
         color = \"#FFFFFF\"\n\
         outline = \"#000000\"\n\
         next_color = \"#9E9E9E\"\n\
         opacity = 90\n\
         show_translation = true\n\
         hide_when_paused = false\n",
        default.quality.token(),
        download_dir,
        lyrics_dir,
        paths::cache_dir().display(),
        default.cache_mb,
        default.history_limit,
        default.notice_seconds,
    );
    std::fs::write(path, text).with_context(|| format!("写 {} 失败", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::keys;
    use unicode_width::UnicodeWidthStr;

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
    fn the_missing_hotkey_section_gives_the_default_combos() {
        let (config, warning) = load_from("hotkeys-missing", "quality = \"flac\"\n");
        assert_eq!(config.hotkeys, HotkeyConfig::default());
        assert_eq!(config.hotkeys.play_pause, "ctrl+shift+space");
        assert_eq!(config.hotkeys.volume_down, "ctrl+shift+down");
        assert_eq!(config.hotkeys.bindings().len(), 6);
        assert!(warning.is_none(), "缺段不是错误：{warning:?}");
    }

    /// 写了 `[hotkeys]` 但只写一项：其余五项还得是默认值。
    #[test]
    fn a_partial_section_keeps_the_other_defaults() {
        let (config, warning) = load_from("hotkeys-partial", "[hotkeys]\nnext = \"ctrl+alt+n\"\n");
        assert_eq!(config.hotkeys.next, "ctrl+alt+n");
        assert_eq!(config.hotkeys.prev, "ctrl+shift+left", "没写的字段用默认组合");
        assert_eq!(config.hotkeys.bindings().len(), 6);
        assert!(warning.is_none(), "{warning:?}");
    }

    #[test]
    fn an_empty_string_disables_one_action() {
        let (config, warning) = load_from("hotkeys-empty", "[hotkeys]\nplay_pause = \"\"\n");
        assert_eq!(config.hotkeys.play_pause, "");
        let bindings = config.hotkeys.bindings();
        assert_eq!(bindings.len(), 5);
        assert!(!bindings.iter().any(|binding| binding.action == keys::Action::PlayPause));
        assert!(warning.is_none(), "明确禁用不该警告：{warning:?}");
    }

    /// 裸键会被系统级抢走：不注册、清成空串、给一条点名到字段的警告。
    #[test]
    fn a_bare_key_is_refused_and_cleared() {
        let (config, warning) = load_from("hotkeys-bare", "[hotkeys]\nplay_pause = \"space\"\n");
        assert_eq!(config.hotkeys.play_pause, "", "非法值要清掉，别留个不会注册的字符串");
        assert_eq!(config.hotkeys.bindings().len(), 5, "其余五条照旧");
        let warning = warning.expect("应给出警告");
        assert!(warning.contains("[hotkeys] play_pause"), "{warning}");
        assert!(warning.contains("space"), "{warning}");
    }

    #[test]
    fn a_bad_key_name_is_refused_and_cleared() {
        let (config, warning) = load_from("hotkeys-bad", "[hotkeys]\nvolume_up = \"ctrl+shift+nope\"\n");
        assert_eq!(config.hotkeys.volume_up, "");
        assert_eq!(config.hotkeys.next, "ctrl+shift+right", "坏字段不影响别的字段");
        let warning = warning.expect("应给出警告");
        assert!(warning.contains("[hotkeys] volume_up"), "{warning}");
    }

    #[test]
    fn a_combo_may_be_written_in_any_case() {
        let (config, warning) = load_from("hotkeys-case", "[hotkeys]\nplay_pause = \"Ctrl+Shift+Space\"\n");
        assert_eq!(config.hotkeys.play_pause, "Ctrl+Shift+Space", "合法就原样留着");
        assert!(warning.is_none(), "{warning:?}");
        assert_eq!(config.hotkeys.bindings()[0].keys, "Ctrl+Shift+Space");
    }

    /// 模板里六组默认组合都要写出来（值跟 `DEFAULTS` 一致），并提醒会吞键。
    #[test]
    fn the_generated_template_documents_the_hotkeys() {
        let dir = std::env::temp_dir().join(format!("canto-config-hotkeys-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let path = dir.join("config.toml");
        write_default(&path).expect("写默认配置");
        let text = std::fs::read_to_string(&path).expect("读回配置");
        std::fs::remove_dir_all(&dir).ok();

        assert!(text.contains("[hotkeys]"), "{text}");
        for (field, keys, _) in crate::hotkeys::DEFAULTS {
            assert!(text.contains(&format!("{field} = \"{keys}\"")), "模板里缺 {field}：{text}");
        }
        assert!(text.contains("别的程序收不到"), "得说清注册期间组合是被独占的：{text}");
    }

    #[test]
    fn the_missing_desktop_lyrics_section_gives_the_documented_defaults() {
        let (config, warning) = load_from("lyrics-missing", "quality = \"flac\"\n");
        assert_eq!(config.desktop_lyrics, DesktopLyricsConfig::default());
        assert!(config.desktop_lyrics.enabled, "默认就开（跟 [hotkeys] 一个口径）");
        assert_eq!(config.desktop_lyrics.font, "Microsoft YaHei UI");
        assert_eq!(config.desktop_lyrics.font_size, 28);
        assert_eq!(config.desktop_lyrics.opacity, 90);
        assert!(!config.desktop_lyrics.hide_when_paused);
        assert!(warning.is_none(), "缺段不是错误：{warning:?}");
    }

    #[test]
    fn colors_parse_in_both_cases_and_warn_on_garbage() {
        assert_eq!(Rgb::parse("#FFFFFF"), Some(Rgb { r: 255, g: 255, b: 255 }));
        assert_eq!(Rgb::parse("ff0000"), Some(Rgb { r: 255, g: 0, b: 0 }));
        assert_eq!(Rgb::parse("#00ff7F"), Some(Rgb { r: 0, g: 255, b: 127 }));
        assert_eq!(Rgb::parse("red"), None);
        assert_eq!(Rgb::parse("#12345"), None);
        assert_eq!(Rgb::parse(""), None);

        // 写坏了只影响那一个字段：用默认值 + 点名字段的一条警告。
        let (config, warning) = load_from("lyrics-color", "[desktop_lyrics]\ncolor = \"red\"\n");
        assert_eq!(config.desktop_lyrics.color, DesktopLyricsConfig::default().color);
        let warning = warning.expect("应给出警告");
        assert!(warning.contains("[desktop_lyrics] color"), "{warning}");
        assert!(warning.contains("red"), "{warning}");
    }

    #[test]
    fn opacity_is_clamped_and_warned() {
        let (config, warning) = load_from("lyrics-opacity", "[desktop_lyrics]\nopacity = 150\n");
        assert_eq!(config.desktop_lyrics.opacity, 100, "夹到上限");
        let warning = warning.expect("应给出警告");
        assert!(warning.contains("[desktop_lyrics] opacity"), "{warning}");

        let (config, warning) = load_from("lyrics-opacity-ok", "[desktop_lyrics]\nopacity = 40\n");
        assert_eq!(config.desktop_lyrics.opacity, 40);
        assert!(warning.is_none(), "{warning:?}");
    }

    /// `opacity` 写成 900（有人以为它是 0-255）或负数，不该把**整份配置**带走：
    /// 夹回 0-100 + 一条点名警告，别的字段（尤其 `[hotkeys]`）一字不动。
    #[test]
    fn an_out_of_range_opacity_does_not_discard_the_whole_config() {
        let (config, warning) = load_from(
            "lyrics-opacity-huge",
            "quality = \"320\"\n[hotkeys]\nnext = \"ctrl+alt+n\"\n[desktop_lyrics]\nopacity = 900\n",
        );
        assert_eq!(config.desktop_lyrics.opacity, 100, "夹到上限");
        assert_eq!(config.hotkeys.next, "ctrl+alt+n", "别的字段不该被带走");
        let warning = warning.expect("应给出警告");
        assert!(warning.contains("[desktop_lyrics] opacity"), "{warning}");

        let (config, warning) =
            load_from("lyrics-opacity-negative", "[desktop_lyrics]\nopacity = -1\n");
        assert_eq!(config.desktop_lyrics.opacity, 0, "下限是 0");
        let warning = warning.expect("应给出警告");
        assert!(warning.contains("[desktop_lyrics] opacity"), "{warning}");
    }

    /// 负的 `font_size` 同理：夹到下限，别把整份配置扔掉。
    #[test]
    fn an_out_of_range_font_size_does_not_discard_the_whole_config() {
        let (config, warning) = load_from(
            "lyrics-size-negative",
            "[hotkeys]\nprev = \"ctrl+alt+p\"\n[desktop_lyrics]\nfont_size = -5\n",
        );
        assert_eq!(config.desktop_lyrics.font_size, 8, "下限是 8");
        assert_eq!(config.hotkeys.prev, "ctrl+alt+p", "别的字段不该被带走");
        let warning = warning.expect("应给出警告");
        assert!(warning.contains("[desktop_lyrics] font_size"), "{warning}");
    }

    /// 只写一项：其余字段还是默认值。
    #[test]
    fn a_partial_desktop_lyrics_section_keeps_the_other_defaults() {
        let (config, warning) =
            load_from("lyrics-partial", "[desktop_lyrics]\nfont_size = 36\nhide_when_paused = true\n");
        assert_eq!(config.desktop_lyrics.font_size, 36);
        assert!(config.desktop_lyrics.hide_when_paused);
        assert_eq!(config.desktop_lyrics.font, "Microsoft YaHei UI", "没写的还是默认");
        assert_eq!(config.desktop_lyrics.opacity, 90);
        assert!(warning.is_none(), "{warning:?}");
    }

    /// 模板里九个键都写出来（值跟默认值一致），否则「改配置」这件事没有说明书。
    #[test]
    fn the_generated_template_documents_desktop_lyrics() {
        let dir = std::env::temp_dir().join(format!("canto-config-lyrics-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let path = dir.join("config.toml");
        write_default(&path).expect("写默认配置");
        let text = std::fs::read_to_string(&path).expect("读回配置");
        std::fs::remove_dir_all(&dir).ok();

        assert!(text.contains("[desktop_lyrics]"), "{text}");
        let defaults = DesktopLyricsConfig::default();
        for key in [
            format!("enabled = {}", defaults.enabled),
            format!("font = \"{}\"", defaults.font),
            format!("font_size = {}", defaults.font_size),
            format!("opacity = {}", defaults.opacity),
            format!("show_translation = {}", defaults.show_translation),
            format!("hide_when_paused = {}", defaults.hide_when_paused),
        ] {
            assert!(text.contains(&key), "模板里缺 {key}：{text}");
        }
        assert!(text.contains("color = \"#FFFFFF\""), "{text}");
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        // 只写一项，其余全用默认值；新加的键（歌词目录 / 缓存上限）也走这条路，
        // 所以老配置文件不用改就能继续用。
        let (config, warning) = load_from("missing", "quality = \"flac\"\n");
        assert_eq!(config.quality, Quality::Flac);
        assert_eq!(config.download_dir, default_download_dir());
        assert_eq!(config.lyrics_dir, default_lyrics_dir());
        assert_eq!(config.cache_mb, DEFAULT_CACHE_MB);
        assert_eq!(config.history_limit, 500);
        assert_eq!(config.notice_seconds, 4);
        assert!(warning.is_none(), "缺字段不是错误：{warning:?}");
    }

    /// 状态行通知的存活时间：默认 4 秒，`0` = 不自动消失。
    #[test]
    fn notice_seconds_defaults_to_four_and_zero_means_never() {
        assert_eq!(Config::default().notice_seconds, 4, "默认 4 秒");

        let (config, warning) = load_from("notice-zero", "notice_seconds = 0\n");
        assert_eq!(config.notice_seconds, 0, "0 要原样保留（= 不过期）");
        assert!(warning.is_none(), "0 是合法取值，不该警告：{warning:?}");

        let (config, _) = load_from("notice-30", "notice_seconds = 30\n");
        assert_eq!(config.notice_seconds, 30);
    }

    #[test]
    fn unknown_quality_warns_and_keeps_default() {
        let (config, warning) = load_from("unknown", "quality = \"cd\"\n");
        assert_eq!(config.quality, Config::default().quality, "不认识就用默认档位");
        let warning = warning.expect("应给出警告");
        assert!(warning.contains("cd"), "{warning}");
        // 提示里要给出可用的写法，别让人回来翻代码。
        assert!(warning.contains("128") && warning.contains("320"), "{warning}");
    }

    /// 默认档位是 128，且模板里四档都写清楚了。
    #[test]
    fn the_generated_template_documents_every_quality_option() {
        assert_eq!(Config::default().quality.token(), "128", "默认档位");

        let dir = std::env::temp_dir().join(format!("canto-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let path = dir.join("config.toml");
        write_default(&path).expect("写默认配置");
        let text = std::fs::read_to_string(&path).expect("读回配置");

        assert!(
            text.contains(&format!("quality = \"{}\"", Config::default().quality.token())),
            "模板里写的是默认档位：{text}"
        );
        // 四档都要在注释里逐行写清楚（规范名 + 显示名 + 体积量级）。
        for quality in Quality::ALL {
            assert!(
                text.contains(quality.token()),
                "模板里没提 {} 一档：{text}",
                quality.token()
            );
            assert!(
                text.contains(quality.label()) && text.contains(quality.size_hint()),
                "模板里没写 {} 的说明：{text}",
                quality.token()
            );
        }
        // 「默认」标记只能落在默认那一档上。
        let marked = text
            .lines()
            .find(|line| line.contains("默认"))
            .expect("模板该标出默认档位");
        assert!(
            marked.contains(&format!("quality \"{}\"", Quality::Low128.token()))
                || marked.contains(Quality::Low128.token()),
            "默认档位是 128，标记却在别处：{marked}"
        );
        assert!(text.contains("lyrics_dir"), "歌词目录要出现在模板里：{text}");
        assert!(text.contains("cache_mb"), "缓存上限要出现在模板里：{text}");
        assert!(
            text.contains("notice_seconds"),
            "通知存活秒数要出现在模板里：{text}"
        );
        // 模板本身必须是能解析回去的合法配置。
        let (config, warning) = load_file(&path);
        assert_eq!(config, Config::default(), "{warning:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn broken_toml_falls_back_to_defaults_with_a_warning() {
        let (config, warning) = load_from("broken", "quality = \n");
        assert_eq!(config, Config::default());
        assert!(warning.expect("应给出警告").contains("默认值"));
    }

    /// 模板里的路径要写成 TOML **字面串**：`C:\Users\...` 里那个 `\U` 是非法转义，
    /// 用基本串（双引号）写会让整个配置文件被判成坏文件——Windows 上实测每次启动都弹警告。
    #[test]
    fn windows_paths_survive_the_template_round_trip() {
        let text = format!(
            "download_dir = {}\nlyrics_dir = {}\n",
            toml_literal(r"C:\Users\me\Music\canto"),
            toml_literal(r"C:\Users\me\Music\canto-lyrics"),
        );
        let (config, warning) = load_from("win-path", &text);
        assert!(warning.is_none(), "路径不该让配置解析失败：{warning:?}");
        assert_eq!(
            config.download_dir,
            PathBuf::from(r"C:\Users\me\Music\canto")
        );
        assert_eq!(
            config.lyrics_dir,
            PathBuf::from(r"C:\Users\me\Music\canto-lyrics")
        );
    }

    /// 给状态行的那句警告必须短——通知位固定 32 列（见 `tui/draw/status.rs`），
    /// 长版本（带路径与解析器原文）会被截成没用的半句话；详情进日志。
    #[test]
    fn a_config_warning_fits_the_status_bar() {
        let (_, warning) = load_from("broken-short", "quality = \n");
        let warning = warning.expect("应给出警告");
        assert!(
            !warning.contains("config.toml") && !warning.contains("quality"),
            "警告里不该带路径或解析器原文：{warning}"
        );
        assert!(
            UnicodeWidthStr::width(warning.as_str()) <= 32,
            "警告要能塞进 32 列：{warning}"
        );
    }

    #[test]
    fn tilde_in_download_dir_expands_to_home() {
        let home = dirs::home_dir().expect("有家目录");
        assert_eq!(expand_home("~/Music/canto"), home.join("Music/canto"));
        assert_eq!(expand_home("/tmp/x"), PathBuf::from("/tmp/x"));
    }
}

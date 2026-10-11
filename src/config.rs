//! 配置文件：`<配置目录>/canto/config.toml`。
//!
//! 自用工具，配置面刻意窄：音质下限、下载目录、历史上限。界面上不提供设置项
//! ——改配置就是开这个文件改，因此**解析失败绝不能变成打不开程序**。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

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

/// 文件里能写的字段；全 `Option`——缺字段就用默认值，不要求写全。
#[derive(Debug, Deserialize)]
struct Raw {
    quality: Option<String>,
    download_dir: Option<String>,
    lyrics_dir: Option<String>,
    /// 缓存上限（MiB）；0 = 关掉缓存。
    cache_mb: Option<u64>,
    history_limit: Option<usize>,
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
        config
    }
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
    let text = format!(
        "# canto 配置。改完重启 canto 生效。\n\
         #\n\
         # 音质下限：从这一档往下找，这一档没有就退到更低的一档（播放与下载都用它）\n\
         {options}\
         quality = \"{}\"\n\
         #\n\
         # 下载目录（`d` 键落盘的歌曲），支持 `~`\n\
         download_dir = \"{}\"\n\
         # 歌词目录（下载时另存的 .lrc，跟歌曲分开放），支持 `~`\n\
         lyrics_dir = \"{}\"\n\
         # 播放缓存上限（MB，0 = 关掉）；缓存在 {}，按「最久没听的先删」维护\n\
         cache_mb = {}\n\
         # 播放历史最多保留多少条\n\
         history_limit = {}\n",
        default.quality.token(),
        default.download_dir.display(),
        default.lyrics_dir.display(),
        paths::cache_dir().display(),
        default.cache_mb,
        default.history_limit,
    );
    std::fs::write(path, text).with_context(|| format!("写 {} 失败", path.display()))
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
        // 只写一项，其余全用默认值；新加的键（歌词目录 / 缓存上限）也走这条路，
        // 所以老配置文件不用改就能继续用。
        let (config, warning) = load_from("missing", "quality = \"flac\"\n");
        assert_eq!(config.quality, Quality::Flac);
        assert_eq!(config.download_dir, default_download_dir());
        assert_eq!(config.lyrics_dir, default_lyrics_dir());
        assert_eq!(config.cache_mb, DEFAULT_CACHE_MB);
        assert_eq!(config.history_limit, 500);
        assert!(warning.is_none(), "缺字段不是错误：{warning:?}");
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
        // 模板本身必须是能解析回去的合法配置。
        let (config, warning) = load_file(&path);
        assert_eq!(config, Config::default(), "{warning:?}");
        std::fs::remove_dir_all(&dir).ok();
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

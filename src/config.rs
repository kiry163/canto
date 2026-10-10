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
                    "quality = {text:?} 不认识（可用：hires / flac / 320 / 128），用默认值 {}",
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

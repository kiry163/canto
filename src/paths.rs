//! 所有落盘路径的唯一来源。
//!
//! 以前 `config.rs` 管会话路径，`main.rs` 自己拼 `login-qr.png` 与 `canto.log`，
//! 同一份目录规则散在三处——改一处忘一处就会出现「日志目录建了、会话目录没建」这类错。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// 应用数据目录：`~/.config/canto`。
///
/// 刻意**不用** `dirs::config_dir()`：它在 macOS 上给的是 `~/Library/Application Support`。
/// 这里按 XDG 习惯统一放 `~/.config`——三平台同一套路径，备份 / 同步 / 手动翻文件都省事。
pub fn data_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("canto")
}

/// 老位置（macOS 的 `~/Library/Application Support/canto`）。只为一次性搬迁留着。
fn legacy_data_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("canto"))
}

/// 把整个数据目录从老位置搬到新位置；`Some(老路径)` 表示确实搬了。
///
/// 只在「新目录还不存在 + 老目录在」时动手，所以搬过一次之后就是普通启动，不会来回折腾。
/// 搬的是整个目录（`rename`）：会话、配置、历史、日志一起过去——因此**必须在
/// `init_tracing` 之前调用**（它会清空日志文件，先初始化就搬不动日志了）。
pub fn migrate_legacy() -> Result<Option<PathBuf>> {
    let Some(legacy) = legacy_data_dir() else {
        return Ok(None);
    };
    let new = data_dir();
    if legacy == new {
        return Ok(None);
    }
    Ok(migrate_dir(&legacy, &new)?.then_some(legacy))
}

/// 搬迁本身：`from` 存在且 `to` 不存在时整目录 `rename`。返回是否搬了。
fn migrate_dir(from: &Path, to: &Path) -> Result<bool> {
    if !from.is_dir() || to.exists() {
        return Ok(false);
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建配置目录失败：{}", parent.display()))?;
    }
    std::fs::rename(from, to)
        .with_context(|| format!("把 {} 搬到 {} 失败", from.display(), to.display()))?;
    Ok(true)
}

/// 会话文件（cookie jar 的 JSON 快照）。
pub fn session_path() -> PathBuf {
    data_dir().join("session.json")
}

/// 配置文件。
pub fn config_path() -> PathBuf {
    data_dir().join("config.toml")
}

/// 播放历史。
pub fn history_path() -> PathBuf {
    data_dir().join("history.json")
}

/// 日志文件。
pub fn log_path() -> PathBuf {
    data_dir().join("canto.log")
}

/// 播放缓存目录：`~/.cache/canto`。
///
/// 刻意跟数据目录分开：缓存随时可以丢（可能好几个 G），数据目录要小而能备份/同步。
pub fn cache_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cache")
        .join("canto")
}

/// 单例锁文件（`File::try_lock` 的目标；内容只是持有者的 pid）。
pub fn lock_path() -> PathBuf {
    data_dir().join("canto.lock")
}

/// 二维码落盘的一份（`o` 用它调系统查看器；作废了就覆盖）。
pub fn qr_png_path() -> PathBuf {
    data_dir().join("login-qr.png")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 数据目录就是 `~/.config/canto`（不跟平台默认走）。
    #[test]
    fn the_data_directory_lives_under_dot_config() {
        let dir = data_dir();
        assert_eq!(dir.file_name(), Some(std::ffi::OsStr::new("canto")));
        assert_eq!(
            dir.parent().and_then(|parent| parent.file_name()),
            Some(std::ffi::OsStr::new(".config")),
            "{}",
            dir.display()
        );
        assert!(dir.starts_with(dirs::home_dir().expect("有 home")));
    }

    /// 缓存目录在 `~/.cache/canto`，不跟数据目录混在一起。
    #[test]
    fn the_cache_directory_lives_under_dot_cache() {
        let dir = cache_dir();
        assert_eq!(dir.file_name(), Some(std::ffi::OsStr::new("canto")));
        assert_eq!(
            dir.parent().and_then(|parent| parent.file_name()),
            Some(std::ffi::OsStr::new(".cache")),
            "{}",
            dir.display()
        );
        assert_ne!(dir, data_dir(), "缓存与数据目录要分开");
    }

    /// 老目录整目录搬过来；新目录已存在、或老目录不在时都不动手。
    #[test]
    fn the_legacy_directory_is_moved_once() {
        let root = std::env::temp_dir().join(format!("canto-migrate-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let old = root.join("Application Support").join("canto");
        let new = root.join(".config").join("canto");
        std::fs::create_dir_all(&old).expect("建老目录");
        std::fs::write(old.join("session.json"), "{}").expect("写个文件");

        assert!(migrate_dir(&old, &new).expect("该搬成功"));
        assert!(!old.exists(), "老目录搬走了");
        assert!(new.join("session.json").exists(), "文件跟着过去");
        assert!(
            !migrate_dir(&old, &new).expect("再搬一次不该报错"),
            "老目录已经不在了，不再动手"
        );

        // 新目录已存在（用户已经在新位置跑过）→ 不覆盖、不合并。
        std::fs::create_dir_all(&old).ok();
        std::fs::write(old.join("config.toml"), "quality = \"hires\"").ok();
        assert!(!migrate_dir(&old, &new).expect("不该报错"), "新目录在，就不动");
        assert!(!new.join("config.toml").exists(), "不合并老目录");
        std::fs::remove_dir_all(&root).ok();
    }

    /// 这些路径都会写进同一个目录（各自只建一次父目录），名字撞车会互相覆盖。
    #[test]
    fn all_paths_share_one_directory_and_are_distinct() {
        let dir = data_dir();
        let paths = [
            session_path(),
            config_path(),
            history_path(),
            log_path(),
            lock_path(),
            qr_png_path(),
        ];
        for path in &paths {
            assert_eq!(
                path.parent(),
                Some(dir.as_path()),
                "{} 不在数据目录里",
                path.display()
            );
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

//! 所有落盘路径的唯一来源。
//!
//! 以前 `config.rs` 管会话路径，`main.rs` 自己拼 `login-qr.png` 与 `canto.log`，
//! 同一份目录规则散在三处——改一处忘一处就会出现「日志目录建了、会话目录没建」这类错。

use std::path::PathBuf;

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

/// 桌面歌词窗口的位置（中心点）。这是**状态**不是配置，所以不写进 `config.toml`。
pub fn desktop_lyrics_path() -> PathBuf {
    data_dir().join("desktop-lyrics.json")
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

/// 本地曲库的索引：`~/.cache/canto/library.json`。
///
/// 放缓存目录（不是数据目录）：它是**派生数据**，删了只会重扫一次；[`data_dir`] 只放
/// 使用者自己建的东西（配置、会话、历史、本地歌单）。
pub fn library_index_path() -> PathBuf {
    cache_dir().join("library.json")
}

/// 本地歌单：`~/.config/canto/local_playlists.json`。
///
/// 跟索引相反，这是使用者建的**数据**（丢了就没了），所以跟历史放一起。
// Task 9 的本地歌单存储接上就摘掉。
#[allow(dead_code)]
pub fn local_playlists_path() -> PathBuf {
    data_dir().join("local_playlists.json")
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

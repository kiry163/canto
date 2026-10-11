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

/// 单例锁文件（`flock` 的目标；内容只是持有者的 pid）。
pub fn lock_path() -> PathBuf {
    data_dir().join("canto.lock")
}

/// 终端装不下二维码时落盘的 PNG。
pub fn qr_png_path() -> PathBuf {
    data_dir().join("login-qr.png")
}

#[cfg(test)]
mod tests {
    use super::*;

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

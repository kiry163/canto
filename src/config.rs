//! 路径与配置。
//!
//! 自用工具，配置只有「会话文件放哪」和「日志级别」，其余都写死在代码里。

use std::path::PathBuf;

/// 应用数据目录：`<系统配置目录>/canto`。
///
/// macOS 上是 `~/Library/Application Support/canto`，Linux 上是 `~/.config/canto`
/// —— 跟随 `dirs::config_dir()`，不自己拼路径。
pub fn data_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("canto")
}

/// 会话文件（cookie jar 的 JSON 快照）。
pub fn session_path() -> PathBuf {
    data_dir().join("session.json")
}

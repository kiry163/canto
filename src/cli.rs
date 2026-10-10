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

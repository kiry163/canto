//! 命令行入口。
//!
//! 界面上能做的事一律进 TUI；这里只留配置文件的 `config` 子命令、排查用的 `--log`，
//! 以及 clap 自带的 `--version` / `--help`。
//!
//! `config` 下的动作都是「干完就退」：不进界面、不加单例锁、不写日志——这样
//! `canto config path` 在一台数据目录还没建起来的机器上也能回答，而 `canto config init`
//! 的全部副作用只有「备份 + 写这一个文件」。

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "canto", version, about = "canto —— 自用的 QQ 音乐播放器")]
pub struct Cli {
    /// 日志级别：error / warn / info / debug / trace（也可写 `reqwest=debug` 这样的过滤串）
    #[arg(short, long, default_value = "warn")]
    pub log: String,

    /// 不进界面，只做子命令说的那件事
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// 子命令。只放「不进界面」的事。
#[derive(Debug, Subcommand)]
pub enum Command {
    /// 配置文件：重新生成一份，或者只是说它在哪
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

/// `canto config …` 的动作。
#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// 重新生成一份带注释的默认配置（旧文件先备份成 config.toml.<时间戳>.bak）
    Init,
    /// 打印配置文件路径
    Path,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_flag_has_a_default_and_takes_a_filter() {
        let cli = Cli::try_parse_from(["canto", "--log", "debug"]).expect("--log 该被接受");
        assert_eq!(cli.log, "debug");
        assert!(cli.command.is_none(), "不认识 --log 之外的参数");
        assert_eq!(
            Cli::try_parse_from(["canto"]).expect("裸跑该被接受").log,
            "warn"
        );
    }

    /// `config` 下面的两个动作，以及「打进用法里的东西都能解析出来」。
    #[test]
    fn the_config_subcommand_knows_two_actions() {
        let cli = Cli::try_parse_from(["canto", "config", "init"]).expect("config init 该被接受");
        assert!(matches!(
            cli.command,
            Some(Command::Config {
                action: ConfigAction::Init
            })
        ));

        let cli = Cli::try_parse_from(["canto", "config", "path"]).expect("config path 该被接受");
        assert!(matches!(
            cli.command,
            Some(Command::Config {
                action: ConfigAction::Path
            })
        ));

        // 裸 `canto config` 不给动作：宁可让人看用法，也不替他把文件重写了。
        assert!(Cli::try_parse_from(["canto", "config"]).is_err());
        assert!(
            Cli::try_parse_from(["canto", "config", "reset"]).is_err(),
            "没有这个动作"
        );
        assert!(
            Cli::try_parse_from(["canto", "playlists"]).is_err(),
            "别的子命令不存在"
        );
    }
}

//! 播放：候选地址 → 解码 → 输出设备。
//!
//! 逐个候选**真去开**：只有能解码的才算数（服务端对不存在的文件也给 purl/vkey，
//! 见 [`crate::qq::vkey`]）。开出来就播到底，并且把"实际播了多久"和"应该多久"
//! 报出来——流中途断了只会在音频线程里静默结束，这是唯一能发现它的地方。

use std::time::Duration;

use anyhow::{Context, Result, bail};
use rodio::source::Source;
use rodio::stream::DeviceSinkBuilder;
use rodio::{Decoder, Player};

use crate::media::MediaStream;
use crate::model::Quality;
use crate::qq::vkey::Candidate;

/// 实际播完（或流中断）之后的交代。
pub struct Played {
    pub quality: Quality,
    /// 实际推进的播放位置。
    pub position: Duration,
    /// 容器声明的总时长（拿不到就是 `None`）。
    pub expected: Option<Duration>,
}

impl Played {
    /// 是否完整播完。差得超过 5 秒就算中断。
    pub fn completed(&self) -> bool {
        match self.expected {
            Some(expected) => self.position + Duration::from_secs(5) >= expected,
            None => true,
        }
    }
}

/// 播第一个能打开的候选，阻塞到放完。
pub fn play(candidates: &[Candidate]) -> Result<Played> {
    let mut sink = DeviceSinkBuilder::open_default_sink()
        .context("打开默认音频设备失败（有别的程序独占音频设备？）")?;
    sink.log_on_drop(false);

    let mut failures = Vec::new();
    for candidate in candidates {
        let stream = match MediaStream::open(&candidate.url) {
            Ok(stream) => stream,
            Err(error) => {
                tracing::debug!(
                    "{} 档打不开（{}），换下一个候选",
                    candidate.quality.label(),
                    error
                );
                failures.push(format!("{:?} 打不开：{error}", candidate.quality));
                continue;
            }
        };
        let decoder = match Decoder::new(stream) {
            Ok(decoder) => decoder,
            Err(error) => {
                tracing::debug!(
                    "{} 档解不了码（{error}），换下一个候选",
                    candidate.quality.label()
                );
                failures.push(format!("{:?} 解不了码：{error}", candidate.quality));
                continue;
            }
        };
        let expected = decoder.total_duration();
        let channels = decoder.channels();
        let sample_rate = decoder.sample_rate();
        // 只要求 `Source + Send`：`Player::append` 会在音频线程上按输出设备
        // 的声道/采样率归一（rodio 的 mixer 负责转换）。
        let player = Player::connect_new(sink.mixer());
        player.append(decoder);
        tracing::info!(
            "播放中：{:?} · {sample_rate}Hz · {channels} 声道",
            candidate.quality
        );
        player.sleep_until_end();
        let played = Played {
            quality: candidate.quality,
            position: player.get_pos(),
            expected,
        };
        if !played.completed() {
            tracing::warn!(
                "流提前结束：只播了 {:.1}s（应为 {}）——网络或 CDN 断了",
                played.position.as_secs_f64(),
                played
                    .expected
                    .map(|expected| format!("{:.1}s", expected.as_secs_f64()))
                    .unwrap_or_else(|| "未知".to_string()),
            );
        }
        return Ok(played);
    }
    bail!("没有能播的档位：{}", failures.join("；"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn played(position_secs: u64, expected: Option<u64>) -> Played {
        Played {
            quality: Quality::Flac,
            position: Duration::from_secs(position_secs),
            expected: expected.map(Duration::from_secs),
        }
    }

    #[test]
    fn short_streams_are_reported_as_interrupted() {
        // 播到底：位置与声明时长一致。
        assert!(played(236, Some(236)).completed());
        // 播放器收尾时会差个零点几秒，5 秒容差要能容忍。
        assert!(played(235, Some(236)).completed());
        // 中途断了：音频线程只会静默结束，靠这个判定才能发现。
        assert!(!played(30, Some(236)).completed());
        // 声明时长拿不到（部分 MP3）就没法判定，不误报。
        assert!(played(30, None).completed());
    }
}

//! 播放地址（vkey）获取。
//!
//! 接口是 `vkey.GetVkeyServer / CgiGetVkey`：一次请求可以带上同一首歌的多个
//! `filename`（`前缀 + media_mid + 扩展名`），服务端逐个回 `purl` + `vkey`。
//!
//! **purl 非空不代表能播**：实测（2026-10-09）对不存在的文件服务端照样返回
//! purl 与 vkey，真正去取时才 404（`{"errorcode":-46628,"errormsg":"file not
//! exist"}`）。所以这里的策略是"把候选按偏好排好队，交给播放器一个个真去开"，
//! 而不是在这儿判断。
//!
//! 候选顺序按**实测规格**定（见 README 的档位表）：
//! - 立体声优先：`Q001` 是 5.1（6 声道）、`O801` 是 4 声道，只在其它档都没有时才兜底；
//! - `F000`/`Q000` 都是 44.1k/16bit 立体声无损（个别歌的 `F000` 是 48k/24bit，更好），
//!   所以 `F000` 排在 `Q000` 前面；
//! - `AI00` 是 192k/24bit 立体声母带（~150MB/首），只在显式要 hires 时用；
//! - `M800`/`M500` 是立体声 320k/128k。

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::model::{Quality, Song};

/// 候选：要请求的 `filename` 前缀、扩展名，以及它代表的档位。
struct Entry {
    prefix: &'static str,
    ext: &'static str,
    quality: Quality,
}

/// 一个可尝试的播放地址。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub quality: Quality,
    /// 容器后缀（`flac`/`mp3`/`ogg`）。下载时要用它定文件名，播放用不到。
    pub ext: &'static str,
    /// `sip + purl`。purl 自带 `guid`/`vkey`/`uin`/`fromtag`，不要另拼参数。
    pub url: String,
}

const fn entry(prefix: &'static str, ext: &'static str, quality: Quality) -> Entry {
    Entry {
        prefix,
        ext,
        quality,
    }
}

/// 按用户要的档位排出候选链（要哪一档，就从那一档往下找）。
fn chain(quality: Option<Quality>) -> Vec<Entry> {
    let mut entries = Vec::new();
    let want = quality.unwrap_or(Quality::Flac);
    if want == Quality::Hires {
        entries.push(entry("AI00", "flac", Quality::Hires));
    }
    if want >= Quality::Flac {
        entries.push(entry("F000", "flac", Quality::Flac));
        entries.push(entry("Q000", "flac", Quality::Flac));
    }
    if want >= Quality::High320 {
        entries.push(entry("M800", "mp3", Quality::High320));
    }
    entries.push(entry("M500", "mp3", Quality::Low128));
    if want >= Quality::Flac {
        // 环绕声版本垫底：能播但会经过 rodio 的降混，立体声档位永远优先。
        entries.push(entry("Q001", "flac", Quality::Flac));
        entries.push(entry("O801", "ogg", Quality::High320));
    }
    entries
}

/// 取这首歌的候选播放地址，按偏好顺序。
pub async fn candidates(song: &Song, quality: Option<Quality>) -> Result<Vec<Candidate>> {
    let media_mid = song.media_mid.as_deref().unwrap_or(&song.mid);
    let entries = chain(quality);
    let filenames: Vec<String> = entries
        .iter()
        .map(|entry| format!("{}{media_mid}.{}", entry.prefix, entry.ext))
        .collect();
    let body = json!({
        "req_0": {
            "module": "vkey.GetVkeyServer",
            "method": "CgiGetVkey",
            "param": {
                "guid": std::process::id().to_string(),
                // 服务端按 filename 逐个回，songmid 只是配对用的上下文。
                "songmid": vec![song.mid.clone(); filenames.len()],
                "songtype": vec![0; filenames.len()],
                "uin": "0",
                "loginflag": 1,
                "platform": "20",
                "filename": filenames,
            }
        },
        "comm": { "uin": 0, "format": "json", "ct": 24, "cv": 0 }
    });
    let json = super::post_musicu(&body).await.context("取播放地址失败")?;
    let data = &json["req_0"]["data"];
    let sip = data["sip"]
        .as_array()
        .and_then(|sip| sip.first())
        .and_then(Value::as_str)
        .filter(|sip| !sip.is_empty())
        .context("响应里没有 CDN 地址（sip），登录可能已失效")?;
    let infos = data["midurlinfo"]
        .as_array()
        .context("响应里没有 midurlinfo，登录可能已失效")?;

    let mut candidates = Vec::new();
    for (entry, filename) in entries.iter().zip(&filenames) {
        let Some(info) = infos
            .iter()
            .find(|info| info["filename"].as_str() == Some(filename.as_str()))
        else {
            continue;
        };
        let purl = info["purl"].as_str().unwrap_or("").trim();
        if purl.is_empty() {
            // 这一档这条歌没有（或账号取不到），换下一个候选。
            tracing::debug!("{} 没有播放地址（{filename}）", song.title);
            continue;
        }
        candidates.push(Candidate {
            quality: entry.quality,
            ext: entry.ext,
            url: format!("{sip}{purl}"),
        });
    }
    if candidates.is_empty() {
        bail!(
            "{} 没有可用的播放地址：账号无版权，或登录已失效",
            song.title
        );
    }
    Ok(candidates)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefixes(quality: Option<Quality>) -> Vec<&'static str> {
        chain(quality).iter().map(|entry| entry.prefix).collect()
    }

    #[test]
    fn stereo_quality_comes_before_surround() {
        let default = prefixes(None);
        assert_eq!(default, ["F000", "Q000", "M800", "M500", "Q001", "O801"]);
        // 5.1 的 Q001 与 4 声道的 O801 垫底（真没别的了才会用到）。
        let q001 = default.iter().position(|p| *p == "Q001").unwrap();
        let m500 = default.iter().position(|p| *p == "M500").unwrap();
        assert!(q001 > m500);
    }

    #[test]
    fn hires_adds_the_master_tape_first() {
        assert_eq!(
            prefixes(Some(Quality::Hires)),
            ["AI00", "F000", "Q000", "M800", "M500", "Q001", "O801"]
        );
        // 默认不用 AI00：150MB/首 的 192k/24bit，听感收益对普通回放设备不成比例。
        assert!(!prefixes(None).contains(&"AI00"));
    }

    #[test]
    fn lower_quality_requests_do_not_reach_up() {
        assert_eq!(prefixes(Some(Quality::High320)), ["M800", "M500"]);
        assert_eq!(prefixes(Some(Quality::Low128)), ["M500"]);
    }
}

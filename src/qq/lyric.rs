//! 歌词获取。
//!
//! 走 `musicu.fcg` 的 `PlayLyricInfo / GetPlayLyricInfo`：**`crypt=0` + `qrc=0`**
//! 时返回明文 base64 的 LRC——正文在 `lyric`，翻译在 `trans`（实测日韩歌都有翻译）。
//!
//! 千万别传 `qrc=1`：那会把返回换成 QQ 自己的加密 QRC（hex 密文），要多一整套解密，
//! 而行级歌词不需要它。voicefox 就是传了 `qrc=1` 又按 base64 解，
//! 拿到的是一堆二进制垃圾。
//!
//! 「没有歌词」（如 `code=24001`）不是错误：返回 `Ok(None)`，界面上显示「暂无歌词」。

use anyhow::{Context, Result};
use base64::Engine;
use serde_json::{Value, json};

use crate::model::{Lyric, Song};
use crate::cache;

/// 取这首歌的歌词；`Ok(None)` 表示这首歌确实没有歌词。
pub async fn get(song: &Song, cache_on: bool) -> Result<Option<Lyric>> {
    if cache_on
        && let Some(cached) = cache::lyric(&song.mid)
    {
        tracing::debug!("命中歌词缓存：{}", song.mid);
        return Ok(cached);
    }
    let lyric = fetch(song).await?;
    if cache_on {
        // 连「这首没有歌词」也记一笔，免得下次再为空结果发一次请求。
        cache::put_lyric(&song.mid, lyric.as_ref());
    }
    Ok(lyric)
}

/// 问接口要歌词（不碰缓存）。
async fn fetch(song: &Song) -> Result<Option<Lyric>> {
    let body = json!({
        "comm": { "ct": 24, "cv": 0, "uin": 0, "format": "json" },
        "req": {
            "module": "music.musichallSong.PlayLyricInfo",
            "method": "GetPlayLyricInfo",
            "param": {
                "songMID": song.mid,
                "songID": song.song_id.unwrap_or(0),
                "format": "json",
                "crypt": 0,
                "qrc": 0,
                "trans": 1,
            }
        }
    });
    let json = super::post_musicu(&body).await.context("取歌词失败")?;
    let req = &json["req"];
    if req["code"].as_i64().unwrap_or(-1) != 0 {
        tracing::debug!("《{}》没有歌词（code={}）", song.title, req["code"]);
        return Ok(None);
    }
    let data = &req["data"];
    let Some(text) = decode_field(&data["lyric"]) else {
        return Ok(None);
    };
    let translation = decode_field(&data["trans"]);
    let lyric = Lyric::parse(&text, translation.as_deref());
    Ok((!lyric.lines.is_empty()).then_some(lyric))
}

/// 接口把歌词按 base64 放在字符串字段里；空串、非字符串、解不出 UTF-8 都算没有。
fn decode_field(value: &Value) -> Option<String> {
    let encoded = value.as_str()?;
    if encoded.is_empty() {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn decodes_base64_lyric_fields() {
        // "[00:01.00]你好" 的 base64。
        let encoded = json!("WzAwOjAxLjAwXeS9oOWlvQ==");
        assert_eq!(decode_field(&encoded).as_deref(), Some("[00:01.00]你好"));
        // 空串 / 数字 / 非 base64 都当没有，而不是当成错误炸出去。
        assert_eq!(decode_field(&json!("")), None);
        assert_eq!(decode_field(&json!(1)), None);
        assert_eq!(decode_field(&json!("not base64!!")), None);
        assert_eq!(decode_field(&json!(&[0xff_u8][..])), None);
    }

    #[test]
    fn decodes_a_real_lyric_and_translation_pair() {
        // 实测抓取（2026-10-09，YOASOBI《アイドル》，各取前 8 行）。
        const LYRIC: &str = "W3RpOuOCouOCpOODieODqyAo5YG25YOPKV0KW2FyOllPQVNPQkkgKOODqOOCouOCveODkyldClthbDrjgqLjgqTjg4njg6tdCltieTpdCltvZmZzZXQ6MF0KWzAwOjAwLjAwXeOCouOCpOODieODqyAtIFlPQVNPQkkgKOODqOOCouOCveODkykKWzAwOjAwLjQyXeivje+8mkF5YXNlClswMDowMC41NF3mm7LvvJpBeWFzZQo=";
        const TRANS: &str = "W3RpOuOCouOCpOODieODqyAo5YG25YOPKV0KW2FyOllPQVNPQkkgKOODqOOCouOCveODkyldClthbDrjgqLjgqTjg4njg6tdCltieTpdCltvZmZzZXQ6MF0KWzAwOjAwLjAwXVFR6Z+z5LmQ5Lqr5pyJ5pys57+76K+R5L2c5ZOB55qE6JGX5L2c5p2DClswMDowMC40Ml0vLwpbMDA6MDAuNTRdLy8K";
        let lyric = decode_field(&json!(LYRIC)).expect("正文要能解出来");
        let translation = decode_field(&json!(TRANS)).expect("翻译要能解出来");
        let parsed = crate::model::Lyric::parse(&lyric, Some(&translation));

        // 元信息行被丢掉，只留三行正文。
        assert_eq!(parsed.lines.len(), 3, "实际：{:?}", parsed.lines);
        assert_eq!(parsed.lines[0].text, "アイドル - YOASOBI (ヨアソビ)");
        assert_eq!(parsed.lines[1].at, Duration::from_millis(420));
        assert_eq!(parsed.lines[1].text, "词：Ayase");
        // 翻译按时间戳贴上来（QQ 的第一条"翻译"是版权说明，这是真实数据）。
        assert_eq!(
            parsed.lines[0].translation.as_deref(),
            Some("QQ音乐享有本翻译作品的著作权")
        );
        assert_eq!(parsed.lines[2].translation.as_deref(), Some("//"));
    }
}

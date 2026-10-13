//! 歌单写入：往**个人歌单**加歌 / 删歌。
//!
//! 走 `musicu.fcg` 的 `music.musicasset.PlaylistDetailWrite`（`AddSonglist` /
//! `DelSonglist`）。**不要**用老的 `fcg_music_add2songdir.fcg`：实测它对任何会话
//! 都回 `403 no permit`（粘贴 cookie 与扫码会话都一样），而这条新链路一次就通。
//!
//! 参数里 `songId` 必须是**数字 id**（不是 mid）。实测服务端 code：`80092` 歌单
//! 不存在/无权、`40000` 方法不存在。
//!
//! 「我喜欢」（`dirId=201`）只读：界面里没有写它的入口——服务端对它的
//! `DelSonglist` 恒回 `80105`，而 `AddSonglist` 就算回了 0 也看不到（写完之后
//! 那屏从不重拉）。只剩读接口用 [`FAVORITES_DIR_ID`] 拼 `dirid`。

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::model::Song;

/// 「我喜欢」对应的目录号（QQ 把它当成一个特殊歌单）。只有读接口用它拼 `dirid`。
pub const FAVORITES_DIR_ID: u64 = 201;

/// 往歌单加歌；返回写进去的条数。
pub async fn add(dir_id: u64, tid: u64, songs: &[Song]) -> Result<usize> {
    write("AddSonglist", dir_id, tid, songs).await
}

/// 从歌单删歌；返回删掉的条数。
pub async fn remove(dir_id: u64, tid: u64, songs: &[Song]) -> Result<usize> {
    write("DelSonglist", dir_id, tid, songs).await
}

async fn write(method: &str, dir_id: u64, tid: u64, songs: &[Song]) -> Result<usize> {
    if songs.is_empty() {
        return Ok(0);
    }
    let body = request_body(method, dir_id, tid, songs)?;
    let json = super::post_musicu(&body)
        .await
        .context("歌单写入请求失败")?;
    if json["code"].as_i64() != Some(0) {
        bail!("歌单写入失败（外层 code={}）", json["code"]);
    }
    match json["req"]["code"].as_i64() {
        Some(0) => Ok(songs.len()),
        Some(code) => bail!("歌单写入失败：{}", explain(code)),
        None => bail!("歌单写入响应异常（没有 req.code）"),
    }
}

/// 请求体独立成纯函数，方便单测钉住字段名（服务端对字段名很敏感）。
fn request_body(method: &str, dir_id: u64, tid: u64, songs: &[Song]) -> Result<Value> {
    let mut infos = Vec::with_capacity(songs.len());
    for song in songs {
        let Some(song_id) = song.song_id else {
            bail!("《{}》缺少数字 id，写不进歌单（只拿到 mid）", song.title);
        };
        infos.push(json!({ "songId": song_id, "songType": 0 }));
    }
    Ok(json!({
        "comm": { "ct": 24, "cv": 0, "uin": 0, "format": "json" },
        "req": {
            "module": "music.musicasset.PlaylistDetailWrite",
            "method": method,
            "param": { "dirId": dir_id, "tid": tid, "bFmtUtf8": true, "v_songInfo": infos }
        }
    }))
}

/// 服务端 code 翻成人话（只翻实测见过的）。
fn explain(code: i64) -> String {
    match code {
        80092 => format!("歌单不存在或当前账号无权（{code}）"),
        40000 => format!("接口不存在，可能需要更新实现（{code}）"),
        other => format!("服务端返回 {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FileSizes;

    fn song(song_id: Option<u64>) -> Song {
        Song {
            mid: "000CbUE00I998M".to_string(),
            song_id,
            media_mid: None,
            title: "天黑黑".to_string(),
            singers: "孙燕姿".to_string(),
            album_mid: String::new(),
            album_name: String::new(),
            duration_secs: 236,
            sizes: FileSizes::default(),
            local_bytes: 0,
            source: crate::model::Source::Qq,
        }
    }

    #[test]
    fn request_shape_is_the_one_the_server_accepts() {
        let body = request_body("AddSonglist", 201, 0, &[song(Some(5211222))]).unwrap();
        assert_eq!(
            body["req"]["module"].as_str(),
            Some("music.musicasset.PlaylistDetailWrite")
        );
        assert_eq!(body["req"]["method"].as_str(), Some("AddSonglist"));
        let param = &body["req"]["param"];
        assert_eq!(param["dirId"].as_u64(), Some(201));
        assert_eq!(param["tid"].as_u64(), Some(0));
        assert_eq!(param["bFmtUtf8"].as_bool(), Some(true));
        // 数字 id + songType 0，字段名一个字母都不能错。
        assert_eq!(param["v_songInfo"][0]["songId"].as_u64(), Some(5211222));
        assert_eq!(param["v_songInfo"][0]["songType"].as_u64(), Some(0));
    }

    #[test]
    fn songs_without_a_numeric_id_are_refused_early() {
        let error = request_body("AddSonglist", 201, 0, &[song(None)]).unwrap_err();
        assert!(error.to_string().contains("数字 id"), "实际：{error}");
    }

    #[test]
    fn server_codes_are_explained() {
        assert!(explain(80092).contains("歌单不存在"));
        assert!(explain(40000).contains("接口不存在"));
        assert!(explain(-1).contains("-1"));
    }
}

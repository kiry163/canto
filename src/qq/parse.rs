//! 把 QQ 音乐接口响应解析成 [`crate::model`] 的模型。
//!
//! 这里的形状都来自实测响应（voicefox 的 tx 音源跑通过同一套接口），
//! 不是照文档猜的：字段缺失、数字被写成字符串都出现过，所以取值一律走
//! 宽容的 helper。

use serde_json::Value;

use crate::model::{FileSizes, Playlist, Song};

/// 数字字段：可能是 number，也可能是数字字符串。
fn value_u64(value: &Value) -> u64 {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        .unwrap_or(0)
}

/// 字符串字段：缺失或空串都算没有。
fn value_str(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// 解析一首歌（歌单详情 / 「我喜欢」里的 `songlist` 条目）。
///
/// 没有 `mid` 的条目直接丢弃：mid 是后续取 vkey、取歌词的主键，留着也无法播放。
pub fn parse_song(item: &Value) -> Option<Song> {
    let mid = value_str(&item["mid"])?;
    let singers = match &item["singer"] {
        Value::Array(singers) => singers
            .iter()
            .filter_map(|singer| value_str(&singer["name"]))
            .collect::<Vec<_>>()
            .join("、"),
        _ => String::new(),
    };
    let file = &item["file"];
    Some(Song {
        mid,
        song_id: Some(value_u64(&item["id"])).filter(|id| *id > 0),
        media_mid: value_str(&file["media_mid"]),
        title: value_str(&item["title"]).unwrap_or_default(),
        singers,
        album_mid: value_str(&item["album"]["mid"]).unwrap_or_default(),
        album_name: value_str(&item["album"]["name"]).unwrap_or_default(),
        duration_secs: value_u64(&item["interval"]),
        sizes: FileSizes {
            mp3_128: value_u64(&file["size_128mp3"]),
            mp3_320: value_u64(&file["size_320mp3"]),
            flac: value_u64(&file["size_flac"]),
            hires: value_u64(&file["size_hires"]),
        },
    })
}

/// 解析搜索结果条目。
///
/// 搜索接口的字段名与歌单详情那套完全不同（`songname`/`songmid`/`size320`…
/// 而且没有嵌套的 `file`），所以单独一份解析。音质只报 `size128/320/flac`：
/// 这个接口不给 hires 大小，所以标签可能低估——取流不依赖它（候选链是固定的）。
pub fn parse_search_song(item: &Value) -> Option<Song> {
    let mid = value_str(&item["songmid"])?;
    let singers = match &item["singer"] {
        Value::Array(singers) => singers
            .iter()
            .filter_map(|singer| value_str(&singer["name"]))
            .collect::<Vec<_>>()
            .join("、"),
        _ => String::new(),
    };
    Some(Song {
        mid,
        song_id: Some(value_u64(&item["songid"])).filter(|id| *id > 0),
        media_mid: value_str(&item["media_mid"]).or_else(|| value_str(&item["strMediaMid"])),
        title: value_str(&item["songname"]).unwrap_or_default(),
        singers,
        album_mid: value_str(&item["albummid"]).unwrap_or_default(),
        album_name: value_str(&item["albumname"]).unwrap_or_default(),
        duration_secs: value_u64(&item["interval"]),
        sizes: FileSizes {
            mp3_128: value_u64(&item["size128"]),
            mp3_320: value_u64(&item["size320"]),
            flac: value_u64(&item["sizeflac"]),
            hires: 0,
        },
    })
}

/// 系统目录的 `dirid`：201 = 我喜欢（曲目走单独的接口）。
const DIRID_FAVORITES: u64 = 201;

/// 解析个人歌单列表里的条目（`fcg_user_created_diss` 的 `data.disslist`）。
///
/// 字段名与 [`parse_song`] 那套完全不同，别互相套：`dirid` 是目录号、`tid` 才是
/// 详情接口要的 `disstid`、名字是 `diss_name`、封面是 `diss_cover`、数量是 `song_cnt`。
///
/// 两种条目不列出来：
/// - `dir_show == 0` 的服务端系统目录（`QZone 背景音乐`、`本地上传`）；
/// - 没有 `tid` 的条目——没有 id 就查不到曲目，列出来只会让人白点一次。
pub fn parse_user_playlist(item: &Value) -> Option<Playlist> {
    let dirid = value_u64(&item["dirid"]);
    if dirid == DIRID_FAVORITES {
        return Some(Playlist {
            id: Playlist::FAVORITES_ID.to_string(),
            dir_id: dirid,
            name: value_str(&item["diss_name"]).unwrap_or_else(|| "我喜欢".to_string()),
            song_count: value_u64(&item["song_cnt"]),
            cover_url: None,
            is_favorites: true,
        });
    }
    if value_u64(&item["dir_show"]) == 0 {
        return None;
    }
    let tid = value_u64(&item["tid"]);
    if tid == 0 {
        return None;
    }
    Some(Playlist {
        id: tid.to_string(),
        dir_id: dirid,
        name: value_str(&item["diss_name"])?,
        song_count: value_u64(&item["song_cnt"]),
        cover_url: value_str(&item["diss_cover"]).map(|url| url.replace("http://", "https://")),
        is_favorites: false,
    })
}

#[cfg(test)]
mod tests {
    use crate::model::Quality;
    use super::*;
    use serde_json::json;

    /// 实测形状：`file` 里逐档给 size，`interval` 是秒。
    fn song_item() -> Value {
        json!({
            "mid": "0039MnYb0qxYhV",
            "id": 1050326,
            "title": "晴天",
            "interval": 269,
            "singer": [{ "name": "周杰伦" }, { "name": "袁咏琳" }],
            "album": { "mid": "000MkMni19ClKG", "name": "叶惠美" },
            "file": {
                "media_mid": "0039MnYb0qxYhV",
                "size_128mp3": 4307816,
                "size_320mp3": 10769465,
                "size_flac": 29532253,
                "size_hires": 0
            }
        })
    }

    #[test]
    fn parses_a_song() {
        let song = parse_song(&song_item()).expect("有效条目要解析出来");
        assert_eq!(song.mid, "0039MnYb0qxYhV");
        assert_eq!(song.song_id, Some(1050326));
        assert_eq!(song.media_mid.as_deref(), Some("0039MnYb0qxYhV"));
        assert_eq!(song.title, "晴天");
        assert_eq!(song.singers, "周杰伦、袁咏琳");
        assert_eq!(song.album_name, "叶惠美");
        assert_eq!(song.duration_secs, 269);
        assert_eq!(song.duration_secs, 4 * 60 + 29, "04:29");
        assert_eq!(song.sizes.flac, 29532253);
        assert_eq!(
            song.sizes.at_or_below(Quality::Hires).0,
            Quality::Flac,
            "没有 hires 就该退到 FLAC"
        );
    }

    #[test]
    fn rejects_songs_without_a_mid() {
        let mut item = song_item();
        item["mid"] = json!("");
        assert!(
            parse_song(&item).is_none(),
            "没有 mid 的歌无法取流，不能进列表"
        );
    }

    #[test]
    fn tolerates_strings_in_number_fields() {
        // 实测里 interval / size 偶尔是字符串。
        let mut item = song_item();
        item["interval"] = json!("210");
        item["file"]["size_hires"] = json!("90000000");
        let song = parse_song(&item).unwrap();
        assert_eq!(song.duration_secs, 210);
        assert_eq!(song.sizes.at_or_below(Quality::Hires).0, Quality::Hires);
    }

    #[test]
    fn parses_a_search_result() {
        // 实测响应（2026-10-09，搜索「晴天 周杰伦」）。
        let item = json!({
            "songname": "晴天",
            "songmid": "0039MnYb0qxYhV",
            "songid": 1050326,
            "singer": [{ "id": 4558, "mid": "0025NhlN2yWrP4", "name": "周杰伦" }],
            "albumname": "叶惠美",
            "albummid": "000MkMni19ClKG",
            "interval": 269,
            "media_mid": "003Qui1q2u1Zho",
            "size128": 4317292,
            "size320": 10792943,
            "sizeflac": 0
        });
        let song = parse_search_song(&item).expect("搜索结果要能解析出来");
        assert_eq!(song.mid, "0039MnYb0qxYhV");
        assert_eq!(song.song_id, Some(1050326));
        assert_eq!(song.media_mid.as_deref(), Some("003Qui1q2u1Zho"));
        assert_eq!(song.title, "晴天");
        assert_eq!(song.singers, "周杰伦");
        assert_eq!(song.album_name, "叶惠美");
        assert_eq!(song.duration_secs, 269);
        assert_eq!(song.sizes.mp3_320, 10792943);
        assert_eq!(song.sizes.at_or_below(Quality::Hires).0, Quality::High320);

        assert!(parse_search_song(&json!({ "songname": "没有 mid" })).is_none());
    }

    #[test]
    fn parses_a_user_playlist() {
        // 实测响应（2026-10-09 抓的，字段名与搜索接口那套完全不同；tid 已换成假值）。
        let item = json!({
            "diss_name": "嗯",
            "diss_cover": "http://y.gtimg.cn/music/photo_new/T002R300x300M000001BMYBD25YV7w.jpg?n=1",
            "song_cnt": 197,
            "listen_num": 2082,
            "dirid": 2,
            "tid": 1000000001u64,
            "dir_show": 2
        });
        let playlist = parse_user_playlist(&item).expect("个人歌单要解析出来");
        assert_eq!(playlist.id, "1000000001", "详情接口要的是 tid，不是 dirid");
        assert_eq!(playlist.dir_id, 2, "写入接口要的是 dirid");
        assert_eq!(playlist.name, "嗯");
        assert_eq!(playlist.song_count, 197);
        assert_eq!(
            playlist.cover_url.as_deref(),
            Some("https://y.gtimg.cn/music/photo_new/T002R300x300M000001BMYBD25YV7w.jpg?n=1"),
            "封面统一升级到 https"
        );
        assert!(!playlist.is_favorites);
    }

    #[test]
    fn maps_favorites_to_its_own_id() {
        let item = json!({
            "diss_name": "我喜欢",
            "diss_cover": "?n=1",
            "song_cnt": 488,
            "dirid": 201,
            "tid": 1000000002u64,
            "dir_show": 0
        });
        let playlist = parse_user_playlist(&item).expect("我喜欢要单独成行");
        assert_eq!(playlist.id, Playlist::FAVORITES_ID);
        assert_eq!(playlist.dir_id, 201, "「我喜欢」的目录号固定 201");
        assert!(playlist.is_favorites);
        assert_eq!(playlist.song_count, 488);
        assert_eq!(playlist.cover_url, None, "占位封面（?n=1）不算封面");
    }

    #[test]
    fn skips_system_dirs_and_id_less_playlists() {
        // QZone 背景音乐 / 本地上传：dir_show=0 的系统目录，不是用户的歌单。
        for item in [
            json!({ "diss_name": "QZone背景音乐", "dirid": 205, "tid": 0, "dir_show": 0, "song_cnt": 3 }),
            json!({ "diss_name": "本地上传", "dirid": 206, "tid": 0, "dir_show": 0, "song_cnt": 0 }),
        ] {
            assert!(
                parse_user_playlist(&item).is_none(),
                "系统目录不该出现：{item}"
            );
        }
        // 没有 tid 就查不到曲目，列出来只会让人白点一次。
        assert!(
            parse_user_playlist(&json!({ "diss_name": "没有 id", "dirid": 9, "dir_show": 1 }))
                .is_none()
        );
    }
}

//! QQ 音乐的数据接口。
//!
//! 只做 M1 需要的读接口：个人歌单、我喜欢、歌单曲目。请求形状照抄 voicefox
//! （同一套接口实测跑过），其中 `musicu.fcg` 的模块名是服务端路由的关键，
//! 改一个字符就 404。

pub mod cookie;
pub mod login;
pub mod lyric;
pub mod parse;
pub mod search;
pub mod vkey;
pub mod write;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::http;
use crate::session;
use crate::model::{Playlist, Song};

/// 音乐接口主入口：绝大多数读接口都走这个 POST，靠 body 里的模块名路由。
const MUSICU: &str = "https://u.y.qq.com/cgi-bin/musicu.fcg";
/// 个人歌单列表（老接口，GET）。
///
/// 注意**没有** `.fcg` 后缀:带后缀的老路径现在 404(voicefox 里那份就是这么写的,
/// 所以它的个人歌单是坏的)。`size` 一次要够:个人歌单不会超过这个数。
const USER_CREATED_DISS: &str = "https://c.y.qq.com/rsc/fcgi-bin/fcg_user_created_diss";
const REFERER: &str = "https://y.qq.com/";

/// 登录态检查，返回账号 ID（`uin` / `wxuin` 的值，个人歌单接口要它）。
fn require_login() -> Result<String> {
    let state = session::jar().state();
    if !state.logged_in {
        bail!("需要登录：按 L");
    }
    state
        .identity
        .filter(|identity| !identity.is_empty())
        .context("会话里缺少账号 ID（uin），请重新登录")
}

/// 个人歌单列表。
pub async fn user_playlists() -> Result<Vec<Playlist>> {
    let uin = require_login()?;
    let url = format!("{USER_CREATED_DISS}?hostuin={uin}&format=json&size=200");
    let json = get_json(&url).await?;
    if json["code"].as_i64().unwrap_or(-1) != 0 {
        bail!("获取个人歌单失败（code={}），登录可能已失效", json["code"]);
    }
    let items = json["data"]["disslist"]
        .as_array()
        .or_else(|| json["data"]["list"].as_array())
        .context("响应里没有歌单列表：登录可能已失效")?;
    Ok(items
        .iter()
        .filter_map(parse::parse_user_playlist)
        .collect())
}

/// 「我喜欢」。QQ 把它当成一个特殊歌单：`dirid=201`。
pub async fn favorites() -> Result<(Playlist, Vec<Song>)> {
    require_login()?;
    let body = json!({
        "comm": { "ct": 20, "cv": 1859, "uin": 0, "format": "json" },
        "music.srfDissInfo.DissInfo": {
            "method": "CgiGetDiss",
            "module": "music.srfDissInfo.DissInfo",
            "param": {
                "disstid": 0,
                "dirid": 201,
                "song_begin": 0,
                "song_num": 1000,
                "enc_host_uin": ""
            }
        }
    });
    let json = post_musicu(&body).await?;
    let data = &json["music.srfDissInfo.DissInfo"];
    if data["code"].as_i64().unwrap_or(-1) != 0 {
        bail!(
            "获取「我喜欢」失败（code={}），登录可能已失效",
            data["code"]
        );
    }
    let data = &data["data"];
    let songs = parse_songlist(data)?;
    let playlist = Playlist {
        id: Playlist::FAVORITES_ID.to_string(),
        dir_id: write::FAVORITES_DIR_ID,
        name: "我喜欢".to_string(),
        song_count: data["total_song_num"]
            .as_u64()
            .unwrap_or(songs.len() as u64),
        cover_url: None,
        is_favorites: true,
    };
    Ok((playlist, songs))
}

/// 歌单曲目（含歌单名/封面）。`id` 是 `playlists` 打出来的数字 id。
pub async fn playlist_tracks(id: &str) -> Result<(Playlist, Vec<Song>)> {
    require_login()?;
    let disstid = id
        .parse::<u64>()
        .with_context(|| format!("歌单 id 必须是数字：{id}"))?;
    let body = json!({
        "comm": { "ct": 20, "cv": 1859, "uin": 0, "format": "json" },
        "req": {
            "module": "music.srfDissInfo.aiDissInfo",
            "method": "uniform_get_Dissinfo",
            "param": {
                "disstid": disstid,
                "enc_host_uin": "",
                "tag": 1,
                "userInfo": 1,
                "song_begin": 0,
                "song_num": 1000,
                "onlysonglist": 0
            }
        }
    });
    let json = post_musicu(&body).await?;
    let req = &json["req"];
    if json["code"].as_i64() != Some(0)
        || req["code"].as_i64() != Some(0)
        || req["data"]["code"].as_i64() != Some(0)
    {
        bail!(
            "获取歌单失败（code={}/{}/{}），id 是否正确、登录是否有效？",
            json["code"],
            req["code"],
            req["data"]["code"]
        );
    }
    let data = &req["data"];
    let songs = parse_songlist(data)?;
    let name = data["dirinfo"]["title"]
        .as_str()
        .or_else(|| data["dissname"].as_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("歌单 {id}"));
    let playlist = Playlist {
        id: id.to_string(),
        // 详情接口只认 tid；写入要的 dirid 得从个人歌单列表里取（`canto playlists`）。
        dir_id: 0,
        name,
        song_count: data["total_song_num"]
            .as_u64()
            .unwrap_or(songs.len() as u64),
        cover_url: data["dirinfo"]["picurl"]
            .as_str()
            .filter(|url| !url.is_empty())
            .map(|url| url.replace("http://", "https://")),
        is_favorites: false,
    };
    Ok((playlist, songs))
}

/// `songlist` 数组 → 歌曲列表；没有这个数组说明响应形状不对（通常是登录失效）。
fn parse_songlist(data: &Value) -> Result<Vec<Song>> {
    let items = data["songlist"]
        .as_array()
        .context("响应里没有 songlist（登录可能已失效）")?;
    Ok(items.iter().filter_map(parse::parse_song).collect())
}

async fn get_json(url: &str) -> Result<Value> {
    let response = http::client()
        .get(url)
        .header("Referer", REFERER)
        .send()
        .await
        .with_context(|| format!("请求失败：{url}"))?;
    read_json(response, url).await
}

async fn post_musicu(body: &Value) -> Result<Value> {
    let response = http::client()
        .post(MUSICU)
        .header("Referer", REFERER)
        .json(body)
        .send()
        .await
        .context("请求 musicu.fcg 失败")?;
    read_json(response, "musicu.fcg").await
}

/// 读 JSON 响应，并在这里**统一**判定「登录已失效」——所有接口都从这里过，
/// 所以「会话过期」这一件事只有一个判定点（界面按 [`crate::session::SessionExpired`]
/// 切回登录屏）。
pub(crate) async fn read_json(response: reqwest::Response, what: &str) -> Result<Value> {
    let status = response.status();
    let text = response
        .text()
        .await
        .with_context(|| format!("读 {what} 响应失败"))?;
    if session::looks_expired(&text, status.as_u16()) {
        tracing::info!("{what}：登录已失效（HTTP {status}）");
        return Err(anyhow::Error::new(session::SessionExpired));
    }
    if !status.is_success() {
        bail!("{what} 返回非 2xx：{status}");
    }
    serde_json::from_str(&text).with_context(|| format!("{what} 响应不是 JSON"))
}

//! 搜索。
//!
//! 走经典接口 `client_search_cp`（不需要签名，实测可用）：
//! `w=关键词&format=json&n=条数&p=页码&cr=1&t=0`。
//!
//! 返回的条目字段与歌单详情那套不同，解析在 [`super::parse::parse_search_song`]。

use anyhow::{Context, Result};
use serde_json::Value;

use crate::model::Song;

/// 单页最多能要多少条（接口上限，再多会被截断）。
const MAX_LIMIT: u32 = 60;

pub async fn songs(keyword: &str, limit: u32) -> Result<Vec<Song>> {
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return Ok(Vec::new());
    }
    let limit = limit.clamp(1, MAX_LIMIT);
    // 关键词必须自己编码：中文、空格、`&` 都在实际输入里出现过。
    let encoded = url::form_urlencoded::byte_serialize(keyword.as_bytes())
        .collect::<String>()
        .replace('+', "%20");
    let url = format!(
        "https://c.y.qq.com/soso/fcgi-bin/client_search_cp?w={encoded}&format=json&n={limit}&p=1&cr=1&t=0"
    );
    let response = crate::http::client()
        .get(&url)
        .header("Referer", "https://y.qq.com/portal/player.html")
        .send()
        .await
        .context("搜索请求失败")?;
    let json: Value = super::read_json(response, "搜索").await?;
    if json["code"].as_i64().unwrap_or(-1) != 0 {
        anyhow::bail!("搜索失败（code={}）", json["code"]);
    }
    let items = json["data"]["song"]["list"]
        .as_array()
        .context("搜索响应里没有结果列表")?;
    Ok(items
        .iter()
        .filter_map(super::parse::parse_search_song)
        .collect())
}

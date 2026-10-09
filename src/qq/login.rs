//! QQ 扫码登录。
//!
//! 链路（顺序与 voicefox 里那套**已被实机调试过**的一致）：
//!
//! 1. `xlogin` 预热，让服务端下发 `pt_login_sig` 等 cookie；
//! 2. `ptqrshow` 取二维码：返回 **PNG**，`Set-Cookie` 里带 `qrsig`；
//! 3. `ptqrlogin` 轮询，`ptqrtoken = hash33(qrsig)`：
//!    `66` 等待扫码 / `67` 已扫码待确认 / `65` 过期 / `0` 成功；
//! 4. 成功后按 `ptuiCB` 给的地址走：
//!    `check_sig`（拿 `p_skey`）→ `graph.qq.com/oauth2.0/authorize`（拿一次性 `code`）
//!    → `musicu.fcg` 的 `QQConnectLogin.LoginServer/QQLogin`（用 `code` 换 `musickey`）。
//!
//! **cookie 全交给会话 jar**：它按域存取、每跳 `Set-Cookie` 自动收下、后续请求
//! 自动带上。voicefox 当初在这条链上栽的两件事——扁平 map 被 `Set-Cookie: p_skey=`
//! 覆盖、自动跟随跳转让中间几跳的 cookie 全丢——canto 从结构上就不会发生。

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde_json::json;

use crate::http;
use crate::session;

const XLOGIN_API: &str = "https://xui.ptlogin2.qq.com/cgi-bin/xlogin";
const QR_SHOW_API: &str = "https://xui.ptlogin2.qq.com/ssl/ptqrshow";
const QR_CHECK_API: &str = "https://xui.ptlogin2.qq.com/ssl/ptqrlogin";
/// 拿 `p_skey` 的那一跳：`ptuiCB` 里的 `ptsigx` 只认它。
const CHECK_SIG_API: &str = "https://ssl.ptlogin2.graph.qq.com/check_sig";
const OAUTH_AUTHORIZE_API: &str = "https://graph.qq.com/oauth2.0/authorize";
const OAUTH_JUMP: &str = "https://graph.qq.com/oauth2.0/login_jump";
/// `authorize` 的回调：QQ 音乐网页版自己带的那个（`login_type=1` 即 QQ 渠道）。
const OAUTH_CALLBACK: &str =
    "https://y.qq.com/portal/wx_redirect.html?login_type=1&surl=https://y.qq.com/";
const OAUTH_CLIENT_ID: &str = "100497308";
const REFERER: &str = "https://xui.ptlogin2.qq.com/";
/// 跳转链最多跟这么多跳，避免异常链接把进程绕死。
const MAX_REDIRECTS: usize = 20;
/// 二维码会话用的"请求地址"，只用来在 jar 里查/存这个域下的 cookie。
const PORTAL_URL: &str = "https://xui.ptlogin2.qq.com/";
const GRAPH_URL: &str = "https://graph.qq.com/";
const MUSIC_URL: &str = "https://y.qq.com/";

/// 轮询一次的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Poll {
    /// 还没人扫。
    Waiting,
    /// 已扫码，等手机上确认。
    Scanned,
    /// 二维码过期，要重新取一张。
    Expired,
    /// 换到会话了（账号 ID）。
    Success(String),
    /// 失败，附服务端说明。
    Failed(String),
}

/// 取一张二维码 PNG（`qrsig` 随 `Set-Cookie` 进会话 jar）。
pub async fn create() -> Result<Vec<u8>> {
    // 预热失败不致命：`ptqrshow` 自己也会下发必需的 cookie。
    let warmup = format!(
        "{XLOGIN_API}?appid=716027609&daid=383&style=33&login_text=%E7%99%BB%E5%BD%95&hide_title_bar=1&hide_border=1&target=self&s_url={OAUTH_JUMP}&pt_3rd_aid={OAUTH_CLIENT_ID}&theme=2"
    );
    if let Err(error) = http::client()
        .get(warmup)
        .header("Referer", REFERER)
        .send()
        .await
    {
        tracing::debug!("xlogin 预热失败（继续）：{error}");
    }

    let url = format!(
        "{QR_SHOW_API}?appid=716027609&e=2&l=M&s=3&d=72&v=4&t={}&daid=383&pt_3rd_aid={OAUTH_CLIENT_ID}&u1={OAUTH_JUMP}",
        now_millis()
    );
    let response = http::client()
        .get(url)
        .header("Referer", REFERER)
        .send()
        .await
        .context("取二维码失败")?;
    if !response.status().is_success() {
        bail!("取二维码失败：HTTP {}", response.status());
    }
    let bytes = response.bytes().await.context("读取二维码响应失败")?;
    if !bytes.starts_with(b"\x89PNG") {
        bail!("二维码响应不是 PNG（{} 字节）", bytes.len());
    }
    Ok(bytes.to_vec())
}

/// 轮询一次；扫码成功后会把会话换成音乐凭证。
pub async fn poll() -> Result<Poll> {
    let Some(qrsig) = session::jar().cookie_for(PORTAL_URL, "qrsig") else {
        bail!("会话里没有 qrsig（二维码会话已失效，重新扫码）");
    };
    let url = format!(
        "{QR_CHECK_API}?u1={OAUTH_JUMP}&ptqrtoken={}&ptredirect=0&h=1&t=1&g=1&from_ui=1&ptlang=2052&action=0-0-{}&js_ver=26071711&js_type=1&login_sig=&pt_uistyle=40&aid=716027609&daid=383&pt_3rd_aid={OAUTH_CLIENT_ID}",
        hash33(&qrsig),
        now_millis()
    );
    let body = http::client()
        .get(url)
        .header("Referer", REFERER)
        .send()
        .await
        .context("轮询登录状态失败")?
        .text()
        .await
        .context("读取登录状态失败")?;
    let Some((code, redirect, message, _)) = parse_ptui_cb(&body) else {
        bail!("登录状态解析失败");
    };
    match code.as_str() {
        "66" => Ok(Poll::Waiting),
        "67" => Ok(Poll::Scanned),
        "65" => Ok(Poll::Expired),
        "0" => complete(&redirect).await,
        other => Ok(Poll::Failed(format!("{message}（{other}）"))),
    }
}

/// 扫码成功后换凭据：`ptuiCB` 地址 → `check_sig` → `authorize` → `QQLogin`。
async fn complete(redirect: &str) -> Result<Poll> {
    let uin = query_value(redirect, "uin");
    let ptsigx = query_value(redirect, "ptsigx");
    tracing::debug!("扫码成功，开始换凭据（有 uin={}）", uin.is_some());

    // 1. 原样跟一遍浏览器会走的地址：链上可能直接给 `code`，也可能下发 `p_skey`。
    let mut code = follow(redirect).await;
    // 2. 链上没给 `p_skey` 时，用 `ptsigx` 显式换一次（它是一次性的，只做兜底）。
    if code.is_none()
        && session::jar().cookie_for(GRAPH_URL, "p_skey").is_none()
        && let (Some(uin), Some(ptsigx)) = (&uin, &ptsigx)
    {
        check_sig(uin, ptsigx).await;
    }
    // 3. 还没 `code` 就用 `p_skey` 走一次 `authorize`。
    if code.is_none() {
        let Some(p_skey) = session::jar().cookie_for(GRAPH_URL, "p_skey") else {
            return Ok(Poll::Failed(
                "登录成功但没有拿到 p_skey，请重新扫码".to_string(),
            ));
        };
        code = authorize(&p_skey).await.ok();
    }
    let Some(code) = code else {
        return Ok(Poll::Failed(
            "登录成功但没有换到 OAuth code，请重新扫码".to_string(),
        ));
    };
    exchange(&code, uin.as_deref()).await
}

/// 显式走一次 `check_sig`（`p_skey` 由它下发）。
async fn check_sig(uin: &str, ptsigx: &str) {
    let url = format!(
        "{CHECK_SIG_API}?uin={uin}&pttype=1&service=ptqrlogin&nodirect=0&ptsigx={ptsigx}&s_url={OAUTH_JUMP}&ptlang=2052&ptredirect=100&aid=716027609&daid=383&j_later=0&low_login_hour=0&regmaster=0&pt_login_type=3&pt_aid=0&pt_aaid=16&pt_light=0&pt_3rd_aid={OAUTH_CLIENT_ID}"
    );
    match http::client()
        .get(url)
        .header("Referer", REFERER)
        .send()
        .await
    {
        Ok(response) => tracing::debug!("check_sig → HTTP {}", response.status()),
        Err(error) => tracing::debug!("check_sig 失败：{error}"),
    }
}

/// 用 `p_skey` 换一次性 `code`：先 POST 表单，不行再按浏览器那样 GET 跳一次。
async fn authorize(p_skey: &str) -> Result<String> {
    let g_tk = hash33_seeded(p_skey, 5381).to_string();
    let auth_time = now_millis().to_string();
    let ui = pseudo_uuid();
    let form = [
        ("response_type", "code"),
        ("client_id", OAUTH_CLIENT_ID),
        ("redirect_uri", OAUTH_CALLBACK),
        ("scope", "get_user_info,get_app_friends"),
        ("state", "state"),
        ("switch", ""),
        ("from_ptlogin", "1"),
        ("src", "1"),
        ("update_auth", "1"),
        ("openapi", "1010_1030"),
        ("g_tk", g_tk.as_str()),
        ("auth_time", auth_time.as_str()),
        ("ui", ui.as_str()),
    ];
    for post in [true, false] {
        let client = http::client_without_redirect();
        let request = if post {
            client.post(OAUTH_AUTHORIZE_API).form(&form)
        } else {
            client.get(OAUTH_AUTHORIZE_API).query(&form)
        };
        let Ok(response) = request.header("Referer", OAUTH_JUMP).send().await else {
            continue;
        };
        let Some(location) = location_of(&response) else {
            continue;
        };
        if let Some(code) = query_value(&location, "code") {
            return Ok(code);
        }
        if let Some(code) = follow(&resolve(OAUTH_AUTHORIZE_API, &location)).await {
            return Ok(code);
        }
    }
    bail!("OAuth 没有返回 code")
}

/// 用一次性 `code` 换音乐凭证并写进会话。
async fn exchange(code: &str, uin: Option<&str>) -> Result<Poll> {
    let body = json!({
        "comm": {
            "tmeAppID": "qqmusic",
            "tmeLoginType": "2",
            "g_tk": 5381,
            "platform": "yqq",
            "ct": 24,
            "cv": 0
        },
        "req": {
            "module": "QQConnectLogin.LoginServer",
            "method": "QQLogin",
            "param": { "code": code }
        }
    });
    let json = super::post_musicu(&body).await.context("换音乐凭证失败")?;
    if json["code"].as_i64() != Some(0) || json["req"]["code"].as_i64() != Some(0) {
        return Ok(Poll::Failed(format!(
            "QQ 登录失败（服务端返回 {}/{}）",
            json["code"], json["req"]["code"]
        )));
    }
    let data = &json["req"]["data"];
    let Some(musickey) = data["musickey"]
        .as_str()
        .map(str::trim)
        .filter(|key| !key.is_empty())
    else {
        return Ok(Poll::Failed(
            "QQ 登录成功但未拿到音乐凭证，请重新扫码".to_string(),
        ));
    };
    let mut cookies = BTreeMap::new();
    cookies.insert("login_type".to_string(), "1".to_string());
    if let Some(uin) = uin.map(str::trim).filter(|uin| !uin.is_empty()) {
        cookies.insert("uin".to_string(), uin.to_string());
    }
    // 两个名字都写：不同接口读的字段不一样（与粘贴 cookie 那条路径保持一致）。
    for name in ["qqmusic_key", "qm_keyst"] {
        cookies.insert(name.to_string(), musickey.to_string());
    }
    let jar = session::jar();
    jar.store_cookies(MUSIC_URL, &cookies);
    // 换回来的 `qqmusic_key` 是音乐凭证，身份 cookie（uin）来自 `ptuiCB`。
    let state = jar.state();
    if !state.logged_in {
        return Ok(Poll::Failed(
            "凭证已换到但会话校验不通过，请重新扫码".to_string(),
        ));
    }
    Ok(Poll::Success(
        state.identity.unwrap_or_else(|| "未知账号".to_string()),
    ))
}

/// 手动跟一遍跳转链（**不能用自动跟随的客户端**：`Location` 与中间各跳的
/// `Set-Cookie` 都要留下），返回链上出现过的 `code`。
async fn follow(start: &str) -> Option<String> {
    let mut location = start.trim().to_string();
    let mut code = None;
    for hop in 0..MAX_REDIRECTS {
        if location.is_empty() {
            break;
        }
        if code.is_none() {
            code = query_value(&location, "code");
        }
        let Ok(response) = http::client_without_redirect()
            .get(&location)
            .header("Referer", OAUTH_JUMP)
            .send()
            .await
        else {
            break;
        };
        tracing::debug!(hop, status = %response.status(), "扫码跳转链");
        let next = location_of(&response).unwrap_or_default();
        if next.is_empty() {
            break;
        }
        location = resolve(&location, &next);
    }
    code
}

fn location_of(response: &reqwest::Response) -> Option<String> {
    response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .filter(|value| !value.is_empty())
}

/// 相对跳转要基于当前地址补全。
fn resolve(current: &str, next: &str) -> String {
    if next.starts_with("http") {
        return next.to_string();
    }
    let Some((scheme, rest)) = current.split_once("://") else {
        return next.to_string();
    };
    let host = rest.split('/').next().unwrap_or_default();
    let path = if next.starts_with('/') {
        next.to_string()
    } else {
        format!("/{next}")
    };
    format!("{scheme}://{host}{path}")
}

/// 取 URL 查询参数（值已做百分号解码）。
fn query_value(url: &str, name: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key == name {
            return Some(percent_decode(value));
        }
    }
    None
}

/// 只处理 `%XX` 与 `+`：够用且不引依赖。
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// 解析 `ptuiCB('code','0','url','0','message','nickname')`。
pub fn parse_ptui_cb(raw: &str) -> Option<(String, String, String, String)> {
    let start = raw.find("ptuiCB(")?;
    let body = &raw[start + "ptuiCB(".len()..];
    let end = body.find(')')?;
    let parts = body[..end]
        .split(',')
        .map(|part| part.trim().trim_matches('\'').to_string())
        .collect::<Vec<_>>();
    if parts.len() < 5 {
        return None;
    }
    Some((
        parts[0].clone(),
        parts[2].clone(),
        parts[4].clone(),
        parts.get(5).cloned().unwrap_or_default(),
    ))
}

/// `hash33`：QQ 各处（`ptqrtoken`）用的滚动哈希。
fn hash33(value: &str) -> u32 {
    hash33_seeded(value, 0)
}

/// 带种值的 `hash33`：`g_tk` 用的就是 `hash33(p_skey, 5381)`。
fn hash33_seeded(value: &str, seed: u32) -> u32 {
    let mut hash = seed;
    for byte in value.bytes() {
        hash = hash
            .wrapping_shl(5)
            .wrapping_add(hash)
            .wrapping_add(u32::from(byte));
    }
    hash & 0x7fff_ffff
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

/// `authorize` 的 `ui` 参数要一个 UUID 形状的随机串（官方页面也是页内现生成的）。
fn pseudo_uuid() -> String {
    let mut seed = now_millis() as u64 ^ (std::process::id() as u64) << 32;
    let mut bytes = [0_u8; 16];
    for byte in &mut bytes {
        // 简易 xorshift：够用来生成一个不会被服务端复用的标识。
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        *byte = (seed & 0xff) as u8;
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash33_matches_the_rolling_formula() {
        // 期望值由同一算法在 python 里独立算过。
        assert_eq!(hash33("test"), 4_282_592);
        assert_eq!(hash33_seeded("test", 5381), 2_090_756_197);
        assert_eq!(hash33(""), 0);
        // 末位必须落在 31 位以内，`ptqrtoken` 才是有效整数。
        assert!(hash33("1234567890") < 0x8000_0000);
    }

    #[test]
    fn parses_ptui_cb() {
        let body = "ptuiCB('0','0','https://ptlogin2.qq.com/login?uin=o1&ptsigx=abc','0','登录成功','昵称')";
        let (code, redirect, message, nickname) = parse_ptui_cb(body).expect("要能解析");
        assert_eq!(code, "0");
        assert_eq!(redirect, "https://ptlogin2.qq.com/login?uin=o1&ptsigx=abc");
        assert_eq!(message, "登录成功");
        assert_eq!(nickname, "昵称");
        // 等待扫码时 URL 位置是空串。
        assert_eq!(
            parse_ptui_cb("ptuiCB('66','0','','0','二维码未失效')")
                .unwrap()
                .0,
            "66"
        );
        assert!(parse_ptui_cb("<html>不是 ptuiCB</html>").is_none());
    }

    #[test]
    fn extracts_and_decodes_query_values() {
        let url = "https://x/login?uin=o0123456789&ptsigx=a%2Bb%3Dc&code=XYZ";
        assert_eq!(query_value(url, "uin").as_deref(), Some("o0123456789"));
        assert_eq!(query_value(url, "ptsigx").as_deref(), Some("a+b=c"));
        assert_eq!(query_value(url, "code").as_deref(), Some("XYZ"));
        assert_eq!(query_value(url, "missing"), None);
    }

    #[test]
    fn resolves_relative_redirects() {
        assert_eq!(
            resolve("https://a.qq.com/x/y", "https://b.qq.com/z"),
            "https://b.qq.com/z"
        );
        assert_eq!(resolve("https://a.qq.com/x/y", "/z"), "https://a.qq.com/z");
        assert_eq!(resolve("https://a.qq.com/x/y", "z"), "https://a.qq.com/z");
    }

    /// 本地三跳服务器：`/start` → 相对跳转 `/mid`（下发 cookie）→ `/end?code=XYZ`（下发 cookie）→ 200。
    ///
    /// 验的正是登录链里最容易写错的一段：手动跟着 `Location` 走、每跳的
    /// `Set-Cookie` 都要收下。voicefox 当年用自动跟随的客户端，这一段全丢了。
    #[test]
    fn follow_walks_the_chain_and_keeps_hop_cookies() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("测试服务要能监听");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut raw = [0_u8; 2048];
                let read = stream.read(&mut raw).unwrap_or(0);
                let request = String::from_utf8_lossy(&raw[..read]).to_string();
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                let response = match path.as_str() {
                    "/start" => {
                        "HTTP/1.1 302 Found\r\nLocation: /mid\r\nSet-Cookie: hop=one; Path=/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    }
                    "/mid" => {
                        "HTTP/1.1 302 Found\r\nLocation: /end?code=XYZ\r\nSet-Cookie: p_skey=test-pskey; Path=/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    }
                    _ => "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                };
                let _ = stream.write_all(response.as_bytes());
            }
        });

        let runtime = tokio::runtime::Runtime::new().expect("测试要能起运行时");
        let start = format!("http://127.0.0.1:{port}/start");
        let code = runtime.block_on(follow(&start));
        assert_eq!(
            code.as_deref(),
            Some("XYZ"),
            "链上带 code 的 Location 要认出来"
        );

        // 这里往全局 jar 里写的只有 127.0.0.1 的 cookie，不落盘（测试不调用 save）。
        let jar = session::jar();
        let origin = format!("http://127.0.0.1:{port}/");
        assert_eq!(jar.cookie_for(&origin, "hop").as_deref(), Some("one"));
        assert_eq!(
            jar.cookie_for(&origin, "p_skey").as_deref(),
            Some("test-pskey"),
            "中间那跳下发的 p_skey 必须留下——换 code 就靠它"
        );
    }

    #[test]
    fn pseudo_uuid_is_v4_shaped() {
        let uuid = pseudo_uuid();
        let parts = uuid.split('-').collect::<Vec<_>>();
        assert_eq!(
            parts.iter().map(|part| part.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
        assert!(uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
        assert_eq!(&uuid[14..15], "4");
    }
}

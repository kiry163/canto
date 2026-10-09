//! 会话：域名感知的 cookie jar + 登录校验 + 落盘。
//!
//! 为什么不用"一个扁平 map 装所有 cookie"：QQ 的服务端会用
//! `Set-Cookie: p_skey=` 明确的去清掉某个**域**上的旧凭据，
//! 扁平 map 会把别的域的同名 cookie 一起覆盖掉（voicefox 的扫码换 code 就是
//! 栽在这里）。这里用 `cookie_store` 按 domain/path/expires 存取，
//! 请求时再按 URL 取该带的那些。

use std::io::BufReader;
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow, bail};
use cookie_store::CookieStore;
use reqwest::header::HeaderValue;
use url::Url;

use crate::config;
use crate::qq::cookie::parse_cookie_header;

/// 粘贴进来的 cookie 统一挂到 `.qq.com`：QQ 音乐的接口分布在
/// `u.y.qq.com` / `c.y.qq.com` / `y.qq.com` 上，host-only 的域带不出去。
const COOKIE_DOMAIN: &str = ".qq.com";

/// 粘贴进来的 cookie 没有过期信息（复制文本里带不出 `Expires`），
/// 不给一个过期时间它就只是**会话 cookie**：`cookie_store` 的 `save` 会跳过
/// 非持久 cookie，登录跨不了进程。这里按 QQ 网页 cookie 的 30 天来给。
const COOKIE_MAX_AGE_SECS: u64 = 30 * 24 * 60 * 60;

/// 写进 jar 时用的"请求地址"：只用来决定默认 domain/path，必须与 [`COOKIE_DOMAIN`] 相容。
const COOKIE_URL: &str = "https://y.qq.com/";

/// 身份 cookie（`uin` 是 QQ 账号，`wxuin` 是微信账号）。
const ID_COOKIES: [&str; 2] = ["uin", "wxuin"];
/// 音乐凭证 cookie（`qqmusic_key` 是 QQ，`qm_keyst` 是微信）。
const KEY_COOKIES: [&str; 2] = ["qqmusic_key", "qm_keyst"];

/// 包一层 `Mutex` 让 jar 能挂在无 `&mut` 的 reqwest 上，同时还能被自己改。
pub struct Jar(Mutex<CookieStore>);

impl Jar {
    /// 从默认位置载入。
    fn load() -> Self {
        Self::load_from(&config::session_path())
    }

    /// 从指定文件载入；文件不存在或坏了都退回空 jar（坏文件不阻断启动，只提示）。
    fn load_from(path: &Path) -> Self {
        let store = read_store(path).unwrap_or_else(|error| {
            if path.exists() {
                tracing::warn!("会话文件读取失败（按未登录处理）：{error:#}");
            }
            CookieStore::default()
        });
        Self(Mutex::new(store))
    }

    fn with_store<T>(&self, act: impl FnOnce(&mut CookieStore) -> T) -> T {
        let mut store = self.0.lock().unwrap_or_else(|error| error.into_inner());
        act(&mut store)
    }

    /// 当前登录态：身份与凭证各命中一个才算登录。
    pub fn state(&self) -> SessionState {
        self.with_store(|store| {
            let url = Url::parse(COOKIE_URL).expect("内置 URL 必须可解析");
            let cookies = store
                .get_request_values(&url)
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect::<std::collections::BTreeMap<_, _>>();
            let identity = ID_COOKIES.iter().find_map(|name| cookies.get(*name));
            let has_key = KEY_COOKIES
                .iter()
                .any(|name| cookies.get(*name).is_some_and(|value| !value.is_empty()));
            SessionState {
                logged_in: identity.is_some() && has_key,
                identity: identity.cloned(),
            }
        })
    }

    /// 把粘贴的 cookie 写进 jar。
    ///
    /// 只改内存，不落盘——落盘由调用方显式调用 [`Jar::save`]（也让测试可以
    /// 用临时文件验证"存了能读回来"）。
    pub fn login_with_cookie(&self, raw: &str) -> Result<SessionState> {
        let cookies = parse_cookie_header(raw);
        if cookies.is_empty() {
            bail!("没解析出 cookie，形如 uin=o…; qqmusic_key=…; login_type=1");
        }
        if !ID_COOKIES.iter().any(|name| cookies.contains_key(*name)) {
            bail!("cookie 里没有身份（{}）", ID_COOKIES.join(" 或 "));
        }
        if !KEY_COOKIES.iter().any(|name| cookies.contains_key(*name)) {
            bail!("cookie 里没有音乐凭证（{}）", KEY_COOKIES.join(" 或 "));
        }

        let url = Url::parse(COOKIE_URL).expect("内置 URL 必须可解析");
        self.with_store(|store| {
            for (name, value) in &cookies {
                let raw = format!(
                    "{name}={value}; Domain={COOKIE_DOMAIN}; Path=/; Max-Age={COOKIE_MAX_AGE_SECS}"
                );
                if store.parse(&raw, &url).err().is_some() {
                    tracing::debug!("cookie {name} 未能写入 jar（形状非法？）");
                }
            }
        });
        let state = self.state();
        if !state.logged_in {
            bail!("cookie 写入后仍不完整，请确认复制的是整串（含 qqmusic_key）");
        }
        Ok(state)
    }

    /// 落盘到默认位置。
    pub fn save(&self) -> Result<()> {
        self.save_to(&config::session_path())
    }

    /// 落盘：`cookie_store` 自带的 JSON 序列化。
    fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建配置目录失败：{}", parent.display()))?;
        }
        let temp = path.with_extension("json.tmp");
        self.with_store(|store| -> Result<()> {
            let file = std::fs::File::create(&temp)
                .with_context(|| format!("写入会话文件失败：{}", temp.display()))?;
            let mut writer = std::io::BufWriter::new(file);
            // 用 `..._incl_expired_and_nonpersistent`：万一有 cookie 没带过期时间
            // （比如以后从响应里直接收下的会话 cookie），也不能在落盘时被静默丢掉。
            cookie_store::serde::json::save_incl_expired_and_nonpersistent(store, &mut writer)
                .map_err(|error| anyhow!("序列化 cookie jar 失败：{error}"))?;
            Ok(())
        })?;
        std::fs::rename(&temp, path)
            .with_context(|| format!("替换会话文件失败：{}", path.display()))?;
        Ok(())
    }

    /// 清空会话（含磁盘文件）。
    pub fn clear(&self) -> Result<()> {
        self.with_store(|store| store.clear());
        let path = config::session_path();
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => {
                Err(error).with_context(|| format!("删除会话文件失败：{}", path.display()))
            }
        }
    }
}

/// 当前登录态快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionState {
    pub logged_in: bool,
    /// 身份（`uin` / `wxuin` 的值），也是查个人歌单要用的账号 ID。
    pub identity: Option<String>,
}

fn read_store(path: &Path) -> Result<CookieStore> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("打开会话文件失败：{}", path.display()))?;
    // cookie_store 返回的是 `Box<dyn Error>`，anyhow 的 `context` 不适用，手动转。
    cookie_store::serde::json::load(BufReader::new(file))
        .map_err(|error| anyhow!("解析 cookie jar 失败：{error}"))
}

static JAR: std::sync::LazyLock<Arc<Jar>> = std::sync::LazyLock::new(|| Arc::new(Jar::load()));

/// 全局唯一的 jar：进程内所有请求共用，登录/退出也改它。
pub fn jar() -> Arc<Jar> {
    JAR.clone()
}

/// 把 `cookie_store` 接到 reqwest 上。
///
/// reqwest 只在 `reqwest_cookie_store`（另一个 crate）里提供适配，而那个 crate
/// 的版本跟得很慢；这里的适配只有两个方法，自己写反而更可控。
impl reqwest::cookie::CookieStore for Jar {
    fn set_cookies(&self, cookie_headers: &mut dyn Iterator<Item = &HeaderValue>, url: &Url) {
        self.with_store(|store| {
            for header in cookie_headers {
                let Ok(raw) = std::str::from_utf8(header.as_bytes()) else {
                    continue;
                };
                if let Err(error) = store.parse(raw, url) {
                    tracing::debug!("忽略无法解析的 Set-Cookie（{error}）：{raw}");
                }
            }
        });
    }

    fn cookies(&self, url: &Url) -> Option<HeaderValue> {
        let value = self.with_store(|store| {
            store
                .get_request_values(url)
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join("; ")
        });
        if value.is_empty() {
            return None;
        }
        HeaderValue::from_str(&value).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个不碰磁盘的 jar，返回登录失败的说明。
    fn login_error(raw: &str) -> String {
        let jar = Jar(Mutex::new(CookieStore::default()));
        jar.login_with_cookie(raw).unwrap_err().to_string()
    }

    #[test]
    fn login_requires_both_halves() {
        let only_key = login_error("qqmusic_key=K");
        assert!(only_key.contains("uin"), "缺身份要指名道姓：{only_key}");
        let only_id = login_error("uin=o123");
        assert!(only_id.contains("音乐凭证"), "缺凭证要指名道姓：{only_id}");
    }

    #[test]
    fn login_survives_a_restart() {
        // 粘贴来的 cookie 没有过期时间，属于**会话 cookie**；`cookie_store` 默认的
        // 落盘会跳过它们，登录就跨不了进程。这条测试就是钉住那个坑。
        let path = std::env::temp_dir()
            .join(format!("canto-session-test-{}", std::process::id()))
            .join("session.json");
        let jar = Jar(Mutex::new(CookieStore::default()));
        let state = jar
            .login_with_cookie("uin=o123; qqmusic_key=K; login_type=1")
            .expect("完整的 cookie 应该能登录");
        assert!(state.logged_in);
        jar.save_to(&path).expect("落盘应该成功");

        let reloaded = Jar::load_from(&path);
        let state = reloaded.state();
        assert!(state.logged_in, "重新载入后必须仍是登录态");
        assert_eq!(state.identity.as_deref(), Some("o123"));
        let header = reqwest::cookie::CookieStore::cookies(
            &reloaded,
            &Url::parse("https://c.y.qq.com/rsc/fcgi-bin/fcg_user_created_diss.fcg").unwrap(),
        )
        .expect("重载后 c.y.qq.com 也要能取到 cookie");
        assert!(header.to_str().unwrap().contains("qqmusic_key=K"));

        if let Some(dir) = path.parent() {
            std::fs::remove_dir_all(dir).ok();
        }
    }

    #[test]
    fn pasted_cookies_apply_to_every_qq_ymusic_host() {
        // 关键点：粘进来的 cookie 必须挂到 .qq.com，否则 u.y.qq.com / c.y.qq.com
        // 上的接口收不到（host-only 的 y.qq.com 带不出去）。
        let jar = Jar(Mutex::new(CookieStore::default()));
        jar.with_store(|store| {
            let url = Url::parse(COOKIE_URL).unwrap();
            store
                .parse("qqmusic_key=K; Domain=.qq.com; Path=/", &url)
                .unwrap();
            store
                .parse("uin=o123; Domain=.qq.com; Path=/", &url)
                .unwrap();
        });
        let state = jar.state();
        assert!(state.logged_in);
        assert_eq!(state.identity.as_deref(), Some("o123"));

        let header = reqwest::cookie::CookieStore::cookies(
            &jar,
            &Url::parse("https://u.y.qq.com/cgi-bin/musicu.fcg").unwrap(),
        )
        .expect("u.y.qq.com 必须能取到 cookie");
        let header = header.to_str().unwrap();
        assert!(header.contains("qqmusic_key=K"), "实际：{header}");
        assert!(header.contains("uin=o123"), "实际：{header}");
    }
}

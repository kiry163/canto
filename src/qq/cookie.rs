//! 手动粘贴 cookie 的解析。
//!
//! 与 voicefox 的 `parse_cookie_header` 同一套容错（那是实测过的形状），
//! 这里重写一份：人从浏览器里复制出来的东西五花八门，宁可多认几种。

use std::collections::BTreeMap;

/// `Set-Cookie` 里的属性名。它们出现在粘贴内容里时不是 cookie，必须丢掉，
/// 否则会作为垃圾 cookie 存进会话。
const COOKIE_ATTRIBUTES: [&str; 10] = [
    "path", "domain", "expires", "max-age", "httponly", "secure", "samesite", "priority",
    "comment", "version",
];

/// 解析粘进来的 cookie 文本。
///
/// 接受这几种形态，可以混着来（`;` 与换行都当分隔符）：
/// - `document.cookie` 或 DevTools 请求头里的 `a=1; b=2`；
/// - 逐行复制的 `Set-Cookie`（`Path` / `Domain` / `Expires` 等属性会被丢掉）；
/// - 带 `Cookie:` 前缀的整行。
///
/// 只做拆键值：不百分号解码、不校验。值为空的键（`p_skey=` 这种「清 cookie」
/// 写法）直接丢弃。
pub fn parse_cookie_header(raw: &str) -> BTreeMap<String, String> {
    let mut cookies = BTreeMap::new();
    for token in raw.split([';', '\n', '\r']) {
        let token = token.trim();
        let token = match token.strip_prefix("Cookie:") {
            Some(rest) => rest.trim(),
            None => token,
        };
        if token.is_empty() || token.starts_with('#') {
            continue;
        }
        let Some((name, value)) = token.split_once('=') else {
            continue;
        };
        let name = name.trim();
        let value = value.trim().trim_matches('"').trim();
        if name.is_empty()
            || value.is_empty()
            || COOKIE_ATTRIBUTES
                .iter()
                .any(|attribute| name.eq_ignore_ascii_case(attribute))
        {
            continue;
        }
        cookies.insert(name.to_string(), value.to_string());
    }
    cookies
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_cookie_header() {
        let cookies = parse_cookie_header("uin=o0123456789; qqmusic_key=ABC; login_type=1");
        assert_eq!(cookies.get("uin").map(String::as_str), Some("o0123456789"));
        assert_eq!(cookies.get("qqmusic_key").map(String::as_str), Some("ABC"));
        assert_eq!(cookies.get("login_type").map(String::as_str), Some("1"));
    }

    #[test]
    fn tolerates_the_shapes_people_actually_paste() {
        let pasted = "Cookie: uin=o1; qqmusic_key=K\r\n\
                      qm_keyst=K2\n\
                      # 注释行\n\
                      not-a-pair\n\
                      p_skey=\n\
                      quoted=\"v\"\n";
        let cookies = parse_cookie_header(pasted);
        assert_eq!(cookies.get("uin").map(String::as_str), Some("o1"));
        assert_eq!(cookies.get("qm_keyst").map(String::as_str), Some("K2"));
        assert_eq!(cookies.get("quoted").map(String::as_str), Some("v"));
        assert!(!cookies.contains_key("not-a-pair"), "没有 = 的片段要忽略");
        assert!(
            !cookies.contains_key("p_skey"),
            "空值等于清 cookie，不能收进来"
        );
    }

    #[test]
    fn drops_set_cookie_attributes() {
        let pasted = "qqmusic_key=K; Path=/; Domain=.qq.com; Expires=Wed, 21 Oct 2026 07:28:00 GMT; \
                      HttpOnly; Secure; SameSite=None; Max-Age=2592000";
        let cookies = parse_cookie_header(pasted);
        assert_eq!(cookies.len(), 1, "只应留下 qqmusic_key：{cookies:?}");
        assert_eq!(cookies.get("qqmusic_key").map(String::as_str), Some("K"));
    }

    #[test]
    fn keeps_values_containing_separators() {
        let cookies = parse_cookie_header("qqmusic_key=AAA=&BBB?C; uin=o2");
        assert_eq!(
            cookies.get("qqmusic_key").map(String::as_str),
            Some("AAA=&BBB?C")
        );
        assert_eq!(cookies.get("uin").map(String::as_str), Some("o2"));
    }
}

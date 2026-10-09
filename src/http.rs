//! HTTP 客户端。
//!
//! 统一 UA / 超时，并挂上会话 jar（见 [`crate::session`]）——QQ 音乐的接口
//! 对 UA 敏感，UA 不一致时返回的东西都不一样。

use std::sync::LazyLock;
use std::time::Duration;

use reqwest::Client;

use crate::session;

/// QQ 音乐网页版在用的 UA。改这个等于换客户端，接口返回可能跟着变。
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                      (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36";

/// 全局复用的客户端（连接池 + 会话 jar 都在这里）。
pub fn client() -> &'static Client {
    static CLIENT: LazyLock<Client> = LazyLock::new(|| {
        Client::builder()
            .user_agent(UA)
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(8))
            .cookie_provider(session::jar())
            .build()
            .expect("HTTP 客户端必须能建起来")
    });
    &CLIENT
}

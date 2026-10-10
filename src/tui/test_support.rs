//! 测试用的构造器：`App`、`Song`、`Playlist`。
//!
//! 只在 `cfg(test)` 下编译，所以不会给正式构建留下 unused 代码。
//! 放在单独的模块里是因为 `state` 的导航测试与 `draw` 的渲染测试都要用。

use std::sync::mpsc::channel;

use crate::config::Config;
use crate::model::{FileSizes, Playlist, Song};
use crate::player::Engine;

use super::state::App;

/// 测试用的 `App`，**连同它用的 tokio 运行时一起返回**——运行时是后台任务的
/// 宿主，测试必须一直持有它（丢掉运行时，后台任务就哑了）。调用方写成
/// `let (mut app, _runtime) = test_app();` 就行。
///
/// 另一点：音频设备是真开的（本机跑测试可用；无音频设备的 CI 要另想办法注入假引擎）。
pub(super) fn test_app() -> (App, tokio::runtime::Runtime) {
    let runtime = tokio::runtime::Runtime::new().expect("测试运行时");
    let (tx, _rx) = channel();
    let app = App::new(
        runtime.handle().clone(),
        Engine::start().expect("打开音频设备"),
        Config::default(),
        tx,
    );
    (app, runtime)
}

pub(super) fn test_song(mid: &str) -> Song {
    Song {
        mid: mid.to_string(),
        song_id: None,
        media_mid: None,
        title: format!("歌 {mid}"),
        singers: "歌手".to_string(),
        album_mid: String::new(),
        album_name: String::new(),
        duration_secs: 200,
        sizes: FileSizes::default(),
    }
}

pub(super) fn test_playlist(id: &str, name: &str) -> Playlist {
    Playlist {
        id: id.to_string(),
        dir_id: 1,
        name: name.to_string(),
        song_count: 3,
        cover_url: None,
        is_favorites: false,
    }
}

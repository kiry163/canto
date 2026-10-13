//! 测试用的构造器：`App`、`Song`、`Playlist`。
//!
//! 只在 `cfg(test)` 下编译，所以不会给正式构建留下 unused 代码。
//! 放在单独的模块里是因为 `state` 的导航测试与 `draw` 的渲染测试都要用。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::channel;

use crate::config::Config;
use crate::model::{FileSizes, Playlist, Song};
use crate::player::Engine;

use super::state::App;

/// 测试用的 `App`，**连同它用的 tokio 运行时一起返回**——运行时是后台任务的
/// 宿主，测试必须一直持有它（丢掉运行时，后台任务就哑了）。调用方写成
/// `let (mut app, _runtime) = test_app();` 就行。
///
/// 音频用哑引擎（`Engine::silent`），不碰真实设备：CI 上没有声卡也跑得动，
/// 开发机也不会因为设备被别的程序独占而挂掉测试。
pub(super) fn test_app() -> (App, tokio::runtime::Runtime) {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let runtime = tokio::runtime::Runtime::new().expect("测试运行时");
    let (tx, _rx) = channel();
    let app = App::new(
        runtime.handle().clone(),
        // 关掉缓存：测试不该往使用者真实的 ~/.cache/canto 里写东西。
        Engine::silent(false),
        Config::default(),
        // 每个测试一个独立的临时历史文件，免得互相看到对方写的。
        std::env::temp_dir().join(format!(
            "canto-test-{}-{unique}-history.json",
            std::process::id()
        )),
        tx,
    );
    (app, runtime)
}

/// 三句歌词，各隔 10 秒（绘制歌词栏 / 频谱区分栏用得上）。
pub(super) fn test_lyric() -> crate::model::Lyric {
    crate::model::Lyric {
        lines: (0..3)
            .map(|index| crate::model::LyricLine {
                at: std::time::Duration::from_secs(index * 10),
                text: format!("第 {} 句", index + 1),
                translation: (index == 1).then(|| "second line".to_string()),
            })
            .collect(),
    }
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
        source: crate::model::Source::Qq,
    }
}

/// 一棵两首曲目的临时曲库树：`<tmp>/canto/{a.wav, b.wav}`（内容是垃圾，只要扫得到）。
pub(super) fn temp_library_tree(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "canto-test-{name}-{}-library",
        std::process::id()
    ));
    std::fs::remove_dir_all(&root).ok();
    let canto = root.join("canto");
    std::fs::create_dir_all(&canto).expect("建曲库树");
    for file in ["a.wav", "b.wav"] {
        std::fs::write(canto.join(file), format!("{file} 的内容")).expect("写曲目");
    }
    root
}

/// 一首本地曲目（路径固定造，标题给得出来就行）。
pub(super) fn local_song(path: &str) -> Song {
    let mut song = test_song("");
    song.source = crate::model::Source::Local(std::path::PathBuf::from(path));
    song.mid = crate::model::local_mid(std::path::Path::new(path));
    song.title = std::path::Path::new(path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    song.singers = "张宇".to_string();
    song.sizes = FileSizes::default();
    song
}

/// 指定标题与歌手的曲目：筛选要按这两个字段命中，`test_song` 那套固定文案测不出来。
pub(super) fn titled_song(mid: &str, title: &str, singers: &str) -> Song {
    let mut song = test_song(mid);
    song.title = title.to_string();
    song.singers = singers.to_string();
    song
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

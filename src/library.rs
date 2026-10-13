//! 本地曲库：扫目录、读 tag、落索引。
//!
//! 与 QQ 那边的分工：`qq/**` 从接口拿歌，这里从磁盘拿歌；两边汇成同一个
//! [`crate::model::Song`]（本地曲目的 `source` 是 `Source::Local`，`mid` 是
//! [`crate::model::local_mid`] 合成的键）。
//!
//! 三条硬约束（都与「不能把使用者机器上的目录扫炸」有关）：**不跟软链**（成环也不怕）、
//! 跳过点开头的目录、同一路径只收一次。
// Task 3/4 的索引与扫描线程一接上就摘掉这个 `allow(dead_code)`：
// 现在是「模块先落地、消费者还没来」的中间态。
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::prelude::Accessor;
use serde::{Deserialize, Serialize};

/// 认得的扩展名。**等于现有解码器能解的那套**（mp3 / flac / m4a-AAC / ogg-Vorbis / wav），
/// 见 spec §8：白名单外的一律不进曲库。
pub const EXTENSIONS: &[&str] = &["mp3", "flac", "m4a", "mp4", "ogg", "wav"];

/// 认得出是音频、但这套解码器解不了的扩展名：只计数，好在扫完给一句人话。
const UNSUPPORTED_AUDIO: &[&str] =
    &["opus", "ape", "wma", "wv", "aiff", "aif", "dsf", "dff", "mpc", "m4b", "amr", "mid"];

/// 一条索引记录：`mtime`/`size` 是「要不要重读 tag」的判据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub path: PathBuf,
    /// 秒级修改时间（拿不到就是 0，那种文件每次都会重读）。
    pub mtime: u64,
    pub size: u64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_secs: u64,
}

/// 走一遍配置里的目录：返回候选文件的 `(路径, mtime, size)`，以及「认得出是音频但解不了」
/// 的条数。目录不存在 / 没权限一律跳过——曲库不该因为配置里一个坏路径就起不来。
pub fn walk(dirs: &[PathBuf]) -> (Vec<(PathBuf, u64, u64)>, usize) {
    let mut files = Vec::new();
    let mut unsupported = 0;
    for dir in dirs {
        walk_into(dir, &mut files, &mut unsupported);
    }
    files.sort();
    files.dedup_by(|left, right| left.0 == right.0);
    (files, unsupported)
}

fn walk_into(dir: &Path, files: &mut Vec<(PathBuf, u64, u64)>, unsupported: &mut usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        let kind = metadata.file_type();
        // 不跟软链：目录成环不会无限递归，文件软链也不会让同一首被收两遍。
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            if !is_hidden(&path) {
                walk_into(&path, files, unsupported);
            }
        } else if kind.is_file() {
            let extension = extension_of(&path);
            if extension.as_deref().is_some_and(|extension| EXTENSIONS.contains(&extension)) {
                files.push((path, mtime_secs(&metadata), metadata.len()));
            } else if extension
                .as_deref()
                .is_some_and(|extension| UNSUPPORTED_AUDIO.contains(&extension))
            {
                *unsupported += 1;
            }
        }
    }
}

/// 读一首文件的 tag 与属性。读不到的字段按「没有」处理：标题回落文件名、其余空、时长 0
/// （界面把 0 渲染成 `--:--`，不再假装 `00:00`）。
pub fn read_entry(path: &Path, mtime: u64, size: u64) -> Entry {
    let tagged = lofty::read_from_path(path).ok();
    let tag = tagged
        .as_ref()
        .and_then(|file| file.primary_tag().or_else(|| file.first_tag()));
    let text = |value: Option<std::borrow::Cow<'_, str>>| {
        value
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    Entry {
        path: path.to_path_buf(),
        mtime,
        size,
        title: tag
            .and_then(|tag| text(tag.title()))
            .unwrap_or_else(|| title_from_file_name(path)),
        artist: tag.and_then(|tag| text(tag.artist())).unwrap_or_default(),
        album: tag.and_then(|tag| text(tag.album())).unwrap_or_default(),
        duration_secs: tagged
            .as_ref()
            .map(|file| file.properties().duration().as_secs())
            .unwrap_or(0),
    }
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'))
}

fn extension_of(path: &Path) -> Option<String> {
    path.extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
}

fn title_from_file_name(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn mtime_secs(metadata: &std::fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一棵测试用的目录树：`canto/{a.mp3, b.FLAC, c.txt, .hidden/d.mp3, sub/e.wav}`。
    fn temp_tree(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("canto-library-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let canto = root.join("canto");
        std::fs::create_dir_all(canto.join(".hidden")).expect("建树");
        std::fs::create_dir_all(canto.join("sub")).expect("建树");
        for (path, bytes) in [
            (canto.join("a.mp3"), &b"aaa"[..]),
            (canto.join("b.FLAC"), &b"bbb"[..]),
            (canto.join("c.txt"), &b"ccc"[..]),
            (canto.join(".hidden/d.mp3"), &b"ddd"[..]),
            (canto.join("sub/e.wav"), &b"eee"[..]),
        ] {
            std::fs::write(path, bytes).expect("写文件");
        }
        root
    }

    fn file_name(path: &Path) -> String {
        path.file_name().unwrap_or_default().to_string_lossy().into_owned()
    }

    #[test]
    fn walk_collects_supported_files_and_counts_the_rest() {
        let dir = temp_tree("walk-basic");
        let (files, skipped) = walk(&[dir.join("canto")]);
        let names: Vec<String> = files.iter().map(|(path, _, _)| file_name(path)).collect();
        assert_eq!(names, ["a.mp3", "b.FLAC", "e.wav"], "大小写不敏感；点开头目录整棵跳过");
        assert_eq!(skipped, 0, "c.txt 不是音频，不参与「不支持的格式」计数");
    }

    #[test]
    fn walk_counts_audio_formats_we_cannot_decode() {
        let dir = temp_tree("walk-unsupported");
        std::fs::write(dir.join("canto/曲子.opus"), b"opus").expect("写 opus");
        std::fs::write(dir.join("canto/曲子.ape"), b"ape").expect("写 ape");
        let (files, skipped) = walk(&[dir.join("canto")]);
        assert_eq!(files.len(), 3, "不支持的音频不进列表");
        assert_eq!(skipped, 2, "opus/ape 要计数（提示才说得出口）");
    }

    #[test]
    fn walk_does_not_follow_symlinks_and_survives_a_cycle() {
        let dir = temp_tree("walk-cycle");
        // 真软链要权限，建不起来就只留「不跟软链」的实现约束（这条断言仍会跑到）。
        make_symlink(&dir.join("canto/loop"), &dir.join("canto")).ok();
        let (files, _) = walk(&[dir.join("canto")]);
        assert!(files.len() <= 3, "成环要不跟软链，不能无限递归：{files:?}");
    }

    #[test]
    fn walk_dedupes_and_sorts() {
        let dir = temp_tree("walk-dedupe");
        let (files, _) = walk(&[dir.join("canto"), dir.join("canto/")]);
        assert_eq!(files.len(), 3, "同一目录写两遍只算一次");
        let paths: Vec<&PathBuf> = files.iter().map(|(path, _, _)| path).collect();
        assert!(paths.windows(2).all(|pair| pair[0] <= pair[1]), "排序稳定：{paths:?}");
    }

    #[test]
    fn walk_skips_missing_and_relative_dirs_without_panicking() {
        let dir = temp_tree("walk-missing");
        let (files, _) = walk(&[dir.join("没有这个目录"), dir.join("canto")]);
        assert_eq!(files.len(), 3, "不存在的目录跳过，别炸");
    }

    #[test]
    fn read_entry_falls_back_to_the_file_name() {
        let dir = temp_tree("entry-fallback");
        let entry = read_entry(&dir.join("canto/a.mp3"), 1, 2);
        assert_eq!(entry.title, "a", "文件名（去掉扩展名）当标题");
        assert_eq!(entry.artist, "");
        assert_eq!(entry.album, "");
        assert_eq!(entry.duration_secs, 0, "读不出时长就是 0（界面渲染成 --:--）");
        assert_eq!(entry.mtime, 1);
        assert_eq!(entry.size, 2);
    }

    #[test]
    fn read_entry_reads_written_tags_and_duration() {
        let dir = temp_tree("entry-tags");
        let path = fixture_with_tags(&dir.join("canto/tagged.wav"), "曲终人散", "张宇", "消息");
        let entry = read_entry(&path, 7, 8);
        assert_eq!(entry.title, "曲终人散");
        assert_eq!(entry.artist, "张宇");
        assert_eq!(entry.album, "消息");
        assert!(entry.duration_secs >= 1, "合成 WAV 有真实时长：{}", entry.duration_secs);
    }

    /// 合成一个真能解码的正弦波 WAV（写法与 `player.rs` 的冒烟夹具一致），再写回 tag。
    fn fixture_with_tags(path: &Path, title: &str, artist: &str, album: &str) -> PathBuf {
        write_sine_wav(path);
        set_tags(path, title, artist, album);
        path.to_path_buf()
    }

    /// 用 `lofty` 把三个字段写进文件（产品代码只读不写，这里是为了造夹具）。
    fn set_tags(path: &Path, title: &str, artist: &str, album: &str) {
        use lofty::config::WriteOptions;
        use lofty::prelude::{Accessor, TagExt};
        use lofty::tag::{Tag, TagType};

        let mut tag = Tag::new(TagType::Id3v2);
        tag.set_title(title.to_string());
        tag.set_artist(artist.to_string());
        tag.set_album(album.to_string());
        tag.save_to_path(path, WriteOptions::default()).expect("写回 tag");
    }

    fn write_sine_wav(path: &Path) {
        const RATE: u32 = 44_100;
        let frames: Vec<i16> = (0..RATE)
            .map(|index| {
                let t = index as f32 / RATE as f32;
                (f32::sin(t * 440.0 * std::f32::consts::TAU) * 8_000.0) as i16
            })
            .collect();
        let mut bytes = Vec::with_capacity(44 + frames.len() * 2);
        bytes.extend(b"RIFF");
        bytes.extend(((36 + frames.len() * 2) as u32).to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes()); // PCM
        bytes.extend(1u16.to_le_bytes()); // 单声道
        bytes.extend(RATE.to_le_bytes());
        bytes.extend((RATE * 2).to_le_bytes());
        bytes.extend(2u16.to_le_bytes());
        bytes.extend(16u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend(((frames.len() * 2) as u32).to_le_bytes());
        for frame in frames {
            bytes.extend(frame.to_le_bytes());
        }
        std::fs::write(path, bytes).expect("写 WAV");
    }

    #[cfg(unix)]
    fn make_symlink(link: &Path, target: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn make_symlink(link: &Path, target: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_dir(target, link)
    }
}

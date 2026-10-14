//! 下载：把候选档位里第一个能打开的存成文件，歌词另存到**单独的目录**。
//!
//! 传输直接用 [`MediaStream`]：它已经处理了"服务端给 purl 但文件其实不存在"
//! （嗅探魔数）与 `Range` 重连，下载等于把它拷到磁盘。
//! 先写 `.part` 再改名，中途失败不会留下坏文件。

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::cache;
use crate::media::MediaStream;
use crate::model::{Lyric, Quality, Song};
use crate::qq::vkey::Candidate;

/// 下载索引：`<download_dir>/downloads.json`。
///
/// 这是「这份文件是哪首 QQ 歌」的**唯一**登记：播放缓存从文件名就读得出 mid，下载这边只有下载
/// 那一刻知道，所以必须写下来（曲库那边根本没有 mid）。字段与缓存侧对齐，另外带上离线显示要用
/// 的标题 / 歌手 / 专辑 / 时长——不登录、没网时列表也得写得出名字与时长。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Track {
    /// 哪首 QQ 歌。只用于两件事：在线歌起播前的本地优先判定、在线列表里那个 `↓`。
    pub mid: String,
    /// 相对 `download_dir` 的文件名（而不是绝对路径：整个目录搬走、备份、同步都还成立）。
    pub file: String,
    /// 下载时那份的档位（缓存侧从文件名读到的也是同一个东西）。
    pub quality: Quality,
    /// 容器名（`.flac` → `FLAC`），列表那一列写它。
    pub container: String,
    /// 登记时的字节数；`validate` 拿它跟磁盘对，不符就以磁盘为准。
    pub bytes: u64,
    pub title: String,
    pub singers: String,
    pub album: String,
    pub duration_secs: u64,
    /// 下载时间（Unix 秒）。
    pub at: u64,
}

/// 索引文件版本。字段含义变了就 +1：老文件整份丢弃（跟曲库索引一个约定，不做迁移）。
pub const INDEX_VERSION: u32 = 1;

/// 索引文件的内容。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Index {
    pub version: u32,
    pub tracks: Vec<Track>,
}

impl Default for Index {
    fn default() -> Self {
        Self {
            version: INDEX_VERSION,
            tracks: Vec::new(),
        }
    }
}

/// 索引文件的位置：**放在下载目录里**——整个目录拷走 / 备份时登记跟着走。
pub fn index_path(dir: &Path) -> PathBuf {
    dir.join("downloads.json")
}

impl Index {
    /// 读索引。缺失 / 坏了 / 版本不符一律当空：宁可当没有，也不要带着半份数据接着写。
    pub fn load(dir: &Path) -> Self {
        let path = index_path(dir);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match serde_json::from_str::<Self>(&text) {
            Ok(index) if index.version == INDEX_VERSION => index,
            Ok(index) => {
                tracing::warn!(
                    "下载索引版本是 {}（当前 {INDEX_VERSION}），按空处理",
                    index.version
                );
                Self::default()
            }
            Err(error) => {
                tracing::warn!("下载索引解析失败（按空处理）：{}：{error}", path.display());
                Self::default()
            }
        }
    }

    /// 写索引（临时文件 + rename，中途失败不留坏文件）。
    pub fn save(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("创建下载目录失败：{}", dir.display()))?;
        let path = index_path(dir);
        let text = serde_json::to_string_pretty(self).context("序列化下载索引失败")?;
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, text).with_context(|| format!("写 {} 失败", temp.display()))?;
        std::fs::rename(&temp, &path)
            .with_context(|| format!("把 {} 换成 {} 失败", temp.display(), path.display()))
    }

    /// 按 mid 找登记（「本地有这份吗」的判定）。
    pub fn by_mid(&self, mid: &str) -> Option<&Track> {
        self.tracks.iter().find(|entry| entry.mid == mid)
    }

    /// 这条登记在磁盘上的路径。
    pub fn path_of(&self, track: &Track, dir: &Path) -> PathBuf {
        dir.join(&track.file)
    }

    /// 这份路径归哪个 mid（界面判「来源」与播放条标签用它）。
    pub fn mid_of_path(&self, dir: &Path, path: &Path) -> Option<&str> {
        self.tracks
            .iter()
            .find(|entry| dir.join(&entry.file) == path)
            .map(|entry| entry.mid.as_str())
    }

    /// 记一条：同 mid 覆盖（顺序不动），新的追加到末尾。
    pub fn upsert(&mut self, track: Track) {
        match self.tracks.iter_mut().find(|entry| entry.mid == track.mid) {
            Some(slot) => *slot = track,
            None => self.tracks.push(track),
        }
    }

    /// 跟磁盘对一遍：文件不在的条目丢掉（幽灵），大小不符的以磁盘为准。
    ///
    /// 返回「有没有改过」，好让调用方决定要不要回写——索引是登记册，真相永远在磁盘上。
    pub fn validate(&mut self, dir: &Path) -> bool {
        let before = self.tracks.len();
        let mut changed = false;
        self.tracks.retain_mut(|entry| {
            let path = dir.join(&entry.file);
            let Ok(metadata) = std::fs::metadata(&path) else {
                tracing::info!("下载索引：{} 不在了，去掉这条", path.display());
                return false;
            };
            if metadata.len() != entry.bytes {
                entry.bytes = metadata.len();
                changed = true;
            }
            true
        });
        changed || self.tracks.len() != before
    }
}

/// 下载结果。
pub struct Saved {
    pub path: PathBuf,
    pub quality: Quality,
    pub bytes: u64,
    /// 一起写下的歌词文件（`.lrc` / `.trans.lrc`）。
    pub lyrics: Vec<PathBuf>,
    /// 文件本来就在，跳过了。
    pub existed: bool,
    /// 直接从播放缓存复制过来的（一个字节都没走网络）。
    pub from_cache: bool,
}

/// 下载第一个能打开的候选；`progress` 每读完一块回调一次（已收字节、总长）。
///
/// `dir` 放音频，`lyrics_dir` 放 `.lrc`——两者刻意分开：歌曲目录里不该夹着一堆歌词，
/// 想单独取歌词时也有个固定位置。
pub fn save(
    song: &Song,
    candidates: &[Candidate],
    dir: &Path,
    lyrics_dir: &Path,
    lyric: Option<&Lyric>,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<Saved> {
    std::fs::create_dir_all(dir).with_context(|| format!("创建下载目录失败：{}", dir.display()))?;
    let stem = file_stem(song);
    let mut failures = Vec::new();

    // 缓存里已经有这首（任意档位）：直接复制过去，别再下一遍。
    // 档位以缓存里那份为准——用户要的是「这首歌的文件」，不是「重新下一份配置档位」。
    if let Some((cached, quality)) = cache::for_song(&song.mid) {
        let ext = cached
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("bin");
        let path = dir.join(format!("{stem}.{ext}"));
        if path.exists() {
            return Ok(Saved {
                path,
                quality,
                bytes: 0,
                lyrics: Vec::new(),
                existed: true,
                from_cache: true,
            });
        }
        let bytes = std::fs::copy(&cached, &path)
            .with_context(|| format!("从缓存复制到 {} 失败", path.display()))?;
        progress(bytes, Some(bytes));
        let lyrics = write_lyrics(&stem, lyrics_dir, lyric)?;
        return Ok(Saved {
            path,
            quality,
            bytes,
            lyrics,
            existed: false,
            from_cache: true,
        });
    }

    for candidate in candidates {
        let path = dir.join(format!("{stem}.{}", candidate.ext));
        if path.exists() {
            return Ok(Saved {
                path,
                quality: candidate.quality,
                bytes: 0,
                lyrics: Vec::new(),
                existed: true,
                from_cache: false,
            });
        }
        let mut stream = match MediaStream::open(&candidate.url) {
            Ok(stream) => stream,
            Err(error) => {
                tracing::debug!("{} 档打不开：{error}", candidate.quality.label());
                failures.push(format!("{} 档打不开：{error}", candidate.quality.label()));
                continue;
            }
        };
        let total = stream.total();
        let part = path.with_extension(format!("{}.part", candidate.ext));
        let mut file =
            File::create(&part).with_context(|| format!("创建文件失败：{}", part.display()))?;
        let mut buffer = vec![0_u8; 256 * 1024];
        let mut received = 0_u64;
        let copied = (|| -> std::io::Result<()> {
            loop {
                let read = stream.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                file.write_all(&buffer[..read])?;
                received += read as u64;
                progress(received, total);
            }
            file.flush()
        })();
        if let Err(error) = copied {
            // 半截文件立刻删掉，别让它看起来像下好了。
            let _ = std::fs::remove_file(&part);
            failures.push(format!("{} 档写盘失败：{error}", candidate.quality.label()));
            continue;
        }
        std::fs::rename(&part, &path).with_context(|| format!("改名失败：{}", path.display()))?;
        let lyrics = write_lyrics(&stem, lyrics_dir, lyric)?;
        return Ok(Saved {
            path,
            quality: candidate.quality,
            bytes: received,
            lyrics,
            existed: false,
            from_cache: false,
        });
    }
    bail!("没有能下载的档位：{}", failures.join("；"))
}

/// 文件名用 `歌手 - 标题`，把路径分隔符等非法字符换掉；空名退回 mid。
///
/// 公开给 crate 内别处用（列表的 `↓` 标记要按同一个名字去下载目录里查表）——**改名规则只能有
/// 这一份**，两边各写一套迟早对不上。
pub(crate) fn file_stem(song: &Song) -> String {
    let sanitize = |text: &str| -> String {
        text.chars()
            .map(|character| match character {
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
                _ => character,
            })
            .collect()
    };
    let stem = format!("{} - {}", sanitize(&song.singers), sanitize(&song.title))
        .trim()
        .trim_matches('-')
        .trim()
        .to_string();
    if stem.is_empty() {
        song.mid.clone()
    } else {
        stem
    }
}

/// 扩展名 → 容器名（列表那一列的「音质」位就写它）。
///
/// **不猜码率**：`.mp3` 可能是 128 也可能是 320，文件本身读不出来；写容器是诚实的。
pub(crate) fn container_label(extension: &str) -> String {
    extension.to_ascii_uppercase()
}

/// 下载目录里的一份文件。
#[derive(Debug, Clone)]
pub struct Downloaded {
    /// 磁盘上那份（起播时直接开它——列表里写着 `↓` 就该放这一份）。
    pub path: PathBuf,
    /// 容器名（`.mp3` → `MP3`）。列表那一列的「音质」位就写它。
    pub container: String,
    pub bytes: u64,
}

/// 扫一遍下载目录：`文件名（不含扩展名） → 那份文件`。
///
/// 列表里的 `↓` 与「体积报磁盘那份」用它（按 `file_stem` 查表，O(1)）；起播优先放本地
/// 那份也用它（见 `player::local_copy`）。用文件名当键是**唯一可选**的口径：文件里没有 mid。
/// 同一个 stem 有多份时（mp3 + flac）按容器排 `flac/wav > m4a/ogg > 其它`，同级取体积大的
/// ——体积是文件本身唯一读得出的「档位高低」信号。
/// 目录不存在 / 没权限一律给空表（下载目录可能还没建）。
pub(crate) fn scan_downloads(dir: &Path) -> HashMap<String, Downloaded> {
    let mut found: HashMap<String, Downloaded> = HashMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let (Some(stem), Some(extension)) = (
            path.file_stem().map(|stem| stem.to_string_lossy().into_owned()),
            path.extension().map(|ext| ext.to_string_lossy().to_ascii_lowercase()),
        ) else {
            continue;
        };
        // 半截文件（`.part`）不算下好的一份。
        if extension == "part" {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let candidate = Downloaded {
            path,
            container: container_label(&extension),
            bytes: metadata.len(),
        };
        match found.get(&stem) {
            Some(current) if rank(&candidate) <= rank(current) => {}
            _ => {
                found.insert(stem, candidate);
            }
        }
    }
    found
}

/// 「哪一份更高」：先看容器，同级看体积。
fn rank(entry: &Downloaded) -> (u8, u64) {
    let tier = match entry.container.as_str() {
        "FLAC" | "WAV" => 2,
        "M4A" | "OGG" => 1,
        _ => 0,
    };
    (tier, entry.bytes)
}

/// 歌词落地：正文 `.lrc`；有翻译再写一份 `.trans.lrc`。都写进 `dir`（歌词目录）。
fn write_lyrics(stem: &str, dir: &Path, lyric: Option<&Lyric>) -> Result<Vec<PathBuf>> {
    let Some(lyric) = lyric.filter(|lyric| !lyric.lines.is_empty()) else {
        return Ok(Vec::new());
    };
    std::fs::create_dir_all(dir).with_context(|| format!("创建歌词目录失败：{}", dir.display()))?;
    let mut written = Vec::new();
    let main = dir.join(format!("{stem}.lrc"));
    std::fs::write(&main, render(lyric, false))
        .with_context(|| format!("写歌词失败：{}", main.display()))?;
    written.push(main);
    if lyric.lines.iter().any(|line| line.translation.is_some()) {
        let translated = dir.join(format!("{stem}.trans.lrc"));
        std::fs::write(&translated, render(lyric, true))
            .with_context(|| format!("写翻译歌词失败：{}", translated.display()))?;
        written.push(translated);
    }
    Ok(written)
}

/// 把解析后的歌词写回 LRC（时间戳已换算成绝对值，offset 也应用过了）。
fn render(lyric: &Lyric, translation: bool) -> String {
    let mut out = String::new();
    for line in &lyric.lines {
        let text = if translation {
            line.translation.as_deref()
        } else {
            Some(line.text.as_str())
        };
        let Some(text) = text else { continue };
        let seconds = line.at.as_secs();
        out.push_str(&format!(
            "[{:02}:{:02}.{:02}]{text}\n",
            seconds / 60,
            seconds % 60,
            line.at.subsec_millis() / 10
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(title: &str, singers: &str) -> Song {
        Song {
            mid: "000CbUE00I998M".to_string(),
            song_id: None,
            media_mid: None,
            title: title.to_string(),
            singers: singers.to_string(),
            album_mid: String::new(),
            album_name: String::new(),
            duration_secs: 236,
            sizes: crate::model::FileSizes::default(),
            local_bytes: 0,
            source: crate::model::Source::Qq,
        }
    }

    #[test]
    fn file_names_drop_path_hostile_characters() {
        assert_eq!(file_stem(&song("晴天", "周杰伦")), "周杰伦 - 晴天");
        // 标题里的 `/`、`:` 会让路径跑偏，必须换掉。
        assert_eq!(
            file_stem(&song("AC/DC: Live", "群星")),
            "群星 - AC_DC_ Live"
        );
        // 标题与歌手都空 → 退回 mid，不能生成 ".flac" 这种文件。
        assert_eq!(file_stem(&song("", "")), "000CbUE00I998M");
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("canto-download-{}-{tag}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    /// 一条索引记录（测试用）：体积 0 表示「还没跟磁盘对过」。
    pub(super) fn track(mid: &str, file: &str) -> Track {
        Track {
            mid: mid.to_string(),
            file: file.to_string(),
            quality: Quality::Flac,
            container: "FLAC".to_string(),
            bytes: 0,
            title: format!("标题 {mid}"),
            singers: "歌手".to_string(),
            album: "专辑".to_string(),
            duration_secs: 1,
            at: 1,
        }
    }

    #[test]
    fn the_index_round_trips_and_treats_anything_odd_as_empty() {
        let dir = temp_dir("index-basics");
        assert!(Index::load(&dir).tracks.is_empty(), "缺失 = 空");

        std::fs::write(index_path(&dir), "{ 不是 JSON").expect("写坏文件");
        assert!(Index::load(&dir).tracks.is_empty(), "坏了 = 空");

        let mut index = Index::default();
        index.upsert(track("m1", "歌手 - 一首.flac"));
        index.save(&dir).expect("落盘");
        let back = Index::load(&dir);
        assert_eq!(back.tracks, index.tracks);
        assert_eq!(
            back.by_mid("m1").map(|entry| entry.file.as_str()),
            Some("歌手 - 一首.flac")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_version_mismatch_is_empty_too() {
        let dir = temp_dir("index-version");
        std::fs::write(index_path(&dir), r#"{"version":99,"tracks":[]}"#).expect("写文件");
        assert!(Index::load(&dir).tracks.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn upsert_replaces_the_same_mid_only() {
        let mut index = Index::default();
        index.upsert(track("m1", "a.flac"));
        index.upsert(track("m2", "b.flac"));
        index.upsert(track("m1", "a2.flac"));
        let files: Vec<&str> = index.tracks.iter().map(|entry| entry.file.as_str()).collect();
        assert_eq!(files, ["a2.flac", "b.flac"], "同 mid 覆盖，别的条目顺序不动");
    }

    #[test]
    fn validate_drops_ghosts_and_trusts_the_disk_for_sizes() {
        let dir = temp_dir("index-validate");
        std::fs::write(dir.join("活着.flac"), vec![7_u8; 11]).expect("写文件");
        let mut index = Index::default();
        index.upsert(track("live", "活着.flac"));
        index.upsert(track("ghost", "没有这个文件.flac"));
        assert!(index.validate(&dir), "丢了一条幽灵条目 = 改过");
        assert_eq!(index.tracks.len(), 1);
        assert_eq!(index.by_mid("live").expect("还在").bytes, 11, "大小以磁盘为准");
        assert!(!index.validate(&dir), "再验一次不该说改过");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_path_knows_its_mid() {
        let dir = temp_dir("index-mid-of-path");
        let mut index = Index::default();
        index.upsert(track("m1", "一首.flac"));
        let path = index.path_of(index.by_mid("m1").expect("有这条"), &dir);
        assert_eq!(index.mid_of_path(&dir, &path), Some("m1"));
        assert_eq!(index.mid_of_path(&dir, Path::new("/别处/一首.flac")), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 歌词写进**歌词目录**（不是歌曲目录），名字与音频同 stem。
    #[test]
    fn lyrics_land_in_their_own_directory() {
        use crate::model::LyricLine;
        let root = std::env::temp_dir().join(format!("canto-dl-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let songs = root.join("songs");
        let lyrics = root.join("lyrics");
        std::fs::create_dir_all(&songs).expect("建歌曲目录");

        let lyric = Lyric {
            lines: vec![
                LyricLine {
                    at: std::time::Duration::from_secs(1),
                    text: "第一句".to_string(),
                    translation: None,
                },
                LyricLine {
                    at: std::time::Duration::from_secs(2),
                    text: "第二句".to_string(),
                    translation: Some("second".to_string()),
                },
            ],
        };
        let written = write_lyrics("歌手 - 标题", &lyrics, Some(&lyric)).expect("写歌词");
        assert_eq!(written.len(), 2, "正文 + 翻译各一份");
        assert!(lyrics.join("歌手 - 标题.lrc").is_file(), "{written:?}");
        assert!(lyrics.join("歌手 - 标题.trans.lrc").is_file(), "{written:?}");
        assert_eq!(
            std::fs::read_dir(&songs).expect("列歌曲目录").count(),
            0,
            "歌曲目录里不该混进歌词"
        );

        // 没有歌词（或歌词是空的）就一个文件都不写。
        assert!(write_lyrics("歌手 - 标题", &lyrics, None).expect("没歌词").is_empty());
        assert!(
            write_lyrics("歌手 - 标题", &lyrics, Some(&Lyric::default()))
                .expect("空歌词")
                .is_empty()
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rendered_lrc_survives_a_round_trip() {
        // 翻译文件带同样的 offset，两边换算后时间一致 → 会配对。
        let lyric = Lyric::parse(
            "[offset:500]\n[00:01.00]第一句\n[01:02.50]第二句\n",
            Some("[offset:500]\n[00:01.00]first\n[01:02.50]second\n"),
        );
        let rendered = render(&lyric, false);
        assert_eq!(
            rendered, "[00:00.50]第一句\n[01:02.00]第二句\n",
            "offset 已经算进时间戳了"
        );
        let again = Lyric::parse(&rendered, None);
        assert_eq!(again.lines, {
            let mut expected = lyric.lines.clone();
            for line in &mut expected {
                line.translation = None;
            }
            expected
        });
        assert_eq!(render(&lyric, true), "[00:00.50]first\n[01:02.00]second\n");

        // 翻译文件若没带同样的 offset，两边绝对时间对不上 → 不配对、不写翻译文件。
        let mismatched = Lyric::parse(
            "[offset:500]\n[00:01.00]第一句\n",
            Some("[00:01.00]first\n"),
        );
        assert!(mismatched.lines[0].translation.is_none());
        assert_eq!(render(&mismatched, true), "");
    }

    /// 扫下载目录：同名多份取「更高」的那份；`.part` 与目录不认；没有目录就是空表。
    #[test]
    fn scan_downloads_keeps_the_highest_copy_and_skips_parts() {
        let dir = std::env::temp_dir().join(format!("canto-dl-scan-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join("sub")).expect("建目录");
        std::fs::write(dir.join("刘若英 - 成全.mp3"), vec![0_u8; 44_000]).expect("写 mp3");
        std::fs::write(dir.join("刘若英 - 成全.flac"), vec![0_u8; 314_570]).expect("写 flac");
        std::fs::write(dir.join("周杰伦 - 晴天.mp3.part"), "半截".as_bytes()).expect("写半截");
        std::fs::write(dir.join("README.txt"), "说明".as_bytes()).expect("写非音频");

        let found = scan_downloads(&dir);
        let best = found.get("刘若英 - 成全").expect("同名两份里总有一份");
        assert_eq!(best.container, "FLAC", "取容器更高的那份：{found:?}");
        assert_eq!(best.bytes, 314_570);
        assert_eq!(
            best.path,
            dir.join("刘若英 - 成全.flac"),
            "路径要指到磁盘上那份（起播直接开它）"
        );
        assert!(
            !found.contains_key("周杰伦 - 晴天"),
            "`.part` 不算：{found:?}"
        );
        assert_eq!(found.get("README").map(|found| found.container.as_str()), Some("TXT"));
        assert!(
            scan_downloads(&dir.join("没有这个目录")).is_empty(),
            "目录不存在就是空表"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 扫出来的键就是落盘时用的文件名：两边共用 `file_stem`，改名规则不许各写一套。
    #[test]
    fn the_scanned_key_matches_the_file_we_write() {
        let song = song("成全", "刘若英");
        let dir = std::env::temp_dir().join(format!("canto-dl-key-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("建目录");
        std::fs::write(dir.join(format!("{}.mp3", file_stem(&song))), b"x").expect("写文件");

        let found = scan_downloads(&dir);
        assert_eq!(
            found.get(&file_stem(&song)).map(|found| found.container.as_str()),
            Some("MP3")
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn container_label_is_the_uppercase_extension() {
        assert_eq!(container_label("flac"), "FLAC");
        assert_eq!(container_label("M4A"), "M4A");
        assert_eq!(container_label("mp3"), "MP3");
    }
}

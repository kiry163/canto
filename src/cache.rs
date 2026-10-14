//! 播放缓存：听过的歌留在本地，重听不再走网络。
//!
//! 规则只有一条：**从头顺序读完**的才算数。边播边写 `<mid>.<ext>.part`，写出去的部分
//! 必须连成一片、并且真的铺到文件尾，才改名成正式缓存；中途往前跳 / 往回拖留下洞的，
//! 这次就不进缓存（`.part` 直接删掉），免得留一堆谁也接不上的半截文件。
//!
//! 文件名是 `<mid>.<档位>.<扩展名>`：mid 全局唯一且不像 vkey 会过期，档位写进名字
//! 是为了算「这份是什么档位」时不用猜（`320` 与 `128` 都是 `.mp3`，光看扩展名分不出）。

use std::collections::HashMap;
use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::model::{Lyric, Quality};
use crate::paths;

/// 缓存文件名（完整 / 临时）。
fn names_in(dir: &Path, mid: &str, tier: &str, ext: &str) -> (PathBuf, PathBuf) {
    let done = dir.join(format!("{mid}.{tier}.{ext}"));
    let part = dir.join(format!("{mid}.{tier}.{ext}.part"));
    (done, part)
}

/// 这首歌**任意档位**的完整缓存：`(路径, 档位)`。
///
/// 播放与下载都先问它：命中就直接读本地文件（播放不必再取流、下载直接复制），
/// 档位以缓存里那份为准。
pub fn for_song(mid: &str) -> Option<(PathBuf, Quality)> {
    for_song_in(&paths::cache_dir(), mid)
}

pub(crate) fn for_song_in(dir: &Path, mid: &str) -> Option<(PathBuf, Quality)> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.metadata().ok()?.is_file() {
            continue;
        }
        let name = path.file_name()?.to_str()?;
        if name.ends_with(".part") {
            continue;
        }
        // `<mid>.<档位>.<扩展名>`：mid 自己可能带点，所以按「从右边切两段」解析。
        let mut parts = name.rsplitn(3, '.');
        let ext = parts.next()?;
        let tier = parts.next()?;
        let name_mid = parts.next()?;
        if name_mid == mid && !ext.is_empty() {
            return Quality::parse(tier).map(|quality| (path, quality));
        }
    }
    None
}

/// 缓存里已经有的那些歌：mid → （档位，文件字节数）。列表拿它标「已缓存」并显示
/// 缓存里那份的档位（缓存命中时播放/下载用的就是它）。启动时扫一遍。
pub fn cached_entries() -> HashMap<String, (Quality, u64)> {
    cached_entries_in(&paths::cache_dir())
}

/// 歌词缓存目录：**就是缓存根目录**（歌词与音频同目录，`<mid>.lrc` 与 `<mid>.<档位>.<ext>`
/// 并排躺着）。老版本的 `lyrics/` 子目录只在 `sweep` 里被顺手清掉，不再被读。
pub fn lyric_dir() -> PathBuf {
    paths::cache_dir()
}

fn read_lyric_in(dir: &Path, mid: &str) -> Option<Option<Lyric>> {
    let text = std::fs::read_to_string(dir.join(format!("{mid}.lrc"))).ok()?;
    if text.trim().is_empty() {
        return Some(None);
    }
    let translation = std::fs::read_to_string(dir.join(format!("{mid}.trans.lrc"))).ok();
    let lyric = Lyric::parse(&text, translation.as_deref());
    Some((!lyric.lines.is_empty()).then_some(lyric))
}

fn put_lyric_in(dir: &Path, mid: &str, lyric: Option<&Lyric>) {
    std::fs::create_dir_all(dir).ok();
    let main = dir.join(format!("{mid}.lrc"));
    let translated = dir.join(format!("{mid}.trans.lrc"));
    let Some(lyric) = lyric.filter(|lyric| !lyric.lines.is_empty()) else {
        let _ = std::fs::write(&main, "");
        let _ = std::fs::remove_file(&translated);
        return;
    };
    let tmp = dir.join(format!("{mid}.lrc.tmp"));
    if std::fs::write(&tmp, lyric.to_lrc(false)).is_err() {
        return;
    }
    let _ = std::fs::rename(&tmp, &main);
    if lyric.lines.iter().any(|line| line.translation.is_some()) {
        if std::fs::write(&tmp, lyric.to_lrc(true)).is_ok() {
            let _ = std::fs::rename(&tmp, &translated);
        }
    } else {
        let _ = std::fs::remove_file(&translated);
    }
}

/// `<mid>.<档位>.<扩展名>` → `(mid, 档位)`；不是音频的一律 `None`。
///
/// 这是「哪些文件算缓存里的歌」的**唯一**口径：`cached_entries_in`（列表要看）与 `sweep_dir`
/// （超限要删）都走它，免得一处认一处不认。mid 自己可能带点，所以按「从右边切两段」解析；
/// `.lrc` / `.trans.lrc` / `library.json`（曲库索引）都在这儿被挡掉。
fn parse_track_name(name: &str) -> Option<(&str, Quality)> {
    let mut parts = name.rsplitn(3, '.');
    let ext = parts.next()?;
    let tier = parts.next()?;
    let mid = parts.next()?;
    // `.part` 是没下完的半截：`x.flac.part` 会被切成 tier = "flac"，光看档位认不出来。
    if ext.is_empty() || ext == "part" || mid.is_empty() {
        return None;
    }
    Quality::parse(tier).map(|quality| (mid, quality))
}

fn cached_entries_in(dir: &Path) -> HashMap<String, (Quality, u64)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return HashMap::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let metadata = entry.metadata().ok()?;
            if !metadata.is_file() {
                return None;
            }
            let name = path.file_name()?.to_str()?;
            let (mid, quality) = parse_track_name(name)?;
            Some((mid.to_string(), (quality, metadata.len())))
        })
        .collect()
}

/// 读歌词缓存：`None` = 没缓存过；`Some(None)` = 上次问过、这首歌确实没有歌词
/// （下次不必再问接口）；`Some(Some(_))` = 有。
pub fn lyric(mid: &str) -> Option<Option<Lyric>> {
    read_lyric_in(&lyric_dir(), mid)
}

/// 写歌词缓存（`lyric = None` 时写个空文件，记住「这首没有歌词」）。
pub fn put_lyric(mid: &str, lyric: Option<&Lyric>) {
    put_lyric_in(&lyric_dir(), mid, lyric)
}

/// 启动时清理：先删上次没退干净留下的 `.part`，再按「最久没听的先删」压到上限以内。
/// 返回 `(删了几个, 释放了多少字节)`。上限 0 = 缓存关掉，这时把存货也清光。
pub fn sweep(limit_bytes: u64) -> (usize, u64) {
    sweep_dir(&paths::cache_dir(), limit_bytes)
}

fn sweep_dir(dir: &Path, limit_bytes: u64) -> (usize, u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    let mut deleted = 0;
    let mut freed = 0;
    let mut kept: Vec<(PathBuf, u64, SystemTime)> = Vec::new();
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "part") {
            // 半截文件接不上（不顺序读完整首就没法用），留着只占地方。
            if std::fs::remove_file(&path).is_ok() {
                deleted += 1;
                freed += metadata.len();
            }
            continue;
        }
        // 只认音频形态：`.lrc`（歌词）、`library.json`（曲库索引）不是缓存——既不算进上限，
        // 也不能被删（旧实现把它们一起算、一起删，`library.json` 就这样被误删过）。
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if parse_track_name(name).is_none() {
            continue;
        }
        let at = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        kept.push((path, metadata.len(), at));
    }
    // 老版本把歌词放在 `<cache>/lyrics/`：现在不再被读，留着只占地方，顺手清掉。
    let legacy = dir.join("lyrics");
    if legacy.is_dir()
        && let Err(error) = std::fs::remove_dir_all(&legacy)
    {
        tracing::warn!("清掉老歌词子目录失败：{}：{error}", legacy.display());
    }
    let mut total: u64 = kept.iter().map(|(_, size, _)| *size).sum();
    if total <= limit_bytes {
        return (deleted, freed);
    }
    kept.sort_by_key(|(_, _, at)| *at);
    for (path, size, _) in kept {
        if total <= limit_bytes {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(size);
            deleted += 1;
            freed += size;
        }
    }
    (deleted, freed)
}

/// 「刚转正」的回传通道：(mid, 档位, 字节数)。音频线程往里塞，界面每帧取走。
pub type Promoted = Arc<Mutex<Vec<(String, Quality, u64)>>>;

/// 边播边写盘的那一半（挂给 `MediaStream`）。
pub(crate) struct Write {
    file: File,
    part: PathBuf,
    done: PathBuf,
    /// 这首的 mid 与档位，转正时回传给界面。
    mid: String,
    tier: Quality,
    /// 转正后把 (mid, 档位, 字节数) 塞进去（界面的「已缓存」标记靠它刷新）。
    promoted: Promoted,
    /// 从 0 开始已经**连续**写到哪；洞之后的字节一律不写。
    prefix: u64,
    /// 收尾过了就不再删 `part`（正常完成或被判定不合格，两种都在 `finish` 里处理）。
    finished: bool,
}

impl Write {
    pub(crate) fn new(
        mid: &str,
        tier: &str,
        ext: &str,
        promoted: Promoted,
    ) -> std::io::Result<Self> {
        Self::new_in(&paths::cache_dir(), mid, tier, ext, promoted)
    }

    fn new_in(
        dir: &Path,
        mid: &str,
        tier: &str,
        ext: &str,
        promoted: Promoted,
    ) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let (done, part) = names_in(dir, mid, tier, ext);
        // 上一次留下的 `.part` 接不上，重来。
        let _ = std::fs::remove_file(&part);
        Ok(Self {
            file: File::create(&part)?,
            part,
            done,
            mid: mid.to_string(),
            // 文件名里的档位一定是规范名（`with_cache` 传的是 `Quality::token()`）。
            tier: Quality::parse(tier).unwrap_or(Quality::High320),
            promoted,
            prefix: 0,
            finished: false,
        })
    }

    /// 写入正好接在连续部分后面的字节；跳着读到的（留洞的）直接丢。
    pub(crate) fn put(&mut self, offset: u64, bytes: &[u8]) {
        if offset != self.prefix || bytes.is_empty() {
            return;
        }
        if self.file.write_all(bytes).is_ok() {
            self.prefix += bytes.len() as u64;
        }
    }

    /// 流读到头了：连续部分真的铺满整个文件才算完整，否则删掉半截。
    ///
    /// `end` = 这次实际读到的末尾偏移（拿不到 `Content-Length` 时用它兜底）。
    pub(crate) fn finish(mut self, end: u64, total: Option<u64>) {
        self.finished = true;
        if self.prefix == total.unwrap_or(end) && self.prefix > 0 {
            let _ = self.file.flush();
            // 转正成功才回传：界面拿它把这一行标成「已缓存」。
            if std::fs::rename(&self.part, &self.done).is_ok()
                && let Ok(mut promoted) = self.promoted.lock()
            {
                promoted.push((self.mid.clone(), self.tier, self.prefix));
            }
        } else {
            let _ = std::fs::remove_file(&self.part);
        }
    }
}

impl Drop for Write {
    fn drop(&mut self) {
        // 半路换歌 / 退出：这次没听完，半截文件没用。
        if !self.finished {
            let _ = std::fs::remove_file(&self.part);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn promoted_sink() -> Promoted {
        Arc::new(Mutex::new(Vec::new()))
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("canto-cache-{}-{tag}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    /// 顺序写完 → 认作完整：正式文件在，`.part` 不在，并且回传 mid 让界面刷新。
    #[test]
    fn a_sequentially_read_track_becomes_a_complete_cache_file() {
        let dir = temp_dir("complete");
        let (done, part) = names_in(&dir, "mid1", "320", "mp3");
        let promoted = promoted_sink();
        let mut write =
            Write::new_in(&dir, "mid1", "320", "mp3", promoted.clone()).expect("建缓存文件");
        write.put(0, &[1, 2, 3]);
        write.put(3, &[4, 5]);
        write.finish(5, Some(5));

        assert!(done.is_file(), "顺序读完该留下完整缓存");
        assert!(!part.exists(), "临时文件该改名过去");
        assert_eq!(std::fs::read(&done).expect("读回"), vec![1, 2, 3, 4, 5]);
        assert_eq!(
            *promoted.lock().expect("锁"),
            vec![("mid1".to_string(), Quality::High320, 5)],
            "转正要回传 mid、档位与字节数"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 跳着读（留洞）→ 不认：正式文件不出现，`.part` 也删掉。
    #[test]
    fn a_hole_in_the_middle_is_not_cached() {
        let dir = temp_dir("hole");
        let (done, part) = names_in(&dir, "mid2", "flac", "flac");
        let mut write = Write::new_in(&dir, "mid2", "flac", "flac", promoted_sink()).expect("建缓存文件");
        write.put(0, &[1, 2, 3]);
        write.put(999, &[9]); // 往前跳：中间这段没读，缓存接不上
        write.finish(1000, Some(1000));

        assert!(!done.exists(), "有洞就不算完整");
        assert!(!part.exists(), "半截文件不该留下");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 半路换歌（没走到 `finish`）→ `Drop` 把 `.part` 删掉。
    #[test]
    fn dropping_mid_track_leaves_nothing_behind() {
        let dir = temp_dir("drop");
        let (done, part) = names_in(&dir, "mid3", "320", "mp3");
        {
            let mut write = Write::new_in(&dir, "mid3", "320", "mp3", promoted_sink()).expect("建缓存文件");
            write.put(0, &[1, 2, 3, 4]);
        }
        assert!(!part.exists(), "没听完就不该留半截");
        assert!(!done.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 歌词与缓存**同目录**（不再有 `lyrics/` 子目录），同目录下音频与歌词互不冒充。
    #[test]
    fn lyrics_live_beside_the_tracks() {
        assert_eq!(
            lyric_dir(),
            paths::cache_dir(),
            "歌词就放缓存根目录，不再单独一个 lyrics/ 子目录"
        );

        let dir = temp_dir("flat-lyrics");
        std::fs::write(dir.join("m1.320.mp3"), vec![0_u8; 10]).expect("写缓存");
        std::fs::write(dir.join("m1.lrc"), "[00:01.00]第一句\n").expect("写歌词");
        std::fs::write(dir.join("m1.trans.lrc"), "[00:01.00]译文\n").expect("写翻译");
        std::fs::write(dir.join("library.json"), "{}").expect("写曲库索引");

        let entries = cached_entries_in(&dir);
        assert_eq!(entries.get("m1"), Some(&(Quality::High320, 10)), "只认音频：{entries:?}");
        assert_eq!(entries.len(), 1, "歌词与索引都不是「缓存里的歌」：{entries:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 超限时只淘汰音频：歌词与曲库索引既不算进上限、也不删（旧实现会把它们一起算）。
    #[test]
    fn sweep_only_evicts_tracks() {
        let dir = temp_dir("sweep-tracks");
        std::fs::write(dir.join("a.128.mp3"), vec![0_u8; 10]).expect("写音频");
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(dir.join("b.128.mp3"), vec![0_u8; 10]).expect("写音频");
        std::fs::write(dir.join("a.lrc"), vec![0_u8; 5]).expect("写歌词");
        std::fs::write(dir.join("library.json"), vec![0_u8; 5]).expect("写曲库索引");

        // 音频一共 20 字节，上限 10：只该走掉最老的那个。
        let (deleted, _) = sweep_dir(&dir, 10);
        assert_eq!(deleted, 1, "只删一个音频");
        assert!(!dir.join("a.128.mp3").exists(), "最久没听的那个音频被删");
        assert!(dir.join("b.128.mp3").exists(), "另一个音频留下");
        assert!(dir.join("a.lrc").exists(), "歌词不能动");
        assert!(dir.join("library.json").exists(), "曲库索引不能动");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 老版本把歌词放在 `<cache>/lyrics/`：不再被读，`sweep` 顺手清掉（上限没超也要清）。
    #[test]
    fn a_legacy_lyrics_subdirectory_is_cleaned_up() {
        let dir = temp_dir("sweep-legacy");
        std::fs::create_dir_all(dir.join("lyrics")).expect("建老歌词目录");
        std::fs::write(dir.join("lyrics/m1.lrc"), "x").expect("写老歌词");
        sweep_dir(&dir, 0);
        assert!(!dir.join("lyrics").exists(), "老歌词子目录顺手删掉");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 清理：先删 `.part`，再按最久没听的先删，压到上限以内。
    #[test]
    fn sweeping_drops_parts_and_the_oldest_files_first() {
        let dir = temp_dir("sweep");
        // 谁更老靠写的先后排出来，不调 `set_modified`：有的环境（实测是 wine）上它不生效，
        // 两个文件的 mtime 会一样，「最久没听」就没法判定了。
        std::fs::write(dir.join("old.320.mp3"), vec![0_u8; 100]).expect("写老文件");
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(dir.join("new.320.mp3"), vec![0_u8; 100]).expect("写新文件");
        std::fs::write(dir.join("half.320.mp3.part"), vec![0_u8; 50]).expect("写半截文件");

        // 上限 150：总共有 200，得删掉最老的 100。
        let (deleted, freed) = sweep_dir(&dir, 150);
        assert_eq!(deleted, 2, "半截文件 + 最老的那个");
        assert_eq!(freed, 150);
        assert!(!dir.join("old.320.mp3").exists(), "最久没听的先走");
        assert!(dir.join("new.320.mp3").exists(), "最近听过的留下");
        assert!(!dir.join("half.320.mp3.part").exists(), "半截文件一律删");

        // 上限 0 = 缓存关掉：存货清光。
        let (deleted, _) = sweep_dir(&dir, 0);
        assert_eq!(deleted, 1, "关掉缓存就把存货也清了");
        assert_eq!(std::fs::read_dir(&dir).expect("列目录").count(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 歌词缓存：写出去再读回来一致；「没有歌词」也记一笔，不用反复问接口。
    #[test]
    fn lyrics_round_trip_through_the_cache() {
        use crate::model::LyricLine;
        use std::time::Duration;

        let dir = temp_dir("lyric");
        let lyric = Lyric {
            lines: vec![
                LyricLine {
                    at: Duration::from_millis(1000),
                    text: "第一句".to_string(),
                    translation: Some("first".to_string()),
                },
                LyricLine {
                    at: Duration::from_secs(62),
                    text: "第二句".to_string(),
                    translation: None,
                },
            ],
        };

        // 写之前没有缓存。
        assert!(read_lyric_in(&dir, "m1").is_none(), "没缓存过就是 None");
        put_lyric_in(&dir, "m1", Some(&lyric));
        let cached = read_lyric_in(&dir, "m1").expect("该有缓存").expect("有歌词");
        assert_eq!(cached.lines, lyric.lines, "写出去再读回来要一致");

        // 没有歌词：空文件当记号，读出来是 Some(None)。
        put_lyric_in(&dir, "m2", None);
        assert_eq!(read_lyric_in(&dir, "m2"), Some(None), "记住「这首没有歌词」");
        // 之后再写成有歌词的，记号要被覆盖，翻译文件也要跟着更新。
        put_lyric_in(&dir, "m2", Some(&lyric));
        assert!(read_lyric_in(&dir, "m2").expect("该有缓存").is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 缓存里已有哪些歌：认正式文件，不认 `.part`，也不认歌词目录；带档位与字节数。
    #[test]
    fn cached_entries_list_only_complete_tracks() {
        let dir = temp_dir("mids");
        std::fs::write(dir.join("aaa.320.mp3"), vec![0_u8; 10]).expect("写完整缓存");
        std::fs::write(dir.join("bbb.flac.flac.part"), vec![0_u8; 5]).expect("写半截");
        std::fs::create_dir_all(dir.join("lyrics")).expect("建歌词目录");
        std::fs::write(dir.join("lyrics/ccc.lrc"), "x").expect("写歌词");

        std::fs::write(dir.join("bbb.flac.flac.part"), vec![0_u8; 5]).expect("写半截");
        let entries = cached_entries_in(&dir);
        assert_eq!(entries.get("aaa"), Some(&(Quality::High320, 10)), "{entries:?}");
        assert!(!entries.contains_key("bbb"), "半截的不算：{entries:?}");
        assert!(!entries.contains_key("ccc"), "歌词目录不算：{entries:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 按 mid 找缓存：任意档位都能命中，返回的档位要准（mp3 也分 320/128）。
    #[test]
    fn for_song_finds_any_tier_and_reports_it() {
        let dir = temp_dir("for-song");
        std::fs::write(dir.join("m1.128.mp3"), vec![0_u8; 10]).expect("写缓存");
        std::fs::write(dir.join("m2.flac.flac"), vec![0_u8; 10]).expect("写缓存");
        std::fs::write(dir.join("m3.320.mp3.part"), vec![0_u8; 10]).expect("写半截");

        assert_eq!(
            for_song_in(&dir, "m1").map(|(_, quality)| quality),
            Some(Quality::Low128),
            "128k 的 mp3 不能被认成 320k"
        );
        assert_eq!(
            for_song_in(&dir, "m2").map(|(_, quality)| quality),
            Some(Quality::Flac)
        );
        assert!(for_song_in(&dir, "m3").is_none(), "半截不算");
        assert!(for_song_in(&dir, "m4").is_none(), "没有就是没有");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 上限没超就不动任何东西；目录不存在也不炸。
    #[test]
    fn sweeping_is_a_no_op_when_it_fits_or_the_directory_is_missing() {
        let dir = temp_dir("fits");
        std::fs::write(dir.join("a.320.mp3"), vec![0_u8; 100]).expect("写文件");
        assert_eq!(sweep_dir(&dir, 1 << 20), (0, 0));
        assert!(dir.join("a.320.mp3").exists());
        assert_eq!(sweep_dir(&dir.join("nope"), 1), (0, 0), "目录不在也不该炸");
        std::fs::remove_dir_all(&dir).ok();
    }
}

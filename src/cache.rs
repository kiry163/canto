//! 播放缓存：听过的歌留在本地，重听不再走网络。
//!
//! 规则只有一条：**从头顺序读完**的才算数。边播边写 `<mid>.<ext>.part`，写出去的部分
//! 必须连成一片、并且真的铺到文件尾，才改名成正式缓存；中途往前跳 / 往回拖留下洞的，
//! 这次就不进缓存（`.part` 直接删掉），免得留一堆谁也接不上的半截文件。
//!
//! 键是 `mid`（全局唯一，且不像 vkey 那样会过期）+ 扩展名：换了档位就是另一个文件。

use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::paths;

/// 缓存文件名（完整 / 临时）。
fn names_in(dir: &Path, mid: &str, ext: &str) -> (PathBuf, PathBuf) {
    let done = dir.join(format!("{mid}.{ext}"));
    let part = dir.join(format!("{mid}.{ext}.part"));
    (done, part)
}

/// 命中一份**完整**缓存。
pub fn complete(mid: &str, ext: &str) -> Option<PathBuf> {
    let (done, _) = names_in(&paths::cache_dir(), mid, ext);
    done.is_file().then_some(done)
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
        let at = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        kept.push((path, metadata.len(), at));
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

/// 边播边写盘的那一半（挂给 `MediaStream`）。
pub(crate) struct Write {
    file: File,
    part: PathBuf,
    done: PathBuf,
    /// 从 0 开始已经**连续**写到哪；洞之后的字节一律不写。
    prefix: u64,
    /// 收尾过了就不再删 `part`（正常完成或被判定不合格，两种都在 `finish` 里处理）。
    finished: bool,
}

impl Write {
    pub(crate) fn new(mid: &str, ext: &str) -> std::io::Result<Self> {
        Self::new_in(&paths::cache_dir(), mid, ext)
    }

    fn new_in(dir: &Path, mid: &str, ext: &str) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let (done, part) = names_in(dir, mid, ext);
        // 上一次留下的 `.part` 接不上，重来。
        let _ = std::fs::remove_file(&part);
        Ok(Self {
            file: File::create(&part)?,
            part,
            done,
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
            let _ = std::fs::rename(&self.part, &self.done);
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

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("canto-cache-{}-{tag}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    /// 顺序写完 → 认作完整：正式文件在，`.part` 不在。
    #[test]
    fn a_sequentially_read_track_becomes_a_complete_cache_file() {
        let dir = temp_dir("complete");
        let (done, part) = names_in(&dir, "mid1", "mp3");
        let mut write = Write::new_in(&dir, "mid1", "mp3").expect("建缓存文件");
        write.put(0, &[1, 2, 3]);
        write.put(3, &[4, 5]);
        write.finish(5, Some(5));

        assert!(done.is_file(), "顺序读完该留下完整缓存");
        assert!(!part.exists(), "临时文件该改名过去");
        assert_eq!(std::fs::read(&done).expect("读回"), vec![1, 2, 3, 4, 5]);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 跳着读（留洞）→ 不认：正式文件不出现，`.part` 也删掉。
    #[test]
    fn a_hole_in_the_middle_is_not_cached() {
        let dir = temp_dir("hole");
        let (done, part) = names_in(&dir, "mid2", "flac");
        let mut write = Write::new_in(&dir, "mid2", "flac").expect("建缓存文件");
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
        let (done, part) = names_in(&dir, "mid3", "mp3");
        {
            let mut write = Write::new_in(&dir, "mid3", "mp3").expect("建缓存文件");
            write.put(0, &[1, 2, 3, 4]);
        }
        assert!(!part.exists(), "没听完就不该留半截");
        assert!(!done.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 清理：先删 `.part`，再按最久没听的先删，压到上限以内。
    #[test]
    fn sweeping_drops_parts_and_the_oldest_files_first() {
        let dir = temp_dir("sweep");
        std::fs::write(dir.join("old.mp3"), vec![0_u8; 100]).expect("写老文件");
        std::fs::write(dir.join("new.mp3"), vec![0_u8; 100]).expect("写新文件");
        std::fs::write(dir.join("half.mp3.part"), vec![0_u8; 50]).expect("写半截文件");
        // 明确把 old 的时间调早，别依赖文件系统 mtime 的粗细。
        let old = File::options()
            .write(true)
            .open(dir.join("old.mp3"))
            .expect("打开老文件");
        old.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1000))
            .expect("改老文件时间");
        let new = File::options()
            .write(true)
            .open(dir.join("new.mp3"))
            .expect("打开新文件");
        new.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(2000))
            .expect("改新文件时间");

        // 上限 150：总共有 200，得删掉最老的 100。
        let (deleted, freed) = sweep_dir(&dir, 150);
        assert_eq!(deleted, 2, "半截文件 + 最老的那个");
        assert_eq!(freed, 150);
        assert!(!dir.join("old.mp3").exists(), "最久没听的先走");
        assert!(dir.join("new.mp3").exists(), "最近听过的留下");
        assert!(!dir.join("half.mp3.part").exists(), "半截文件一律删");

        // 上限 0 = 缓存关掉：存货清光。
        let (deleted, _) = sweep_dir(&dir, 0);
        assert_eq!(deleted, 1, "关掉缓存就把存货也清了");
        assert_eq!(std::fs::read_dir(&dir).expect("列目录").count(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 上限没超就不动任何东西；目录不存在也不炸。
    #[test]
    fn sweeping_is_a_no_op_when_it_fits_or_the_directory_is_missing() {
        let dir = temp_dir("fits");
        std::fs::write(dir.join("a.mp3"), vec![0_u8; 100]).expect("写文件");
        assert_eq!(sweep_dir(&dir, 1 << 20), (0, 0));
        assert!(dir.join("a.mp3").exists());
        assert_eq!(sweep_dir(&dir.join("nope"), 1), (0, 0), "目录不在也不该炸");
        std::fs::remove_dir_all(&dir).ok();
    }
}

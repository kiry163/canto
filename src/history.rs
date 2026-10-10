//! 播放历史：`<配置目录>/canto/history.json`。
//!
//! 只记「哪首、什么时候、从哪个队列来的」——够历史屏显示与重播就行。
//! 新条目插到最前，超上限从尾部丢：列表按时间倒序读，不用每次排序。

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// 一条播放记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub mid: String,
    pub title: String,
    pub singers: String,
    /// Unix 秒。
    pub at: u64,
    /// 当时队列的来源名（歌单名 / `搜索：晴天` / `历史`）。
    pub origin: String,
}

#[derive(Debug, Clone, Default)]
pub struct History {
    /// 新的在前。
    entries: Vec<Entry>,
}

impl History {
    /// 读历史；文件不在或读不动就当空历史（历史丢了不该阻塞启动）。
    pub fn load(path: &Path, limit: usize) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let Ok(mut entries) = serde_json::from_str::<Vec<Entry>>(&text) else {
            return Self::default();
        };
        entries.truncate(limit.max(1));
        Self { entries }
    }

    /// 记一条。与最上面那条同 mid 时只刷新时间戳——单曲循环否则会把历史刷成一条。
    pub fn push(&mut self, entry: Entry, limit: usize) {
        if let Some(first) = self.entries.first_mut()
            && first.mid == entry.mid
        {
            first.at = entry.at;
            return;
        }
        self.entries.insert(0, entry);
        self.entries.truncate(limit.max(1));
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 落盘（临时文件 + rename，中途失败不留坏文件）。
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建配置目录失败：{}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(&self.entries).context("序列化历史失败")?;
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, text).with_context(|| format!("写 {} 失败", temp.display()))?;
        std::fs::rename(&temp, path).with_context(|| format!("替换 {} 失败", path.display()))
    }

    /// 清空内存与磁盘。
    pub fn clear(&mut self, path: &Path) -> Result<()> {
        self.entries.clear();
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| format!("删除 {} 失败", path.display())),
        }
    }
}

/// 当前 Unix 秒。
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// `刚刚` / `12 分钟前` / `21:30` / `昨天 21:30` / `3月5日` / `2025年12月31日`。
pub fn relative(now: u64, at: u64) -> String {
    let delta = now.saturating_sub(at);
    if delta < 60 {
        return "刚刚".to_string();
    }
    if delta < 3600 {
        return format!("{} 分钟前", delta / 60);
    }
    let (now_tm, at_tm) = (local_tm(now), local_tm(at));
    let hhmm = format!("{:02}:{:02}", at_tm.tm_hour, at_tm.tm_min);
    if now_tm.tm_year == at_tm.tm_year {
        if now_tm.tm_yday == at_tm.tm_yday {
            return hhmm;
        }
        if now_tm.tm_yday - at_tm.tm_yday == 1 {
            return format!("昨天 {hhmm}");
        }
        return format!("{}月{}日", at_tm.tm_mon + 1, at_tm.tm_mday);
    }
    format!(
        "{}年{}月{}日",
        at_tm.tm_year + 1900,
        at_tm.tm_mon + 1,
        at_tm.tm_mday
    )
}

/// 本地时间（借 `libc`，不为一句 `HH:MM` 再拉一个时间库）。
fn local_tm(at: u64) -> libc::tm {
    let seconds = at as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: `localtime_r` 只写 `tm`，两个指针都指向本栈上的有效值。
    unsafe { libc::localtime_r(&seconds, &mut tm) };
    tm
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(mid: &str, at: u64) -> Entry {
        Entry {
            mid: mid.to_string(),
            title: format!("歌 {mid}"),
            singers: "歌手".to_string(),
            at,
            origin: "嗯".to_string(),
        }
    }

    fn temp_path(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("canto-history-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir.join("history.json")
    }

    #[test]
    fn keeps_newest_first_and_truncates_to_limit() {
        let mut history = History::default();
        for index in 0..5u64 {
            history.push(entry(&format!("m{index}"), 100 + index), 3);
        }
        let mids: Vec<_> = history.entries().iter().map(|e| e.mid.as_str()).collect();
        assert_eq!(mids, ["m4", "m3", "m2"], "新的在前且只留 3 条");
    }

    #[test]
    fn repeats_of_the_same_song_only_refresh_the_timestamp() {
        let mut history = History::default();
        history.push(entry("m1", 100), 10);
        history.push(entry("m2", 200), 10);
        history.push(entry("m2", 300), 10);
        assert_eq!(history.len(), 2, "连续同 mid 不该刷屏");
        assert_eq!(history.entries()[0].at, 300, "时间戳应被刷新");
    }

    #[test]
    fn round_trips_through_disk_and_tolerates_a_broken_file() {
        let path = temp_path("roundtrip");
        let mut history = History::default();
        history.push(entry("m1", 100), 10);
        history.save(&path).expect("落盘");
        let loaded = History::load(&path, 10);
        assert_eq!(loaded.entries(), history.entries());

        std::fs::write(&path, "{ 不是 JSON").expect("写坏文件");
        assert!(
            History::load(&path, 10).is_empty(),
            "坏文件当空历史，不阻塞启动"
        );
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn clear_empties_memory_and_disk() {
        let path = temp_path("clear");
        let mut history = History::default();
        history.push(entry("m1", 100), 10);
        history.save(&path).expect("落盘");
        history.clear(&path).expect("清空");
        assert!(history.is_empty());
        assert!(History::load(&path, 10).is_empty());
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn relative_time_reads_like_a_person_wrote_it() {
        let now = now_secs();
        assert_eq!(relative(now, now), "刚刚");
        assert_eq!(relative(now, now - 300), "5 分钟前");
        let text = relative(now, now - 2 * 24 * 3600);
        assert!(!text.is_empty() && text != "刚刚", "久远的记录给出日期：{text}");
    }
}

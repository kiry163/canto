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
    /// 本地曲目记下自己的路径（重播不再经过 vkey）；QQ 歌没有这一项。
    ///
    /// `serde(default)`：这个字段是后加的，老 `history.json` 没有它，读回来是 `None`。
    #[serde(default)]
    pub local_path: Option<std::path::PathBuf>,
    /// 时长（秒）。后加的字段：老记录没有 → 0 → 列表里写 `--:--`。
    #[serde(default)]
    pub duration_secs: u64,
    /// 起播那一刻接口报的各档位体积。后加的字段：老记录没有 → 全 0 → 体积 `—`。
    ///
    /// 存**原始档位表**而不是算好的那一档：配置改了之后列表会按新下限重算（跟别的屏一致）。
    #[serde(default)]
    pub sizes: crate::model::FileSizes,
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
    let hhmm = format!("{:02}:{:02}", at_tm.hour, at_tm.min);
    if now_tm.year == at_tm.year {
        let (now_day, at_day) = (day_of_year(&now_tm), day_of_year(&at_tm));
        if now_day == at_day {
            return hhmm;
        }
        if now_day - at_day == 1 {
            return format!("昨天 {hhmm}");
        }
        return format!("{}月{}日", at_tm.month, at_tm.mday);
    }
    format!("{}年{}月{}日", at_tm.year, at_tm.month, at_tm.mday)
}

/// 一年里的第几天（0 基）。
///
/// 自己算而不用系统的 `tm_yday`：Windows 的 `SYSTEMTIME` 里根本没有这个字段，
/// 与其两边各写一套，不如统一从年月日推。
fn day_of_year(tm: &LocalTm) -> i32 {
    /// 平年每月之前的天数（1 月 = 0）。
    const BEFORE: [i32; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let month = tm.month.clamp(1, 12);
    let leap = month >= 3 && leap_year(tm.year);
    BEFORE[(month - 1) as usize] + tm.mday - 1 + i32::from(leap)
}

/// 闰年：4 年一闰，100 年不闰，400 年再闰。
fn leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// 本地时间：只留排一句话要用的字段。
struct LocalTm {
    year: i32,  // 公元年
    month: i32, // 1-12
    mday: i32,  // 1-31
    hour: i32,
    min: i32,
}

/// Unix 借 `libc`，Windows 借 `FileTimeToSystemTime` + 时区换算；
/// 都不为一句 `HH:MM` 再拉一个时间库。
#[cfg(unix)]
fn local_tm(at: u64) -> LocalTm {
    let seconds = at as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: `localtime_r` 只写 `tm`，两个指针都指向本栈上的有效值。
    unsafe { libc::localtime_r(&seconds, &mut tm) };
    LocalTm {
        year: tm.tm_year + 1900,
        month: tm.tm_mon + 1,
        mday: tm.tm_mday,
        hour: tm.tm_hour,
        min: tm.tm_min,
    }
}

/// Windows 没有 `localtime_r`：Unix 秒 → UTC `FILETIME` → 按当前时区（含夏令时）
/// 折成 `SYSTEMTIME`，再取同样的几个字段。
#[cfg(windows)]
fn local_tm(at: u64) -> LocalTm {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};

    let (low, high) = filetime(at);
    let utc = FILETIME {
        dwLowDateTime: low,
        dwHighDateTime: high,
    };
    let mut utc_sys: SYSTEMTIME = unsafe { std::mem::zeroed() };
    let mut local: SYSTEMTIME = unsafe { std::mem::zeroed() };
    // SAFETY: 两个调用都只写传进去的 `SYSTEMTIME`，时区传 null = 用当前系统时区。
    // 时间超出范围时调用返回失败、`local` 保持全零，跟 Unix 那边 `localtime_r`
    // 失败留下一份空 `tm` 一样，只影响这一行文字。
    unsafe {
        FileTimeToSystemTime(&utc, &mut utc_sys);
        SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc_sys, &mut local);
    }
    LocalTm {
        year: local.wYear as i32,
        month: local.wMonth as i32,
        mday: local.wDay as i32,
        hour: local.wHour as i32,
        min: local.wMinute as i32,
    }
}

/// Unix 秒 → `FILETIME` 的两个 32 位半区。
///
/// 1601-01-01 到 1970-01-01 差 11644473600 秒，而 `FILETIME` 的单位是 100ns。
/// 只有 Windows 会调它，但算式本身跟平台无关，所以留在 `cfg` 外面——能被测试盯着。
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn filetime(at: u64) -> (u32, u32) {
    let ticks = ((at as u128 + 11_644_473_600) * 10_000_000).min(u64::MAX as u128) as u64;
    (ticks as u32, (ticks >> 32) as u32)
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
            local_path: None,
            duration_secs: 0,
            sizes: crate::model::FileSizes::default(),
        }
    }

    /// 新写的记录带着时长与各档位体积；读回来还在。
    #[test]
    fn an_entry_keeps_the_duration_and_sizes() {
        let sizes = crate::model::FileSizes { mp3_128: 4_400_000, ..Default::default() };
        let entry = Entry {
            mid: "a".to_string(),
            title: "成全".to_string(),
            singers: "刘若英".to_string(),
            at: 1,
            origin: "我的收藏".to_string(),
            local_path: None,
            duration_secs: 276,
            sizes,
        };
        let text = serde_json::to_string(&entry).expect("序列化");
        let back: Entry = serde_json::from_str(&text).expect("反序列化");
        assert_eq!(back.duration_secs, 276);
        assert_eq!(back.sizes.mp3_128, 4_400_000);
    }

    /// 升级前的老文件（没有这两个字段）照样读得出来：默认 0，列表里写 `--:--`/`—`。
    #[test]
    fn an_old_history_file_still_loads() {
        let old = r#"[{"mid":"a","title":"成全","singers":"刘若英","at":1,"origin":"我的收藏"}]"#;
        let entries: Vec<Entry> = serde_json::from_str(old).expect("老文件要读得出来");
        assert_eq!(entries[0].duration_secs, 0);
        assert_eq!(entries[0].sizes.mp3_128, 0);
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

    /// 「昨天」靠自己算的年内天数判断，闰年 2 月之后要整体多一天。
    #[test]
    fn day_of_year_counts_days_from_new_year() {
        let day = |year, month, mday| {
            day_of_year(&LocalTm {
                year,
                month,
                mday,
                hour: 0,
                min: 0,
            })
        };
        assert_eq!(day(2025, 1, 1), 0);
        assert_eq!(day(2025, 12, 31), 364);
        assert_eq!(day(2024, 3, 1), 60, "2024 闰年：3 月 1 日前有 31 + 29 天");
        assert_eq!(day(2025, 3, 1), 59, "2025 平年：3 月 1 日前有 31 + 28 天");
        assert_eq!(day(2000, 12, 31), 365, "400 年再闰：2000 是闰年");
        assert_eq!(day(1900, 3, 1), 59, "100 年不闰：1900 不是闰年");
    }

    /// `FILETIME` 的 64 位拆分：1970-01-01 在 Windows 纪元里是 0x019DB1DED53E8000。
    #[test]
    fn filetime_splits_a_timestamp_into_two_halves() {
        let halves = |at| {
            let (low, high) = filetime(at);
            (u64::from(high) << 32) | u64::from(low)
        };
        assert_eq!(halves(0), 0x019D_B1DE_D53E_8000, "1970-01-01 的 FILETIME");
        for at in [1_u64, 1_000_000_000, 1_700_000_000, u32::MAX as u64 + 7] {
            assert_eq!(halves(at) / 10_000_000 - 11_644_473_600, at, "at = {at}");
        }
    }
}

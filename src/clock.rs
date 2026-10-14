//! 本地时间：历史文案（「昨天 21:30」）与配置备份文件名都要**按使用者时区**的钟点。
//!
//! 两个平台各有一套系统调用（Unix 的 `localtime_r`、Windows 的 `FileTimeToSystemTime` +
//! 时区换算），都不为这点事再拉一个时间库；两边都收在这一个文件里，外面只认
//! [`now_secs`]、[`local_tm`]、[`stamp`]。

use std::time::{SystemTime, UNIX_EPOCH};

/// 本地时间：只留「排一句人话 / 拼一个文件名」要用的字段。
pub struct LocalTm {
    /// 公元年。
    pub year: i32,
    /// 1-12。
    pub month: i32,
    /// 1-31。
    pub mday: i32,
    pub hour: i32,
    pub min: i32,
    pub sec: i32,
}

/// 当前 Unix 秒。
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// `2026-10-14-153045`：拼备份文件名用。
///
/// 刻意**不带冒号**（Windows 的文件名不允许），也刻意用本地时间——备份就躺在使用者自己的目录里，
/// 名字该跟旁边的文件对得上表。
pub fn stamp(at: u64) -> String {
    let tm = local_tm(at);
    format!(
        "{:04}-{:02}-{:02}-{:02}{:02}{:02}",
        tm.year, tm.month, tm.mday, tm.hour, tm.min, tm.sec
    )
}

/// 一年里的第几天（0 基）。
///
/// 自己算而不用系统的 `tm_yday` / `wDayOfYear`：Windows 的 `SYSTEMTIME` 里根本没有这个字段，
/// 与其两边各写一套，不如统一从年月日推。
pub fn day_of_year(tm: &LocalTm) -> i32 {
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

/// Unix 秒 → 本地时间。
#[cfg(unix)]
pub fn local_tm(at: u64) -> LocalTm {
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
        sec: tm.tm_sec,
    }
}

/// Windows 没有 `localtime_r`：Unix 秒 → UTC `FILETIME` → 按当前时区（含夏令时）
/// 折成 `SYSTEMTIME`，再取同样的几个字段。
#[cfg(windows)]
pub fn local_tm(at: u64) -> LocalTm {
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
        sec: local.wSecond as i32,
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
                sec: 0,
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

    /// 备份名里的时间戳形状是固定的（`2026-10-14-153045`）——具体数字跟时区有关，
    /// 所以只钉形状：文件名里不能出现冒号。
    #[test]
    fn the_stamp_has_a_filename_safe_shape() {
        let text = stamp(1_700_000_000);
        let shape: Vec<char> = text
            .chars()
            .map(|c| if c.is_ascii_digit() { '9' } else { c })
            .collect();
        assert_eq!(
            shape,
            "9999-99-99-999999".chars().collect::<Vec<_>>(),
            "{text}"
        );
        assert!(!text.contains(':'), "Windows 文件名里不能有冒号：{text}");
        assert_eq!(text.len(), 17, "{text}");
    }

    /// `now_secs` 是「现在的秒」，不是 0（除非系统时钟真回到 1970）。
    #[test]
    fn now_secs_moves_forward() {
        assert!(now_secs() > 1_700_000_000, "系统时钟看起来不对");
    }
}

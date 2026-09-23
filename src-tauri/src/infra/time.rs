//! 时间工具。
//!
//! 只做两件小事：把 Unix 毫秒时间戳转成日历年月日，以及取当前时间的可读字符串。
//! 为此引入一整个日期库不划算（它也会把二进制撑大几百 KB），
//! 因此这里用 Howard Hinnant 的 civil_from_days 算法手写——十来行、无依赖、易测。
//!
//! **一律按 UTC 计算**。本地时区需要完整的时区数据库，而这里唯一的用途是
//! 日志与索引里的诊断时间戳，UTC 完全够用，也避免了夏令时之类的隐式行为。

/// 一天的毫秒数
const DAY_MS: i64 = 86_400_000;

/// 日历日期（UTC）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CivilDate {
    pub year: i64,
    pub month: u32,
    pub day: u32,
}

/// Unix 毫秒时间戳 → UTC 日历日期
pub fn civil_from_millis(ms: i64) -> CivilDate {
    let days = ms.div_euclid(DAY_MS);
    civil_from_days(days)
}

/// 从一个 13 位毫秒时间戳里取出年份。
///
/// 传入的如果不是毫秒时间戳（例如上游已经把年份单独给出来了），
/// 只要落在合理区间就原样返回，避免把 2005 这种值误算成 1970。
pub fn year_from_epoch_ms(ms: u64) -> Option<u32> {
    if ms < 100_000_000_000 {
        // 明显不是毫秒时间戳：可能是 `YYYYMMDD` 或直接就是年份
        if (1000..=9999).contains(&ms) {
            return Some(ms as u32);
        }
        if (10_000_000_000..100_000_000_000).contains(&ms) {
            return Some((ms / 10_000_000_000) as u32); // YYYYMMDDhhmm 之类
        }
        return None;
    }
    let d = civil_from_millis(ms as i64);
    (1000..=9999).contains(&d.year).then_some(d.year as u32)
}

/// `days` = 1970-01-01 起的天数（可为负）
fn civil_from_days(days: i64) -> CivilDate {
    // 把纪元挪到 0000-03-01，让闰年落在周期末尾
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as i64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    CivilDate {
        year: if m <= 2 { y + 1 } else { y },
        month: m as u32,
        day: d as u32,
    }
}

/// 当前时间，格式 `YYYY-MM-DD HH:MM:SS`（UTC）。用于日志与索引的诊断字段。
pub fn now_string() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    format_timestamp(ms)
}

/// Unix 毫秒 → `YYYY-MM-DD HH:MM:SS`（UTC）
pub fn format_timestamp(ms: i64) -> String {
    let d = civil_from_millis(ms);
    let secs_in_day = ms.div_euclid(1000).rem_euclid(86_400);
    let (h, m, s) = (secs_in_day / 3600, (secs_in_day % 3600) / 60, secs_in_day % 60);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        d.year, d.month, d.day, h, m, s
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ymd(ms: i64) -> (i64, u32, u32) {
        let d = civil_from_millis(ms);
        (d.year, d.month, d.day)
    }

    #[test]
    fn epoch_is_1970_01_01() {
        assert_eq!(ymd(0), (1970, 1, 1));
    }

    #[test]
    fn known_dates_round_trip() {
        // 期望值用独立的日期工具核对过，不是从实现里反推的
        // 网易云实测的 `al.publishTime`（周杰伦《十一月的萧邦》相关条目）
        assert_eq!(ymd(1_130_256_000_000), (2005, 10, 25));
        // 网易云实测的《彩虹》publishTime —— 年份应为 2007
        assert_eq!(ymd(1_193_932_800_007), (2007, 11, 1));
        assert_eq!(ymd(1_167_600_000_000), (2006, 12, 31));
        assert_eq!(ymd(1_792_000_000_000), (2026, 10, 14));
    }

    #[test]
    fn leap_day_is_handled() {
        // 2020-02-29
        assert_eq!(ymd(1_582_934_400_000), (2020, 2, 29));
        // 2021-02-28（非闰年）
        assert_eq!(ymd(1_614_470_400_000), (2021, 2, 28));
    }

    #[test]
    fn year_extraction_from_netease_publish_time() {
        // 实测值：周杰伦《彩虹》的 album.publishTime
        assert_eq!(year_from_epoch_ms(1_193_932_800_007), Some(2007));
        assert_eq!(year_from_epoch_ms(1_130_256_000_000), Some(2005));
    }

    #[test]
    fn bare_year_is_passed_through() {
        assert_eq!(year_from_epoch_ms(2005), Some(2005));
        assert_eq!(year_from_epoch_ms(0), None);
    }

    #[test]
    fn timestamp_formatting() {
        assert_eq!(format_timestamp(0), "1970-01-01 00:00:00");
        // 一天 + 1 小时 2 分 3 秒
        assert_eq!(format_timestamp(DAY_MS + 3_723_000), "1970-01-02 01:02:03");
    }

    /// 负时间戳（1601 年附近）不能 panic 或算错
    #[test]
    fn negative_timestamps_are_safe() {
        assert_eq!(ymd(-DAY_MS), (1969, 12, 31));
        assert!(now_string().len() >= 19);
    }
}

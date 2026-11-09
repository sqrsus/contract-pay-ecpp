use crate::error::{DomainError, Result};
use std::time::{SystemTime, UNIX_EPOCH};

/// 是否形如 `YYYY-MM-DD` 且日期真实存在（含闰年判断）。
pub fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    if !b
        .iter()
        .enumerate()
        .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
    {
        return false;
    }
    let y: i32 = s[0..4].parse().unwrap_or(0);
    let m: u32 = s[5..7].parse().unwrap_or(0);
    let d: u32 = s[8..10].parse().unwrap_or(0);
    if !(1..=12).contains(&m) || d == 0 {
        return false;
    }
    d <= days_in_month(y, m)
}

fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// 自然年上加年：`2024-05-10` + 2 → `2026-05-10`。
///
/// 2 月 29 日落到非闰年时归到 2 月 28 日，避免算出不存在的日期。
pub fn plus_years(date: &str, years: i32) -> Option<String> {
    if !is_iso_date(date) {
        return None;
    }
    let y: i32 = date[0..4].parse().ok()?;
    let m: u32 = date[5..7].parse().ok()?;
    let d: u32 = date[8..10].parse().ok()?;
    let ny = y + years;
    let nd = d.min(days_in_month(ny, m));
    Some(format!("{ny:04}-{m:02}-{nd:02}"))
}

/// `from` 到 `as_of` 是否已**严格超过** `years` 个自然年。
///
/// 例：`from = 2024-05-10`、`years = 2` 时，`as_of = 2026-05-10` 返回 false，
/// `as_of = 2026-05-11` 返回 true。ISO 日期可直接按字符串比较。
pub fn years_elapsed_over(from: &str, as_of: &str, years: i32) -> bool {
    match plus_years(from, years) {
        Some(limit) => is_iso_date(as_of) && as_of > limit.as_str(),
        None => false,
    }
}

pub fn validate_iso_date(s: &str) -> Result<()> {
    if is_iso_date(s) {
        Ok(())
    } else {
        Err(DomainError::DateFormat(s.to_string()))
    }
}

/// 可选日期：None 或合法日期。
pub fn validate_opt_date(s: Option<&str>) -> Result<()> {
    match s {
        Some(v) if !v.trim().is_empty() => validate_iso_date(v),
        _ => Ok(()),
    }
}

fn unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 由 Unix 天数还原公历年月日（Howard Hinnant civil_from_days）。
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}

/// 今天（UTC），`YYYY-MM-DD`。
pub fn today_iso() -> String {
    let secs = unix_secs();
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// 当前时点（UTC），`YYYY-MM-DDTHH:MM:SSZ`，仅用于审计列。
pub fn now_iso() -> String {
    let secs = unix_secs();
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// 本地时区（东八区）相对 UTC 的偏移秒数。
const LOCAL_OFFSET_SECS: i64 = 8 * 3600;

/// 当前本地时点（东八区），`YYYY-MM-DD HH:MM:SS`，供界面直接展示。
///
/// 审计列仍写 UTC 的 [`now_iso`]；这里只负责「显示」，与操作日志列表里
/// `datetime(created_at, '+8 hours')` 的换算口径保持一致，避免把差 8 小时的
/// UTC 时刻直接摆到用户面前。
pub fn now_local_text() -> String {
    local_text_from_unix(unix_secs())
}

fn local_text_from_unix(utc_secs: i64) -> String {
    let secs = utc_secs + LOCAL_OFFSET_SECS;
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// 期间键 `YYYY-MM`（报表按期间统计用）。
pub fn period_of(date: &str) -> String {
    date.get(0..7).unwrap_or("").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 日期校验() {
        assert!(is_iso_date("2026-11-04"));
        assert!(is_iso_date("2024-02-29"));
        assert!(!is_iso_date("2025-02-29"));
        assert!(!is_iso_date("2026-13-01"));
        assert!(!is_iso_date("2026-1-1"));
        assert!(!is_iso_date("2026/11/04"));
        assert!(validate_iso_date("2026-11-04").is_ok());
        assert!(validate_iso_date("2026-11-31").is_err());
    }

    #[test]
    fn 今天与期间键格式合法() {
        let t = today_iso();
        assert!(is_iso_date(&t), "today_iso 输出不合法: {t}");
        assert_eq!(period_of("2026-11-04"), "2026-11");
        assert_eq!(now_iso().len(), 20);
    }

    #[test]
    fn 公历还原基准点() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_000), (2022, 1, 8));
    }

    #[test]
    fn 本地时间按东八区换算且能正确跨日() {
        assert_eq!(local_text_from_unix(0), "1970-01-01 08:00:00");
        assert_eq!(local_text_from_unix(16 * 3600), "1970-01-02 00:00:00");
        assert_eq!(now_local_text().len(), 19);
    }

    #[test]
    fn 自然年加年与满年判定() {
        assert_eq!(plus_years("2024-05-10", 2).as_deref(), Some("2026-05-10"));
        // 闰日归到平年 2 月 28 日
        assert_eq!(plus_years("2024-02-29", 1).as_deref(), Some("2025-02-28"));
        assert_eq!(plus_years("2026-13-01", 2), None);

        assert!(years_elapsed_over("2024-05-10", "2026-05-11", 2));
        assert!(
            !years_elapsed_over("2024-05-10", "2026-05-10", 2),
            "同日不算超过"
        );
        assert!(!years_elapsed_over("2024-05-10", "2026-05-09", 2));
        // 开单日期早于交工日期（录错）按未满处理
        assert!(!years_elapsed_over("2026-05-10", "2026-01-01", 2));
    }
}

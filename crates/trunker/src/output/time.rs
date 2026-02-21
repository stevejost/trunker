//! UTC time formatting utilities without external datetime dependencies.
//!
//! Provides calendar conversion from `SystemTime` using Howard Hinnant's
//! algorithm. No `chrono` or `time` crate required.

use std::time::SystemTime;

/// Decompose a `SystemTime` into calendar components (UTC).
///
/// Returns `(year, month, day, hour, minute, second)`.
///
/// Returns an error if the timestamp is before the Unix epoch.
pub fn decompose_system_time(time: SystemTime) -> std::io::Result<(i32, u32, u32, u32, u32, u32)> {
    let duration = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let total_seconds = duration.as_secs();

    // Days since epoch and time of day.
    let days = (total_seconds / 86400) as i64;
    let time_of_day = total_seconds % 86400;
    let hour = (time_of_day / 3600) as u32;
    let minute = ((time_of_day % 3600) / 60) as u32;
    let second = (time_of_day % 60) as u32;

    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let (year, month, day) = civil_from_days(days);
    Ok((year, month, day, hour, minute, second))
}

/// Format a `SystemTime` as an ISO 8601 UTC timestamp string.
///
/// Returns a string in the format `"YYYY-MM-DDTHH:MM:SSZ"`.
///
/// Falls back to the Unix epoch if the timestamp is before it.
pub fn format_utc_timestamp(time: SystemTime) -> String {
    let (year, month, day, hour, minute, second) =
        decompose_system_time(time).unwrap_or((1970, 1, 1, 0, 0, 0));
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Convert days since 1970-01-01 to a civil date (year, month, day).
///
/// Uses Howard Hinnant's algorithm from chrono-free date conversion.
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64 + era * 400) as i32;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Create a SystemTime from a known UTC timestamp.
    fn system_time_from_epoch_secs(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn decompose_epoch() {
        let (y, m, d, h, min, s) = decompose_system_time(SystemTime::UNIX_EPOCH).unwrap();
        assert_eq!((y, m, d, h, min, s), (1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn decompose_known_date() {
        // 2026-02-20 15:30:45 UTC
        let ts = system_time_from_epoch_secs(1771601445);
        let (y, m, d, h, min, s) = decompose_system_time(ts).unwrap();
        assert_eq!((y, m, d, h, min, s), (2026, 2, 20, 15, 30, 45));
    }

    #[test]
    fn decompose_leap_year() {
        // 2024-02-29 12:00:00 UTC = 1709208000 epoch seconds
        let ts = system_time_from_epoch_secs(1709208000);
        let (y, m, d, _, _, _) = decompose_system_time(ts).unwrap();
        assert_eq!((y, m, d), (2024, 2, 29));
    }

    #[test]
    fn decompose_end_of_year() {
        // 2025-12-31 23:59:59 UTC = 1767225599 epoch seconds
        let ts = system_time_from_epoch_secs(1767225599);
        let (y, m, d, h, min, s) = decompose_system_time(ts).unwrap();
        assert_eq!((y, m, d, h, min, s), (2025, 12, 31, 23, 59, 59));
    }

    #[test]
    fn civil_from_days_epoch() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn civil_from_days_known_dates() {
        // 2000-01-01 is day 10957
        assert_eq!(civil_from_days(10957), (2000, 1, 1));
        // 2026-02-20 is day 20504
        assert_eq!(civil_from_days(20504), (2026, 2, 20));
    }

    #[test]
    fn format_utc_timestamp_epoch() {
        let ts = format_utc_timestamp(SystemTime::UNIX_EPOCH);
        assert_eq!(ts, "1970-01-01T00:00:00Z");
    }

    #[test]
    fn format_utc_timestamp_known_date() {
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(1771601445);
        let ts = format_utc_timestamp(time);
        assert_eq!(ts, "2026-02-20T15:30:45Z");
    }
}

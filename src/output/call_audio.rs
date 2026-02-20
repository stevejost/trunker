//! Output directory structure for per-call audio files.
//!
//! Generates file paths following the convention:
//! `{output_dir}/{date}/{talkgroup}_{timestamp}.wav`
//!
//! Directories are created on demand. No rotation or cleanup
//! (Unix philosophy: user manages disk).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::p25::types::TalkgroupId;

/// Generate a WAV file path for a call recording.
///
/// Path format: `{output_dir}/{YYYY-MM-DD}/{talkgroup}_{YYYYMMDD_HHMMSS}.wav`
///
/// Creates the date subdirectory if it does not exist.
pub fn call_audio_path(
    output_dir: &Path,
    talkgroup: TalkgroupId,
    timestamp: SystemTime,
) -> std::io::Result<PathBuf> {
    let (date_dir, filename) = format_path_components(talkgroup, timestamp);
    let dir = output_dir.join(&date_dir);
    fs::create_dir_all(&dir)?;
    Ok(dir.join(filename))
}

/// Format the date subdirectory and filename without touching the filesystem.
///
/// Returns `(date_dir, filename)` where:
/// - `date_dir` is `"YYYY-MM-DD"`
/// - `filename` is `"{talkgroup}_{YYYYMMDD_HHMMSS}.wav"`
fn format_path_components(talkgroup: TalkgroupId, timestamp: SystemTime) -> (String, String) {
    let (year, month, day, hour, minute, second) = decompose_system_time(timestamp);
    let date_dir = format!("{year:04}-{month:02}-{day:02}");
    let filename = format!(
        "{tg}_{year:04}{month:02}{day:02}_{hour:02}{minute:02}{second:02}.wav",
        tg = talkgroup.value(),
    );
    (date_dir, filename)
}

/// Decompose a `SystemTime` into calendar components (UTC).
///
/// Returns `(year, month, day, hour, minute, second)`.
fn decompose_system_time(time: SystemTime) -> (i32, u32, u32, u32, u32, u32) {
    let duration = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let total_seconds = duration.as_secs();

    // Days since epoch and time of day.
    let days = (total_seconds / 86400) as i64;
    let time_of_day = total_seconds % 86400;
    let hour = (time_of_day / 3600) as u32;
    let minute = ((time_of_day % 3600) / 60) as u32;
    let second = (time_of_day % 60) as u32;

    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let (year, month, day) = civil_from_days(days);
    (year, month, day, hour, minute, second)
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
    fn format_components_epoch() {
        // 1970-01-01 00:00:00 UTC
        let ts = system_time_from_epoch_secs(0);
        let (date_dir, filename) = format_path_components(TalkgroupId::new(100), ts);
        assert_eq!(date_dir, "1970-01-01");
        assert_eq!(filename, "100_19700101_000000.wav");
    }

    #[test]
    fn format_components_known_timestamp() {
        // 2026-02-20 15:30:45 UTC = 1771601445 epoch seconds
        let ts = system_time_from_epoch_secs(1771601445);
        let (date_dir, filename) = format_path_components(TalkgroupId::new(42), ts);
        assert_eq!(date_dir, "2026-02-20");
        assert_eq!(filename, "42_20260220_153045.wav");
    }

    #[test]
    fn format_components_large_talkgroup() {
        let ts = system_time_from_epoch_secs(1771601445);
        let (_, filename) = format_path_components(TalkgroupId::new(65535), ts);
        assert_eq!(filename, "65535_20260220_153045.wav");
    }

    #[test]
    fn call_audio_path_creates_directory() {
        let dir = tempfile::tempdir().unwrap();
        let ts = system_time_from_epoch_secs(1771601445);
        let path = call_audio_path(dir.path(), TalkgroupId::new(100), ts).unwrap();

        assert!(
            path.parent().unwrap().exists(),
            "date directory should exist"
        );
        assert_eq!(path.file_name().unwrap(), "100_20260220_153045.wav");
        assert!(
            path.parent().unwrap().ends_with("2026-02-20"),
            "path should contain date subdirectory"
        );
    }

    #[test]
    fn call_audio_path_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let ts = system_time_from_epoch_secs(1771601445);

        // Calling twice should not fail (directory already exists).
        let path1 = call_audio_path(dir.path(), TalkgroupId::new(100), ts).unwrap();
        let path2 = call_audio_path(dir.path(), TalkgroupId::new(200), ts).unwrap();

        assert_eq!(path1.parent(), path2.parent());
        assert_ne!(path1.file_name(), path2.file_name());
    }

    #[test]
    fn decompose_epoch() {
        let (y, m, d, h, min, s) = decompose_system_time(SystemTime::UNIX_EPOCH);
        assert_eq!((y, m, d, h, min, s), (1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn decompose_known_date() {
        // 2026-02-20 15:30:45 UTC
        let ts = system_time_from_epoch_secs(1771601445);
        let (y, m, d, h, min, s) = decompose_system_time(ts);
        assert_eq!((y, m, d, h, min, s), (2026, 2, 20, 15, 30, 45));
    }

    #[test]
    fn decompose_leap_year() {
        // 2024-02-29 12:00:00 UTC = 1709208000 epoch seconds
        let ts = system_time_from_epoch_secs(1709208000);
        let (y, m, d, _, _, _) = decompose_system_time(ts);
        assert_eq!((y, m, d), (2024, 2, 29));
    }

    #[test]
    fn decompose_end_of_year() {
        // 2025-12-31 23:59:59 UTC = 1767225599 epoch seconds
        let ts = system_time_from_epoch_secs(1767225599);
        let (y, m, d, h, min, s) = decompose_system_time(ts);
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
}

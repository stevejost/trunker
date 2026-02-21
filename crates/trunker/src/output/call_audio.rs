//! Output directory structure for per-call audio files.
//!
//! Generates file paths following the convention:
//! `{output_dir}/{date}/{talkgroup}_{timestamp}.{ext}`
//!
//! Directories are created on demand. No rotation or cleanup
//! (Unix philosophy: user manages disk).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::time::decompose_system_time;
use crate::p25::types::TalkgroupId;

/// Generate an audio file path for a call recording.
///
/// Path format: `{output_dir}/{YYYY-MM-DD}/{talkgroup}_{YYYYMMDD_HHMMSS}.{extension}`
///
/// Creates the date subdirectory if it does not exist.
pub fn call_audio_path(
    output_dir: &Path,
    talkgroup: TalkgroupId,
    timestamp: SystemTime,
    extension: &str,
) -> std::io::Result<PathBuf> {
    let (date_dir, filename) = format_path_components(talkgroup, timestamp, extension)?;
    let dir = output_dir.join(&date_dir);
    fs::create_dir_all(&dir)?;
    Ok(dir.join(filename))
}

/// Format the date subdirectory and filename without touching the filesystem.
///
/// Returns `(date_dir, filename)` where:
/// - `date_dir` is `"YYYY-MM-DD"`
/// - `filename` is `"{talkgroup}_{YYYYMMDD_HHMMSS}.{extension}"`
///
/// Returns an error if the timestamp is before the Unix epoch.
fn format_path_components(
    talkgroup: TalkgroupId,
    timestamp: SystemTime,
    extension: &str,
) -> std::io::Result<(String, String)> {
    let (year, month, day, hour, minute, second) = decompose_system_time(timestamp)?;
    let date_dir = format!("{year:04}-{month:02}-{day:02}");
    let filename = format!(
        "{tg}_{year:04}{month:02}{day:02}_{hour:02}{minute:02}{second:02}.{extension}",
        tg = talkgroup.value(),
    );
    Ok((date_dir, filename))
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
        let (date_dir, filename) =
            format_path_components(TalkgroupId::new(100), ts, "wav").unwrap();
        assert_eq!(date_dir, "1970-01-01");
        assert_eq!(filename, "100_19700101_000000.wav");
    }

    #[test]
    fn format_components_known_timestamp() {
        // 2026-02-20 15:30:45 UTC = 1771601445 epoch seconds
        let ts = system_time_from_epoch_secs(1771601445);
        let (date_dir, filename) = format_path_components(TalkgroupId::new(42), ts, "wav").unwrap();
        assert_eq!(date_dir, "2026-02-20");
        assert_eq!(filename, "42_20260220_153045.wav");
    }

    #[test]
    fn format_components_large_talkgroup() {
        let ts = system_time_from_epoch_secs(1771601445);
        let (_, filename) = format_path_components(TalkgroupId::new(65535), ts, "wav").unwrap();
        assert_eq!(filename, "65535_20260220_153045.wav");
    }

    #[test]
    fn format_components_opus_extension() {
        let ts = system_time_from_epoch_secs(1771601445);
        let (_, filename) = format_path_components(TalkgroupId::new(100), ts, "opus").unwrap();
        assert_eq!(filename, "100_20260220_153045.opus");
    }

    #[test]
    fn call_audio_path_creates_directory() {
        let dir = tempfile::tempdir().unwrap();
        let ts = system_time_from_epoch_secs(1771601445);
        let path = call_audio_path(dir.path(), TalkgroupId::new(100), ts, "wav").unwrap();

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
    fn call_audio_path_opus_extension() {
        let dir = tempfile::tempdir().unwrap();
        let ts = system_time_from_epoch_secs(1771601445);
        let path = call_audio_path(dir.path(), TalkgroupId::new(100), ts, "opus").unwrap();

        assert_eq!(path.file_name().unwrap(), "100_20260220_153045.opus");
        assert!(
            path.extension().is_some_and(|ext| ext == "opus"),
            "path should have .opus extension"
        );
    }

    #[test]
    fn call_audio_path_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let ts = system_time_from_epoch_secs(1771601445);

        // Calling twice should not fail (directory already exists).
        let path1 = call_audio_path(dir.path(), TalkgroupId::new(100), ts, "wav").unwrap();
        let path2 = call_audio_path(dir.path(), TalkgroupId::new(200), ts, "wav").unwrap();

        assert_eq!(path1.parent(), path2.parent());
        assert_ne!(path1.file_name(), path2.file_name());
    }
}

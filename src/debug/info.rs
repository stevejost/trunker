//! File inspection action for the debug subcommand.
//!
//! Computes signal statistics (RMS, peak, DC offset) and file metadata
//! (sample count, duration) from an IQ sample file.

use std::io::Write;
use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use super::{FileInput, IqFormat, open_debug_source};

/// Number of bytes per complex sample for each format.
const BYTES_PER_SAMPLE_CF32: u64 = 8;
const BYTES_PER_SAMPLE_U8: u64 = 2;

/// File metadata and signal statistics, serialized as JSON output.
#[derive(Debug, Serialize)]
pub struct FileInfo {
    /// Path to the IQ file.
    pub path: String,
    /// IQ sample format name.
    pub format: &'static str,
    /// File size in bytes.
    pub file_size: u64,
    /// Number of complex IQ samples.
    pub sample_count: u64,
    /// Sample rate in hertz.
    pub sample_rate: u32,
    /// Duration in seconds.
    pub duration_seconds: f64,
    /// RMS signal level (magnitude of complex samples).
    pub rms: f64,
    /// Peak absolute signal level.
    pub peak: f64,
    /// DC offset on the I (real) channel.
    pub dc_offset_i: f64,
    /// DC offset on the Q (imaginary) channel.
    pub dc_offset_q: f64,
}

/// Run the info action: compute file stats and write JSON to `output`.
pub fn run(file: FileInput, output: &mut impl Write) -> Result<()> {
    let path = Path::new(&file.input);
    let file_size = std::fs::metadata(path)?.len();

    let bytes_per_sample = match file.format {
        IqFormat::Cf32 => BYTES_PER_SAMPLE_CF32,
        IqFormat::U8 => BYTES_PER_SAMPLE_U8,
    };
    let sample_count = file_size / bytes_per_sample;
    let duration_seconds = if file.sample_rate > 0 {
        sample_count as f64 / file.sample_rate as f64
    } else {
        0.0
    };

    let format_name = match file.format {
        IqFormat::Cf32 => "cf32",
        IqFormat::U8 => "u8",
    };

    let stats = compute_signal_stats(&file)?;

    let info = FileInfo {
        path: file.input.clone(),
        format: format_name,
        file_size,
        sample_count,
        sample_rate: file.sample_rate,
        duration_seconds,
        rms: stats.rms,
        peak: stats.peak,
        dc_offset_i: stats.dc_offset_i,
        dc_offset_q: stats.dc_offset_q,
    };

    let json = serde_json::to_string_pretty(&info)?;
    writeln!(output, "{json}")?;
    Ok(())
}

/// Intermediate signal statistics computed from IQ samples.
struct SignalStats {
    rms: f64,
    peak: f64,
    dc_offset_i: f64,
    dc_offset_q: f64,
}

/// Compute signal statistics by streaming through all samples.
fn compute_signal_stats(file: &FileInput) -> Result<SignalStats> {
    let source = open_debug_source(file)?;

    let mut count: u64 = 0;
    let mut sum_sq: f64 = 0.0;
    let mut peak: f64 = 0.0;
    let mut sum_i: f64 = 0.0;
    let mut sum_q: f64 = 0.0;

    for sample in source {
        let re = sample.re as f64;
        let im = sample.im as f64;

        sum_sq += re * re + im * im;
        let magnitude = (re * re + im * im).sqrt();
        if magnitude > peak {
            peak = magnitude;
        }
        sum_i += re;
        sum_q += im;
        count += 1;
    }

    if count == 0 {
        return Ok(SignalStats {
            rms: 0.0,
            peak: 0.0,
            dc_offset_i: 0.0,
            dc_offset_q: 0.0,
        });
    }

    let rms = (sum_sq / count as f64).sqrt();
    let dc_offset_i = sum_i / count as f64;
    let dc_offset_q = sum_q / count as f64;

    Ok(SignalStats {
        rms,
        peak,
        dc_offset_i,
        dc_offset_q,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use num_complex::Complex;
    use std::io::Write as IoWrite;
    use tempfile::NamedTempFile;

    fn write_cf32_file(samples: &[Complex<f32>]) -> NamedTempFile {
        let mut file = NamedTempFile::new().expect("create temp file");
        for s in samples {
            file.write_all(&s.re.to_ne_bytes()).expect("write real");
            file.write_all(&s.im.to_ne_bytes()).expect("write imag");
        }
        file.flush().expect("flush");
        file
    }

    fn write_u8_file(byte_pairs: &[[u8; 2]]) -> NamedTempFile {
        let mut file = NamedTempFile::new().expect("create temp file");
        for pair in byte_pairs {
            file.write_all(pair).expect("write pair");
        }
        file.flush().expect("flush");
        file
    }

    fn run_info_to_string(file: FileInput) -> String {
        let mut output = Vec::new();
        run(file, &mut output).expect("info should succeed");
        String::from_utf8(output).expect("valid UTF-8")
    }

    #[test]
    fn sample_count_cf32() {
        let samples = vec![Complex::new(1.0, 0.0); 100];
        let temp = write_cf32_file(&samples);

        let file_input = FileInput {
            input: temp.path().to_str().unwrap().to_string(),
            format: IqFormat::Cf32,
            sample_rate: 48_000,
        };

        let json_str = run_info_to_string(file_input);
        let v: serde_json::Value = serde_json::from_str(json_str.trim()).unwrap();

        assert_eq!(v["sample_count"], 100);
        assert_eq!(v["file_size"], 800); // 100 * 8 bytes
    }

    #[test]
    fn sample_count_u8() {
        let pairs: Vec<[u8; 2]> = vec![[128, 128]; 50];
        let temp = write_u8_file(&pairs);

        let file_input = FileInput {
            input: temp.path().to_str().unwrap().to_string(),
            format: IqFormat::U8,
            sample_rate: 48_000,
        };

        let json_str = run_info_to_string(file_input);
        let v: serde_json::Value = serde_json::from_str(json_str.trim()).unwrap();

        assert_eq!(v["sample_count"], 50);
        assert_eq!(v["file_size"], 100); // 50 * 2 bytes
    }

    #[test]
    fn duration_calculation() {
        let samples = vec![Complex::new(0.0, 0.0); 48_000];
        let temp = write_cf32_file(&samples);

        let file_input = FileInput {
            input: temp.path().to_str().unwrap().to_string(),
            format: IqFormat::Cf32,
            sample_rate: 48_000,
        };

        let json_str = run_info_to_string(file_input);
        let v: serde_json::Value = serde_json::from_str(json_str.trim()).unwrap();

        let duration = v["duration_seconds"].as_f64().unwrap();
        assert!(
            (duration - 1.0).abs() < 1e-9,
            "48000 samples at 48000 Hz = 1.0 second, got {duration}"
        );
    }

    #[test]
    fn rms_and_peak_known_signal() {
        // Constant I=0.5, Q=0.0 for 100 samples.
        // RMS = sqrt(mean(0.5^2 + 0^2)) = sqrt(0.25) = 0.5
        // Peak magnitude = sqrt(0.5^2 + 0^2) = 0.5
        let samples = vec![Complex::new(0.5, 0.0); 100];
        let temp = write_cf32_file(&samples);

        let file_input = FileInput {
            input: temp.path().to_str().unwrap().to_string(),
            format: IqFormat::Cf32,
            sample_rate: 48_000,
        };

        let json_str = run_info_to_string(file_input);
        let v: serde_json::Value = serde_json::from_str(json_str.trim()).unwrap();

        let rms = v["rms"].as_f64().unwrap();
        assert!((rms - 0.5).abs() < 1e-6, "RMS should be 0.5, got {rms}");

        let peak = v["peak"].as_f64().unwrap();
        assert!((peak - 0.5).abs() < 1e-6, "peak should be 0.5, got {peak}");
    }

    #[test]
    fn dc_offset_known_signal() {
        // I=0.3, Q=-0.7 for all samples.
        // DC offset I = 0.3, DC offset Q = -0.7
        let samples = vec![Complex::new(0.3, -0.7); 200];
        let temp = write_cf32_file(&samples);

        let file_input = FileInput {
            input: temp.path().to_str().unwrap().to_string(),
            format: IqFormat::Cf32,
            sample_rate: 48_000,
        };

        let json_str = run_info_to_string(file_input);
        let v: serde_json::Value = serde_json::from_str(json_str.trim()).unwrap();

        let dc_i = v["dc_offset_i"].as_f64().unwrap();
        assert!(
            (dc_i - 0.3).abs() < 1e-5,
            "DC offset I should be 0.3, got {dc_i}"
        );

        let dc_q = v["dc_offset_q"].as_f64().unwrap();
        assert!(
            (dc_q - (-0.7)).abs() < 1e-5,
            "DC offset Q should be -0.7, got {dc_q}"
        );
    }

    #[test]
    fn peak_with_varying_amplitudes() {
        // Mix of small and large samples; peak should be the largest magnitude.
        let samples = vec![
            Complex::new(0.1, 0.0),
            Complex::new(0.0, 0.9), // magnitude = 0.9
            Complex::new(0.6, 0.8), // magnitude = 1.0
            Complex::new(-0.1, 0.0),
        ];
        let temp = write_cf32_file(&samples);

        let file_input = FileInput {
            input: temp.path().to_str().unwrap().to_string(),
            format: IqFormat::Cf32,
            sample_rate: 48_000,
        };

        let json_str = run_info_to_string(file_input);
        let v: serde_json::Value = serde_json::from_str(json_str.trim()).unwrap();

        let peak = v["peak"].as_f64().unwrap();
        assert!(
            (peak - 1.0).abs() < 1e-5,
            "peak should be 1.0 (from 0.6+0.8j), got {peak}"
        );
    }

    #[test]
    fn json_contains_all_fields() {
        let samples = vec![Complex::new(0.5, -0.5); 10];
        let temp = write_cf32_file(&samples);

        let file_input = FileInput {
            input: temp.path().to_str().unwrap().to_string(),
            format: IqFormat::Cf32,
            sample_rate: 2_400_000,
        };

        let json_str = run_info_to_string(file_input);
        let v: serde_json::Value = serde_json::from_str(json_str.trim()).unwrap();

        assert!(v["path"].is_string(), "path should be a string");
        assert!(v["format"].is_string(), "format should be a string");
        assert!(v["file_size"].is_u64(), "file_size should be a number");
        assert!(
            v["sample_count"].is_u64(),
            "sample_count should be a number"
        );
        assert!(v["sample_rate"].is_u64(), "sample_rate should be a number");
        assert!(
            v["duration_seconds"].is_f64(),
            "duration_seconds should be a number"
        );
        assert!(v["rms"].is_f64(), "rms should be a number");
        assert!(v["peak"].is_f64(), "peak should be a number");
        assert!(v["dc_offset_i"].is_f64(), "dc_offset_i should be a number");
        assert!(v["dc_offset_q"].is_f64(), "dc_offset_q should be a number");

        assert_eq!(v["format"], "cf32");
        assert_eq!(v["sample_rate"], 2_400_000);
    }

    #[test]
    fn empty_file_returns_zeros() {
        let temp = write_cf32_file(&[]);

        let file_input = FileInput {
            input: temp.path().to_str().unwrap().to_string(),
            format: IqFormat::Cf32,
            sample_rate: 48_000,
        };

        let json_str = run_info_to_string(file_input);
        let v: serde_json::Value = serde_json::from_str(json_str.trim()).unwrap();

        assert_eq!(v["sample_count"], 0);
        assert_eq!(v["file_size"], 0);
        let duration = v["duration_seconds"].as_f64().unwrap();
        assert!((duration).abs() < 1e-9, "duration should be 0.0");
        let rms = v["rms"].as_f64().unwrap();
        assert!((rms).abs() < 1e-9, "rms should be 0.0");
        let peak = v["peak"].as_f64().unwrap();
        assert!((peak).abs() < 1e-9, "peak should be 0.0");
        let dc_i = v["dc_offset_i"].as_f64().unwrap();
        assert!((dc_i).abs() < 1e-9, "dc_offset_i should be 0.0");
        let dc_q = v["dc_offset_q"].as_f64().unwrap();
        assert!((dc_q).abs() < 1e-9, "dc_offset_q should be 0.0");
    }
}

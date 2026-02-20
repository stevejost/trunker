//! Channel extraction action for the debug subcommand.
//!
//! Shifts a channel to baseband with an NCO, applies a decimating low-pass
//! filter chain, optionally normalizes to 0.5 RMS, and writes CF32 output.
//!
//! The input sample rate must be an integer multiple of the target output
//! rate. For example, 2400000 Hz decimates cleanly to 24000 Hz (100x) or
//! 48000 Hz (50x), but not to 44100 Hz. The error message suggests the
//! nearest valid rates when a non-divisible rate is requested.

use std::io::Write;
use std::path::Path;

use anyhow::Result;
use num_complex::Complex;

use crate::dsp::filter::DecimatingFilter;
use crate::dsp::nco::Nco;
use crate::pipeline::DecimationConfig;

use super::{FileInput, open_debug_source};

/// Target RMS level for normalized output.
const TARGET_RMS: f64 = 0.5;

/// Run the filter action: NCO shift + LPF + decimate, write CF32 output.
///
/// By default, the output is normalized to 0.5 RMS so downstream CQPSK
/// demodulation (Costas loop, Gardner timing) works with calibrated levels.
/// Pass `no_normalize = true` to disable this and write raw filter output.
///
/// Uses a two-pass approach when normalization is enabled: first pass
/// collects samples into memory and computes RMS, second pass scales
/// and writes the output file.
pub fn run(
    file: FileInput,
    offset: f64,
    output_rate: u32,
    output_path: &str,
    no_normalize: bool,
) -> Result<()> {
    let source = open_debug_source(&file)?;
    let decimation = DecimationConfig::compute_to(file.sample_rate, output_rate)?;

    let mut filters: Vec<DecimatingFilter> = decimation
        .stages
        .iter()
        .map(|stage| {
            DecimatingFilter::new(
                stage.cutoff_hz,
                stage.input_rate,
                stage.num_taps,
                stage.decimation_factor,
            )
        })
        .collect();

    let mut nco = if offset != 0.0 {
        Some(Nco::new(offset, file.sample_rate as f64))
    } else {
        None
    };

    let mut output_samples: Vec<Complex<f32>> = Vec::new();

    for iq_sample in source {
        let shifted = match nco.as_mut() {
            Some(nco) => nco.shift(iq_sample),
            None => iq_sample,
        };

        let mut current = shifted;
        let mut passed = true;
        for filter in &mut filters {
            match filter.process(current) {
                Some(out) => current = out,
                None => {
                    passed = false;
                    break;
                }
            }
        }

        if passed {
            output_samples.push(current);
        }
    }

    if !no_normalize {
        normalize_rms(&mut output_samples);
    }

    write_cf32(Path::new(output_path), &output_samples)?;

    Ok(())
}

/// Scale samples so the RMS magnitude equals [`TARGET_RMS`].
///
/// Does nothing if the signal has zero energy (avoids division by zero).
fn normalize_rms(samples: &mut [Complex<f32>]) {
    if samples.is_empty() {
        return;
    }

    let sum_sq: f64 = samples
        .iter()
        .map(|s| {
            let re = s.re as f64;
            let im = s.im as f64;
            re * re + im * im
        })
        .sum();

    let rms = (sum_sq / samples.len() as f64).sqrt();
    if rms < 1e-12 {
        return;
    }

    let scale = (TARGET_RMS / rms) as f32;
    for s in samples.iter_mut() {
        s.re *= scale;
        s.im *= scale;
    }
}

/// Write complex samples as interleaved f32 pairs (CF32 format).
fn write_cf32(path: &Path, samples: &[Complex<f32>]) -> Result<()> {
    let file = std::fs::File::create(path)?;
    let mut writer = std::io::BufWriter::new(file);

    for s in samples {
        writer.write_all(&s.re.to_ne_bytes())?;
        writer.write_all(&s.im.to_ne_bytes())?;
    }

    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::debug::{FileInput, IqFormat};
    use crate::sdr::cf32_reader::Cf32Reader;
    use num_complex::Complex;
    use std::f32::consts::PI;
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

    fn compute_rms(samples: &[Complex<f32>]) -> f64 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum_sq: f64 = samples
            .iter()
            .map(|s| {
                let re = s.re as f64;
                let im = s.im as f64;
                re * re + im * im
            })
            .sum();
        (sum_sq / samples.len() as f64).sqrt()
    }

    #[test]
    fn decimation_produces_correct_sample_count() {
        // 48 kHz -> 24 kHz = 2x decimation, 48000 input -> 24000 output.
        let num_input = 48_000;
        let samples: Vec<Complex<f32>> = (0..num_input)
            .map(|i| {
                let t = i as f32 / 48_000.0;
                Complex::new((2.0 * PI * 1000.0 * t).cos() * 0.5, 0.0)
            })
            .collect();
        let temp = write_cf32_file(&samples);
        let dir = tempfile::tempdir().expect("create temp dir");
        let out_path = dir.path().join("out.cf32");

        let file = FileInput {
            input: temp.path().to_str().unwrap().to_string(),
            format: IqFormat::Cf32,
            sample_rate: 48_000,
        };

        run(file, 0.0, 24_000, out_path.to_str().unwrap(), true).unwrap();

        let reader = Cf32Reader::open(&out_path, 24_000).unwrap();
        let output: Vec<_> = reader.collect();
        assert_eq!(output.len(), num_input / 2);
    }

    #[test]
    fn output_cf32_is_readable_round_trip() {
        // Write known DC samples, filter at same rate (factor-1 LPF), read back.
        // The LPF has a ~25-sample transient; steady-state should pass DC.
        let samples = vec![Complex::new(0.3, -0.4); 1000];
        let temp = write_cf32_file(&samples);
        let dir = tempfile::tempdir().expect("create temp dir");
        let out_path = dir.path().join("out.cf32");

        let file = FileInput {
            input: temp.path().to_str().unwrap().to_string(),
            format: IqFormat::Cf32,
            sample_rate: 24_000,
        };

        run(file, 0.0, 24_000, out_path.to_str().unwrap(), true).unwrap();

        let reader = Cf32Reader::open(&out_path, 24_000).unwrap();
        let output: Vec<_> = reader.collect();
        assert_eq!(output.len(), 1000);
        // Skip filter transient, check steady-state DC passes through.
        for (i, s) in output.iter().enumerate().skip(60) {
            assert!(
                (s.re - 0.3).abs() < 1e-3,
                "sample {i} real mismatch: {}",
                s.re
            );
            assert!(
                (s.im - (-0.4)).abs() < 1e-3,
                "sample {i} imag mismatch: {}",
                s.im
            );
        }
    }

    #[test]
    fn normalization_sets_rms_to_target() {
        // Input at 0.1 RMS should be scaled to 0.5 RMS.
        let samples: Vec<Complex<f32>> = (0..48_000)
            .map(|i| {
                let t = i as f32 / 48_000.0;
                Complex::new((2.0 * PI * 1000.0 * t).cos() * 0.1, 0.0)
            })
            .collect();
        let temp = write_cf32_file(&samples);
        let dir = tempfile::tempdir().expect("create temp dir");
        let out_path = dir.path().join("normalized.cf32");

        let file = FileInput {
            input: temp.path().to_str().unwrap().to_string(),
            format: IqFormat::Cf32,
            sample_rate: 48_000,
        };

        // With normalization enabled (no_normalize = false).
        run(file, 0.0, 24_000, out_path.to_str().unwrap(), false).unwrap();

        let reader = Cf32Reader::open(&out_path, 24_000).unwrap();
        let output: Vec<_> = reader.collect();
        // Skip filter transient, measure steady state.
        let steady = &output[output.len() / 2..];
        let rms = compute_rms(steady);
        assert!(
            (rms - 0.5).abs() < 0.05,
            "normalized RMS should be ~0.5, got {rms}"
        );
    }

    #[test]
    fn no_normalize_preserves_original_level() {
        // Passthrough at same rate, no_normalize: output should match input RMS.
        let amplitude = 0.3;
        let samples: Vec<Complex<f32>> = vec![Complex::new(amplitude, 0.0); 1000];
        let input_rms = compute_rms(&samples);
        let temp = write_cf32_file(&samples);
        let dir = tempfile::tempdir().expect("create temp dir");
        let out_path = dir.path().join("raw.cf32");

        let file = FileInput {
            input: temp.path().to_str().unwrap().to_string(),
            format: IqFormat::Cf32,
            sample_rate: 24_000,
        };

        run(file, 0.0, 24_000, out_path.to_str().unwrap(), true).unwrap();

        let reader = Cf32Reader::open(&out_path, 24_000).unwrap();
        let output: Vec<_> = reader.collect();
        let output_rms = compute_rms(&output);
        assert!(
            (output_rms - input_rms).abs() < 0.01,
            "raw output RMS {output_rms} should match input RMS {input_rms}"
        );
    }

    #[test]
    fn rejects_indivisible_output_rate() {
        let samples = vec![Complex::new(0.0, 0.0); 100];
        let temp = write_cf32_file(&samples);
        let dir = tempfile::tempdir().expect("create temp dir");
        let out_path = dir.path().join("out.cf32");

        let file = FileInput {
            input: temp.path().to_str().unwrap().to_string(),
            format: IqFormat::Cf32,
            sample_rate: 48_000,
        };

        let result = run(file, 0.0, 44_100, out_path.to_str().unwrap(), true);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("not a multiple of 44100"),
            "error should explain the rate constraint, got: {err_msg}"
        );
    }

    #[test]
    #[ignore]
    fn gold_file_filter_produces_readable_output() {
        let gold_u8_path = "samples/iq/audio/gold_voice_852462500.u8";
        if !Path::new(gold_u8_path).exists() {
            eprintln!("gold file not found: {gold_u8_path}");
            return;
        }

        let dir = tempfile::tempdir().expect("create temp dir");
        let out_path = dir.path().join("filtered_24k.cf32");

        let file = FileInput {
            input: gold_u8_path.to_string(),
            format: IqFormat::U8,
            sample_rate: 2_400_000,
        };

        run(file, 0.0, 24_000, out_path.to_str().unwrap(), false).unwrap();

        let reader = Cf32Reader::open(&out_path, 24_000).unwrap();
        let output: Vec<_> = reader.collect();
        assert!(!output.is_empty(), "filtered output should not be empty");

        // Compare against pre-filtered gold file if it exists.
        let gold_cf32_path = "samples/iq/audio/gold_voice_852462500_24k.cf32";
        if Path::new(gold_cf32_path).exists() {
            let gold_reader = Cf32Reader::open(Path::new(gold_cf32_path), 24_000).unwrap();
            let gold: Vec<_> = gold_reader.collect();
            let diff = (output.len() as i64 - gold.len() as i64).unsigned_abs();
            assert!(
                diff <= 1,
                "sample count diff {diff} exceeds tolerance (output={}, gold={})",
                output.len(),
                gold.len()
            );
        }
    }
}

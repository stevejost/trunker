//! Debug subcommand for diagnosing DSP pipeline issues.
//!
//! Provides composable diagnostic actions for inspecting IQ files,
//! decoding control/voice channels, and extracting narrowband channels
//! from wideband captures.

mod filter;
mod info;

use std::io::Write;
use std::path::Path;

use anyhow::Result;
use clap::{Args, Subcommand, ValueEnum};
use num_complex::Complex;

use crate::dsp::nco::Nco;
use crate::output::event_handler;
use crate::output::wav::WavWriter;
use crate::p25::ident::IdentTable;
use crate::p25::nid::NidIntegrityPolicy;
use crate::pipeline::{ChannelPipeline, Modulation, PipelineConfig};
use crate::sdr::cf32_reader::Cf32Reader;
use crate::sdr::u8_reader::U8Reader;
use crate::vocoder::ImbeDecoder;

/// Debug actions available under `p25 debug`.
#[derive(Subcommand)]
pub enum DebugAction {
    /// Inspect an IQ file: format, duration, signal stats.
    Info {
        #[command(flatten)]
        file: FileInput,
    },

    /// Decode a P25 control channel from an IQ file.
    #[command(name = "decode-cc")]
    DecodeCc {
        #[command(flatten)]
        file: FileInput,

        /// NCO frequency offset in hertz. Positive values shift a channel
        /// above the center frequency to baseband; negative values shift below.
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        offset: f64,

        /// Modulation type: c4fm (default) or cqpsk (simulcast).
        #[arg(short, long, default_value = "c4fm")]
        modulation: DebugModulation,

        /// NID integrity policy: strict (default) or permissive.
        #[arg(long, default_value = "strict")]
        nid_integrity: DebugNidIntegrity,
    },

    /// Decode a P25 voice channel from an IQ file.
    #[command(name = "decode-voice")]
    DecodeVoice {
        #[command(flatten)]
        file: FileInput,

        /// NCO frequency offset in hertz. Positive values shift a channel
        /// above the center frequency to baseband; negative values shift below.
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        offset: f64,

        /// Modulation type: c4fm or cqpsk (default cqpsk).
        #[arg(short, long, default_value = "cqpsk")]
        modulation: DebugModulation,

        /// NID integrity policy: strict (default) or permissive.
        #[arg(long, default_value = "strict")]
        nid_integrity: DebugNidIntegrity,

        /// Decode IMBE voice frames into audio (CPU-intensive).
        #[arg(long)]
        decode_audio: bool,

        /// Write decoded audio to a WAV file (implies --decode-audio).
        #[arg(long)]
        audio_file: Option<String>,
    },

    /// Extract a narrowband channel: NCO shift + LPF + decimate to CF32.
    Filter {
        #[command(flatten)]
        file: FileInput,

        /// NCO frequency offset in hertz. Positive values shift a channel
        /// above the center frequency to baseband; negative values shift below.
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        offset: f64,

        /// Target output sample rate in hertz. The input sample rate must be
        /// an integer multiple of this value (e.g. 2400000 -> 24000 or 48000).
        #[arg(long)]
        output_rate: u32,

        /// Output file path for extracted CF32 samples.
        #[arg(short, long)]
        output: String,

        /// Disable RMS normalization of the output signal. By default, output
        /// is normalized to 0.5 RMS so the Costas loop and Gardner timing
        /// recovery operate at calibrated gain levels. Use this flag when
        /// preserving the original signal amplitude matters (e.g. comparing
        /// power levels between captures).
        #[arg(long)]
        no_normalize: bool,
    },
}

/// Shared file input arguments for debug actions.
#[derive(Args)]
pub struct FileInput {
    /// Path to an IQ sample file.
    #[arg(short, long)]
    pub input: String,

    /// IQ sample format.
    #[arg(long, default_value = "cf32")]
    pub format: IqFormat,

    /// Sample rate of the recording in hertz (required, no default).
    #[arg(short, long)]
    pub sample_rate: u32,
}

/// IQ sample format for file input.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum IqFormat {
    /// Complex float32 (interleaved f32 I/Q pairs, 8 bytes per sample).
    Cf32,
    /// Unsigned 8-bit (interleaved u8 I/Q pairs, 2 bytes per sample).
    U8,
}

/// Modulation type for debug CLI argument parsing.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum DebugModulation {
    /// C4FM (continuous 4-level FM) -- standard P25 modulation.
    C4fm,
    /// CQPSK (compatible quadrature phase shift keying) -- P25 simulcast.
    Cqpsk,
}

impl From<DebugModulation> for Modulation {
    fn from(m: DebugModulation) -> Self {
        match m {
            DebugModulation::C4fm => Self::C4fm,
            DebugModulation::Cqpsk => Self::Cqpsk,
        }
    }
}

/// NID integrity policy for debug CLI argument parsing.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum DebugNidIntegrity {
    /// Reject data units when the NID fails integrity checks.
    Strict,
    /// Accept data units with failed NID integrity (for debugging).
    Permissive,
}

impl From<DebugNidIntegrity> for NidIntegrityPolicy {
    fn from(p: DebugNidIntegrity) -> Self {
        match p {
            DebugNidIntegrity::Strict => Self::Strict,
            DebugNidIntegrity::Permissive => Self::Permissive,
        }
    }
}

/// IQ sample source for debug actions: CF32 or U8 file reader.
pub enum DebugSource {
    /// CF32 (complex float32) file.
    Cf32(Cf32Reader),
    /// Unsigned 8-bit file.
    U8(U8Reader),
}

impl Iterator for DebugSource {
    type Item = Complex<f32>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            DebugSource::Cf32(reader) => reader.next(),
            DebugSource::U8(reader) => reader.next(),
        }
    }
}

/// Open an IQ file as a [`DebugSource`] based on the format argument.
pub fn open_debug_source(file: &FileInput) -> Result<DebugSource> {
    let path = Path::new(&file.input);
    match file.format {
        IqFormat::Cf32 => {
            let reader = Cf32Reader::open(path, file.sample_rate)?;
            Ok(DebugSource::Cf32(reader))
        }
        IqFormat::U8 => {
            let reader = U8Reader::open(path, file.sample_rate)?;
            Ok(DebugSource::U8(reader))
        }
    }
}

/// Decode a P25 control channel from an IQ file.
///
/// Streams samples through an optional NCO shift and the full
/// `ChannelPipeline`, writing decoded events as JSON lines to `output`.
///
/// The sample rate must be a multiple of 24000 Hz (the pipeline's
/// internal channel rate). Non-multiple rates will return an error.
fn decode_cc(
    file: FileInput,
    offset: f64,
    modulation: Modulation,
    nid_integrity: NidIntegrityPolicy,
    output: &mut impl Write,
) -> Result<()> {
    let source = open_debug_source(&file)?;

    let config = PipelineConfig {
        sample_rate: file.sample_rate,
        modulation,
        nid_integrity,
    };
    let mut pipeline = ChannelPipeline::new(config)?;
    let mut ident_table = IdentTable::new();
    let mut tsbk_count: u64 = 0;
    let mut decoder = None;

    let mut nco = if offset != 0.0 {
        Some(Nco::new(offset, file.sample_rate as f64))
    } else {
        None
    };

    for iq_sample in source {
        let sample = match nco.as_mut() {
            Some(nco) => nco.shift(iq_sample),
            None => iq_sample,
        };

        if let Some(event) = pipeline.process_sample(sample) {
            let nac = pipeline.current_nac();
            event_handler::handle_receiver_event(
                nac,
                &mut ident_table,
                &mut tsbk_count,
                &mut decoder,
                None,
                event,
                output,
            );
        }
    }

    Ok(())
}

/// Decode a P25 voice channel from an IQ file.
///
/// Streams samples through an optional NCO shift and the full
/// `ChannelPipeline`, writing decoded events as JSON lines to `output`.
/// Voice frames are optionally decoded through the IMBE vocoder and
/// written to a WAV file.
///
/// The sample rate must be a multiple of 24000 Hz. For wideband captures,
/// use `--offset` to shift the voice channel to baseband, or pre-filter
/// with `p25 debug filter`. CQPSK demodulation requires clean narrowband
/// input for the Costas loop to lock.
fn decode_voice(
    file: FileInput,
    offset: f64,
    modulation: Modulation,
    nid_integrity: NidIntegrityPolicy,
    decode_audio: bool,
    audio_file: Option<&str>,
    output: &mut impl Write,
) -> Result<()> {
    let source = open_debug_source(&file)?;

    let config = PipelineConfig {
        sample_rate: file.sample_rate,
        modulation,
        nid_integrity,
    };
    let mut pipeline = ChannelPipeline::new(config)?;
    let mut ident_table = IdentTable::new();
    let mut tsbk_count: u64 = 0;

    let should_decode_audio = decode_audio || audio_file.is_some();
    let mut decoder = if should_decode_audio {
        Some(ImbeDecoder::new())
    } else {
        None
    };

    let mut wav_writer = if let Some(path) = audio_file {
        let writer = WavWriter::create(Path::new(path))
            .map_err(|e| anyhow::anyhow!("failed to create WAV file '{path}': {e}"))?;
        Some(writer)
    } else {
        None
    };

    let mut nco = if offset != 0.0 {
        Some(Nco::new(offset, file.sample_rate as f64))
    } else {
        None
    };

    for iq_sample in source {
        let sample = match nco.as_mut() {
            Some(nco) => nco.shift(iq_sample),
            None => iq_sample,
        };

        if let Some(event) = pipeline.process_sample(sample) {
            let nac = pipeline.current_nac();
            event_handler::handle_receiver_event(
                nac,
                &mut ident_table,
                &mut tsbk_count,
                &mut decoder,
                wav_writer.as_mut(),
                event,
                output,
            );
        }
    }

    if let Some(writer) = wav_writer {
        writer
            .finalize()
            .map_err(|e| anyhow::anyhow!("failed to finalize WAV file: {e}"))?;
    }

    Ok(())
}

/// Run a debug action.
pub fn run(action: DebugAction) -> Result<()> {
    match action {
        DebugAction::Info { file } => info::run(file, &mut std::io::stdout()),
        DebugAction::DecodeCc {
            file,
            offset,
            modulation,
            nid_integrity,
        } => decode_cc(
            file,
            offset,
            modulation.into(),
            nid_integrity.into(),
            &mut std::io::stdout(),
        ),
        DebugAction::DecodeVoice {
            file,
            offset,
            modulation,
            nid_integrity,
            decode_audio,
            audio_file,
        } => decode_voice(
            file,
            offset,
            modulation.into(),
            nid_integrity.into(),
            decode_audio,
            audio_file.as_deref(),
            &mut std::io::stdout(),
        ),
        DebugAction::Filter {
            file,
            offset,
            output_rate,
            output,
            no_normalize,
        } => filter::run(file, offset, output_rate, &output, no_normalize),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use num_complex::Complex;
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// Wrapper to parse debug subcommands via clap.
    #[derive(Parser)]
    #[command(name = "p25")]
    struct TestCli {
        #[command(subcommand)]
        command: TestCommand,
    }

    #[derive(clap::Subcommand)]
    enum TestCommand {
        /// Debug subcommand.
        Debug {
            #[command(subcommand)]
            action: DebugAction,
        },
    }

    // -- CLI parsing tests --

    #[test]
    fn cli_debug_info_parses() {
        let cli = TestCli::try_parse_from([
            "p25", "debug", "info", "-i", "test.u8", "--format", "u8", "-s", "2400000",
        ]);
        assert!(cli.is_ok(), "debug info should parse: {:?}", cli.err());
    }

    #[test]
    fn cli_debug_info_requires_sample_rate() {
        let cli =
            TestCli::try_parse_from(["p25", "debug", "info", "-i", "test.u8", "--format", "u8"]);
        assert!(cli.is_err(), "should require --sample-rate");
    }

    #[test]
    fn cli_debug_info_format_defaults_to_cf32() {
        let cli =
            TestCli::try_parse_from(["p25", "debug", "info", "-i", "test.cf32", "-s", "2400000"])
                .unwrap();
        match cli.command {
            TestCommand::Debug {
                action: DebugAction::Info { file },
            } => {
                assert!(matches!(file.format, IqFormat::Cf32));
            }
            _ => panic!("expected Debug Info"),
        }
    }

    #[test]
    fn cli_debug_decode_cc_parses_all_args() {
        let cli = TestCli::try_parse_from([
            "p25",
            "debug",
            "decode-cc",
            "-i",
            "wideband.u8",
            "--format",
            "u8",
            "-s",
            "2400000",
            "--offset",
            "-112500",
            "-m",
            "c4fm",
            "--nid-integrity",
            "permissive",
        ]);
        assert!(cli.is_ok(), "debug decode-cc should parse: {:?}", cli.err());
        match cli.unwrap().command {
            TestCommand::Debug {
                action:
                    DebugAction::DecodeCc {
                        file,
                        offset,
                        modulation,
                        nid_integrity,
                    },
            } => {
                assert_eq!(file.input, "wideband.u8");
                assert!(matches!(file.format, IqFormat::U8));
                assert_eq!(file.sample_rate, 2_400_000);
                assert!((offset - (-112_500.0)).abs() < 0.1);
                assert!(matches!(modulation, DebugModulation::C4fm));
                assert!(matches!(nid_integrity, DebugNidIntegrity::Permissive));
            }
            _ => panic!("expected Debug DecodeCc"),
        }
    }

    #[test]
    fn cli_debug_decode_voice_parses_all_args() {
        let cli = TestCli::try_parse_from([
            "p25",
            "debug",
            "decode-voice",
            "-i",
            "voice.u8",
            "--format",
            "u8",
            "-s",
            "24000",
            "-m",
            "cqpsk",
            "--nid-integrity",
            "permissive",
            "--audio-file",
            "/tmp/out.wav",
        ]);
        assert!(
            cli.is_ok(),
            "debug decode-voice should parse: {:?}",
            cli.err()
        );
        match cli.unwrap().command {
            TestCommand::Debug {
                action:
                    DebugAction::DecodeVoice {
                        file,
                        modulation,
                        nid_integrity,
                        audio_file,
                        ..
                    },
            } => {
                assert_eq!(file.input, "voice.u8");
                assert_eq!(file.sample_rate, 24_000);
                assert!(matches!(modulation, DebugModulation::Cqpsk));
                assert!(matches!(nid_integrity, DebugNidIntegrity::Permissive));
                assert_eq!(audio_file.as_deref(), Some("/tmp/out.wav"));
            }
            _ => panic!("expected Debug DecodeVoice"),
        }
    }

    #[test]
    fn cli_debug_filter_parses_all_args() {
        let cli = TestCli::try_parse_from([
            "p25",
            "debug",
            "filter",
            "-i",
            "wideband.u8",
            "--format",
            "u8",
            "-s",
            "2400000",
            "--offset",
            "112500",
            "--output-rate",
            "24000",
            "-o",
            "/tmp/chan.cf32",
        ]);
        assert!(cli.is_ok(), "debug filter should parse: {:?}", cli.err());
        match cli.unwrap().command {
            TestCommand::Debug {
                action:
                    DebugAction::Filter {
                        file,
                        offset,
                        output_rate,
                        output,
                        no_normalize,
                    },
            } => {
                assert_eq!(file.input, "wideband.u8");
                assert!(matches!(file.format, IqFormat::U8));
                assert_eq!(file.sample_rate, 2_400_000);
                assert!((offset - 112_500.0).abs() < 0.1);
                assert_eq!(output_rate, 24_000);
                assert_eq!(output, "/tmp/chan.cf32");
                assert!(!no_normalize, "no_normalize should default to false");
            }
            _ => panic!("expected Debug Filter"),
        }
    }

    #[test]
    fn cli_debug_filter_no_normalize_flag() {
        let cli = TestCli::try_parse_from([
            "p25",
            "debug",
            "filter",
            "-i",
            "test.cf32",
            "-s",
            "48000",
            "--output-rate",
            "24000",
            "-o",
            "/tmp/out.cf32",
            "--no-normalize",
        ])
        .unwrap();
        match cli.command {
            TestCommand::Debug {
                action: DebugAction::Filter { no_normalize, .. },
            } => {
                assert!(
                    no_normalize,
                    "no_normalize should be true when flag is passed"
                );
            }
            _ => panic!("expected Debug Filter"),
        }
    }

    #[test]
    fn cli_debug_decode_cc_offset_defaults_to_zero() {
        let cli = TestCli::try_parse_from([
            "p25",
            "debug",
            "decode-cc",
            "-i",
            "test.cf32",
            "-s",
            "2400000",
        ])
        .unwrap();
        match cli.command {
            TestCommand::Debug {
                action: DebugAction::DecodeCc { offset, .. },
            } => {
                assert!((offset).abs() < 0.001);
            }
            _ => panic!("expected Debug DecodeCc"),
        }
    }

    #[test]
    fn cli_debug_modulation_converts_to_pipeline() {
        let c4fm: Modulation = DebugModulation::C4fm.into();
        assert_eq!(c4fm, Modulation::C4fm);

        let cqpsk: Modulation = DebugModulation::Cqpsk.into();
        assert_eq!(cqpsk, Modulation::Cqpsk);
    }

    #[test]
    fn cli_debug_nid_integrity_converts_to_policy() {
        let strict: NidIntegrityPolicy = DebugNidIntegrity::Strict.into();
        assert_eq!(strict, NidIntegrityPolicy::Strict);

        let permissive: NidIntegrityPolicy = DebugNidIntegrity::Permissive.into();
        assert_eq!(permissive, NidIntegrityPolicy::Permissive);
    }

    // -- DebugSource delegation tests --

    fn write_cf32_samples(samples: &[Complex<f32>]) -> NamedTempFile {
        let mut file = NamedTempFile::new().expect("create temp file");
        for s in samples {
            file.write_all(&s.re.to_ne_bytes()).expect("write real");
            file.write_all(&s.im.to_ne_bytes()).expect("write imag");
        }
        file.flush().expect("flush");
        file
    }

    fn write_u8_samples(byte_pairs: &[[u8; 2]]) -> NamedTempFile {
        let mut file = NamedTempFile::new().expect("create temp file");
        for pair in byte_pairs {
            file.write_all(pair).expect("write pair");
        }
        file.flush().expect("flush");
        file
    }

    #[test]
    fn debug_source_cf32_delegates() {
        let expected = vec![Complex::new(1.0, 2.0), Complex::new(3.0, 4.0)];
        let temp = write_cf32_samples(&expected);

        let reader = Cf32Reader::open(temp.path(), 48_000).unwrap();
        let source = DebugSource::Cf32(reader);
        let samples: Vec<_> = source.collect();

        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0], expected[0]);
        assert_eq!(samples[1], expected[1]);
    }

    #[test]
    fn debug_source_u8_delegates() {
        let temp = write_u8_samples(&[[0, 255], [128, 128]]);

        let reader = U8Reader::open(temp.path(), 48_000).unwrap();
        let source = DebugSource::U8(reader);
        let samples: Vec<_> = source.collect();

        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0].re, -1.0);
        assert_eq!(samples[0].im, 1.0);
    }

    #[test]
    fn open_debug_source_cf32() {
        let expected = vec![Complex::new(0.5, -0.5)];
        let temp = write_cf32_samples(&expected);

        let file_input = FileInput {
            input: temp.path().to_str().unwrap().to_string(),
            format: IqFormat::Cf32,
            sample_rate: 2_400_000,
        };
        let source = open_debug_source(&file_input).unwrap();
        let samples: Vec<_> = source.collect();

        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0], expected[0]);
    }

    #[test]
    fn open_debug_source_u8() {
        let temp = write_u8_samples(&[[255, 0]]);

        let file_input = FileInput {
            input: temp.path().to_str().unwrap().to_string(),
            format: IqFormat::U8,
            sample_rate: 2_400_000,
        };
        let source = open_debug_source(&file_input).unwrap();
        let samples: Vec<_> = source.collect();

        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].re, 1.0);
        assert_eq!(samples[0].im, -1.0);
    }

    #[test]
    fn open_debug_source_nonexistent_file_returns_error() {
        let file_input = FileInput {
            input: "/nonexistent/file.u8".to_string(),
            format: IqFormat::U8,
            sample_rate: 48_000,
        };
        let result = open_debug_source(&file_input);
        assert!(result.is_err());
    }

    // -- decode-cc tests --

    #[test]
    fn decode_cc_silence_produces_no_events() {
        let samples = vec![Complex::new(0.0, 0.0); 48_000];
        let temp = write_cf32_samples(&samples);

        let file = FileInput {
            input: temp.path().to_str().unwrap().to_string(),
            format: IqFormat::Cf32,
            sample_rate: 48_000,
        };
        let mut output = Vec::new();

        let result = decode_cc(
            file,
            0.0,
            Modulation::C4fm,
            NidIntegrityPolicy::Strict,
            &mut output,
        );
        assert!(result.is_ok());
        assert!(output.is_empty(), "silence should produce no JSON events");
    }

    #[test]
    fn decode_cc_noise_does_not_crash() {
        let samples: Vec<Complex<f32>> = (0..48_000u32)
            .map(|i| {
                let phase = i as f32 * 0.73;
                Complex::new(phase.cos() * 0.1, phase.sin() * 0.1)
            })
            .collect();
        let temp = write_cf32_samples(&samples);

        let file = FileInput {
            input: temp.path().to_str().unwrap().to_string(),
            format: IqFormat::Cf32,
            sample_rate: 48_000,
        };
        let mut output = Vec::new();

        let result = decode_cc(
            file,
            0.0,
            Modulation::C4fm,
            NidIntegrityPolicy::Strict,
            &mut output,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn decode_cc_output_goes_to_writer() {
        // Verify that output is written to the provided Write destination,
        // not to stdout. We do this by passing a Vec<u8> and confirming
        // it receives any output (or stays empty for silence).
        let samples = vec![Complex::new(0.0, 0.0); 24_000];
        let temp = write_cf32_samples(&samples);

        let file = FileInput {
            input: temp.path().to_str().unwrap().to_string(),
            format: IqFormat::Cf32,
            sample_rate: 24_000,
        };
        let mut output = Vec::new();

        decode_cc(
            file,
            0.0,
            Modulation::C4fm,
            NidIntegrityPolicy::Strict,
            &mut output,
        )
        .unwrap();

        // Silence produces no events, so output should be empty.
        // The important thing is that it compiled and ran against Vec<u8>,
        // proving output goes to the writer, not stdout.
        assert!(output.is_empty());
    }

    #[test]
    fn decode_cc_invalid_sample_rate_returns_error() {
        let samples = vec![Complex::new(0.0, 0.0); 100];
        let temp = write_cf32_samples(&samples);

        let file = FileInput {
            input: temp.path().to_str().unwrap().to_string(),
            format: IqFormat::Cf32,
            sample_rate: 44_100, // not a multiple of 24000
        };
        let mut output = Vec::new();

        let result = decode_cc(
            file,
            0.0,
            Modulation::C4fm,
            NidIntegrityPolicy::Strict,
            &mut output,
        );
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("not a multiple of 24000"),
            "error should explain the sample rate constraint, got: {err_msg}"
        );
    }

    #[test]
    fn decode_cc_with_nco_offset_does_not_crash() {
        let samples = vec![Complex::new(0.0, 0.0); 48_000];
        let temp = write_cf32_samples(&samples);

        let file = FileInput {
            input: temp.path().to_str().unwrap().to_string(),
            format: IqFormat::Cf32,
            sample_rate: 48_000,
        };
        let mut output = Vec::new();

        let result = decode_cc(
            file,
            -12_500.0,
            Modulation::Cqpsk,
            NidIntegrityPolicy::Permissive,
            &mut output,
        );
        assert!(result.is_ok());
    }

    // -- decode-voice tests --

    #[test]
    fn decode_voice_silence_produces_no_events() {
        let samples = vec![Complex::new(0.0, 0.0); 48_000];
        let temp = write_cf32_samples(&samples);

        let file = FileInput {
            input: temp.path().to_str().unwrap().to_string(),
            format: IqFormat::Cf32,
            sample_rate: 48_000,
        };
        let mut output = Vec::new();

        let result = decode_voice(
            file,
            0.0,
            Modulation::Cqpsk,
            NidIntegrityPolicy::Strict,
            false,
            None,
            &mut output,
        );
        assert!(result.is_ok());
        assert!(output.is_empty(), "silence should produce no JSON events");
    }

    #[test]
    fn decode_voice_wav_file_created_with_valid_header() {
        let samples = vec![Complex::new(0.0, 0.0); 48_000];
        let temp = write_cf32_samples(&samples);
        let dir = tempfile::tempdir().expect("create temp dir");
        let wav_path = dir.path().join("test_voice.wav");

        let file = FileInput {
            input: temp.path().to_str().unwrap().to_string(),
            format: IqFormat::Cf32,
            sample_rate: 48_000,
        };
        let mut output = Vec::new();

        let result = decode_voice(
            file,
            0.0,
            Modulation::Cqpsk,
            NidIntegrityPolicy::Strict,
            false,
            Some(wav_path.to_str().unwrap()),
            &mut output,
        );
        assert!(result.is_ok());

        // WAV file should exist with a valid header.
        let wav_data = std::fs::read(&wav_path).expect("read WAV file");
        assert!(
            wav_data.len() >= 44,
            "WAV file should have at least a 44-byte header"
        );
        assert_eq!(&wav_data[0..4], b"RIFF");
        assert_eq!(&wav_data[8..12], b"WAVE");
        assert_eq!(&wav_data[36..40], b"data");

        // Sample rate in header should be 8000 (IMBE decode rate).
        let sample_rate =
            u32::from_le_bytes([wav_data[24], wav_data[25], wav_data[26], wav_data[27]]);
        assert_eq!(sample_rate, 8000);
    }

    #[test]
    fn decode_voice_output_goes_to_writer() {
        let samples = vec![Complex::new(0.0, 0.0); 24_000];
        let temp = write_cf32_samples(&samples);

        let file = FileInput {
            input: temp.path().to_str().unwrap().to_string(),
            format: IqFormat::Cf32,
            sample_rate: 24_000,
        };
        let mut output = Vec::new();

        decode_voice(
            file,
            0.0,
            Modulation::Cqpsk,
            NidIntegrityPolicy::Strict,
            false,
            None,
            &mut output,
        )
        .unwrap();

        // Silence produces no events; the key assertion is that this
        // compiled and ran with Vec<u8> as the Write target.
        assert!(output.is_empty());
    }

    #[test]
    fn decode_voice_invalid_sample_rate_returns_error() {
        let samples = vec![Complex::new(0.0, 0.0); 100];
        let temp = write_cf32_samples(&samples);

        let file = FileInput {
            input: temp.path().to_str().unwrap().to_string(),
            format: IqFormat::Cf32,
            sample_rate: 44_100, // not a multiple of 24000
        };
        let mut output = Vec::new();

        let result = decode_voice(
            file,
            0.0,
            Modulation::Cqpsk,
            NidIntegrityPolicy::Strict,
            false,
            None,
            &mut output,
        );
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("not a multiple of 24000"),
            "error should explain the sample rate constraint, got: {err_msg}"
        );
    }

    #[test]
    #[ignore]
    fn decode_voice_gold_file_produces_events() {
        let gold_path = "samples/iq/audio/gold_voice_852462500_24k.cf32";
        if !Path::new(gold_path).exists() {
            eprintln!("gold file not found: {gold_path}");
            return;
        }

        let file = FileInput {
            input: gold_path.to_string(),
            format: IqFormat::Cf32,
            sample_rate: 24_000,
        };
        let mut output = Vec::new();

        decode_voice(
            file,
            0.0,
            Modulation::Cqpsk,
            NidIntegrityPolicy::Permissive,
            false,
            None,
            &mut output,
        )
        .unwrap();

        let json_str = String::from_utf8(output).unwrap();
        let event_count = json_str.lines().count();
        assert!(
            event_count >= 200,
            "gold file should produce >= 200 events, got {event_count}"
        );
    }
}

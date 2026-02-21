use std::io::{self, IsTerminal, LineWriter, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use num_complex::Complex;

use trunker::channel_manager::{ChannelManager, ChannelManagerConfig, VoiceChannelEvent};
use trunker::dsp::nco::Nco;
use trunker::output::call_recorder::{CallRecorder, CompletedRecording};
use trunker::output::call_writer::AudioFormat;
use trunker::output::event_handler;
use trunker::output::json;
use trunker::output::wav::WavWriter;
use trunker::p25::ident::IdentTable;
use trunker::p25::nid::NidIntegrityPolicy;
use trunker::p25::receiver::ReceiverEvent;
use trunker::p25::tsbk::{Tsbk, TsbkOpcode, TsbkPayload};
use trunker::p25::types::Frequency;
use trunker::pipeline::{self, ChannelPipeline, PipelineConfig};
use trunker::sdr::cf32_reader::Cf32Reader;
use trunker::sdr::soapy_source::{self, SoapySource};
use trunker::sdr::u8_reader::U8Reader;
use trunker::vocoder::ImbeDecoder;

/// P25 trunked radio decoder — RF in, JSON out.
#[derive(Parser)]
#[command(name = "p25", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Modulation type for CLI argument parsing.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliModulation {
    /// C4FM (continuous 4-level FM) -- standard P25 modulation.
    C4fm,
    /// CQPSK (compatible quadrature phase shift keying) -- P25 simulcast.
    Cqpsk,
}

impl From<CliModulation> for pipeline::Modulation {
    fn from(m: CliModulation) -> Self {
        match m {
            CliModulation::C4fm => Self::C4fm,
            CliModulation::Cqpsk => Self::Cqpsk,
        }
    }
}

/// NID integrity policy for CLI argument parsing.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliNidIntegrity {
    /// Reject data units when the NID fails integrity checks.
    Strict,
    /// Accept data units with failed NID integrity (for debugging).
    Permissive,
}

impl From<CliNidIntegrity> for NidIntegrityPolicy {
    fn from(p: CliNidIntegrity) -> Self {
        match p {
            CliNidIntegrity::Strict => Self::Strict,
            CliNidIntegrity::Permissive => Self::Permissive,
        }
    }
}

/// Audio file format for CLI argument parsing.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliAudioFormat {
    /// WAV: 8 kHz, 16-bit signed PCM, mono.
    Wav,
    /// Opus: 8 kHz, mono, VOIP mode, 16 kbps, OGG container.
    Opus,
}

impl From<CliAudioFormat> for AudioFormat {
    fn from(f: CliAudioFormat) -> Self {
        match f {
            CliAudioFormat::Wav => Self::Wav,
            CliAudioFormat::Opus => Self::Opus,
        }
    }
}

/// Input source selection: file or live SDR device (mutually exclusive).
#[derive(Args)]
#[group(required = true, multiple = false)]
struct InputSource {
    /// Path to an IQ sample file.
    #[arg(short, long)]
    input: Option<String>,

    /// SoapySDR device argument string (e.g. "driver=rtlsdr").
    #[arg(short, long)]
    device: Option<String>,
}

/// IQ file sample format.
#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum CliFormat {
    /// Interleaved 32-bit float I/Q pairs (8 bytes per sample).
    #[default]
    Cf32,
    /// Unsigned 8-bit I/Q pairs (2 bytes per sample, RTL-SDR native).
    U8,
}

/// Gain control for live SDR mode (mutually exclusive).
#[derive(Args)]
#[group(required = false, multiple = false)]
struct GainControl {
    /// Manual gain in dB for live SDR mode.
    #[arg(long)]
    gain: Option<f64>,

    /// Enable automatic gain control for live SDR mode.
    #[arg(long)]
    auto_gain: bool,
}

/// Device-specific settings applied via SoapySDR write_setting().
///
/// These correspond to the "Other Settings" shown by `SoapySDRUtil --probe`.
/// Each setting is a key=value pair. Can be specified multiple times.
///
/// Example: `--setting rfgain_sel=24 --setting hdr_ctrl=false`
#[derive(Args, Clone)]
struct DeviceSettings {
    /// Device-specific setting as key=value (repeatable).
    #[arg(long = "setting", value_name = "KEY=VALUE")]
    settings: Vec<String>,
}

/// Available subcommands.
#[derive(Subcommand)]
enum Command {
    /// Decode a P25 control channel from IQ samples.
    Cc {
        #[command(flatten)]
        source: InputSource,

        #[command(flatten)]
        gain_control: GainControl,

        #[command(flatten)]
        device_settings: DeviceSettings,

        /// Antenna port name (e.g. "Antenna A"). See `p25 devices` output.
        #[arg(long)]
        antenna: Option<String>,

        /// IQ file format: cf32 (default) or u8 (RTL-SDR native / .cu8).
        #[arg(long, default_value = "cf32")]
        format: CliFormat,

        /// Sample rate in Hz.
        #[arg(short, long, default_value_t = 2_400_000)]
        sample_rate: u32,

        /// Signal frequency in Hz (required for live SDR mode).
        #[arg(short, long, default_value_t = 0)]
        frequency: u64,

        /// Center frequency of the IQ capture in Hz (for file input only).
        /// When provided with --frequency, an NCO shifts the signal to baseband.
        #[arg(long, default_value_t = 0)]
        center_freq: u64,

        /// Modulation type: c4fm (default) or cqpsk (simulcast).
        #[arg(short, long, default_value = "c4fm")]
        modulation: CliModulation,

        /// NID integrity policy: strict (default) rejects data units with
        /// failed parity; permissive continues with a warning.
        #[arg(long, default_value = "strict")]
        nid_integrity: CliNidIntegrity,

        /// Decode IMBE voice frames into audio (CPU-intensive).
        #[arg(long)]
        decode_audio: bool,

        /// Write decoded audio to a WAV file (implies --decode-audio).
        #[arg(long)]
        audio_file: Option<String>,

        /// SDR read buffer depth in milliseconds (live SDR mode only).
        ///
        /// Larger values absorb more processing jitter but add latency.
        #[arg(long, default_value_t = 500)]
        buffer_ms: u32,
    },

    /// Decode a wideband P25 trunked system (control + voice channels).
    Trunk {
        #[command(flatten)]
        source: InputSource,

        #[command(flatten)]
        gain_control: GainControl,

        #[command(flatten)]
        device_settings: DeviceSettings,

        /// Antenna port name (e.g. "Antenna A"). See `p25 devices` output.
        #[arg(long)]
        antenna: Option<String>,

        /// IQ file format: cf32 (default) or u8 (RTL-SDR native / .cu8).
        #[arg(long, default_value = "cf32")]
        format: CliFormat,

        /// Sample rate in Hz.
        #[arg(short, long, default_value_t = 2_400_000)]
        sample_rate: u32,

        /// Control channel frequency in Hz. When different from
        /// --center-freq, an NCO shifts the CC to baseband.
        #[arg(long, default_value_t = 0)]
        frequency: u64,

        /// Center frequency of the capture in Hz.
        #[arg(short = 'f', long)]
        center_freq: u64,

        /// Modulation type: c4fm or cqpsk (default cqpsk for trunking).
        #[arg(short, long, default_value = "cqpsk")]
        modulation: CliModulation,

        /// Seconds before tearing down an idle voice channel.
        #[arg(long, default_value_t = 3.0)]
        call_timeout: f64,

        /// NID integrity policy: strict (default) rejects data units with
        /// failed parity; permissive continues with a warning.
        #[arg(long, default_value = "strict")]
        nid_integrity: CliNidIntegrity,

        /// Decode IMBE voice frames into audio (CPU-intensive).
        #[arg(long)]
        decode_audio: bool,

        /// Output directory for per-call audio recordings.
        ///
        /// When set, each voice call is recorded to a separate file
        /// under `{output_dir}/{date}/{talkgroup}_{timestamp}.{ext}`.
        /// Implies --decode-audio.
        #[arg(long)]
        output_dir: Option<String>,

        /// Audio file format for call recordings: wav (default) or opus.
        ///
        /// Opus produces significantly smaller files at 16 kbps with
        /// negligible quality loss for voice (VOIP mode, 8 kHz mono).
        #[arg(long, default_value = "wav")]
        audio_format: CliAudioFormat,

        /// SDR read buffer depth in milliseconds (live SDR mode only).
        ///
        /// Larger values absorb more processing jitter but add latency.
        #[arg(long, default_value_t = 500)]
        buffer_ms: u32,

        /// Maximum simultaneous voice channel pipelines.
        ///
        /// Limits CPU load by capping how many voice channels are decoded
        /// in parallel. Grants beyond this limit are silently dropped.
        /// Set to 0 to disable the limit.
        #[arg(long, default_value_t = 10)]
        max_voices: usize,

        /// Emit full JSON lines for every decoded event (TSBK, voice frame,
        /// link control, etc.) instead of periodic stats summaries.
        ///
        /// Default behavior prints a compact stats line every 10 seconds.
        /// Use --json-output to get per-event JSON for piping to downstream
        /// consumers like `p25 monitor`.
        #[arg(long)]
        json_output: bool,
    },

    /// Diagnostic tools for inspecting IQ files and debugging the DSP pipeline.
    Debug {
        #[command(subcommand)]
        action: trunker::debug::DebugAction,
    },

    /// List available SoapySDR devices.
    Devices,

    /// Monitor P25 control channel activity from JSON lines on stdin.
    Monitor {
        /// Grant expiry timeout in seconds.
        #[arg(long, default_value_t = 3)]
        grant_timeout: u64,
    },
}

fn main() -> Result<()> {
    // When stdout is piped (e.g., `p25 cc ... | p25 monitor`), suppress all
    // stderr output. SoapySDR and hardware drivers print directly to stderr
    // and cannot be individually silenced, corrupting downstream TUI displays.
    // Skip suppression if RUST_LOG is set so debug output is visible.
    // To view logs while piping: `p25 cc ... 2>decode.log | p25 monitor`
    let rust_log_set = std::env::var_os("RUST_LOG").is_some();
    if !std::io::stdout().is_terminal() && !rust_log_set {
        suppress_stderr();
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Cc {
            source,
            gain_control,
            device_settings,
            antenna,
            format,
            sample_rate,
            frequency,
            center_freq,
            modulation,
            nid_integrity,
            decode_audio,
            audio_file,
            buffer_ms,
        } => {
            let running = setup_signal_handler()?;
            let settings = parse_settings(&device_settings.settings)?;
            let sample_source = open_sample_source(
                source,
                format,
                &gain_control,
                antenna.as_deref(),
                &settings,
                sample_rate,
                frequency,
                buffer_ms,
                running.clone(),
            )?;

            // --audio-file implies --decode-audio.
            let decode_audio = decode_audio || audio_file.is_some();

            // Compute NCO offset when both --frequency and --center-freq are set.
            let offset_hz = if frequency != 0 && center_freq != 0 {
                frequency as f64 - center_freq as f64
            } else {
                0.0
            };

            let pipeline_modulation: pipeline::Modulation = modulation.into();
            let nid_policy: NidIntegrityPolicy = nid_integrity.into();
            tracing::info!(
                sample_rate,
                frequency,
                center_freq,
                modulation = ?modulation,
                nid_integrity = ?nid_policy,
                "starting control channel decoder"
            );
            decode_control_channel(
                sample_source,
                sample_rate,
                offset_hz,
                pipeline_modulation,
                nid_policy,
                decode_audio,
                audio_file.as_deref(),
                &running,
            )?;
        }
        Command::Trunk {
            source,
            gain_control,
            device_settings,
            antenna,
            format,
            sample_rate,
            frequency,
            center_freq,
            modulation,
            call_timeout,
            nid_integrity,
            decode_audio,
            output_dir,
            audio_format,
            buffer_ms,
            max_voices,
            json_output,
        } => {
            let running = setup_signal_handler()?;
            let settings = parse_settings(&device_settings.settings)?;
            let sample_source = open_sample_source(
                source,
                format,
                &gain_control,
                antenna.as_deref(),
                &settings,
                sample_rate,
                center_freq,
                buffer_ms,
                running.clone(),
            )?;

            // Compute NCO offset to shift the CC to baseband when
            // --frequency (CC freq) differs from --center-freq.
            let cc_offset_hz = if frequency != 0 {
                frequency as f64 - center_freq as f64
            } else {
                0.0
            };

            // --output-dir implies --decode-audio.
            let decode_audio = decode_audio || output_dir.is_some();

            let pipeline_modulation: pipeline::Modulation = modulation.into();
            let nid_policy: NidIntegrityPolicy = nid_integrity.into();
            tracing::info!(
                sample_rate,
                center_freq,
                ?frequency,
                cc_offset_hz,
                modulation = ?modulation,
                nid_integrity = ?nid_policy,
                call_timeout,
                ?output_dir,
                "starting wideband trunked decoder"
            );
            // max_voices=0 means unlimited; otherwise cap at the given value.
            let max_voices = if max_voices == 0 {
                None
            } else {
                Some(max_voices)
            };
            decode_trunked(
                sample_source,
                sample_rate,
                center_freq,
                cc_offset_hz,
                pipeline_modulation,
                nid_policy,
                call_timeout,
                decode_audio,
                output_dir.as_deref().map(Path::new),
                audio_format.into(),
                max_voices,
                json_output,
                &running,
            )?;
        }
        Command::Debug { action } => {
            trunker::debug::run(action)?;
        }
        Command::Devices => {
            soapy_source::list_devices();
        }
        Command::Monitor { grant_timeout } => {
            let config = trunker::monitor::MonitorConfig {
                grant_timeout: std::time::Duration::from_secs(grant_timeout),
            };
            trunker::monitor::run(config)?;
        }
    }

    Ok(())
}

/// IQ sample source: file or live SDR hardware.
enum SampleSource {
    /// Read from a CF32 IQ file.
    Cf32(Cf32Reader),
    /// Read from a U8 IQ file (RTL-SDR native / .cu8).
    U8(U8Reader),
    /// Stream from a SoapySDR device.
    Soapy(SoapySource),
}

impl SampleSource {
    /// Returns shared SDR reader stats handles, or `None` for file-based sources.
    ///
    /// Call this before the source is consumed by a `for` loop so that stats
    /// remain accessible during iteration.
    fn sdr_stats_handles(&self) -> Option<SdrStatsHandles> {
        match self {
            SampleSource::Soapy(source) => Some(SdrStatsHandles {
                chunk_count: source.chunk_count_handle(),
                overflow_count: source.overflow_count_handle(),
            }),
            _ => None,
        }
    }
}

/// Shared atomic counters from the SDR reader thread.
struct SdrStatsHandles {
    chunk_count: Arc<AtomicU64>,
    overflow_count: Arc<AtomicU64>,
}

impl SdrStatsHandles {
    fn snapshot(&self) -> (u64, u64) {
        (
            self.chunk_count.load(Ordering::Relaxed),
            self.overflow_count.load(Ordering::Relaxed),
        )
    }
}

impl Iterator for SampleSource {
    type Item = Complex<f32>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            SampleSource::Cf32(reader) => reader.next(),
            SampleSource::U8(reader) => reader.next(),
            SampleSource::Soapy(source) => source.next(),
        }
    }
}

/// Permanently redirect stderr to `/dev/null`.
///
/// Called when stdout is piped so that stderr noise from SoapySDR drivers,
/// tracing output, and overflow indicators doesn't corrupt downstream displays.
fn suppress_stderr() {
    unsafe {
        let devnull = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
        if devnull >= 0 {
            libc::dup2(devnull, libc::STDERR_FILENO);
            libc::close(devnull);
        }
    }
}

/// Install a Ctrl-C handler that sets the returned flag to `false`.
fn setup_signal_handler() -> Result<Arc<AtomicBool>> {
    let running = Arc::new(AtomicBool::new(true));
    let running_clone = running.clone();
    ctrlc::set_handler(move || {
        running_clone.store(false, Ordering::Relaxed);
    })?;
    Ok(running)
}

/// Open the appropriate sample source based on CLI arguments.
#[allow(clippy::too_many_arguments)]
fn open_sample_source(
    source: InputSource,
    format: CliFormat,
    gain_control: &GainControl,
    antenna: Option<&str>,
    settings: &[(String, String)],
    sample_rate: u32,
    frequency: u64,
    buffer_ms: u32,
    running: Arc<AtomicBool>,
) -> Result<SampleSource> {
    if let Some(input_path) = source.input {
        let path = Path::new(&input_path);
        match format {
            CliFormat::Cf32 => {
                let reader = Cf32Reader::open(path, sample_rate)?;
                Ok(SampleSource::Cf32(reader))
            }
            CliFormat::U8 => {
                let reader = U8Reader::open(path, sample_rate)?;
                Ok(SampleSource::U8(reader))
            }
        }
    } else if let Some(device_args) = source.device {
        validate_device_args(frequency, gain_control)?;
        let gain = resolve_gain(gain_control);
        let soapy = SoapySource::open(
            &device_args,
            frequency,
            sample_rate,
            gain,
            antenna,
            settings,
            buffer_ms,
            running,
        )?;
        Ok(SampleSource::Soapy(soapy))
    } else {
        // clap's required group ensures we never reach here.
        bail!("specify --input or --device")
    }
}

/// Parse `key=value` setting strings into tuples.
fn parse_settings(raw: &[String]) -> Result<Vec<(String, String)>> {
    raw.iter()
        .map(|s| {
            let (key, value) = s
                .split_once('=')
                .ok_or_else(|| anyhow::anyhow!("invalid setting '{s}': expected key=value"))?;
            Ok((key.to_string(), value.to_string()))
        })
        .collect()
}

/// Validate that required args are present for live SDR mode.
fn validate_device_args(frequency: u64, gain_control: &GainControl) -> Result<()> {
    if frequency == 0 {
        bail!("frequency is required for live SDR mode (use --frequency)");
    }
    if gain_control.gain.is_none() && !gain_control.auto_gain {
        bail!("specify --gain or --auto-gain for live SDR mode");
    }
    Ok(())
}

/// Convert GainControl args into the Option<f64> that SoapySource expects.
fn resolve_gain(gain_control: &GainControl) -> Option<f64> {
    if gain_control.auto_gain {
        None
    } else {
        gain_control.gain
    }
}

/// Run the control channel decode pipeline.
///
/// * `offset_hz` - NCO frequency offset in hertz. When non-zero, each sample
///   is shifted by this amount before entering the pipeline.
#[allow(clippy::too_many_arguments)]
fn decode_control_channel(
    source: SampleSource,
    sample_rate: u32,
    offset_hz: f64,
    modulation: pipeline::Modulation,
    nid_integrity: NidIntegrityPolicy,
    decode_audio: bool,
    audio_file: Option<&str>,
    running: &Arc<AtomicBool>,
) -> Result<()> {
    let config = PipelineConfig {
        sample_rate,
        modulation,
        nid_integrity,
        sync_timeout_samples: Some(pipeline::CC_SYNC_TIMEOUT_SAMPLES),
    };
    let mut pipeline = ChannelPipeline::new(config)?;
    let mut ident_table = IdentTable::new();
    let mut tsbk_count: u64 = 0;
    let mut decoder = if decode_audio {
        Some(ImbeDecoder::new())
    } else {
        None
    };

    let mut wav_writer = if let Some(path) = audio_file {
        let writer = WavWriter::create(Path::new(path))
            .map_err(|e| anyhow::anyhow!("failed to create WAV file '{path}': {e}"))?;
        tracing::info!(path, "writing decoded audio to WAV file");
        Some(writer)
    } else {
        None
    };

    let mut nco = if offset_hz != 0.0 {
        tracing::info!(offset_hz, "applying NCO shift from center");
        Some(Nco::new(offset_hz, sample_rate as f64))
    } else {
        None
    };

    let mut stdout = LineWriter::new(io::stdout().lock());

    for iq_sample in source {
        if !running.load(Ordering::Relaxed) {
            tracing::info!("interrupted by signal");
            break;
        }

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
                &mut stdout,
            );
        }
    }

    if let Some(writer) = wav_writer {
        writer
            .finalize()
            .map_err(|e| anyhow::anyhow!("failed to finalize WAV file: {e}"))?;
        tracing::info!("WAV file finalized");
    }

    tracing::info!(
        samples = pipeline.sample_count(),
        tsbks = tsbk_count,
        "decode complete"
    );
    Ok(())
}

/// Periodic stats counters for the trunked decoder.
///
/// Accumulates event counts between stats report intervals. The `prev_*`
/// fields track totals at the last report so deltas can be computed.
struct TrunkStats {
    /// CC sync detections since last report.
    cc_syncs: u64,
    /// Voice frames decoded since last report.
    voice_frames: u64,
    /// Total TSBKs at last report (for computing delta).
    prev_tsbk_count: u64,
    /// Total expired-timeout at last report.
    prev_expired_timeout: u64,
    /// Total expired-no-carrier at last report.
    prev_expired_no_carrier: u64,
    /// Total SDR chunks at last report (for computing delta).
    prev_sdr_chunks: u64,
    /// Accumulated raw IQ power (|sample|^2) over interval.
    iq_power_sum: f64,
    /// Number of samples accumulated for power measurement.
    iq_power_count: u64,
}

/// Snapshot of current decoder state for stats reporting.
struct StatsSnapshot {
    elapsed_seconds: u64,
    tsbk_count: u64,
    active_voices: usize,
    costas_freq: f32,
    expired_timeout: u64,
    expired_no_carrier: u64,
    /// `(chunk_count, overflow_count)` from the SDR reader thread, if live.
    sdr_stats: Option<(u64, u64)>,
    /// Average raw IQ power (|sample|^2) over this interval.
    iq_power_db: f32,
}

impl TrunkStats {
    fn new() -> Self {
        Self {
            cc_syncs: 0,
            voice_frames: 0,
            prev_tsbk_count: 0,
            prev_expired_timeout: 0,
            prev_expired_no_carrier: 0,
            prev_sdr_chunks: 0,
            iq_power_sum: 0.0,
            iq_power_count: 0,
        }
    }

    /// Print a stats line and reset interval counters.
    fn report(&mut self, snap: &StatsSnapshot, out: &mut impl Write) {
        let tsbk_delta = snap.tsbk_count - self.prev_tsbk_count;
        let expired_delta = (snap.expired_timeout - self.prev_expired_timeout)
            + (snap.expired_no_carrier - self.prev_expired_no_carrier);
        let no_carrier_delta = snap.expired_no_carrier - self.prev_expired_no_carrier;

        // Base stats line.
        let _ = write!(
            out,
            "[{:>4}s] tsbks={} (+{}) voices={} frames={} cc_syncs={} \
             costas_freq={:.4} iq_power={:.1}dB expired={} (no_carrier={})",
            snap.elapsed_seconds,
            snap.tsbk_count,
            tsbk_delta,
            snap.active_voices,
            self.voice_frames,
            self.cc_syncs,
            snap.costas_freq,
            snap.iq_power_db,
            expired_delta,
            no_carrier_delta,
        );

        // Append SDR reader thread stats when running live.
        if let Some((chunks, overflows)) = snap.sdr_stats {
            let chunk_delta = chunks - self.prev_sdr_chunks;
            let _ = write!(
                out,
                " sdr_chunks={chunks} (+{chunk_delta}) overflows={overflows}"
            );
            self.prev_sdr_chunks = chunks;
        }

        let _ = writeln!(out);

        // Reset interval counters.
        self.cc_syncs = 0;
        self.voice_frames = 0;
        self.prev_tsbk_count = snap.tsbk_count;
        self.prev_expired_timeout = snap.expired_timeout;
        self.prev_expired_no_carrier = snap.expired_no_carrier;
    }
}

/// Run the wideband trunked decoder (CC + voice channels).
#[allow(clippy::too_many_arguments)]
fn decode_trunked(
    source: SampleSource,
    sample_rate: u32,
    center_freq: u64,
    cc_offset_hz: f64,
    modulation: pipeline::Modulation,
    nid_integrity: NidIntegrityPolicy,
    call_timeout: f64,
    decode_audio: bool,
    output_dir: Option<&Path>,
    audio_format: AudioFormat,
    max_voices: Option<usize>,
    json_output: bool,
    running: &Arc<AtomicBool>,
) -> Result<()> {
    let config = PipelineConfig {
        sample_rate,
        modulation,
        nid_integrity,
        sync_timeout_samples: Some(pipeline::CC_SYNC_TIMEOUT_SAMPLES),
    };
    let mut cc_pipeline = ChannelPipeline::new(config)?;
    let mut ident_table = IdentTable::new();
    let mut tsbk_count: u64 = 0;

    // NCO to shift the CC to baseband when it's not at DC.
    let mut cc_nco = if cc_offset_hz.abs() > 0.1 {
        Some(Nco::new(cc_offset_hz, f64::from(sample_rate)))
    } else {
        None
    };

    let mut channel_manager = ChannelManager::new(ChannelManagerConfig {
        center_frequency: Frequency::from_hz(center_freq),
        sample_rate,
        call_timeout_seconds: call_timeout,
        nid_integrity,
        modulation,
        decode_audio,
        max_channels: max_voices,
    });

    let mut recorder = output_dir.map(|dir| CallRecorder::new(dir, audio_format));
    let mut voice_events = Vec::new();
    let mut stdout = LineWriter::new(io::stdout().lock());

    // Grab shared SDR stats handles before the for loop consumes `source`.
    let sdr_handles = source.sdr_stats_handles();

    // Stats mode: report every 10 seconds instead of per-event JSON.
    let stats_interval_samples = u64::from(sample_rate) * 10;
    let mut stats = TrunkStats::new();
    let mut samples_since_report: u64 = 0;
    let mut total_elapsed_seconds: u64 = 0;

    for iq_sample in source {
        if !running.load(Ordering::Relaxed) {
            tracing::info!("interrupted by signal");
            break;
        }

        // Accumulate raw IQ power for diagnostics.
        stats.iq_power_sum += (iq_sample.re * iq_sample.re + iq_sample.im * iq_sample.im) as f64;
        stats.iq_power_count += 1;

        // Shift CC to baseband if needed, then feed to CC pipeline.
        let cc_sample = match &mut cc_nco {
            Some(nco) => nco.shift(iq_sample),
            None => iq_sample,
        };
        if let Some(event) = cc_pipeline.process_sample(cc_sample) {
            let nac = cc_pipeline.current_nac();

            // Keep voice channel Costas seed in sync with CC's locked state.
            channel_manager.update_costas_seed(&cc_pipeline);

            // Count CC syncs for stats mode.
            if matches!(event, ReceiverEvent::Nid(_)) {
                stats.cc_syncs += 1;
            }

            if json_output {
                handle_cc_event(
                    nac,
                    &mut ident_table,
                    &mut tsbk_count,
                    &mut channel_manager,
                    recorder.as_mut(),
                    event,
                    &mut stdout,
                );
            } else {
                // Stats mode: process the event without JSON output.
                handle_cc_event_quiet(
                    &mut ident_table,
                    &mut tsbk_count,
                    &mut channel_manager,
                    recorder.as_mut(),
                    event,
                );
            }
        }

        // Feed to all active voice channel pipelines.
        voice_events.clear();
        channel_manager.process_sample(iq_sample, &mut voice_events);
        for voice_event in &voice_events {
            if let Some(recorder) = recorder.as_mut()
                && let Some(completed) = recorder.process_event(voice_event)
            {
                log_completed_recording(&completed);
            }
            if matches!(voice_event.event, ReceiverEvent::VoiceFrame(_)) {
                stats.voice_frames += 1;
            }
            if json_output {
                emit_voice_event(voice_event, &mut stdout);
            }
        }

        // Periodic stats reporting.
        if !json_output {
            samples_since_report += 1;
            if samples_since_report >= stats_interval_samples {
                total_elapsed_seconds += 10;
                let avg_power = if stats.iq_power_count > 0 {
                    (stats.iq_power_sum / stats.iq_power_count as f64) as f32
                } else {
                    0.0
                };
                let iq_power_db = if avg_power > 0.0 {
                    10.0 * avg_power.log10()
                } else {
                    -100.0
                };
                let snap = StatsSnapshot {
                    elapsed_seconds: total_elapsed_seconds,
                    tsbk_count,
                    active_voices: channel_manager.active_channel_count(),
                    costas_freq: cc_pipeline.costas_frequency(),
                    expired_timeout: channel_manager.expired_timeout(),
                    expired_no_carrier: channel_manager.expired_no_carrier(),
                    sdr_stats: sdr_handles.as_ref().map(|h| h.snapshot()),
                    iq_power_db,
                };
                stats.iq_power_sum = 0.0;
                stats.iq_power_count = 0;
                stats.report(&snap, &mut io::stderr());
                samples_since_report = 0;
            }
        }
    }

    // Finalize any open recordings on shutdown.
    if let Some(recorder) = recorder.as_mut() {
        let recordings = recorder.finalize_all();
        for recording in &recordings {
            log_completed_recording(recording);
        }
        tracing::info!(
            finalized = recordings.len(),
            "finalized open recordings on shutdown"
        );
    }

    tracing::info!(
        samples = cc_pipeline.sample_count(),
        tsbks = tsbk_count,
        active_voice_channels = channel_manager.active_channel_count(),
        "trunked decode complete"
    );
    Ok(())
}

/// Handle a CC pipeline event: print JSON, update ident table, forward
/// grant events to the channel manager, and start recordings.
fn handle_cc_event(
    nac: trunker::p25::types::Nac,
    ident_table: &mut IdentTable,
    tsbk_count: &mut u64,
    channel_manager: &mut ChannelManager,
    recorder: Option<&mut CallRecorder>,
    event: ReceiverEvent,
    out: &mut impl Write,
) {
    match event {
        ReceiverEvent::Nid(nid) => {
            tracing::debug!(
                nac = %nid.access_code,
                duid = ?nid.data_unit,
                parity_ok = nid.parity_ok,
                "CC NID decoded"
            );
        }
        ReceiverEvent::Tsbk(tsbk) => {
            if matches!(tsbk.payload, TsbkPayload::IdentifierUpdate { .. }) {
                ident_table.update(&tsbk);
            }

            // Forward grant events to channel manager.
            let is_grant = matches!(
                tsbk.header.opcode,
                TsbkOpcode::GroupVoiceChannelGrant
                    | TsbkOpcode::GroupVoiceChannelGrantUpdate
                    | TsbkOpcode::GroupVoiceChannelGrantUpdateExplicit
            );
            if is_grant {
                channel_manager.handle_grant(&tsbk, ident_table);
                // Start a new recording only for in-band channels that the
                // channel manager actually activated.
                if let Some(recorder) = recorder {
                    start_recording_for_grant(recorder, &tsbk, ident_table, channel_manager);
                }
            }

            let line = json::to_json_line(nac, &tsbk, ident_table);
            if writeln!(out, "{line}").is_err() {
                return; // BrokenPipe or other I/O error
            }
            *tsbk_count += 1;
        }
        ReceiverEvent::Error(err) => {
            tracing::debug!(error = %err, "CC decode error");
        }
        // CC shouldn't produce voice events, but handle gracefully.
        _ => {}
    }
}

/// Handle a CC pipeline event in stats mode: update ident table, forward
/// grant events, and bump counters -- but skip JSON output.
fn handle_cc_event_quiet(
    ident_table: &mut IdentTable,
    tsbk_count: &mut u64,
    channel_manager: &mut ChannelManager,
    recorder: Option<&mut CallRecorder>,
    event: ReceiverEvent,
) {
    match event {
        ReceiverEvent::Tsbk(tsbk) => {
            if matches!(tsbk.payload, TsbkPayload::IdentifierUpdate { .. }) {
                ident_table.update(&tsbk);
            }

            let is_grant = matches!(
                tsbk.header.opcode,
                TsbkOpcode::GroupVoiceChannelGrant
                    | TsbkOpcode::GroupVoiceChannelGrantUpdate
                    | TsbkOpcode::GroupVoiceChannelGrantUpdateExplicit
            );
            if is_grant {
                channel_manager.handle_grant(&tsbk, ident_table);
                if let Some(recorder) = recorder {
                    start_recording_for_grant(recorder, &tsbk, ident_table, channel_manager);
                }
            }

            *tsbk_count += 1;
        }
        ReceiverEvent::Error(err) => {
            tracing::debug!(error = %err, "CC decode error");
        }
        _ => {}
    }
}

/// Start a call recording for a grant TSBK if it represents a new channel
/// activation (not a periodic refresh) and the frequency is within capture bandwidth.
fn start_recording_for_grant(
    recorder: &mut CallRecorder,
    tsbk: &Tsbk,
    ident_table: &IdentTable,
    channel_manager: &ChannelManager,
) {
    match &tsbk.payload {
        TsbkPayload::GroupVoiceChannelGrant {
            channel,
            talkgroup,
            source,
        } => {
            if let Some(freq) = ident_table.resolve_frequency(*channel)
                && channel_manager.is_in_band(freq)
                && !recorder.has_active_recording(&freq)
                && let Some(displaced) = recorder.start_call(freq, *talkgroup, *source)
            {
                log_completed_recording(&displaced);
            }
        }
        TsbkPayload::GroupVoiceChannelGrantUpdate {
            channel_a,
            talkgroup_a,
            channel_b,
            talkgroup_b,
        } => {
            let zero_source = trunker::p25::types::SourceId::new(0);
            if let Some(freq) = ident_table.resolve_frequency(*channel_a)
                && channel_manager.is_in_band(freq)
                && !recorder.has_active_recording(&freq)
                && let Some(displaced) = recorder.start_call(freq, *talkgroup_a, zero_source)
            {
                log_completed_recording(&displaced);
            }
            if let Some(freq) = ident_table.resolve_frequency(*channel_b)
                && channel_manager.is_in_band(freq)
                && !recorder.has_active_recording(&freq)
                && let Some(displaced) = recorder.start_call(freq, *talkgroup_b, zero_source)
            {
                log_completed_recording(&displaced);
            }
        }
        TsbkPayload::GroupVoiceChannelGrantUpdateExplicit {
            receive_channel,
            talkgroup,
            ..
        } => {
            let zero_source = trunker::p25::types::SourceId::new(0);
            if let Some(freq) = ident_table.resolve_frequency(*receive_channel)
                && channel_manager.is_in_band(freq)
                && !recorder.has_active_recording(&freq)
                && let Some(displaced) = recorder.start_call(freq, *talkgroup, zero_source)
            {
                log_completed_recording(&displaced);
            }
        }
        _ => {}
    }
}

/// Log metadata for a completed call recording.
fn log_completed_recording(recording: &CompletedRecording) {
    tracing::info!(
        talkgroup = %recording.talkgroup,
        source = %recording.source,
        frequency = %recording.frequency,
        frame_count = recording.frame_count,
        end_reason = ?recording.end_reason,
        audio_file = ?recording.audio_file,
        "call recording completed"
    );
}

/// Emit a voice channel event as a JSON line with call context
/// (frequency, talkgroup, source from the CC grant).
fn emit_voice_event(voice_event: &VoiceChannelEvent, out: &mut impl Write) {
    let nac = voice_event.nac;
    let freq = voice_event.frequency;
    let tg = voice_event.talkgroup;
    let src = voice_event.source;

    match &voice_event.event {
        ReceiverEvent::VoiceFrame(vf) => {
            let line = json::voice_frame_with_context(nac, vf, freq, tg, src);
            let _ = writeln!(out, "{line}");
        }
        ReceiverEvent::LinkControl(lc) => {
            let line = json::link_control_with_context(nac, lc, freq, tg, src);
            let _ = writeln!(out, "{line}");
        }
        ReceiverEvent::CryptoControl(cc) => {
            let line = json::crypto_control_with_context(nac, cc, freq, tg, src);
            let _ = writeln!(out, "{line}");
        }
        ReceiverEvent::VoiceHeader(hdr) => {
            let line = json::voice_header_with_context(nac, hdr, freq, tg, src);
            let _ = writeln!(out, "{line}");
        }
        ReceiverEvent::DataFragment(frag) => {
            let line = json::data_fragment_with_context(nac, *frag, freq, tg, src);
            let _ = writeln!(out, "{line}");
        }
        ReceiverEvent::Nid(nid) => {
            tracing::debug!(
                frequency = %voice_event.frequency,
                talkgroup = %voice_event.talkgroup,
                nac = %nid.access_code,
                duid = ?nid.data_unit,
                "voice channel NID"
            );
        }
        ReceiverEvent::Tsbk(_) => {
            // Voice channels shouldn't produce TSBKs; ignore.
        }
        ReceiverEvent::Error(err) => {
            tracing::debug!(
                frequency = %voice_event.frequency,
                error = %err,
                "voice channel decode error"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    // -- CLI parsing tests --

    #[test]
    fn cli_file_mode_parses() {
        let cli = Cli::try_parse_from(["p25", "cc", "--input", "test.iq"]);
        assert!(cli.is_ok(), "file mode should parse: {:?}", cli.err());
    }

    #[test]
    fn cli_file_mode_with_modulation_parses() {
        let cli = Cli::try_parse_from(["p25", "cc", "--input", "test.iq", "--modulation", "cqpsk"]);
        assert!(cli.is_ok());
    }

    #[test]
    fn cli_device_mode_with_gain_parses() {
        let cli = Cli::try_parse_from([
            "p25",
            "cc",
            "--device",
            "driver=rtlsdr",
            "--frequency",
            "852350000",
            "--gain",
            "40",
        ]);
        assert!(
            cli.is_ok(),
            "device mode with gain should parse: {:?}",
            cli.err()
        );
    }

    #[test]
    fn cli_device_mode_with_auto_gain_parses() {
        let cli = Cli::try_parse_from([
            "p25",
            "cc",
            "--device",
            "driver=rtlsdr",
            "--frequency",
            "852350000",
            "--auto-gain",
        ]);
        assert!(
            cli.is_ok(),
            "device mode with auto-gain should parse: {:?}",
            cli.err()
        );
    }

    #[test]
    fn cli_devices_subcommand_parses() {
        let cli = Cli::try_parse_from(["p25", "devices"]);
        assert!(cli.is_ok());
    }

    #[test]
    fn cli_rejects_input_and_device_together() {
        let cli = Cli::try_parse_from([
            "p25",
            "cc",
            "--input",
            "test.iq",
            "--device",
            "driver=rtlsdr",
        ]);
        assert!(cli.is_err(), "should reject --input and --device together");
    }

    #[test]
    fn cli_rejects_gain_and_auto_gain_together() {
        let cli = Cli::try_parse_from([
            "p25",
            "cc",
            "--device",
            "driver=rtlsdr",
            "--gain",
            "40",
            "--auto-gain",
        ]);
        assert!(
            cli.is_err(),
            "should reject --gain and --auto-gain together"
        );
    }

    #[test]
    fn cli_requires_input_or_device() {
        let cli = Cli::try_parse_from(["p25", "cc"]);
        assert!(cli.is_err(), "should require --input or --device");
    }

    // -- Validation tests --

    #[test]
    fn validate_device_args_requires_frequency() {
        let gain = GainControl {
            gain: Some(40.0),
            auto_gain: false,
        };
        let result = validate_device_args(0, &gain);
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("frequency"),
            "error should mention frequency: {msg}"
        );
    }

    #[test]
    fn validate_device_args_requires_gain() {
        let gain = GainControl {
            gain: None,
            auto_gain: false,
        };
        let result = validate_device_args(852_350_000, &gain);
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("gain"), "error should mention gain: {msg}");
    }

    #[test]
    fn validate_device_args_accepts_manual_gain() {
        let gain = GainControl {
            gain: Some(40.0),
            auto_gain: false,
        };
        assert!(validate_device_args(852_350_000, &gain).is_ok());
    }

    #[test]
    fn validate_device_args_accepts_auto_gain() {
        let gain = GainControl {
            gain: None,
            auto_gain: true,
        };
        assert!(validate_device_args(852_350_000, &gain).is_ok());
    }

    // -- resolve_gain tests --

    #[test]
    fn resolve_gain_returns_manual_value() {
        let gain = GainControl {
            gain: Some(42.5),
            auto_gain: false,
        };
        assert_eq!(resolve_gain(&gain), Some(42.5));
    }

    #[test]
    fn resolve_gain_returns_none_for_auto() {
        let gain = GainControl {
            gain: None,
            auto_gain: true,
        };
        assert_eq!(resolve_gain(&gain), None);
    }

    // -- SampleSource::Cf32 delegation test --

    fn write_test_cf32(samples: &[Complex<f32>]) -> NamedTempFile {
        let mut file = NamedTempFile::new().expect("create temp file");
        for s in samples {
            file.write_all(&s.re.to_ne_bytes()).expect("write real");
            file.write_all(&s.im.to_ne_bytes()).expect("write imag");
        }
        file.flush().expect("flush");
        file
    }

    #[test]
    fn sample_source_file_delegates_to_cf32_reader() {
        let expected = vec![
            Complex::new(1.0, 2.0),
            Complex::new(3.0, 4.0),
            Complex::new(-0.5, 0.25),
        ];
        let temp = write_test_cf32(&expected);

        let reader = Cf32Reader::open(temp.path(), 2_400_000).unwrap();
        let source = SampleSource::Cf32(reader);
        let samples: Vec<_> = source.collect();

        assert_eq!(samples.len(), 3);
        for (got, want) in samples.iter().zip(expected.iter()) {
            assert_eq!(got.re, want.re);
            assert_eq!(got.im, want.im);
        }
    }

    #[test]
    fn sample_source_file_empty_yields_none() {
        let temp = write_test_cf32(&[]);
        let reader = Cf32Reader::open(temp.path(), 48_000).unwrap();
        let source = SampleSource::Cf32(reader);
        let samples: Vec<_> = source.collect();
        assert!(samples.is_empty());
    }

    // -- Trunk CLI parsing tests --

    #[test]
    fn cli_trunk_file_mode_parses() {
        let cli = Cli::try_parse_from([
            "p25",
            "trunk",
            "--input",
            "wideband.iq",
            "--center-freq",
            "852350000",
        ]);
        assert!(cli.is_ok(), "trunk file mode should parse: {:?}", cli.err());
    }

    #[test]
    fn cli_trunk_with_all_options_parses() {
        let cli = Cli::try_parse_from([
            "p25",
            "trunk",
            "--input",
            "wideband.iq",
            "--center-freq",
            "852350000",
            "--modulation",
            "cqpsk",
            "--call-timeout",
            "5.0",
            "--sample-rate",
            "2400000",
        ]);
        assert!(
            cli.is_ok(),
            "trunk with all options should parse: {:?}",
            cli.err()
        );
    }

    #[test]
    fn cli_trunk_requires_center_freq() {
        let cli = Cli::try_parse_from(["p25", "trunk", "--input", "wideband.iq"]);
        assert!(cli.is_err(), "trunk should require --center-freq");
    }

    #[test]
    fn cli_trunk_requires_input_or_device() {
        let cli = Cli::try_parse_from(["p25", "trunk", "--center-freq", "852350000"]);
        assert!(cli.is_err(), "trunk should require --input or --device");
    }

    #[test]
    fn cli_trunk_defaults_to_cqpsk() {
        let cli = Cli::try_parse_from([
            "p25",
            "trunk",
            "--input",
            "wideband.iq",
            "--center-freq",
            "852350000",
        ])
        .unwrap();
        match cli.command {
            Command::Trunk { modulation, .. } => {
                assert!(matches!(modulation, CliModulation::Cqpsk));
            }
            _ => panic!("expected Trunk command"),
        }
    }

    #[test]
    fn cli_trunk_default_call_timeout_is_3() {
        let cli = Cli::try_parse_from([
            "p25",
            "trunk",
            "--input",
            "wideband.iq",
            "--center-freq",
            "852350000",
        ])
        .unwrap();
        match cli.command {
            Command::Trunk { call_timeout, .. } => {
                assert!((call_timeout - 3.0).abs() < 1e-6);
            }
            _ => panic!("expected Trunk command"),
        }
    }

    // -- output-dir CLI tests --

    #[test]
    fn cli_trunk_output_dir_parses() {
        let cli = Cli::try_parse_from([
            "p25",
            "trunk",
            "--input",
            "wideband.iq",
            "--center-freq",
            "852350000",
            "--output-dir",
            "/tmp/recordings",
        ]);
        assert!(
            cli.is_ok(),
            "trunk with --output-dir should parse: {:?}",
            cli.err()
        );
        match cli.unwrap().command {
            Command::Trunk { output_dir, .. } => {
                assert_eq!(output_dir.as_deref(), Some("/tmp/recordings"));
            }
            _ => panic!("expected Trunk command"),
        }
    }

    #[test]
    fn cli_trunk_output_dir_defaults_to_none() {
        let cli = Cli::try_parse_from([
            "p25",
            "trunk",
            "--input",
            "wideband.iq",
            "--center-freq",
            "852350000",
        ])
        .unwrap();
        match cli.command {
            Command::Trunk { output_dir, .. } => {
                assert!(output_dir.is_none());
            }
            _ => panic!("expected Trunk command"),
        }
    }

    // -- Audio file CLI tests --

    #[test]
    fn cli_cc_audio_file_parses() {
        let cli = Cli::try_parse_from([
            "p25",
            "cc",
            "--input",
            "test.iq",
            "--audio-file",
            "output.wav",
        ]);
        assert!(
            cli.is_ok(),
            "cc with --audio-file should parse: {:?}",
            cli.err()
        );
        match cli.unwrap().command {
            Command::Cc { audio_file, .. } => {
                assert_eq!(audio_file.as_deref(), Some("output.wav"));
            }
            _ => panic!("expected Cc command"),
        }
    }

    #[test]
    fn cli_cc_audio_file_defaults_to_none() {
        let cli = Cli::try_parse_from(["p25", "cc", "--input", "test.iq"]).unwrap();
        match cli.command {
            Command::Cc { audio_file, .. } => {
                assert!(audio_file.is_none());
            }
            _ => panic!("expected Cc command"),
        }
    }

    // -- center-freq CLI tests --

    #[test]
    fn cli_cc_center_freq_parses() {
        let cli = Cli::try_parse_from([
            "p25",
            "cc",
            "--input",
            "wideband.iq",
            "--frequency",
            "852350000",
            "--center-freq",
            "851000000",
        ]);
        assert!(
            cli.is_ok(),
            "cc with --center-freq should parse: {:?}",
            cli.err()
        );
        match cli.unwrap().command {
            Command::Cc {
                frequency,
                center_freq,
                ..
            } => {
                assert_eq!(frequency, 852_350_000);
                assert_eq!(center_freq, 851_000_000);
            }
            _ => panic!("expected Cc command"),
        }
    }

    #[test]
    fn cli_cc_center_freq_defaults_to_zero() {
        let cli = Cli::try_parse_from(["p25", "cc", "--input", "test.iq"]).unwrap();
        match cli.command {
            Command::Cc { center_freq, .. } => {
                assert_eq!(center_freq, 0);
            }
            _ => panic!("expected Cc command"),
        }
    }

    // -- Audio format CLI tests --

    #[test]
    fn cli_trunk_audio_format_defaults_to_wav() {
        let cli = Cli::try_parse_from([
            "p25",
            "trunk",
            "--input",
            "wideband.iq",
            "--center-freq",
            "852350000",
        ])
        .unwrap();
        match cli.command {
            Command::Trunk { audio_format, .. } => {
                assert!(matches!(audio_format, CliAudioFormat::Wav));
            }
            _ => panic!("expected Trunk command"),
        }
    }

    #[test]
    fn cli_trunk_audio_format_opus_parses() {
        let cli = Cli::try_parse_from([
            "p25",
            "trunk",
            "--input",
            "wideband.iq",
            "--center-freq",
            "852350000",
            "--audio-format",
            "opus",
        ]);
        assert!(
            cli.is_ok(),
            "trunk with --audio-format opus should parse: {:?}",
            cli.err()
        );
        match cli.unwrap().command {
            Command::Trunk { audio_format, .. } => {
                assert!(matches!(audio_format, CliAudioFormat::Opus));
            }
            _ => panic!("expected Trunk command"),
        }
    }

    #[test]
    fn cli_trunk_audio_format_wav_parses() {
        let cli = Cli::try_parse_from([
            "p25",
            "trunk",
            "--input",
            "wideband.iq",
            "--center-freq",
            "852350000",
            "--audio-format",
            "wav",
        ])
        .unwrap();
        match cli.command {
            Command::Trunk { audio_format, .. } => {
                assert!(matches!(audio_format, CliAudioFormat::Wav));
            }
            _ => panic!("expected Trunk command"),
        }
    }

    #[test]
    fn cli_trunk_audio_format_invalid_rejected() {
        let cli = Cli::try_parse_from([
            "p25",
            "trunk",
            "--input",
            "wideband.iq",
            "--center-freq",
            "852350000",
            "--audio-format",
            "mp3",
        ]);
        assert!(cli.is_err(), "invalid audio format should be rejected");
    }
}

use std::io::IsTerminal;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use num_complex::Complex;

use trunker::channel_manager::{ChannelManager, ChannelManagerConfig, VoiceChannelEvent};
use trunker::dsp::nco::Nco;
use trunker::output::event_handler;
use trunker::output::json;
use trunker::output::wav::WavWriter;
use trunker::p25::ident::IdentTable;
use trunker::p25::nid::NidIntegrityPolicy;
use trunker::p25::receiver::ReceiverEvent;
use trunker::p25::tsbk::{TsbkOpcode, TsbkPayload};
use trunker::p25::types::Frequency;
use trunker::pipeline::{self, ChannelPipeline, PipelineConfig};
use trunker::sdr::cf32_reader::Cf32Reader;
use trunker::sdr::soapy_source::{self, SoapySource};
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

/// Input source selection: file or live SDR device (mutually exclusive).
#[derive(Args)]
#[group(required = true, multiple = false)]
struct InputSource {
    /// Path to an IQ sample file (CF32 format).
    #[arg(short, long)]
    input: Option<String>,

    /// SoapySDR device argument string (e.g. "driver=rtlsdr").
    #[arg(short, long)]
    device: Option<String>,
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

        /// Sample rate in Hz.
        #[arg(short, long, default_value_t = 2_400_000)]
        sample_rate: u32,

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
            sample_rate,
            frequency,
            center_freq,
            modulation,
            nid_integrity,
            decode_audio,
            audio_file,
        } => {
            let running = setup_signal_handler()?;
            let settings = parse_settings(&device_settings.settings)?;
            let sample_source = open_sample_source(
                source,
                &gain_control,
                antenna.as_deref(),
                &settings,
                sample_rate,
                frequency,
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
            sample_rate,
            center_freq,
            modulation,
            call_timeout,
            nid_integrity,
            decode_audio,
        } => {
            let running = setup_signal_handler()?;
            let settings = parse_settings(&device_settings.settings)?;
            let sample_source = open_sample_source(
                source,
                &gain_control,
                antenna.as_deref(),
                &settings,
                sample_rate,
                center_freq,
                running.clone(),
            )?;

            let pipeline_modulation: pipeline::Modulation = modulation.into();
            let nid_policy: NidIntegrityPolicy = nid_integrity.into();
            tracing::info!(
                sample_rate,
                center_freq,
                modulation = ?modulation,
                nid_integrity = ?nid_policy,
                call_timeout,
                "starting wideband trunked decoder"
            );
            decode_trunked(
                sample_source,
                sample_rate,
                center_freq,
                pipeline_modulation,
                nid_policy,
                call_timeout,
                decode_audio,
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
    File(Cf32Reader),
    /// Stream from a SoapySDR device.
    Soapy(SoapySource),
}

impl Iterator for SampleSource {
    type Item = Complex<f32>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            SampleSource::File(reader) => reader.next(),
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
        running_clone.store(false, Ordering::SeqCst);
    })?;
    Ok(running)
}

/// Open the appropriate sample source based on CLI arguments.
fn open_sample_source(
    source: InputSource,
    gain_control: &GainControl,
    antenna: Option<&str>,
    settings: &[(String, String)],
    sample_rate: u32,
    frequency: u64,
    running: Arc<AtomicBool>,
) -> Result<SampleSource> {
    if let Some(input_path) = source.input {
        let reader = Cf32Reader::open(Path::new(&input_path), sample_rate)?;
        Ok(SampleSource::File(reader))
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

    for iq_sample in source {
        if !running.load(Ordering::SeqCst) {
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
                &mut std::io::stdout(),
            );
        }
    }

    if let Some(writer) = wav_writer {
        writer.finalize()
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

/// Run the wideband trunked decoder (CC + voice channels).
#[allow(clippy::too_many_arguments)]
fn decode_trunked(
    source: SampleSource,
    sample_rate: u32,
    center_freq: u64,
    modulation: pipeline::Modulation,
    nid_integrity: NidIntegrityPolicy,
    call_timeout: f64,
    decode_audio: bool,
    running: &Arc<AtomicBool>,
) -> Result<()> {
    let config = PipelineConfig {
        sample_rate,
        modulation,
        nid_integrity,
    };
    let mut cc_pipeline = ChannelPipeline::new(config)?;
    let mut ident_table = IdentTable::new();
    let mut tsbk_count: u64 = 0;

    let mut channel_manager = ChannelManager::new(ChannelManagerConfig {
        center_frequency: Frequency::from_hz(center_freq),
        sample_rate,
        call_timeout_seconds: call_timeout,
        nid_integrity,
        modulation,
        decode_audio,
    });

    for iq_sample in source {
        if !running.load(Ordering::SeqCst) {
            tracing::info!("interrupted by signal");
            break;
        }

        // Feed to CC pipeline (CC is at DC / center frequency).
        if let Some(event) = cc_pipeline.process_sample(iq_sample) {
            let nac = cc_pipeline.current_nac();

            // Keep voice channel Costas seed in sync with CC's locked state.
            channel_manager.update_costas_seed(&cc_pipeline);

            handle_cc_event(
                nac,
                &mut ident_table,
                &mut tsbk_count,
                &mut channel_manager,
                event,
            );
        }

        // Feed to all active voice channel pipelines.
        for voice_event in channel_manager.process_sample(iq_sample) {
            emit_voice_event(&voice_event);
        }
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
/// grant events to the channel manager.
fn handle_cc_event(
    nac: trunker::p25::types::Nac,
    ident_table: &mut IdentTable,
    tsbk_count: &mut u64,
    channel_manager: &mut ChannelManager,
    event: ReceiverEvent,
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
            }

            let line = json::to_json_line(nac, &tsbk, ident_table);
            println!("{line}");
            *tsbk_count += 1;
        }
        ReceiverEvent::Error(err) => {
            tracing::debug!(error = %err, "CC decode error");
        }
        // CC shouldn't produce voice events, but handle gracefully.
        _ => {}
    }
}

/// Emit a voice channel event as a JSON line with call context
/// (frequency, talkgroup, source from the CC grant).
fn emit_voice_event(voice_event: &VoiceChannelEvent) {
    let nac = voice_event.nac;
    let freq = voice_event.frequency;
    let tg = voice_event.talkgroup;
    let src = voice_event.source;

    match &voice_event.event {
        ReceiverEvent::VoiceFrame(vf) => {
            let line = json::voice_frame_with_context(nac, vf, freq, tg, src);
            println!("{line}");
        }
        ReceiverEvent::LinkControl(lc) => {
            let line = json::link_control_with_context(nac, lc, freq, tg, src);
            println!("{line}");
        }
        ReceiverEvent::CryptoControl(cc) => {
            let line = json::crypto_control_with_context(nac, cc, freq, tg, src);
            println!("{line}");
        }
        ReceiverEvent::VoiceHeader(hdr) => {
            let line = json::voice_header_with_context(nac, hdr, freq, tg, src);
            println!("{line}");
        }
        ReceiverEvent::DataFragment(frag) => {
            let line = json::data_fragment_with_context(nac, *frag, freq, tg, src);
            println!("{line}");
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

    // -- SampleSource::File delegation test --

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
        let source = SampleSource::File(reader);
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
        let source = SampleSource::File(reader);
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
        assert!(cli.is_ok(), "cc with --audio-file should parse: {:?}", cli.err());
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
}

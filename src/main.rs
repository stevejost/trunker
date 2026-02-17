use std::io::IsTerminal;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use num_complex::Complex;

use trunker::dsp::cqpsk_demod::CqpskDemodulator;
use trunker::dsp::dc_block::DcBlocker;
use trunker::dsp::filter::DecimatingFilter;
use trunker::dsp::fm_demod::FmDemodulator;
use trunker::dsp::rrc_filter::RrcFilter;
use trunker::dsp::timing::{SymbolEvent, SymbolTiming};
use trunker::output::json;
use trunker::p25::ident::IdentTable;
use trunker::p25::receiver::{DataUnitReceiver, ReceiverEvent};
use trunker::p25::status::{StatusDeinterleaver, StreamSymbol};
use trunker::p25::tsbk::TsbkPayload;
use trunker::p25::types::{Dibit, Nac};
use trunker::sdr::cf32_reader::Cf32Reader;
use trunker::sdr::soapy_source::{self, SoapySource};

/// P25 trunked radio decoder — RF in, JSON out.
#[derive(Parser)]
#[command(name = "p25", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Modulation type for demodulation path selection.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum Modulation {
    /// C4FM (continuous 4-level FM) — standard P25 modulation.
    C4fm,
    /// CQPSK (compatible quadrature phase shift keying) — P25 simulcast.
    Cqpsk,
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

/// Available subcommands.
#[derive(Subcommand)]
enum Command {
    /// Decode a P25 control channel from IQ samples.
    Cc {
        #[command(flatten)]
        source: InputSource,

        #[command(flatten)]
        gain_control: GainControl,

        /// Sample rate in Hz.
        #[arg(short, long, default_value_t = 2_400_000)]
        sample_rate: u32,

        /// Center frequency in Hz (required for live SDR mode).
        #[arg(short, long, default_value_t = 0)]
        frequency: u64,

        /// Modulation type: c4fm (default) or cqpsk (simulcast).
        #[arg(short, long, default_value = "c4fm")]
        modulation: Modulation,
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

/// Two-stage decimation filter parameters.
/// Stage 1: 2.4 MS/s -> 240 kHz (decimate by 10).
/// Cutoff at 12 kHz with enough taps for >60 dB alias rejection at 120 kHz.
const STAGE1_CUTOFF_HZ: f32 = 12_000.0;
const STAGE1_TAPS: usize = 201;
const STAGE1_DECIMATION: usize = 10;

/// Stage 2: 240 kHz -> 24 kHz (decimate by 10).
/// Channel isolation; 6.25 kHz cutoff rejects out-of-band noise before FM demod.
const STAGE2_CUTOFF_HZ: f32 = 6_250.0;
const STAGE2_TAPS: usize = 61;
const STAGE2_DECIMATION: usize = 10;

/// Total decimation factor (stage 1 * stage 2).
const TOTAL_DECIMATION: usize = STAGE1_DECIMATION * STAGE2_DECIMATION;

fn main() -> Result<()> {
    // When stdout is piped (e.g., `p25 cc ... | p25 monitor`), suppress all
    // stderr output. SoapySDR and hardware drivers print directly to stderr
    // and cannot be individually silenced, corrupting downstream TUI displays.
    // To view logs while piping: `p25 cc ... 2>decode.log | p25 monitor`
    if !std::io::stdout().is_terminal() {
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
            sample_rate,
            frequency,
            modulation,
        } => {
            let running = setup_signal_handler()?;
            let sample_source = open_sample_source(
                source,
                &gain_control,
                sample_rate,
                frequency,
                running.clone(),
            )?;

            tracing::info!(
                sample_rate,
                frequency,
                modulation = ?modulation,
                "starting control channel decoder"
            );
            decode_control_channel(sample_source, sample_rate, modulation, &running)?;
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
        let soapy = SoapySource::open(&device_args, frequency, sample_rate, gain, running)?;
        Ok(SampleSource::Soapy(soapy))
    } else {
        // clap's required group ensures we never reach here.
        bail!("specify --input or --device")
    }
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

/// Demodulation path — holds the DSP blocks specific to each modulation type.
enum DemodPath {
    /// C4FM: FM discriminator -> DC block -> RRC matched filter -> symbol timing.
    C4fm {
        demod: FmDemodulator,
        dc_block: DcBlocker,
        rrc: RrcFilter,
        timing: SymbolTiming,
    },
    /// CQPSK: coherent demodulation (AGC, RRC, Gardner, diff decoder).
    Cqpsk { demod: CqpskDemodulator },
}

/// DSP and protocol state for the control channel decode pipeline.
struct Pipeline {
    filter_stage1: DecimatingFilter,
    filter_stage2: DecimatingFilter,
    demod_path: DemodPath,
    status_deinterleaver: StatusDeinterleaver,
    receiver: DataUnitReceiver,
    ident_table: IdentTable,
    current_nac: Nac,
    synced: bool,
    sample_count: u64,
    tsbk_count: u64,
    symbol_count: u64,
    baseband_min: f32,
    baseband_max: f32,
}

impl Pipeline {
    /// Build a new pipeline for the given sample rate and modulation.
    fn new(sample_rate: u32, modulation: Modulation) -> Self {
        let output_rate = sample_rate as f32 / TOTAL_DECIMATION as f32;

        let demod_path = match modulation {
            Modulation::C4fm => {
                let mut timing = SymbolTiming::new();

                // Bootstrap slicer with expected FM demod output levels.
                // At 24 kHz sample rate, outer deviation (+/- 1800 Hz) produces
                // approximately +/- 0.47 radians/sample from atan2 discriminator.
                let expected_outer = 2.0 * std::f32::consts::PI * 1800.0 / output_rate;
                timing.set_initial_thresholds(expected_outer, -expected_outer);

                DemodPath::C4fm {
                    demod: FmDemodulator::new(),
                    dc_block: DcBlocker::new(0.999),
                    rrc: RrcFilter::new(4800.0, output_rate, 0.2, 5),
                    timing,
                }
            }
            Modulation::Cqpsk => DemodPath::Cqpsk {
                demod: CqpskDemodulator::new(),
            },
        };

        Self {
            filter_stage1: DecimatingFilter::new(
                STAGE1_CUTOFF_HZ,
                sample_rate as f32,
                STAGE1_TAPS,
                STAGE1_DECIMATION,
            ),
            filter_stage2: DecimatingFilter::new(
                STAGE2_CUTOFF_HZ,
                sample_rate as f32 / STAGE1_DECIMATION as f32,
                STAGE2_TAPS,
                STAGE2_DECIMATION,
            ),
            demod_path,
            status_deinterleaver: StatusDeinterleaver::new(),
            receiver: DataUnitReceiver::new(),
            ident_table: IdentTable::new(),
            current_nac: Nac::new(0),
            synced: false,
            sample_count: 0,
            tsbk_count: 0,
            symbol_count: 0,
            baseband_min: f32::MAX,
            baseband_max: f32::MIN,
        }
    }
}

/// Run the control channel decode pipeline.
fn decode_control_channel(
    source: SampleSource,
    sample_rate: u32,
    modulation: Modulation,
    running: &Arc<AtomicBool>,
) -> Result<()> {
    let mut p = Pipeline::new(sample_rate, modulation);

    for iq_sample in source {
        if !running.load(Ordering::SeqCst) {
            tracing::info!("interrupted by signal");
            break;
        }

        p.sample_count += 1;

        // Two-stage decimation: 2.4 MS/s -> 240 kHz -> 24 kHz.
        let stage1_out = match p.filter_stage1.process(iq_sample) {
            Some(s) => s,
            None => continue,
        };
        let filtered = match p.filter_stage2.process(stage1_out) {
            Some(s) => s,
            None => continue,
        };

        // Demodulate through the selected path.
        let event = match &mut p.demod_path {
            DemodPath::C4fm {
                demod,
                dc_block,
                rrc,
                timing,
            } => {
                let baseband = rrc.process(dc_block.process(demod.process(filtered)));
                p.baseband_min = p.baseband_min.min(baseband);
                p.baseband_max = p.baseband_max.max(baseband);
                timing.process(baseband)
            }
            DemodPath::Cqpsk { demod } => demod.process(filtered),
        };

        match event {
            Some(SymbolEvent::SyncDetected) => handle_sync(&mut p),
            Some(SymbolEvent::Symbol(dibit)) => handle_symbol(&mut p, dibit),
            None => {}
        }
    }

    log_summary(&p);
    Ok(())
}

/// Reset protocol state when a new frame sync is detected.
fn handle_sync(p: &mut Pipeline) {
    p.status_deinterleaver = StatusDeinterleaver::new();
    p.receiver.reset();
    p.synced = true;
    let (upper, mid, lower) = match &p.demod_path {
        DemodPath::C4fm { timing, .. } => timing.slicer_thresholds(),
        DemodPath::Cqpsk { demod } => demod.slicer_thresholds(),
    };
    tracing::debug!(
        sample = p.sample_count,
        baseband_sample = p.sample_count / TOTAL_DECIMATION as u64,
        upper,
        mid,
        lower,
        "frame sync detected"
    );
}

/// Process a single decoded symbol through the protocol stack.
fn handle_symbol(p: &mut Pipeline, dibit: Dibit) {
    p.symbol_count += 1;
    if !p.synced {
        return;
    }

    // Strip status symbols.
    let data_dibit = match p.status_deinterleaver.feed(dibit) {
        StreamSymbol::Data(d) => d,
        StreamSymbol::Status(_) => return,
    };

    // Feed to protocol receiver.
    if let Some(event) = p.receiver.feed(data_dibit) {
        handle_receiver_event(p, event);
    }

    // If receiver finished this data unit, wait for next sync.
    if p.receiver.is_done() {
        p.synced = false;
    }
}

/// Dispatch a decoded protocol event (NID, TSBK, or error).
fn handle_receiver_event(p: &mut Pipeline, event: ReceiverEvent) {
    match event {
        ReceiverEvent::Nid(nid) => {
            p.current_nac = nid.access_code;
            tracing::debug!(
                nac = %p.current_nac,
                duid = ?nid.data_unit,
                parity_ok = nid.parity_ok,
                "NID decoded"
            );
        }
        ReceiverEvent::Tsbk(tsbk) => {
            if matches!(tsbk.payload, TsbkPayload::IdentifierUpdate { .. }) {
                p.ident_table.update(&tsbk);
            }

            let line = json::to_json_line(p.current_nac, &tsbk, &p.ident_table);
            println!("{line}");
            p.tsbk_count += 1;
        }
        ReceiverEvent::VoiceFrame(vf) => {
            let line = json::voice_frame_json_line(p.current_nac, &vf);
            println!("{line}");
        }
        ReceiverEvent::LinkControl(lc) => {
            let line = json::link_control_json_line(p.current_nac, &lc);
            println!("{line}");
        }
        ReceiverEvent::CryptoControl(cc) => {
            let line = json::crypto_control_json_line(p.current_nac, &cc);
            println!("{line}");
        }
        ReceiverEvent::VoiceHeader(hdr) => {
            let line = json::voice_header_json_line(p.current_nac, &hdr);
            println!("{line}");
        }
        ReceiverEvent::DataFragment(frag) => {
            let line = json::data_fragment_json_line(p.current_nac, frag);
            println!("{line}");
        }
        ReceiverEvent::Error(err) => {
            tracing::debug!(error = %err, "decode error");
        }
    }
}

/// Log final decode statistics.
fn log_summary(p: &Pipeline) {
    tracing::info!(
        samples = p.sample_count,
        baseband_samples = p.sample_count / TOTAL_DECIMATION as u64,
        symbols = p.symbol_count,
        tsbks = p.tsbk_count,
        baseband_min = p.baseband_min,
        baseband_max = p.baseband_max,
        "decode complete"
    );
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
}

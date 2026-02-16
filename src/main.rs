use std::path::Path;

use anyhow::Result;
use clap::{Parser, Subcommand};

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

/// P25 trunked radio decoder — RF in, JSON out.
#[derive(Parser)]
#[command(name = "p25", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Available subcommands.
#[derive(Subcommand)]
enum Command {
    /// Decode a P25 control channel from IQ samples.
    Cc {
        /// Path to an IQ sample file (CF32 format).
        #[arg(short, long)]
        input: String,

        /// Sample rate in Hz.
        #[arg(short, long, default_value_t = 2_400_000)]
        sample_rate: u32,

        /// Center frequency in Hz (informational only).
        #[arg(short, long, default_value_t = 0)]
        frequency: u64,
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
            input,
            sample_rate,
            frequency,
        } => {
            tracing::info!(
                input = %input,
                sample_rate,
                frequency,
                "starting control channel decoder"
            );
            decode_control_channel(&input, sample_rate)?;
        }
    }

    Ok(())
}

/// DSP and protocol state for the control channel decode pipeline.
struct Pipeline {
    filter_stage1: DecimatingFilter,
    filter_stage2: DecimatingFilter,
    demod: FmDemodulator,
    dc_block: DcBlocker,
    rrc: RrcFilter,
    timing: SymbolTiming,
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
    /// Build a new pipeline for the given sample rate.
    fn new(sample_rate: u32) -> Self {
        let output_rate = sample_rate as f32 / TOTAL_DECIMATION as f32;
        let mut timing = SymbolTiming::new();

        // Bootstrap slicer with expected FM demod output levels.
        // At 24 kHz sample rate, outer deviation (+/- 1800 Hz) produces
        // approximately +/- 0.47 radians/sample from atan2 discriminator.
        let expected_outer = 2.0 * std::f32::consts::PI * 1800.0 / output_rate;
        timing.set_initial_thresholds(expected_outer, -expected_outer);

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
            demod: FmDemodulator::new(),
            dc_block: DcBlocker::new(0.999),
            rrc: RrcFilter::new(4800.0, output_rate, 0.2, 5),
            timing,
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
fn decode_control_channel(input_path: &str, sample_rate: u32) -> Result<()> {
    let reader = Cf32Reader::open(Path::new(input_path), sample_rate)?;
    let mut p = Pipeline::new(sample_rate);

    for iq_sample in reader {
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

        // FM discriminator -> DC block -> matched filter.
        let baseband = p.rrc.process(p.dc_block.process(p.demod.process(filtered)));

        p.baseband_min = p.baseband_min.min(baseband);
        p.baseband_max = p.baseband_max.max(baseband);

        // Symbol timing + frame sync detection.
        match p.timing.process(baseband) {
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
    let (upper, mid, lower) = p.timing.slicer_thresholds();
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

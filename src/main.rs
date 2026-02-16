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
use trunker::p25::types::Nac;
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
const STAGE1_CUTOFF_HZ: f32 = 12_000.0;
const STAGE1_TAPS: usize = 51;
const STAGE1_DECIMATION: usize = 10;

/// Stage 2: 240 kHz -> 24 kHz (decimate by 10).
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

/// Run the control channel decode pipeline.
fn decode_control_channel(input_path: &str, sample_rate: u32) -> Result<()> {
    let reader = Cf32Reader::open(Path::new(input_path), sample_rate)?;

    let mut filter_stage1 = DecimatingFilter::new(
        STAGE1_CUTOFF_HZ,
        sample_rate as f32,
        STAGE1_TAPS,
        STAGE1_DECIMATION,
    );
    let mut filter_stage2 = DecimatingFilter::new(
        STAGE2_CUTOFF_HZ,
        sample_rate as f32 / STAGE1_DECIMATION as f32,
        STAGE2_TAPS,
        STAGE2_DECIMATION,
    );
    let mut demod = FmDemodulator::new();
    let mut dc_block = DcBlocker::new(0.999);
    let output_rate = sample_rate as f32 / TOTAL_DECIMATION as f32;
    let mut rrc = RrcFilter::new(4800.0, output_rate, 0.2, 5);
    let mut timing = SymbolTiming::new();

    // Bootstrap slicer with expected FM demod output levels.
    // At 24 kHz sample rate, outer deviation (+/- 1800 Hz) produces
    // approximately +/- 0.47 radians/sample from atan2 discriminator.
    let expected_outer = 2.0 * std::f32::consts::PI * 1800.0 / 24_000.0;
    timing.set_initial_thresholds(expected_outer, -expected_outer);

    // Protocol layer state, initialized on each frame sync.
    let mut status_deinterleaver = StatusDeinterleaver::new();
    let mut receiver = DataUnitReceiver::new();
    let mut ident_table = IdentTable::new();
    let mut current_nac = Nac::new(0);
    let mut synced = false;

    let mut sample_count: u64 = 0;
    let mut tsbk_count: u64 = 0;
    let mut symbol_count: u64 = 0;
    let mut baseband_min: f32 = f32::MAX;
    let mut baseband_max: f32 = f32::MIN;

    for iq_sample in reader {
        sample_count += 1;

        // Two-stage decimation: 2.4 MS/s -> 240 kHz -> 24 kHz.
        let stage1_out = match filter_stage1.process(iq_sample) {
            Some(s) => s,
            None => continue,
        };
        let filtered = match filter_stage2.process(stage1_out) {
            Some(s) => s,
            None => continue,
        };

        // FM discriminator: complex -> baseband f32.
        let raw_baseband = demod.process(filtered);

        // Remove DC offset from frequency error between SDR and carrier.
        let dc_removed = dc_block.process(raw_baseband);

        // Matched filter: root-raised-cosine for zero ISI at symbol centers.
        let baseband = rrc.process(dc_removed);

        if baseband < baseband_min {
            baseband_min = baseband;
        }
        if baseband > baseband_max {
            baseband_max = baseband;
        }

        // Symbol timing + frame sync detection.
        let event = match timing.process(baseband) {
            Some(e) => e,
            None => continue,
        };

        match event {
            SymbolEvent::SyncDetected => {
                // New frame sync found. Reset protocol state.
                status_deinterleaver = StatusDeinterleaver::new();
                receiver.reset();
                synced = true;
                let (upper, mid, lower) = timing.slicer_thresholds();
                tracing::debug!(
                    sample = sample_count,
                    baseband_sample = sample_count / TOTAL_DECIMATION as u64,
                    upper,
                    mid,
                    lower,
                    "frame sync detected"
                );
            }
            SymbolEvent::Symbol(_dibit) => {
                symbol_count += 1;
                let dibit = _dibit;
                if !synced {
                    continue;
                }

                // Strip status symbols.
                let data_dibit = match status_deinterleaver.feed(dibit) {
                    StreamSymbol::Data(d) => d,
                    StreamSymbol::Status(_) => continue,
                };

                // Feed to protocol receiver.
                if let Some(receiver_event) = receiver.feed(data_dibit) {
                    match receiver_event {
                        ReceiverEvent::Nid(nid) => {
                            current_nac = nid.access_code;
                            tracing::debug!(
                                nac = %current_nac,
                                duid = ?nid.data_unit,
                                parity_ok = nid.parity_ok,
                                "NID decoded"
                            );
                        }
                        ReceiverEvent::Tsbk(tsbk) => {
                            // Update identifier table if this is an ident message.
                            if matches!(tsbk.payload, TsbkPayload::IdentifierUpdate { .. }) {
                                ident_table.update(&tsbk);
                            }

                            let line = json::to_json_line(current_nac, &tsbk, &ident_table);
                            println!("{line}");

                            tsbk_count += 1;
                        }
                        ReceiverEvent::Error(err) => {
                            tracing::debug!(error = %err, "decode error");
                        }
                    }
                }

                // If receiver finished this data unit, wait for next sync.
                if receiver.is_done() {
                    synced = false;
                }
            }
        }
    }

    tracing::info!(
        samples = sample_count,
        baseband_samples = sample_count / TOTAL_DECIMATION as u64,
        symbols = symbol_count,
        tsbks = tsbk_count,
        baseband_min,
        baseband_max,
        "decode complete"
    );

    Ok(())
}

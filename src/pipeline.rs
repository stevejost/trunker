//! Reusable per-channel DSP and protocol decode pipeline.
//!
//! Encapsulates the full chain from IQ samples to protocol events:
//! decimation -> demodulation -> status deinterleaving -> data unit decoding.
//!
//! Each `ChannelPipeline` is independent and can be instantiated per-channel
//! in a wideband trunking decoder.

use num_complex::Complex;

use crate::dsp::cqpsk_demod::CqpskDemodulator;
use crate::dsp::dc_block::DcBlocker;
use crate::dsp::filter::DecimatingFilter;
use crate::dsp::fm_demod::FmDemodulator;
use crate::dsp::rrc_filter::RrcFilter;
use crate::dsp::timing::{SymbolEvent, SymbolTiming};
use crate::p25::receiver::{DataUnitReceiver, ReceiverEvent};
use crate::p25::status::{StatusDeinterleaver, StreamSymbol};
use crate::p25::types::{Dibit, Nac};

/// Two-stage decimation filter parameters.
/// Stage 1: 2.4 MS/s -> 240 kHz (decimate by 10).
const STAGE1_CUTOFF_HZ: f32 = 12_000.0;
const STAGE1_TAPS: usize = 201;
const STAGE1_DECIMATION: usize = 10;

/// Stage 2: 240 kHz -> 24 kHz (decimate by 10).
const STAGE2_CUTOFF_HZ: f32 = 6_250.0;
const STAGE2_TAPS: usize = 61;
const STAGE2_DECIMATION: usize = 10;

/// Total decimation factor (stage 1 * stage 2).
pub const TOTAL_DECIMATION: usize = STAGE1_DECIMATION * STAGE2_DECIMATION;

/// Modulation type for demodulation path selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modulation {
    /// C4FM (continuous 4-level FM) -- standard P25 modulation.
    C4fm,
    /// CQPSK (compatible quadrature phase shift keying) -- P25 simulcast.
    Cqpsk,
}

/// Demodulation path -- holds the DSP blocks specific to each modulation type.
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

/// Configuration for constructing a channel pipeline.
#[derive(Debug, Clone, Copy)]
pub struct PipelineConfig {
    /// Input sample rate in hertz.
    pub sample_rate: u32,
    /// Modulation type.
    pub modulation: Modulation,
}

/// Per-channel DSP and protocol decode pipeline.
///
/// Processes complex IQ samples at the input sample rate and produces
/// `ReceiverEvent`s when protocol data units are decoded.
///
/// Does **not** contain the identifier table or NAC state -- those are
/// managed by the orchestrator that owns this pipeline.
pub struct ChannelPipeline {
    filter_stage1: DecimatingFilter,
    filter_stage2: DecimatingFilter,
    demod_path: DemodPath,
    status_deinterleaver: StatusDeinterleaver,
    receiver: DataUnitReceiver,
    synced: bool,
    /// Most recently decoded NAC from NID.
    current_nac: Nac,
    /// Total input samples processed.
    sample_count: u64,
}

impl ChannelPipeline {
    /// Build a new pipeline with the given configuration.
    pub fn new(config: PipelineConfig) -> Self {
        let output_rate = config.sample_rate as f32 / TOTAL_DECIMATION as f32;

        let demod_path = match config.modulation {
            Modulation::C4fm => {
                let mut timing = SymbolTiming::new();
                let expected_outer =
                    2.0 * std::f32::consts::PI * 1800.0 / output_rate;
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
                config.sample_rate as f32,
                STAGE1_TAPS,
                STAGE1_DECIMATION,
            ),
            filter_stage2: DecimatingFilter::new(
                STAGE2_CUTOFF_HZ,
                config.sample_rate as f32 / STAGE1_DECIMATION as f32,
                STAGE2_TAPS,
                STAGE2_DECIMATION,
            ),
            demod_path,
            status_deinterleaver: StatusDeinterleaver::new(),
            receiver: DataUnitReceiver::new(),
            synced: false,
            current_nac: Nac::new(0),
            sample_count: 0,
        }
    }

    /// Process one IQ sample through the full pipeline.
    ///
    /// Returns decoded protocol events, if any. Most samples produce
    /// no events (decimation, inter-symbol samples, buffering).
    pub fn process_sample(&mut self, sample: Complex<f32>) -> Option<ReceiverEvent> {
        self.sample_count += 1;

        // Two-stage decimation: 2.4 MS/s -> 240 kHz -> 24 kHz.
        let stage1_out = self.filter_stage1.process(sample)?;
        let filtered = self.filter_stage2.process(stage1_out)?;

        // Demodulate through the selected path.
        let event = match &mut self.demod_path {
            DemodPath::C4fm {
                demod,
                dc_block,
                rrc,
                timing,
            } => {
                let baseband = rrc.process(dc_block.process(demod.process(filtered)));
                timing.process(baseband)
            }
            DemodPath::Cqpsk { demod } => demod.process(filtered),
        };

        match event {
            Some(SymbolEvent::SyncDetected) => {
                self.handle_sync();
                None
            }
            Some(SymbolEvent::Symbol(dibit)) => self.handle_symbol(dibit),
            None => None,
        }
    }

    /// Return the most recently decoded NAC.
    pub fn current_nac(&self) -> Nac {
        self.current_nac
    }

    /// Return the total number of input samples processed.
    pub fn sample_count(&self) -> u64 {
        self.sample_count
    }

    /// Reset protocol state when a new frame sync is detected.
    fn handle_sync(&mut self) {
        self.status_deinterleaver = StatusDeinterleaver::new();
        self.receiver.reset();
        self.synced = true;

        let (upper, mid, lower) = match &self.demod_path {
            DemodPath::C4fm { timing, .. } => timing.slicer_thresholds(),
            DemodPath::Cqpsk { demod } => demod.slicer_thresholds(),
        };
        tracing::debug!(
            sample = self.sample_count,
            baseband_sample = self.sample_count / TOTAL_DECIMATION as u64,
            upper,
            mid,
            lower,
            "frame sync detected"
        );
    }

    /// Process a single decoded symbol through the protocol stack.
    fn handle_symbol(&mut self, dibit: Dibit) -> Option<ReceiverEvent> {
        if !self.synced {
            return None;
        }

        // Strip status symbols.
        let data_dibit = match self.status_deinterleaver.feed(dibit) {
            StreamSymbol::Data(d) => d,
            StreamSymbol::Status(_) => return None,
        };

        // Feed to protocol receiver.
        let event = self.receiver.feed(data_dibit);

        // If receiver finished this data unit, wait for next sync.
        if self.receiver.is_done() {
            self.synced = false;
        }

        // Update NAC from NID events.
        if let Some(ReceiverEvent::Nid(ref nid)) = event {
            self.current_nac = nid.access_code;
        }

        event
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_constructs_c4fm() {
        let config = PipelineConfig {
            sample_rate: 2_400_000,
            modulation: Modulation::C4fm,
        };
        let pipeline = ChannelPipeline::new(config);
        assert_eq!(pipeline.sample_count(), 0);
        assert_eq!(pipeline.current_nac(), Nac::new(0));
    }

    #[test]
    fn pipeline_constructs_cqpsk() {
        let config = PipelineConfig {
            sample_rate: 2_400_000,
            modulation: Modulation::Cqpsk,
        };
        let pipeline = ChannelPipeline::new(config);
        assert_eq!(pipeline.sample_count(), 0);
    }

    #[test]
    fn pipeline_processes_silence_without_events() {
        let config = PipelineConfig {
            sample_rate: 2_400_000,
            modulation: Modulation::Cqpsk,
        };
        let mut pipeline = ChannelPipeline::new(config);

        let silence = Complex::new(0.0, 0.0);
        let mut event_count = 0;
        for _ in 0..10_000 {
            if pipeline.process_sample(silence).is_some() {
                event_count += 1;
            }
        }

        assert_eq!(event_count, 0, "silence should not produce events");
        assert_eq!(pipeline.sample_count(), 10_000);
    }

    #[test]
    fn pipeline_processes_noise_without_panic() {
        let config = PipelineConfig {
            sample_rate: 2_400_000,
            modulation: Modulation::C4fm,
        };
        let mut pipeline = ChannelPipeline::new(config);

        // Feed pseudo-random noise to exercise the full path.
        for i in 0..50_000u32 {
            let phase = i as f32 * 0.73;
            let sample = Complex::new(phase.cos() * 0.1, phase.sin() * 0.1);
            let _ = pipeline.process_sample(sample);
        }

        assert_eq!(pipeline.sample_count(), 50_000);
    }
}

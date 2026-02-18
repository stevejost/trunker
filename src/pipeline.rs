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

/// Target IF rate after all decimation: 24 kHz (5 samples/symbol at 4800 baud).
const CHANNEL_RATE: u32 = 24_000;

/// Reference first-stage filter parameters at 2.4 MS/s.
/// Used to compute proportional tap counts at other sample rates.
const STAGE1_REF_RATE: f32 = 2_400_000.0;
const STAGE1_REF_TAPS: f32 = 201.0;

/// Reference final-stage filter parameters at 240 kHz.
const FINAL_STAGE_REF_RATE: f32 = 240_000.0;
const FINAL_STAGE_REF_TAPS: f32 = 61.0;

/// Cutoff frequency for non-final stages (protects 12.5 kHz P25 channel).
const NON_FINAL_CUTOFF_HZ: f32 = 12_000.0;

/// Cutoff frequency for the final stage (half of 12.5 kHz channel bandwidth).
const FINAL_CUTOFF_HZ: f32 = 6_250.0;

/// Maximum decimation factor per stage.
const MAX_STAGE_FACTOR: usize = 25;

/// Error type for decimation configuration.
#[derive(Debug, thiserror::Error)]
pub enum DecimationError {
    /// Sample rate is not an integer multiple of the 24 kHz channel rate.
    #[error("sample rate {sample_rate} Hz is not a multiple of 24000; try {nearest_lower} or {nearest_higher}")]
    NotDivisible {
        /// The invalid sample rate.
        sample_rate: u32,
        /// Nearest valid rate below.
        nearest_lower: u32,
        /// Nearest valid rate above.
        nearest_higher: u32,
    },
    /// Total decimation factor cannot be split into stages of 25x or less.
    #[error("cannot factor total decimation {total}x into stages of 25x or less")]
    NoValidFactoring {
        /// The unfactorable total decimation.
        total: usize,
    },
}

/// Configuration for one decimation filter stage.
#[derive(Debug, Clone)]
pub struct DecimationStage {
    /// Decimation factor for this stage.
    pub decimation_factor: usize,
    /// Low-pass filter cutoff frequency in hertz.
    pub cutoff_hz: f32,
    /// Number of FIR filter taps.
    pub num_taps: usize,
    /// Input sample rate for this stage in hertz.
    pub input_rate: f32,
}

/// Computed decimation configuration for a given input sample rate.
#[derive(Debug, Clone)]
pub struct DecimationConfig {
    /// Ordered list of decimation stages (first applied first).
    pub stages: Vec<DecimationStage>,
}

impl DecimationConfig {
    /// Compute the decimation stages required to reduce `sample_rate` to 24 kHz.
    ///
    /// Returns an error if the sample rate is not a multiple of 24000 or
    /// cannot be factored into stages of 25x or less.
    pub fn compute(sample_rate: u32) -> Result<Self, DecimationError> {
        if sample_rate == 0 {
            return Err(DecimationError::NotDivisible {
                sample_rate: 0,
                nearest_lower: CHANNEL_RATE,
                nearest_higher: CHANNEL_RATE,
            });
        }
        if !sample_rate.is_multiple_of(CHANNEL_RATE) {
            let nearest_lower = (sample_rate / CHANNEL_RATE) * CHANNEL_RATE;
            let nearest_higher = nearest_lower + CHANNEL_RATE;
            return Err(DecimationError::NotDivisible {
                sample_rate,
                nearest_lower,
                nearest_higher,
            });
        }

        let total = (sample_rate / CHANNEL_RATE) as usize;

        let factors = factor_into_stages(total)?;
        let stage_count = factors.len();

        let mut stages = Vec::with_capacity(stage_count);
        let mut current_rate = sample_rate as f32;

        for (i, &factor) in factors.iter().enumerate() {
            let is_final = i == stage_count - 1;

            let cutoff_hz = if is_final {
                FINAL_CUTOFF_HZ
            } else {
                NON_FINAL_CUTOFF_HZ
            };

            let num_taps = if i == 0 {
                compute_taps(current_rate, STAGE1_REF_RATE, STAGE1_REF_TAPS)
            } else {
                compute_taps(current_rate, FINAL_STAGE_REF_RATE, FINAL_STAGE_REF_TAPS)
            };

            stages.push(DecimationStage {
                decimation_factor: factor,
                cutoff_hz,
                num_taps,
                input_rate: current_rate,
            });

            current_rate /= factor as f32;
        }

        Ok(Self { stages })
    }

    /// Total decimation factor across all stages.
    pub fn total_decimation(&self) -> usize {
        self.stages.iter().map(|s| s.decimation_factor).product()
    }
}

/// Compute the number of FIR taps scaled proportionally from a reference.
///
/// Maintains the same transition bandwidth as the reference configuration
/// at different sample rates. Returns an odd number >= 51.
fn compute_taps(input_rate: f32, ref_rate: f32, ref_taps: f32) -> usize {
    let scaled = ref_taps * input_rate / ref_rate;
    let rounded = scaled.round() as usize;
    // Ensure odd and at least 51.
    let at_least_51 = rounded.max(51);
    if at_least_51.is_multiple_of(2) {
        at_least_51 + 1
    } else {
        at_least_51
    }
}

/// Preferred final-stage factors, ordered by filter quality.
/// Smaller factors in the final stage give better stopband attenuation
/// with fewer taps.
const PREFERRED_SMALL_FACTORS: &[usize] = &[10, 8, 5, 4, 3, 2];
const PREFERRED_LARGE_FACTORS: &[usize] = &[20, 15, 12, 10, 8, 5, 4, 3, 2];

/// Factor a total decimation into stages where each factor is <= 25.
///
/// Uses a two-pass approach: first tries final factors <= 10 for best
/// filter quality, then allows final factors up to 20. Falls back to
/// three stages if no two-stage decomposition works.
fn factor_into_stages(total: usize) -> Result<Vec<usize>, DecimationError> {
    if total <= MAX_STAGE_FACTOR {
        return Ok(vec![total]);
    }

    // Pass 1: two-stage with small final factor (<= 10).
    for &f in PREFERRED_SMALL_FACTORS {
        if total.is_multiple_of(f) && total / f <= MAX_STAGE_FACTOR {
            return Ok(vec![total / f, f]);
        }
    }

    // Pass 2: two-stage with larger final factor (<= 20).
    for &f in PREFERRED_LARGE_FACTORS {
        if total.is_multiple_of(f) && total / f <= MAX_STAGE_FACTOR {
            return Ok(vec![total / f, f]);
        }
    }

    // Pass 3: three-stage fallback.
    for &f3 in PREFERRED_SMALL_FACTORS {
        if total.is_multiple_of(f3) {
            let remaining = total / f3;
            for &f2 in PREFERRED_SMALL_FACTORS {
                if remaining.is_multiple_of(f2) && remaining / f2 <= MAX_STAGE_FACTOR {
                    return Ok(vec![remaining / f2, f2, f3]);
                }
            }
        }
    }

    Err(DecimationError::NoValidFactoring { total })
}

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
    filters: Vec<DecimatingFilter>,
    total_decimation: usize,
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
    ///
    /// Returns an error if the sample rate cannot be decimated to 24 kHz.
    pub fn new(config: PipelineConfig) -> Result<Self, DecimationError> {
        let decimation = DecimationConfig::compute(config.sample_rate)?;
        let output_rate = config.sample_rate as f32 / decimation.total_decimation() as f32;

        let filters: Vec<DecimatingFilter> = decimation
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

        Ok(Self {
            filters,
            total_decimation: decimation.total_decimation(),
            demod_path,
            status_deinterleaver: StatusDeinterleaver::new(),
            receiver: DataUnitReceiver::new(),
            synced: false,
            current_nac: Nac::new(0),
            sample_count: 0,
        })
    }

    /// Process one IQ sample through the full pipeline.
    ///
    /// Returns decoded protocol events, if any. Most samples produce
    /// no events (decimation, inter-symbol samples, buffering).
    pub fn process_sample(&mut self, sample: Complex<f32>) -> Option<ReceiverEvent> {
        self.sample_count += 1;

        // Chain through all decimation filter stages.
        let mut current = sample;
        for filter in &mut self.filters {
            current = filter.process(current)?;
        }
        let filtered = current;

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
            baseband_sample = self.sample_count / self.total_decimation as u64,
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
        let pipeline = ChannelPipeline::new(config).expect("2.4M should be valid");
        assert_eq!(pipeline.sample_count(), 0);
        assert_eq!(pipeline.current_nac(), Nac::new(0));
    }

    #[test]
    fn pipeline_constructs_cqpsk() {
        let config = PipelineConfig {
            sample_rate: 2_400_000,
            modulation: Modulation::Cqpsk,
        };
        let pipeline = ChannelPipeline::new(config).expect("2.4M should be valid");
        assert_eq!(pipeline.sample_count(), 0);
    }

    #[test]
    fn pipeline_processes_silence_without_events() {
        let config = PipelineConfig {
            sample_rate: 2_400_000,
            modulation: Modulation::Cqpsk,
        };
        let mut pipeline = ChannelPipeline::new(config).expect("2.4M should be valid");

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
        let mut pipeline = ChannelPipeline::new(config).expect("2.4M should be valid");

        // Feed pseudo-random noise to exercise the full path.
        for i in 0..50_000u32 {
            let phase = i as f32 * 0.73;
            let sample = Complex::new(phase.cos() * 0.1, phase.sin() * 0.1);
            let _ = pipeline.process_sample(sample);
        }

        assert_eq!(pipeline.sample_count(), 50_000);
    }

    #[test]
    fn decimation_config_2400k_regression() {
        let config = DecimationConfig::compute(2_400_000).expect("2.4M should be valid");
        assert_eq!(config.stages.len(), 2);
        assert_eq!(config.total_decimation(), 100);
        assert_eq!(config.stages[0].decimation_factor, 10);
        assert_eq!(config.stages[1].decimation_factor, 10);
        assert_eq!(config.stages[0].num_taps, 201);
        assert_eq!(config.stages[1].num_taps, 61);
        assert!((config.stages[0].cutoff_hz - 12_000.0).abs() < 0.01);
        assert!((config.stages[1].cutoff_hz - 6_250.0).abs() < 0.01);
        assert!((config.stages[0].input_rate - 2_400_000.0).abs() < 1.0);
        assert!((config.stages[1].input_rate - 240_000.0).abs() < 1.0);
    }

    #[test]
    fn decimation_config_rejects_invalid_rate() {
        assert!(DecimationConfig::compute(2_000_000).is_err());
        assert!(DecimationConfig::compute(0).is_err());
    }

    #[test]
    fn decimation_config_single_stage() {
        // 48000 / 24000 = 2, single stage.
        let config = DecimationConfig::compute(48_000).expect("48k should be valid");
        assert_eq!(config.stages.len(), 1);
        assert_eq!(config.total_decimation(), 2);
    }
}

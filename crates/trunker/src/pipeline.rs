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
use crate::p25::nid::NidIntegrityPolicy;
use crate::p25::receiver::{DataUnitReceiver, ReceiverEvent};
use crate::p25::status::{StatusDeinterleaver, StreamSymbol};
use crate::p25::types::{Dibit, Nac};

/// Target IF rate after all decimation: 24 kHz (5 samples/symbol at 4800 baud).
const CHANNEL_RATE: u32 = 24_000;

/// Default sync timeout for CC pipelines: 2500 post-decimation samples (~104 ms).
///
/// At the 24 kHz channel rate (5 samples per 4800-baud symbol), 2500 samples
/// equals 500 symbol periods. This is enough time for normal inter-data-unit
/// gaps but short enough to recover from a stuck PLL within a few hundred ms.
///
/// The timeout counts post-decimation samples (not symbols) so that it fires
/// even when the demodulator is unlocked and stops producing symbol events.
pub const CC_SYNC_TIMEOUT_SAMPLES: u32 = 24_000;

/// Reference filter parameters for first and final decimation stages.
///
/// This reference produces moderate tap counts that preserve CQPSK phase
/// fidelity. At the final stage's low sample rate, fewer taps prevent the
/// normalized cutoff from becoming so narrow that passband distortion
/// disrupts the Costas PLL lock.
const PRIMARY_REF_RATE: f32 = 2_400_000.0;
const PRIMARY_REF_TAPS: f32 = 201.0;

/// Reference filter parameters for intermediate decimation stages.
///
/// Intermediate stages use a reference that produces more taps (relative
/// to rate) for stronger stopband attenuation. This is necessary for
/// C4FM alias rejection at rates between the first and final stages.
const INTERMEDIATE_REF_RATE: f32 = 240_000.0;
const INTERMEDIATE_REF_TAPS: f32 = 61.0;

/// Low-pass filter cutoff frequency for all decimation stages (hertz).
///
/// Set to half the 12.5 kHz P25 channel bandwidth. The actual P25 signal
/// occupies only ~5760 Hz (4800 baud × 1.2 raised-cosine rolloff), so
/// 6250 Hz provides ample margin. Using the same cutoff for all stages
/// is critical for CQPSK: wider intermediate cutoffs let adjacent-channel
/// energy through that disrupts the Costas PLL phase tracking.
const CHANNEL_CUTOFF_HZ: f32 = 6_250.0;

/// Maximum decimation factor per stage. Kept at 10 to preserve CQPSK
/// phase fidelity — stages larger than 10x create filters whose narrow
/// normalized cutoff distorts phase, preventing the Costas loop from locking.
const MAX_STAGE_FACTOR: usize = 10;

/// Error type for decimation configuration.
#[derive(Debug, thiserror::Error)]
pub enum DecimationError {
    /// Sample rate is not an integer multiple of the target rate.
    #[error(
        "sample rate {sample_rate} Hz is not a multiple of {target_rate}; try {nearest_lower} or {nearest_higher}"
    )]
    NotDivisible {
        /// The invalid sample rate.
        sample_rate: u32,
        /// The target rate that must divide evenly.
        target_rate: u32,
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
        Self::compute_to(sample_rate, CHANNEL_RATE)
    }

    /// Compute decimation stages to reduce `sample_rate` to `target_rate`.
    ///
    /// Returns an error if `target_rate` does not evenly divide `sample_rate`
    /// or the total decimation cannot be factored into stages of 25x or less.
    ///
    /// When rates are equal (total decimation = 1), creates a single stage
    /// with decimation factor 1 that still applies the channel LPF. This
    /// filter is required for demodulators like the CQPSK Costas loop that
    /// expect band-limited input.
    pub fn compute_to(sample_rate: u32, target_rate: u32) -> Result<Self, DecimationError> {
        if sample_rate == 0 || target_rate == 0 {
            return Err(DecimationError::NotDivisible {
                sample_rate,
                target_rate: target_rate.max(1),
                nearest_lower: target_rate.max(1),
                nearest_higher: target_rate.max(1),
            });
        }

        if !sample_rate.is_multiple_of(target_rate) {
            let nearest_lower = (sample_rate / target_rate) * target_rate;
            let nearest_higher = nearest_lower + target_rate;
            return Err(DecimationError::NotDivisible {
                sample_rate,
                target_rate,
                nearest_lower,
                nearest_higher,
            });
        }

        let total = (sample_rate / target_rate) as usize;

        let factors = factor_into_stages(total)?;
        let stage_count = factors.len();

        let mut stages = Vec::with_capacity(stage_count);
        let mut current_rate = sample_rate as f32;

        for (i, &factor) in factors.iter().enumerate() {
            let is_intermediate = i > 0 && i < stage_count - 1;
            let num_taps = if is_intermediate {
                compute_taps(current_rate, INTERMEDIATE_REF_RATE, INTERMEDIATE_REF_TAPS)
            } else {
                compute_taps(current_rate, PRIMARY_REF_RATE, PRIMARY_REF_TAPS)
            };

            stages.push(DecimationStage {
                decimation_factor: factor,
                cutoff_hz: CHANNEL_CUTOFF_HZ,
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

/// Preferred factors, ordered by preference (largest first).
const PREFERRED_FACTORS: &[usize] = &[10, 8, 5, 4, 3, 2];

/// Factor a total decimation into stages where each factor is <= 10.
///
/// Places the largest factors first (earliest stages) where the sample
/// rate is highest. This minimizes total computation: a large first-stage
/// factor means the expensive convolution fires less often per input
/// sample. For example, at 3.6 MS/s with 150x total decimation:
///
/// - [3, 5, 10] (old): 303 taps / 3 = 101 MACs/sample → 438M MACs/sec
/// - [10, 5, 3] (new): 303 taps / 10 = 30 MACs/sample → 117M MACs/sec
fn factor_into_stages(total: usize) -> Result<Vec<usize>, DecimationError> {
    if total <= MAX_STAGE_FACTOR {
        return Ok(vec![total]);
    }

    for &f in PREFERRED_FACTORS {
        if total.is_multiple_of(f) {
            let remaining = total / f;
            if let Ok(mut later) = factor_into_stages(remaining) {
                later.insert(0, f);
                return Ok(later);
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
    /// NID integrity policy for accepting/rejecting data units.
    pub nid_integrity: NidIntegrityPolicy,
    /// Post-decimation samples without frame sync before resetting the
    /// demod for re-acquisition. `None` disables the timeout entirely.
    ///
    /// CC pipelines use [`CC_SYNC_TIMEOUT_SAMPLES`] (24000, ~1 s) to
    /// allow cold-start re-acquisition after PLL divergence. Voice channel pipelines should
    /// set `None` — they need 250 ms-1 s for initial acquisition, and
    /// zombie channels are handled by the acquisition timeout in
    /// [`ChannelManager`].
    pub sync_timeout_samples: Option<u32>,
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
    /// Post-decimation samples received while not synced. When this
    /// exceeds the configured timeout, the Costas loop and Gardner TED
    /// are reset to allow re-acquisition.
    unsynced_samples: u32,
    /// Sync timeout threshold in post-decimation samples (`None` = disabled).
    sync_timeout_samples: Option<u32>,
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

        Ok(Self {
            filters,
            total_decimation: decimation.total_decimation(),
            demod_path,
            status_deinterleaver: StatusDeinterleaver::new(),
            receiver: DataUnitReceiver::new(config.nid_integrity),
            synced: false,
            current_nac: Nac::new(0),
            sample_count: 0,
            unsynced_samples: 0,
            sync_timeout_samples: config.sync_timeout_samples,
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

        // Check sync timeout on every post-decimation sample so it fires
        // even when the demodulator is unlocked and stops producing symbols.
        if !self.synced
            && let Some(timeout) = self.sync_timeout_samples
        {
            self.unsynced_samples += 1;
            if self.unsynced_samples >= timeout {
                self.reset_tracking();
            }
        }

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
                self.unsynced_samples = 0;
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

    /// Return the Costas loop state from a CQPSK pipeline.
    ///
    /// Returns `Some((phase, frequency))` for CQPSK pipelines, `None`
    /// for C4FM (which has no Costas loop).
    pub fn costas_state(&self) -> Option<(f32, f32)> {
        match &self.demod_path {
            DemodPath::Cqpsk { demod } => Some(demod.costas_state()),
            DemodPath::C4fm { .. } => None,
        }
    }

    /// Return the Costas loop frequency from a CQPSK pipeline.
    ///
    /// Returns `0.0` for C4FM pipelines (no Costas loop).
    pub fn costas_frequency(&self) -> f32 {
        self.costas_state().map_or(0.0, |(_, freq)| freq)
    }

    /// Seed the Costas loop with phase and frequency from another
    /// locked pipeline (e.g., the control channel).
    ///
    /// No-op for C4FM pipelines.
    pub fn seed_costas(&mut self, phase: f32, frequency: f32) {
        if let DemodPath::Cqpsk { demod } = &mut self.demod_path {
            demod.seed_costas(phase, frequency);
        }
    }

    /// Reset the entire signal path for re-acquisition.
    ///
    /// Clears decimation filter delay lines and all demodulator state
    /// so stale samples don't prevent the Costas loop from converging.
    fn reset_tracking(&mut self) {
        // Log diagnostics BEFORE reset so we can see what state the demod was in.
        match &self.demod_path {
            DemodPath::Cqpsk { demod } => {
                let (agc_gain, costas_phase, costas_freq, locked) = demod.diagnostics();
                tracing::info!(
                    sample = self.sample_count,
                    unsynced_samples = self.unsynced_samples,
                    agc_gain = format_args!("{agc_gain:.2}"),
                    costas_phase = format_args!("{costas_phase:.4}"),
                    costas_freq = format_args!("{costas_freq:.6}"),
                    locked,
                    "CC sync timeout: resetting demodulator"
                );
            }
            DemodPath::C4fm { .. } => {
                tracing::info!(
                    sample = self.sample_count,
                    unsynced_samples = self.unsynced_samples,
                    "CC sync timeout: resetting demodulator"
                );
            }
        }
        for filter in &mut self.filters {
            filter.reset();
        }
        match &mut self.demod_path {
            DemodPath::Cqpsk { demod } => demod.reset_tracking(),
            DemodPath::C4fm {
                demod: _,
                dc_block,
                rrc,
                timing,
            } => {
                *dc_block = DcBlocker::default();
                rrc.reset();
                *timing = SymbolTiming::new();
            }
        }
        self.unsynced_samples = 0;
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
            nid_integrity: NidIntegrityPolicy::default(),
            sync_timeout_samples: None,
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
            nid_integrity: NidIntegrityPolicy::default(),
            sync_timeout_samples: None,
        };
        let pipeline = ChannelPipeline::new(config).expect("2.4M should be valid");
        assert_eq!(pipeline.sample_count(), 0);
    }

    #[test]
    fn pipeline_processes_silence_without_events() {
        let config = PipelineConfig {
            sample_rate: 2_400_000,
            modulation: Modulation::Cqpsk,
            nid_integrity: NidIntegrityPolicy::default(),
            sync_timeout_samples: None,
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
            nid_integrity: NidIntegrityPolicy::default(),
            sync_timeout_samples: None,
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
        assert_eq!(config.stages[1].num_taps, 51);
        assert!((config.stages[0].cutoff_hz - 6_250.0).abs() < 0.01);
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

    // -- compute_to tests --

    #[test]
    fn compute_to_2400k_to_48k() {
        let config =
            DecimationConfig::compute_to(2_400_000, 48_000).expect("2.4M->48k should be valid");
        assert_eq!(config.total_decimation(), 50);
    }

    #[test]
    fn compute_to_matches_compute_for_24k_target() {
        let via_compute = DecimationConfig::compute(2_400_000).unwrap();
        let via_compute_to = DecimationConfig::compute_to(2_400_000, 24_000).unwrap();

        assert_eq!(via_compute.stages.len(), via_compute_to.stages.len());
        assert_eq!(
            via_compute.total_decimation(),
            via_compute_to.total_decimation()
        );
        for (a, b) in via_compute.stages.iter().zip(via_compute_to.stages.iter()) {
            assert_eq!(a.decimation_factor, b.decimation_factor);
            assert_eq!(a.num_taps, b.num_taps);
            assert!((a.cutoff_hz - b.cutoff_hz).abs() < 0.01);
            assert!((a.input_rate - b.input_rate).abs() < 1.0);
        }
    }

    #[test]
    fn compute_to_rejects_indivisible_target() {
        let result = DecimationConfig::compute_to(2_400_000, 44_100);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("not a multiple of 44100"),
            "error should mention target rate, got: {err_msg}"
        );
    }

    #[test]
    fn compute_to_identity_applies_lpf() {
        // When sample_rate == target_rate, we still get a filter stage
        // (factor 1) so the channel LPF is applied. This is essential
        // for CQPSK demod which requires band-limited input.
        let config = DecimationConfig::compute_to(24_000, 24_000).expect("identity should work");
        assert_eq!(config.stages.len(), 1);
        assert_eq!(config.total_decimation(), 1);
        assert_eq!(config.stages[0].decimation_factor, 1);
        assert!((config.stages[0].cutoff_hz - CHANNEL_CUTOFF_HZ).abs() < 0.01);
    }

    #[test]
    fn decimation_config_6000k_three_stages() {
        // 6 MSPS = 250x total: 10x -> 5x -> 5x.
        // Largest factor first minimizes computation at the highest rate.
        let config = DecimationConfig::compute(6_000_000).expect("6M should be valid");
        assert_eq!(config.stages.len(), 3);
        assert_eq!(config.total_decimation(), 250);
        assert_eq!(config.stages[0].decimation_factor, 10);
        assert_eq!(config.stages[1].decimation_factor, 5);
        assert_eq!(config.stages[2].decimation_factor, 5);
        // All stages use the same channel cutoff.
        for stage in &config.stages {
            assert!((stage.cutoff_hz - CHANNEL_CUTOFF_HZ).abs() < 0.01);
        }
        assert!((config.stages[0].input_rate - 6_000_000.0).abs() < 1.0);
        assert!((config.stages[1].input_rate - 600_000.0).abs() < 1.0);
        assert!((config.stages[2].input_rate - 120_000.0).abs() < 1.0);
    }

    #[test]
    fn decimation_config_3000k_three_stages() {
        // 3 MSPS = 125x total: 5x -> 5x -> 5x.
        let config = DecimationConfig::compute(3_000_000).expect("3M should be valid");
        assert_eq!(config.stages.len(), 3);
        assert_eq!(config.total_decimation(), 125);
        assert_eq!(config.stages[0].decimation_factor, 5);
        assert_eq!(config.stages[1].decimation_factor, 5);
        assert_eq!(config.stages[2].decimation_factor, 5);
    }

    #[test]
    fn decimation_config_9000k_four_stages() {
        // 9 MSPS = 375x total: 5x -> 5x -> 5x -> 3x.
        let config = DecimationConfig::compute(9_000_000).expect("9M should be valid");
        assert_eq!(config.stages.len(), 4);
        assert_eq!(config.total_decimation(), 375);
        assert_eq!(config.stages[0].decimation_factor, 5);
        assert_eq!(config.stages[1].decimation_factor, 5);
        assert_eq!(config.stages[2].decimation_factor, 5);
        assert_eq!(config.stages[3].decimation_factor, 3);
    }

    #[test]
    fn decimation_config_3600k_largest_factor_first() {
        // 3.6 MSPS = 150x total: 10x -> 5x -> 3x.
        // Putting 10x first at 3.6 MS/s gives 303 taps / 10 = 30 MACs/sample.
        // The old order [3, 5, 10] gave 303 taps / 3 = 101 MACs/sample (3.3x worse).
        let config = DecimationConfig::compute(3_600_000).expect("3.6M should be valid");
        assert_eq!(config.stages.len(), 3);
        assert_eq!(config.total_decimation(), 150);
        assert_eq!(config.stages[0].decimation_factor, 10);
        assert_eq!(config.stages[1].decimation_factor, 5);
        assert_eq!(config.stages[2].decimation_factor, 3);
    }

    #[test]
    fn factor_into_stages_no_factor_exceeds_10() {
        // Verify all standard P25-compatible sample rates factor cleanly.
        let rates = [
            48_000, 240_000, 480_000, 1_200_000, 2_400_000, 3_000_000, 6_000_000, 9_000_000,
        ];
        for rate in rates {
            let total = (rate / CHANNEL_RATE) as usize;
            let factors = factor_into_stages(total).unwrap_or_else(|e| {
                panic!("rate {rate} (total {total}) should factor: {e}");
            });
            for &f in &factors {
                assert!(f <= 10, "rate {rate}: factor {f} exceeds 10");
            }
            let product: usize = factors.iter().product();
            assert_eq!(product, total, "rate {rate}: product mismatch");
        }
    }
}

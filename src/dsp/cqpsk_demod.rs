//! CQPSK demodulation pipeline for P25 simulcast.
//!
//! Assembles the CQPSK sub-blocks into a complete demodulation path:
//! AGC -> ComplexRRC -> Gardner -> DiffDecoder -> rescale -> slicer
//!
//! Input: complex IQ samples at the IF rate (24 kHz).
//! Output: frame sync events and dibits (same `SymbolEvent` as C4FM).

use num_complex::Complex;

use super::agc::Agc;
use super::diff_decoder::DifferentialDecoder;
use super::gardner::GardnerTed;
use super::rrc_filter::ComplexRrcFilter;
use super::slicer::DibitSlicer;
use super::sync::SyncDetector;
use super::timing::SymbolEvent;

/// Number of dibits in the frame sync pattern.
const SYNC_DIBITS: usize = 24;

/// Indices of dibit 01 (+3 deviation) positions in the sync pattern.
const SYNC_POSITIVE_INDICES: [usize; 11] = [0, 1, 2, 3, 4, 6, 7, 10, 11, 16, 18];

/// Indices of dibit 11 (-3 deviation) positions in the sync pattern.
const SYNC_NEGATIVE_INDICES: [usize; 13] =
    [5, 8, 9, 12, 13, 14, 15, 17, 19, 20, 21, 22, 23];

/// CQPSK demodulator for P25 simulcast signals.
///
/// Processes complex IQ samples at the IF rate and produces
/// synchronized dibits compatible with the protocol decoder.
#[derive(Debug)]
pub struct CqpskDemodulator {
    agc: Agc,
    rrc: ComplexRrcFilter,
    gardner: GardnerTed,
    diff_decoder: DifferentialDecoder,
    slicer: DibitSlicer,
    sync_detector: SyncDetector,
    locked: bool,
    /// Symbol-rate samples (rescaled to 4-level) for sync calibration.
    sync_samples: Vec<f32>,
}

impl CqpskDemodulator {
    /// Create a new CQPSK demodulator with default P25 parameters.
    pub fn new() -> Self {
        // Bootstrap slicer with expected output levels.
        // After rescaling, the 4 constellation points map to +/-3, +/-1.
        let mut slicer = DibitSlicer::new();
        slicer.calibrate(3.0, -3.0);

        Self {
            agc: Agc::default(),
            rrc: ComplexRrcFilter::new(4800.0, 24_000.0, 0.2, 5),
            gardner: GardnerTed::new(),
            diff_decoder: DifferentialDecoder::new(),
            slicer,
            sync_detector: SyncDetector::new(),
            locked: false,
            sync_samples: Vec::with_capacity(SYNC_DIBITS * 2),
        }
    }

    /// Whether the demodulator has locked to a frame sync.
    pub fn is_locked(&self) -> bool {
        self.locked
    }

    /// Return the current slicer thresholds as (upper, mid, lower).
    pub fn slicer_thresholds(&self) -> (f32, f32, f32) {
        self.slicer.thresholds()
    }

    /// Process one complex IF sample through the CQPSK chain.
    ///
    /// Returns `Some(SymbolEvent)` at symbol boundaries, or `None`
    /// for inter-symbol samples.
    pub fn process(&mut self, sample: Complex<f32>) -> Option<SymbolEvent> {
        // AGC -> RRC -> Gardner (produces symbol-rate output).
        let normalized = self.agc.process(sample);
        let filtered = self.rrc.process(normalized);
        let symbol = self.gardner.process(filtered)?;

        // DiffDecoder -> rescale to 4-level.
        let diff = self.diff_decoder.process(symbol);
        let phase_dibit = diff.bits();

        // The differential decoder already gives us dibits.
        // But we also need the 4-level value for slicer calibration
        // from the sync pattern. Map dibit to level:
        let level = dibit_to_level(phase_dibit);

        self.sync_samples.push(level);

        // Feed to sync detector.
        if self.sync_detector.feed(diff) {
            self.locked = true;
            self.calibrate_from_sync();
            return Some(SymbolEvent::SyncDetected);
        }

        if self.locked {
            if self.sync_samples.len() > SYNC_DIBITS * 2 {
                self.sync_samples
                    .drain(..self.sync_samples.len() - SYNC_DIBITS);
            }
            return Some(SymbolEvent::Symbol(diff));
        }

        None
    }

    /// Calibrate slicer thresholds from the sync pattern samples.
    fn calibrate_from_sync(&mut self) {
        if self.sync_samples.len() < SYNC_DIBITS {
            return;
        }

        let start = self.sync_samples.len() - SYNC_DIBITS;
        let sync_slice = &self.sync_samples[start..];

        let positive_sum: f32 = SYNC_POSITIVE_INDICES.iter().map(|&i| sync_slice[i]).sum();
        let negative_sum: f32 = SYNC_NEGATIVE_INDICES.iter().map(|&i| sync_slice[i]).sum();

        let positive_avg = positive_sum / SYNC_POSITIVE_INDICES.len() as f32;
        let negative_avg = negative_sum / SYNC_NEGATIVE_INDICES.len() as f32;

        self.slicer.calibrate(positive_avg, negative_avg);
    }
}

impl Default for CqpskDemodulator {
    fn default() -> Self {
        Self::new()
    }
}

/// Map a dibit to its corresponding 4-level deviation value.
///
/// Matches C4FM deviation mapping for compatibility with the slicer
/// and sync calibration.
fn dibit_to_level(dibit_bits: u8) -> f32 {
    match dibit_bits {
        0b01 => 3.0,
        0b00 => 1.0,
        0b10 => -1.0,
        _ => -3.0, // 0b11
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demodulator_produces_events() {
        let mut demod = CqpskDemodulator::new();

        // Feed enough samples to see some output (even if no sync).
        let mut event_count = 0;
        for i in 0..1000 {
            let phase = i as f32 * 0.3;
            let sample = Complex::from_polar(1.0, phase);
            if demod.process(sample).is_some() {
                event_count += 1;
            }
        }

        // Gardner should produce ~1000/5 = 200 symbols, but since
        // the signal is not a valid P25 signal, no syncs will be found
        // and no events will be emitted (locked == false).
        // This test verifies the pipeline doesn't crash.
        assert_eq!(event_count, 0, "random signal should not produce events");
    }

    #[test]
    fn dibit_to_level_mapping() {
        assert!((dibit_to_level(0b01) - 3.0).abs() < 1e-6);
        assert!((dibit_to_level(0b00) - 1.0).abs() < 1e-6);
        assert!((dibit_to_level(0b10) - (-1.0)).abs() < 1e-6);
        assert!((dibit_to_level(0b11) - (-3.0)).abs() < 1e-6);
    }

    #[test]
    fn default_slicer_thresholds_are_set() {
        let demod = CqpskDemodulator::new();
        let (upper, mid, lower) = demod.slicer_thresholds();
        // Calibrated with positive=3.0, negative=-3.0
        assert!((mid - 0.0).abs() < 1e-6, "mid should be 0: got {mid}");
        assert!(upper > 0.0, "upper should be positive: got {upper}");
        assert!(lower < 0.0, "lower should be negative: got {lower}");
    }
}

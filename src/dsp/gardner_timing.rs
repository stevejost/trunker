//! Adaptive symbol timing recovery using Gardner TED.
//!
//! Replaces fixed-phase timing with a Gardner timing error detector
//! and PI loop filter for continuous symbol clock tracking. Integrates
//! with frame sync detection and slicer calibration.
//!
//! This module operates on real-valued baseband samples (post FM
//! demodulation) for the C4FM pipeline path. For CQPSK, use the
//! complex-valued [`super::gardner::GardnerTed`] directly.

use super::interpolator::lagrange4_real;
use super::slicer::DibitSlicer;
use super::sync::SyncDetector;
use super::timing::SymbolEvent;

/// Nominal samples per symbol at 24 kHz sample rate, 4800 baud.
const NOMINAL_OMEGA: f32 = 5.0;

/// Proportional gain for the timing loop (controls mu).
const GAIN_MU: f32 = 0.025;

/// Integral gain for the timing loop (controls omega).
const GAIN_OMEGA: f32 = GAIN_MU * GAIN_MU / 4.0;

/// Maximum relative deviation of omega from nominal.
const OMEGA_RELATIVE_LIMIT: f32 = 0.002;

/// Size of the sample delay line. Must hold enough samples for
/// interpolation at both on-time and mid-point offsets.
const DELAY_LINE_SIZE: usize = 16;

/// Number of dibits in the frame sync pattern.
const SYNC_DIBITS: usize = 24;

/// Indices of dibit 01 (+3 deviation) positions in the sync pattern.
const SYNC_POSITIVE_INDICES: [usize; 11] = [0, 1, 2, 3, 4, 6, 7, 10, 11, 16, 18];

/// Indices of dibit 11 (-3 deviation) positions in the sync pattern.
const SYNC_NEGATIVE_INDICES: [usize; 13] = [5, 8, 9, 12, 13, 14, 15, 17, 19, 20, 21, 22, 23];

/// Adaptive symbol timing recovery using Gardner TED.
///
/// Processes real-valued baseband samples at 24 kHz and produces
/// synchronized dibits. Uses the Gardner timing error detector with
/// a PI loop filter for continuous timing tracking, and calibrates
/// the slicer from frame sync patterns.
#[derive(Debug)]
pub struct GardnerSymbolTiming {
    slicer: DibitSlicer,
    sync_detector: SyncDetector,
    /// Circular delay line of baseband samples.
    delay_line: [f32; DELAY_LINE_SIZE],
    /// Write position in the delay line.
    write_index: usize,
    /// Fractional sample offset. Symbol decision when mu <= 0.
    mu: f32,
    /// Estimated samples per symbol.
    omega: f32,
    /// Center value for omega clamping.
    omega_mid: f32,
    /// Previous on-time sample for Gardner error computation.
    last_sample: f32,
    /// Whether we've locked to a frame sync.
    locked: bool,
    /// Symbol-rate samples collected for sync calibration.
    sync_samples: Vec<f32>,
}

impl GardnerSymbolTiming {
    /// Create a new adaptive symbol timing block.
    pub fn new() -> Self {
        Self {
            slicer: DibitSlicer::new(),
            sync_detector: SyncDetector::new(),
            delay_line: [0.0; DELAY_LINE_SIZE],
            write_index: 0,
            mu: 0.0,
            omega: NOMINAL_OMEGA,
            omega_mid: NOMINAL_OMEGA,
            last_sample: 0.0,
            locked: false,
            sync_samples: Vec::with_capacity(SYNC_DIBITS * 2),
        }
    }

    /// Set initial slicer thresholds before sync is acquired.
    pub fn set_initial_thresholds(&mut self, positive_avg: f32, negative_avg: f32) {
        self.slicer.calibrate(positive_avg, negative_avg);
    }

    /// Whether timing is currently locked to a frame sync.
    pub fn is_locked(&self) -> bool {
        self.locked
    }

    /// Return the current slicer thresholds as (upper, mid, lower).
    pub fn slicer_thresholds(&self) -> (f32, f32, f32) {
        self.slicer.thresholds()
    }

    /// Process one baseband sample.
    ///
    /// Returns `Some(SymbolEvent)` at symbol boundaries, or `None`
    /// for inter-symbol samples.
    pub fn process(&mut self, sample: f32) -> Option<SymbolEvent> {
        self.push_sample(sample);
        self.mu -= 1.0;

        if self.mu > 0.0 {
            return None;
        }

        // Interpolate on-time and mid-point samples.
        let (half_whole, half_frac) = self.half_omega_offset();
        let on_time = self.interpolate_at(half_whole, half_frac);
        let midpoint = self.interpolate_at(0, self.fractional_mu());

        // Gardner timing error (real-valued).
        let error = self.compute_error(on_time, midpoint);
        self.last_sample = on_time;
        self.update_loop(error, on_time);

        // Slice to dibit and check for sync.
        let dibit = self.slicer.slice(on_time);
        self.sync_samples.push(on_time);

        if self.sync_detector.feed(dibit) {
            self.locked = true;
            self.calibrate_from_sync();
            return Some(SymbolEvent::SyncDetected);
        }

        if self.locked {
            if self.sync_samples.len() > SYNC_DIBITS * 2 {
                self.sync_samples
                    .drain(..self.sync_samples.len() - SYNC_DIBITS);
            }
            return Some(SymbolEvent::Symbol(dibit));
        }

        None
    }

    /// Push a sample into the circular delay line.
    fn push_sample(&mut self, sample: f32) {
        self.delay_line[self.write_index] = sample;
        self.write_index = (self.write_index + 1) % DELAY_LINE_SIZE;
    }

    /// Compute the fractional mu for interpolation (0.0..1.0).
    fn fractional_mu(&self) -> f32 {
        self.mu + self.mu.floor().abs()
    }

    /// Compute the whole-sample and fractional offsets for the half-symbol point.
    fn half_omega_offset(&self) -> (usize, f32) {
        let half = self.omega / 2.0;
        let whole = half.floor() as usize;
        let frac = self.mu + half - whole as f32;
        if frac > 1.0 {
            (whole + 1, frac - 1.0)
        } else {
            (whole, frac)
        }
    }

    /// Interpolate from the delay line using Lagrange 4-point.
    fn interpolate_at(&self, offset: usize, mu: f32) -> f32 {
        let base = (self.write_index + DELAY_LINE_SIZE - 1 - offset) % DELAY_LINE_SIZE;
        let samples = [
            self.delay_line[(base + DELAY_LINE_SIZE - 1) % DELAY_LINE_SIZE],
            self.delay_line[base],
            self.delay_line[(base + 1) % DELAY_LINE_SIZE],
            self.delay_line[(base + 2) % DELAY_LINE_SIZE],
        ];
        lagrange4_real(samples, mu)
    }

    /// Compute the Gardner timing error for real-valued samples.
    fn compute_error(&self, on_time: f32, midpoint: f32) -> f32 {
        let error = (self.last_sample - on_time) * midpoint;
        if error.is_nan() {
            0.0
        } else {
            error.clamp(-1.0, 1.0)
        }
    }

    /// Update omega and mu via the PI loop filter.
    fn update_loop(&mut self, error: f32, sample: f32) {
        self.omega += GAIN_OMEGA * error * sample.abs();
        let deviation =
            (self.omega - self.omega_mid).clamp(-OMEGA_RELATIVE_LIMIT, OMEGA_RELATIVE_LIMIT);
        self.omega = self.omega_mid + deviation;
        self.mu += self.omega + GAIN_MU * error;
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

impl Default for GardnerSymbolTiming {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::super::sync::sync_pattern_to_dibits;
    use super::*;
    use crate::p25::types::Dibit;

    const SAMPLES_PER_SYMBOL: usize = 5;

    /// Build a baseband signal from a dibit sequence at 5 samples/symbol.
    fn dibits_to_baseband(dibits: &[Dibit]) -> Vec<f32> {
        dibits
            .iter()
            .flat_map(|d| {
                let level = match d.bits() {
                    0b01 => 3.0,
                    0b00 => 1.0,
                    0b10 => -1.0,
                    0b11 => -3.0,
                    _ => 0.0,
                };
                std::iter::repeat_n(level, SAMPLES_PER_SYMBOL)
            })
            .collect()
    }

    fn sync_dibits() -> Vec<Dibit> {
        sync_pattern_to_dibits().to_vec()
    }

    #[test]
    fn detects_sync_in_clean_signal() {
        let mut timing = GardnerSymbolTiming::new();
        timing.set_initial_thresholds(3.0, -3.0);

        let pattern = sync_dibits();
        let baseband = dibits_to_baseband(&pattern);

        let mut found_sync = false;
        for sample in baseband {
            if let Some(SymbolEvent::SyncDetected) = timing.process(sample) {
                found_sync = true;
            }
        }
        assert!(found_sync, "should detect sync in clean signal");
    }

    #[test]
    fn extracts_symbols_after_sync() {
        let mut timing = GardnerSymbolTiming::new();
        timing.set_initial_thresholds(3.0, -3.0);

        let sync = sync_dibits();
        let data = vec![
            Dibit::new(0b01),
            Dibit::new(0b00),
            Dibit::new(0b10),
            Dibit::new(0b11),
        ];

        let mut all_dibits: Vec<Dibit> = sync;
        all_dibits.extend_from_slice(&data);
        let baseband = dibits_to_baseband(&all_dibits);

        let mut symbols = Vec::new();
        for sample in baseband {
            match timing.process(sample) {
                Some(SymbolEvent::Symbol(d)) => symbols.push(d),
                _ => {}
            }
        }

        assert!(
            symbols.len() >= 4,
            "expected >= 4 symbols, got {}",
            symbols.len()
        );

        let last_four: Vec<_> = symbols.iter().rev().take(4).rev().collect();
        assert_eq!(last_four[0].bits(), 0b01);
        assert_eq!(last_four[1].bits(), 0b00);
        assert_eq!(last_four[2].bits(), 0b10);
        assert_eq!(last_four[3].bits(), 0b11);
    }

    #[test]
    fn locks_timing_after_sync() {
        let mut timing = GardnerSymbolTiming::new();
        timing.set_initial_thresholds(3.0, -3.0);

        let pattern = sync_dibits();
        let baseband = dibits_to_baseband(&pattern);

        for sample in baseband {
            timing.process(sample);
        }

        assert!(timing.is_locked(), "should be locked after sync");
    }

    #[test]
    fn detects_sync_with_timing_offset() {
        let mut timing = GardnerSymbolTiming::new();
        timing.set_initial_thresholds(3.0, -3.0);

        let pattern = sync_dibits();
        let baseband = dibits_to_baseband(&pattern);

        // Prepend 2 samples to shift timing offset.
        let mut shifted = vec![0.0; 2];
        shifted.extend_from_slice(&baseband);

        let mut found_sync = false;
        for sample in shifted {
            if let Some(SymbolEvent::SyncDetected) = timing.process(sample) {
                found_sync = true;
            }
        }
        assert!(found_sync, "should detect sync despite timing offset");
    }

    #[test]
    fn omega_stays_near_nominal() {
        let mut timing = GardnerSymbolTiming::new();
        timing.set_initial_thresholds(3.0, -3.0);

        let pattern = sync_dibits();
        let data: Vec<Dibit> = (0..100).map(|i| Dibit::new((i % 4) as u8)).collect();

        let mut all_dibits = pattern;
        all_dibits.extend_from_slice(&data);
        let baseband = dibits_to_baseband(&all_dibits);

        for sample in baseband {
            timing.process(sample);
        }

        assert!(
            (timing.omega - NOMINAL_OMEGA).abs() < 0.02,
            "omega drifted: expected ~{}, got {}",
            NOMINAL_OMEGA,
            timing.omega
        );
    }
}

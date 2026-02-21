//! Dibit slicer: maps FM demodulator output to 2-bit symbols.
//!
//! P25 C4FM uses four deviation levels mapped to dibits per
//! TIA-102.BAAA Table 4. This module slices baseband samples
//! into dibits using three amplitude thresholds.

use crate::p25::types::Dibit;

/// C4FM symbol-to-dibit mapping (TIA-102.BAAA Table 4).
///
/// - +3 deviation (1800 Hz) -> dibit 01
/// - +1 deviation (600 Hz)  -> dibit 00
/// - -1 deviation (600 Hz)  -> dibit 10
/// - -3 deviation (1800 Hz) -> dibit 11
const DIBIT_POSITIVE_OUTER: Dibit = Dibit::new(0b01);
const DIBIT_POSITIVE_INNER: Dibit = Dibit::new(0b00);
const DIBIT_NEGATIVE_INNER: Dibit = Dibit::new(0b10);
const DIBIT_NEGATIVE_OUTER: Dibit = Dibit::new(0b11);

/// Slices baseband FM demodulator output into P25 dibits.
///
/// Uses three thresholds to partition the amplitude range into four
/// decision regions corresponding to the four C4FM deviation levels.
#[derive(Debug)]
pub struct DibitSlicer {
    /// Threshold between +3 and +1 levels.
    upper: f32,
    /// Threshold between +1 and -1 levels (zero crossing).
    mid: f32,
    /// Threshold between -1 and -3 levels.
    lower: f32,
}

impl DibitSlicer {
    /// Create a slicer with fixed, evenly-spaced thresholds.
    ///
    /// Suitable for clean recordings. For noisy signals, use
    /// adaptive thresholds calibrated from the frame sync pattern.
    pub fn new() -> Self {
        Self {
            upper: 0.0,
            mid: 0.0,
            lower: 0.0,
        }
    }

    /// Calibrate thresholds from measured outer deviation levels.
    ///
    /// `positive_avg` is the average sample value at +3 deviation.
    /// `negative_avg` is the average sample value at -3 deviation.
    /// The inner thresholds are placed at 2/3 of the way from mid
    /// to the outer levels, matching the 3:1 deviation ratio.
    pub fn calibrate(&mut self, positive_avg: f32, negative_avg: f32) {
        self.mid = (positive_avg + negative_avg) / 2.0;
        self.upper = self.mid + (positive_avg - self.mid) * 2.0 / 3.0;
        self.lower = self.mid + (negative_avg - self.mid) * 2.0 / 3.0;
    }

    /// Return the current thresholds as (upper, mid, lower).
    pub fn thresholds(&self) -> (f32, f32, f32) {
        (self.upper, self.mid, self.lower)
    }

    /// Slice a single baseband sample into a dibit.
    pub fn slice(&self, sample: f32) -> Dibit {
        if sample > self.upper {
            DIBIT_POSITIVE_OUTER
        } else if sample > self.mid {
            DIBIT_POSITIVE_INNER
        } else if sample > self.lower {
            DIBIT_NEGATIVE_INNER
        } else {
            DIBIT_NEGATIVE_OUTER
        }
    }
}

impl Default for DibitSlicer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_thresholds_all_four_levels() {
        // With default thresholds at 0, the slicer splits at 0.
        // We need to calibrate to get meaningful boundaries.
        let mut slicer = DibitSlicer::new();
        slicer.calibrate(1.0, -1.0);

        // +3 deviation (above upper threshold)
        assert_eq!(slicer.slice(0.9).bits(), 0b01);
        // +1 deviation (between mid and upper)
        assert_eq!(slicer.slice(0.2).bits(), 0b00);
        // -1 deviation (between lower and mid)
        assert_eq!(slicer.slice(-0.2).bits(), 0b10);
        // -3 deviation (below lower threshold)
        assert_eq!(slicer.slice(-0.9).bits(), 0b11);
    }

    #[test]
    fn calibrate_symmetric() {
        let mut slicer = DibitSlicer::new();
        slicer.calibrate(3.0, -3.0);

        // mid should be 0
        assert!((slicer.mid).abs() < 1e-6);
        // upper should be 2.0 (0 + 3 * 2/3)
        assert!((slicer.upper - 2.0).abs() < 1e-6);
        // lower should be -2.0 (0 + (-3) * 2/3)
        assert!((slicer.lower - (-2.0)).abs() < 1e-6);
    }

    #[test]
    fn calibrate_asymmetric_offset() {
        let mut slicer = DibitSlicer::new();
        // Simulate DC offset: +3 maps to 4.0, -3 maps to -2.0
        slicer.calibrate(4.0, -2.0);

        // mid = (4 + (-2)) / 2 = 1.0
        assert!((slicer.mid - 1.0).abs() < 1e-6);
        // upper = 1.0 + (4.0 - 1.0) * 2/3 = 1.0 + 2.0 = 3.0
        assert!((slicer.upper - 3.0).abs() < 1e-6);
        // lower = 1.0 + (-2.0 - 1.0) * 2/3 = 1.0 - 2.0 = -1.0
        assert!((slicer.lower - (-1.0)).abs() < 1e-6);
    }

    #[test]
    fn dibit_mapping_matches_tia102() {
        // Verify the constant dibit values match TIA-102.BAAA Table 4.
        assert_eq!(DIBIT_POSITIVE_OUTER.bits(), 0b01); // +3
        assert_eq!(DIBIT_POSITIVE_INNER.bits(), 0b00); // +1
        assert_eq!(DIBIT_NEGATIVE_INNER.bits(), 0b10); // -1
        assert_eq!(DIBIT_NEGATIVE_OUTER.bits(), 0b11); // -3
    }

    #[test]
    fn boundary_values() {
        let mut slicer = DibitSlicer::new();
        slicer.calibrate(1.0, -1.0);

        // Exactly at mid threshold (0.0) should go to negative inner
        assert_eq!(slicer.slice(0.0).bits(), 0b10);
        // Exactly at upper threshold should go to positive inner
        let at_upper = slicer.upper;
        assert_eq!(slicer.slice(at_upper).bits(), 0b00);
        // Just above upper
        assert_eq!(slicer.slice(at_upper + 0.001).bits(), 0b01);
    }
}

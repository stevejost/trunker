//! IMBE frame parameters derived from the quantized pitch period.
//!
//! The base parameters (fundamental frequency, harmonic count, band count)
//! are computed from the 8-bit pitch period value b_0 using equations 46-48
//! of TIA-102.BABA.

use std::cmp::min;
use std::f32::consts::PI;

/// Base parameters of an IMBE voice frame.
///
/// Derived from the quantized pitch period b_0, these control the spectral
/// structure of the frame: how many harmonics to synthesize and how many
/// frequency bands to classify as voiced or unvoiced.
#[derive(Debug, Copy, Clone)]
pub struct BaseParams {
    /// Fundamental frequency w_0 in radians per sample.
    pub fundamental_frequency: f32,
    /// Number of harmonics L of the fundamental frequency (9..=56).
    pub harmonic_count: u32,
    /// Number of frequency bands K, each containing up to 3 harmonics (1..=12).
    ///
    /// Each band contains 3 harmonics of the fundamental frequency, except
    /// the last which may contain fewer.
    pub band_count: u32,
}

impl BaseParams {
    /// Create a new `BaseParams` from the quantized 8-bit pitch period b_0.
    ///
    /// Computes fundamental frequency (Eq 46), harmonic count (Eq 47),
    /// and band count (Eq 48) per TIA-102.BABA.
    pub fn new(period: u8) -> Self {
        Self::from_period(period as f32)
    }

    /// Compute base parameters from a floating-point pitch period.
    fn from_period(period: f32) -> Self {
        // Eq 46: fundamental frequency w_0
        let fundamental_frequency = 4.0 * PI / (period + 39.5);
        // Eq 47: number of harmonics L
        let harmonic_count = (0.9254 * (PI / fundamental_frequency + 0.25).floor()) as u32;
        // Eq 48: number of frequency bands K
        let band_count = min(harmonic_count.div_ceil(3), 12);

        Self {
            fundamental_frequency,
            harmonic_count,
            band_count,
        }
    }
}

impl Default for BaseParams {
    /// Initial default parameters per TIA-102.BABA section on decoder reset.
    fn default() -> Self {
        Self {
            fundamental_frequency: 0.02985 * PI,
            harmonic_count: 30,
            band_count: 10,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_207_gives_max_harmonics() {
        let params = BaseParams::new(207);
        assert!((params.fundamental_frequency - 0.050_979_191).abs() < 1e-8);
        assert_eq!(params.harmonic_count, 56);
        assert_eq!(params.band_count, 12);
    }

    #[test]
    fn period_0_gives_min_harmonics() {
        let params = BaseParams::new(0);
        assert!((params.fundamental_frequency - 0.318_135_965).abs() < 1e-8);
        assert_eq!(params.harmonic_count, 9);
        assert_eq!(params.band_count, 3);
    }

    #[test]
    fn period_104_gives_mid_range() {
        let params = BaseParams::new(104);
        assert!((params.fundamental_frequency - 0.087_570_527).abs() < 1e-8);
        assert_eq!(params.harmonic_count, 33);
        assert_eq!(params.band_count, 11);
    }

    #[test]
    fn default_matches_decoder_reset() {
        let params = BaseParams::default();
        assert!((params.fundamental_frequency - 0.02985 * PI).abs() < 1e-8);
        assert_eq!(params.harmonic_count, 30);
        assert_eq!(params.band_count, 10);
    }
}

//! Automatic gain control for CQPSK demodulation.
//!
//! Normalizes complex input samples to a target RMS amplitude using
//! an exponential moving average of the signal power. CQPSK has a
//! varying envelope (unlike constant-envelope C4FM), so the Costas
//! loop requires consistent amplitude to function correctly.

use num_complex::Complex;

/// Default smoothing factor for the RMS power estimate.
///
/// Higher values track faster but introduce more gain variation.
const DEFAULT_ALPHA: f32 = 0.45;

/// Default target RMS amplitude after normalization.
const DEFAULT_REFERENCE: f32 = 0.85;

/// Minimum RMS power estimate to avoid division by zero.
const MIN_POWER: f32 = 1e-12;

/// RMS-based automatic gain control for complex samples.
///
/// Tracks the input signal power with an exponential moving average
/// and scales samples so the output RMS converges to the reference
/// level.
#[derive(Debug)]
pub struct Agc {
    /// Smoothing factor for power estimate (0..1).
    alpha: f32,
    /// Target RMS amplitude.
    reference: f32,
    /// Exponential moving average of |sample|^2.
    power_estimate: f32,
}

impl Agc {
    /// Create a new AGC with the given smoothing factor and reference level.
    ///
    /// * `alpha` - Smoothing factor for power tracking (0..1). Higher
    ///   values adapt faster.
    /// * `reference` - Target RMS amplitude for the output signal.
    pub fn new(alpha: f32, reference: f32) -> Self {
        Self {
            alpha,
            reference,
            power_estimate: reference * reference,
        }
    }

    /// Process one complex sample, returning the gain-normalized output.
    pub fn process(&mut self, sample: Complex<f32>) -> Complex<f32> {
        let power = sample.norm_sqr();
        self.power_estimate = self.alpha * power + (1.0 - self.alpha) * self.power_estimate;
        let gain = self.reference / self.power_estimate.max(MIN_POWER).sqrt();
        sample * gain
    }
}

impl Default for Agc {
    fn default() -> Self {
        Self::new(DEFAULT_ALPHA, DEFAULT_REFERENCE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_amplitude_converges_to_reference() {
        let reference = 0.85;
        let mut agc = Agc::new(0.45, reference);
        let input = Complex::new(2.0, 0.0);

        // Let the AGC settle.
        let mut output = Complex::new(0.0, 0.0);
        for _ in 0..200 {
            output = agc.process(input);
        }

        let output_rms = output.norm();
        assert!(
            (output_rms - reference).abs() < 0.05,
            "expected output RMS near {reference}, got {output_rms}"
        );
    }

    #[test]
    fn small_amplitude_is_boosted() {
        let reference = 0.85;
        let mut agc = Agc::new(0.45, reference);
        let input = Complex::new(0.1, 0.0);

        let mut output = Complex::new(0.0, 0.0);
        for _ in 0..200 {
            output = agc.process(input);
        }

        let output_rms = output.norm();
        assert!(
            output_rms > input.norm(),
            "small input should be boosted: input={}, output={}",
            input.norm(),
            output_rms
        );
        assert!(
            (output_rms - reference).abs() < 0.05,
            "expected output RMS near {reference}, got {output_rms}"
        );
    }

    #[test]
    fn varying_amplitude_tracks() {
        let reference = 0.85;
        let mut agc = Agc::new(0.45, reference);

        // Start with amplitude 1.0.
        for _ in 0..100 {
            agc.process(Complex::new(1.0, 0.0));
        }

        // Switch to amplitude 5.0 and let it adapt.
        let mut output = Complex::new(0.0, 0.0);
        for _ in 0..100 {
            output = agc.process(Complex::new(5.0, 0.0));
        }

        let output_rms = output.norm();
        assert!(
            (output_rms - reference).abs() < 0.1,
            "AGC should track amplitude change: got {output_rms}"
        );
    }

    #[test]
    fn zero_input_does_not_produce_nan_or_inf() {
        let mut agc = Agc::new(0.45, 0.85);

        for _ in 0..100 {
            let output = agc.process(Complex::new(0.0, 0.0));
            assert!(
                output.re.is_finite(),
                "output.re is not finite: {}",
                output.re
            );
            assert!(
                output.im.is_finite(),
                "output.im is not finite: {}",
                output.im
            );
        }
    }

    #[test]
    fn complex_input_preserves_phase() {
        let mut agc = Agc::new(0.45, 0.85);
        let phase = std::f32::consts::FRAC_PI_4;
        let input = Complex::from_polar(2.0, phase);

        // Let AGC settle.
        let mut output = Complex::new(0.0, 0.0);
        for _ in 0..200 {
            output = agc.process(input);
        }

        let output_phase = output.arg();
        assert!(
            (output_phase - phase).abs() < 1e-5,
            "phase should be preserved: expected {phase}, got {output_phase}"
        );
    }

    #[test]
    fn default_uses_expected_constants() {
        let agc = Agc::default();
        assert!((agc.alpha - DEFAULT_ALPHA).abs() < 1e-6);
        assert!((agc.reference - DEFAULT_REFERENCE).abs() < 1e-6);
    }
}

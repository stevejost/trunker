//! Automatic gain control for CQPSK demodulation.
//!
//! Normalizes complex input samples to a target RMS amplitude using
//! an exponential moving average of the signal power. CQPSK has a
//! varying envelope (unlike constant-envelope C4FM), so the Costas
//! loop requires consistent amplitude to function correctly.

use num_complex::Complex;

/// Default smoothing factor for the RMS power estimate.
///
/// Controls how quickly the AGC tracks power changes. A value of 0.005
/// gives a time constant of ~200 samples, smoothing over many symbols
/// to avoid injecting gain noise into the Costas loop during fades.
/// Too fast (e.g., 0.45) pumps gain sample-by-sample and destabilizes
/// CQPSK carrier recovery.
const DEFAULT_ALPHA: f32 = 0.005;

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

    /// Current gain factor applied to input samples.
    ///
    /// Computed as `reference / sqrt(power_estimate)`. Higher values
    /// indicate weaker input signal being boosted.
    pub fn gain(&self) -> f32 {
        self.reference / self.power_estimate.max(MIN_POWER).sqrt()
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

    /// Number of samples needed for AGC with slow alpha (0.005) to settle.
    /// Time constant is ~1/alpha = 200 samples. Convergence from the initial
    /// power estimate to a very different target needs ~10x time constants.
    const SETTLE_SAMPLES: usize = 2000;

    #[test]
    fn constant_amplitude_converges_to_reference() {
        let reference = 0.85;
        let mut agc = Agc::default();
        let input = Complex::new(2.0, 0.0);

        let mut output = Complex::new(0.0, 0.0);
        for _ in 0..SETTLE_SAMPLES {
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
        let mut agc = Agc::default();
        let input = Complex::new(0.1, 0.0);

        let mut output = Complex::new(0.0, 0.0);
        for _ in 0..SETTLE_SAMPLES {
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
        let mut agc = Agc::default();

        // Start with amplitude 1.0.
        for _ in 0..SETTLE_SAMPLES {
            agc.process(Complex::new(1.0, 0.0));
        }

        // Switch to amplitude 5.0 and let it adapt.
        let mut output = Complex::new(0.0, 0.0);
        for _ in 0..SETTLE_SAMPLES {
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
        let mut agc = Agc::default();

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
        let mut agc = Agc::default();
        let phase = std::f32::consts::FRAC_PI_4;
        let input = Complex::from_polar(2.0, phase);

        let mut output = Complex::new(0.0, 0.0);
        for _ in 0..SETTLE_SAMPLES {
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

    #[test]
    fn gain_accessor_matches_applied_gain() {
        let reference = 0.85;
        let mut agc = Agc::default();
        let input = Complex::new(2.0, 0.0);

        for _ in 0..SETTLE_SAMPLES {
            agc.process(input);
        }

        // After settling on constant input of amplitude 2.0,
        // power_estimate converges to 4.0, so gain ~ 0.85 / 2.0 = 0.425.
        let gain = agc.gain();
        let expected = reference / 2.0;
        assert!(
            (gain - expected).abs() < 0.05,
            "expected gain near {expected}, got {gain}"
        );
    }

    #[test]
    fn slow_alpha_reduces_gain_variance() {
        let reference = 0.85;
        let mut slow_agc = Agc::new(DEFAULT_ALPHA, reference);
        let mut fast_agc = Agc::new(0.45, reference);

        // Feed alternating amplitudes to simulate fading.
        let mut slow_gains = Vec::new();
        let mut fast_gains = Vec::new();
        for i in 0..SETTLE_SAMPLES {
            let amplitude = if i % 20 < 10 { 1.0 } else { 3.0 };
            let sample = Complex::new(amplitude, 0.0);
            slow_agc.process(sample);
            fast_agc.process(sample);
            slow_gains.push(slow_agc.gain());
            fast_gains.push(fast_agc.gain());
        }

        // Compute variance of gain over last 500 samples.
        let slow_tail = &slow_gains[500..];
        let fast_tail = &fast_gains[500..];
        let slow_var = variance(slow_tail);
        let fast_var = variance(fast_tail);

        assert!(
            slow_var < fast_var,
            "slow AGC should have less gain variance: slow={slow_var:.6}, fast={fast_var:.6}"
        );
    }

    /// Compute variance of a slice of f32 values.
    fn variance(values: &[f32]) -> f32 {
        let n = values.len() as f32;
        let mean = values.iter().sum::<f32>() / n;
        values.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / n
    }
}

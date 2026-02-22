//! Numerically Controlled Oscillator for frequency shifting.
//!
//! Shifts wideband IQ samples to baseband by multiplying with a complex
//! exponential at the specified offset frequency. Uses an incremental
//! phase accumulator with modular wrap to prevent precision loss.

use std::f64::consts::TAU;

use num_complex::Complex;

/// A complex NCO for frequency-shifting IQ samples.
///
/// Generates `exp(-j * 2 * pi * offset_hz / sample_rate * n)` incrementally,
/// multiplying each input sample to shift the target channel to DC.
///
/// The phase accumulator is f64 for long-term precision, but sin/cos is
/// computed at f32 precision since the output is `Complex<f32>`.
#[derive(Debug)]
pub struct Nco {
    /// Phase increment per sample in radians.
    phase_increment: f64,
    /// Current accumulated phase in radians.
    phase: f64,
}

impl Nco {
    /// Create a new NCO that shifts by `offset_hz` at the given sample rate.
    ///
    /// * `offset_hz` - Frequency offset in hertz (positive = channel above center).
    /// * `sample_rate` - Input sample rate in hertz.
    ///
    /// The NCO multiplies by `exp(-j * phase)` to shift the target down to DC.
    pub fn new(offset_hz: f64, sample_rate: f64) -> Self {
        Self {
            phase_increment: -TAU * offset_hz / sample_rate,
            phase: 0.0,
        }
    }

    /// Frequency-shift one complex sample and advance the phase.
    #[inline]
    pub fn shift(&mut self, sample: Complex<f32>) -> Complex<f32> {
        let (sin, cos) = (self.phase as f32).sin_cos();
        let rotator = Complex::new(cos, sin);
        self.phase += self.phase_increment;
        if self.phase >= TAU {
            self.phase -= TAU;
        } else if self.phase < 0.0 {
            self.phase += TAU;
        }
        sample * rotator
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_shifted_to_dc() {
        // Generate a 1 kHz tone at 48 kHz sample rate, then shift by +1 kHz.
        // After shifting, the tone should be at DC.
        let sample_rate = 48_000.0;
        let tone_hz = 1_000.0;
        let mut nco = Nco::new(tone_hz, sample_rate);

        let num_samples = 4800;
        let mut output = Vec::with_capacity(num_samples);

        for n in 0..num_samples {
            let phase = TAU * tone_hz * n as f64 / sample_rate;
            let input = Complex::new(phase.cos() as f32, phase.sin() as f32);
            output.push(nco.shift(input));
        }

        // After settling, the output should be approximately real-valued
        // (DC = constant magnitude, zero imaginary).
        let steady = &output[100..];
        let avg_re: f32 = steady.iter().map(|s| s.re).sum::<f32>() / steady.len() as f32;
        let avg_im: f32 = steady.iter().map(|s| s.im).sum::<f32>() / steady.len() as f32;

        assert!(
            (avg_re - 1.0).abs() < 0.01,
            "DC real component should be ~1.0, got {avg_re}"
        );
        assert!(
            avg_im.abs() < 0.01,
            "DC imaginary component should be ~0.0, got {avg_im}"
        );
    }

    #[test]
    fn phase_does_not_drift_over_millions_of_samples() {
        let mut nco = Nco::new(1234.5, 2_400_000.0);
        let input = Complex::new(1.0, 0.0);

        // Run for 10 million samples.
        for _ in 0..10_000_000 {
            nco.shift(input);
        }

        // Phase should still be within [0, TAU) due to conditional wrapping.
        assert!(
            nco.phase >= 0.0 && nco.phase < TAU,
            "phase out of range: {}",
            nco.phase
        );

        // Verify the output still has unit magnitude (no precision drift).
        let output = nco.shift(input);
        let magnitude = output.norm();
        assert!(
            (magnitude - 1.0).abs() < 0.001,
            "magnitude drifted: {magnitude}"
        );
    }

    #[test]
    fn zero_offset_is_passthrough() {
        let mut nco = Nco::new(0.0, 48_000.0);
        let input = Complex::new(0.5, -0.3);
        let output = nco.shift(input);

        assert!(
            (output.re - input.re).abs() < 1e-6,
            "zero offset should pass through: got {output}"
        );
        assert!(
            (output.im - input.im).abs() < 1e-6,
            "zero offset should pass through: got {output}"
        );
    }

    #[test]
    fn negative_offset_shifts_up() {
        // Shift a -1 kHz tone by -1 kHz should bring it to DC.
        let sample_rate = 48_000.0;
        let tone_hz = -1_000.0;
        let mut nco = Nco::new(tone_hz, sample_rate);

        let num_samples = 4800;
        let mut output = Vec::with_capacity(num_samples);

        for n in 0..num_samples {
            let phase = TAU * tone_hz * n as f64 / sample_rate;
            let input = Complex::new(phase.cos() as f32, phase.sin() as f32);
            output.push(nco.shift(input));
        }

        let steady = &output[100..];
        let avg_re: f32 = steady.iter().map(|s| s.re).sum::<f32>() / steady.len() as f32;
        let avg_im: f32 = steady.iter().map(|s| s.im).sum::<f32>() / steady.len() as f32;

        assert!(
            (avg_re - 1.0).abs() < 0.01,
            "DC real should be ~1.0, got {avg_re}"
        );
        assert!(
            avg_im.abs() < 0.01,
            "DC imaginary should be ~0.0, got {avg_im}"
        );
    }

    #[test]
    fn phase_increment_sign_is_correct() {
        // Positive offset_hz should produce negative phase_increment
        // (shift DOWN to baseband).
        let nco = Nco::new(1000.0, 48_000.0);
        assert!(
            nco.phase_increment < 0.0,
            "positive offset should produce negative phase increment"
        );

        let nco_neg = Nco::new(-1000.0, 48_000.0);
        assert!(
            nco_neg.phase_increment > 0.0,
            "negative offset should produce positive phase increment"
        );
    }

    #[test]
    fn phase_increment_matches_expected() {
        let nco = Nco::new(1000.0, 48_000.0);
        let expected = -TAU * 1000.0 / 48_000.0;
        assert!(
            (nco.phase_increment - expected).abs() < 1e-12,
            "phase increment mismatch: got {}, expected {expected}",
            nco.phase_increment
        );
    }

    #[test]
    fn f32_sincos_precision_is_sufficient() {
        // Verify that computing sin_cos at f32 precision (from f64 phase)
        // produces results close to full f64 sin_cos cast to f32.
        let mut max_sin_error: f32 = 0.0;
        let mut max_cos_error: f32 = 0.0;

        let num_points = 10_000;
        for i in 0..num_points {
            let phase_f64 = TAU * i as f64 / num_points as f64;
            let (ref_sin, ref_cos) = phase_f64.sin_cos();

            let (f32_sin, f32_cos) = (phase_f64 as f32).sin_cos();

            let sin_err = (f32_sin - ref_sin as f32).abs();
            let cos_err = (f32_cos - ref_cos as f32).abs();
            max_sin_error = max_sin_error.max(sin_err);
            max_cos_error = max_cos_error.max(cos_err);
        }

        // f32 sincos should be within ~1e-6 of f64 sincos cast to f32.
        let tolerance = 2e-6;
        assert!(
            max_sin_error < tolerance,
            "f32 sin max error {max_sin_error} exceeds tolerance {tolerance}"
        );
        assert!(
            max_cos_error < tolerance,
            "f32 cos max error {max_cos_error} exceeds tolerance {tolerance}"
        );
    }
}

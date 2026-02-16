//! FM discriminator for C4FM demodulation.
//!
//! Converts complex IQ samples to instantaneous frequency deviation
//! using a polar discriminator: `arg(z[n] * conj(z[n-1]))`.

use num_complex::Complex;

/// FM discriminator that converts complex IQ samples to baseband
/// frequency deviation values.
///
/// Output is proportional to instantaneous frequency. For P25 C4FM
/// at 24 kHz sample rate, the output range is approximately
/// +/- 0.47 radians/sample for outer deviation (+/- 1800 Hz).
#[derive(Debug)]
pub struct FmDemodulator {
    previous: Complex<f32>,
}

impl FmDemodulator {
    /// Create a new FM demodulator.
    pub fn new() -> Self {
        Self {
            previous: Complex::new(0.0, 0.0),
        }
    }

    /// Demodulate one complex sample, returning the instantaneous
    /// phase difference (proportional to frequency).
    pub fn process(&mut self, sample: Complex<f32>) -> f32 {
        let product = sample * self.previous.conj();
        self.previous = sample;
        product.im.atan2(product.re)
    }
}

impl Default for FmDemodulator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    /// Generate a complex exponential at the given frequency.
    fn complex_tone(frequency_hz: f32, sample_rate: f32, num_samples: usize) -> Vec<Complex<f32>> {
        (0..num_samples)
            .map(|n| {
                let phase = 2.0 * PI * frequency_hz * n as f32 / sample_rate;
                Complex::new(phase.cos(), phase.sin())
            })
            .collect()
    }

    #[test]
    fn dc_input_produces_zero_output() {
        let mut demod = FmDemodulator::new();
        let dc = Complex::new(1.0, 0.0);

        // Skip first sample (transition from zero).
        demod.process(dc);

        for _ in 0..100 {
            let output = demod.process(dc);
            assert!(
                output.abs() < 1e-6,
                "DC should produce zero frequency: got {output}"
            );
        }
    }

    #[test]
    fn constant_frequency_produces_constant_output() {
        let sample_rate = 24_000.0;
        let tone_hz = 1800.0;
        let samples = complex_tone(tone_hz, sample_rate, 200);
        let expected_phase_per_sample = 2.0 * PI * tone_hz / sample_rate;

        let mut demod = FmDemodulator::new();

        // Skip first few samples for startup transient.
        for sample in &samples[..5] {
            demod.process(*sample);
        }

        for sample in &samples[5..] {
            let output = demod.process(*sample);
            assert!(
                (output - expected_phase_per_sample).abs() < 1e-4,
                "expected {expected_phase_per_sample}, got {output}"
            );
        }
    }

    #[test]
    fn negative_frequency_produces_negative_output() {
        let sample_rate = 24_000.0;
        let tone_hz = -1800.0;
        let samples = complex_tone(tone_hz, sample_rate, 200);
        let expected = 2.0 * PI * tone_hz / sample_rate;

        let mut demod = FmDemodulator::new();
        for sample in &samples[..5] {
            demod.process(*sample);
        }

        for sample in &samples[5..] {
            let output = demod.process(*sample);
            assert!(
                (output - expected).abs() < 1e-4,
                "expected {expected}, got {output}"
            );
        }
    }

    #[test]
    fn outer_deviation_magnitude() {
        // P25 outer deviation is 1800 Hz at 24 kHz sample rate.
        let sample_rate = 24_000.0;
        let deviation_hz = 1800.0;
        let expected_rad = 2.0 * PI * deviation_hz / sample_rate;

        let samples = complex_tone(deviation_hz, sample_rate, 100);
        let mut demod = FmDemodulator::new();

        for sample in &samples[..5] {
            demod.process(*sample);
        }

        let output = demod.process(samples[5]);
        assert!(
            (output - expected_rad).abs() < 1e-3,
            "outer deviation: expected {expected_rad:.4}, got {output:.4}"
        );
    }
}

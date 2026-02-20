//! FIR low-pass filter with decimation for channel isolation.
//!
//! Uses a windowed-sinc design to isolate the P25 channel before
//! FM demodulation, and decimates to reduce the sample rate.

use std::f32::consts::PI;

use num_complex::Complex;

/// A FIR low-pass filter that also decimates the output.
///
/// Applies a windowed-sinc filter to complex IQ samples, then keeps
/// every `decimation_factor`-th output sample.
///
/// The delay line is sized to a power of two so that circular buffer
/// indexing uses a bitmask instead of modulo division.
#[derive(Debug)]
pub struct DecimatingFilter {
    coefficients: Vec<f32>,
    delay_line: Vec<Complex<f32>>,
    delay_index: usize,
    /// Bitmask for circular buffer indexing (`delay_line.len() - 1`).
    delay_mask: usize,
    /// Number of filter taps (may be less than `delay_line.len()`).
    num_taps: usize,
    decimation_factor: usize,
    decimation_counter: usize,
}

impl DecimatingFilter {
    /// Design a low-pass FIR filter with the given parameters.
    ///
    /// * `cutoff_hz` - Filter cutoff frequency in hertz.
    /// * `sample_rate` - Input sample rate in hertz.
    /// * `num_taps` - Number of filter taps (should be odd for symmetry).
    /// * `decimation_factor` - Keep every N-th output sample.
    pub fn new(
        cutoff_hz: f32,
        sample_rate: f32,
        num_taps: usize,
        decimation_factor: usize,
    ) -> Self {
        let coefficients = design_lowpass(cutoff_hz, sample_rate, num_taps);
        let buf_len = coefficients.len().next_power_of_two();
        Self {
            num_taps: coefficients.len(),
            delay_line: vec![Complex::new(0.0, 0.0); buf_len],
            delay_index: 0,
            delay_mask: buf_len - 1,
            coefficients,
            decimation_factor,
            decimation_counter: decimation_factor,
        }
    }

    /// Process one input sample, returning a filtered output if this
    /// sample lands on a decimation boundary.
    #[inline]
    pub fn process(&mut self, sample: Complex<f32>) -> Option<Complex<f32>> {
        self.delay_line[self.delay_index] = sample;
        self.delay_index = (self.delay_index + 1) & self.delay_mask;

        self.decimation_counter -= 1;
        if self.decimation_counter != 0 {
            return None;
        }
        self.decimation_counter = self.decimation_factor;

        Some(self.convolve())
    }

    /// Compute the FIR convolution over the delay line.
    #[inline]
    fn convolve(&self) -> Complex<f32> {
        let mask = self.delay_mask;
        let mut sum = Complex::new(0.0, 0.0);
        for i in 0..self.num_taps {
            let delay_pos = (self.delay_index + mask - i) & mask;
            sum += self.delay_line[delay_pos] * self.coefficients[i];
        }
        sum
    }
}

/// Design a windowed-sinc low-pass filter.
///
/// Uses a Blackman window for good stopband attenuation.
/// Returns normalized coefficients that sum to 1.0.
fn design_lowpass(cutoff_hz: f32, sample_rate: f32, num_taps: usize) -> Vec<f32> {
    let normalized_cutoff = cutoff_hz / sample_rate;
    let center = (num_taps - 1) as f32 / 2.0;

    let mut coefficients: Vec<f32> = (0..num_taps)
        .map(|i| {
            let n = i as f32 - center;
            let sinc = if n.abs() < 1e-10 {
                2.0 * PI * normalized_cutoff
            } else {
                (2.0 * PI * normalized_cutoff * n).sin() / n
            };
            let window = blackman_window(i, num_taps);
            sinc * window
        })
        .collect();

    let sum: f32 = coefficients.iter().sum();
    if sum.abs() > 1e-10 {
        for c in &mut coefficients {
            *c /= sum;
        }
    }

    coefficients
}

/// Compute the Blackman window value for tap `i` of `num_taps`.
fn blackman_window(i: usize, num_taps: usize) -> f32 {
    let n = i as f32;
    let m = (num_taps - 1) as f32;
    0.42 - 0.5 * (2.0 * PI * n / m).cos() + 0.08 * (4.0 * PI * n / m).cos()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coefficients_sum_to_one() {
        let coeffs = design_lowpass(6250.0, 2_400_000.0, 101);
        let sum: f32 = coeffs.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5, "sum = {sum}");
    }

    #[test]
    fn coefficients_are_symmetric() {
        let coeffs = design_lowpass(6250.0, 2_400_000.0, 101);
        let n = coeffs.len();
        for i in 0..n / 2 {
            assert!(
                (coeffs[i] - coeffs[n - 1 - i]).abs() < 1e-6,
                "asymmetry at tap {i}"
            );
        }
    }

    #[test]
    fn decimation_reduces_output_rate() {
        let mut filter = DecimatingFilter::new(6250.0, 2_400_000.0, 101, 100);
        let mut output_count = 0;
        let input_count = 1000;

        for _ in 0..input_count {
            if filter.process(Complex::new(1.0, 0.0)).is_some() {
                output_count += 1;
            }
        }

        assert_eq!(output_count, input_count / 100);
    }

    #[test]
    fn dc_input_passes_through() {
        let mut filter = DecimatingFilter::new(6250.0, 48_000.0, 51, 1);
        let dc = Complex::new(1.0, 0.0);

        // Feed enough samples to fill the delay line.
        let mut last_output = Complex::new(0.0, 0.0);
        for _ in 0..200 {
            if let Some(out) = filter.process(dc) {
                last_output = out;
            }
        }

        // DC should pass through with gain ~1.0.
        assert!(
            (last_output.re - 1.0).abs() < 0.01,
            "DC passthrough: got {last_output}"
        );
        assert!(
            last_output.im.abs() < 0.01,
            "imaginary should be ~0: got {last_output}"
        );
    }

    #[test]
    fn two_stage_decimation_preserves_tone_rejects_noise() {
        // Generate test signal at 2.4 MS/s:
        //   - 4800 Hz tone (P25 symbol rate, within passband)
        //   - 100 kHz tone (out-of-band interference, should be rejected)
        let input_rate = 2_400_000.0_f32;
        let tone_hz = 4_800.0_f32;
        let noise_hz = 100_000.0_f32;
        let num_samples = 240_000; // 100 ms of data

        let input: Vec<Complex<f32>> = (0..num_samples)
            .map(|i| {
                let t = i as f32 / input_rate;
                let tone = (2.0 * PI * tone_hz * t).cos();
                let noise = (2.0 * PI * noise_hz * t).cos();
                Complex::new(tone + noise, 0.0)
            })
            .collect();

        // Stage 1: 2.4 MS/s -> 240 kS/s, cutoff 6.25 kHz, 201 taps
        let mut stage1 = DecimatingFilter::new(6_250.0, 2_400_000.0, 201, 10);
        let mut intermediate = Vec::new();
        for &sample in &input {
            if let Some(out) = stage1.process(sample) {
                intermediate.push(out);
            }
        }
        assert_eq!(intermediate.len(), num_samples / 10);

        // Stage 2: 240 kS/s -> 24 kS/s, cutoff 6.25 kHz, 61 taps
        let mut stage2 = DecimatingFilter::new(6_250.0, 240_000.0, 61, 10);
        let mut output = Vec::new();
        for &sample in &intermediate {
            if let Some(out) = stage2.process(sample) {
                output.push(out);
            }
        }
        assert_eq!(output.len(), num_samples / 100);

        // Skip the transient at the start (filter settling time).
        // Use the last half of the output for measurement.
        let output_rate = 24_000.0_f32;
        let measure_start = output.len() / 2;
        let steady_state = &output[measure_start..];
        let n = steady_state.len() as f32;

        // Measure power at 4800 Hz using a matched filter (DFT bin).
        let mut tone_acc = Complex::new(0.0, 0.0);
        for (i, &sample) in steady_state.iter().enumerate() {
            let t = i as f32 / output_rate;
            let basis = Complex::new(
                (2.0 * PI * tone_hz * t).cos(),
                -(2.0 * PI * tone_hz * t).sin(),
            );
            tone_acc += sample * basis;
        }
        let tone_power = (tone_acc / n).norm();

        // Measure power at 100 kHz. At 24 kS/s output rate, 100 kHz is
        // above Nyquist (12 kHz), so it aliases. Regardless, any residual
        // energy at the alias frequency should be negligible.
        // Check the alias: 100 kHz mod 24 kHz = 4 kHz.
        let alias_hz = 100_000.0_f32 % output_rate;
        let mut noise_acc = Complex::new(0.0, 0.0);
        for (i, &sample) in steady_state.iter().enumerate() {
            let t = i as f32 / output_rate;
            let basis = Complex::new(
                (2.0 * PI * alias_hz * t).cos(),
                -(2.0 * PI * alias_hz * t).sin(),
            );
            noise_acc += sample * basis;
        }
        let noise_power = (noise_acc / n).norm();

        // The 4800 Hz tone should have significant power (> 0.1).
        assert!(
            tone_power > 0.1,
            "4800 Hz tone was suppressed: power = {tone_power}"
        );

        // The 100 kHz noise (aliased to 4 kHz) should be heavily
        // attenuated (at least 40 dB below the tone).
        let rejection_ratio = tone_power / (noise_power + 1e-10);
        assert!(
            rejection_ratio > 100.0,
            "100 kHz noise not sufficiently rejected: tone={tone_power}, noise={noise_power}, ratio={rejection_ratio}"
        );
    }

    #[test]
    fn delay_line_is_power_of_two() {
        // 101 taps should round up to 128-element delay line.
        let filter = DecimatingFilter::new(6250.0, 2_400_000.0, 101, 10);
        assert!(filter.delay_line.len().is_power_of_two());
        assert_eq!(filter.delay_line.len(), 128);
        assert_eq!(filter.delay_mask, 127);
        assert_eq!(filter.num_taps, 101);

        // 201 taps should round up to 256.
        let filter = DecimatingFilter::new(6250.0, 2_400_000.0, 201, 10);
        assert!(filter.delay_line.len().is_power_of_two());
        assert_eq!(filter.delay_line.len(), 256);
        assert_eq!(filter.delay_mask, 255);
        assert_eq!(filter.num_taps, 201);

        // Already a power of two (64 taps) stays at 64.
        let filter = DecimatingFilter::new(6250.0, 2_400_000.0, 64, 1);
        assert_eq!(filter.delay_line.len(), 64);
        assert_eq!(filter.delay_mask, 63);
    }
}

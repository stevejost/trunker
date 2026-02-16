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
#[derive(Debug)]
pub struct DecimatingFilter {
    coefficients: Vec<f32>,
    delay_line: Vec<Complex<f32>>,
    delay_index: usize,
    decimation_factor: usize,
    input_count: usize,
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
        Self {
            delay_line: vec![Complex::new(0.0, 0.0); coefficients.len()],
            delay_index: 0,
            coefficients,
            decimation_factor,
            input_count: 0,
        }
    }

    /// Process one input sample, returning a filtered output if this
    /// sample lands on a decimation boundary.
    pub fn process(&mut self, sample: Complex<f32>) -> Option<Complex<f32>> {
        self.delay_line[self.delay_index] = sample;
        self.delay_index = (self.delay_index + 1) % self.delay_line.len();
        self.input_count += 1;

        if !self.input_count.is_multiple_of(self.decimation_factor) {
            return None;
        }

        Some(self.convolve())
    }

    /// Compute the FIR convolution over the delay line.
    fn convolve(&self) -> Complex<f32> {
        let len = self.coefficients.len();
        let mut sum = Complex::new(0.0, 0.0);
        for i in 0..len {
            let delay_pos = (self.delay_index + len - 1 - i) % len;
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
}

//! Root-raised-cosine (RRC) matched filter.
//!
//! Implements the receiver-side matched filter for P25 C4FM demodulation.
//! The transmitter uses raised-cosine pulse shaping; this RRC filter
//! completes the matched filter pair, providing zero inter-symbol
//! interference (ISI) at the correct sampling instants.

use std::f32::consts::PI;

use num_complex::Complex;

/// FIR root-raised-cosine matched filter.
///
/// Operates on real-valued baseband samples (post FM demodulation).
/// Filters at the symbol rate to minimize ISI and maximize SNR
/// at the symbol sampling points.
#[derive(Debug)]
pub struct RrcFilter {
    coefficients: Vec<f32>,
    delay_line: Vec<f32>,
    delay_index: usize,
}

impl RrcFilter {
    /// Design an RRC filter for the given parameters.
    ///
    /// * `symbol_rate` - Symbol rate in hertz (4800 for P25).
    /// * `sample_rate` - Sample rate in hertz (24000 for our pipeline).
    /// * `excess_bw` - Roll-off factor (0.2 for P25, matching OP25).
    /// * `num_symbols` - Filter span in symbols (each side). Total taps = 2 * num_symbols * sps + 1.
    pub fn new(symbol_rate: f32, sample_rate: f32, excess_bw: f32, num_symbols: usize) -> Self {
        let sps = (sample_rate / symbol_rate).round() as usize;
        let num_taps = 2 * num_symbols * sps + 1;
        let coefficients = design_rrc(sps, excess_bw, num_taps);
        Self {
            delay_line: vec![0.0; coefficients.len()],
            delay_index: 0,
            coefficients,
        }
    }

    /// Clear the delay line for re-acquisition.
    pub fn reset(&mut self) {
        self.delay_line.fill(0.0);
        self.delay_index = 0;
    }

    /// Process one baseband sample through the matched filter.
    #[inline]
    pub fn process(&mut self, sample: f32) -> f32 {
        self.delay_line[self.delay_index] = sample;
        self.delay_index = (self.delay_index + 1) % self.delay_line.len();
        self.convolve()
    }

    /// Compute the FIR convolution.
    #[inline]
    fn convolve(&self) -> f32 {
        let len = self.coefficients.len();
        let mut sum = 0.0_f32;
        for i in 0..len {
            let delay_pos = (self.delay_index + len - 1 - i) % len;
            sum += self.delay_line[delay_pos] * self.coefficients[i];
        }
        sum
    }
}

/// Design root-raised-cosine filter coefficients.
///
/// Implements the standard RRC impulse response formula:
///
/// h(t) = (sin(pi*t/T*(1-alpha)) + 4*alpha*t/T*cos(pi*t/T*(1+alpha)))
///        / (pi*t/T * (1 - (4*alpha*t/T)^2))
///
/// where T is the symbol period and alpha is the excess bandwidth.
fn design_rrc(samples_per_symbol: usize, alpha: f32, num_taps: usize) -> Vec<f32> {
    let sps = samples_per_symbol as f32;
    let center = (num_taps - 1) as f32 / 2.0;

    let mut coefficients: Vec<f32> = (0..num_taps)
        .map(|i| {
            let t = (i as f32 - center) / sps; // Time in symbol periods

            if t.abs() < 1e-7 {
                // t = 0: h(0) = 1 - alpha + 4*alpha/pi
                1.0 - alpha + 4.0 * alpha / PI
            } else if (t.abs() - 1.0 / (4.0 * alpha)).abs() < 1e-7 {
                // t = +/- T/(4*alpha): special case to avoid 0/0
                (alpha / (2.0_f32).sqrt())
                    * ((1.0 + 2.0 / PI) * (PI / (4.0 * alpha)).sin()
                        + (1.0 - 2.0 / PI) * (PI / (4.0 * alpha)).cos())
            } else {
                let pi_t = PI * t;
                let numerator =
                    (pi_t * (1.0 - alpha)).sin() + 4.0 * alpha * t * (pi_t * (1.0 + alpha)).cos();
                let denominator = pi_t * (1.0 - (4.0 * alpha * t).powi(2));
                numerator / denominator
            }
        })
        .collect();

    // Normalize to unit energy.
    let energy: f32 = coefficients.iter().map(|c| c * c).sum();
    let norm = energy.sqrt();
    if norm > 1e-10 {
        for c in &mut coefficients {
            *c /= norm;
        }
    }

    coefficients
}

/// RRC matched filter for complex (I/Q) samples.
///
/// Applies the same real-valued RRC coefficients independently to
/// the in-phase and quadrature channels. This is mathematically
/// equivalent to complex convolution with a real-valued kernel.
/// Used in the CQPSK demodulation path where the signal remains
/// in the complex domain until after carrier recovery.
#[derive(Debug)]
pub struct ComplexRrcFilter {
    filter_i: RrcFilter,
    filter_q: RrcFilter,
}

impl ComplexRrcFilter {
    /// Design a complex RRC filter with the given parameters.
    ///
    /// Parameters are identical to [`RrcFilter::new`].
    pub fn new(symbol_rate: f32, sample_rate: f32, excess_bw: f32, num_symbols: usize) -> Self {
        Self {
            filter_i: RrcFilter::new(symbol_rate, sample_rate, excess_bw, num_symbols),
            filter_q: RrcFilter::new(symbol_rate, sample_rate, excess_bw, num_symbols),
        }
    }

    /// Clear both I and Q delay lines for re-acquisition.
    pub fn reset(&mut self) {
        self.filter_i.reset();
        self.filter_q.reset();
    }

    /// Process one complex sample through the matched filter.
    #[inline]
    pub fn process(&mut self, sample: Complex<f32>) -> Complex<f32> {
        Complex::new(
            self.filter_i.process(sample.re),
            self.filter_q.process(sample.im),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrc_coefficients_are_symmetric() {
        let coeffs = design_rrc(5, 0.2, 51);
        let n = coeffs.len();
        for i in 0..n / 2 {
            assert!(
                (coeffs[i] - coeffs[n - 1 - i]).abs() < 1e-6,
                "asymmetry at tap {i}: {} vs {}",
                coeffs[i],
                coeffs[n - 1 - i]
            );
        }
    }

    #[test]
    fn rrc_unit_energy() {
        let coeffs = design_rrc(5, 0.2, 51);
        let energy: f32 = coeffs.iter().map(|c| c * c).sum();
        assert!(
            (energy - 1.0).abs() < 0.01,
            "energy should be ~1.0: got {energy}"
        );
    }

    #[test]
    fn rrc_center_tap_is_maximum() {
        let coeffs = design_rrc(5, 0.2, 51);
        let center = coeffs.len() / 2;
        let center_val = coeffs[center].abs();
        for (i, c) in coeffs.iter().enumerate() {
            if i != center {
                assert!(
                    c.abs() <= center_val + 1e-6,
                    "tap {i} ({}) exceeds center tap ({})",
                    c.abs(),
                    center_val
                );
            }
        }
    }

    #[test]
    fn rrc_passes_dc_signal() {
        let mut filter = RrcFilter::new(4800.0, 24_000.0, 0.2, 5);

        // Feed constant input; after settling, output should be stable.
        for _ in 0..200 {
            filter.process(1.0);
        }

        let out = filter.process(1.0);
        assert!(out.abs() > 0.01, "DC should pass through: got {out}");
    }

    #[test]
    fn rrc_clean_symbol_sequence() {
        let mut filter = RrcFilter::new(4800.0, 24_000.0, 0.2, 5);
        let sps = 5;

        // Create a symbol sequence: +3, -3, +3, -3, ...
        // Each symbol held for `sps` samples.
        let symbols = [
            3.0_f32, -3.0, 3.0, -3.0, 3.0, -3.0, 3.0, -3.0, 3.0, -3.0, 3.0, -3.0, 3.0, -3.0, 3.0,
            -3.0, 3.0, -3.0, 3.0, -3.0,
        ];

        let input: Vec<f32> = symbols
            .iter()
            .flat_map(|&s| std::iter::repeat_n(s, sps))
            .collect();

        let mut output = Vec::new();
        for &sample in &input {
            output.push(filter.process(sample));
        }

        // After the filter settles, the output at symbol centers should
        // alternate between positive and negative values.
        let settle = 10 * sps; // Skip initial transient
        if output.len() > settle + 2 * sps {
            let mid = settle + sps / 2;
            let val1 = output[mid];
            let val2 = output[mid + sps];
            // They should have opposite signs.
            assert!(
                val1 * val2 < 0.0,
                "alternating symbols should produce alternating output: {} and {}",
                val1,
                val2
            );
        }
    }

    #[test]
    fn complex_rrc_passes_dc() {
        let mut filter = ComplexRrcFilter::new(4800.0, 24_000.0, 0.2, 5);
        let dc = Complex::new(1.0, 0.5);

        for _ in 0..200 {
            filter.process(dc);
        }

        let out = filter.process(dc);
        assert!(
            out.re.abs() > 0.01,
            "I channel DC should pass through: got {}",
            out.re
        );
        assert!(
            out.im.abs() > 0.01,
            "Q channel DC should pass through: got {}",
            out.im
        );
    }

    #[test]
    fn complex_rrc_filters_channels_independently() {
        let mut filter = ComplexRrcFilter::new(4800.0, 24_000.0, 0.2, 5);
        let mut filter_i = RrcFilter::new(4800.0, 24_000.0, 0.2, 5);
        let mut filter_q = RrcFilter::new(4800.0, 24_000.0, 0.2, 5);

        let sample_rate = 24_000.0;
        let tone_i = 2400.0_f32;
        let tone_q = 1200.0_f32;

        for n in 0..500 {
            let t = n as f32 / sample_rate;
            let i_val = (2.0 * PI * tone_i * t).sin();
            let q_val = (2.0 * PI * tone_q * t).cos();

            let complex_out = filter.process(Complex::new(i_val, q_val));
            let real_i = filter_i.process(i_val);
            let real_q = filter_q.process(q_val);

            assert!(
                (complex_out.re - real_i).abs() < 1e-6,
                "I channel mismatch at sample {n}: {} vs {}",
                complex_out.re,
                real_i
            );
            assert!(
                (complex_out.im - real_q).abs() < 1e-6,
                "Q channel mismatch at sample {n}: {} vs {}",
                complex_out.im,
                real_q
            );
        }
    }

    #[test]
    fn complex_rrc_preserves_phase_of_passband_tone() {
        let mut filter = ComplexRrcFilter::new(4800.0, 24_000.0, 0.2, 5);
        let sample_rate = 24_000.0;
        let tone_hz = 2400.0; // Within passband (< 2880 Hz)

        // Let the filter settle.
        for n in 0..500 {
            let t = n as f32 / sample_rate;
            let sample = Complex::from_polar(1.0, 2.0 * PI * tone_hz * t);
            filter.process(sample);
        }

        // Measure output phase relationship over a few samples.
        let mut outputs = Vec::new();
        for n in 500..510 {
            let t = n as f32 / sample_rate;
            let sample = Complex::from_polar(1.0, 2.0 * PI * tone_hz * t);
            outputs.push(filter.process(sample));
        }

        // Output should have consistent phase progression (not scrambled).
        for i in 1..outputs.len() {
            let phase_diff = (outputs[i] * outputs[i - 1].conj()).arg();
            let expected = 2.0 * PI * tone_hz / sample_rate;
            assert!(
                (phase_diff - expected).abs() < 0.1,
                "phase progression should be consistent: got {phase_diff:.3}, expected {expected:.3}"
            );
        }
    }
}

//! Root-raised-cosine (RRC) matched filter.
//!
//! Implements the receiver-side matched filter for P25 C4FM demodulation.
//! The transmitter uses raised-cosine pulse shaping; this RRC filter
//! completes the matched filter pair, providing zero inter-symbol
//! interference (ISI) at the correct sampling instants.

use std::f32::consts::PI;

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

    /// Process one baseband sample through the matched filter.
    pub fn process(&mut self, sample: f32) -> f32 {
        self.delay_line[self.delay_index] = sample;
        self.delay_index = (self.delay_index + 1) % self.delay_line.len();
        self.convolve()
    }

    /// Compute the FIR convolution.
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
}

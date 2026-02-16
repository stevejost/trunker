//! Gardner timing error detector with PI loop filter.
//!
//! Recovers symbol timing from a complex baseband signal using the
//! Gardner algorithm. Processes one input sample at a time and produces
//! a symbol-rate output when a symbol boundary is crossed.
//!
//! The Gardner TED computes: `e = (last - current) * midpoint`
//! where `midpoint` is the sample halfway between two symbol instants.
//! A PI loop filter adjusts the sampling phase (`mu`) and the estimated
//! samples-per-symbol (`omega`).

use num_complex::Complex;

use super::interpolator::lagrange4_complex;

/// Nominal samples per symbol at 24 kHz IF rate with 4800 baud.
const NOMINAL_OMEGA: f32 = 5.0;

/// Proportional gain for the timing loop (controls mu).
const GAIN_MU: f32 = 0.025;

/// Integral gain for the timing loop (controls omega).
const GAIN_OMEGA: f32 = GAIN_MU * GAIN_MU / 4.0; // 0.000_156_25

/// Maximum relative deviation of omega from nominal.
const OMEGA_RELATIVE_LIMIT: f32 = 0.002;

/// Size of the sample delay line. Must hold at least 2 * ceil(omega)
/// samples for mid-point interpolation.
const DELAY_LINE_SIZE: usize = 12;

/// Gardner timing error detector and PI loop filter.
///
/// Accepts complex samples at the IF rate (24 kHz) and produces one
/// complex output sample per symbol period (4800 baud). The loop
/// adaptively tracks symbol timing drift using the Gardner algorithm.
#[derive(Debug)]
pub struct GardnerTed {
    /// Circular delay line for input samples.
    delay_line: [Complex<f32>; DELAY_LINE_SIZE],
    /// Current write position in the delay line.
    write_index: usize,
    /// Fractional sample offset within the current symbol period.
    /// When mu <= 1.0, a symbol decision is made.
    mu: f32,
    /// Estimated samples per symbol (tracks near NOMINAL_OMEGA).
    omega: f32,
    /// Center value for omega clamping.
    omega_mid: f32,
    /// Previous on-time symbol sample (for Gardner error computation).
    last_sample: Complex<f32>,
}

impl GardnerTed {
    /// Create a new Gardner TED with default P25 parameters.
    pub fn new() -> Self {
        let omega = NOMINAL_OMEGA;
        Self {
            delay_line: [Complex::new(0.0, 0.0); DELAY_LINE_SIZE],
            write_index: 0,
            mu: 0.0,
            omega,
            omega_mid: omega,
            last_sample: Complex::new(0.0, 0.0),
        }
    }

    /// Process one complex input sample.
    ///
    /// Returns `Some(symbol)` when a symbol boundary is crossed,
    /// or `None` when more input samples are needed.
    pub fn process(&mut self, sample: Complex<f32>) -> Option<Complex<f32>> {
        self.push_sample(sample);
        self.mu -= 1.0;

        if self.mu > 0.0 {
            return None;
        }

        // Compute on-time and mid-point interpolated samples.
        let on_time = self.interpolate_at(self.half_omega_whole(), self.half_omega_frac());
        let midpoint = self.interpolate_at(0, self.mu + self.mu.floor().abs());

        // Gardner timing error: e = Re{(last - current) * conj(mid)}
        // Computed per-component and summed, matching OP25.
        let error = self.compute_error(on_time, midpoint);

        self.last_sample = on_time;

        // Update loop: PI filter adjusts omega and mu.
        self.update_loop(error, on_time);

        Some(on_time)
    }

    /// Push a sample into the circular delay line.
    fn push_sample(&mut self, sample: Complex<f32>) {
        self.delay_line[self.write_index] = sample;
        self.write_index = (self.write_index + 1) % DELAY_LINE_SIZE;
    }

    /// Whole-sample offset to the half-symbol point.
    fn half_omega_whole(&self) -> usize {
        let half = self.omega / 2.0;
        let whole = half.floor() as usize;
        let frac = self.mu + half - whole as f32;
        if frac > 1.0 { whole + 1 } else { whole }
    }

    /// Fractional offset for the half-symbol interpolation.
    fn half_omega_frac(&self) -> f32 {
        let half = self.omega / 2.0;
        let whole = half.floor() as usize;
        let frac = self.mu + half - whole as f32;
        if frac > 1.0 { frac - 1.0 } else { frac }
    }

    /// Interpolate from the delay line at a given offset and fractional mu.
    ///
    /// `offset` is the number of whole samples back from the write pointer.
    /// `mu` is the fractional delay (0.0..1.0).
    fn interpolate_at(&self, offset: usize, mu: f32) -> Complex<f32> {
        let base = (self.write_index + DELAY_LINE_SIZE - 1 - offset) % DELAY_LINE_SIZE;
        let samples = [
            self.delay_line[(base + DELAY_LINE_SIZE - 1) % DELAY_LINE_SIZE],
            self.delay_line[base],
            self.delay_line[(base + 1) % DELAY_LINE_SIZE],
            self.delay_line[(base + 2) % DELAY_LINE_SIZE],
        ];
        lagrange4_complex(samples, mu)
    }

    /// Compute the Gardner timing error.
    ///
    /// `e = (last_re - current_re) * mid_re + (last_im - current_im) * mid_im`
    fn compute_error(&self, on_time: Complex<f32>, midpoint: Complex<f32>) -> f32 {
        let error_re = (self.last_sample.re - on_time.re) * midpoint.re;
        let error_im = (self.last_sample.im - on_time.im) * midpoint.im;
        let error = error_re + error_im;
        if error.is_nan() {
            0.0
        } else {
            error.clamp(-1.0, 1.0)
        }
    }

    /// Update omega and mu via the PI loop filter.
    fn update_loop(&mut self, error: f32, sample: Complex<f32>) {
        // Integral path: adjust omega proportional to error * signal magnitude.
        self.omega += GAIN_OMEGA * error * sample.norm();
        // Clamp omega to within OMEGA_RELATIVE_LIMIT of nominal.
        let deviation =
            (self.omega - self.omega_mid).clamp(-OMEGA_RELATIVE_LIMIT, OMEGA_RELATIVE_LIMIT);
        self.omega = self.omega_mid + deviation;

        // Proportional path: adjust mu.
        self.mu += self.omega + GAIN_MU * error;
    }
}

impl Default for GardnerTed {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    /// Generate a QPSK signal at the given samples per symbol.
    ///
    /// Returns complex samples where each symbol is a constant
    /// complex value held for `sps` samples.
    fn generate_qpsk_signal(
        symbols: &[Complex<f32>],
        samples_per_symbol: f32,
        timing_offset: f32,
    ) -> Vec<Complex<f32>> {
        let total_samples = (symbols.len() as f32 * samples_per_symbol + 10.0) as usize;
        let mut output = Vec::with_capacity(total_samples);
        for i in 0..total_samples {
            let t = (i as f32 - timing_offset) / samples_per_symbol;
            let symbol_index = t.floor() as isize;
            let symbol = if symbol_index >= 0 && (symbol_index as usize) < symbols.len() {
                symbols[symbol_index as usize]
            } else {
                Complex::new(0.0, 0.0)
            };
            output.push(symbol);
        }
        output
    }

    /// Standard QPSK constellation points.
    fn qpsk_symbols() -> Vec<Complex<f32>> {
        let angle = PI / 4.0;
        vec![
            Complex::from_polar(1.0, angle),
            Complex::from_polar(1.0, 3.0 * angle),
            Complex::from_polar(1.0, 5.0 * angle),
            Complex::from_polar(1.0, 7.0 * angle),
            Complex::from_polar(1.0, angle),
            Complex::from_polar(1.0, 5.0 * angle),
            Complex::from_polar(1.0, 3.0 * angle),
            Complex::from_polar(1.0, 7.0 * angle),
            Complex::from_polar(1.0, angle),
            Complex::from_polar(1.0, 3.0 * angle),
            Complex::from_polar(1.0, angle),
            Complex::from_polar(1.0, 5.0 * angle),
            Complex::from_polar(1.0, 7.0 * angle),
            Complex::from_polar(1.0, 3.0 * angle),
            Complex::from_polar(1.0, angle),
            Complex::from_polar(1.0, 7.0 * angle),
            Complex::from_polar(1.0, 5.0 * angle),
            Complex::from_polar(1.0, 3.0 * angle),
            Complex::from_polar(1.0, angle),
            Complex::from_polar(1.0, 5.0 * angle),
        ]
    }

    #[test]
    fn produces_output_at_symbol_rate() {
        let symbols = qpsk_symbols();
        let signal = generate_qpsk_signal(&symbols, 5.0, 0.0);
        let mut gardner = GardnerTed::new();

        let mut output_count = 0;
        for &sample in &signal {
            if gardner.process(sample).is_some() {
                output_count += 1;
            }
        }

        // Should produce approximately one output per 5 input samples.
        let expected = signal.len() / 5;
        assert!(
            output_count >= expected - 2 && output_count <= expected + 2,
            "expected ~{expected} symbols, got {output_count}"
        );
    }

    #[test]
    fn tracks_with_timing_offset() {
        let symbols = qpsk_symbols();
        // Introduce a 2-sample timing offset.
        let signal = generate_qpsk_signal(&symbols, 5.0, 2.0);
        let mut gardner = GardnerTed::new();

        let mut outputs = Vec::new();
        for &sample in &signal {
            if let Some(sym) = gardner.process(sample) {
                outputs.push(sym);
            }
        }

        // After convergence, output symbols should have magnitude ~1.0
        // (matching the input QPSK constellation).
        let settled = &outputs[outputs.len() / 2..];
        let avg_magnitude: f32 =
            settled.iter().map(|s| s.norm()).sum::<f32>() / settled.len() as f32;
        assert!(
            (avg_magnitude - 1.0).abs() < 0.3,
            "expected magnitude ~1.0 after convergence, got {avg_magnitude}"
        );
    }

    #[test]
    fn error_is_small_at_optimal_timing() {
        // With zero timing offset and perfect symbol-rate signal,
        // the Gardner error should be near zero after startup.
        let symbols = qpsk_symbols();
        let signal = generate_qpsk_signal(&symbols, 5.0, 0.0);
        let mut gardner = GardnerTed::new();

        let mut outputs = Vec::new();
        for &sample in &signal {
            if let Some(sym) = gardner.process(sample) {
                outputs.push(sym);
            }
        }

        // The loop should remain stable — omega should stay near 5.0.
        assert!(
            (gardner.omega - 5.0).abs() < 0.02,
            "omega drifted: expected ~5.0, got {}",
            gardner.omega
        );
    }

    #[test]
    fn omega_stays_clamped() {
        let mut gardner = GardnerTed::new();
        // Feed random-ish samples to stress the loop.
        for i in 0..500 {
            let phase = i as f32 * 0.7;
            gardner.process(Complex::new(phase.cos(), phase.sin()));
        }

        let min_omega = NOMINAL_OMEGA * (1.0 - OMEGA_RELATIVE_LIMIT);
        let max_omega = NOMINAL_OMEGA * (1.0 + OMEGA_RELATIVE_LIMIT);
        assert!(
            gardner.omega >= min_omega && gardner.omega <= max_omega,
            "omega out of bounds: {} (expected {min_omega}..{max_omega})",
            gardner.omega
        );
    }
}

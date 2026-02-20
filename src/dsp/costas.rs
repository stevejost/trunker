//! Order-4 Costas loop for QPSK/CQPSK carrier and phase recovery.
//!
//! Tracks residual carrier frequency offset and phase error in the
//! complex baseband signal. The phase detector uses the QPSK decision-
//! directed formula: `sgn(I)*Q - sgn(Q)*I`, which produces zero error
//! when the constellation is aligned to the decision axes.
//!
//! The loop filter is a second-order proportional-integral controller
//! that corrects both phase and frequency offsets.

use std::f32::consts::FRAC_PI_2;

use num_complex::Complex;

/// Proportional gain for the loop filter.
///
/// Controls how quickly the loop reacts to phase errors.
const DEFAULT_ALPHA: f32 = 0.02234;

/// Integral gain for the loop filter.
///
/// Controls frequency acquisition speed. Smaller than alpha for
/// stability.
const DEFAULT_BETA: f32 = 0.000253;

/// Maximum phase accumulator magnitude (radians).
///
/// Limited to pi/2 to prevent cycle slipping. The loop can correct
/// offsets within one quadrant; larger errors require re-acquisition.
const MAX_PHASE: f32 = FRAC_PI_2;

/// Order-4 Costas loop for QPSK carrier recovery.
///
/// Applies a phase correction to each input sample via an NCO
/// (numerically controlled oscillator), then computes the phase error
/// from the corrected constellation point and updates the loop state.
#[derive(Debug)]
pub struct CostasLoop {
    /// Proportional gain.
    alpha: f32,
    /// Integral gain.
    beta: f32,
    /// Phase accumulator (radians).
    phase: f32,
    /// Frequency accumulator (radians/sample).
    frequency: f32,
}

impl CostasLoop {
    /// Create a new Costas loop with the given loop gains.
    pub fn new(alpha: f32, beta: f32) -> Self {
        Self {
            alpha,
            beta,
            phase: 0.0,
            frequency: 0.0,
        }
    }

    /// Process one complex sample through the Costas loop.
    ///
    /// Returns the phase-corrected sample. The internal NCO tracks
    /// residual carrier offset so the output constellation is aligned.
    #[inline]
    pub fn process(&mut self, sample: Complex<f32>) -> Complex<f32> {
        let corrected = sample * Complex::from_polar(1.0, -self.phase);
        let error = Self::phase_error(corrected);
        self.advance(error);
        corrected
    }

    /// Return the current frequency estimate in radians per sample.
    pub fn frequency(&self) -> f32 {
        self.frequency
    }

    /// Return the current phase estimate in radians.
    pub fn phase(&self) -> f32 {
        self.phase
    }

    /// Seed the loop with an initial phase and frequency estimate.
    ///
    /// Use this to pre-load the loop state from another locked Costas
    /// loop (e.g., the control channel), reducing acquisition time from
    /// ~22 frames to ~2-5 frames.
    pub fn seed(&mut self, phase: f32, frequency: f32) {
        self.phase = phase.clamp(-MAX_PHASE, MAX_PHASE);
        self.frequency = frequency;
    }

    /// QPSK phase error detector: `sgn(I)*Q - sgn(Q)*I`.
    ///
    /// This is the standard decision-directed error for order-4 loops.
    /// It produces zero when the sample sits exactly on a constellation
    /// point axis and a signed error proportional to the angular offset.
    fn phase_error(sample: Complex<f32>) -> f32 {
        let i = sample.re;
        let q = sample.im;
        signum(i) * q - signum(q) * i
    }

    /// Advance the loop filter and NCO by one sample.
    fn advance(&mut self, error: f32) {
        self.frequency += self.beta * error;
        self.phase += self.frequency + self.alpha * error;
        self.phase = self.phase.clamp(-MAX_PHASE, MAX_PHASE);
    }
}

impl Default for CostasLoop {
    fn default() -> Self {
        Self::new(DEFAULT_ALPHA, DEFAULT_BETA)
    }
}

/// Signum function that returns +1.0 for non-negative values.
fn signum(x: f32) -> f32 {
    if x >= 0.0 { 1.0 } else { -1.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    /// Generate a QPSK signal at the given frequency offset (Hz) and
    /// phase offset (radians), at `samples_per_symbol` samples per symbol.
    fn generate_qpsk_signal(
        num_symbols: usize,
        samples_per_symbol: usize,
        freq_offset_hz: f32,
        sample_rate: f32,
        phase_offset: f32,
    ) -> Vec<Complex<f32>> {
        let freq_offset_rad = 2.0 * PI * freq_offset_hz / sample_rate;
        let phases = [
            PI / 4.0,        // 45 deg (QPSK point)
            3.0 * PI / 4.0,  // 135 deg
            -3.0 * PI / 4.0, // -135 deg
            -PI / 4.0,       // -45 deg
        ];

        let mut samples = Vec::with_capacity(num_symbols * samples_per_symbol);
        for i in 0..num_symbols {
            let constellation_phase = phases[i % 4];
            for j in 0..samples_per_symbol {
                let sample_idx = (i * samples_per_symbol + j) as f32;
                let phase = constellation_phase + phase_offset + freq_offset_rad * sample_idx;
                samples.push(Complex::from_polar(1.0, phase));
            }
        }
        samples
    }

    #[test]
    fn locks_with_zero_offset() {
        let mut costas = CostasLoop::default();
        let samples_per_symbol = 5;
        let signal = generate_qpsk_signal(200, samples_per_symbol, 0.0, 24_000.0, 0.0);

        let mut last_output = Complex::new(0.0, 0.0);
        for &sample in &signal {
            last_output = costas.process(sample);
        }

        // With no offset, the loop should stay near zero phase/freq.
        assert!(
            costas.phase().abs() < 0.1,
            "phase should be near zero: got {}",
            costas.phase()
        );
        assert!(
            costas.frequency().abs() < 0.001,
            "frequency should be near zero: got {}",
            costas.frequency()
        );
        // Output should still be a valid constellation point.
        assert!(
            last_output.norm() > 0.5,
            "output magnitude too low: {}",
            last_output.norm()
        );
    }

    #[test]
    fn corrects_frequency_offset() {
        let mut costas = CostasLoop::default();
        let sample_rate = 24_000.0;
        let freq_offset = 50.0; // Hz
        let samples_per_symbol = 5;
        let signal = generate_qpsk_signal(500, samples_per_symbol, freq_offset, sample_rate, 0.0);

        // Let the loop settle.
        for &sample in &signal[..signal.len() / 2] {
            costas.process(sample);
        }

        // Measure phase error over the settled portion.
        let expected_freq_rad = 2.0 * PI * freq_offset / sample_rate;
        let freq_estimate = costas.frequency();

        // The loop should have acquired the frequency offset.
        assert!(
            (freq_estimate - expected_freq_rad).abs() < 0.01,
            "frequency estimate should be near {expected_freq_rad:.5}: got {freq_estimate:.5}"
        );
    }

    #[test]
    fn corrects_phase_offset() {
        let phase_offset = 30.0_f32.to_radians();
        let mut costas = CostasLoop::default();
        let samples_per_symbol = 5;
        let signal = generate_qpsk_signal(300, samples_per_symbol, 0.0, 24_000.0, phase_offset);

        // Let the loop settle.
        for &sample in &signal {
            costas.process(sample);
        }

        // The accumulated phase should have converged toward the offset.
        let phase_est = costas.phase();
        assert!(
            (phase_est - phase_offset).abs() < 0.15,
            "phase estimate should be near {phase_offset:.3}: got {phase_est:.3}"
        );
    }

    #[test]
    fn error_signal_near_zero_when_locked() {
        let mut costas = CostasLoop::default();
        let samples_per_symbol = 5;
        let signal = generate_qpsk_signal(400, samples_per_symbol, 0.0, 24_000.0, 0.0);

        // Let the loop settle first.
        for &sample in &signal[..signal.len() / 2] {
            costas.process(sample);
        }

        // Measure error on the settled portion (at symbol centers).
        let mut error_sum = 0.0_f32;
        let mut count = 0;
        for (i, &sample) in signal[signal.len() / 2..].iter().enumerate() {
            let corrected = costas.process(sample);
            if i % samples_per_symbol == samples_per_symbol / 2 {
                let error = CostasLoop::phase_error(corrected);
                error_sum += error.abs();
                count += 1;
            }
        }

        let mean_error = error_sum / count as f32;
        assert!(
            mean_error < 0.1,
            "mean phase error should be small when locked: got {mean_error}"
        );
    }

    #[test]
    fn phase_error_is_zero_on_constellation_points() {
        // QPSK constellation points at +/-45, +/-135 degrees should
        // produce zero phase error.
        let points = [
            Complex::new(1.0, 1.0),   // +45 deg
            Complex::new(-1.0, 1.0),  // +135 deg
            Complex::new(-1.0, -1.0), // -135 deg
            Complex::new(1.0, -1.0),  // -45 deg
        ];

        for point in &points {
            let error = CostasLoop::phase_error(*point);
            assert!(
                error.abs() < 1e-6,
                "error should be zero on constellation: point={point}, error={error}"
            );
        }
    }

    #[test]
    fn phase_error_sign_indicates_direction() {
        // Slightly rotated clockwise from +45 deg constellation point
        // should give a positive error (telling the loop to rotate back).
        let slightly_cw = Complex::from_polar(1.0, 0.6); // ~34 deg (below +45)
        let error_cw = CostasLoop::phase_error(slightly_cw);

        let slightly_ccw = Complex::from_polar(1.0, 0.9); // ~51 deg (above +45)
        let error_ccw = CostasLoop::phase_error(slightly_ccw);

        // Errors should have opposite signs around the constellation point.
        assert!(
            error_cw * error_ccw < 0.0,
            "errors should have opposite signs: cw={error_cw}, ccw={error_ccw}"
        );
    }

    #[test]
    fn default_uses_expected_constants() {
        let costas = CostasLoop::default();
        assert!((costas.alpha - DEFAULT_ALPHA).abs() < 1e-6);
        assert!((costas.beta - DEFAULT_BETA).abs() < 1e-6);
    }

    #[test]
    fn seed_sets_phase_and_frequency() {
        let mut costas = CostasLoop::default();
        costas.seed(0.3, 0.001);
        assert!((costas.phase() - 0.3).abs() < 1e-6);
        assert!((costas.frequency() - 0.001).abs() < 1e-6);
    }

    #[test]
    fn seed_clamps_phase_to_max() {
        let mut costas = CostasLoop::default();
        costas.seed(3.0, 0.0); // exceeds MAX_PHASE (pi/2)
        assert!(costas.phase() <= MAX_PHASE + 1e-6);
    }

    #[test]
    fn seeded_loop_acquires_faster_than_cold() {
        let sample_rate = 24_000.0;
        let freq_offset = 30.0; // Hz
        let phase_offset = 0.4; // radians
        let samples_per_symbol = 5;
        let signal = generate_qpsk_signal(
            200,
            samples_per_symbol,
            freq_offset,
            sample_rate,
            phase_offset,
        );

        // Cold start: measure samples to reach low error.
        let mut cold = CostasLoop::default();
        let cold_acquisition = measure_acquisition(&mut cold, &signal, samples_per_symbol);

        // Seeded start: pre-load with the known offsets.
        let expected_freq_rad = 2.0 * PI * freq_offset / sample_rate;
        let mut seeded = CostasLoop::default();
        seeded.seed(phase_offset, expected_freq_rad);
        let seeded_acquisition = measure_acquisition(&mut seeded, &signal, samples_per_symbol);

        // Frequency-only seed (phase=0): matches what channel_manager does.
        let mut freq_only = CostasLoop::default();
        freq_only.seed(0.0, expected_freq_rad);
        let freq_only_acquisition =
            measure_acquisition(&mut freq_only, &signal, samples_per_symbol);

        println!(
            "Cold: {cold_acquisition} symbols, Freq-only seed: {freq_only_acquisition} symbols, Full seed: {seeded_acquisition} symbols"
        );
        assert!(
            seeded_acquisition < cold_acquisition,
            "seeded ({seeded_acquisition}) should acquire faster than cold ({cold_acquisition})"
        );
        assert!(
            freq_only_acquisition < cold_acquisition,
            "freq-only ({freq_only_acquisition}) should acquire faster than cold ({cold_acquisition})"
        );
    }

    /// Count the number of symbols until the mean phase error drops below
    /// a threshold, as a proxy for acquisition time.
    fn measure_acquisition(
        costas: &mut CostasLoop,
        signal: &[Complex<f32>],
        samples_per_symbol: usize,
    ) -> usize {
        let threshold = 0.15;
        let window = 10; // symbols to average over
        let mut errors: Vec<f32> = Vec::new();
        let mut symbol_count = 0;

        for (i, &sample) in signal.iter().enumerate() {
            let corrected = costas.process(sample);
            if i % samples_per_symbol == samples_per_symbol / 2 {
                errors.push(CostasLoop::phase_error(corrected).abs());
                symbol_count += 1;

                if errors.len() >= window {
                    let recent: f32 =
                        errors[errors.len() - window..].iter().sum::<f32>() / window as f32;
                    if recent < threshold {
                        return symbol_count;
                    }
                }
            }
        }
        symbol_count // didn't converge within signal length
    }
}

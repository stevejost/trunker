//! DC offset removal filter.
//!
//! Removes slowly-drifting DC offset from FM demodulator output using
//! a single-pole IIR high-pass filter. This is critical when the SDR
//! center frequency is not perfectly aligned with the P25 carrier,
//! which manifests as a DC offset in the discriminator output.

/// Single-pole IIR DC blocking filter.
///
/// Implements `y[n] = x[n] - x[n-1] + alpha * y[n-1]` where alpha
/// controls the cutoff frequency. Alpha close to 1.0 gives a very
/// low cutoff (removes only DC), while smaller values remove more
/// low-frequency content.
///
/// At 24 kHz sample rate with alpha = 0.999, the -3dB cutoff is
/// approximately 3.8 Hz, well below the 4800 baud symbol rate.
#[derive(Debug)]
pub struct DcBlocker {
    alpha: f32,
    prev_input: f32,
    prev_output: f32,
}

impl DcBlocker {
    /// Create a new DC blocking filter.
    ///
    /// `alpha` controls the cutoff frequency: higher values (closer to 1.0)
    /// mean lower cutoff. Typical values: 0.999 for slow drift removal,
    /// 0.995 for faster tracking.
    pub fn new(alpha: f32) -> Self {
        Self {
            alpha,
            prev_input: 0.0,
            prev_output: 0.0,
        }
    }

    /// Process one sample through the DC blocking filter.
    #[inline]
    pub fn process(&mut self, input: f32) -> f32 {
        let output = input - self.prev_input + self.alpha * self.prev_output;
        self.prev_input = input;
        self.prev_output = output;
        output
    }
}

impl Default for DcBlocker {
    fn default() -> Self {
        Self::new(0.999)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_dc_offset() {
        let mut filter = DcBlocker::new(0.999);
        let dc_offset = 0.5;

        // Feed constant DC. After settling, output should be near zero.
        for _ in 0..10_000 {
            filter.process(dc_offset);
        }

        let output = filter.process(dc_offset);
        assert!(output.abs() < 0.01, "DC should be removed: got {output}");
    }

    #[test]
    fn preserves_ac_signal() {
        let mut filter = DcBlocker::new(0.999);
        let sample_rate = 24_000.0;
        let tone_hz = 4800.0; // Symbol rate
        let num_samples = 10_000;

        // Let filter settle with the tone.
        for i in 0..num_samples {
            let t = i as f32 / sample_rate;
            let input = (2.0 * std::f32::consts::PI * tone_hz * t).sin();
            filter.process(input);
        }

        // Measure output amplitude over one cycle.
        let mut max_output: f32 = 0.0;
        for i in num_samples..(num_samples + 5) {
            let t = i as f32 / sample_rate;
            let input = (2.0 * std::f32::consts::PI * tone_hz * t).sin();
            let output = filter.process(input);
            max_output = max_output.max(output.abs());
        }

        // Signal at 4800 Hz should pass through with minimal attenuation.
        assert!(
            max_output > 0.9,
            "4800 Hz tone was attenuated: max = {max_output}"
        );
    }

    #[test]
    fn removes_dc_preserves_ac() {
        let mut filter = DcBlocker::new(0.999);
        let sample_rate = 24_000.0;
        let tone_hz = 2400.0;
        let dc_offset = 1.5;
        let amplitude = 0.5;

        // Let filter settle.
        for i in 0..20_000 {
            let t = i as f32 / sample_rate;
            let input = dc_offset + amplitude * (2.0 * std::f32::consts::PI * tone_hz * t).sin();
            filter.process(input);
        }

        // Measure output: should be centered near zero with ~0.5 amplitude.
        let mut min_out: f32 = f32::MAX;
        let mut max_out: f32 = f32::MIN;
        for i in 20_000..20_100 {
            let t = i as f32 / sample_rate;
            let input = dc_offset + amplitude * (2.0 * std::f32::consts::PI * tone_hz * t).sin();
            let output = filter.process(input);
            min_out = min_out.min(output);
            max_out = max_out.max(output);
        }

        let center = (max_out + min_out) / 2.0;
        let measured_amplitude = (max_out - min_out) / 2.0;

        assert!(
            center.abs() < 0.05,
            "output should be centered near zero: center = {center}"
        );
        assert!(
            (measured_amplitude - amplitude).abs() < 0.05,
            "amplitude should be preserved: got {measured_amplitude}"
        );
    }
}

//! Differential phase decoder for CQPSK (pi/4 DQPSK) demodulation.
//!
//! In CQPSK, data is encoded in phase *transitions* between successive
//! symbols rather than in absolute phase. This module computes the
//! phase change between consecutive complex symbols and maps it to a
//! dibit per TIA-102.BAAA-B Table 20.
//!
//! Phase change to dibit mapping:
//! - +135 degrees -> dibit 01 (symbol +3)
//! - +45 degrees  -> dibit 00 (symbol +1)
//! - -45 degrees  -> dibit 10 (symbol -1)
//! - -135 degrees -> dibit 11 (symbol -3)

use std::f32::consts::PI;

use num_complex::Complex;

use crate::p25::types::Dibit;

/// Differential phase decoder for pi/4 DQPSK symbols.
///
/// Computes `y[n] = x[n] * conj(x[n-1])` to extract the phase
/// transition, then maps the resulting angle to a dibit.
#[derive(Debug)]
pub struct DifferentialDecoder {
    previous: Complex<f32>,
}

impl DifferentialDecoder {
    /// Create a new differential decoder.
    pub fn new() -> Self {
        Self {
            previous: Complex::new(1.0, 0.0),
        }
    }

    /// Decode one complex symbol, returning the dibit encoded in the
    /// phase transition from the previous symbol.
    pub fn process(&mut self, sample: Complex<f32>) -> Dibit {
        let product = sample * self.previous.conj();
        self.previous = sample;
        phase_to_dibit(product.arg())
    }
}

impl Default for DifferentialDecoder {
    fn default() -> Self {
        Self::new()
    }
}

/// Map a phase angle (radians) to a dibit per TIA-102.BAAA-B Table 20.
///
/// Decision boundaries are at 0, +/-90, and +/-180 degrees, placing
/// each of the four constellation points at the center of its region.
fn phase_to_dibit(phase: f32) -> Dibit {
    if phase > PI / 2.0 {
        // +90 to +180: center at +135 deg
        Dibit::new(0b01)
    } else if phase > 0.0 {
        // 0 to +90: center at +45 deg
        Dibit::new(0b00)
    } else if phase > -PI / 2.0 {
        // -90 to 0: center at -45 deg
        Dibit::new(0b10)
    } else {
        // -180 to -90: center at -135 deg
        Dibit::new(0b11)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_4;

    /// Build a complex sample at the given phase angle (radians).
    fn at_phase(radians: f32) -> Complex<f32> {
        Complex::from_polar(1.0, radians)
    }

    #[test]
    fn phase_to_dibit_all_four_quadrants() {
        // +135 degrees -> dibit 01
        let phase_135 = 135.0_f32.to_radians();
        assert_eq!(phase_to_dibit(phase_135).bits(), 0b01);

        // +45 degrees -> dibit 00
        let phase_45 = 45.0_f32.to_radians();
        assert_eq!(phase_to_dibit(phase_45).bits(), 0b00);

        // -45 degrees -> dibit 10
        let phase_neg45 = (-45.0_f32).to_radians();
        assert_eq!(phase_to_dibit(phase_neg45).bits(), 0b10);

        // -135 degrees -> dibit 11
        let phase_neg135 = (-135.0_f32).to_radians();
        assert_eq!(phase_to_dibit(phase_neg135).bits(), 0b11);
    }

    #[test]
    fn decode_positive_135_transition() {
        let mut decoder = DifferentialDecoder::new();
        // Previous at 0 deg, current at +135 deg -> delta = +135
        let sample = at_phase(3.0 * FRAC_PI_4);
        assert_eq!(decoder.process(sample).bits(), 0b01);
    }

    #[test]
    fn decode_positive_45_transition() {
        let mut decoder = DifferentialDecoder::new();
        let sample = at_phase(FRAC_PI_4);
        assert_eq!(decoder.process(sample).bits(), 0b00);
    }

    #[test]
    fn decode_negative_45_transition() {
        let mut decoder = DifferentialDecoder::new();
        let sample = at_phase(-FRAC_PI_4);
        assert_eq!(decoder.process(sample).bits(), 0b10);
    }

    #[test]
    fn decode_negative_135_transition() {
        let mut decoder = DifferentialDecoder::new();
        let sample = at_phase(-3.0 * FRAC_PI_4);
        assert_eq!(decoder.process(sample).bits(), 0b11);
    }

    #[test]
    fn insensitive_to_constant_phase_offset() {
        // A constant phase offset should not affect differential decoding
        // because we measure *transitions*, not absolute phase.
        let offset = 1.23_f32; // arbitrary offset in radians

        let dibits_expected = [0b01, 0b00, 0b10, 0b11];
        let phase_changes = [
            3.0 * FRAC_PI_4,  // +135
            FRAC_PI_4,        // +45
            -FRAC_PI_4,       // -45
            -3.0 * FRAC_PI_4, // -135
        ];

        let mut decoder = DifferentialDecoder::new();
        // Set initial phase to the offset.
        decoder.previous = at_phase(offset);

        let mut current_phase = offset;
        for (i, &delta) in phase_changes.iter().enumerate() {
            current_phase += delta;
            let sample = at_phase(current_phase);
            let dibit = decoder.process(sample);
            assert_eq!(
                dibit.bits(),
                dibits_expected[i],
                "mismatch at symbol {i}: expected {:02b}, got {:02b}",
                dibits_expected[i],
                dibit.bits()
            );
        }
    }

    #[test]
    fn round_trip_known_dibit_sequence() {
        // Encode a known dibit sequence using TIA-102.BAAA-B Table 20
        // phase transitions, then decode and verify.
        let input_dibits: [u8; 8] = [0b01, 0b00, 0b10, 0b11, 0b00, 0b01, 0b11, 0b10];
        let phase_deltas: [f32; 8] = [
            3.0 * FRAC_PI_4,  // 01 -> +135
            FRAC_PI_4,        // 00 -> +45
            -FRAC_PI_4,       // 10 -> -45
            -3.0 * FRAC_PI_4, // 11 -> -135
            FRAC_PI_4,        // 00 -> +45
            3.0 * FRAC_PI_4,  // 01 -> +135
            -3.0 * FRAC_PI_4, // 11 -> -135
            -FRAC_PI_4,       // 10 -> -45
        ];

        // Build symbol stream from phase transitions.
        let mut symbols = Vec::with_capacity(input_dibits.len());
        let mut phase = 0.0_f32;
        for &delta in &phase_deltas {
            phase += delta;
            symbols.push(at_phase(phase));
        }

        // Decode and verify round-trip.
        let mut decoder = DifferentialDecoder::new();
        for (i, &symbol) in symbols.iter().enumerate() {
            let dibit = decoder.process(symbol);
            assert_eq!(
                dibit.bits(),
                input_dibits[i],
                "round-trip mismatch at symbol {i}: expected {:02b}, got {:02b}",
                input_dibits[i],
                dibit.bits()
            );
        }
    }

    #[test]
    fn table_21_phase_state_transitions() {
        // Verify against TIA-102.BAAA-B Table 21 CQPSK I and Q Lookup.
        // Phase states 0-7 represent 0, 45, 90, 135, 180, 225, 270, 315 deg.
        // Starting at state 0, information bits 00 -> state 1 (delta = +45 deg).
        // Starting at state 0, information bits 01 -> state 3 (delta = +135 deg).
        // Starting at state 0, information bits 11 -> state 5 (delta = -135/+225 deg).
        // Starting at state 0, information bits 10 -> state 7 (delta = -45/+315 deg).

        let state_to_phase = |state: u8| -> f32 { (state as f32) * FRAC_PI_4 };

        let cases: [(u8, u8, u8, u8); 8] = [
            // (current_state, dibit, expected_next_state, _)
            (0, 0b00, 1, 0b00),
            (0, 0b01, 3, 0b01),
            (0, 0b11, 5, 0b11),
            (0, 0b10, 7, 0b10),
            (1, 0b00, 2, 0b00),
            (1, 0b01, 4, 0b01),
            (1, 0b11, 6, 0b11),
            (1, 0b10, 0, 0b10),
        ];

        for &(current_state, dibit, next_state, expected_dibit) in &cases {
            let mut decoder = DifferentialDecoder::new();
            decoder.previous = at_phase(state_to_phase(current_state));
            let sample = at_phase(state_to_phase(next_state));
            let result = decoder.process(sample);
            assert_eq!(
                result.bits(),
                expected_dibit,
                "state {} + dibit {:02b} -> state {}: expected {:02b}, got {:02b}",
                current_state,
                dibit,
                next_state,
                expected_dibit,
                result.bits()
            );
        }
    }
}

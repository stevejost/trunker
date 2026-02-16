//! CQPSK (pi/4 DQPSK) test signal modulator.
//!
//! Generates synthetic CQPSK baseband signals from a dibit sequence
//! using the TIA-102.BAAA-B Table 21 I/Q lookup table. Used for
//! testing the CQPSK demodulation chain end-to-end.

use std::f32::consts::{FRAC_1_SQRT_2, PI};

use num_complex::Complex;

use crate::p25::types::Dibit;

/// Number of phase states in the pi/4 DQPSK constellation.
const NUM_PHASE_STATES: usize = 8;

/// Shorthand for 1/sqrt(2) used in the I/Q lookup table.
const S: f32 = FRAC_1_SQRT_2;

/// Next-state table from TIA-102.BAAA-B Table 21.
///
/// Indexed as `NEXT_STATE[current_state][dibit_bits]`.
/// Dibit ordering: 00=0, 01=1, 10=2, 11=3.
const NEXT_STATE: [[u8; 4]; NUM_PHASE_STATES] = [
    [1, 3, 7, 5], // state 0: 00->1, 01->3, 10->7, 11->5
    [2, 4, 0, 6], // state 1
    [3, 5, 1, 7], // state 2
    [4, 6, 2, 0], // state 3
    [5, 7, 3, 1], // state 4
    [6, 0, 4, 2], // state 5
    [7, 1, 5, 3], // state 6
    [0, 2, 6, 4], // state 7
];

/// I/Q levels from TIA-102.BAAA-B Table 21.
///
/// Indexed as `IQ_LEVELS[current_state][dibit_bits]` -> (I, Q).
/// Values are exact: 1/sqrt(2) for diagonal points, 0.0/1.0 for axis points.
const IQ_LEVELS: [[(f32, f32); 4]; NUM_PHASE_STATES] = [
    [( S,  S), (-S,  S), ( S, -S), (-S, -S)], // state 0
    [( 0.0,  1.0), (-1.0,  0.0), ( 1.0,  0.0), ( 0.0, -1.0)], // state 1
    [(-S,  S), (-S, -S), ( S,  S), ( S, -S)], // state 2
    [(-1.0,  0.0), ( 0.0, -1.0), ( 0.0,  1.0), ( 1.0,  0.0)], // state 3
    [(-S, -S), ( S, -S), (-S,  S), ( S,  S)], // state 4
    [( 0.0, -1.0), ( 1.0,  0.0), (-1.0,  0.0), ( 0.0,  1.0)], // state 5
    [( S, -S), ( S,  S), (-S, -S), (-S,  S)], // state 6
    [( 1.0,  0.0), ( 0.0,  1.0), ( 0.0, -1.0), (-1.0,  0.0)], // state 7
];

/// Modulate a dibit sequence into CQPSK complex baseband samples.
///
/// Produces `dibits.len() * samples_per_symbol` output samples.
/// Each symbol is upsampled (zero-stuffed) and RRC pulse-shaped.
///
/// * `dibits` - Input dibit sequence to modulate.
/// * `samples_per_symbol` - Number of output samples per symbol.
/// * `excess_bandwidth` - RRC roll-off factor (0.2 for P25).
pub fn cqpsk_modulate(
    dibits: &[Dibit],
    samples_per_symbol: usize,
    excess_bandwidth: f32,
) -> Vec<Complex<f32>> {
    // Map dibits to constellation points via Table 21.
    let symbols = dibits_to_symbols(dibits);

    // Upsample: insert zeros between symbols.
    let upsampled_len = symbols.len() * samples_per_symbol;
    let mut upsampled = vec![Complex::new(0.0, 0.0); upsampled_len];
    for (i, &sym) in symbols.iter().enumerate() {
        upsampled[i * samples_per_symbol] = sym;
    }

    // Design RRC filter for pulse shaping.
    let filter_span_symbols = 5;
    let num_taps = 2 * filter_span_symbols * samples_per_symbol + 1;
    let coefficients = design_rrc(samples_per_symbol, excess_bandwidth, num_taps);

    // Apply RRC filter (convolution).
    convolve_complex(&upsampled, &coefficients)
}

/// Map a dibit sequence to constellation points using Table 21.
///
/// Returns one complex sample per dibit, representing the ideal
/// (unfiltered) symbol value.
pub fn dibits_to_symbols(dibits: &[Dibit]) -> Vec<Complex<f32>> {
    let mut state: usize = 0;
    let mut symbols = Vec::with_capacity(dibits.len());

    for dibit in dibits {
        let idx = dibit.bits() as usize;
        let (i_level, q_level) = IQ_LEVELS[state][idx];
        symbols.push(Complex::new(i_level, q_level));
        state = NEXT_STATE[state][idx] as usize;
    }

    symbols
}

/// Design RRC filter coefficients (same algorithm as rrc_filter.rs).
fn design_rrc(samples_per_symbol: usize, alpha: f32, num_taps: usize) -> Vec<f32> {
    let sps = samples_per_symbol as f32;
    let center = (num_taps - 1) as f32 / 2.0;

    let mut coefficients: Vec<f32> = (0..num_taps)
        .map(|i| {
            let t = (i as f32 - center) / sps;

            if t.abs() < 1e-7 {
                1.0 - alpha + 4.0 * alpha / PI
            } else if (t.abs() - 1.0 / (4.0 * alpha)).abs() < 1e-7 {
                (alpha / 2.0_f32.sqrt())
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

/// Convolve a complex signal with real-valued filter coefficients.
fn convolve_complex(signal: &[Complex<f32>], coefficients: &[f32]) -> Vec<Complex<f32>> {
    let half = coefficients.len() / 2;
    let mut output = Vec::with_capacity(signal.len());

    for i in 0..signal.len() {
        let mut sum = Complex::new(0.0, 0.0);
        for (j, &coeff) in coefficients.iter().enumerate() {
            let sig_idx = i as isize + j as isize - half as isize;
            if sig_idx >= 0 && (sig_idx as usize) < signal.len() {
                sum += signal[sig_idx as usize] * coeff;
            }
        }
        output.push(sum);
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Phase angle for each state (radians). State n = n * pi/4.
    const PHASE_STATES: [f32; NUM_PHASE_STATES] = [
        0.0,
        PI / 4.0,
        PI / 2.0,
        3.0 * PI / 4.0,
        PI,
        -3.0 * PI / 4.0, // 5*pi/4 = -3*pi/4
        -PI / 2.0,        // 6*pi/4 = -pi/2
        -PI / 4.0,        // 7*pi/4 = -pi/4
    ];

    #[test]
    fn next_state_table_matches_table_21() {
        // Verify a selection of transitions from Table 21.
        // State 0, dibit 00 -> state 1
        assert_eq!(NEXT_STATE[0][0b00], 1);
        // State 0, dibit 01 -> state 3
        assert_eq!(NEXT_STATE[0][0b01], 3);
        // State 0, dibit 11 -> state 5
        assert_eq!(NEXT_STATE[0][0b11], 5);
        // State 0, dibit 10 -> state 7
        assert_eq!(NEXT_STATE[0][0b10], 7);
        // State 3, dibit 00 -> state 4
        assert_eq!(NEXT_STATE[3][0b00], 4);
        // State 7, dibit 10 -> state 6
        assert_eq!(NEXT_STATE[7][0b10], 6);
    }

    #[test]
    fn iq_levels_match_table_21() {
        // State 0, dibit 00: I=1/sqrt(2), Q=1/sqrt(2)
        let (i, q) = IQ_LEVELS[0][0b00];
        assert!((i - S).abs() < 1e-6, "I={i}");
        assert!((q - S).abs() < 1e-6, "Q={q}");

        // State 1, dibit 00: I=0.0, Q=1.0
        let (i, q) = IQ_LEVELS[1][0b00];
        assert!(i.abs() < 1e-6, "I={i}");
        assert!((q - 1.0).abs() < 1e-6, "Q={q}");

        // State 4, dibit 01: I=1/sqrt(2), Q=-1/sqrt(2)
        let (i, q) = IQ_LEVELS[4][0b01];
        assert!((i - S).abs() < 1e-6, "I={i}");
        assert!((q - (-S)).abs() < 1e-6, "Q={q}");

        // State 5, dibit 11: I=0.0, Q=1.0
        let (i, q) = IQ_LEVELS[5][0b11];
        assert!(i.abs() < 1e-6, "I={i}");
        assert!((q - 1.0).abs() < 1e-6, "Q={q}");
    }

    #[test]
    fn iq_levels_consistent_with_phase_states() {
        // Each I/Q level should match the phase of the next state.
        for state in 0..NUM_PHASE_STATES {
            for dibit in 0..4_usize {
                let next = NEXT_STATE[state][dibit] as usize;
                let expected_phase = PHASE_STATES[next];
                let (i, q) = IQ_LEVELS[state][dibit];
                let actual_phase = q.atan2(i);

                let mut diff = (actual_phase - expected_phase).abs();
                if diff > PI {
                    diff = 2.0 * PI - diff;
                }

                assert!(
                    diff < 0.01,
                    "state {state} dibit {dibit:02b}: expected phase {expected_phase:.3}, \
                     got {actual_phase:.3} from I={i} Q={q}"
                );
            }
        }
    }

    #[test]
    fn phase_transitions_match_table_20() {
        // Table 20: dibit 01 -> +135 deg, 00 -> +45 deg,
        //           10 -> -45 deg, 11 -> -135 deg.
        let expected_deltas: [(u8, f32); 4] = [
            (0b01, 135.0),
            (0b00, 45.0),
            (0b10, -45.0),
            (0b11, -135.0),
        ];

        // Check from every starting state.
        for state in 0..NUM_PHASE_STATES {
            for &(dibit, expected_deg) in &expected_deltas {
                let next = NEXT_STATE[state][dibit as usize] as usize;
                let current_phase = PHASE_STATES[state];
                let next_phase = PHASE_STATES[next];

                let mut delta = next_phase - current_phase;
                // Normalize to (-pi, pi].
                while delta > PI {
                    delta -= 2.0 * PI;
                }
                while delta <= -PI {
                    delta += 2.0 * PI;
                }

                let expected_rad = expected_deg.to_radians();
                let mut diff = (delta - expected_rad).abs();
                if diff > PI {
                    diff = 2.0 * PI - diff;
                }

                assert!(
                    diff < 0.01,
                    "state {state} dibit {dibit:02b}: expected delta {expected_deg} deg, \
                     got {:.1} deg",
                    delta.to_degrees()
                );
            }
        }
    }

    #[test]
    fn dibits_to_symbols_produces_unit_magnitude() {
        let dibits: Vec<Dibit> = (0..20).map(|i| Dibit::new(i % 4)).collect();
        let symbols = dibits_to_symbols(&dibits);

        for (i, sym) in symbols.iter().enumerate() {
            let mag = sym.norm();
            assert!(
                (mag - 1.0).abs() < 0.01,
                "symbol {i} magnitude should be ~1.0: got {mag}"
            );
        }
    }

    #[test]
    fn modulate_produces_correct_length() {
        let dibits: Vec<Dibit> = vec![Dibit::new(0b01); 10];
        let sps = 5;
        let output = cqpsk_modulate(&dibits, sps, 0.2);
        assert_eq!(output.len(), dibits.len() * sps);
    }

    #[test]
    fn modulate_round_trip_with_diff_decoder() {
        use crate::dsp::diff_decoder::DifferentialDecoder;

        let input_dibits: Vec<Dibit> = [0b01, 0b00, 0b10, 0b11, 0b00, 0b01, 0b11, 0b10]
            .iter()
            .map(|&b| Dibit::new(b))
            .collect();

        // Modulate without pulse shaping (1 sample per symbol) for
        // clean round-trip test.
        let symbols = dibits_to_symbols(&input_dibits);

        // Decode with differential decoder.
        let mut decoder = DifferentialDecoder::new();
        let mut output_dibits = Vec::new();
        for &sym in &symbols {
            output_dibits.push(decoder.process(sym));
        }

        for (i, (input, output)) in input_dibits.iter().zip(output_dibits.iter()).enumerate() {
            assert_eq!(
                input.bits(),
                output.bits(),
                "round-trip mismatch at symbol {i}: expected {:02b}, got {:02b}",
                input.bits(),
                output.bits()
            );
        }
    }

    #[test]
    fn modulate_round_trip_with_pulse_shaping() {
        use crate::dsp::diff_decoder::DifferentialDecoder;

        // Longer sequence to let the filter settle.
        let input_dibits: Vec<Dibit> = [
            0b01, 0b00, 0b10, 0b11, 0b00, 0b01, 0b11, 0b10,
            0b01, 0b00, 0b10, 0b11, 0b00, 0b01, 0b11, 0b10,
            0b01, 0b00, 0b10, 0b11, 0b00, 0b01, 0b11, 0b10,
            0b01, 0b00, 0b10, 0b11, 0b00, 0b01, 0b11, 0b10,
        ]
        .iter()
        .map(|&b| Dibit::new(b))
        .collect();

        let sps = 5;
        let excess_bw = 0.2;
        let tx_output = cqpsk_modulate(&input_dibits, sps, excess_bw);

        // Apply matched RRC filter on the receive side. Transmit RRC +
        // receive RRC = raised cosine, which has zero ISI at symbol centers.
        let filter_span_symbols = 5;
        let num_taps = 2 * filter_span_symbols * sps + 1;
        let rx_coefficients = design_rrc(sps, excess_bw, num_taps);
        let rx_output = convolve_complex(&tx_output, &rx_coefficients);

        // The convolve_complex function is centered (subtracts half from
        // the index), so there is no net group delay — symbol k peaks at
        // sample index k * sps.
        let mut decoder = DifferentialDecoder::new();
        let mut decoded = Vec::new();
        for i in 0..input_dibits.len() {
            let sample_idx = i * sps;
            if sample_idx < rx_output.len() {
                decoded.push(decoder.process(rx_output[sample_idx]));
            }
        }

        // Skip the first few symbols (filter transient) and check the rest.
        let skip = 6;
        let mut correct = 0;
        let mut total = 0;
        for i in skip..decoded.len().min(input_dibits.len()) {
            total += 1;
            if decoded[i].bits() == input_dibits[i].bits() {
                correct += 1;
            }
        }

        assert!(total > 0, "should have symbols to check");
        let accuracy = correct as f32 / total as f32;
        assert!(
            accuracy > 0.9,
            "round-trip accuracy with matched RRC should be >90%: got {correct}/{total} ({:.0}%)",
            accuracy * 100.0
        );
    }
}

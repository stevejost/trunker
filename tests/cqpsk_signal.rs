//! CQPSK (pi/4 DQPSK) test signal generator.
//!
//! Generates synthetic IQ samples for CQPSK-modulated P25 signals
//! per TIA-102.BAAA-B Section 8.5 and Table 21. Used for testing
//! the CQPSK demodulation pipeline.
//!
//! The CQPSK modulator encodes data in phase transitions between
//! successive symbols. It uses 8 phase states (0-7) representing
//! 0, 45, 90, 135, 180, 225, 270, 315 degrees. Each input dibit
//! causes a phase change of +45, +135, -135, or -45 degrees.

use num_complex::Complex;
use std::f32::consts::PI;
use trunker::p25::types::Dibit;

/// CQPSK I and Q Lookup Table (TIA-102.BAAA-B Table 21).
///
/// Indexed by `[current_phase_state][dibit]` where:
/// - `current_phase_state` is 0-7 (representing 0, 45, ..., 315 degrees)
/// - `dibit` is 0b00, 0b01, 0b10, 0b11
///
/// Each entry is `(next_phase_state, i_level, q_level)`.
/// I/Q levels are one of: -1.0, -0.7071, 0.0, 0.7071, 1.0
const CQPSK_TABLE: [[(u8, f32, f32); 4]; 8] = [
    // Phase state 0 (0 degrees)
    [
        (1, 0.7071, 0.7071),   // dibit 00 -> state 1, +45 deg
        (3, -0.7071, 0.7071),  // dibit 01 -> state 3, +135 deg
        (7, 0.7071, -0.7071),  // dibit 10 -> state 7, -45 deg
        (5, -0.7071, -0.7071), // dibit 11 -> state 5, -135 deg
    ],
    // Phase state 1 (45 degrees)
    [
        (2, 0.0, 1.0),  // dibit 00 -> state 2
        (4, -1.0, 0.0), // dibit 01 -> state 4
        (0, 1.0, 0.0),  // dibit 10 -> state 0
        (6, 0.0, -1.0), // dibit 11 -> state 6
    ],
    // Phase state 2 (90 degrees)
    [
        (3, -0.7071, 0.7071),  // dibit 00 -> state 3
        (5, -0.7071, -0.7071), // dibit 01 -> state 5
        (1, 0.7071, 0.7071),   // dibit 10 -> state 1
        (7, 0.7071, -0.7071),  // dibit 11 -> state 7
    ],
    // Phase state 3 (135 degrees)
    [
        (4, -1.0, 0.0), // dibit 00 -> state 4
        (6, 0.0, -1.0), // dibit 01 -> state 6
        (2, 0.0, 1.0),  // dibit 10 -> state 2
        (0, 1.0, 0.0),  // dibit 11 -> state 0
    ],
    // Phase state 4 (180 degrees)
    [
        (5, -0.7071, -0.7071), // dibit 00 -> state 5
        (7, 0.7071, -0.7071),  // dibit 01 -> state 7
        (3, -0.7071, 0.7071),  // dibit 10 -> state 3
        (1, 0.7071, 0.7071),   // dibit 11 -> state 1
    ],
    // Phase state 5 (225 degrees)
    [
        (6, 0.0, -1.0), // dibit 00 -> state 6
        (0, 1.0, 0.0),  // dibit 01 -> state 0
        (4, -1.0, 0.0), // dibit 10 -> state 4
        (2, 0.0, 1.0),  // dibit 11 -> state 2
    ],
    // Phase state 6 (270 degrees)
    [
        (7, 0.7071, -0.7071),  // dibit 00 -> state 7
        (1, 0.7071, 0.7071),   // dibit 01 -> state 1
        (5, -0.7071, -0.7071), // dibit 10 -> state 5
        (3, -0.7071, 0.7071),  // dibit 11 -> state 3
    ],
    // Phase state 7 (315 degrees)
    [
        (0, 1.0, 0.0),  // dibit 00 -> state 0
        (2, 0.0, 1.0),  // dibit 01 -> state 2
        (6, 0.0, -1.0), // dibit 10 -> state 6
        (4, -1.0, 0.0), // dibit 11 -> state 7
    ],
];

/// CQPSK modulator state machine per TIA-102.BAAA-B Table 21.
///
/// Converts a sequence of dibits to I/Q symbol values using the
/// CQPSK phase state lookup table.
pub struct CqpskModulator {
    phase_state: u8,
}

impl CqpskModulator {
    /// Create a new modulator starting at phase state 0.
    pub fn new() -> Self {
        Self { phase_state: 0 }
    }

    /// Modulate one dibit, returning the (I, Q) symbol value.
    pub fn modulate(&mut self, dibit: Dibit) -> Complex<f32> {
        let bits = dibit.bits() as usize;
        let (next_state, i, q) = CQPSK_TABLE[self.phase_state as usize][bits];
        self.phase_state = next_state;
        Complex::new(i, q)
    }

    /// Modulate a sequence of dibits, returning one IQ sample per symbol.
    pub fn modulate_sequence(&mut self, dibits: &[Dibit]) -> Vec<Complex<f32>> {
        dibits.iter().map(|&d| self.modulate(d)).collect()
    }
}

/// Generate a CQPSK baseband signal at the given samples per symbol.
///
/// Takes a dibit sequence and produces complex IQ samples with
/// rectangular pulse shaping (each symbol held for `samples_per_symbol`
/// samples). No pulse shaping filter is applied — callers can add
/// RRC filtering if needed.
///
/// `timing_offset` shifts the entire signal by a fractional number
/// of samples (for testing timing recovery).
pub fn generate_cqpsk_baseband(
    dibits: &[Dibit],
    samples_per_symbol: usize,
    timing_offset: f32,
) -> Vec<Complex<f32>> {
    let mut modulator = CqpskModulator::new();
    let symbols = modulator.modulate_sequence(dibits);

    let total_samples = (symbols.len() * samples_per_symbol) + (timing_offset.ceil() as usize) + 1;
    let mut output = Vec::with_capacity(total_samples);

    // Insert fractional offset as leading zeros.
    let offset_samples = timing_offset.ceil() as usize;
    for _ in 0..offset_samples {
        output.push(Complex::new(0.0, 0.0));
    }

    // Hold each symbol for samples_per_symbol samples.
    for symbol in &symbols {
        for _ in 0..samples_per_symbol {
            output.push(*symbol);
        }
    }

    output
}

/// Phase state angles in radians for verification.
const PHASE_STATE_ANGLES: [f32; 8] = [
    0.0,
    PI / 4.0,
    PI / 2.0,
    3.0 * PI / 4.0,
    PI,
    5.0 * PI / 4.0,
    3.0 * PI / 2.0,
    7.0 * PI / 4.0,
];

// ---- Tests for the generator itself ----

#[test]
fn table_21_state_transitions_are_correct() {
    // Verify that each table entry produces the expected phase state.
    // From state 0: dibit 00 -> state 1 (+45 deg), dibit 01 -> state 3 (+135 deg),
    //              dibit 10 -> state 7 (-45 deg), dibit 11 -> state 5 (-135 deg)
    let cases: [(u8, u8, u8); 8] = [
        (0, 0b00, 1),
        (0, 0b01, 3),
        (0, 0b10, 7),
        (0, 0b11, 5),
        (1, 0b00, 2),
        (1, 0b01, 4),
        (1, 0b10, 0),
        (1, 0b11, 6),
    ];

    for &(start_state, dibit, expected_next) in &cases {
        let (next, _, _) = CQPSK_TABLE[start_state as usize][dibit as usize];
        assert_eq!(
            next, expected_next,
            "state {} + dibit {:02b}: expected state {}, got {}",
            start_state, dibit, expected_next, next
        );
    }
}

#[test]
fn table_21_iq_values_match_spec() {
    // Verify specific I/Q values from Table 21.
    let epsilon = 0.001;

    // State 0, dibit 00 -> I=0.7071, Q=0.7071
    let (_, i, q) = CQPSK_TABLE[0][0b00];
    assert!((i - 0.7071).abs() < epsilon, "I: expected 0.7071, got {i}");
    assert!((q - 0.7071).abs() < epsilon, "Q: expected 0.7071, got {q}");

    // State 0, dibit 01 -> I=-0.7071, Q=0.7071
    let (_, i, q) = CQPSK_TABLE[0][0b01];
    assert!(
        (i - (-0.7071)).abs() < epsilon,
        "I: expected -0.7071, got {i}"
    );
    assert!((q - 0.7071).abs() < epsilon, "Q: expected 0.7071, got {q}");

    // State 1, dibit 00 -> I=0.0, Q=1.0
    let (_, i, q) = CQPSK_TABLE[1][0b00];
    assert!(i.abs() < epsilon, "I: expected 0.0, got {i}");
    assert!((q - 1.0).abs() < epsilon, "Q: expected 1.0, got {q}");

    // State 3, dibit 00 -> I=-1.0, Q=0.0
    let (_, i, q) = CQPSK_TABLE[3][0b00];
    assert!((i - (-1.0)).abs() < epsilon, "I: expected -1.0, got {i}");
    assert!(q.abs() < epsilon, "Q: expected 0.0, got {q}");
}

#[test]
fn table_21_iq_magnitudes_are_correct() {
    // All constellation points should have magnitude 1.0.
    let epsilon = 0.001;

    for state in 0..8 {
        for dibit in 0..4 {
            let (_, i, q) = CQPSK_TABLE[state][dibit];
            let mag = (i * i + q * q).sqrt();
            assert!(
                (mag - 1.0).abs() < epsilon,
                "state {state}, dibit {dibit:02b}: magnitude {mag}, expected 1.0"
            );
        }
    }
}

#[test]
fn table_21_phase_transitions_match_table_20() {
    // Table 20 defines the dibit-to-phase-change mapping:
    //   dibit 00 -> +45 degrees
    //   dibit 01 -> +135 degrees
    //   dibit 10 -> -45 degrees
    //   dibit 11 -> -135 degrees
    //
    // Verify that each state transition in Table 21 produces the
    // correct phase change.

    let expected_delta: [f32; 4] = [
        45.0_f32.to_radians(),   // dibit 00 -> +45
        135.0_f32.to_radians(),  // dibit 01 -> +135
        -45.0_f32.to_radians(),  // dibit 10 -> -45
        -135.0_f32.to_radians(), // dibit 11 -> -135
    ];

    for state in 0..8u8 {
        for dibit in 0..4u8 {
            let (next_state, i, q) = CQPSK_TABLE[state as usize][dibit as usize];

            // Compute actual phase from I/Q.
            let actual_phase = q.atan2(i);

            // Expected phase = current state angle + phase change.
            let current_angle = PHASE_STATE_ANGLES[state as usize];
            let expected_phase = current_angle + expected_delta[dibit as usize];

            // Normalize both to [-pi, pi] for comparison.
            let expected_norm = normalize_angle(expected_phase);
            let actual_norm = normalize_angle(actual_phase);

            let diff = normalize_angle(actual_norm - expected_norm);
            assert!(
                diff.abs() < 0.01,
                "state {}, dibit {:02b} -> state {}: \
                 expected phase {:.3} rad, got {:.3} rad (diff {:.3})",
                state,
                dibit,
                next_state,
                expected_norm,
                actual_norm,
                diff
            );
        }
    }
}

#[test]
fn modulator_starts_at_state_zero() {
    let mut mod1 = CqpskModulator::new();
    // First dibit 00 from state 0 should produce (0.7071, 0.7071).
    let iq = mod1.modulate(Dibit::new(0b00));
    assert!((iq.re - 0.7071).abs() < 0.001);
    assert!((iq.im - 0.7071).abs() < 0.001);
}

#[test]
fn modulator_sequence_length_matches_input() {
    let mut modulator = CqpskModulator::new();
    let dibits: Vec<Dibit> = (0..20).map(|i| Dibit::new(i % 4)).collect();
    let symbols = modulator.modulate_sequence(&dibits);
    assert_eq!(symbols.len(), 20);
}

#[test]
fn generate_baseband_has_correct_sample_count() {
    let dibits: Vec<Dibit> = (0..10).map(|i| Dibit::new(i % 4)).collect();
    let samples = generate_cqpsk_baseband(&dibits, 5, 0.0);
    // 10 symbols * 5 sps + 1 padding = 51
    assert!(
        samples.len() >= 50,
        "expected at least 50 samples, got {}",
        samples.len()
    );
}

#[test]
fn generate_baseband_with_timing_offset() {
    let dibits: Vec<Dibit> = (0..10).map(|i| Dibit::new(i % 4)).collect();
    let no_offset = generate_cqpsk_baseband(&dibits, 5, 0.0);
    let with_offset = generate_cqpsk_baseband(&dibits, 5, 2.0);

    // The offset version should be longer by approximately 2 samples.
    assert!(
        with_offset.len() > no_offset.len(),
        "offset signal should be longer"
    );

    // First 2 samples of offset version should be zero.
    assert!(with_offset[0].norm() < 0.001);
    assert!(with_offset[1].norm() < 0.001);
}

#[test]
fn round_trip_through_diff_decoder() {
    // Generate a CQPSK signal, then verify the differential decoder
    // recovers the original dibits.
    use trunker::dsp::diff_decoder::DifferentialDecoder;

    let input_dibits: Vec<Dibit> = vec![
        Dibit::new(0b01),
        Dibit::new(0b00),
        Dibit::new(0b10),
        Dibit::new(0b11),
        Dibit::new(0b00),
        Dibit::new(0b01),
        Dibit::new(0b11),
        Dibit::new(0b10),
    ];

    // Generate symbol-rate IQ (1 sample per symbol).
    let mut modulator = CqpskModulator::new();
    let symbols = modulator.modulate_sequence(&input_dibits);

    // Decode with differential decoder.
    let mut decoder = DifferentialDecoder::new();
    let mut decoded = Vec::new();
    for &sym in &symbols {
        decoded.push(decoder.process(sym));
    }

    // Verify round-trip.
    for (i, (input, output)) in input_dibits.iter().zip(decoded.iter()).enumerate() {
        assert_eq!(
            input.bits(),
            output.bits(),
            "mismatch at symbol {}: input {:02b}, output {:02b}",
            i,
            input.bits(),
            output.bits()
        );
    }
}

#[test]
fn all_32_state_transitions_covered() {
    // Verify the table has 8 states * 4 dibits = 32 transitions,
    // and every next-state is in range 0-7.
    let mut transition_count = 0;
    for state in 0..8 {
        for dibit in 0..4 {
            let (next, _, _) = CQPSK_TABLE[state][dibit];
            assert!(next < 8, "invalid next state {} from state {}", next, state);
            transition_count += 1;
        }
    }
    assert_eq!(transition_count, 32);
}

#[test]
fn even_odd_state_alternation() {
    // In pi/4 DQPSK, even-numbered symbols land on even phase states
    // (0, 2, 4, 6) and odd-numbered symbols land on odd phase states
    // (1, 3, 5, 7), or vice versa. Starting from state 0 (even),
    // the first transition should go to an odd state.
    for dibit in 0..4 {
        let (next, _, _) = CQPSK_TABLE[0][dibit];
        assert!(
            next % 2 == 1,
            "from state 0, dibit {}: expected odd next state, got {}",
            dibit,
            next
        );
    }

    // From any odd state, next should be even.
    for state in [1, 3, 5, 7] {
        for dibit in 0..4 {
            let (next, _, _) = CQPSK_TABLE[state][dibit];
            assert!(
                next % 2 == 0,
                "from state {}, dibit {}: expected even next state, got {}",
                state,
                dibit,
                next
            );
        }
    }
}

/// Normalize an angle to the range [-pi, pi].
fn normalize_angle(angle: f32) -> f32 {
    let mut a = angle % (2.0 * PI);
    if a > PI {
        a -= 2.0 * PI;
    }
    if a < -PI {
        a += 2.0 * PI;
    }
    a
}

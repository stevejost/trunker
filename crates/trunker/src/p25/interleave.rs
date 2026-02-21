//! Data payload deinterleaving for TSBK packets.
//!
//! The 98 coded dibits in a TSBK data unit are interleaved before transmission.
//! This module reverses that interleaving so the dibits can be passed to the
//! trellis decoder in the correct order.

use crate::p25::consts::CODING_DIBITS;
use crate::p25::types::Dibit;

/// Deinterleave permutation table (98 entries).
///
/// Maps transmitted position to original position.
const DEINTERLEAVE_TABLE: [usize; CODING_DIBITS] = [
    0, 1, 26, 27, 50, 51, 74, 75, 2, 3, 28, 29, 52, 53, 76, 77, 4, 5, 30, 31, 54, 55, 78, 79, 6, 7,
    32, 33, 56, 57, 80, 81, 8, 9, 34, 35, 58, 59, 82, 83, 10, 11, 36, 37, 60, 61, 84, 85, 12, 13,
    38, 39, 62, 63, 86, 87, 14, 15, 40, 41, 64, 65, 88, 89, 16, 17, 42, 43, 66, 67, 90, 91, 18, 19,
    44, 45, 68, 69, 92, 93, 20, 21, 46, 47, 70, 71, 94, 95, 22, 23, 48, 49, 72, 73, 96, 97, 24, 25,
];

/// Deinterleave 98 coded dibits from transmitted order to original order.
///
/// Takes dibits in the order they were received (after status symbol removal)
/// and returns them in the order expected by the trellis decoder.
///
/// The table maps output position to input position: output[i] comes from
/// received[DEINTERLEAVE_TABLE[i]].
pub fn deinterleave(received: &[Dibit; CODING_DIBITS]) -> [Dibit; CODING_DIBITS] {
    let mut output = [Dibit::new(0); CODING_DIBITS];
    for (output_index, &input_index) in DEINTERLEAVE_TABLE.iter().enumerate() {
        output[output_index] = received[input_index];
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deinterleave_table_is_valid_permutation() {
        let mut seen = [false; CODING_DIBITS];
        for &index in &DEINTERLEAVE_TABLE {
            assert!(index < CODING_DIBITS, "index {index} out of range");
            assert!(!seen[index], "duplicate index {index}");
            seen[index] = true;
        }
    }

    #[test]
    fn deinterleave_matches_reference_table() {
        let expected: [usize; 98] = [
            0, 1, 26, 27, 50, 51, 74, 75, 2, 3, 28, 29, 52, 53, 76, 77, 4, 5, 30, 31, 54, 55, 78,
            79, 6, 7, 32, 33, 56, 57, 80, 81, 8, 9, 34, 35, 58, 59, 82, 83, 10, 11, 36, 37, 60, 61,
            84, 85, 12, 13, 38, 39, 62, 63, 86, 87, 14, 15, 40, 41, 64, 65, 88, 89, 16, 17, 42, 43,
            66, 67, 90, 91, 18, 19, 44, 45, 68, 69, 92, 93, 20, 21, 46, 47, 70, 71, 94, 95, 22, 23,
            48, 49, 72, 73, 96, 97, 24, 25,
        ];
        assert_eq!(DEINTERLEAVE_TABLE, expected);
    }

    #[test]
    fn deinterleave_specific_positions() {
        // Verify specific position mappings from the table.
        // Output position 0 comes from input position 0.
        assert_eq!(DEINTERLEAVE_TABLE[0], 0);
        // Output position 2 comes from input position 26.
        assert_eq!(DEINTERLEAVE_TABLE[2], 26);
        // Output position 96 comes from input position 24.
        assert_eq!(DEINTERLEAVE_TABLE[96], 24);
        // Output position 97 comes from input position 25.
        assert_eq!(DEINTERLEAVE_TABLE[97], 25);
    }

    #[test]
    fn deinterleave_preserves_all_dibits() {
        // Every dibit in the input should appear exactly once in the output.
        let mut input = [Dibit::new(0); CODING_DIBITS];
        for i in 0..CODING_DIBITS {
            input[i] = Dibit::new((i % 4) as u8);
        }

        let output = deinterleave(&input);

        // Count occurrences of each dibit value in output.
        let mut counts = [0usize; 4];
        for d in &output {
            counts[d.bits() as usize] += 1;
        }
        // Same distribution as input: 25 each for 0,1 and 24 each for 2,3.
        let mut input_counts = [0usize; 4];
        for d in &input {
            input_counts[d.bits() as usize] += 1;
        }
        assert_eq!(counts, input_counts);
    }
}

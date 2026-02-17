//! Shortened (16, 8, 5) cyclic code for P25 low-speed data.
//!
//! This code is shortened from the base (17, 9, 5) cyclic code with generator
//! polynomial g(x) = x^8 + x^5 + x^4 + x^3 + 1. It can detect up to 4
//! errors and correct up to 2 errors.
//!
//! Used for low-speed data fragments in LDU1/LDU2 voice superframes.
//!
//! The decoding algorithm uses cyclic rotation with syndrome lookup, based on
//! Lin and Costello's *Error Control Coding* (1983) and Roman's *Coding and
//! Information Theory* (1992).
//!
//! The key insight that this is shortened from a (17, 9, 5) code comes from
//! Simon, "Standard APCO25 Physical Layer of the Radio Transmission Chain",
//! 2014.

/// Encode the given 8 data bits into a 16-bit codeword.
///
/// The input must have only the 8 least significant bits set.
/// Returns `[data (8 bits) | parity (8 bits)]`.
pub fn encode(data: u8) -> u16 {
    let base = encode_base(data as u16);
    // Drop the MSB (bit 16) which is always 0 for shortened code.
    debug_assert!(base >> 16 == 0, "MSB must be zero for shortened code");
    base as u16
}

/// Try to decode the given 16-bit word to the nearest codeword, correcting
/// up to 2 bit errors.
///
/// Returns `Some((data, error_count))` on success, where `data` is the 8
/// recovered data bits and `error_count` is 0, 1, or 2. Returns `None` if
/// the error pattern is uncorrectable.
pub fn decode(word: u16) -> Option<(u8, usize)> {
    // Expand to 17-bit base code word (MSB = 0 for shortened code).
    let base_word = word as u32;
    decode_base(base_word).and_then(|(data, errors)| {
        if data >> 8 == 0 {
            Some((data as u8, errors))
        } else {
            None
        }
    })
}

/// Compute the GF(2) dot product of a binary vector with each row of a
/// matrix, producing a result value.
fn gf2_vector_matrix(word: u32, matrix: &[u32]) -> u32 {
    matrix.iter().fold(0u32, |accumulator, &row| {
        let bit = (word & row).count_ones() & 1;
        (accumulator << 1) | bit
    })
}

/// Systematic encoding: returns `[data | data * G^T]` as a 17-bit word.
fn encode_base(data: u16) -> u32 {
    debug_assert!(data >> 9 == 0, "base data must be 9 bits, got {data:#X}");
    let parity = gf2_vector_matrix(data as u32, &GENERATOR);
    ((data as u32) << 8) | parity
}

/// Decode a 17-bit base code word using cyclic rotation with syndrome
/// lookup.
fn decode_base(word: u32) -> Option<(u16, usize)> {
    debug_assert!(word >> 17 == 0, "word must be 17 bits, got {word:#X}");

    let mut current = word;
    let mut error_count: Option<usize> = Some(0);

    // Rotate through all 17 positions. At each step, compute the syndrome
    // and check if the error pattern is correctable with the LSB set.
    for _ in 0..17 {
        let syndrome = gf2_vector_matrix(current, &PARITY_CHECK) as u8;

        if syndrome == 0 {
            current = rotate_17(current);
            continue;
        }

        let pattern = PATTERNS[syndrome as usize];
        if pattern == 0 {
            // Uncorrectable syndrome at this rotation
            error_count = None;
            current = rotate_17(current);
        } else {
            error_count = Some(pattern.count_ones() as usize);
            current = rotate_17(current ^ pattern);
        }
    }

    // After 17 rotations, data bits are back in their original position.
    error_count.map(|errors| ((current >> 8) as u16, errors))
}

/// Cyclically rotate a 17-bit word right by 1 position.
fn rotate_17(word: u32) -> u32 {
    let lsb = word & 1;
    (word >> 1) | (lsb << 16)
}

/// Transpose of the generator matrix (parity submatrix).
///
/// Each row computes one parity bit from the 9 data bits.
/// Generator polynomial: g(x) = x^8 + x^5 + x^4 + x^3 + 1.
const GENERATOR: [u32; 8] = [
    0b100111100,
    0b010011110,
    0b001001111,
    0b100011011,
    0b110110001,
    0b111100100,
    0b011110010,
    0b001111001,
];

/// Transpose of the parity-check matrix.
///
/// Used to compute the 8-bit syndrome: s = word * H^T.
const PARITY_CHECK: [u32; 8] = [
    0b10011110010000000,
    0b01001111001000000,
    0b00100111100100000,
    0b10001101100010000,
    0b11011000100001000,
    0b11110010000000100,
    0b01111001000000010,
    0b00111100100000001,
];

/// Maps each 8-bit syndrome to a 17-bit error pattern.
///
/// Zero entries indicate uncorrectable syndromes. Because the code is
/// cyclic, the rotation-based decoder only needs patterns where the errors
/// include the LSB position.
///
/// Indexed by syndrome value (0..256). Non-zero entries exist only at
/// syndromes corresponding to: single-bit error at bit 0, or two-bit
/// errors where one error is at bit 0.
#[rustfmt::skip]
const PATTERNS: [u32; 256] = {
    let mut table = [0u32; 256];
    table[0x01] = 0b00000000000000001; // bit 0
    table[0x03] = 0b00000000000000011; // bits 0,1
    table[0x05] = 0b00000000000000101; // bits 0,2
    table[0x09] = 0b00000000000001001; // bits 0,3
    table[0x11] = 0b00000000000010001; // bits 0,4
    table[0x21] = 0b00000000000100001; // bits 0,5
    table[0x26] = 0b00100000000000001; // bits 0,14
    table[0x38] = 0b00000000100000001; // bits 0,8
    table[0x41] = 0b00000000001000001; // bits 0,6
    table[0x4F] = 0b01000000000000001; // bits 0,15
    table[0x73] = 0b00000001000000001; // bits 0,9
    table[0x81] = 0b00000000010000001; // bits 0,7
    table[0x8E] = 0b00010000000000001; // bits 0,13
    table[0x9D] = 0b10000000000000001; // bits 0,16
    table[0xDA] = 0b00001000000000001; // bits 0,12
    table[0xE5] = 0b00000010000000001; // bits 0,10
    table[0xF0] = 0b00000100000000001; // bits 0,11
    table
};

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Base (17,9,5) code tests ----

    #[test]
    fn base_encode_known_vectors() {
        assert_eq!(encode_base(0b000000000), 0b000000000_00000000);
        assert_eq!(encode_base(0b111111111), 0b111111111_11111111);
        assert_eq!(encode_base(0b100000001), 0b100000001_10100101);
        assert_eq!(encode_base(0b000001001), 0b000001001_11001000);
        assert_eq!(encode_base(0b000001011), 0b000001011_10111010);
        assert_eq!(encode_base(0b000001010), 0b000001010_10000011);
        assert_eq!(encode_base(0b000001000), 0b000001000_11110001);
    }

    #[test]
    fn base_exhaustive_round_trip() {
        for data in 0..1u32 << 9 {
            let data = data as u16;
            assert_eq!(
                decode_base(encode_base(data)),
                Some((data, 0)),
                "round-trip failed for data {data:#011b}"
            );
        }
    }

    #[test]
    fn base_single_bit_correction_all_positions() {
        let data: u16 = 0b1010101;
        let encoded = encode_base(data);
        assert_eq!(encoded, 0b1010101_00100001);

        for bit in 0..17 {
            let corrupted = encoded ^ (1 << bit);
            assert_eq!(
                decode_base(corrupted),
                Some((data, 1)),
                "single-bit correction failed at position {bit}"
            );
        }
    }

    #[test]
    fn base_two_bit_correction_exhaustive() {
        let data: u16 = 0b1010101;
        let encoded = encode_base(data);

        for i in 0..17u32 {
            for j in (i + 1)..17 {
                let corrupted = encoded ^ (1 << i) ^ (1 << j);
                assert_eq!(
                    decode_base(corrupted),
                    Some((data, 2)),
                    "two-bit correction failed at positions {i},{j}"
                );
            }
        }
    }

    #[test]
    fn rotate_17_full_cycle() {
        let original: u32 = 0b11100011001010101;
        let mut word = original;
        for _ in 0..17 {
            word = rotate_17(word);
        }
        assert_eq!(word, original);
    }

    #[test]
    fn rotate_17_single_bit() {
        assert_eq!(rotate_17(0b00000000000000001), 0b10000000000000000);
        assert_eq!(rotate_17(0b10000000000000000), 0b01000000000000000);
        assert_eq!(rotate_17(0b00000000000000010), 0b00000000000000001);
    }

    // ---- Shortened (16,8,5) P25 code tests ----

    #[test]
    fn encode_known_vector() {
        let encoded = encode(0b10101011);
        assert_eq!(encoded, 0b10101011_01111011);
    }

    #[test]
    fn exhaustive_round_trip() {
        for data in 0..=255u16 {
            let data = data as u8;
            assert_eq!(
                decode(encode(data)),
                Some((data, 0)),
                "round-trip failed for data {data:#010b}"
            );
        }
    }

    #[test]
    fn single_bit_correction_all_positions() {
        let data: u8 = 0b10101011;
        let encoded = encode(data);

        for bit in 0..16 {
            let corrupted = encoded ^ (1 << bit);
            assert_eq!(
                decode(corrupted),
                Some((data, 1)),
                "single-bit correction failed at position {bit}"
            );
        }
    }

    #[test]
    fn two_bit_correction_from_reference() {
        // Test vectors from p25.rs reference implementation
        let data: u8 = 0b10101011;
        let encoded = encode(data);
        assert_eq!(encoded, 0b10101011_01111011);

        assert_eq!(decode(encoded ^ 0b1000000000000001), Some((data, 2)));
        assert_eq!(decode(encoded ^ 0b0011000000000000), Some((data, 2)));
        assert_eq!(decode(encoded ^ 0b0010000000000001), Some((data, 2)));
        assert_eq!(decode(encoded ^ 0b0001000000000010), Some((data, 2)));
        assert_eq!(decode(encoded ^ 0b0000100000000100), Some((data, 2)));
        assert_eq!(decode(encoded ^ 0b0000010000001000), Some((data, 2)));
        assert_eq!(decode(encoded ^ 0b0000001000010000), Some((data, 2)));
        assert_eq!(decode(encoded ^ 0b0000000100100000), Some((data, 2)));
        assert_eq!(decode(encoded ^ 0b0000000011000000), Some((data, 2)));
        assert_eq!(decode(encoded ^ 0b0000000001010000), Some((data, 2)));
        assert_eq!(decode(encoded ^ 0b0000000010001000), Some((data, 2)));
        assert_eq!(decode(encoded ^ 0b0000000100000100), Some((data, 2)));
        assert_eq!(decode(encoded ^ 0b0000001000000010), Some((data, 2)));
        assert_eq!(decode(encoded ^ 0b0000010000000001), Some((data, 2)));
        assert_eq!(decode(encoded ^ 0b0100000000000100), Some((data, 2)));
        assert_eq!(decode(encoded ^ 0b1000000000001000), Some((data, 2)));
    }

    #[test]
    fn two_bit_correction_exhaustive() {
        let data: u8 = 0b10101011;
        let encoded = encode(data);

        for i in 0..16u32 {
            for j in (i + 1)..16 {
                let corrupted = encoded ^ (1 << i) ^ (1 << j);
                assert_eq!(
                    decode(corrupted),
                    Some((data, 2)),
                    "two-bit correction failed at positions {i},{j}"
                );
            }
        }
    }

    #[test]
    fn encode_all_zeros() {
        assert_eq!(encode(0), 0);
    }

    #[test]
    fn encode_all_ones() {
        let encoded = encode(0xFF);
        assert_eq!(decode(encoded), Some((0xFF, 0)));
    }

    #[test]
    fn decode_all_zeros() {
        assert_eq!(decode(0), Some((0, 0)));
    }
}

//! Golay error-correcting codes for P25 voice and header decoding.
//!
//! Implements three variants:
//! - **Standard (23,12,7)**: Corrects up to 3 bit errors. Used for IMBE voice
//!   frame chunks u_0 through u_3.
//! - **Extended (24,12,8)**: Corrects up to 3 bit errors. Used for TDULC
//!   header words.
//! - **Shortened (18,6,8)**: Corrects up to 3 bit errors. Used for HDU header
//!   words and voice extra (LC/CC) hexbit words.
//!
//! The generator polynomial for these codes is:
//! g(x) = x^11 + x^10 + x^6 + x^5 + x^4 + x^2 + 1
//!
//! Decoding algorithms are derived from Hankerson et al (2000), Roman (1992),
//! and Lee et al (2013).

/// Compute the GF(2) dot product of a binary vector with each row of a
/// matrix, shifting each resulting bit into the LSB of an accumulator.
///
/// This performs the vector-matrix multiplication **v** * **M**^T over GF(2),
/// where `word` is the input vector and `matrix` contains the rows of **M**.
fn gf2_matrix_multiply(word: u32, matrix: &[u32]) -> u16 {
    matrix.iter().fold(0u16, |accumulator, &row| {
        let bit = ((word & row).count_ones() & 1) as u16;
        (accumulator << 1) | bit
    })
}

/// Encoding and decoding of the (23, 12, 7) standard Golay code.
///
/// Corrects up to 3 bit errors and detects 4 bit errors.
/// Used for IMBE voice frame chunks u_0 through u_3.
pub mod standard {
    use super::*;

    /// Encode the given 12 data bits into a 23-bit codeword.
    ///
    /// The input must have only the 12 least significant bits set.
    /// Returns the systematic codeword: `[data (12 bits) | parity (11 bits)]`.
    pub fn encode(data: u16) -> u32 {
        debug_assert!(data >> 12 == 0, "data must be 12 bits, got {data:#X}");
        let parity = gf2_matrix_multiply(data as u32, &PARITY_GENERATOR_32);
        ((data as u32) << 11) | parity as u32
    }

    /// Try to decode the given 23-bit word to the nearest codeword,
    /// correcting up to 3 bit errors.
    ///
    /// Returns `Some((data, error_count))` on success, where `data` is
    /// the 12 recovered data bits and `error_count` is the number of
    /// corrected bits. Returns `None` for unrecoverable errors (4+ bits).
    pub fn decode(word: u32) -> Option<(u16, usize)> {
        debug_assert!(word >> 23 == 0, "word must be 23 bits, got {word:#X}");

        let data = (word >> 11) as u16;

        // Compute syndrome: s = word * H^T
        let syndrome = gf2_matrix_multiply(word, &PARITY_CHECK);
        let weight = syndrome.count_ones() as usize;

        // Case 1: 0-3 errors in parity bits only
        if weight <= 3 {
            return Some((data, weight));
        }

        // Case 2: 1 error in data bits, 0-2 errors in parity bits
        for (i, &single_syndrome) in DATA_BIT_SYNDROMES.iter().enumerate() {
            let residual = syndrome ^ single_syndrome;
            let weight = residual.count_ones() as usize;
            if weight <= 2 {
                return Some((data ^ (1 << i), weight + 1));
            }
        }

        // Case 3: 2-3 errors in data bits (rotate and recheck)
        let rotated = rotate_23(word);
        let syndrome = gf2_matrix_multiply(rotated, &PARITY_CHECK);
        let weight = syndrome.count_ones() as usize;

        if weight <= 3 {
            return Some((data ^ syndrome, weight));
        }

        // Case 4: 2-3 errors in data bits with 0-1 in parity bits
        for (i, &single_syndrome) in DATA_BIT_SYNDROMES.iter().enumerate() {
            let residual = syndrome ^ single_syndrome;
            let weight = residual.count_ones() as usize;
            if weight <= 2 {
                // Syndrome index 0 = data MSB (now rotated into upper bits)
                return if i == 0 {
                    Some((data ^ residual ^ (1 << 11), weight + 1))
                } else {
                    Some((data ^ residual, 3))
                };
            }
        }

        None
    }

    /// Circularly shift a 23-bit word right by 11 positions.
    fn rotate_23(word: u32) -> u32 {
        let lower = word & 0x7FF; // bits 0..11
        (word >> 11) | (lower << 12)
    }

    /// Transpose of the generator parity submatrix **A**^T (as u32).
    const PARITY_GENERATOR_32: [u32; 11] = [
        0b101001001111,
        0b111101101000,
        0b011110110100,
        0b001111011010,
        0b000111101101,
        0b101010111001,
        0b111100010011,
        0b110111000110,
        0b011011100011,
        0b100100111110,
        0b010010011111,
    ];

    /// Parity-check matrix **H** = [**A**^T | **I**].
    ///
    /// Used to compute syndromes: s = word * **H**^T.
    const PARITY_CHECK: [u32; 11] = [
        0b10100100111110000000000,
        0b11110110100001000000000,
        0b01111011010000100000000,
        0b00111101101000010000000,
        0b00011110110100001000000,
        0b10101011100100000100000,
        0b11110001001100000010000,
        0b11011100011000000001000,
        0b01101110001100000000100,
        0b10010011111000000000010,
        0b01001001111100000000001,
    ];

    /// Syndromes for single-bit errors in the 12 data bit positions.
    ///
    /// Index 0 corresponds to bit 12 (data LSB in the codeword),
    /// index 11 corresponds to bit 23 (data MSB).
    const DATA_BIT_SYNDROMES: [u16; 12] = [
        0b10001110101,
        0b10010011111,
        0b10101001011,
        0b11011100011,
        0b00110110011,
        0b01101100110,
        0b11011001100,
        0b00111101101,
        0b01111011010,
        0b11110110100,
        0b01100011101,
        0b11000111010,
    ];
}

/// Encoding and decoding of the (24, 12, 8) extended Golay code.
///
/// Corrects up to 3 bit errors and detects 4 bit errors.
/// This is the standard (23,12) code with an additional overall parity bit.
/// Used for TDULC header words.
pub mod extended {
    use super::*;

    /// Encode the given 12 data bits into a 24-bit codeword.
    ///
    /// The input must have only the 12 least significant bits set.
    /// Returns `[data (12 bits) | parity (12 bits)]`.
    pub fn encode(data: u16) -> u32 {
        debug_assert!(data >> 12 == 0, "data must be 12 bits, got {data:#X}");
        let parity = gf2_matrix_multiply(data as u32, &PARITY_GENERATOR);
        let codeword = ((data as u32) << 12) | parity as u32;
        debug_assert!(codeword >> 24 == 0);
        codeword
    }

    /// Try to decode the given 24-bit word to the nearest codeword,
    /// correcting up to 3 bit errors.
    ///
    /// Returns `Some((data, error_count))` on success, `None` for
    /// unrecoverable errors (4+ bits).
    pub fn decode(word: u32) -> Option<(u16, usize)> {
        debug_assert!(word >> 24 == 0, "word must be 24 bits, got {word:#X}");

        let data = (word >> 12) as u16;

        // Compute syndrome using alternative parity-check (self-dual property):
        // s = word * G^T, checking errors in upper 12 bits
        let syndrome = gf2_matrix_multiply(word, &PARITY_CHECK_ALT);
        let weight = syndrome.count_ones() as usize;

        if weight <= 3 {
            return Some((data ^ syndrome, weight));
        }

        // Check single error in lower 12 bits + 0-2 errors in upper 12 bits
        for &row in &PARITY_GENERATOR {
            let residual = syndrome ^ row as u16;
            let weight = residual.count_ones() as usize;
            if weight <= 2 {
                return Some((data ^ residual, weight + 1));
            }
        }

        // Compute syndrome using standard parity-check: s = word * H^T
        // This checks errors in lower 12 bits
        let syndrome = gf2_matrix_multiply(word, &PARITY_CHECK);
        let weight = syndrome.count_ones() as usize;

        if weight <= 3 {
            return Some((data, weight));
        }

        // Check single error in upper 12 bits + 0-2 errors in lower 12 bits
        for (i, &row) in GENERATOR_CORE.iter().enumerate() {
            let residual = syndrome ^ row;
            if residual.count_ones() <= 2 {
                let error = 1u16 << (11 - i);
                return Some((data ^ error, 3));
            }
        }

        None
    }

    /// Generator parity submatrix **A**.
    const GENERATOR_CORE: [u16; 12] = [
        0b110001110101,
        0b011000111011,
        0b111101101000,
        0b011110110100,
        0b001111011010,
        0b110110011001,
        0b011011001101,
        0b001101100111,
        0b110111000110,
        0b101010010111,
        0b100100111110,
        0b100011101011,
    ];

    /// Transpose of generator parity submatrix **A**^T.
    const PARITY_GENERATOR: [u32; 12] = [
        0b101001001111,
        0b111101101000,
        0b011110110100,
        0b001111011010,
        0b000111101101,
        0b101010111001,
        0b111100010011,
        0b110111000110,
        0b011011100011,
        0b100100111110,
        0b010010011111,
        0b110001110101,
    ];

    /// Standard parity-check matrix **H** = [**A**^T | **I**].
    const PARITY_CHECK: [u32; 12] = [
        0b101001001111_100000000000,
        0b111101101000_010000000000,
        0b011110110100_001000000000,
        0b001111011010_000100000000,
        0b000111101101_000010000000,
        0b101010111001_000001000000,
        0b111100010011_000000100000,
        0b110111000110_000000010000,
        0b011011100011_000000001000,
        0b100100111110_000000000100,
        0b010010011111_000000000010,
        0b110001110101_000000000001,
    ];

    /// Alternative parity-check matrix **G** = [**I** | **A**].
    ///
    /// Since the extended Golay code is self-dual, this can also serve as
    /// a parity-check matrix.
    const PARITY_CHECK_ALT: [u32; 12] = [
        0b100000000000_110001110101,
        0b010000000000_011000111011,
        0b001000000000_111101101000,
        0b000100000000_011110110100,
        0b000010000000_001111011010,
        0b000001000000_110110011001,
        0b000000100000_011011001101,
        0b000000010000_001101100111,
        0b000000001000_110111000110,
        0b000000000100_101010010111,
        0b000000000010_100100111110,
        0b000000000001_100011101011,
    ];
}

/// Encoding and decoding of the (18, 6, 8) shortened Golay code.
///
/// Corrects up to 3 bit errors. This is the extended (24,12) code
/// shortened by fixing the upper 6 data bits to zero.
/// Used for HDU header words and voice extra (LC/CC) hexbit words.
pub mod shortened {
    /// Encode the given 6 data bits into an 18-bit codeword.
    ///
    /// The input must have only the 6 least significant bits set.
    pub fn encode(data: u8) -> u32 {
        debug_assert!(data >> 6 == 0, "data must be 6 bits, got {data:#X}");
        super::extended::encode(data as u16)
    }

    /// Try to decode the given 18-bit word to the nearest codeword,
    /// correcting up to 3 bit errors.
    ///
    /// Returns `Some((data, error_count))` on success, where `data` is
    /// the 6 recovered data bits. Returns `None` if the upper 6 data bits
    /// are nonzero after correction (indicating an unrecoverable error).
    pub fn decode(word: u32) -> Option<(u8, usize)> {
        debug_assert!(word >> 18 == 0, "word must be 18 bits, got {word:#X}");
        super::extended::decode(word).and_then(|(data, errors)| {
            if data >> 6 == 0 {
                Some((data as u8, errors))
            } else {
                None
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Standard (23,12,7) tests ----

    #[test]
    fn standard_encode_known_vectors() {
        assert_eq!(standard::encode(0), 0);
        assert_eq!(
            standard::encode(0b111111111111),
            0b111111111111_11111111111
        );
        assert_eq!(
            standard::encode(0b111111000000),
            0b111111000000_11001101000
        );
        assert_eq!(
            standard::encode(0b000000111111),
            0b000000111111_00110010111
        );
        assert_eq!(
            standard::encode(0b100000000001),
            0b100000000001_01001001111
        );
    }

    #[test]
    fn standard_encode_specific_qa_vector() {
        assert_eq!(
            standard::encode(0b101010101010),
            0b1010101010_1000101111001
        );
    }

    #[test]
    fn standard_decode_error_free() {
        let data = 0b101010101010u16;
        let encoded = standard::encode(data);
        assert_eq!(standard::decode(encoded), Some((data, 0)));
    }

    #[test]
    fn standard_exhaustive_round_trip() {
        for data in 0..1u32 << 12 {
            let data = data as u16;
            assert_eq!(
                standard::decode(standard::encode(data)),
                Some((data, 0)),
                "round-trip failed for data {data:#06b}"
            );
        }
    }

    #[test]
    fn standard_single_bit_correction() {
        let data = 0b101010101010u16;
        let encoded = standard::encode(data);
        for bit in 0..23 {
            let corrupted = encoded ^ (1 << bit);
            assert_eq!(
                standard::decode(corrupted),
                Some((data, 1)),
                "single-bit correction failed at position {bit}"
            );
        }
    }

    #[test]
    fn standard_two_bit_correction() {
        let data = 0b101010101010u16;
        let encoded = standard::encode(data);
        // Test specific two-bit error patterns from cai_golay tests
        let patterns: &[u32] = &[
            0b01000000000000000000010,
            0b00100000000000000000100,
            0b00010000000000000001000,
            0b00001000000000000010000,
            0b00000100000000000100000,
            0b00000010000000001000000,
            0b00000001000000010000000,
            0b00000000100000100000000,
            0b00000000010001000000000,
            0b00000000001010000000000,
        ];
        for &pattern in patterns {
            let corrupted = encoded ^ pattern;
            let result = standard::decode(corrupted);
            assert_eq!(
                result,
                Some((data, 2)),
                "two-bit correction failed for pattern {pattern:#025b}"
            );
        }
    }

    #[test]
    fn standard_three_bit_contiguous_correction() {
        let data = 0b101010101010u16;
        let encoded = standard::encode(data);
        for start in 0..21 {
            let pattern = 0b111u32 << start;
            let corrupted = encoded ^ pattern;
            assert_eq!(
                standard::decode(corrupted),
                Some((data, 3)),
                "3-bit contiguous correction failed at start position {start}"
            );
        }
    }

    #[test]
    fn standard_all_0_to_3_bit_errors_same_half() {
        // The syndrome-table decoder corrects all 0-to-3-bit errors where
        // errors are confined to one half (data or parity) of the codeword,
        // plus all cases with 1 error in data and up to 2 in parity.
        // Some 3-bit patterns spanning both halves may return None.
        // This matches the behavior of cai_golay.
        let data = 0b110111101110u16;
        let encoded = standard::encode(data);

        // All 0-bit and 1-bit errors: always correctable
        assert_eq!(standard::decode(encoded), Some((data, 0)));
        for i in 0..23u32 {
            assert_eq!(
                standard::decode(encoded ^ (1 << i)),
                Some((data, 1)),
                "single-bit correction failed at position {i}"
            );
        }

        // All 2-bit errors with one bit in each half: correctable
        for i in 0..11u32 {
            for j in 11..23 {
                let error = (1 << i) | (1 << j);
                let result = standard::decode(encoded ^ error);
                assert_eq!(
                    result,
                    Some((data, 2)),
                    "two-bit cross-half correction failed at bits {i},{j}"
                );
            }
        }

        // Contiguous 3-bit patterns: all correctable
        for start in 0..21u32 {
            let pattern = 0b111 << start;
            assert_eq!(
                standard::decode(encoded ^ pattern),
                Some((data, 3)),
                "3-bit contiguous at {start}"
            );
        }
    }

    // ---- Extended (24,12,8) tests ----

    #[test]
    fn extended_encode_known_vectors() {
        assert_eq!(extended::encode(0), 0);
        assert_eq!(
            extended::encode(0b111111111111),
            0b111111111111_111111111111
        );
        assert_eq!(
            extended::encode(0b111111000000),
            0b111111000000_110011010001
        );
        assert_eq!(
            extended::encode(0b000000111111),
            0b000000111111_001100101110
        );
        assert_eq!(
            extended::encode(0b100000000001),
            0b100000000001_010010011110
        );
    }

    #[test]
    fn extended_exhaustive_round_trip() {
        for data in 0..1u32 << 12 {
            let data = data as u16;
            assert_eq!(
                extended::decode(extended::encode(data)),
                Some((data, 0)),
                "round-trip failed for data {data:#06b}"
            );
        }
    }

    #[test]
    fn extended_single_bit_correction() {
        let data = 0b111111101010u16;
        let encoded = extended::encode(data);
        for bit in 0..24 {
            let corrupted = encoded ^ (1 << bit);
            assert_eq!(
                extended::decode(corrupted),
                Some((data, 1)),
                "single-bit correction failed at position {bit}"
            );
        }
    }

    #[test]
    fn extended_two_bit_correction() {
        let data = 0b111111101010u16;
        let encoded = extended::encode(data);
        let patterns: &[u32] = &[
            0b100000000000000000000010,
            0b010000000000000000000001,
            0b001000000000000000000010,
            0b000100000000000000000100,
            0b000010000000000000001000,
            0b000001000000000000010000,
            0b000000100000000000100000,
            0b000000010000000001000000,
            0b000000001000000010000000,
            0b000000000100000100000000,
            0b000000000010001000000000,
            0b000000000001010000000000,
        ];
        for &pattern in patterns {
            let corrupted = encoded ^ pattern;
            let result = extended::decode(corrupted);
            assert_eq!(
                result,
                Some((data, 2)),
                "two-bit correction failed for pattern {pattern:#026b}"
            );
        }
    }

    #[test]
    fn extended_three_bit_contiguous_correction() {
        let data = 0b111111101010u16;
        let encoded = extended::encode(data);
        for start in 0..22 {
            let pattern = 0b111u32 << start;
            let corrupted = encoded ^ pattern;
            assert_eq!(
                extended::decode(corrupted),
                Some((data, 3)),
                "3-bit contiguous correction failed at start position {start}"
            );
        }
    }

    #[test]
    fn extended_four_bit_errors_detected() {
        let data = 0b110110100110u16;
        let encoded = extended::encode(data);
        for h in 0..24u32 {
            for i in (h + 1)..24 {
                for j in (i + 1)..24 {
                    for k in (j + 1)..24 {
                        let error = (1 << h) | (1 << i) | (1 << j) | (1 << k);
                        assert_eq!(
                            extended::decode(encoded ^ error),
                            None,
                            "4-bit error not detected: bits {h},{i},{j},{k}"
                        );
                    }
                }
            }
        }
    }

    // ---- Shortened (18,6,8) tests ----

    #[test]
    fn shortened_encode_known_vectors() {
        assert_eq!(shortened::encode(0), 0);
        assert_eq!(shortened::encode(0b111111), 0b111111_001100101110);
        assert_eq!(shortened::encode(0b000111), 0b000111_101101000010);
        assert_eq!(shortened::encode(0b111000), 0b111000_100001101100);
        assert_eq!(shortened::encode(0b100001), 0b100001_111000100110);
    }

    #[test]
    fn shortened_exhaustive_round_trip() {
        for data in 0..1u32 << 6 {
            let data = data as u8;
            assert_eq!(
                shortened::decode(shortened::encode(data)),
                Some((data, 0)),
                "round-trip failed for data {data:#04b}"
            );
        }
    }

    #[test]
    fn shortened_two_bit_correction() {
        let data = 0b101010u8;
        let encoded = shortened::encode(data);
        assert_eq!(encoded, 0b101010_001000110101);

        let patterns: &[u32] = &[
            0b100000000000000001,
            0b010000000000000010,
            0b001000000000000100,
            0b000100000000001000,
            0b000010000000010000,
            0b000001000000100000,
            0b000000100001000000,
            0b000000010010000000,
            0b000000001100000000,
        ];
        for &pattern in patterns {
            let corrupted = encoded ^ pattern;
            assert_eq!(
                shortened::decode(corrupted),
                Some((data, 2)),
                "two-bit correction failed for pattern {pattern:#020b}"
            );
        }
    }

    #[test]
    fn shortened_three_bit_contiguous_correction() {
        let data = 0b101010u8;
        let encoded = shortened::encode(data);
        for start in 0..16 {
            let pattern = 0b111u32 << start;
            let corrupted = encoded ^ pattern;
            assert_eq!(
                shortened::decode(corrupted),
                Some((data, 3)),
                "3-bit contiguous correction failed at start {start}"
            );
        }
    }

    #[test]
    fn shortened_error_count_progression() {
        let data = 0b101010u8;
        let encoded = shortened::encode(data);
        assert_eq!(shortened::decode(encoded), Some((data, 0)));
        assert_eq!(
            shortened::decode(encoded ^ 0b000000000000000001),
            Some((data, 1))
        );
        assert_eq!(
            shortened::decode(encoded ^ 0b000000000000000011),
            Some((data, 2))
        );
        assert_eq!(
            shortened::decode(encoded ^ 0b000000000000000111),
            Some((data, 3))
        );
    }

    #[test]
    fn shortened_high_bit_error_count() {
        let data = 0b101010u8;
        let encoded = shortened::encode(data);
        assert_eq!(
            shortened::decode(encoded ^ 0b000100000000000000),
            Some((data, 1))
        );
        assert_eq!(
            shortened::decode(encoded ^ 0b001100000000000000),
            Some((data, 2))
        );
        assert_eq!(
            shortened::decode(encoded ^ 0b011100000000000000),
            Some((data, 3))
        );
    }

    #[test]
    fn shortened_mixed_error_patterns() {
        let data = 0b101010u8;
        let encoded = shortened::encode(data);
        assert_eq!(
            shortened::decode(encoded ^ 0b001100000000000010),
            Some((data, 3))
        );
        assert_eq!(
            shortened::decode(encoded ^ 0b001000000000000110),
            Some((data, 3))
        );
    }
}

//! Hamming error-correcting codes for P25 voice frame decoding.
//!
//! Implements two variants:
//! - **Standard (15, 11, 3)**: Corrects up to 1 bit error. Used for IMBE
//!   voice frame chunks u_4 through u_6.
//! - **Shortened (10, 6, 3)**: Corrects up to 1 bit error. Used for voice
//!   extra packet hexbit words before Reed-Solomon decoding.
//!
//! Both codes use parity-check matrix syndrome decoding: compute s = word *
//! H^T, then look up the error location from a syndrome table.
//!
//! These algorithms are sourced from *Coding Theory and Cryptography: The
//! Essentials*, Hankerson, Hoffman, et al, 2000.

/// Compute the GF(2) dot product of a binary vector with each row of a
/// parity-check matrix, producing a syndrome value.
fn syndrome<T: Into<u32> + Copy>(word: T, parity_check: &[u16]) -> usize {
    let word = word.into();
    parity_check.iter().fold(0usize, |accumulator, &row| {
        let bit = ((word & row as u32).count_ones() & 1) as usize;
        (accumulator << 1) | bit
    })
}

/// Encoding and decoding of the (15, 11, 3) standard Hamming code.
///
/// Corrects up to 1 bit error. Used for IMBE voice frame chunks u_4
/// through u_6 (lower-priority voice data).
pub mod standard {
    use super::*;

    /// Encode the given 11 data bits into a 15-bit codeword.
    ///
    /// The input must have only the 11 least significant bits set.
    /// Returns `[data (11 bits) | parity (4 bits)]`.
    pub fn encode(data: u16) -> u16 {
        debug_assert!(data >> 11 == 0, "data must be 11 bits, got {data:#X}");
        let mut parity = 0u16;
        for &row in &GENERATOR {
            let bit = ((data & row).count_ones() & 1) as u16;
            parity = (parity << 1) | bit;
        }
        (data << 4) | parity
    }

    /// Try to decode the given 15-bit word to the nearest codeword,
    /// correcting up to 1 bit error.
    ///
    /// Returns `Some((data, error_count))` on success, where `data` is
    /// the 11 recovered data bits and `error_count` is 0 or 1. Returns
    /// `None` if the syndrome maps to an uncorrectable error pattern.
    pub fn decode(word: u16) -> Option<(u16, usize)> {
        debug_assert!(word >> 15 == 0, "word must be 15 bits, got {word:#X}");

        let s = syndrome(word, &PARITY_CHECK);
        if s == 0 {
            return Some((word >> 4, 0));
        }

        LOCATIONS.get(s).and_then(|&location| {
            if location == 0 {
                None
            } else {
                Some(((word ^ location) >> 4, 1))
            }
        })
    }

    /// Generator matrix (parity submatrix), without the identity part.
    ///
    /// Each row computes one parity bit from the 11 data bits.
    const GENERATOR: [u16; 4] = [
        0b11111110000,
        0b11110001110,
        0b11001101101,
        0b10101011011,
    ];

    /// Parity-check matrix **H** = [**A**^T | **I**].
    ///
    /// Used to compute the 4-bit syndrome: s = word * **H**^T.
    const PARITY_CHECK: [u16; 4] = [
        0b111111100001000,
        0b111100011100100,
        0b110011011010010,
        0b101010110110001,
    ];

    /// Maps 4-bit syndrome values to single-bit error locations.
    ///
    /// A zero entry means the syndrome corresponds to a multi-bit error
    /// that cannot be corrected (returns `None`).
    const LOCATIONS: [u16; 16] = [
        0,                    // 0b0000: no error
        0b0000000000000001,   // 0b0001
        0b0000000000000010,   // 0b0010
        0b0000000000010000,   // 0b0011
        0b0000000000000100,   // 0b0100
        0b0000000000100000,   // 0b0101
        0b0000000001000000,   // 0b0110
        0b0000000010000000,   // 0b0111
        0b0000000000001000,   // 0b1000
        0b0000000100000000,   // 0b1001
        0b0000001000000000,   // 0b1010
        0b0000010000000000,   // 0b1011
        0b0000100000000000,   // 0b1100
        0b0001000000000000,   // 0b1101
        0b0010000000000000,   // 0b1110
        0b0100000000000000,   // 0b1111
    ];
}

/// Encoding and decoding of the (10, 6, 3) shortened Hamming code.
///
/// Corrects up to 1 bit error. Used for voice extra packet hexbit words
/// before Reed-Solomon decoding.
pub mod shortened {
    use super::*;

    /// Encode the given 6 data bits into a 10-bit codeword.
    ///
    /// The input must have only the 6 least significant bits set.
    /// Returns `[data (6 bits) | parity (4 bits)]`.
    pub fn encode(data: u8) -> u16 {
        debug_assert!(data >> 6 == 0, "data must be 6 bits, got {data:#X}");
        let mut parity = 0u16;
        for &row in &GENERATOR {
            let bit = (((data & row).count_ones()) & 1) as u16;
            parity = (parity << 1) | bit;
        }
        ((data as u16) << 4) | parity
    }

    /// Try to decode the given 10-bit word to the nearest codeword,
    /// correcting up to 1 bit error.
    ///
    /// Returns `Some((data, error_count))` on success, where `data` is
    /// the 6 recovered data bits and `error_count` is 0 or 1. Returns
    /// `None` if the syndrome maps to an uncorrectable error pattern.
    pub fn decode(word: u16) -> Option<(u8, usize)> {
        debug_assert!(word >> 10 == 0, "word must be 10 bits, got {word:#X}");

        let s = syndrome(word, &PARITY_CHECK);
        if s == 0 {
            return Some(((word >> 4) as u8, 0));
        }

        LOCATIONS.get(s).and_then(|&location| {
            if location == 0 {
                None
            } else {
                Some((((word ^ location) >> 4) as u8, 1))
            }
        })
    }

    /// Generator matrix (parity submatrix), without the identity part.
    const GENERATOR: [u8; 4] = [
        0b111001,
        0b110101,
        0b101110,
        0b011110,
    ];

    /// Parity-check matrix **H** = [**A**^T | **I**].
    const PARITY_CHECK: [u16; 4] = [
        0b1110011000,
        0b1101010100,
        0b1011100010,
        0b0111100001,
    ];

    /// Maps 4-bit syndrome values to single-bit error locations.
    ///
    /// Zero entries indicate uncorrectable syndromes (shortened code has
    /// 5 syndromes that don't correspond to valid single-bit errors).
    pub(super) const LOCATIONS: [u16; 16] = [
        0,                    // 0b0000: no error
        0b0000000000000001,   // 0b0001
        0b0000000000000010,   // 0b0010
        0b0000000000100000,   // 0b0011
        0b0000000000000100,   // 0b0100
        0,                    // 0b0101: uncorrectable
        0,                    // 0b0110: uncorrectable
        0b0000000001000000,   // 0b0111
        0b0000000000001000,   // 0b1000
        0,                    // 0b1001: uncorrectable
        0,                    // 0b1010: uncorrectable
        0b0000000010000000,   // 0b1011
        0b0000000000010000,   // 0b1100
        0b0000000100000000,   // 0b1101
        0b0000001000000000,   // 0b1110
        0,                    // 0b1111: uncorrectable
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Standard (15,11,3) tests ----

    #[test]
    fn standard_encode_known_vectors() {
        assert_eq!(standard::encode(0), 0);
        assert_eq!(
            standard::encode(0b11111111111),
            0b11111111111_1111
        );
        assert_eq!(
            standard::encode(0b10101010101),
            0b10101010101_0101
        );
    }

    #[test]
    fn standard_exhaustive_round_trip() {
        for data in 0..1u32 << 11 {
            let data = data as u16;
            assert_eq!(
                standard::decode(standard::encode(data)),
                Some((data, 0)),
                "round-trip failed for data {data:#013b}"
            );
        }
    }

    #[test]
    fn standard_single_bit_correction_all_positions() {
        let data = 0b10101010101u16;
        let encoded = standard::encode(data);
        assert_eq!(encoded, 0b10101010101_0101);

        for bit in 0..15 {
            let corrupted = encoded ^ (1 << bit);
            assert_eq!(
                standard::decode(corrupted),
                Some((data, 1)),
                "single-bit correction failed at position {bit}"
            );
        }
    }

    #[test]
    fn standard_two_bit_errors_detected_as_one() {
        // Two-bit errors are detected as single-bit errors with an
        // incorrect correction — this is expected Hamming behavior.
        let data = 0b10101010101u16;
        let encoded = standard::encode(data);

        for i in 0..15u32 {
            for j in (i + 1)..15 {
                let corrupted = encoded ^ (1 << i) ^ (1 << j);
                let result = standard::decode(corrupted);
                // Should decode (possibly to wrong data), not return None
                let (_, error_count) = result.unwrap();
                assert_eq!(error_count, 1, "two-bit error at {i},{j} not detected");
            }
        }
    }

    #[test]
    fn standard_decode_all_zeros() {
        assert_eq!(standard::decode(0), Some((0, 0)));
    }

    #[test]
    fn standard_decode_all_ones() {
        let encoded = standard::encode(0b11111111111);
        assert_eq!(encoded, 0b111111111111111);
        assert_eq!(standard::decode(encoded), Some((0b11111111111, 0)));
    }

    // ---- Shortened (10,6,3) tests ----

    #[test]
    fn shortened_encode_known_vectors() {
        assert_eq!(shortened::encode(0), 0);
        assert_eq!(shortened::encode(0b111111), 0b111111_0000);
        assert_eq!(shortened::encode(0b101010), 0b101010_0110);
    }

    #[test]
    fn shortened_exhaustive_round_trip() {
        for data in 0..1u32 << 6 {
            let data = data as u8;
            assert_eq!(
                shortened::decode(shortened::encode(data)),
                Some((data, 0)),
                "round-trip failed for data {data:#08b}"
            );
        }
    }

    #[test]
    fn shortened_single_bit_correction_all_positions() {
        let data = 0b101010u8;
        let encoded = shortened::encode(data);
        assert_eq!(encoded, 0b101010_0110);

        for bit in 0..10 {
            let corrupted = encoded ^ (1 << bit);
            assert_eq!(
                shortened::decode(corrupted),
                Some((data, 1)),
                "single-bit correction failed at position {bit}"
            );
        }
    }

    #[test]
    fn shortened_two_bit_errors_never_silently_correct() {
        // Two-bit errors in the shortened (10,6,3) code either:
        // - Hit an uncorrectable syndrome (return None), or
        // - Miscorrect to a different codeword (return Some with error_count=1).
        // They should never return the original data with error_count=0.
        let data = 0b101010u8;
        let encoded = shortened::encode(data);
        let mut none_count = 0;

        for i in 0..10u32 {
            for j in (i + 1)..10 {
                let corrupted = encoded ^ (1 << i) ^ (1 << j);
                match shortened::decode(corrupted) {
                    Some((decoded, error_count)) => {
                        // Miscorrection is expected Hamming behavior
                        assert_eq!(error_count, 1, "two-bit error at {i},{j}");
                        assert_ne!(
                            decoded, data,
                            "two-bit error at {i},{j} decoded to original data"
                        );
                    }
                    None => none_count += 1,
                }
            }
        }

        // The shortened code has 5 uncorrectable syndromes, so some
        // two-bit patterns should map to them.
        assert!(none_count > 0, "expected some uncorrectable two-bit patterns");
    }

    #[test]
    fn shortened_decode_all_zeros() {
        assert_eq!(shortened::decode(0), Some((0, 0)));
    }

    #[test]
    fn shortened_decode_all_ones_data() {
        let encoded = shortened::encode(0b111111);
        assert_eq!(encoded, 0b1111110000);
        assert_eq!(shortened::decode(encoded), Some((0b111111, 0)));
    }

    #[test]
    fn shortened_locations_table_has_correct_zero_entries() {
        // The shortened code has 5 syndromes that map to zero (uncorrectable).
        // These are syndromes 0b0101, 0b0110, 0b1001, 0b1010, 0b1111.
        let zero_syndromes: Vec<usize> = shortened::LOCATIONS
            .iter()
            .enumerate()
            .filter(|&(i, loc)| i > 0 && *loc == 0)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(zero_syndromes, vec![0b0101, 0b0110, 0b1001, 0b1010, 0b1111]);
    }
}

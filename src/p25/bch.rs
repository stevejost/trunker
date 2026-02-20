//! BCH(63,16,23) decoder for P25 NID error correction.
//!
//! Corrects up to 11 bit errors in the 63-bit BCH codeword portion
//! of the P25 Network Identifier. Uses Berlekamp-Massey decoding
//! with Chien search over GF(2^6).
//!
//! Reference: TIA-102.BAAA, Hankerson et al "Coding Theory and
//! Cryptography: The Essentials" (2000).

/// Number of syndromes to compute (2t where t = 11).
const NUM_SYNDROMES: usize = 22;

/// Maximum number of correctable bit errors.
const MAX_ERRORS: usize = 11;

/// Field order: 2^6 - 1 = 63.
const FIELD_ORDER: usize = 63;

// ---------------------------------------------------------------------------
// GF(2^6) arithmetic with primitive polynomial x^6 + x + 1.
// ---------------------------------------------------------------------------

/// Exponential table: `EXP[i] = α^i` for GF(2^6).
///
/// Doubled to 126 entries so multiplication can index `LOG[a] + LOG[b]`
/// without a modular reduction step.
const EXP: [u8; 126] = generate_exp_table();

/// Logarithm table: `LOG[v] = i` where `α^i = v`.
///
/// `LOG[0]` is undefined (unused); all other entries are in 0..62.
const LOG: [u8; 64] = generate_log_table();

const fn generate_exp_table() -> [u8; 126] {
    let mut table = [0u8; 126];
    let mut val: u8 = 1;
    let mut i = 0;
    while i < FIELD_ORDER {
        table[i] = val;
        table[i + FIELD_ORDER] = val;
        // Multiply by α: shift left, reduce by x^6 + x + 1 = 0x43.
        let shifted = (val as u16) << 1;
        val = if shifted >= 64 {
            ((shifted as u8) ^ 0x43) & 0x3F
        } else {
            shifted as u8
        };
        i += 1;
    }
    table
}

const fn generate_log_table() -> [u8; 64] {
    let exp = generate_exp_table();
    let mut table = [0u8; 64];
    let mut i = 0;
    while i < FIELD_ORDER {
        table[exp[i] as usize] = i as u8;
        i += 1;
    }
    table
}

/// Multiply two GF(2^6) elements.
fn gf_mul(a: u8, b: u8) -> u8 {
    if a == 0 || b == 0 {
        return 0;
    }
    EXP[LOG[a as usize] as usize + LOG[b as usize] as usize]
}

/// Divide `a / b` in GF(2^6). `b` must not be zero.
fn gf_div(a: u8, b: u8) -> u8 {
    debug_assert!(b != 0, "division by zero in GF(2^6)");
    if a == 0 {
        return 0;
    }
    let diff = LOG[a as usize] as usize + FIELD_ORDER - LOG[b as usize] as usize;
    EXP[diff % FIELD_ORDER]
}

// ---------------------------------------------------------------------------
// Syndrome computation.
// ---------------------------------------------------------------------------

/// Compute the 22 syndromes of a 63-bit received word.
///
/// `syndromes[j] = r(α^{j+1})` for j = 0..21, where r(x) is the
/// received polynomial with bit 0 as the constant term.
fn compute_syndromes(received: u64) -> [u8; NUM_SYNDROMES] {
    let mut syndromes = [0u8; NUM_SYNDROMES];
    for (j, syndrome) in syndromes.iter_mut().enumerate() {
        let mut sum = 0u8;
        for bit in 0..FIELD_ORDER {
            if received >> bit & 1 != 0 {
                let power = (bit * (j + 1)) % FIELD_ORDER;
                sum ^= EXP[power];
            }
        }
        *syndrome = sum;
    }
    syndromes
}

// ---------------------------------------------------------------------------
// Berlekamp-Massey algorithm.
// ---------------------------------------------------------------------------

/// Error locator polynomial coefficients (σ_0 through σ_{MAX_ERRORS}).
type LocatorPoly = [u8; MAX_ERRORS + 1];

/// Run the Berlekamp-Massey algorithm to find the error locator polynomial.
///
/// Returns `(degree, σ)` where `degree` is the number of errors detected
/// and `σ` are the polynomial coefficients, or `None` if more than
/// `MAX_ERRORS` errors are detected.
fn berlekamp_massey(syndromes: &[u8; NUM_SYNDROMES]) -> Option<(usize, LocatorPoly)> {
    let mut sigma: LocatorPoly = [0; MAX_ERRORS + 1];
    sigma[0] = 1;
    let mut prev: LocatorPoly = [0; MAX_ERRORS + 1];
    prev[0] = 1;

    let mut lfsr_len = 0usize;
    let mut prev_discrepancy: u8 = 1;
    let mut shift = 1usize;

    for n in 0..NUM_SYNDROMES {
        // Compute discrepancy: d = S_{n+1} + Σ σ_i * S_{n+1-i}.
        let mut discrepancy = syndromes[n];
        for i in 1..=lfsr_len {
            discrepancy ^= gf_mul(sigma[i], syndromes[n - i]);
        }

        if discrepancy == 0 {
            shift += 1;
            continue;
        }

        let scale = gf_div(discrepancy, prev_discrepancy);

        if 2 * lfsr_len <= n {
            // LFSR length must increase.
            let saved = sigma;
            for i in shift..=MAX_ERRORS {
                sigma[i] ^= gf_mul(scale, prev[i - shift]);
            }
            prev = saved;
            lfsr_len = n + 1 - lfsr_len;
            prev_discrepancy = discrepancy;
            shift = 1;
        } else {
            // Length stays the same.
            for i in shift..=MAX_ERRORS {
                sigma[i] ^= gf_mul(scale, prev[i - shift]);
            }
            shift += 1;
        }
    }

    if lfsr_len > MAX_ERRORS {
        return None;
    }

    Some((lfsr_len, sigma))
}

// ---------------------------------------------------------------------------
// Chien search — find roots of the error locator polynomial.
// ---------------------------------------------------------------------------

/// Evaluate the error locator polynomial at all field elements to find
/// error positions.
///
/// Returns the list of bit positions (0..62) where errors occurred.
fn chien_search(degree: usize, sigma: &LocatorPoly) -> Vec<usize> {
    let mut errors = Vec::with_capacity(degree);

    for i in 0..FIELD_ORDER {
        // Evaluate σ(α^i).
        let mut val = sigma[0]; // always 1
        for (j, &coeff) in sigma[1..=degree].iter().enumerate() {
            let power = (i * (j + 1)) % FIELD_ORDER;
            val ^= gf_mul(coeff, EXP[power]);
        }

        if val == 0 {
            // α^i is a root → error at position (63 - i) % 63.
            let error_pos = (FIELD_ORDER - i) % FIELD_ORDER;
            errors.push(error_pos);
        }
    }

    errors
}

// ---------------------------------------------------------------------------
// Public API.
// ---------------------------------------------------------------------------

/// Result of BCH decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BchResult {
    /// The corrected 63-bit BCH codeword.
    pub corrected: u64,
    /// Number of bit errors that were corrected (0..=11).
    pub errors_corrected: usize,
}

/// Decode a 63-bit BCH(63,16,23) codeword, correcting up to 11 bit errors.
///
/// Only the lower 63 bits of `received` are used. Returns the corrected
/// codeword and error count, or `None` if the error pattern exceeds the
/// correction capability.
pub fn decode(received: u64) -> Option<BchResult> {
    let received = received & ((1u64 << FIELD_ORDER) - 1);

    let syndromes = compute_syndromes(received);

    if syndromes.iter().all(|&s| s == 0) {
        return Some(BchResult {
            corrected: received,
            errors_corrected: 0,
        });
    }

    let (degree, sigma) = berlekamp_massey(&syndromes)?;
    let errors = chien_search(degree, &sigma);

    // If Chien search didn't find exactly `degree` roots, the error
    // pattern is uncorrectable.
    if errors.len() != degree {
        return None;
    }

    let mut corrected = received;
    for &pos in &errors {
        corrected ^= 1u64 << pos;
    }

    // Verify correction by recomputing syndromes.
    let check = compute_syndromes(corrected);
    if !check.iter().all(|&s| s == 0) {
        return None;
    }

    Some(BchResult {
        corrected,
        errors_corrected: degree,
    })
}

/// Extract the 16 data bits (NAC + DUID) from a 63-bit BCH codeword.
///
/// Data occupies the most significant 16 bits: positions 62..47.
pub fn extract_data(codeword: u64) -> u16 {
    ((codeword >> 47) & 0xFFFF) as u16
}

// ---------------------------------------------------------------------------
// Encoder (test support and diagnostics).
// ---------------------------------------------------------------------------

/// Generator polynomial roots: powers of α whose conjugacy classes
/// intersect {1..22}. These 47 roots define the BCH(63,16,23) code.
const GENERATOR_ROOTS: [usize; 47] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19,
    20, 21, 22, 24, 25, 26, 28, 30, 32, 33, 34, 35, 36, 37, 38, 39, 40,
    41, 42, 44, 48, 49, 50, 51, 52, 56, 57, 60,
];

/// Compute the generator polynomial g(x) as a u64 bitmask.
///
/// g(x) = Π(x + α^j) for each root power j, computed in GF(2^6)[x]
/// then verified to have binary (GF(2)) coefficients.
fn compute_generator_poly() -> u64 {
    // Polynomial coefficients in GF(2^6). Index = degree.
    let mut poly = [0u8; 48];
    poly[0] = 1;
    let mut deg = 0usize;

    for &rp in &GENERATOR_ROOTS {
        let root = EXP[rp];
        deg += 1;
        // Multiply by (x + root): new[i] = old[i-1] XOR root*old[i].
        for i in (1..=deg).rev() {
            poly[i] = poly[i - 1] ^ gf_mul(poly[i], root);
        }
        poly[0] = gf_mul(poly[0], root);
    }

    // Convert to u64 bitmask (all coefficients must be 0 or 1).
    let mut result = 0u64;
    for (i, &c) in poly.iter().enumerate().take(48) {
        debug_assert!(
            c == 0 || c == 1,
            "non-binary coefficient at x^{i}: {c}"
        );
        if c == 1 {
            result |= 1u64 << i;
        }
    }
    result
}

/// Encode 16 data bits into a 63-bit BCH(63,16,23) codeword.
///
/// Uses systematic encoding: data occupies bits 62-47 and the 47
/// parity bits occupy bits 46-0.
pub fn encode(data: u16) -> u64 {
    let genpoly = compute_generator_poly();

    // Shift data to bits 62-47 of the codeword.
    let mut dividend = (data as u64) << 47;

    // Polynomial division over GF(2): compute remainder of data*x^47 / g(x).
    for i in (47..63).rev() {
        if dividend >> i & 1 == 1 {
            dividend ^= genpoly << (i - 47);
        }
    }

    // Remainder is in bits 0-46; combine with original data.
    let remainder = dividend & ((1u64 << 47) - 1);
    ((data as u64) << 47) | remainder
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- GF(2^6) field tests --

    #[test]
    fn field_primitive_element_order() {
        // α^63 should equal α^0 = 1 (the field has order 63).
        assert_eq!(EXP[0], 1);
        assert_eq!(EXP[63], 1, "α^63 should wrap to 1");
    }

    #[test]
    fn field_all_elements_distinct() {
        let mut seen = [false; 64];
        for i in 0..FIELD_ORDER {
            let val = EXP[i] as usize;
            assert!(!seen[val], "duplicate element at power {i}: {val}");
            assert!(val > 0 && val < 64, "out of range at power {i}: {val}");
            seen[val] = true;
        }
    }

    #[test]
    fn field_log_exp_roundtrip() {
        for i in 0..FIELD_ORDER {
            let val = EXP[i];
            assert_eq!(LOG[val as usize], i as u8, "log/exp mismatch at power {i}");
        }
    }

    #[test]
    fn field_mul_identity() {
        for i in 1..64u8 {
            assert_eq!(gf_mul(i, 1), i, "x * 1 != x for {i}");
            assert_eq!(gf_mul(1, i), i, "1 * x != x for {i}");
        }
    }

    #[test]
    fn field_mul_zero() {
        for i in 0..64u8 {
            assert_eq!(gf_mul(i, 0), 0, "x * 0 != 0 for {i}");
            assert_eq!(gf_mul(0, i), 0, "0 * x != 0 for {i}");
        }
    }

    #[test]
    fn field_mul_inverse() {
        fn gf_inv(a: u8) -> u8 {
            EXP[FIELD_ORDER - LOG[a as usize] as usize]
        }
        for i in 1..64u8 {
            let inv = gf_inv(i);
            assert_eq!(gf_mul(i, inv), 1, "{i} * inv({i}) != 1");
        }
    }

    #[test]
    fn field_primitive_poly_check() {
        // α^6 = α + 1 = 3 (from primitive polynomial x^6 + x + 1).
        assert_eq!(EXP[6], 3);
    }

    // -- Syndrome tests --

    #[test]
    fn syndromes_zero_for_zero_codeword() {
        let s = compute_syndromes(0);
        assert!(s.iter().all(|&v| v == 0), "zero word should have zero syndromes");
    }

    #[test]
    fn syndromes_nonzero_for_single_bit_error() {
        let word = 1u64; // single bit set at position 0
        let s = compute_syndromes(word);
        assert!(!s.iter().all(|&v| v == 0), "single bit should produce nonzero syndromes");
    }

    // -- BCH decoder tests --

    #[test]
    fn decode_zero_codeword_no_errors() {
        let result = decode(0).expect("zero codeword should decode");
        assert_eq!(result.corrected, 0);
        assert_eq!(result.errors_corrected, 0);
    }

    #[test]
    fn decode_corrects_single_bit_error() {
        // The all-zeros word is a valid codeword. Flip each bit
        // individually and verify correction.
        for bit in 0..FIELD_ORDER {
            let corrupted = 1u64 << bit;
            let result = decode(corrupted).unwrap_or_else(|| {
                panic!("should correct single bit error at position {bit}");
            });
            assert_eq!(result.corrected, 0, "bit {bit} not corrected");
            assert_eq!(result.errors_corrected, 1);
        }
    }

    #[test]
    fn decode_corrects_two_bit_errors() {
        for a in 0..10 {
            for b in (a + 1)..15 {
                let corrupted = (1u64 << a) | (1u64 << b);
                let result = decode(corrupted).unwrap_or_else(|| {
                    panic!("should correct 2-bit error at positions {a}, {b}");
                });
                assert_eq!(result.corrected, 0);
                assert_eq!(result.errors_corrected, 2);
            }
        }
    }

    #[test]
    fn decode_corrects_eleven_bit_errors() {
        // Maximum correctable: 11 errors.
        let mut corrupted = 0u64;
        for bit in 0..MAX_ERRORS {
            corrupted |= 1u64 << bit;
        }
        let result = decode(corrupted).expect("should correct 11-bit error pattern");
        assert_eq!(result.corrected, 0);
        assert_eq!(result.errors_corrected, MAX_ERRORS);
    }

    #[test]
    fn decode_corrects_eleven_scattered_errors() {
        // 11 errors spread across the codeword.
        let positions = [0, 5, 11, 17, 23, 29, 35, 41, 47, 53, 59];
        assert_eq!(positions.len(), MAX_ERRORS);
        let mut corrupted = 0u64;
        for &pos in &positions {
            corrupted |= 1u64 << pos;
        }
        let result = decode(corrupted).expect("should correct 11 scattered errors");
        assert_eq!(result.corrected, 0);
        assert_eq!(result.errors_corrected, MAX_ERRORS);
    }

    #[test]
    fn decode_rejects_twelve_errors() {
        // 12 errors should exceed correction capability.
        let mut corrupted = 0u64;
        for bit in 0..12 {
            corrupted |= 1u64 << bit;
        }
        assert!(
            decode(corrupted).is_none(),
            "12 errors should be uncorrectable"
        );
    }

    #[test]
    fn decode_preserves_valid_codeword() {
        // A valid codeword should pass through unchanged.
        let result = decode(0).unwrap();
        assert_eq!(result.errors_corrected, 0);
    }

    #[test]
    fn extract_data_from_zero() {
        assert_eq!(extract_data(0), 0);
    }

    #[test]
    fn extract_data_top_bits() {
        // Place data 0xABCD at bits 62-47.
        let word = 0xABCDu64 << 47;
        assert_eq!(extract_data(word), 0xABCD);
    }

    #[test]
    fn decode_mask_strips_bit_63() {
        // Bit 63 is outside the BCH codeword; decode should ignore it.
        let with_extra = 1u64 << 63;
        let result = decode(with_extra).expect("bit 63 should be masked off");
        assert_eq!(result.corrected, 0);
        assert_eq!(result.errors_corrected, 0);
    }

    // -- Encoder tests --

    #[test]
    fn generator_poly_has_correct_degree() {
        let genpoly = compute_generator_poly();
        // Degree 47: bit 47 must be set, bits above 47 must be clear.
        assert!(genpoly >> 47 & 1 == 1, "leading coefficient missing");
        assert!(genpoly >> 48 == 0, "bits above degree 47 should be zero");
    }

    #[test]
    fn encode_zero_data() {
        // Encoding zero data should produce the zero codeword.
        assert_eq!(encode(0), 0);
    }

    #[test]
    fn encode_decode_roundtrip() {
        // Every encoded word should decode with zero errors.
        for data in [0u16, 1, 0x5FC7, 0x293A, 0xFFFF, 0x8000, 0x0001] {
            let codeword = encode(data);
            let result = decode(codeword).unwrap_or_else(|| {
                panic!("encoded data 0x{data:04X} should decode cleanly");
            });
            assert_eq!(result.errors_corrected, 0);
            assert_eq!(extract_data(result.corrected), data);
        }
    }

    #[test]
    fn encode_produces_valid_syndromes() {
        // Syndromes of a valid codeword must all be zero.
        let codeword = encode(0x5FC7);
        let s = compute_syndromes(codeword);
        assert!(s.iter().all(|&v| v == 0), "encoded word should have zero syndromes");
    }

    #[test]
    fn encode_then_corrupt_then_decode() {
        let data = 0x5FC7u16; // NAC=0x5FC, DUID=0x7
        let codeword = encode(data);

        // Corrupt 5 bits and verify correction.
        let corrupted = codeword ^ (1 << 0) ^ (1 << 15) ^ (1 << 30) ^ (1 << 45) ^ (1 << 60);
        let result = decode(corrupted).expect("5 errors should be correctable");
        assert_eq!(result.errors_corrected, 5);
        assert_eq!(extract_data(result.corrected), data);
    }

    #[test]
    fn encode_decode_exhaustive_single_error() {
        let data = 0x293Au16;
        let codeword = encode(data);

        for bit in 0..FIELD_ORDER {
            let corrupted = codeword ^ (1u64 << bit);
            let result = decode(corrupted).unwrap_or_else(|| {
                panic!("single bit error at {bit} should be correctable");
            });
            assert_eq!(extract_data(result.corrected), data, "bit {bit}");
            assert_eq!(result.errors_corrected, 1);
        }
    }
}

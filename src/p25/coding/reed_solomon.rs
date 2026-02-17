//! Reed-Solomon decoders for P25 voice link control and crypto control.
//!
//! P25 uses three Reed-Solomon codes over GF(2^6):
//! - **Short (24, 12, 13)**: protects LDU1 Link Control (corrects up to 6 hexbit errors)
//! - **Medium (24, 16, 9)**: protects LDU2 Crypto Control (corrects up to 4 hexbit errors)
//! - Long (36, 20, 17): protects voice header (deferred — not needed for Phase 0)
//!
//! Each operates on 6-bit symbols (hexbits) using the GF(2^6) Galois field.

use super::bmcf;
use super::galois::{impl_polynomial_coefs, Codeword, Polynomial, PolynomialCoefs};
use crate::p25::types::Hexbit;

// ---------------------------------------------------------------------------
// Coefficient storage types for each RS code variant
// ---------------------------------------------------------------------------

impl_polynomial_coefs!(ShortCoefs, 13, 24);
impl_polynomial_coefs!(MediumCoefs, 9, 24);

// ---------------------------------------------------------------------------
// Short (24, 12, 13) code
// ---------------------------------------------------------------------------

/// Encoding and decoding of the (24, 12, 13) short Reed-Solomon code.
///
/// Corrects up to 6 hexbit symbol errors. Used for LDU1 Link Control.
pub mod short {
    use super::*;

    /// Transpose of the generator matrix G_LC (12 parity rows x 12 data columns).
    const GENERATOR: [[u8; 12]; 12] = [
        [0o62, 0o11, 0o03, 0o21, 0o30, 0o01, 0o61, 0o24, 0o72, 0o72, 0o73, 0o71],
        [0o44, 0o12, 0o01, 0o70, 0o22, 0o41, 0o76, 0o22, 0o42, 0o14, 0o65, 0o05],
        [0o03, 0o11, 0o05, 0o27, 0o03, 0o27, 0o21, 0o71, 0o05, 0o65, 0o36, 0o55],
        [0o25, 0o11, 0o75, 0o45, 0o75, 0o56, 0o55, 0o56, 0o20, 0o54, 0o61, 0o03],
        [0o14, 0o16, 0o14, 0o16, 0o15, 0o76, 0o76, 0o21, 0o43, 0o35, 0o42, 0o71],
        [0o16, 0o64, 0o06, 0o67, 0o15, 0o64, 0o01, 0o35, 0o47, 0o25, 0o22, 0o34],
        [0o27, 0o67, 0o20, 0o23, 0o33, 0o21, 0o63, 0o73, 0o33, 0o41, 0o17, 0o60],
        [0o03, 0o55, 0o44, 0o64, 0o15, 0o53, 0o35, 0o42, 0o56, 0o16, 0o04, 0o11],
        [0o53, 0o01, 0o66, 0o73, 0o51, 0o04, 0o30, 0o57, 0o01, 0o15, 0o44, 0o74],
        [0o04, 0o76, 0o06, 0o33, 0o03, 0o25, 0o13, 0o74, 0o16, 0o40, 0o20, 0o02],
        [0o36, 0o26, 0o70, 0o44, 0o53, 0o01, 0o64, 0o43, 0o13, 0o71, 0o25, 0o41],
        [0o47, 0o73, 0o66, 0o21, 0o50, 0o12, 0o70, 0o76, 0o76, 0o26, 0o05, 0o50],
    ];

    /// Compute 12 parity hexbits from the first 12 data hexbits.
    pub fn encode(buf: &mut [Hexbit; 24]) {
        let (data, parity) = buf.split_at_mut(12);
        super::encode(data, parity, &GENERATOR);
    }

    /// Decode a 24-hexbit word, correcting up to 6 symbol errors.
    ///
    /// Returns `Some((data_slice, error_count))` on success, or `None` if
    /// the errors are unrecoverable.
    pub fn decode(buf: &mut [Hexbit; 24]) -> Option<(&[Hexbit], usize)> {
        super::decode::<ShortCoefs>(buf).map(move |(poly, err)| {
            (super::extract_data(poly, &mut buf[..12]), err)
        })
    }
}

// ---------------------------------------------------------------------------
// Medium (24, 16, 9) code
// ---------------------------------------------------------------------------

/// Encoding and decoding of the (24, 16, 9) medium Reed-Solomon code.
///
/// Corrects up to 4 hexbit symbol errors. Used for LDU2 Crypto Control.
pub mod medium {
    use super::*;

    /// Transpose of the generator matrix G_ES (8 parity rows x 16 data columns).
    const GENERATOR: [[u8; 16]; 8] = [
        [0o51, 0o57, 0o05, 0o73, 0o75, 0o20, 0o02, 0o24, 0o42, 0o32, 0o65, 0o64, 0o62, 0o55, 0o24, 0o67],
        [0o45, 0o25, 0o01, 0o07, 0o15, 0o32, 0o75, 0o74, 0o64, 0o32, 0o36, 0o06, 0o63, 0o43, 0o23, 0o75],
        [0o67, 0o63, 0o31, 0o47, 0o51, 0o14, 0o43, 0o15, 0o07, 0o55, 0o25, 0o54, 0o74, 0o34, 0o23, 0o45],
        [0o15, 0o73, 0o04, 0o14, 0o51, 0o42, 0o05, 0o72, 0o22, 0o41, 0o07, 0o32, 0o70, 0o71, 0o05, 0o60],
        [0o64, 0o71, 0o16, 0o41, 0o17, 0o75, 0o01, 0o24, 0o61, 0o57, 0o50, 0o76, 0o05, 0o57, 0o50, 0o57],
        [0o67, 0o22, 0o54, 0o77, 0o67, 0o42, 0o40, 0o26, 0o20, 0o66, 0o16, 0o46, 0o27, 0o76, 0o70, 0o24],
        [0o52, 0o40, 0o25, 0o47, 0o17, 0o70, 0o12, 0o74, 0o40, 0o21, 0o40, 0o14, 0o37, 0o50, 0o42, 0o06],
        [0o12, 0o15, 0o76, 0o11, 0o57, 0o54, 0o64, 0o61, 0o65, 0o77, 0o51, 0o36, 0o46, 0o64, 0o23, 0o26],
    ];

    /// Compute 8 parity hexbits from the first 16 data hexbits.
    pub fn encode(buf: &mut [Hexbit; 24]) {
        let (data, parity) = buf.split_at_mut(16);
        super::encode(data, parity, &GENERATOR);
    }

    /// Decode a 24-hexbit word, correcting up to 4 symbol errors.
    ///
    /// Returns `Some((data_slice, error_count))` on success, or `None` if
    /// the errors are unrecoverable.
    pub fn decode(buf: &mut [Hexbit; 24]) -> Option<(&[Hexbit], usize)> {
        super::decode::<MediumCoefs>(buf).map(move |(poly, err)| {
            (super::extract_data(poly, &mut buf[..16]), err)
        })
    }
}

// ---------------------------------------------------------------------------
// Shared encode/decode implementation
// ---------------------------------------------------------------------------

/// Encode data using the given generator matrix rows.
fn encode<const PARITY: usize, const DATA: usize>(
    data: &[Hexbit],
    parity: &mut [Hexbit],
    generator: &[[u8; DATA]; PARITY],
) {
    for (i, row) in generator.iter().enumerate() {
        let val = row
            .iter()
            .zip(data.iter())
            .fold(Codeword::default(), |sum, (&col, &d)| {
                sum + Codeword::new(d.bits()) * Codeword::new(col)
            });
        parity[i] = Hexbit::new(val.bits());
    }
}

/// Decode a hexbit word using Reed-Solomon error correction.
///
/// Returns the corrected polynomial and error count, or `None` if
/// unrecoverable.
fn decode<P: PolynomialCoefs>(word: &[Hexbit]) -> Option<(Polynomial<P>, usize)> {
    // Build polynomial: first received symbol = highest degree coefficient.
    let mut poly = Polynomial::new(word.iter().rev().map(|h| Codeword::new(h.bits())));

    // Compute syndromes and run BMCF pipeline.
    bmcf::Errors::new(syndromes(&poly)).and_then(|(nerr, errs)| {
        for (loc, pat) in errs {
            match poly.get_mut(loc) {
                Some(coef) => *coef = *coef + pat,
                // Error location outside the codeword = unrecoverable.
                None => return None,
            }
        }
        Some((poly, nerr))
    })
}

/// Compute the syndrome polynomial s(x) = s_1 + s_2*x + ... + s_{2t}*x^{2t-1}.
fn syndromes<P: PolynomialCoefs>(word: &Polynomial<P>) -> Polynomial<P> {
    Polynomial::new(
        (1..=P::syndromes()).map(|p| word.eval(Codeword::for_power(p))),
    )
}

/// Extract data hexbits from the corrected polynomial back into a buffer.
///
/// The polynomial stores coefficients in ascending degree order, but the
/// first received symbol corresponds to the highest degree. So we reverse.
fn extract_data<P: PolynomialCoefs>(poly: Polynomial<P>, data: &mut [Hexbit]) -> &[Hexbit] {
    for (dst, coef) in data.iter_mut().zip(poly.iter().rev()) {
        *dst = Hexbit::new(coef.bits());
    }
    data
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_coefs() {
        ShortCoefs::default().validate();
        MediumCoefs::default().validate();
    }

    #[test]
    fn verify_short_generator_polynomial() {
        // g(x) = (x + alpha^1)(x + alpha^2)...(x + alpha^8) for minimum distance 13
        // but the first 8 roots (d-1=12 roots, but polynomial is degree 8 for the parity)
        // Actually for short (24,12,13), d=13, so g(x) has roots alpha^1..alpha^12
        // but the polynomial coefficients match the reference.
        let p = Polynomial::<ShortCoefs>::new(
            [Codeword::for_power(1), Codeword::for_power(0)].into_iter(),
        ) * Polynomial::new(
            [Codeword::for_power(2), Codeword::for_power(0)].into_iter(),
        ) * Polynomial::new(
            [Codeword::for_power(3), Codeword::for_power(0)].into_iter(),
        ) * Polynomial::new(
            [Codeword::for_power(4), Codeword::for_power(0)].into_iter(),
        ) * Polynomial::new(
            [Codeword::for_power(5), Codeword::for_power(0)].into_iter(),
        ) * Polynomial::new(
            [Codeword::for_power(6), Codeword::for_power(0)].into_iter(),
        ) * Polynomial::new(
            [Codeword::for_power(7), Codeword::for_power(0)].into_iter(),
        ) * Polynomial::new(
            [Codeword::for_power(8), Codeword::for_power(0)].into_iter(),
        );

        assert_eq!(p.degree().unwrap(), 8);
        assert_eq!(p.coef(0).bits(), 0o26);
        assert_eq!(p.coef(1).bits(), 0o06);
        assert_eq!(p.coef(2).bits(), 0o24);
        assert_eq!(p.coef(3).bits(), 0o57);
        assert_eq!(p.coef(4).bits(), 0o60);
        assert_eq!(p.coef(5).bits(), 0o45);
        assert_eq!(p.coef(6).bits(), 0o75);
        assert_eq!(p.coef(7).bits(), 0o67);
        assert_eq!(p.coef(8).bits(), 0o01);
    }

    #[test]
    fn verify_medium_generator_polynomial() {
        let p = Polynomial::<MediumCoefs>::new(
            [Codeword::for_power(1), Codeword::for_power(0)].into_iter(),
        ) * Polynomial::new(
            [Codeword::for_power(2), Codeword::for_power(0)].into_iter(),
        ) * Polynomial::new(
            [Codeword::for_power(3), Codeword::for_power(0)].into_iter(),
        ) * Polynomial::new(
            [Codeword::for_power(4), Codeword::for_power(0)].into_iter(),
        ) * Polynomial::new(
            [Codeword::for_power(5), Codeword::for_power(0)].into_iter(),
        ) * Polynomial::new(
            [Codeword::for_power(6), Codeword::for_power(0)].into_iter(),
        ) * Polynomial::new(
            [Codeword::for_power(7), Codeword::for_power(0)].into_iter(),
        ) * Polynomial::new(
            [Codeword::for_power(8), Codeword::for_power(0)].into_iter(),
        ) * Polynomial::new(
            [Codeword::for_power(9), Codeword::for_power(0)].into_iter(),
        ) * Polynomial::new(
            [Codeword::for_power(10), Codeword::for_power(0)].into_iter(),
        ) * Polynomial::new(
            [Codeword::for_power(11), Codeword::for_power(0)].into_iter(),
        ) * Polynomial::new(
            [Codeword::for_power(12), Codeword::for_power(0)].into_iter(),
        );

        assert_eq!(p.degree().unwrap(), 12);
        assert_eq!(p.coef(0).bits(), 0o50);
        assert_eq!(p.coef(1).bits(), 0o41);
        assert_eq!(p.coef(2).bits(), 0o02);
        assert_eq!(p.coef(3).bits(), 0o74);
        assert_eq!(p.coef(4).bits(), 0o11);
        assert_eq!(p.coef(5).bits(), 0o60);
        assert_eq!(p.coef(6).bits(), 0o34);
        assert_eq!(p.coef(7).bits(), 0o71);
        assert_eq!(p.coef(8).bits(), 0o03);
        assert_eq!(p.coef(9).bits(), 0o55);
        assert_eq!(p.coef(10).bits(), 0o05);
        assert_eq!(p.coef(11).bits(), 0o71);
        assert_eq!(p.coef(12).bits(), 0o01);
    }

    #[test]
    fn short_encode_decode_clean() {
        let mut buf = [Hexbit::default(); 24];
        buf[0] = Hexbit::new(1);

        short::encode(&mut buf);
        let (data, err) = short::decode(&mut buf).unwrap();
        assert_eq!(err, 0);
        assert_eq!(data[0].bits(), 1);
        for h in &data[1..] {
            assert_eq!(h.bits(), 0);
        }
    }

    #[test]
    fn short_decode_corrects_6_errors() {
        let mut buf = [Hexbit::default(); 24];
        buf[0] = Hexbit::new(1);
        short::encode(&mut buf);

        // Introduce 6 errors at various positions.
        buf[0] = Hexbit::new(0o00);
        buf[2] = Hexbit::new(0o60);
        buf[7] = Hexbit::new(0o42);
        buf[13] = Hexbit::new(0o14);
        buf[18] = Hexbit::new(0o56);
        buf[23] = Hexbit::new(0o72);

        let (data, err) = short::decode(&mut buf).unwrap();
        assert_eq!(err, 6);
        assert_eq!(data.len(), 12);
        assert_eq!(data[0].bits(), 1);
        for h in &data[1..] {
            assert_eq!(h.bits(), 0);
        }
    }

    #[test]
    fn medium_encode_decode_clean() {
        let mut buf = [Hexbit::default(); 24];
        buf[0] = Hexbit::new(1);

        medium::encode(&mut buf);
        let (data, err) = medium::decode(&mut buf).unwrap();
        assert_eq!(err, 0);
        assert_eq!(data[0].bits(), 1);
        for h in &data[1..] {
            assert_eq!(h.bits(), 0);
        }
    }

    #[test]
    fn medium_decode_corrects_4_errors() {
        let mut buf = [Hexbit::default(); 24];
        buf[0] = Hexbit::new(1);
        medium::encode(&mut buf);

        // Introduce 4 errors.
        buf[0] = Hexbit::new(0o00);
        buf[10] = Hexbit::new(0o60);
        buf[16] = Hexbit::new(0o42);
        buf[23] = Hexbit::new(0o14);

        let (data, err) = medium::decode(&mut buf).unwrap();
        assert_eq!(err, 4);
        assert_eq!(data.len(), 16);
        assert_eq!(data[0].bits(), 1);
        for h in &data[1..] {
            assert_eq!(h.bits(), 0);
        }
    }

    #[test]
    fn short_single_error_at_each_position() {
        let data_vals: [u8; 12] = [0o77; 12];
        let expected: Vec<Hexbit> = data_vals.iter().map(|&v| Hexbit::new(v)).collect();

        for i in 0..24 {
            let mut buf = [Hexbit::default(); 24];
            for (j, &v) in data_vals.iter().enumerate() {
                buf[j] = Hexbit::new(v);
            }
            short::encode(&mut buf);
            buf[i] = Hexbit::new(0);

            let (data, err) = short::decode(&mut buf).unwrap();
            assert_eq!(err, 1, "position {i}");
            assert_eq!(data, &expected[..], "data mismatch at position {i}");
        }
    }

    #[test]
    fn medium_single_error_at_each_position() {
        let data_vals: [u8; 16] = [0o77; 16];
        let expected: Vec<Hexbit> = data_vals.iter().map(|&v| Hexbit::new(v)).collect();

        for i in 0..24 {
            let mut buf = [Hexbit::default(); 24];
            for (j, &v) in data_vals.iter().enumerate() {
                buf[j] = Hexbit::new(v);
            }
            medium::encode(&mut buf);
            buf[i] = Hexbit::new(0);

            let (data, err) = medium::decode(&mut buf).unwrap();
            assert_eq!(err, 1, "position {i}");
            assert_eq!(data, &expected[..], "data mismatch at position {i}");
        }
    }

    #[test]
    fn short_out_of_bounds_correction_returns_none() {
        // This word caused attempted access at location 61 in p25.rs.
        let mut w = [
            Hexbit::new(0), Hexbit::new(0), Hexbit::new(0), Hexbit::new(4),
            Hexbit::new(0), Hexbit::new(3), Hexbit::new(34), Hexbit::new(28),
            Hexbit::new(7), Hexbit::new(13), Hexbit::new(61), Hexbit::new(32),
            Hexbit::new(27), Hexbit::new(55), Hexbit::new(49), Hexbit::new(7),
            Hexbit::new(0), Hexbit::new(0), Hexbit::new(0), Hexbit::new(0),
            Hexbit::new(0), Hexbit::new(0), Hexbit::new(0), Hexbit::new(0),
        ];
        assert_eq!(short::decode(&mut w), None);
    }

    #[test]
    fn medium_out_of_bounds_correction_returns_none() {
        // This word caused attempted access at location 34 in p25.rs.
        let mut w = [
            Hexbit::new(0), Hexbit::new(0), Hexbit::new(0), Hexbit::new(0),
            Hexbit::new(51), Hexbit::new(19), Hexbit::new(8), Hexbit::new(35),
            Hexbit::new(48), Hexbit::new(61), Hexbit::new(0), Hexbit::new(1),
            Hexbit::new(11), Hexbit::new(44), Hexbit::new(10), Hexbit::new(0),
            Hexbit::new(11), Hexbit::new(0), Hexbit::new(15), Hexbit::new(56),
            Hexbit::new(50), Hexbit::new(0), Hexbit::new(0), Hexbit::new(0),
        ];
        assert_eq!(medium::decode(&mut w), None);
    }
}

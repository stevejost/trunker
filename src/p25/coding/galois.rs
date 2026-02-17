//! Galois field GF(2^6) arithmetic for P25 Reed-Solomon coding.
//!
//! P25 uses the Galois field GF(2^6) characterized by the primitive
//! polynomial x^6 + x + 1. The field has 63 non-zero elements
//! (codewords), each representable as a 6-bit value.
//!
//! This module provides:
//! - `Codeword` — a field element with arithmetic operations
//! - `Polynomial` — a polynomial over the field with evaluation,
//!   truncation, differentiation, and shift operations
//! - `PolynomialCoefs` — trait for bounding polynomial storage size

use std::fmt;
use std::ops;

/// Number of non-zero elements in GF(2^6).
const FIELD_SIZE: usize = 63;

// ---------------------------------------------------------------------------
// GF(2^6) field tables
// ---------------------------------------------------------------------------

/// Maps power i to the codeword alpha^i (bit representation of x^i mod h(x),
/// where h(x) = x^6 + x + 1).
const CODEWORDS: [u8; FIELD_SIZE] = [
    0b000001, 0b000010, 0b000100, 0b001000, 0b010000, 0b100000,
    0b000011, 0b000110, 0b001100, 0b011000, 0b110000, 0b100011,
    0b000101, 0b001010, 0b010100, 0b101000, 0b010011, 0b100110,
    0b001111, 0b011110, 0b111100, 0b111011, 0b110101, 0b101001,
    0b010001, 0b100010, 0b000111, 0b001110, 0b011100, 0b111000,
    0b110011, 0b100101, 0b001001, 0b010010, 0b100100, 0b001011,
    0b010110, 0b101100, 0b011011, 0b110110, 0b101111, 0b011101,
    0b111010, 0b110111, 0b101101, 0b011001, 0b110010, 0b100111,
    0b001101, 0b011010, 0b110100, 0b101011, 0b010101, 0b101010,
    0b010111, 0b101110, 0b011111, 0b111110, 0b111111, 0b111101,
    0b111001, 0b110001, 0b100001,
];

/// Maps codeword bit pattern (minus 1, zero-based) to its power.
/// That is, if codeword alpha^i has bit pattern `b`, then POWERS[b-1] = i.
const POWERS: [usize; FIELD_SIZE] = [
     0,  1,  6,  2, 12,  7, 26,  3, 32, 13, 35,  8, 48, 27, 18,
     4, 24, 33, 16, 14, 52, 36, 54,  9, 45, 49, 38, 28, 41, 19,
    56,  5, 62, 25, 11, 34, 31, 17, 47, 15, 23, 53, 51, 37, 44,
    55, 40, 10, 61, 46, 30, 50, 22, 39, 43, 29, 60, 42, 21, 20,
    59, 57, 58,
];

/// Look up the codeword for power `i` (modulo the field size).
fn codeword_for_power(power: usize) -> u8 {
    CODEWORDS[power % FIELD_SIZE]
}

/// Look up the power for a non-zero codeword bit pattern.
///
/// # Panics
///
/// Panics if `bits` is 0 (the zero element has no power).
fn power_of_codeword(bits: u8) -> usize {
    debug_assert!(bits != 0 && bits < 64);
    POWERS[bits as usize - 1]
}

// ---------------------------------------------------------------------------
// Codeword — a single field element
// ---------------------------------------------------------------------------

/// An element of GF(2^6), stored as a 6-bit value.
///
/// The zero element (bits = 0) is the additive identity.
/// Non-zero elements alpha^i are identified by their power `i`.
#[derive(Copy, Clone)]
pub struct Codeword {
    bits: u8,
}

impl Codeword {
    /// Construct a codeword from a 6-bit value.
    ///
    /// Bits beyond the low 6 are silently masked off.
    pub fn new(bits: u8) -> Self {
        Self { bits: bits & 0x3F }
    }

    /// Construct the codeword alpha^power (modulo the field size).
    pub fn for_power(power: usize) -> Self {
        Self { bits: codeword_for_power(power) }
    }

    /// The 6-bit representation of this codeword.
    pub fn bits(self) -> u8 {
        self.bits
    }

    /// Whether this is the zero element.
    pub fn zero(self) -> bool {
        self.bits == 0
    }

    /// The discrete logarithm: if this codeword is alpha^i, returns `Some(i)`.
    /// Returns `None` for the zero element.
    pub fn power(self) -> Option<usize> {
        if self.zero() {
            None
        } else {
            Some(power_of_codeword(self.bits))
        }
    }

    /// Multiplicative inverse: 1 / alpha^i = alpha^(63-i).
    ///
    /// # Panics
    ///
    /// Panics if the codeword is zero.
    pub fn invert(self) -> Codeword {
        match self.power() {
            Some(p) => Codeword::for_power(FIELD_SIZE - p),
            None => panic!("cannot invert zero codeword"),
        }
    }

    /// Raise to a power: (alpha^i)^p = alpha^(i*p).
    /// Returns zero if the base is zero.
    pub fn pow(self, exponent: usize) -> Codeword {
        match self.power() {
            Some(p) => Codeword::for_power(p * exponent),
            None => Codeword::default(),
        }
    }
}

impl Default for Codeword {
    /// The additive identity (zero element, bits = 0).
    fn default() -> Self {
        Self { bits: 0 }
    }
}

impl PartialEq for Codeword {
    fn eq(&self, other: &Self) -> bool {
        self.bits == other.bits
    }
}

impl Eq for Codeword {}

impl PartialEq<u8> for Codeword {
    fn eq(&self, other: &u8) -> bool {
        self.bits == *other
    }
}

/// Galois addition: XOR of bit patterns.
#[allow(clippy::suspicious_arithmetic_impl)]
impl ops::Add for Codeword {
    type Output = Codeword;

    fn add(self, rhs: Codeword) -> Codeword {
        Codeword::new(self.bits ^ rhs.bits)
    }
}

/// Galois subtraction: identical to addition in GF(2^r).
#[allow(clippy::suspicious_arithmetic_impl)]
impl ops::Sub for Codeword {
    type Output = Codeword;

    fn sub(self, rhs: Codeword) -> Codeword {
        self + rhs
    }
}

/// Galois multiplication: add powers modulo field size.
#[allow(clippy::suspicious_arithmetic_impl)]
impl ops::Mul for Codeword {
    type Output = Codeword;

    fn mul(self, rhs: Codeword) -> Codeword {
        match (self.power(), rhs.power()) {
            (Some(p), Some(q)) => Codeword::for_power(p + q),
            _ => Codeword::default(),
        }
    }
}

/// Galois division: subtract powers modulo field size.
///
/// # Panics
///
/// Panics if the divisor is zero.
impl ops::Div for Codeword {
    type Output = Codeword;

    fn div(self, rhs: Codeword) -> Codeword {
        match (self.power(), rhs.power()) {
            (Some(p), Some(q)) => Codeword::for_power(FIELD_SIZE + p - q),
            (None, Some(_)) => Codeword::default(),
            (_, None) => panic!("division by zero codeword"),
        }
    }
}

impl fmt::Debug for Codeword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.power() {
            Some(p) => write!(f, "Codeword::for_power({p})"),
            None => write!(f, "Codeword::default()"),
        }
    }
}

// ---------------------------------------------------------------------------
// PolynomialCoefs — trait for coefficient storage
// ---------------------------------------------------------------------------

/// Coefficient storage for a bounded-degree Galois polynomial.
///
/// Each RS code variant provides its own storage type with the appropriate
/// minimum Hamming distance, which determines the number of correctable
/// errors and the required buffer size.
pub trait PolynomialCoefs:
    Default + Copy + Clone + ops::Deref<Target = [Codeword]> + ops::DerefMut
{
    /// Minimum Hamming distance `d` for this code.
    fn distance() -> usize;

    /// Maximum correctable errors: t = floor(d / 2).
    fn errors() -> usize {
        Self::distance() / 2
    }

    /// Number of syndromes: 2t.
    fn syndromes() -> usize {
        2 * Self::errors()
    }

    /// Verify the storage is well-formed.
    fn validate(&self) {
        assert!(Self::distance() % 2 == 1, "distance must be odd");
        assert!(
            self.len() > Self::syndromes(),
            "storage too small for syndrome polynomial"
        );
    }
}

/// Create a coefficient storage type for a given code.
///
/// In the first form, the polynomial is sized for Berlekamp-Massey decoding.
/// In the second form, the polynomial has a specific size.
#[allow(unused_macros)]
macro_rules! impl_polynomial_coefs {
    ($name:ident, $dist:expr) => {
        impl_polynomial_coefs!($name, $dist, $dist + 1);
    };
    ($name:ident, $dist:expr, $len:expr) => {
        #[derive(Copy)]
        pub struct $name([Codeword; $len]);

        impl PolynomialCoefs for $name {
            fn distance() -> usize { $dist }
        }

        impl Default for $name {
            fn default() -> Self {
                $name([Codeword::default(); $len])
            }
        }

        impl Clone for $name {
            fn clone(&self) -> Self { *self }
        }

        impl std::ops::Deref for $name {
            type Target = [Codeword];
            fn deref(&self) -> &Self::Target { &self.0[..] }
        }

        impl std::ops::DerefMut for $name {
            fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0[..] }
        }
    };
}

#[allow(unused_imports)]
pub(crate) use impl_polynomial_coefs;

// ---------------------------------------------------------------------------
// Polynomial — polynomial over GF(2^6)
// ---------------------------------------------------------------------------

/// A polynomial with `Codeword` coefficients: p(x) = c_0 + c_1*x + ... + c_k*x^k.
///
/// Coefficients are stored starting from degree 0. The `start` field allows
/// O(1) division by x (shift) without moving data.
#[derive(Copy, Clone)]
pub struct Polynomial<P: PolynomialCoefs> {
    coefs: P,
    /// Index of the degree-0 coefficient in `coefs`.
    start: usize,
}

impl<P: PolynomialCoefs> Polynomial<P> {
    /// Construct from an iterator of coefficients c_0, c_1, ..., c_k.
    ///
    /// Extra coefficients beyond the storage capacity are silently dropped.
    pub fn new(init: impl Iterator<Item = Codeword>) -> Self {
        let mut coefs = P::default();
        for (dst, src) in coefs.iter_mut().zip(init) {
            *dst = src;
        }
        Self { coefs, start: 0 }
    }

    /// Construct the monomial x^n.
    pub fn unit_power(n: usize) -> Self {
        let mut coefs = P::default();
        coefs[n] = Codeword::for_power(0);
        Self { coefs, start: 0 }
    }

    /// The degree-0 coefficient c_0.
    pub fn constant(&self) -> Codeword {
        self.coefs[self.start]
    }

    /// Degree of the polynomial, or `None` if p(x) = 0.
    pub fn degree(&self) -> Option<usize> {
        for (deg, coef) in self.coefs.iter().enumerate().rev() {
            if !coef.zero() {
                return Some(deg - self.start);
            }
        }
        None
    }

    /// Divide by x: shift all coefficients to a lower degree.
    ///
    /// # Panics
    ///
    /// Panics if c_0 is not zero.
    pub fn shift(mut self) -> Self {
        assert!(self.constant().zero(), "shift requires c_0 == 0");
        self.coefs[self.start] = Codeword::default();
        self.start += 1;
        self
    }

    /// Retrieve coefficient c_i (the x^i term). Returns zero if out of bounds.
    pub fn coef(&self, i: usize) -> Codeword {
        self.get(self.start + i)
    }

    /// Evaluate p(x) using Horner's method.
    pub fn eval(&self, x: Codeword) -> Codeword {
        self.iter().rev().fold(Codeword::default(), |acc, &c| acc * x + c)
    }

    /// Truncate so that deg(p(x)) <= max_degree.
    pub fn truncate(mut self, max_degree: usize) -> Self {
        for i in (self.start + max_degree + 1)..self.coefs.len() {
            self.coefs[i] = Codeword::default();
        }
        self
    }

    /// Formal derivative p'(x).
    ///
    /// In GF(2), the derivative zeroes out even-power terms and shifts
    /// odd-power terms down by one degree.
    pub fn deriv(mut self) -> Self {
        for i in self.start..self.coefs.len() {
            self.coefs[i] = if (i - self.start).is_multiple_of(2) {
                self.get(i + 1)
            } else {
                Codeword::default()
            };
        }
        self
    }

    /// Get the mutable coefficient at absolute index, or `None` if out of bounds.
    pub fn get_mut(&mut self, idx: usize) -> Option<&mut Codeword> {
        self.coefs.get_mut(idx)
    }

    /// Get coefficient at absolute storage index, or zero if out of bounds.
    fn get(&self, idx: usize) -> Codeword {
        self.coefs.get(idx).copied().unwrap_or_default()
    }
}

impl<P: PolynomialCoefs> Default for Polynomial<P> {
    fn default() -> Self {
        Self::new(std::iter::empty())
    }
}

/// Provides a coefficient slice starting at degree 0: [c_0, c_1, ...].
impl<P: PolynomialCoefs> ops::Deref for Polynomial<P> {
    type Target = [Codeword];
    fn deref(&self) -> &[Codeword] {
        &self.coefs[self.start..]
    }
}

impl<P: PolynomialCoefs> ops::DerefMut for Polynomial<P> {
    fn deref_mut(&mut self) -> &mut [Codeword] {
        &mut self.coefs[self.start..]
    }
}

/// Add polynomials (Galois addition of corresponding coefficients).
impl<P: PolynomialCoefs> ops::Add for Polynomial<P> {
    type Output = Polynomial<P>;

    fn add(mut self, rhs: Polynomial<P>) -> Polynomial<P> {
        // Sum coefficients, resetting start to 0.
        // Since start >= 0, start+i >= i, so no overwriting occurs.
        for i in 0..self.coefs.len() {
            self.coefs[i] = self.coef(i) + rhs.coef(i);
        }
        self.start = 0;
        self
    }
}

/// Scale polynomial by a codeword.
impl<P: PolynomialCoefs> ops::Mul<Codeword> for Polynomial<P> {
    type Output = Polynomial<P>;

    fn mul(mut self, rhs: Codeword) -> Polynomial<P> {
        for coef in self.coefs.iter_mut() {
            *coef = *coef * rhs;
        }
        self
    }
}

/// Multiply two polynomials.
///
/// Terms beyond the storage capacity are silently discarded.
impl<P: PolynomialCoefs> ops::Mul<Polynomial<P>> for Polynomial<P> {
    type Output = Polynomial<P>;

    fn mul(self, rhs: Polynomial<P>) -> Polynomial<P> {
        let mut out = Polynomial::<P>::default();

        for (i, &coef) in self.iter().enumerate() {
            for (j, &mult) in rhs.iter().enumerate() {
                if let Some(c) = out.coefs.get_mut(i + j) {
                    *c = *c + coef * mult;
                }
            }
        }

        out
    }
}

impl<P: PolynomialCoefs> fmt::Debug for Polynomial<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Polynomial({:?})", &self.coefs[..])
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    impl_polynomial_coefs!(TestCoefs, 23);
    type TestPoly = Polynomial<TestCoefs>;

    impl_polynomial_coefs!(ShortCoefs, 5);
    type ShortPoly = Polynomial<ShortCoefs>;

    // -- Field table validation --

    #[test]
    fn all_codewords_fit_in_6_bits() {
        for (i, &cw) in CODEWORDS.iter().enumerate() {
            assert!(
                cw >> 6 == 0,
                "codeword at power {i} has bits outside 6-bit range: 0b{cw:06b}",
            );
        }
    }

    #[test]
    fn no_codeword_collisions() {
        for (i, &cw_i) in CODEWORDS.iter().enumerate() {
            for (j, &cw_j) in CODEWORDS.iter().enumerate().skip(i + 1) {
                assert_ne!(
                    cw_i, cw_j,
                    "collision: codeword({i}) == codeword({j}) == 0b{cw_i:06b}",
                );
            }
        }
    }

    #[test]
    fn power_codeword_roundtrip() {
        for i in 0..FIELD_SIZE {
            let bits = codeword_for_power(i);
            let recovered = power_of_codeword(bits);
            assert_eq!(
                recovered, i,
                "roundtrip failed: power {i} -> bits 0b{bits:06b} -> power {recovered}"
            );
        }
    }

    // -- Codeword basics --

    #[test]
    fn codeword_coefs_trait_validation() {
        TestCoefs::default().validate();
        ShortCoefs::default().validate();
    }

    #[test]
    fn for_power_known_values() {
        assert!(Codeword::for_power(0) == 0b000001);
        assert!(Codeword::for_power(62) == 0b100001);
        // Wraps around field size.
        assert!(Codeword::for_power(63) == 0b000001);
    }

    #[test]
    fn zero_codeword() {
        let z = Codeword::default();
        assert!(z.zero());
        assert_eq!(z.bits(), 0);
        assert!(z.power().is_none());
    }

    // -- Galois addition --

    #[test]
    fn add_is_xor() {
        assert!(Codeword::new(0b100000) + Codeword::new(0b010000) == 0b110000);
        assert!(Codeword::new(0b100001) + Codeword::new(0b110100) == 0b010101);
    }

    #[test]
    fn sub_equals_add() {
        assert!(Codeword::new(0b100000) - Codeword::new(0b010000) == 0b110000);
        assert!(Codeword::new(0b100001) - Codeword::new(0b100001) == 0b000000);
    }

    #[test]
    fn add_self_is_zero_exhaustive() {
        // Includes the zero element.
        for bits in 0..64u8 {
            let c = Codeword::new(bits);
            assert!((c + c).zero(), "a + a != 0 for bits 0b{bits:06b}");
        }
    }

    // -- Galois multiplication --

    #[test]
    fn mul_known_values() {
        assert!(Codeword::new(0b000110) * Codeword::new(0b000101) == 0b011110);
        assert!(Codeword::new(0b100001) * Codeword::new(0b000001) == 0b100001);
        assert!(Codeword::new(0b100001) * Codeword::new(0b000010) == 0b000001);
        assert!(Codeword::new(0b110011) * Codeword::new(0b110011) == 0b111001);
        assert!(Codeword::new(0b101111) * Codeword::new(0b101111) == 0b100110);
    }

    #[test]
    fn mul_by_zero() {
        assert!((Codeword::new(0b000000) * Codeword::new(0b000101)).zero());
        assert!((Codeword::new(0b000110) * Codeword::new(0b000000)).zero());
        assert!((Codeword::new(0b000000) * Codeword::new(0b000000)).zero());
    }

    // -- Galois division --

    #[test]
    fn div_known_values() {
        assert!(Codeword::new(0b001000) / Codeword::new(0b000101) == 0b010111);
        assert!((Codeword::new(0b000000) / Codeword::new(0b101000)).zero());
        assert!(Codeword::new(0b011110) / Codeword::new(0b000001) == 0b011110);
        assert!(Codeword::new(0b011110) / Codeword::new(0b011110) == 0b000001);
    }

    // -- Exhaustive field consistency --

    #[test]
    fn mul_div_roundtrip_exhaustive() {
        for i in 0..FIELD_SIZE {
            let a = Codeword::for_power(i);
            for j in 0..FIELD_SIZE {
                let b = Codeword::for_power(j);
                let product = a * b;
                let recovered = product / b;
                assert_eq!(
                    recovered.bits(),
                    a.bits(),
                    "a*b/b != a for powers ({i}, {j})"
                );
            }
        }
    }

    // -- Codeword power operations --

    #[test]
    fn pow_known_values() {
        assert_eq!(Codeword::for_power(0).pow(10).power().unwrap(), 0);
        assert_eq!(Codeword::for_power(1).pow(10).power().unwrap(), 10);
        assert_eq!(Codeword::for_power(62).pow(10).power().unwrap(), 53);
        assert!(Codeword::default().pow(20).power().is_none());
    }

    #[test]
    fn cmp_zero() {
        assert!(Codeword::new(0b000000) == Codeword::new(0b000000));
    }

    // -- Polynomial basics --

    #[test]
    fn polynomial_degree() {
        let p = TestPoly::new((0..23).map(Codeword::for_power));
        assert_eq!(p.degree().unwrap(), 22);
        assert!(p.constant() == Codeword::for_power(0));

        let p = TestPoly::new((1..23).map(Codeword::for_power));
        assert_eq!(p.degree().unwrap(), 21);
        assert!(p.constant() == Codeword::for_power(1));
    }

    // -- Polynomial evaluation --

    #[test]
    fn eval_simple() {
        // p(x) = 1 + x + x^2, evaluate at alpha^1.
        let p = TestPoly::new((0..3).map(|_| Codeword::for_power(0)));
        assert!(p.eval(Codeword::for_power(1)) == 0b000111);

        // p(x) = 1 + x, evaluate at alpha^1.
        let p = TestPoly::new((0..2).map(|_| Codeword::for_power(0)));
        assert_eq!(p.eval(Codeword::for_power(1)), 0b000011);
    }

    #[test]
    fn eval_single_high_term() {
        // p(x) = x^3 at alpha^3.
        let p = TestPoly::new(
            [
                Codeword::default(),
                Codeword::default(),
                Codeword::default(),
                Codeword::for_power(0),
            ]
            .into_iter(),
        );
        assert_eq!(p.eval(Codeword::for_power(3)), 0b011000);
    }

    #[test]
    fn eval_mixed_terms() {
        // p(x) = x + x^3 at alpha^3.
        let p = TestPoly::new(
            [
                Codeword::default(),
                Codeword::for_power(0),
                Codeword::default(),
                Codeword::for_power(0),
            ]
            .into_iter(),
        );
        assert_eq!(p.eval(Codeword::for_power(3)), 0b010000);
    }

    #[test]
    fn eval_high_degree_single_term() {
        // p(x) = alpha^5 * x^23 at alpha^4.
        let mut coefs_vec: Vec<Codeword> = (0..23).map(|_| Codeword::default()).collect();
        coefs_vec.push(Codeword::for_power(5));
        let p = TestPoly::new(coefs_vec.into_iter());
        assert_eq!(p.eval(Codeword::for_power(4)), 0b100100);
    }

    #[test]
    fn eval_constant_plus_high() {
        // p(x) = alpha^12 + alpha^5 * x^23 at alpha^4.
        let mut coefs_vec = vec![Codeword::for_power(12)];
        for _ in 1..23 {
            coefs_vec.push(Codeword::default());
        }
        coefs_vec.push(Codeword::for_power(5));
        let p = TestPoly::new(coefs_vec.into_iter());
        assert_eq!(p.eval(Codeword::for_power(4)), 0b100001);
    }

    #[test]
    fn eval_all_ones_at_one() {
        // p(x) = 1 + x + x^2 + ... + x^23 at alpha^0 = 1.
        // In GF(2), sum of 24 copies of 1 = 0 (even count).
        let p = TestPoly::new((0..24).map(|_| Codeword::for_power(0)));
        assert!(p.eval(Codeword::for_power(0)).zero());
    }

    // -- Polynomial arithmetic --

    #[test]
    fn polynomial_scale() {
        let p = TestPoly::new((1..23).map(Codeword::for_power));

        let q = p * Codeword::for_power(0);
        assert_eq!(q.degree().unwrap(), 21);
        assert!(q.constant() == Codeword::for_power(1));

        let q = p * Codeword::for_power(2);
        assert_eq!(q.degree().unwrap(), 21);
        assert!(q.constant() == Codeword::for_power(3));
    }

    #[test]
    fn polynomial_add_self_is_zero() {
        let p = TestPoly::new((1..23).map(Codeword::for_power));
        let q = p + p;
        assert!(q.constant().zero());
        for coef in q.iter() {
            assert!(coef.zero());
        }
    }

    #[test]
    fn polynomial_add_different_lengths() {
        let p = TestPoly::new((4..27).map(Codeword::for_power));
        let q = TestPoly::new((4..26).map(Codeword::for_power));
        let r = p + q;

        // First 22 terms cancel, last term survives.
        assert!(r.coefs[0].zero());
        assert!(r.coefs[1].zero());
        assert!(r.coefs[2].zero());
        assert!(r.coefs[3].zero());
        assert!(r.coefs[4].zero());
        assert!(!r.coefs[22].zero());
    }

    #[test]
    fn polynomial_add_with_offset() {
        let p = TestPoly::new((0..2).map(|_| Codeword::for_power(0)));
        let q = TestPoly::new((0..4).map(|_| Codeword::for_power(1)));
        let r = p + q;
        assert!(r.coef(0) == Codeword::for_power(6));
    }

    // -- Polynomial multiplication --

    #[test]
    fn poly_mul_simple() {
        // (1+x) * (1+x) = 1 + 0*x + x^2 (in GF(2), x+x=0).
        let p = TestPoly::new((0..2).map(|_| Codeword::for_power(0)));
        let q = p;
        let r = p * q;

        assert_eq!(r.coef(0).power().unwrap(), 0);
        assert!(r.coef(1).power().is_none());
        assert_eq!(r.coef(2).power().unwrap(), 0);
    }

    #[test]
    fn poly_mul_shift() {
        // (1 + alpha*x + alpha^2*x^2) * (x).
        let p = TestPoly::new((0..3).map(Codeword::for_power));
        let q = TestPoly::new(
            [Codeword::default(), Codeword::for_power(0)].into_iter(),
        );
        let r = p * q;

        assert!(r.coef(0).power().is_none());
        assert_eq!(r.coef(1).power().unwrap(), 0);
        assert_eq!(r.coef(2).power().unwrap(), 1);
        assert_eq!(r.coef(3).power().unwrap(), 2);
    }

    // -- Truncate --

    #[test]
    fn truncate_reduces_degree() {
        let p = TestPoly::new((0..5).map(|_| Codeword::for_power(0)));
        assert_eq!(p.degree().unwrap(), 4);
        assert_eq!(p.coefs[4].power().unwrap(), 0);
        assert!(p.coefs[5].power().is_none());

        let p = p.truncate(2);
        assert_eq!(p.degree().unwrap(), 2);
        assert_eq!(p.coefs[2].power().unwrap(), 0);
        assert!(p.coefs[3].power().is_none());
    }

    // -- Unit power --

    #[test]
    fn unit_power_degree_0() {
        let p = TestPoly::unit_power(0);
        assert_eq!(p[0], Codeword::for_power(0));
        assert_eq!(p.degree().unwrap(), 0);
    }

    #[test]
    fn unit_power_degree_2() {
        let p = TestPoly::unit_power(2);
        assert_eq!(p[0], Codeword::default());
        assert_eq!(p[1], Codeword::default());
        assert_eq!(p[2], Codeword::for_power(0));
        assert_eq!(p.degree().unwrap(), 2);
    }

    #[test]
    fn unit_power_degree_10() {
        let p = TestPoly::unit_power(10);
        for i in 0..10 {
            assert_eq!(p[i], Codeword::default());
        }
        assert_eq!(p[10], Codeword::for_power(0));
        assert_eq!(p.degree().unwrap(), 10);
    }

    // -- Derivative --

    #[test]
    fn deriv_basic() {
        // p(x) = alpha^0 + alpha^3*x + alpha^58*x^2 => p'(x) = alpha^3.
        let p = TestPoly::new(
            [
                Codeword::for_power(0),
                Codeword::for_power(3),
                Codeword::for_power(58),
            ]
            .into_iter(),
        );
        let q = p.deriv();
        assert!(q.coefs[0] == Codeword::for_power(3));
        assert!(q.coefs[1] == Codeword::default());
        assert!(q.coefs[2] == Codeword::default());
    }

    #[test]
    fn deriv_after_shift() {
        let p = TestPoly::new(
            [
                Codeword::default(),
                Codeword::for_power(5),
                Codeword::for_power(3),
                Codeword::for_power(58),
            ]
            .into_iter(),
        );
        let q = p.shift().deriv();

        assert!(q.coef(0) == Codeword::for_power(3));
        assert!(q.coef(1) == Codeword::default());
        assert!(q.coef(2) == Codeword::default());
    }

    #[test]
    fn deriv_four_terms() {
        let p = TestPoly::new(
            [
                Codeword::for_power(0),
                Codeword::for_power(5),
                Codeword::for_power(3),
                Codeword::for_power(58),
            ]
            .into_iter(),
        )
        .deriv();

        assert!(p.coef(0) == Codeword::for_power(5));
        assert!(p.coef(1) == Codeword::default());
        assert!(p.coef(2) == Codeword::for_power(58));
        assert!(p.coef(3) == Codeword::default());
        assert!(p.coef(4) == Codeword::default());
    }

    #[test]
    fn deriv_five_terms() {
        let p = TestPoly::new(
            [
                Codeword::for_power(0),
                Codeword::for_power(5),
                Codeword::for_power(3),
                Codeword::for_power(58),
                Codeword::for_power(43),
            ]
            .into_iter(),
        )
        .deriv();

        assert!(p.coef(0) == Codeword::for_power(5));
        assert!(p.coef(1) == Codeword::default());
        assert!(p.coef(2) == Codeword::for_power(58));
        assert!(p.coef(3) == Codeword::default());
        assert!(p.coef(4) == Codeword::default());
        assert!(p.coef(5) == Codeword::default());
    }

    #[test]
    fn deriv_six_terms() {
        let p = TestPoly::new(
            [
                Codeword::for_power(0),
                Codeword::for_power(5),
                Codeword::for_power(3),
                Codeword::for_power(58),
                Codeword::for_power(43),
                Codeword::for_power(15),
            ]
            .into_iter(),
        )
        .deriv();

        assert!(p.coef(0) == Codeword::for_power(5));
        assert!(p.coef(1) == Codeword::default());
        assert!(p.coef(2) == Codeword::for_power(58));
        assert!(p.coef(3) == Codeword::default());
        assert!(p.coef(4) == Codeword::for_power(15));
        assert!(p.coef(5) == Codeword::default());
    }

    #[test]
    fn deriv_short_poly_five_terms() {
        let p = ShortPoly::new(
            [
                Codeword::for_power(0),
                Codeword::for_power(5),
                Codeword::for_power(3),
                Codeword::for_power(58),
                Codeword::for_power(43),
            ]
            .into_iter(),
        )
        .deriv();

        assert!(p.coef(0) == Codeword::for_power(5));
        assert!(p.coef(1) == Codeword::default());
        assert!(p.coef(2) == Codeword::for_power(58));
        assert!(p.coef(3) == Codeword::default());
        assert!(p.coef(4) == Codeword::default());
    }

    #[test]
    fn deriv_full_24_terms() {
        let p = TestPoly::new(
            [
                Codeword::for_power(0),
                Codeword::for_power(5),
                Codeword::for_power(3),
                Codeword::for_power(58),
                Codeword::for_power(43),
                Codeword::for_power(15),
                Codeword::for_power(5),
                Codeword::for_power(3),
                Codeword::for_power(58),
                Codeword::for_power(43),
                Codeword::for_power(15),
                Codeword::for_power(5),
                Codeword::for_power(3),
                Codeword::for_power(58),
                Codeword::for_power(43),
                Codeword::for_power(15),
                Codeword::for_power(5),
                Codeword::for_power(3),
                Codeword::for_power(58),
                Codeword::for_power(43),
                Codeword::for_power(15),
                Codeword::for_power(5),
                Codeword::for_power(3),
                Codeword::for_power(58),
            ]
            .into_iter(),
        )
        .deriv();

        assert!(p.coef(0) == Codeword::for_power(5));
        assert!(p.coef(1) == Codeword::default());
        assert!(p.coef(2) == Codeword::for_power(58));
        assert!(p.coef(3) == Codeword::default());
        assert!(p.coef(4) == Codeword::for_power(15));
        assert!(p.coef(5) == Codeword::default());
        assert!(p.coef(6) == Codeword::for_power(3));
        assert!(p.coef(7) == Codeword::default());
        assert!(p.coef(8) == Codeword::for_power(43));
        assert!(p.coef(9) == Codeword::default());
        assert!(p.coef(10) == Codeword::for_power(5));
        assert!(p.coef(11) == Codeword::default());
        assert!(p.coef(12) == Codeword::for_power(58));
        assert!(p.coef(13) == Codeword::default());
        assert!(p.coef(14) == Codeword::for_power(15));
        assert!(p.coef(15) == Codeword::default());
        assert!(p.coef(16) == Codeword::for_power(3));
        assert!(p.coef(17) == Codeword::default());
        assert!(p.coef(18) == Codeword::for_power(43));
        assert!(p.coef(19) == Codeword::default());
        assert!(p.coef(20) == Codeword::for_power(5));
        assert!(p.coef(21) == Codeword::default());
        assert!(p.coef(22) == Codeword::for_power(58));
        assert!(p.coef(23) == Codeword::default());
    }
}

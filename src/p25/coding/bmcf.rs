//! Berlekamp-Massey, Chien Search, and Forney algorithms for Reed-Solomon
//! error correction.
//!
//! This module implements the standard decoding pipeline for Reed-Solomon
//! codes over GF(2^6):
//!
//! 1. **Berlekamp-Massey** (`ErrorLocator`): builds the error locator
//!    polynomial from the syndrome polynomial.
//! 2. **Chien Search** (`PolynomialRoots`): finds roots of the error locator
//!    by brute-force evaluation over the field.
//! 3. **Forney** (`ErrorDescriptions`): computes error patterns from roots.
//! 4. **Errors**: top-level decoder tying the three together.

use super::galois::{Codeword, Polynomial, PolynomialCoefs};

/// Number of non-zero elements in GF(2^6).
const FIELD_SIZE: usize = 63;

// ---------------------------------------------------------------------------
// Berlekamp-Massey: syndrome polynomial -> error locator polynomial
// ---------------------------------------------------------------------------

/// Builds the error locator polynomial from a syndrome polynomial using the
/// Berlekamp-Massey algorithm.
///
/// Uses the Hankerson et al. variant: the result is the connection polynomial
/// of the shortest LFSR that generates the syndrome sequence.
struct ErrorLocator<P: PolynomialCoefs> {
    /// Saved p polynomial from the last update step.
    p_saved: Polynomial<P>,
    /// Current p polynomial.
    p_cur: Polynomial<P>,
    /// Saved q polynomial from the last update step.
    q_saved: Polynomial<P>,
    /// Current q polynomial.
    q_cur: Polynomial<P>,
    /// Degree-related term of the saved polynomial.
    deg_saved: usize,
    /// Degree-related term of the current polynomial.
    deg_cur: usize,
}

impl<P: PolynomialCoefs> ErrorLocator<P> {
    /// Create a new error locator from the syndrome polynomial s(x).
    fn new(syn: Polynomial<P>) -> Self {
        // q_saved = 1 + s(x)
        let q_saved = Polynomial::new(
            std::iter::once(Codeword::for_power(0))
                .chain(syn.iter().take(P::syndromes()).copied()),
        );

        Self {
            q_saved,
            q_cur: syn,
            // p_saved = x^{2t+1}
            p_saved: Polynomial::unit_power(P::syndromes() + 1),
            // p_cur = x^{2t}
            p_cur: Polynomial::unit_power(P::syndromes()),
            deg_saved: 0,
            deg_cur: 1,
        }
    }

    /// Run the algorithm for 2t iterations and return the error locator.
    fn build(mut self) -> Polynomial<P> {
        for _ in 0..P::syndromes() {
            self.step();
        }
        self.p_cur
    }

    /// One iteration of the algorithm.
    fn step(&mut self) {
        let (save, q, p, d) = if self.q_cur.constant().zero() {
            self.reduce()
        } else {
            self.transform()
        };

        if save {
            self.q_saved = self.q_cur;
            self.p_saved = self.p_cur;
            self.deg_saved = self.deg_cur;
        }

        self.q_cur = q;
        self.p_cur = p;
        self.deg_cur = d;
    }

    /// Shift step when the constant term is zero.
    fn reduce(&mut self) -> (bool, Polynomial<P>, Polynomial<P>, usize) {
        (
            false,
            self.q_cur.shift(),
            self.p_cur.shift(),
            2 + self.deg_cur,
        )
    }

    /// Normalize and shift step when the constant term is non-zero.
    fn transform(&mut self) -> (bool, Polynomial<P>, Polynomial<P>, usize) {
        let mult = self.q_cur.constant() / self.q_saved.constant();
        (
            self.deg_cur >= self.deg_saved,
            (self.q_cur + self.q_saved * mult).shift(),
            (self.p_cur + self.p_saved * mult).shift(),
            2 + std::cmp::min(self.deg_cur, self.deg_saved),
        )
    }
}

// ---------------------------------------------------------------------------
// Chien Search: find roots of the error locator polynomial
// ---------------------------------------------------------------------------

/// Finds roots of a polynomial over GF(2^6) by evaluating at each field
/// element using the Chien Search optimization.
///
/// Rather than re-evaluating the full polynomial each time, each coefficient
/// is multiplied by its corresponding power of alpha to advance to the next
/// evaluation point.
struct PolynomialRoots<P: PolynomialCoefs> {
    /// Working copy of the locator coefficients, updated each step.
    loc: Polynomial<P>,
    /// Range of powers still to evaluate.
    pow: std::ops::Range<usize>,
}

impl<P: PolynomialCoefs> PolynomialRoots<P> {
    /// Create a new root finder for the given polynomial.
    fn new(loc: Polynomial<P>) -> Self {
        Self {
            loc,
            pow: 0..FIELD_SIZE,
        }
    }

    /// Advance each term's coefficient to the next evaluation point.
    fn update_terms(&mut self) {
        for (pow, term) in self.loc.iter_mut().enumerate() {
            *term = *term * Codeword::for_power(pow);
        }
    }

    /// Sum all current coefficients to evaluate the polynomial.
    fn eval(&self) -> Codeword {
        self.loc.iter().fold(Codeword::default(), |sum, &x| sum + x)
    }
}

impl<P: PolynomialCoefs> Iterator for PolynomialRoots<P> {
    type Item = Codeword;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let pow = self.pow.next()?;
            let eval = self.eval();
            self.update_terms();

            if eval.zero() {
                return Some(Codeword::for_power(pow));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Forney algorithm: roots -> error locations and patterns
// ---------------------------------------------------------------------------

/// Computes error locations and patterns from the roots of the error locator
/// polynomial using the Forney algorithm.
///
/// For each root alpha^(-k) of the locator, the error location is k and the
/// error pattern is Omega(root) / Lambda'(root).
struct ErrorDescriptions<P: PolynomialCoefs> {
    /// Derivative of error locator polynomial.
    deriv: Polynomial<P>,
    /// Error evaluator polynomial: Lambda(x) * s(x) mod x^{2t}.
    vals: Polynomial<P>,
}

impl<P: PolynomialCoefs> ErrorDescriptions<P> {
    /// Create from the syndrome polynomial and error locator polynomial.
    fn new(syn: Polynomial<P>, loc: Polynomial<P>) -> Self {
        Self {
            deriv: loc.deriv(),
            vals: (loc * syn).truncate(P::syndromes() - 1),
        }
    }

    /// Compute the error location and pattern for a root of the locator.
    fn for_root(&self, root: Codeword) -> (usize, Codeword) {
        (
            root.invert().power().unwrap(),
            self.vals.eval(root) / self.deriv.eval(root),
        )
    }
}

// ---------------------------------------------------------------------------
// Top-level decoder
// ---------------------------------------------------------------------------

/// Decodes Reed-Solomon errors from a syndrome polynomial.
///
/// Returns the number of errors and an iterator over (location, pattern) pairs.
/// Returns `None` if the codeword is unrecoverable (root count mismatch).
pub struct Errors<P: PolynomialCoefs> {
    /// Buffered roots of the error locator polynomial.
    roots: Polynomial<P>,
    /// Forney evaluator for computing locations and patterns.
    descs: ErrorDescriptions<P>,
    /// Current position in the root buffer.
    pos: std::ops::Range<usize>,
}

impl<P: PolynomialCoefs> Errors<P> {
    /// Decode errors from the syndrome polynomial s(x).
    ///
    /// Returns `Some((error_count, error_iterator))` on success, or `None` if
    /// the number of roots does not match the degree of the error locator
    /// (unrecoverable error).
    pub fn new(syn: Polynomial<P>) -> Option<(usize, Self)> {
        let loc = ErrorLocator::new(syn).build();

        let errors = match loc.degree() {
            Some(e) => e,
            None => return Some((0, Self::empty(syn, loc))),
        };

        // Buffer roots: collect into polynomial-sized storage.
        let mut roots = Polynomial::<P>::default();
        let mut nroots = 0;
        for root in PolynomialRoots::new(loc) {
            if nroots < roots.len() {
                roots[nroots] = root;
            }
            nroots += 1;
        }

        // Root count must match deg(Lambda) for valid decoding.
        if nroots != errors {
            return None;
        }

        Some((
            errors,
            Self {
                roots,
                descs: ErrorDescriptions::new(syn, loc),
                pos: 0..errors,
            },
        ))
    }

    /// Create an empty error iterator (no errors detected).
    fn empty(syn: Polynomial<P>, loc: Polynomial<P>) -> Self {
        Self {
            roots: Polynomial::default(),
            descs: ErrorDescriptions::new(syn, loc),
            pos: 0..0,
        }
    }
}

impl<P: PolynomialCoefs> Iterator for Errors<P> {
    type Item = (usize, Codeword);

    fn next(&mut self) -> Option<Self::Item> {
        self.pos.next().map(|i| self.descs.for_root(self.roots[i]))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p25::coding::galois::impl_polynomial_coefs;

    impl_polynomial_coefs!(TestCoefs, 9);
    type TestPoly = Polynomial<TestCoefs>;

    #[test]
    fn chien_search_finds_known_roots() {
        // p(x) = (1 + alpha^42 * x)(1 + alpha^13 * x)(1 + alpha^57 * x)
        let p = TestPoly::new(
            [Codeword::for_power(0), Codeword::for_power(42)]
                .into_iter(),
        ) * TestPoly::new(
            [Codeword::for_power(0), Codeword::for_power(13)]
                .into_iter(),
        ) * TestPoly::new(
            [Codeword::for_power(0), Codeword::for_power(57)]
                .into_iter(),
        );

        let roots: Vec<Codeword> = PolynomialRoots::new(p).collect();
        assert_eq!(roots.len(), 3);

        let inv42 = Codeword::for_power(42).invert();
        let inv13 = Codeword::for_power(13).invert();
        let inv57 = Codeword::for_power(57).invert();

        assert!(roots.iter().any(|r| r.bits() == inv42.bits()));
        assert!(roots.iter().any(|r| r.bits() == inv13.bits()));
        assert!(roots.iter().any(|r| r.bits() == inv57.bits()));
    }

    #[test]
    fn chien_search_constant_polynomial_has_no_roots() {
        let p = TestPoly::unit_power(0);
        let roots: Vec<Codeword> = PolynomialRoots::new(p).collect();
        assert!(roots.is_empty());
    }

    #[test]
    fn errors_zero_syndrome_returns_zero_errors() {
        let syn = TestPoly::default();
        let (nerr, errs) = Errors::new(syn).unwrap();
        assert_eq!(nerr, 0);
        assert_eq!(errs.count(), 0);
    }

    #[test]
    fn berlekamp_massey_single_error() {
        // Syndrome for a single error at position p with pattern e:
        // s_i = e * alpha^(p*i) for i = 1..2t
        let error_pos: usize = 5;
        let error_val = Codeword::for_power(10);

        let syn = TestPoly::new((1..=TestCoefs::syndromes()).map(|i| {
            error_val * Codeword::for_power(error_pos * i)
        }));

        let (nerr, mut errs) = Errors::new(syn).unwrap();
        assert_eq!(nerr, 1);

        let (loc, pat) = errs.next().unwrap();
        assert_eq!(loc, error_pos);
        assert_eq!(pat.bits(), error_val.bits());
        assert!(errs.next().is_none());
    }

    #[test]
    fn berlekamp_massey_two_errors() {
        // Two errors at positions p1, p2 with patterns e1, e2:
        // s_i = e1 * alpha^(p1*i) + e2 * alpha^(p2*i)
        let p1: usize = 3;
        let p2: usize = 10;
        let e1 = Codeword::for_power(7);
        let e2 = Codeword::for_power(20);

        let syn = TestPoly::new((1..=TestCoefs::syndromes()).map(|i| {
            e1 * Codeword::for_power(p1 * i) + e2 * Codeword::for_power(p2 * i)
        }));

        let (nerr, errs) = Errors::new(syn).unwrap();
        assert_eq!(nerr, 2);

        let corrections: Vec<(usize, Codeword)> = errs.collect();
        assert_eq!(corrections.len(), 2);

        // Find each error by location.
        let c1 = corrections.iter().find(|(l, _)| *l == p1).unwrap();
        let c2 = corrections.iter().find(|(l, _)| *l == p2).unwrap();
        assert_eq!(c1.1.bits(), e1.bits());
        assert_eq!(c2.1.bits(), e2.bits());
    }

    #[test]
    fn berlekamp_massey_four_errors() {
        // Four errors (max for distance 9, t=4)
        let positions: [usize; 4] = [1, 15, 30, 50];
        let patterns: [Codeword; 4] = [
            Codeword::for_power(0),
            Codeword::for_power(11),
            Codeword::for_power(33),
            Codeword::for_power(55),
        ];

        let syn = TestPoly::new((1..=TestCoefs::syndromes()).map(|i| {
            positions
                .iter()
                .zip(patterns.iter())
                .fold(Codeword::default(), |acc, (&pos, &pat)| {
                    acc + pat * Codeword::for_power(pos * i)
                })
        }));

        let (nerr, errs) = Errors::new(syn).unwrap();
        assert_eq!(nerr, 4);

        let corrections: Vec<(usize, Codeword)> = errs.collect();
        assert_eq!(corrections.len(), 4);

        for (&pos, &pat) in positions.iter().zip(patterns.iter()) {
            let c = corrections.iter().find(|(l, _)| *l == pos).unwrap();
            assert_eq!(c.1.bits(), pat.bits(), "wrong pattern at position {pos}");
        }
    }
}

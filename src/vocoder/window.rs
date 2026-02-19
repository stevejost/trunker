//! Speech synthesis window for frame overlap-add.
//!
//! The synthesis window is a 211-coefficient trapezoidal (Tukey) window
//! with linear tapers from 0 to 1 over 50 samples, flat at 1.0 for
//! 111 samples, symmetric about the center. Used by the voiced and
//! unvoiced synthesis stages per TIA-102.BABA [p95].

/// Retrieve the speech synthesis window w_s.
pub(crate) fn synthesis() -> Window {
    Window::new(&SYNTHESIS_COEFFICIENTS)
}

/// Wraps a set of window coefficients and remaps the center coefficient
/// to index 0.
///
/// Supports signed index access: `get(n)` for n in -offset..=+offset,
/// returning 0.0 for out-of-bounds indices.
pub(crate) struct Window {
    /// Coefficients of the window.
    coefficients: &'static [f32],
    /// Offset into the coefficients array of the center coefficient (n = 0).
    offset: isize,
}

impl Window {
    /// Create a new `Window` with the given coefficients.
    ///
    /// The center coefficient is at index `coefficients.len() / 2`.
    fn new(coefficients: &'static [f32]) -> Self {
        Self {
            coefficients,
            offset: coefficients.len() as isize / 2,
        }
    }

    /// Retrieve the coefficient w(n) for the given signed index n.
    ///
    /// Returns 0.0 for out-of-bounds indices. The center coefficient
    /// (n = 0) corresponds to the midpoint of the coefficient array.
    pub(crate) fn get(&self, n: isize) -> f32 {
        let index = n + self.offset;
        if index < 0 {
            return 0.0;
        }
        match self.coefficients.get(index as usize) {
            Some(&coefficient) => coefficient,
            None => 0.0,
        }
    }
}

/// Energy of the speech synthesis window (sum of w_s^2).
///
/// Used by unvoiced synthesis for energy normalization.
#[allow(clippy::excessive_precision)]
pub(crate) const SYNTHESIS_ENERGY: f32 = 143.3399810791015625;

/// Coefficients of the speech synthesis window [p95].
///
/// 211 coefficients: 50-sample linear taper (0.0 to 1.0), 111 samples
/// at 1.0, 50-sample linear taper (1.0 to 0.0). Symmetric about the
/// center (index 105).
static SYNTHESIS_COEFFICIENTS: [f32; 211] = [
    0.000000, 0.020000, 0.040000, 0.060000, 0.080000,
    0.100000, 0.120000, 0.140000, 0.160000, 0.180000,
    0.200000, 0.220000, 0.240000, 0.260000, 0.280000,
    0.300000, 0.320000, 0.340000, 0.360000, 0.380000,
    0.400000, 0.420000, 0.440000, 0.460000, 0.480000,
    0.500000, 0.520000, 0.540000, 0.560000, 0.580000,
    0.600000, 0.620000, 0.640000, 0.660000, 0.680000,
    0.700000, 0.720000, 0.740000, 0.760000, 0.780000,
    0.800000, 0.820000, 0.840000, 0.860000, 0.880000,
    0.900000, 0.920000, 0.940000, 0.960000, 0.980000,
    1.000000, 1.000000, 1.000000, 1.000000, 1.000000,
    1.000000, 1.000000, 1.000000, 1.000000, 1.000000,
    1.000000, 1.000000, 1.000000, 1.000000, 1.000000,
    1.000000, 1.000000, 1.000000, 1.000000, 1.000000,
    1.000000, 1.000000, 1.000000, 1.000000, 1.000000,
    1.000000, 1.000000, 1.000000, 1.000000, 1.000000,
    1.000000, 1.000000, 1.000000, 1.000000, 1.000000,
    1.000000, 1.000000, 1.000000, 1.000000, 1.000000,
    1.000000, 1.000000, 1.000000, 1.000000, 1.000000,
    1.000000, 1.000000, 1.000000, 1.000000, 1.000000,
    1.000000, 1.000000, 1.000000, 1.000000, 1.000000,
    1.000000, 1.000000, 1.000000, 1.000000, 1.000000,
    1.000000, 1.000000, 1.000000, 1.000000, 1.000000,
    1.000000, 1.000000, 1.000000, 1.000000, 1.000000,
    1.000000, 1.000000, 1.000000, 1.000000, 1.000000,
    1.000000, 1.000000, 1.000000, 1.000000, 1.000000,
    1.000000, 1.000000, 1.000000, 1.000000, 1.000000,
    1.000000, 1.000000, 1.000000, 1.000000, 1.000000,
    1.000000, 1.000000, 1.000000, 1.000000, 1.000000,
    1.000000, 1.000000, 1.000000, 1.000000, 1.000000,
    1.000000, 1.000000, 1.000000, 1.000000, 1.000000,
    1.000000, 1.000000, 1.000000, 1.000000, 1.000000,
    1.000000,
    0.980000, 0.960000, 0.940000, 0.920000, 0.900000,
    0.880000, 0.860000, 0.840000, 0.820000, 0.800000,
    0.780000, 0.760000, 0.740000, 0.720000, 0.700000,
    0.680000, 0.660000, 0.640000, 0.620000, 0.600000,
    0.580000, 0.560000, 0.540000, 0.520000, 0.500000,
    0.480000, 0.460000, 0.440000, 0.420000, 0.400000,
    0.380000, 0.360000, 0.340000, 0.320000, 0.300000,
    0.280000, 0.260000, 0.240000, 0.220000, 0.200000,
    0.180000, 0.160000, 0.140000, 0.120000, 0.100000,
    0.080000, 0.060000, 0.040000, 0.020000, 0.000000,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthesis_window_spot_checks() {
        let w = synthesis();

        // Far out-of-bounds
        assert_eq!(w.get(-200), 0.0);
        // Just outside window
        assert_eq!(w.get(-106), 0.0);
        // First coefficient (zero at edge)
        assert_eq!(w.get(-105), 0.0);
        // In the rising taper
        assert_eq!(w.get(-104), 0.02);
        assert_eq!(w.get(-68), 0.74);
        // Center
        assert_eq!(w.get(0), 1.0);
        // In the falling taper
        assert_eq!(w.get(77), 0.56);
        assert_eq!(w.get(104), 0.02);
        // Last coefficient (zero at edge)
        assert_eq!(w.get(105), 0.0);
        // Just outside window
        assert_eq!(w.get(106), 0.0);
        // Far out-of-bounds
        assert_eq!(w.get(200), 0.0);
    }

    #[test]
    fn synthesis_window_energy() {
        let energy: f32 = SYNTHESIS_COEFFICIENTS.iter().map(|&x| x.powi(2)).sum();
        assert_eq!(energy, SYNTHESIS_ENERGY);
    }

    #[test]
    fn synthesis_window_symmetry() {
        let w = synthesis();
        for n in 0..=105 {
            assert_eq!(
                w.get(n),
                w.get(-n),
                "window not symmetric at n={n}"
            );
        }
    }

    #[test]
    fn synthesis_window_length() {
        assert_eq!(SYNTHESIS_COEFFICIENTS.len(), 211);
    }

    #[test]
    fn synthesis_window_taper_is_linear() {
        // The rising taper should be 0.02 * i for i in 0..=50.
        for i in 0..=50 {
            let expected = 0.02 * i as f32;
            assert!(
                (SYNTHESIS_COEFFICIENTS[i] - expected).abs() < 1e-6,
                "taper mismatch at index {i}: expected {expected}, got {}",
                SYNTHESIS_COEFFICIENTS[i]
            );
        }
    }
}

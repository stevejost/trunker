//! Cubic Lagrange interpolator for sub-sample timing recovery.
//!
//! Given 4 consecutive samples and a fractional delay, computes the
//! interpolated value using Lagrange 4-point polynomial interpolation.
//! Used by the Gardner timing error detector to evaluate the signal
//! at arbitrary points between samples.

use num_complex::Complex;

/// Interpolate a real-valued signal using 4-point Lagrange interpolation.
///
/// `samples` contains \[x\[-1\], x\[0\], x\[1\], x\[2\]\] — four consecutive
/// samples centered around the interpolation region. `mu` is the fractional
/// delay in the range 0.0..1.0, where mu=0 returns x\[0\] and mu=1 returns x\[1\].
pub fn lagrange4_real(samples: [f32; 4], mu: f32) -> f32 {
    let [xm1, x0, x1, x2] = samples;
    let mu_m1 = mu - 1.0;
    let mu_m2 = mu - 2.0;
    let mu_p1 = mu + 1.0;

    xm1 * (-mu * mu_m1 * mu_m2) / 6.0
        + x0 * (mu_p1 * mu_m1 * mu_m2) / 2.0
        + x1 * (-mu_p1 * mu * mu_m2) / 2.0
        + x2 * (mu_p1 * mu * mu_m1) / 6.0
}

/// Interpolate a complex-valued signal using 4-point Lagrange interpolation.
///
/// Same algorithm as [`lagrange4_real`] but operates on complex samples.
/// Used in the CQPSK demodulation path where timing recovery operates
/// on complex IQ data before carrier recovery.
pub fn lagrange4_complex(samples: [Complex<f32>; 4], mu: f32) -> Complex<f32> {
    let [xm1, x0, x1, x2] = samples;
    let mu_m1 = mu - 1.0;
    let mu_m2 = mu - 2.0;
    let mu_p1 = mu + 1.0;

    let c0 = -mu * mu_m1 * mu_m2 / 6.0;
    let c1 = mu_p1 * mu_m1 * mu_m2 / 2.0;
    let c2 = -mu_p1 * mu * mu_m2 / 2.0;
    let c3 = mu_p1 * mu * mu_m1 / 6.0;

    xm1 * c0 + x0 * c1 + x1 * c2 + x2 * c3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mu_zero_returns_x0() {
        let samples = [1.0, 5.0, 9.0, 13.0];
        let result = lagrange4_real(samples, 0.0);
        assert!(
            (result - 5.0).abs() < 1e-6,
            "mu=0 should return x[0]=5.0, got {result}"
        );
    }

    #[test]
    fn mu_one_returns_x1() {
        let samples = [1.0, 5.0, 9.0, 13.0];
        let result = lagrange4_real(samples, 1.0);
        assert!(
            (result - 9.0).abs() < 1e-6,
            "mu=1 should return x[1]=9.0, got {result}"
        );
    }

    #[test]
    fn interpolates_linear_exactly() {
        // Lagrange 4-point exactly fits any polynomial up to degree 3.
        // For a linear function f(x) = 2x + 1, samples at x = -1, 0, 1, 2:
        let samples = [-1.0, 1.0, 3.0, 5.0]; // f(-1)=-1, f(0)=1, f(1)=3, f(2)=5
        let result = lagrange4_real(samples, 0.5);
        // f(0.5) = 2*0.5 + 1 = 2.0
        assert!(
            (result - 2.0).abs() < 1e-6,
            "linear at mu=0.5: expected 2.0, got {result}"
        );
    }

    #[test]
    fn interpolates_quadratic_exactly() {
        // f(x) = x^2, samples at x = -1, 0, 1, 2:
        let samples = [1.0, 0.0, 1.0, 4.0]; // f(-1)=1, f(0)=0, f(1)=1, f(2)=4
        let result = lagrange4_real(samples, 0.5);
        // f(0.5) = 0.25
        assert!(
            (result - 0.25).abs() < 1e-6,
            "quadratic at mu=0.5: expected 0.25, got {result}"
        );
    }

    #[test]
    fn interpolates_cubic_exactly() {
        // f(x) = x^3, samples at x = -1, 0, 1, 2:
        let samples = [-1.0, 0.0, 1.0, 8.0]; // f(-1)=-1, f(0)=0, f(1)=1, f(2)=8
        let result = lagrange4_real(samples, 0.5);
        // f(0.5) = 0.125
        assert!(
            (result - 0.125).abs() < 1e-5,
            "cubic at mu=0.5: expected 0.125, got {result}"
        );
    }

    #[test]
    fn complex_mu_zero_returns_x0() {
        let samples = [
            Complex::new(1.0, 2.0),
            Complex::new(3.0, 4.0),
            Complex::new(5.0, 6.0),
            Complex::new(7.0, 8.0),
        ];
        let result = lagrange4_complex(samples, 0.0);
        assert!(
            (result.re - 3.0).abs() < 1e-6 && (result.im - 4.0).abs() < 1e-6,
            "mu=0 should return x[0]=(3,4), got ({}, {})",
            result.re,
            result.im
        );
    }

    #[test]
    fn complex_mu_one_returns_x1() {
        let samples = [
            Complex::new(1.0, 2.0),
            Complex::new(3.0, 4.0),
            Complex::new(5.0, 6.0),
            Complex::new(7.0, 8.0),
        ];
        let result = lagrange4_complex(samples, 1.0);
        assert!(
            (result.re - 5.0).abs() < 1e-6 && (result.im - 6.0).abs() < 1e-6,
            "mu=1 should return x[1]=(5,6), got ({}, {})",
            result.re,
            result.im
        );
    }

    #[test]
    fn complex_interpolates_linear_exactly() {
        // Linear in both real and imaginary: re = 2x+1, im = -x+3
        // Samples at x = -1, 0, 1, 2:
        let samples = [
            Complex::new(-1.0, 4.0), // f(-1) = (-1, 4)
            Complex::new(1.0, 3.0),  // f(0)  = (1, 3)
            Complex::new(3.0, 2.0),  // f(1)  = (3, 2)
            Complex::new(5.0, 1.0),  // f(2)  = (5, 1)
        ];
        let result = lagrange4_complex(samples, 0.5);
        // f(0.5) = (2, 2.5)
        assert!(
            (result.re - 2.0).abs() < 1e-6 && (result.im - 2.5).abs() < 1e-6,
            "complex linear at mu=0.5: expected (2, 2.5), got ({}, {})",
            result.re,
            result.im
        );
    }

    #[test]
    fn real_and_complex_agree_on_real_data() {
        let real_samples = [2.0, 4.0, 7.0, 11.0];
        let complex_samples = real_samples.map(|r| Complex::new(r, 0.0));

        for mu_int in 0..=10 {
            let mu = mu_int as f32 / 10.0;
            let real_result = lagrange4_real(real_samples, mu);
            let complex_result = lagrange4_complex(complex_samples, mu);
            assert!(
                (real_result - complex_result.re).abs() < 1e-6,
                "mu={mu}: real={real_result}, complex.re={}",
                complex_result.re
            );
            assert!(
                complex_result.im.abs() < 1e-6,
                "mu={mu}: imaginary should be 0, got {}",
                complex_result.im
            );
        }
    }
}

//! Unvoiced spectrum synthesis.
//!
//! Generates the noise component of speech by constructing a frequency-domain
//! representation of shaped random noise, bandpass filtering by voiced/unvoiced
//! decisions, and performing an inverse DFT to produce time-domain samples.
//!
//! The DFT of a windowed white noise signal can have its points sampled from a
//! Gaussian distribution rather than computed via O(N) DFT. For a real white
//! noise signal with unit variance, the DFT has real and imaginary parts
//! distributed as N(0, E_w / 2) where E_w is the synthesis window energy.
//!
//! Since the noise signal is real, the DFT has conjugate symmetry and only the
//! positive half (128 bins) is needed. The IDFT then simplifies to a sum over
//! cosine and sine terms without complex arithmetic.

use std::f32::consts::PI;

use num_complex::Complex;
use rand::Rng;
use rand_distr::{Distribution, Normal};

use super::consts::SAMPLES_PER_FRAME;
use super::descramble::VoiceDecisions;
use super::enhance::EnhancedSpectrals;
use super::params::BaseParams;
use super::window;

/// Unvoiced scaling coefficient gamma_w computed from Eq 121.
#[allow(clippy::excessive_precision)]
const SCALING_COEFFICIENT: f32 = 146.6432708443356;

/// Number of points in the DFT / IDFT.
const DFT_SIZE: usize = 256;

/// Number of points in the positive half of the DFT.
const DFT_HALF: usize = DFT_SIZE / 2;

/// Frequency-domain representation of the unvoiced noise signal.
///
/// Contains the positive-half DFT bins (128 complex values). Voiced bands
/// are zeroed; unvoiced bands contain scaled Gaussian noise matching the
/// enhanced spectral amplitudes.
pub(crate) struct UnvoicedDft {
    /// Positive-half DFT bins U_w(0) through U_w(127).
    bins: [Complex<f32>; DFT_HALF],
}

impl UnvoicedDft {
    /// Construct a new `UnvoicedDft` from the given frame parameters and RNG.
    ///
    /// For each unvoiced harmonic band:
    /// 1. Populates DFT bins with Gaussian noise (variance E_w / 2)
    /// 2. Computes band energy and normalizes to match the enhanced spectral
    ///    amplitude (Eq 120)
    pub(crate) fn new<R: Rng>(
        params: &BaseParams,
        voice: &VoiceDecisions,
        enhanced: &EnhancedSpectrals,
        mut rng: R,
    ) -> Self {
        // DFT values default to 0 according to Eqs 119 and 124.
        let mut bins = [Complex::new(0.0f32, 0.0); DFT_HALF];

        // Create a Gaussian distribution with mean 0 and variance E_w / 2.
        // Normal::new takes standard deviation, not variance.
        let stddev = (window::SYNTHESIS_ENERGY / 2.0).sqrt();
        let gaussian = Normal::new(0.0f32, stddev).unwrap();

        for (l, &amplitude) in enhanced.iter().enumerate() {
            let l = l + 1;

            if voice.is_voiced(l) {
                continue;
            }

            // Compute the lower and upper frequency band edges for this harmonic.
            let (lower, upper) = edges(l, params);

            // Populate the current band with random spectrum.
            for bin in &mut bins[lower..upper] {
                *bin = Complex::new(
                    gaussian.sample(&mut rng),
                    gaussian.sample(&mut rng),
                );
            }

            // Compute energy of current band according to Eq 120.
            let energy: f32 = bins[lower..upper]
                .iter()
                .map(|c| c.norm_sqr())
                .fold(0.0, |s, x| s + x);
            // Compute power of current band according to Eq 120.
            let power = energy / (upper - lower) as f32;
            // Compute scale for current enhanced spectral amplitude according to Eq 120.
            let scale = SCALING_COEFFICIENT * amplitude / power.sqrt();

            // Scale the band according to Eq 120.
            for bin in &mut bins[lower..upper] {
                *bin *= scale;
            }
        }

        UnvoicedDft { bins }
    }

    /// Compute the IDFT u_w(n) at the given point n.
    ///
    /// Uses the real-signal symmetry to compute the IDFT from only the
    /// positive-half DFT bins. Returns 0.0 for n outside [-128, 127].
    pub(crate) fn idft(&self, n: isize) -> f32 {
        // The IDFT is zero outside the defined range [p59].
        if n < -(DFT_HALF as isize) || n >= DFT_HALF as isize {
            return 0.0;
        }

        // Compute the angular step for this sample index.
        let step = 2.0 / DFT_SIZE as f32 * PI * n as f32;

        // Sum over the positive-half bins using conjugate symmetry.
        // Each bin m contributes: 2 * (Re[U(m)] * cos(m*step) - Im[U(m)] * sin(m*step))
        let sum: f32 = self
            .bins
            .iter()
            .enumerate()
            .map(|(m, bin)| {
                let angle = step * m as f32;
                bin.re * angle.cos() - bin.im * angle.sin()
            })
            .fold(0.0, |s, x| s + x);

        2.0 / DFT_SIZE as f32 * sum
    }
}

impl Default for UnvoicedDft {
    /// Create a new `UnvoicedDft` in the default state.
    ///
    /// By default all DFT bins are zero [p64], producing zero IDFT output.
    fn default() -> Self {
        UnvoicedDft {
            bins: [Complex::new(0.0, 0.0); DFT_HALF],
        }
    }
}

/// Synthesizes unvoiced spectrum signal s_uv(n).
///
/// Combines the current and previous frame unvoiced signals using weighted
/// overlap-add with the synthesis window (Eq 126).
pub(crate) struct Unvoiced<'a> {
    /// Unvoiced DFT/IDFT for the current frame.
    current: &'a UnvoicedDft,
    /// Unvoiced DFT/IDFT for the previous frame.
    previous: &'a UnvoicedDft,
    /// Synthesis window for weighted overlap-add.
    window: window::Window,
}

impl<'a> Unvoiced<'a> {
    /// Create a new `Unvoiced` from the given unvoiced spectrums of the
    /// current and previous frames.
    pub(crate) fn new(current: &'a UnvoicedDft, previous: &'a UnvoicedDft) -> Self {
        Unvoiced {
            current,
            previous,
            window: window::synthesis(),
        }
    }

    /// Compute the unvoiced signal sample s_uv(n) for the given n,
    /// 0 <= n < 160 (Eq 126).
    ///
    /// Uses weighted overlap-add: the denominator w_s(n)^2 + w_s(n-N)^2
    /// is always positive within the frame because at least one of the two
    /// window evaluations is non-zero for any valid n.
    pub(crate) fn get(&self, n: usize) -> f32 {
        debug_assert!(n < SAMPLES_PER_FRAME);

        let n = n as isize;

        // Compute numerator in Eq 126.
        let numerator = self.window.get(n) * self.previous.idft(n)
            + self.window.get(n - SAMPLES_PER_FRAME as isize)
                * self.current.idft(n - SAMPLES_PER_FRAME as isize);

        // Compute denominator in Eq 126.
        // Always positive: w_s(n) and w_s(n-N) overlap such that at least
        // one is non-zero for all 0 <= n < N.
        let denominator = self.window.get(n).powi(2)
            + self.window.get(n - SAMPLES_PER_FRAME as isize).powi(2);

        numerator / denominator
    }
}

/// Determine the lower and upper band edges (a_l, b_l) for the given
/// harmonic of the fundamental frequency (Eqs 122-123).
fn edges(l: usize, params: &BaseParams) -> (usize, usize) {
    let common = DFT_SIZE as f32 / (2.0 * PI) * params.fundamental_frequency;

    (
        // Compute Eq 122: a_l = ceil(256 / (2*pi) * w_0 * (l - 0.5))
        (common * (l as f32 - 0.5)).ceil() as usize,
        // Compute Eq 123: b_l = ceil(256 / (2*pi) * w_0 * (l + 0.5))
        (common * (l as f32 + 0.5)).ceil() as usize,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vocoder::descramble::Bootstrap;
    use rand::SeedableRng;
    use rand::rngs::SmallRng;

    #[test]
    fn band_edges_with_16_harmonics() {
        let chunks = [
            0b001000010010,
            0b110011001100,
            0b111000111000,
            0b111111111111,
            0b10100110101,
            0b00101111010,
            0b01110111011,
            0b00001000,
        ];

        let bootstrap = Bootstrap::new(&chunks);
        let params = BaseParams::new(bootstrap.unwrap_period());

        assert!((params.fundamental_frequency - 0.17575344).abs() < 0.000001);

        let (lower, upper) = edges(1, &params);
        assert_eq!(lower, 4);
        assert_eq!(upper, 11);

        let (lower, upper) = edges(2, &params);
        assert_eq!(lower, 11);
        assert_eq!(upper, 18);

        let (lower, upper) = edges(5, &params);
        assert_eq!(lower, 33);
        assert_eq!(upper, 40);

        let (lower, upper) = edges(8, &params);
        assert_eq!(lower, 54);
        assert_eq!(upper, 61);

        let (lower, upper) = edges(16, &params);
        assert_eq!(lower, 111);
        assert_eq!(upper, 119);
    }

    #[test]
    fn band_edges_are_contiguous() {
        // Verify that adjacent harmonics have contiguous band edges:
        // upper of harmonic l == lower of harmonic l+1.
        let params = BaseParams::new(42);

        for l in 1..params.harmonic_count as usize {
            let (_, upper) = edges(l, &params);
            let (lower_next, _) = edges(l + 1, &params);
            assert_eq!(
                upper, lower_next,
                "band edge gap between harmonics {l} and {}",
                l + 1
            );
        }
    }

    #[test]
    fn band_edges_within_dft_half() {
        // Verify that all band edges stay within [0, DFT_HALF).
        for period in 0..=207u8 {
            let params = BaseParams::new(period);
            for l in 1..=params.harmonic_count as usize {
                let (lower, upper) = edges(l, &params);
                assert!(
                    lower < DFT_HALF && upper <= DFT_HALF,
                    "band edge out of range for period={period}, l={l}: ({lower}, {upper})"
                );
            }
        }
    }

    #[test]
    fn dft_voiced_bands_are_zero() {
        // Verify that DFT bins in voiced bands are zeroed.
        let params = BaseParams::new(42);
        assert_eq!(params.harmonic_count, 18);

        let mut voice = VoiceDecisions::new(0b101001, &params);
        voice.force_voiced(5);
        voice.force_voiced(13);
        voice.force_voiced(14);

        let enhanced = EnhancedSpectrals::from_values(&[
            2.0, 1.0, 4.0, 6.0, 42.0, 8.0, 1.5, 0.5, 24.0, 32.0, 3.0, 7.0, 13.0, 5.0, 4.2,
            11.0, 9.0, 18.0,
        ]);

        let rng = SmallRng::seed_from_u64(42);
        let dft = UnvoicedDft::new(&params, &voice, &enhanced, rng);

        // Check each harmonic band.
        for l in 1..=params.harmonic_count as usize {
            let (lower, upper) = edges(l, &params);
            let is_voiced = voice.is_voiced(l);

            for m in lower..upper {
                if is_voiced {
                    assert_eq!(
                        dft.bins[m],
                        Complex::new(0.0, 0.0),
                        "voiced harmonic {l} bin {m} should be zero"
                    );
                }
            }
        }

        // Bins below band a_1 should be zero.
        let (a1, _) = edges(1, &params);
        for m in 0..a1 {
            assert_eq!(
                dft.bins[m],
                Complex::new(0.0, 0.0),
                "bin {m} below a_1 should be zero"
            );
        }

        // Bins above band b_L should be zero.
        let (_, b_last) = edges(params.harmonic_count as usize, &params);
        for m in b_last..DFT_HALF {
            assert_eq!(
                dft.bins[m],
                Complex::new(0.0, 0.0),
                "bin {m} above b_L should be zero"
            );
        }
    }

    #[test]
    fn dft_unvoiced_bands_are_nonzero() {
        // Verify that DFT bins in unvoiced bands are populated.
        let params = BaseParams::new(42);

        let mut voice = VoiceDecisions::new(0b101001, &params);
        voice.force_voiced(5);
        voice.force_voiced(13);
        voice.force_voiced(14);

        let enhanced = EnhancedSpectrals::from_values(&[
            2.0, 1.0, 4.0, 6.0, 42.0, 8.0, 1.5, 0.5, 24.0, 32.0, 3.0, 7.0, 13.0, 5.0, 4.2,
            11.0, 9.0, 18.0,
        ]);

        let rng = SmallRng::seed_from_u64(42);
        let dft = UnvoicedDft::new(&params, &voice, &enhanced, rng);

        // Unvoiced harmonics: 4, 6, 10, 11, 12, 15
        let unvoiced_harmonics = [4, 6, 10, 11, 12, 15];
        for &l in &unvoiced_harmonics {
            let (lower, upper) = edges(l, &params);
            let has_nonzero = (lower..upper).any(|m| dft.bins[m] != Complex::new(0.0, 0.0));
            assert!(
                has_nonzero,
                "unvoiced harmonic {l} should have non-zero bins"
            );
        }
    }

    #[test]
    fn dft_energy_scaling() {
        // Verify that unvoiced bands are scaled so that the average bin
        // power matches SCALING_COEFFICIENT^2 * amplitude^2.
        let params = BaseParams::new(42);

        // All unvoiced.
        let voice = VoiceDecisions::new(0, &params);

        let enhanced = EnhancedSpectrals::from_values(&[
            10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 10.0,
            10.0, 10.0, 10.0, 10.0, 10.0,
        ]);

        let rng = SmallRng::seed_from_u64(123);
        let dft = UnvoicedDft::new(&params, &voice, &enhanced, rng);

        // After scaling, each band should have:
        // sum(|U(m)|^2) / bandwidth = (SCALING_COEFFICIENT * amplitude)^2
        // i.e., per-bin average power = (SCALING_COEFFICIENT * amplitude)^2
        let expected_power = (SCALING_COEFFICIENT * 10.0).powi(2);

        for l in 1..=params.harmonic_count as usize {
            let (lower, upper) = edges(l, &params);
            let bandwidth = (upper - lower) as f32;
            let band_energy: f32 = dft.bins[lower..upper]
                .iter()
                .map(|c| c.norm_sqr())
                .sum();
            let average_power = band_energy / bandwidth;

            assert!(
                (average_power - expected_power).abs() / expected_power < 0.01,
                "harmonic {l}: expected power {expected_power}, got {average_power}"
            );
        }
    }

    #[test]
    fn idft_out_of_range_is_zero() {
        let dft = UnvoicedDft::default();
        assert_eq!(dft.idft(-129), 0.0);
        assert_eq!(dft.idft(-128), 0.0); // in range but all bins zero
        assert_eq!(dft.idft(127), 0.0); // in range but all bins zero
        assert_eq!(dft.idft(128), 0.0);
        assert_eq!(dft.idft(200), 0.0);
    }

    #[test]
    fn idft_default_is_zero() {
        let dft = UnvoicedDft::default();
        for n in -128..128 {
            assert_eq!(dft.idft(n), 0.0, "default IDFT should be zero at n={n}");
        }
    }

    #[test]
    fn idft_single_bin() {
        // Place a known value in bin 1 and verify the IDFT produces cosine.
        let mut dft = UnvoicedDft::default();
        dft.bins[1] = Complex::new(128.0, 0.0);

        // IDFT at n should be: 2/256 * 128 * cos(2*pi*n/256) = cos(2*pi*n/256)
        for n in -128..128isize {
            let expected = (2.0 * PI * n as f32 / 256.0).cos();
            assert!(
                (dft.idft(n) - expected).abs() < 1e-3,
                "IDFT mismatch at n={n}: expected {expected}, got {}",
                dft.idft(n)
            );
        }
    }

    #[test]
    fn unvoiced_all_voiced_is_zero() {
        // When all harmonics are voiced, unvoiced signal should be zero.
        let params = BaseParams::new(32);
        let voice = VoiceDecisions::new(0b111111, &params); // all voiced

        let enhanced = EnhancedSpectrals::from_values(&[1.0; 16]);

        let rng = SmallRng::seed_from_u64(0);
        let dft = UnvoicedDft::new(&params, &voice, &enhanced, rng);

        let prev_dft = UnvoicedDft::default();
        let uv = Unvoiced::new(&dft, &prev_dft);

        for n in 0..SAMPLES_PER_FRAME {
            assert_eq!(uv.get(n), 0.0, "all-voiced should produce zero at n={n}");
        }
    }

    #[test]
    fn unvoiced_produces_nonzero_signal() {
        // Verify that unvoiced synthesis produces non-zero output.
        let params = BaseParams::new(42);
        let voice = VoiceDecisions::new(0, &params); // all unvoiced

        let enhanced = EnhancedSpectrals::from_values(&[
            10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 10.0,
            10.0, 10.0, 10.0, 10.0, 10.0,
        ]);

        let rng = SmallRng::seed_from_u64(42);
        let dft = UnvoicedDft::new(&params, &voice, &enhanced, rng);

        // Use same DFT for both current and previous (like reference test).
        let uv = Unvoiced::new(&dft, &dft);

        let has_nonzero = (0..SAMPLES_PER_FRAME).any(|n| uv.get(n).abs() > 0.01);
        assert!(has_nonzero, "unvoiced signal should be non-zero");
    }

    #[test]
    fn unvoiced_denominator_always_positive() {
        // Verify that the denominator w_s(n)^2 + w_s(n-N)^2 > 0 for all
        // valid sample indices.
        let w = window::synthesis();

        for n in 0..SAMPLES_PER_FRAME {
            let n = n as isize;
            let denom =
                w.get(n).powi(2) + w.get(n - SAMPLES_PER_FRAME as isize).powi(2);
            assert!(
                denom > 0.0,
                "denominator must be positive at n={n}, got {denom}"
            );
        }
    }
}

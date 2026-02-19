//! Spectral amplitude computation from DCT coefficients.
//!
//! Spectral amplitudes M_l (1 <= l <= L) measure the spectral envelope of
//! the voiced/unvoiced signal spectrum. They are computed from the DCT
//! coefficients T_l using inter-frame prediction with the previous frame's
//! spectral amplitudes, per TIA-102.BABA [p35].

use super::coefs::Coefficients;
use super::consts::MAX_HARMONICS;
use super::params::BaseParams;
use super::prev::PrevFrame;

/// Spectral amplitudes M_l, 1 <= l <= L.
///
/// Each amplitude measures the energy of a harmonic of the fundamental
/// frequency. Values are in the linear domain (not log).
#[derive(Clone)]
pub(crate) struct Spectrals {
    /// M_l values, indexed 0-based (element 0 = M_1).
    values: Vec<f32>,
}

impl Spectrals {
    /// Create new spectral amplitudes from the given DCT coefficients T_l
    /// and current/previous frame parameters.
    ///
    /// Uses inter-frame prediction: the previous frame's spectral amplitudes
    /// are interpolated and combined with the current DCT coefficients to
    /// produce smoothed spectral amplitudes.
    pub(crate) fn new(
        coefficients: &Coefficients,
        params: &BaseParams,
        previous: &PrevFrame,
    ) -> Self {
        // Compute L(-1) / L(0): ratio of previous to current harmonic count.
        let scale =
            previous.params.harmonic_count as f32 / params.harmonic_count as f32;

        // Compute (k_l, delta_l) for the given harmonic l [p35].
        // k_l is the integer part and delta_l is the fractional part of the
        // scaled harmonic index, used for interpolating previous spectrals.
        let interpolation_index = |l: u32| {
            let k = scale * l as f32;
            (k.trunc() as usize, k.fract())
        };

        // Compute prediction coefficient rho [p27].
        // Clamped to [0.4, 0.7] to prevent over- or under-prediction.
        let prediction = (0.03 * params.harmonic_count as f32 - 0.05).clamp(0.4, 0.7);

        // Compute the mean log-spectral term (sum over all harmonics).
        let mean_log_spectral: f32 = (1..=params.harmonic_count)
            .map(|l| {
                let (k, delta) = interpolation_index(l);
                (1.0 - delta) * previous.spectrals.get(k).log2()
                    + delta * previous.spectrals.get(k + 1).log2()
            })
            .sum::<f32>()
            / params.harmonic_count as f32;

        // Compute M_l for each harmonic l.
        let values = (1..=params.harmonic_count)
            .map(|l| {
                let (k, delta) = interpolation_index(l);

                let predicted = (1.0 - delta) * previous.spectrals.get(k).log2()
                    + delta * previous.spectrals.get(k + 1).log2()
                    - mean_log_spectral;

                (coefficients.get(l as usize) + prediction * predicted).exp2()
            })
            .collect();

        Self { values }
    }

    /// Retrieve the spectral amplitude M_l for the given harmonic index.
    ///
    /// Boundary behavior per TIA-102.BABA:
    /// - `l == 0`: returns 1.0 (Eq 78)
    /// - `l > L`: returns M_L, the last amplitude (Eq 79)
    /// - `1 <= l <= L`: returns M_l
    pub(crate) fn get(&self, l: usize) -> f32 {
        if l == 0 {
            // Handles case of Eq 78.
            1.0
        } else if l > self.values.len() {
            // Handle case of Eq 79: clamp to last element.
            // Safety: default Spectrals has MAX_HARMONICS elements, and
            // constructed Spectrals always has at least MIN_HARMONICS (9).
            // So values is never empty.
            self.values[self.values.len() - 1]
        } else {
            self.values[l - 1]
        }
    }

    /// Return the number of spectral amplitudes (equal to L, the harmonic count).
    pub(crate) fn len(&self) -> usize {
        self.values.len()
    }
}

impl Default for Spectrals {
    /// Construct the default set of spectral amplitudes.
    ///
    /// By default, M_l = 1.0 for all harmonics [p35]. Uses MAX_HARMONICS
    /// entries so that interpolation lookups for any valid L never go out
    /// of bounds.
    fn default() -> Self {
        Self {
            values: vec![1.0; MAX_HARMONICS],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vocoder::coefs::Coefficients;
    use crate::vocoder::descramble::{descramble, Bootstrap};
    use crate::vocoder::gain::Gains;

    #[test]
    fn default_spectrals_are_all_ones() {
        let s = Spectrals::default();
        assert_eq!(s.len(), MAX_HARMONICS);
        for l in 1..=MAX_HARMONICS {
            assert!((s.get(l) - 1.0).abs() < 1e-10);
        }
    }

    #[test]
    fn get_boundary_l_zero_returns_one() {
        let s = Spectrals::default();
        assert!((s.get(0) - 1.0).abs() < 1e-10);
    }

    #[test]
    fn get_boundary_l_beyond_length_returns_last() {
        let mut s = Spectrals::default();
        s.values[MAX_HARMONICS - 1] = 42.0;
        assert!((s.get(MAX_HARMONICS + 1) - 42.0).abs() < 1e-10);
        assert!((s.get(MAX_HARMONICS + 100) - 42.0).abs() < 1e-10);
    }

    #[test]
    fn spectrals_from_default_previous_frame() {
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
        let (amplitudes, _, gain_index) = descramble(&chunks, &params).unwrap();
        let gains = Gains::new(gain_index, &amplitudes, &params);
        let coefficients = Coefficients::new(&gains, &amplitudes, &params);
        let previous = PrevFrame::default();
        let spectrals = Spectrals::new(&coefficients, &params, &previous);

        assert_eq!(spectrals.len(), 16);

        // l=0 boundary
        assert!((spectrals.get(0) - 1.0).abs() < 1e-6);

        // M_1 through M_16
        assert!((spectrals.get(1) - 0.5306769781475001).abs() < 1e-6);
        assert!((spectrals.get(2) - 0.3535007196676076).abs() < 1e-6);
        assert!((spectrals.get(3) - 0.9173875577243951).abs() < 1e-6);
        assert!((spectrals.get(4) - 0.13169278935782622).abs() < 1e-6);
        assert!((spectrals.get(5) - 4.438599873836148).abs() < 1e-5);
        assert!((spectrals.get(6) - 0.6796441620283217).abs() < 1e-6);
        assert!((spectrals.get(7) - 0.9439604687782126).abs() < 1e-6);
        assert!((spectrals.get(8) - 10.646341109175768).abs() < 1e-6);
        assert!((spectrals.get(9) - 1.3058035207362984).abs() < 1e-6);
        assert!((spectrals.get(10) - 1.566168983101457).abs() < 1e-6);
        assert!((spectrals.get(11) - 8.325591838823936).abs() < 1e-5);
        assert!((spectrals.get(12) - 1.5943520720633444).abs() < 1e-6);
        assert!((spectrals.get(13) - 1.556318849691248).abs() < 1e-6);
        assert!((spectrals.get(14) - 15.882295806333524).abs() < 1e-5);
        assert!((spectrals.get(15) - 11.386537444591385).abs() < 1e-5);
        assert!((spectrals.get(16) - 12.498792573794846).abs() < 1e-5);

        // l > L boundary: should return M_L (last element)
        assert!((spectrals.get(17) - 12.498792573794846).abs() < 1e-5);
        assert!((spectrals.get(30) - 12.498792573794846).abs() < 1e-5);
        assert!((spectrals.get(56) - 12.498792573794846).abs() < 1e-5);
    }

    #[test]
    fn spectrals_with_previous_frame_prediction() {
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
        let (amplitudes, _, gain_index) = descramble(&chunks, &params).unwrap();
        let gains = Gains::new(gain_index, &amplitudes, &params);
        let coefficients = Coefficients::new(&gains, &amplitudes, &params);

        // First frame: default previous
        let mut previous = PrevFrame::default();
        let first = Spectrals::new(&coefficients, &params, &previous);

        // Second frame: use first frame's spectrals as previous.
        // Note: previous.params stays as default (L=30), matching the
        // reference test which only updates spectrals, not params.
        previous.spectrals = first;
        let second = Spectrals::new(&coefficients, &params, &previous);

        assert!((second.get(1) - 0.18026009141598204).abs() < 1e-6);
        assert!((second.get(2) - 0.09466759399487303).abs() < 1e-6);
        assert!((second.get(3) - 0.546543586730317).abs() < 1e-6);
        assert!((second.get(4) - 0.11240949805284323).abs() < 1e-6);
        assert!((second.get(5) - 2.664303862945382).abs() < 1e-6);
        assert!((second.get(6) - 0.7356498197494585).abs() < 1e-6);
        assert!((second.get(7) - 0.6722918529743475).abs() < 1e-6);
        assert!((second.get(8) - 15.748045510026174).abs() < 1e-6);
        assert!((second.get(9) - 2.010522620838216).abs() < 1e-6);
        assert!((second.get(10) - 2.411402725277654).abs() < 1e-6);
        assert!((second.get(11) - 12.818766727159018).abs() < 1e-5);
        assert!((second.get(12) - 2.4547957296486485).abs() < 1e-6);
        assert!((second.get(13) - 2.396236648815911).abs() < 1e-6);
        assert!((second.get(14) - 24.45369037714974).abs() < 1e-4);
        assert!((second.get(15) - 17.53165062111628).abs() < 1e-4);
        assert!((second.get(16) - 19.244170201509185).abs() < 1e-4);
    }
}

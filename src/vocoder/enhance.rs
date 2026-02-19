//! Frame error handling, spectral amplitude enhancement, and adaptive smoothing.
//!
//! This module performs three functions:
//! 1. Error aggregation and frame repeat/mute decisions (EnhanceErrors)
//! 2. Energy computation and tracking (FrameEnergy)
//! 3. Spectral reshaping with energy preservation (EnhancedSpectrals)
//! 4. Adaptive smoothing of voice decisions and amplitudes (smooth)

use std::f32::consts::PI;

use super::descramble::VoiceDecisions;
use super::frame::Errors;
use super::params::BaseParams;
use super::spectral::Spectrals;

/// Values derived from error correction decoding.
///
/// Aggregates FEC error counts from the current frame for use in
/// frame repeat/mute decisions and adaptive smoothing.
pub(crate) struct EnhanceErrors {
    /// Total number of errors corrected in the current frame, epsilon_T [p45].
    pub(crate) total: usize,
    /// Error rate tracking term, epsilon_R [p45].
    pub(crate) rate: f32,
    /// Errors corrected in first Golay-coded chunk u_0, epsilon_0.
    pub(crate) golay_initial: usize,
    /// Errors corrected in first Hamming-coded chunk u_4, epsilon_4.
    pub(crate) hamming_initial: usize,
}

impl EnhanceErrors {
    /// Create a new `EnhanceErrors` from the errors corrected in the current
    /// frame and the previous frame's error rate.
    pub(crate) fn new(errors: &Errors, previous_rate: f32) -> Self {
        let total: usize = errors.iter().sum();

        Self {
            total,
            // Compute Eq 96: EWMA error rate.
            rate: 0.95 * previous_rate + 0.000365 * total as f32,
            golay_initial: errors[0],
            hamming_initial: errors[4],
        }
    }
}

/// Energy-related parameters for a voice frame.
///
/// Tracks spectral amplitude energy R_M0, cosine-weighted energy R_M1,
/// and a moving average energy tracker S_E.
pub(crate) struct FrameEnergy {
    /// Spectral amplitude energy, R_M0 (sum of M_l^2).
    pub(crate) energy: f32,
    /// Cosine-weighted energy value, R_M1.
    pub(crate) scaled: f32,
    /// Moving average energy tracker, S_E.
    pub(crate) tracking: f32,
}

impl FrameEnergy {
    /// Create a new `FrameEnergy` from the given spectral amplitudes M_l,
    /// previous frame energy values, and current frame parameters.
    pub(crate) fn new(
        spectrals: &Spectrals,
        previous: &FrameEnergy,
        params: &BaseParams,
    ) -> Self {
        // Compute energy of spectral amplitudes according to Eq 105.
        let energy: f32 = spectrals.iter().map(|&m| m.powi(2)).sum();

        // Compute cosine-weighted energy according to Eq 106.
        let scaled: f32 = spectrals
            .iter()
            .enumerate()
            .map(|(l, &m)| {
                m.powi(2) * (params.fundamental_frequency * (l + 1) as f32).cos()
            })
            .sum();

        Self {
            energy,
            scaled,
            // Compute energy tracking EWMA according to Eq 111.
            // Floor of 10000.0 prevents tracking from decaying to zero.
            tracking: (0.95 * previous.tracking + 0.05 * energy).max(10000.0),
        }
    }
}

impl Default for FrameEnergy {
    /// Create a new `FrameEnergy` with default initial values per [p64].
    fn default() -> Self {
        Self {
            energy: 0.0,
            scaled: 0.0,
            tracking: 75000.0,
        }
    }
}

/// Enhanced spectral amplitudes (overbar M_l) derived from the decoded
/// spectral amplitudes (tilde M_l) via spectral reshaping.
///
/// Enhancement applies a weight derived from the spectral envelope shape,
/// clamped to [0.5, 1.2], then rescales to preserve total energy.
#[derive(Clone)]
pub(crate) struct EnhancedSpectrals {
    /// Enhanced amplitude values, indexed 0-based (element 0 = M_1).
    values: Vec<f32>,
}

impl EnhancedSpectrals {
    /// Create new `EnhancedSpectrals` from the given base spectral amplitudes
    /// M_l, current frame energy values, and parameters.
    pub(crate) fn new(
        spectrals: &Spectrals,
        frame_energy: &FrameEnergy,
        params: &BaseParams,
    ) -> Self {
        // Compute R_M0^2.
        let energy_squared = frame_energy.energy.powi(2);
        // Compute R_M1^2.
        let scaled_squared = frame_energy.scaled.powi(2);
        // Compute denominator term of Eq 107.
        let denominator = params.fundamental_frequency
            * frame_energy.energy
            * (energy_squared - scaled_squared);

        let mut values: Vec<f32> = spectrals
            .iter()
            .enumerate()
            .map(|(l, &m)| {
                let l = l + 1;

                // Handle fast-path case in Eq 108: lower harmonics pass through.
                if 8 * l as u32 <= params.harmonic_count {
                    return m;
                }

                // Compute Eq 107: enhancement weight.
                let weight = m.sqrt()
                    * (0.96
                        * PI
                        * (energy_squared + scaled_squared
                            - 2.0
                                * frame_energy.energy
                                * frame_energy.scaled
                                * (params.fundamental_frequency * l as f32).cos())
                        / denominator)
                        .powf(0.25);

                // Scale current spectral amplitude according to Eq 108.
                // Weight clamped to [0.5, 1.2] to prevent extreme reshaping.
                m * weight.clamp(0.5, 1.2)
            })
            .collect();

        // Compute root ratio of energies according to Eq 109.
        let enhanced_energy: f32 = values.iter().map(|&m| m.powi(2)).sum();
        let scale = (frame_energy.energy / enhanced_energy).sqrt();

        // Perform second scaling pass according to Eq 110:
        // rescale to preserve original total energy.
        for value in &mut values {
            *value *= scale;
        }

        Self { values }
    }

    /// Retrieve the enhanced spectral amplitude M_l, 1 <= l <= L.
    ///
    /// Returns 0.0 for out-of-bounds indices [p60], unlike `Spectrals::get()`
    /// which clamps to the last element.
    pub(crate) fn get(&self, l: usize) -> f32 {
        debug_assert!(l >= 1, "enhanced spectral index must be >= 1, got {l}");

        if l > self.values.len() {
            0.0
        } else {
            self.values[l - 1]
        }
    }

    /// Return the number of enhanced spectral amplitudes.
    pub(crate) fn len(&self) -> usize {
        self.values.len()
    }

    /// Iterate over enhanced spectral amplitudes.
    pub(crate) fn iter(&self) -> std::slice::Iter<'_, f32> {
        self.values.iter()
    }

    /// Mutably iterate over enhanced spectral amplitudes.
    fn iter_mut(&mut self) -> std::slice::IterMut<'_, f32> {
        self.values.iter_mut()
    }

    /// Create `EnhancedSpectrals` from a raw slice of amplitude values.
    #[cfg(test)]
    pub(crate) fn from_values(values: &[f32]) -> Self {
        Self {
            values: values.to_vec(),
        }
    }
}

impl Default for EnhancedSpectrals {
    /// Create new `EnhancedSpectrals` with default initial values.
    ///
    /// By default, all enhanced amplitudes are 0 [p64] (empty vector).
    fn default() -> Self {
        Self { values: Vec::new() }
    }
}

/// Compute the spectral amplitude threshold tau_M used in adaptive smoothing
/// from the given error characteristics and previous amplitude threshold.
pub(crate) fn amplitude_threshold(errors: &EnhanceErrors, previous: f32) -> f32 {
    // Compute Eq 115.
    if errors.rate <= 0.005 && errors.total <= 6 {
        20480.0
    } else {
        6000.0 - 300.0 * errors.total as f32 + previous
    }
}

/// Smooth the given enhanced spectral amplitudes M_l and voiced/unvoiced
/// decisions v_l based on error characteristics, current frame energy,
/// and spectral amplitude threshold tau_M for the current frame.
///
/// This function mutates both `enhanced` and `voiced`:
/// 1. First updates voice decisions (Eq 113): harmonics with amplitudes
///    exceeding the threshold are forced voiced.
/// 2. Then scales amplitudes (Eq 114-116): if total amplitude exceeds
///    tau_M, all amplitudes are scaled down proportionally.
pub(crate) fn smooth(
    enhanced: &mut EnhancedSpectrals,
    voiced: &mut VoiceDecisions,
    errors: &EnhanceErrors,
    frame_energy: &FrameEnergy,
    threshold: f32,
) {
    // Compute voicing threshold according to Eq 112.
    let voicing_threshold = if errors.rate <= 0.005 && errors.total <= 4 {
        f32::MAX
    } else if errors.rate <= 0.0125 && errors.hamming_initial == 0 {
        45.255 * frame_energy.tracking.powf(0.375) / (277.26 * errors.rate).exp()
    } else {
        1.414 * frame_energy.tracking.powf(0.375)
    };

    // Update voiced/unvoiced decisions according to Eq 113.
    // Must happen BEFORE amplitude scaling.
    for (l, &m) in enhanced.iter().enumerate() {
        if m > voicing_threshold {
            voiced.force_voiced(l + 1);
        }
    }

    // Compute amplitude sum in Eq 114.
    let amplitude_sum: f32 = enhanced.iter().sum();
    // Compute scale factor in Eq 116.
    let scale = (threshold / amplitude_sum).min(1.0);

    // Scale each enhanced M_l [p50].
    for value in enhanced.iter_mut() {
        *value *= scale;
    }
}

/// Check whether the current frame should be discarded and the previous
/// repeated based on the given error characteristics of the current frame.
pub(crate) fn should_repeat(errors: &EnhanceErrors) -> bool {
    // Check the conditions in Eqs 97 and 98.
    errors.golay_initial >= 2 && errors.total as f32 >= 10.0 + 40.0 * errors.rate
}

/// Check if the current frame should be discarded and replaced with
/// silence/comfort noise based on the given error characteristics.
pub(crate) fn should_mute(errors: &EnhanceErrors) -> bool {
    // Check the condition on [p47].
    errors.rate > 0.0875
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vocoder::coefs::Coefficients;
    use crate::vocoder::descramble::{descramble, Bootstrap};
    use crate::vocoder::gain::Gains;
    use crate::vocoder::prev::PrevFrame;

    #[test]
    fn enhance_errors_computation() {
        let errors = EnhanceErrors::new(&[1, 2, 3, 4, 5, 6, 7], 0.5);

        assert_eq!(errors.total, 28);
        assert!((errors.rate - 0.48522).abs() < 1e-5);
        assert_eq!(errors.golay_initial, 1);
        assert_eq!(errors.hamming_initial, 5);
    }

    #[test]
    fn should_repeat_conditions() {
        // golay_initial < 2: no repeat regardless of total
        let errors = EnhanceErrors {
            total: 100,
            rate: 0.0,
            golay_initial: 1,
            hamming_initial: 0,
        };
        assert!(!should_repeat(&errors));

        // golay_initial >= 2, total >= 10 + 40*rate: repeat
        let errors = EnhanceErrors {
            total: 12,
            rate: 0.0,
            golay_initial: 2,
            hamming_initial: 0,
        };
        assert!(should_repeat(&errors));

        // golay_initial >= 2, total < 10 + 40*rate: no repeat
        let errors = EnhanceErrors {
            total: 9,
            rate: 0.0,
            golay_initial: 2,
            hamming_initial: 0,
        };
        assert!(!should_repeat(&errors));
    }

    #[test]
    fn should_mute_threshold() {
        let errors = EnhanceErrors {
            total: 0,
            rate: 0.0875,
            golay_initial: 0,
            hamming_initial: 0,
        };
        assert!(!should_mute(&errors));

        let errors = EnhanceErrors {
            total: 0,
            rate: 0.0876,
            golay_initial: 0,
            hamming_initial: 0,
        };
        assert!(should_mute(&errors));
    }

    /// Helper: build spectrals from the standard L=16 test chunks.
    fn build_spectrals() -> (Spectrals, BaseParams) {
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

        (spectrals, params)
    }

    #[test]
    fn frame_energy_computation() {
        let (spectrals, params) = build_spectrals();

        let previous_energy = FrameEnergy::default();
        let energy = FrameEnergy::new(&spectrals, &previous_energy, &params);

        assert!((energy.energy - 752.2228257467827461).abs() < 1e-4);
        assert!((energy.scaled - -452.3599305372189292).abs() < 1e-4);
        assert!((energy.tracking - 71287.6111412873433437).abs() < 1e-4);

        // Second iteration: tracking decays further.
        let energy2 = FrameEnergy::new(&spectrals, &energy, &params);

        assert!((energy2.energy - 752.2228257467827461).abs() < 1e-4);
        assert!((energy2.scaled - -452.3599305372189292).abs() < 1e-4);
        assert!((energy2.tracking - 67760.8417255103122443).abs() < 0.1);
    }

    #[test]
    fn frame_energy_default_values() {
        let default = FrameEnergy::default();
        assert!((default.energy - 0.0).abs() < 1e-10);
        assert!((default.scaled - 0.0).abs() < 1e-10);
        assert!((default.tracking - 75000.0).abs() < 1e-10);
    }

    #[test]
    fn enhanced_spectrals_computation() {
        let (spectrals, params) = build_spectrals();
        let previous_energy = FrameEnergy::default();
        let energy = FrameEnergy::new(&spectrals, &previous_energy, &params);
        let enhanced = EnhancedSpectrals::new(&spectrals, &energy, &params);

        assert_eq!(enhanced.len(), 16);

        assert!((enhanced.get(1) - 0.46407439358059288103675044112606).abs() < 1e-6);
        assert!((enhanced.get(2) - 0.30913461049404472591461967567739).abs() < 1e-6);
        assert!((enhanced.get(3) - 0.41587411398142343221806527253648).abs() < 1e-6);
        assert!((enhanced.get(4) - 0.05758233878573310732251755439393).abs() < 1e-6);
        assert!((enhanced.get(5) - 4.29488628936570737693045884952880).abs() < 1e-6);
        assert!((enhanced.get(6) - 0.29717281253740035484867121340358).abs() < 1e-6);
        assert!((enhanced.get(7) - 0.41274456522882696507537048091763).abs() < 1e-6);
        assert!((enhanced.get(8) - 11.17220652652428292128661269089207).abs() < 1e-6);
        assert!((enhanced.get(9) - 0.61137412496551313267900695791468).abs() < 1e-6);
        assert!((enhanced.get(10) - 0.76976839668918539683062363110366).abs() < 1e-6);
        assert!((enhanced.get(11) - 8.73682419143608512968057766556740).abs() < 1e-6);
        assert!((enhanced.get(12) - 0.71114975707359529000228803852224).abs() < 1e-6);
        assert!((enhanced.get(13) - 0.68049652809620897464526478870539).abs() < 1e-6);
        assert!((enhanced.get(14) - 16.66679545607086865288692933972925).abs() < 1e-6);
        assert!((enhanced.get(15) - 10.89394258052602992847823770716786).abs() < 1e-6);
        assert!((enhanced.get(16) - 11.55358738738138946189337730174884).abs() < 1e-6);

        // Out-of-bounds returns 0.0, not clamped.
        assert_eq!(enhanced.get(17), 0.0);
        assert_eq!(enhanced.get(30), 0.0);
        assert_eq!(enhanced.get(56), 0.0);
    }

    #[test]
    fn enhanced_spectrals_default_is_empty() {
        let default = EnhancedSpectrals::default();
        assert_eq!(default.len(), 0);
    }

    #[test]
    fn smooth_updates_voicing_and_scales_amplitudes() {
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
        let (amplitudes, mut voice, gain_index) = descramble(&chunks, &params).unwrap();
        let gains = Gains::new(gain_index, &amplitudes, &params);
        let coefficients = Coefficients::new(&gains, &amplitudes, &params);
        let previous = PrevFrame::default();
        let spectrals = Spectrals::new(&coefficients, &params, &previous);
        let previous_energy = FrameEnergy::default();
        let mut energy = FrameEnergy::new(&spectrals, &previous_energy, &params);
        let mut enhanced = EnhancedSpectrals::new(&spectrals, &energy, &params);

        let errors = EnhanceErrors {
            total: 0,
            rate: 0.0250,
            golay_initial: 0,
            hamming_initial: 0,
        };

        // Dummy value just to get a good adaptive threshold.
        energy.tracking = 100.0;

        // Verify initial voicing decisions.
        assert!(voice.is_voiced(1));
        assert!(voice.is_voiced(2));
        assert!(voice.is_voiced(3));
        assert!(!voice.is_voiced(4));
        assert!(!voice.is_voiced(5));
        assert!(!voice.is_voiced(6));
        assert!(voice.is_voiced(7));
        assert!(voice.is_voiced(8));
        assert!(voice.is_voiced(9));
        assert!(!voice.is_voiced(10));
        assert!(!voice.is_voiced(11));
        assert!(!voice.is_voiced(12));
        assert!(!voice.is_voiced(13));
        assert!(!voice.is_voiced(14));
        assert!(!voice.is_voiced(15));
        assert!(voice.is_voiced(16));

        smooth(&mut enhanced, &mut voice, &errors, &energy, 42.0);

        // Verify updated voicing: harmonics 11, 14, 15 forced voiced.
        assert!(voice.is_voiced(1));
        assert!(voice.is_voiced(2));
        assert!(voice.is_voiced(3));
        assert!(!voice.is_voiced(4));
        assert!(!voice.is_voiced(5));
        assert!(!voice.is_voiced(6));
        assert!(voice.is_voiced(7));
        assert!(voice.is_voiced(8));
        assert!(voice.is_voiced(9));
        assert!(!voice.is_voiced(10));
        assert!(voice.is_voiced(11));
        assert!(!voice.is_voiced(12));
        assert!(!voice.is_voiced(13));
        assert!(voice.is_voiced(14));
        assert!(voice.is_voiced(15));
        assert!(voice.is_voiced(16));

        // Verify scaled amplitudes.
        assert!((enhanced.get(1) - 0.28643362145733153312221475061961).abs() < 1e-6);
        assert!((enhanced.get(2) - 0.19080248172803679351794414742471).abs() < 1e-6);
        assert!((enhanced.get(3) - 0.25668369163611542971281664904382).abs() < 1e-6);
        assert!((enhanced.get(4) - 0.03554067636251981299189139917871).abs() < 1e-6);
        assert!((enhanced.get(5) - 2.65086772859580133143708735588007).abs() < 1e-6);
        assert!((enhanced.get(6) - 0.18341948202958965885578379584331).abs() < 1e-6);
        assert!((enhanced.get(7) - 0.25475208757622064270620398929168).abs() < 1e-6);
        assert!((enhanced.get(8) - 6.89565211812498901622348057571799).abs() < 1e-6);
        assert!((enhanced.get(9) - 0.37734920758726891998335872813186).abs() < 1e-6);
        assert!((enhanced.get(10) - 0.47511250910851310358395949151600).abs() < 1e-6);
        assert!((enhanced.get(11) - 5.39249790077991697501147427828982).abs() < 1e-6);
        assert!((enhanced.get(12) - 0.43893221245295155341636927914806).abs() < 1e-6);
        assert!((enhanced.get(13) - 0.42001258338742591957881700182043).abs() < 1e-6);
        assert!((enhanced.get(14) - 10.28699416862334992117666843114421).abs() < 1e-6);
        assert!((enhanced.get(15) - 6.72390346990002374383266214863397).abs() < 1e-6);
        assert!((enhanced.get(16) - 7.13104606064994861469585885060951).abs() < 1e-4);

        // Out-of-bounds still returns 0.0 after smoothing.
        assert_eq!(enhanced.get(17), 0.0);
        assert_eq!(enhanced.get(30), 0.0);
        assert_eq!(enhanced.get(56), 0.0);
    }

    #[test]
    fn amplitude_threshold_low_error() {
        let errors = EnhanceErrors {
            total: 4,
            rate: 0.003,
            golay_initial: 0,
            hamming_initial: 0,
        };
        assert!((amplitude_threshold(&errors, 0.0) - 20480.0).abs() < 1e-10);
    }

    #[test]
    fn amplitude_threshold_high_error() {
        let errors = EnhanceErrors {
            total: 10,
            rate: 0.01,
            golay_initial: 0,
            hamming_initial: 0,
        };
        // 6000 - 300*10 + 100.0 = 3100.0
        assert!((amplitude_threshold(&errors, 100.0) - 3100.0).abs() < 1e-10);
    }
}

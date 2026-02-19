//! Quantized gain vector for IMBE spectral amplitude reconstruction.
//!
//! The gain vector G_m (1 <= m <= 6) provides a coarse spectral envelope for
//! each voice frame. G_1 is looked up from the 6-bit gain index b_2, and
//! G_2..G_6 are computed from the quantized amplitudes and step sizes.
//! The inverse DCT of the gain vector produces the log spectral amplitudes
//! used in later synthesis stages.

use std::f32::consts::PI;

use super::allocs::allocations;
use super::descramble::QuantizedAmplitudes;
use super::params::BaseParams;

/// Number of gain values in the coarse spectral envelope.
const NUM_GAINS: usize = 6;

/// The gain vector G_m, 1 <= m <= 6, giving a coarse spectral envelope
/// for the voice frame.
#[derive(Debug, Copy, Clone)]
pub struct Gains([f32; NUM_GAINS]);

impl Gains {
    /// Create a new `Gains` vector from the given gain index b_2,
    /// quantized amplitudes b_m (3 <= m <= L+1), and frame parameters.
    ///
    /// G_1 is looked up directly from the gain table using `gain_index`.
    /// G_2 through G_6 are computed from the quantized amplitudes, bit
    /// allocations, and step sizes per TIA-102.BABA.
    pub fn new(
        gain_index: usize,
        amplitudes: &QuantizedAmplitudes,
        params: &BaseParams,
    ) -> Self {
        let mut gains = [0.0f32; NUM_GAINS];

        let (allocation, _) = allocations(params.harmonic_count);
        let steps = &STEPS[params.harmonic_count as usize - 9];

        // G_1 is found from the b_2 index.
        gains[0] = GAIN_TABLE[gain_index];

        // Compute G_2 through G_6 from quantized amplitudes b_3..b_7.
        for m in 3..=7 {
            let bits = allocation[m - 3];
            let step = steps[m - 3];

            gains[m - 2] = if bits == 0 {
                0.0
            } else {
                step * (amplitudes.get(m) as f32 - (1u32 << (bits - 1)) as f32 + 0.5)
            };
        }

        Self(gains)
    }

    /// Compute the inverse DCT value R_i, 1 <= i <= 6.
    ///
    /// This transforms the gain vector from the frequency domain back to
    /// the log spectral amplitude domain per TIA-102.BABA.
    ///
    /// # Panics
    ///
    /// Panics in debug builds if `i` is outside the valid range 1..=6.
    pub fn inverse_dct(&self, i: usize) -> f32 {
        debug_assert!(
            (1..=NUM_GAINS).contains(&i),
            "inverse_dct index {i} out of range 1..=6"
        );

        self.0[0]
            + 2.0
                * (2..=6).fold(0.0f32, |sum, m| {
                    sum + self.0[m - 1]
                        * (PI * (m as f32 - 1.0) * (i as f32 - 0.5) / 6.0).cos()
                })
    }
}

/// Step sizes indexed by `[L - 9][m - 3]` for harmonics parameter L and
/// amplitude index m (3 <= m <= 7).
///
/// Each row corresponds to a harmonic count L (9..=56). Each column gives
/// the step size delta_m used to reconstruct gain values G_2..G_6 from the
/// quantized amplitudes.
static STEPS: [[f32; 5]; 48] = [
    [0.003100, 0.004020, 0.003360, 0.002900, 0.002640],
    [0.006200, 0.004020, 0.006720, 0.005800, 0.005280],
    [0.012400, 0.008040, 0.006720, 0.011600, 0.010560],
    [0.012400, 0.016080, 0.013440, 0.011600, 0.010560],
    [0.024800, 0.016080, 0.013440, 0.021750, 0.019800],
    [0.024800, 0.030150, 0.025200, 0.021750, 0.019800],
    [0.024800, 0.030150, 0.025200, 0.021750, 0.036960],
    [0.046500, 0.030150, 0.025200, 0.040600, 0.036960],
    [0.046500, 0.030150, 0.047040, 0.040600, 0.036960],
    [0.046500, 0.056280, 0.047040, 0.040600, 0.036960],
    [0.046500, 0.056280, 0.047040, 0.058000, 0.052800],
    [0.046500, 0.056280, 0.047040, 0.058000, 0.052800],
    [0.086800, 0.056280, 0.047040, 0.058000, 0.052800],
    [0.086800, 0.056280, 0.067200, 0.058000, 0.052800],
    [0.086800, 0.080400, 0.067200, 0.058000, 0.052800],
    [0.086800, 0.080400, 0.067200, 0.058000, 0.052800],
    [0.086800, 0.080400, 0.067200, 0.058000, 0.085800],
    [0.086800, 0.080400, 0.067200, 0.094250, 0.085800],
    [0.086800, 0.080400, 0.067200, 0.094250, 0.085800],
    [0.124000, 0.080400, 0.067200, 0.094250, 0.085800],
    [0.124000, 0.080400, 0.067200, 0.094250, 0.085800],
    [0.124000, 0.080400, 0.067200, 0.094250, 0.085800],
    [0.124000, 0.080400, 0.109200, 0.094250, 0.085800],
    [0.124000, 0.080400, 0.109200, 0.094250, 0.085800],
    [0.124000, 0.130650, 0.109200, 0.094250, 0.085800],
    [0.124000, 0.130650, 0.109200, 0.094250, 0.085800],
    [0.124000, 0.130650, 0.109200, 0.094250, 0.085800],
    [0.124000, 0.130650, 0.109200, 0.094250, 0.085800],
    [0.124000, 0.130650, 0.109200, 0.094250, 0.112200],
    [0.124000, 0.130650, 0.109200, 0.094250, 0.112200],
    [0.124000, 0.130650, 0.109200, 0.094250, 0.112200],
    [0.124000, 0.130650, 0.109200, 0.094250, 0.112200],
    [0.124000, 0.130650, 0.109200, 0.123250, 0.112200],
    [0.124000, 0.130650, 0.109200, 0.123250, 0.112200],
    [0.124000, 0.130650, 0.109200, 0.123250, 0.112200],
    [0.124000, 0.130650, 0.109200, 0.123250, 0.112200],
    [0.124000, 0.130650, 0.109200, 0.123250, 0.112200],
    [0.201500, 0.130650, 0.109200, 0.123250, 0.112200],
    [0.201500, 0.130650, 0.109200, 0.123250, 0.112200],
    [0.201500, 0.130650, 0.109200, 0.123250, 0.112200],
    [0.201500, 0.130650, 0.109200, 0.123250, 0.112200],
    [0.201500, 0.130650, 0.109200, 0.123250, 0.112200],
    [0.201500, 0.130650, 0.109200, 0.123250, 0.112200],
    [0.201500, 0.130650, 0.142800, 0.123250, 0.112200],
    [0.201500, 0.130650, 0.142800, 0.123250, 0.112200],
    [0.201500, 0.130650, 0.142800, 0.123250, 0.112200],
    [0.201500, 0.130650, 0.142800, 0.123250, 0.112200],
    [0.201500, 0.130650, 0.142800, 0.123250, 0.112200],
];

/// Gain lookup table indexed by the 6-bit gain index b_2.
///
/// Each entry gives the first gain value G_1 for the corresponding index.
/// These values are from TIA-102.BABA.
const GAIN_TABLE: [f32; 64] = [
    -2.842205, -2.694235, -2.55826, -2.38285, -2.221042, -2.095574, -1.980845, -1.836058,
    -1.645556, -1.417658, -1.261301, -1.125631, -0.958207, -0.781591, -0.555837, -0.346976,
    -0.147249, 0.027755, 0.211495, 0.388380, 0.552873, 0.737223, 0.932197, 1.139032,
    1.320955, 1.483433, 1.648297, 1.801447, 1.942731, 2.118613, 2.321486, 2.504443,
    2.653909, 2.780654, 2.925355, 3.07639, 3.220825, 3.402869, 3.585096, 3.784606,
    3.955521, 4.155636, 4.314009, 4.44415, 4.577542, 4.735552, 4.909493, 5.085264,
    5.254767, 5.411894, 5.568094, 5.738523, 5.919215, 6.087701, 6.280685, 6.464201,
    6.647736, 6.834672, 7.022583, 7.211777, 7.471016, 7.738948, 8.124863, 8.69582,
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vocoder::descramble::{descramble, Bootstrap};

    #[test]
    fn gains_with_9_harmonics() {
        let chunks = [
            0b000000010010,
            0b110011001100,
            0b111000111000,
            0b111111111111,
            0b11010110101,
            0b00101111010,
            0b01110111011,
            0b00001000,
        ];

        let bootstrap = Bootstrap::new(&chunks);
        let params = BaseParams::new(bootstrap.unwrap_period());
        let (amplitudes, _, gain) = descramble(&chunks, &params).unwrap();

        assert_eq!(params.harmonic_count, 9);
        assert_eq!(gain, 21);

        let g = Gains::new(gain, &amplitudes, &params);

        assert!((g.0[0] - 0.737223).abs() < 1e-6);
        assert!((g.0[1] - -0.21235).abs() < 1e-5);
        assert!((g.0[2] - -0.01005).abs() < 1e-5);
        assert!((g.0[3] - 0.29736).abs() < 1e-5);
        assert!((g.0[4] - 0.25375).abs() < 1e-5);
        assert!((g.0[5] - -0.25476).abs() < 1e-5);

        assert!((f64::from(g.inverse_dct(1)) - 0.8519942560055926_f64).abs() < 1e-6);
        assert!((f64::from(g.inverse_dct(2)) - -0.13083074772702047_f64).abs() < 1e-6);
        assert!((f64::from(g.inverse_dct(3)) - -0.014229409757043066_f64).abs() < 1e-6);
        assert!((f64::from(g.inverse_dct(4)) - 2.0309896309891773_f64).abs() < 1e-6);
        assert!((f64::from(g.inverse_dct(5)) - 0.5902767477270205_f64).abs() < 1e-6);
        assert!((f64::from(g.inverse_dct(6)) - 1.0951375227622724_f64).abs() < 1e-6);
    }

    #[test]
    fn gains_with_16_harmonics() {
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
        let (amplitudes, _, gain) = descramble(&chunks, &params).unwrap();

        assert_eq!(params.harmonic_count, 16);
        assert_eq!(params.band_count, 6);
        assert_eq!(gain, 21);

        let g = Gains::new(gain, &amplitudes, &params);

        assert!((g.0[0] - 0.737223).abs() < 1e-6);
        assert!((g.0[1] - -1.18575).abs() < 1e-5);
        assert!((g.0[2] - 0.075375).abs() < 1e-6);
        assert!((g.0[3] - -0.1134).abs() < 1e-4);
        assert!((g.0[4] - 0.3857).abs() < 1e-4);
        assert!((g.0[5] - -0.01848).abs() < 1e-5);

        assert!((f64::from(g.inverse_dct(1)) - -1.2071545373041197_f64).abs() < 1e-6);
        assert!((f64::from(g.inverse_dct(2)) - -1.524574246978134_f64).abs() < 1e-6);
        assert!((f64::from(g.inverse_dct(3)) - 0.5032515043523329_f64).abs() < 1e-6);
        assert!((f64::from(g.inverse_dct(4)) - 1.481487836406659_f64).abs() < 1e-6);
        assert!((f64::from(g.inverse_dct(5)) - 1.456220246978134_f64).abs() < 1e-6);
        assert!((f64::from(g.inverse_dct(6)) - 3.7141071965451276_f64).abs() < 1e-6);
    }

    #[test]
    fn gain_table_length() {
        assert_eq!(GAIN_TABLE.len(), 64);
    }

    #[test]
    fn steps_table_dimensions() {
        assert_eq!(STEPS.len(), 48);
        for row in &STEPS {
            assert_eq!(row.len(), 5);
        }
    }

    #[test]
    fn gain_table_monotonic() {
        // The gain table should be monotonically increasing.
        for window in GAIN_TABLE.windows(2) {
            assert!(
                window[1] > window[0],
                "GAIN_TABLE not monotonic: {} >= {}",
                window[0],
                window[1]
            );
        }
    }
}

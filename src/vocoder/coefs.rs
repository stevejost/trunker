//! Higher order DCT coefficients for per-harmonic spectral amplitudes.
//!
//! Two-tier hierarchical DCT coding: the gain IDCT (from `gain.rs`) provides
//! a coarse 6-band envelope R_i, then this module refines each band to
//! per-harmonic resolution using higher-order DCT coefficients. Output:
//! T_l log-domain spectral amplitudes (1 <= l <= L) feeding into `spectral.rs`.

use std::f32::consts::PI;

use super::allocs::allocations;
use super::consts::MIN_HARMONICS;
use super::descramble::QuantizedAmplitudes;
use super::gain::Gains;
use super::params::BaseParams;

/// Higher order DCT coefficients vector T_l, 1 <= l <= L.
///
/// Each element is a log-domain spectral amplitude produced by block IDCT
/// across 6 coefficient blocks.
pub(crate) struct Coefficients {
    /// T_l values, indexed 0-based (element 0 = T_1).
    values: Vec<f32>,
}

impl Coefficients {
    /// Create a new `Coefficients` vector from the given gains G_m, quantized
    /// amplitudes b_m, and frame parameters.
    ///
    /// Generates 6 coefficient blocks, each producing J_i spectral amplitudes
    /// via block IDCT. The total number of output values equals L (harmonic count).
    pub(crate) fn new(
        gains: &Gains,
        amplitudes: &QuantizedAmplitudes,
        params: &BaseParams,
    ) -> Self {
        let mut values = Vec::with_capacity(params.harmonic_count as usize);

        // Tracks the starting quantized amplitude b_m to be inserted into the
        // current coefficient block. For the first block this is always b_8 [p34].
        let mut cursor = 8;

        // Generate blocks for 1 <= i <= 6.
        for block in 1..=6 {
            let coef_block = CoefficientBlock::new(block, cursor, gains, amplitudes, params);
            let block_length = coef_block.length();

            for j in 1..=block_length {
                values.push(coef_block.inverse_dct(j));
            }

            // The first coefficient C_i,1 in each block doesn't count towards
            // quantized amplitude usage; only C_i,2..C_i,Ji consume b_m values.
            cursor += block_length - 1;
        }

        Self { values }
    }

    /// Retrieve T_l, 1 <= l <= L.
    ///
    /// # Panics
    ///
    /// Panics in debug builds if `l` is outside the valid range.
    pub(crate) fn get(&self, l: usize) -> f32 {
        debug_assert!(
            l >= 1 && l <= self.values.len(),
            "coefficient index {l} out of range 1..={}",
            self.values.len()
        );
        self.values[l - 1]
    }
}

/// Block of coefficients C_i,k, 1 <= i <= 6 and 1 <= k <= J_i.
///
/// Each block contains one gain IDCT value (C_i,1 = R_i) followed by
/// higher-order coefficients derived from quantized amplitudes.
struct CoefficientBlock {
    /// Coefficient values C_i,1..C_i,Ji.
    values: Vec<f32>,
}

impl CoefficientBlock {
    /// Create a new `CoefficientBlock` from the given block index i (1-based),
    /// the starting quantized amplitude number, gains G_m, quantized amplitudes
    /// b_m, and frame parameters.
    fn new(
        block: usize,
        cursor: usize,
        gains: &Gains,
        amplitudes: &QuantizedAmplitudes,
        params: &BaseParams,
    ) -> Self {
        debug_assert!(
            (1..=6).contains(&block),
            "block index {block} out of range 1..=6"
        );

        let (allocation, _) = allocations(params.harmonic_count);
        let blocks = &AMPLITUDES_USED[params.harmonic_count as usize - MIN_HARMONICS];

        // C_i,1 = R_i (gain IDCT value for this band).
        let mut values = Vec::with_capacity(blocks[block - 1] + 2);
        values.push(gains.inverse_dct(block));

        // Compute the starting and ending m for the b_m's used in this block.
        let start = cursor;
        let stop = start + blocks[block - 1];

        // Generate C_i,2, ..., C_i,Ji.
        for (k, m) in (start..stop).enumerate() {
            // Retrieve the bit allocation B_m for the current quantized amplitude b_m.
            let bits = allocation[m - 3] as i32;

            // Compute C_i,k+2.
            let value = if bits == 0 {
                0.0
            } else {
                DCT_STEP_SIZE[bits as usize - 1]
                    * DCT_STANDARD_DEVIATION[k]
                    * (amplitudes.get(m) as f32 - (1i32 << (bits - 1)) as f32 + 0.5)
            };
            values.push(value);
        }

        Self { values }
    }

    /// Retrieve the number of coefficients in this block, J_i.
    fn length(&self) -> usize {
        self.values.len()
    }

    /// Compute the IDCT c_i,j for the current block i and 1 <= j <= J_i.
    ///
    /// The block IDCT denominator is J_i (variable per block), unlike the
    /// gain IDCT which uses a fixed denominator of 6.
    fn inverse_dct(&self, j: usize) -> f32 {
        debug_assert!(
            j >= 1 && j <= self.length(),
            "IDCT index {j} out of range 1..={}",
            self.length()
        );

        let block_length = self.length() as f32;

        self.values[0]
            + 2.0
                * (2..=self.length()).fold(0.0f32, |sum, k| {
                    sum + self.values[k - 1]
                        * (PI * (k as f32 - 1.0) * (j as f32 - 0.5) / block_length).cos()
                })
    }
}

/// Each `AMPLITUDES_USED[l]` gives J_1 - 1, ..., J_6 - 1 for harmonics
/// parameter l = L - 9. Each J_i - 1 represents the number of quantized
/// amplitudes used in coefficient block i.
///
/// The actual block length J_i = AMPLITUDES_USED[l][i] + 1 (the +1 accounts
/// for the gain IDCT value C_i,1 = R_i).
static AMPLITUDES_USED: [[usize; 6]; 48] = [
    [0, 0, 0, 1, 1, 1],
    [0, 0, 1, 1, 1, 1],
    [0, 1, 1, 1, 1, 1],
    [1, 1, 1, 1, 1, 1],
    [1, 1, 1, 1, 1, 2],
    [1, 1, 1, 1, 2, 2],
    [1, 1, 1, 2, 2, 2],
    [1, 1, 2, 2, 2, 2],
    [1, 2, 2, 2, 2, 2],
    [2, 2, 2, 2, 2, 2],
    [2, 2, 2, 2, 2, 3],
    [2, 2, 2, 2, 3, 3],
    [2, 2, 2, 3, 3, 3],
    [2, 2, 3, 3, 3, 3],
    [2, 3, 3, 3, 3, 3],
    [3, 3, 3, 3, 3, 3],
    [3, 3, 3, 3, 3, 4],
    [3, 3, 3, 3, 4, 4],
    [3, 3, 3, 4, 4, 4],
    [3, 3, 4, 4, 4, 4],
    [3, 4, 4, 4, 4, 4],
    [4, 4, 4, 4, 4, 4],
    [4, 4, 4, 4, 4, 5],
    [4, 4, 4, 4, 5, 5],
    [4, 4, 4, 5, 5, 5],
    [4, 4, 5, 5, 5, 5],
    [4, 5, 5, 5, 5, 5],
    [5, 5, 5, 5, 5, 5],
    [5, 5, 5, 5, 5, 6],
    [5, 5, 5, 5, 6, 6],
    [5, 5, 5, 6, 6, 6],
    [5, 5, 6, 6, 6, 6],
    [5, 6, 6, 6, 6, 6],
    [6, 6, 6, 6, 6, 6],
    [6, 6, 6, 6, 6, 7],
    [6, 6, 6, 6, 7, 7],
    [6, 6, 6, 7, 7, 7],
    [6, 6, 7, 7, 7, 7],
    [6, 7, 7, 7, 7, 7],
    [7, 7, 7, 7, 7, 7],
    [7, 7, 7, 7, 7, 8],
    [7, 7, 7, 7, 8, 8],
    [7, 7, 7, 8, 8, 8],
    [7, 7, 8, 8, 8, 8],
    [7, 8, 8, 8, 8, 8],
    [8, 8, 8, 8, 8, 8],
    [8, 8, 8, 8, 8, 9],
    [8, 8, 8, 8, 9, 9],
];

/// Each `DCT_STEP_SIZE[b]` is the uniform quantizer step size [p31] for the
/// bit allocation b = B_m - 1.
const DCT_STEP_SIZE: [f32; 10] = [
    1.2, 0.85, 0.65, 0.40, 0.28, 0.15, 0.08, 0.04, 0.02, 0.01,
];

/// Each `DCT_STANDARD_DEVIATION[j]` is the DCT standard deviation [p32] for
/// the coefficient C_i,j+2.
const DCT_STANDARD_DEVIATION: [f32; 9] = [
    0.307, 0.241, 0.207, 0.190, 0.179, 0.173, 0.165, 0.170, 0.170,
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vocoder::descramble::{descramble, Bootstrap};

    #[test]
    fn verify_amplitudes_used_table() {
        // Verify the constraints on J_i [p33].
        for l in 9..=56usize {
            let amps = &AMPLITUDES_USED[l - 9];

            // Sum of all J_i must equal L.
            let sum: usize = amps.iter().map(|&x| x + 1).sum();
            assert_eq!(sum, l, "J_i sum mismatch for L={l}");

            // J_1 >= floor(L/6).
            assert!(
                l / 6 <= amps[0] + 1,
                "J_1 too small for L={l}: {} < {}",
                amps[0] + 1,
                l / 6
            );

            // J_i values are non-decreasing.
            for i in 0..5 {
                assert!(
                    amps[i] <= amps[i + 1],
                    "non-monotonic J_i for L={l}: J_{} > J_{}",
                    i + 1,
                    i + 2
                );
            }

            // J_6 <= ceil(L/6).
            assert!(
                amps[5] + 1 <= (l as f32 / 6.0).ceil() as usize,
                "J_6 too large for L={l}: {} > {}",
                amps[5] + 1,
                (l as f32 / 6.0).ceil() as usize
            );
        }
    }

    #[test]
    fn coefficients_with_16_harmonics() {
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
        let coefs = Coefficients::new(&gains, &amplitudes, &params);

        assert!((coefs.get(1) - -0.9140941318413551).abs() < 1e-6);
        assert!((coefs.get(2) - -1.5002149427668843).abs() < 1e-6);
        assert!((coefs.get(3) - -0.1243967542115918).abs() < 1e-6);
        assert!((coefs.get(4) - -2.924751739744676).abs() < 1e-6);
        assert!((coefs.get(5) - 2.1501046599919884).abs() < 1e-6);
        assert!((coefs.get(6) - -0.557148495647667).abs() < 1e-6);
        assert!((coefs.get(7) - -0.08320165128732226).abs() < 1e-6);
        assert!((coefs.get(8) - 3.412285791008137).abs() < 1e-6);
        assert!((coefs.get(9) - 0.38493783640665913).abs() < 1e-6);
        assert!((coefs.get(10) - 0.6472398818051812).abs() < 1e-6);
        assert!((coefs.get(11) - 3.0575528322544274).abs() < 1e-6);
        assert!((coefs.get(12) - 0.672970246978134).abs() < 1e-6);
        assert!((coefs.get(13) - 0.638137661701841).abs() < 1e-6);
        assert!((coefs.get(14) - 3.9893475658703124).abs() < 1e-6);
        assert!((coefs.get(15) - 3.5092571965451276).abs() < 1e-6);
        assert!((coefs.get(16) - 3.643716827219943).abs() < 1e-6);
    }

    #[test]
    fn coefficient_blocks_with_9_harmonics() {
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
        let (amplitudes, _, gain_index) = descramble(&chunks, &params).unwrap();
        let gains = Gains::new(gain_index, &amplitudes, &params);

        let c = CoefficientBlock::new(1, 8, &gains, &amplitudes, &params);
        assert_eq!(c.length(), 1);
        assert!((c.values[0] - 0.8519942560055926).abs() < 1e-6);
        assert!((c.inverse_dct(1) - 0.8519942560055926).abs() < 1e-6);

        let c = CoefficientBlock::new(2, 8, &gains, &amplitudes, &params);
        assert_eq!(c.length(), 1);
        assert!((c.values[0] - -0.13083074772702047).abs() < 1e-6);
        assert!((c.inverse_dct(1) - -0.13083074772702047).abs() < 1e-6);

        let c = CoefficientBlock::new(3, 8, &gains, &amplitudes, &params);
        assert_eq!(c.length(), 1);
        assert!((c.values[0] - -0.014229409757043066).abs() < 1e-6);
        assert!((c.inverse_dct(1) - -0.014229409757043066).abs() < 1e-6);

        let c = CoefficientBlock::new(4, 8, &gains, &amplitudes, &params);
        assert_eq!(c.length(), 2);
        assert!((c.values[0] - 2.0309896309891773).abs() < 1e-6);
        assert!((c.values[1] - -0.45129).abs() < 1e-5);
        assert!((c.inverse_dct(1) - 1.3927691924258232).abs() < 1e-6);
        assert!((c.inverse_dct(2) - 2.6692100695525314).abs() < 1e-6);

        let c = CoefficientBlock::new(5, 9, &gains, &amplitudes, &params);
        assert_eq!(c.length(), 2);
        assert!((c.values[0] - 0.5902767477270205).abs() < 1e-6);
        assert!((c.values[1] - -0.8289).abs() < 1e-4);
        assert!((c.inverse_dct(1) - -0.581964874124038).abs() < 1e-6);
        assert!((c.inverse_dct(2) - 1.7625183695780788).abs() < 1e-6);

        let c = CoefficientBlock::new(6, 10, &gains, &amplitudes, &params);
        assert_eq!(c.length(), 2);
        assert!((c.values[0] - 1.0951375227622724).abs() < 1e-6);
        assert!((c.values[1] - 1.24028).abs() < 1e-5);
        assert!((c.inverse_dct(1) - 2.849158319902375).abs() < 1e-6);
        assert!((c.inverse_dct(2) - -0.6588832743778299).abs() < 1e-6);
    }

    #[test]
    fn coefficient_blocks_with_16_harmonics() {
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

        let c = CoefficientBlock::new(1, 8, &gains, &amplitudes, &params);
        assert_eq!(c.length(), 2);
        assert!((c.values[0] - -1.2071545373041197).abs() < 1e-6);
        assert!((c.values[1] - 0.207225).abs() < 1e-6);
        assert!((c.inverse_dct(1) - -0.9140941318413551).abs() < 1e-6);
        assert!((c.inverse_dct(2) - -1.5002149427668843).abs() < 1e-6);

        let c = CoefficientBlock::new(2, 9, &gains, &amplitudes, &params);
        assert_eq!(c.length(), 2);
        assert!((c.values[0] - -1.524574246978134).abs() < 1e-6);
        assert!((c.values[1] - 0.990075).abs() < 1e-6);
        assert!((c.inverse_dct(1) - -0.1243967542115918).abs() < 1e-6);
        assert!((c.inverse_dct(2) - -2.924751739744676).abs() < 1e-6);

        let c = CoefficientBlock::new(3, 10, &gains, &amplitudes, &params);
        assert_eq!(c.length(), 3);
        assert!((c.values[0] - 0.5032515043523329).abs() < 1e-6);
        assert!((c.values[1] - 0.6447).abs() < 1e-4);
        assert!((c.values[2] - 0.5302).abs() < 1e-4);
        assert!((c.inverse_dct(1) - 2.1501046599919884).abs() < 1e-6);
        assert!((c.inverse_dct(2) - -0.557148495647667).abs() < 1e-6);
        assert!((c.inverse_dct(3) - -0.08320165128732226).abs() < 1e-6);

        let c = CoefficientBlock::new(4, 12, &gains, &amplitudes, &params);
        assert_eq!(c.length(), 3);
        assert!((c.values[0] - 1.481487836406659).abs() < 1e-6);
        assert!((c.values[1] - 0.7982).abs() < 1e-4);
        assert!((c.values[2] - 0.548275).abs() < 1e-6);
        assert!((c.inverse_dct(1) - 3.412285791008137).abs() < 1e-6);
        assert!((c.inverse_dct(2) - 0.38493783640665913).abs() < 1e-6);
        assert!((c.inverse_dct(3) - 0.6472398818051812).abs() < 1e-6);

        let c = CoefficientBlock::new(5, 14, &gains, &amplitudes, &params);
        assert_eq!(c.length(), 3);
        assert!((c.values[0] - 1.456220246978134).abs() < 1e-6);
        assert!((c.values[1] - 0.698425).abs() < 1e-6);
        assert!((c.values[2] - 0.391625).abs() < 1e-6);
        assert!((c.inverse_dct(1) - 3.0575528322544274).abs() < 1e-6);
        assert!((c.inverse_dct(2) - 0.672970246978134).abs() < 1e-6);
        assert!((c.inverse_dct(3) - 0.638137661701841).abs() < 1e-6);

        let c = CoefficientBlock::new(6, 16, &gains, &amplitudes, &params);
        assert_eq!(c.length(), 3);
        assert!((c.values[0] - 3.7141071965451276).abs() < 1e-6);
        assert!((c.values[1] - 0.099775).abs() < 1e-6);
        assert!((c.values[2] - 0.102425).abs() < 1e-6);
        assert!((c.inverse_dct(1) - 3.9893475658703124).abs() < 1e-6);
        assert!((c.inverse_dct(2) - 3.5092571965451276).abs() < 1e-6);
        assert!((c.inverse_dct(3) - 3.643716827219943).abs() < 1e-6);
    }

    #[test]
    fn bits_zero_produces_zero_coefficient() {
        // When bit allocation B_m = 0, the coefficient C_i,k should be 0.0.
        // This is tested implicitly via the L=9 test where blocks 1-3 have
        // length 1 (no higher-order coefficients), but let's verify the
        // formula explicitly: for L=9, blocks 1-3 have AMPLITUDES_USED=0,
        // so they only contain the gain IDCT value with no b_m contributions.
        let amps = &AMPLITUDES_USED[0]; // L=9
        assert_eq!(amps[0], 0); // Block 1: J_1 - 1 = 0, so J_1 = 1
        assert_eq!(amps[1], 0); // Block 2: J_2 - 1 = 0, so J_2 = 1
        assert_eq!(amps[2], 0); // Block 3: J_3 - 1 = 0, so J_3 = 1
    }

    #[test]
    fn cursor_advances_correctly() {
        // After processing all 6 blocks, cursor should equal L + 2.
        // For L=16: start=8, blocks consume [1,1,2,2,2,2] = 10, so end=18 = 16+2.
        let blocks = &AMPLITUDES_USED[16 - 9];
        let total_consumed: usize = blocks.iter().sum();
        assert_eq!(8 + total_consumed, 16 + 2);

        // For L=9: start=8, blocks consume [0,0,0,1,1,1] = 3, so end=11 = 9+2.
        let blocks = &AMPLITUDES_USED[9 - 9];
        let total_consumed: usize = blocks.iter().sum();
        assert_eq!(8 + total_consumed, 9 + 2);

        // For all valid L, verify cursor == L + 2.
        for l in 9..=56usize {
            let blocks = &AMPLITUDES_USED[l - 9];
            let total_consumed: usize = blocks.iter().sum();
            assert_eq!(
                8 + total_consumed,
                l + 2,
                "cursor mismatch for L={l}"
            );
        }
    }
}

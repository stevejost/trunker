//! Constants used in the IMBE vocoder.

/// Audio samples per second (Hz).
#[allow(dead_code)]
pub const SAMPLE_RATE: usize = 8000;

/// Number of audio samples per voiced/unvoiced frame (20 ms at 8 kHz).
pub const SAMPLES_PER_FRAME: usize = 160;

/// Minimum number of harmonics L (when fundamental frequency is maximum).
pub const MIN_HARMONICS: usize = 9;

/// Maximum number of harmonics L (when fundamental frequency is minimum).
pub const MAX_HARMONICS: usize = 56;

/// Number of discrete values that the harmonics parameter L can take.
pub const NUM_HARMONICS: usize = MAX_HARMONICS - MIN_HARMONICS + 1;

/// Maximum number of quantized amplitudes.
///
/// Since b_m for 3 <= m <= L+1 must be stored and 9 <= L <= 56,
/// the maximum capacity required is 57 - 3 + 1 = 55 (i.e. MAX_HARMONICS - 1).
pub const MAX_QUANTIZED_AMPLITUDES: usize = MAX_HARMONICS - 1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_duration_is_20ms() {
        assert_eq!(SAMPLES_PER_FRAME * 1000 / SAMPLE_RATE, 20);
    }

    #[test]
    fn num_harmonics_is_range_inclusive() {
        assert_eq!(NUM_HARMONICS, 48);
    }
}

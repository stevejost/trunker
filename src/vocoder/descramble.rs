//! Initial descrambling of prioritized chunks into voice parameters.
//!
//! Extracts the quantized amplitudes b_m, voiced/unvoiced decisions, and
//! gain index from the prioritized chunks u_0..u_7 using the scanning
//! procedures defined in TIA-102.BABA.

use super::allocs::allocations;
use super::consts::MAX_QUANTIZED_AMPLITUDES;
use super::error::VocoderError;
use super::frame::Chunks;
use super::params::BaseParams;
use super::scan::{ScanBits, ScanChunks, ScanSeparator};

/// Descramble the given prioritized chunks u_i into the underlying quantized
/// amplitudes b_m, voiced/unvoiced decisions, and initial gain index b_2.
///
/// Returns an error if the scan bit count doesn't match the bit allocation
/// table (indicates a logic bug, not an RF condition).
pub fn descramble(
    chunks: &Chunks,
    params: &BaseParams,
) -> Result<(QuantizedAmplitudes, VoiceDecisions, usize), VocoderError> {
    let separator = ScanSeparator::new(chunks, params);

    Ok((
        QuantizedAmplitudes::new(
            ScanBits::new(ScanChunks::new(chunks, separator.scanned, params)),
            params,
        )?,
        VoiceDecisions::new(separator.voiced, params),
        gain_index(chunks, separator.gain_index_part),
    ))
}

/// Decoded bootstrap value b_0 from the frame header.
///
/// Determines whether the frame contains normal voice data (with an encoded
/// pitch period), is a silence frame, or has an invalid period value.
#[derive(Debug, Copy, Clone)]
pub enum Bootstrap {
    /// Frame contains voiced/unvoiced data derived from the enclosed pitch period b_0.
    Period(u8),
    /// Frame is silence.
    Silence,
    /// Invalid b_0 value was detected.
    Invalid,
}

impl Bootstrap {
    /// Parse a `Bootstrap` value from the given chunks.
    ///
    /// Period values 0..=207 are valid voice frames, 216..=219 indicate
    /// silence, and all other values are invalid per TIA-102.BABA [p46].
    pub fn new(chunks: &Chunks) -> Self {
        match period(chunks) {
            p @ 0..=207 => Self::Period(p),
            216..=219 => Self::Silence,
            _ => Self::Invalid,
        }
    }

    /// Unwrap the period value, panicking if not a `Period` variant.
    ///
    /// Only available in test builds.
    #[cfg(test)]
    pub fn unwrap_period(&self) -> u8 {
        if let Self::Period(p) = *self {
            p
        } else {
            panic!("attempted unwrap of invalid/silence period");
        }
    }
}

/// Compute the 8-bit quantized pitch period b_0 from chunks u_0 and u_7.
///
/// Concatenates the 6 MSBs of u_0 with bits 2 and 1 of u_7 per [p39].
fn period(chunks: &Chunks) -> u8 {
    ((chunks[0] >> 4) as u8 & 0b11111100) | ((chunks[7] >> 1) as u8 & 0b11)
}

/// Compute the 6-bit gain index b_2 used to look up the quantized gain value.
///
/// Concatenates bits 5..3 of u_0, the two separator index bits, and bit 3
/// of u_7 per [p39].
fn gain_index(chunks: &Chunks, index_part: u32) -> usize {
    ((chunks[0] & 0b111000) | (index_part << 1) | ((chunks[7] >> 3) & 1)) as usize
}

/// Reconstructed quantized amplitudes b_3 through b_{L+1}.
///
/// Stores up to `MAX_QUANTIZED_AMPLITUDES` (55) values extracted from the
/// scanning procedure. The number of active amplitudes is `L - 1` where
/// L is the harmonic count.
#[derive(Debug, Copy, Clone)]
pub struct QuantizedAmplitudes {
    /// Amplitude values indexed by `m - 3`.
    values: [u32; MAX_QUANTIZED_AMPLITUDES],
    /// Number of active amplitudes (harmonic_count - 1).
    count: usize,
}

impl QuantizedAmplitudes {
    /// Reconstruct quantized amplitudes from the given bit scan.
    ///
    /// Iterates through bit levels MSB-to-LSB per TIA-102.BABA [p39],
    /// distributing scanned bits across amplitudes according to the
    /// bit allocation table.
    ///
    /// Returns an error if the scan produces too few or too many bits
    /// for the given harmonic count (indicates a logic/table bug).
    fn new(mut scan: ScanBits, params: &BaseParams) -> Result<Self, VocoderError> {
        let count = (params.harmonic_count - 1) as usize;
        let mut values = [0u32; MAX_QUANTIZED_AMPLITUDES];

        let (bits, max) = allocations(params.harmonic_count);

        // Iterate through bit levels, MSB to LSB.
        for level in (0..max).rev() {
            // Iterate over all b_i.
            for i in 0..count {
                // Skip this b_i if there are no bits allocated at this level.
                if bits[i] <= level {
                    continue;
                }

                // Shift the next scanned bit onto the LSB.
                values[i] <<= 1;
                values[i] |= scan.next().ok_or(VocoderError::ScanExhausted {
                    harmonic_count: params.harmonic_count,
                })?;
            }
        }

        if scan.next().is_some() {
            return Err(VocoderError::ScanNotExhausted {
                harmonic_count: params.harmonic_count,
            });
        }

        Ok(Self { values, count })
    }

    /// Retrieve the quantized amplitude b_m where 3 <= m <= L+1.
    ///
    /// # Panics
    ///
    /// Panics in debug builds if `m` is out of range.
    pub fn get(&self, m: usize) -> u32 {
        let index = m - 3;
        debug_assert!(index < self.count, "amplitude index {m} out of range");
        self.values[index]
    }
}

/// Tracks per-harmonic voiced/unvoiced decisions.
///
/// Decisions are initialized from the received band-level voiced/unvoiced
/// vector b_1, expanding each band decision to cover its 3 harmonics
/// (the last band may cover fewer). Individual harmonics can later be
/// forced voiced by adaptive smoothing per [p49].
#[derive(Debug, Copy, Clone)]
pub struct VoiceDecisions {
    /// Parameters for the current frame.
    params: BaseParams,
    /// Per-harmonic voiced/unvoiced bitmap.
    ///
    /// Bit `(L - l)` is set if harmonic `l` (1-based) is voiced.
    voiced: u64,
}

impl VoiceDecisions {
    /// Create new `VoiceDecisions` from the band-level voiced/unvoiced bitmap
    /// b_1 and frame parameters.
    pub fn new(voiced: u32, params: &BaseParams) -> Self {
        Self {
            params: *params,
            voiced: generate_harmonics_bitmap(voiced, params),
        }
    }

    /// Force the given harmonic (1-based) to be voiced.
    pub fn force_voiced(&mut self, harmonic: usize) {
        self.voiced |= self.mask(harmonic);
    }

    /// Compute the number of unvoiced harmonics.
    pub fn unvoiced_count(&self) -> u32 {
        self.params.harmonic_count - self.voiced.count_ones()
    }

    /// Check if the given harmonic (1-based) is voiced.
    pub fn is_voiced(&self, harmonic: usize) -> bool {
        if harmonic as u32 > self.params.harmonic_count {
            false
        } else {
            self.voiced & self.mask(harmonic) != 0
        }
    }

    /// Create a 1-bit mask for the bit position of the given harmonic (1-based).
    fn mask(&self, harmonic: usize) -> u64 {
        1 << (self.params.harmonic_count as usize - harmonic)
    }
}

impl Default for VoiceDecisions {
    /// Create `VoiceDecisions` in default state: all harmonics unvoiced per [p64].
    fn default() -> Self {
        Self::new(0, &BaseParams::default())
    }
}

/// Expand band-level voiced/unvoiced bitmap b_1 into a per-harmonic bitmap.
///
/// Each band contains 3 harmonics (except possibly the last, which may
/// contain fewer when `harmonic_count % 3 != 0`). The resulting bitmap
/// has the LSB represent harmonic L, ascending through more significant
/// bits to harmonic 1.
fn generate_harmonics_bitmap(voiced: u32, params: &BaseParams) -> u64 {
    // Expand each band bit into 3 harmonic bits.
    let bits = (0..params.band_count).rev().fold(0u64, |bits, i| {
        bits << 3
            | if voiced >> i & 1 == 1 {
                0b111
            } else {
                0
            }
    });

    // Check if the last band contains fewer than 3 harmonics.
    let remainder = params.harmonic_count % 3;

    if remainder == 0 {
        bits
    } else {
        // Strip off extra harmonics from the last band.
        bits >> (3 - remainder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_extraction() {
        let chunks = [
            0b010101111111,
            0b111111111111,
            0b111111111111,
            0b111111111111,
            0b11111111111,
            0b11111111111,
            0b11111111111,
            0b1111011,
        ];
        assert_eq!(period(&chunks), 0b01010101);
    }

    #[test]
    fn gain_index_extraction() {
        let chunks = [
            0b111111010111,
            0b111111111111,
            0b111111111111,
            0b111111111111,
            0b11111111111,
            0b11111111111,
            0b11111111111,
            0b1110111,
        ];
        assert_eq!(gain_index(&chunks, 0b01), 0b010010);
    }

    #[test]
    #[should_panic]
    fn invalid_period_panics_on_unwrap() {
        let chunks = [
            0b111111000000,
            0, 0, 0, 0, 0, 0,
            0b00000010,
        ];
        Bootstrap::new(&chunks).unwrap_period();
    }

    #[test]
    fn descramble_with_16_harmonics() {
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
        let (amplitudes, voice, gain) = descramble(&chunks, &params).unwrap();

        assert_eq!(params.harmonic_count, 16);
        assert_eq!(params.band_count, 6);
        assert_eq!(gain, 0b010101);

        assert_eq!(amplitudes.get(3), 0b000110);
        assert_eq!(amplitudes.get(4), 0b100010);
        assert_eq!(amplitudes.get(5), 0b011011);
        assert_eq!(amplitudes.get(6), 0b11001);
        assert_eq!(amplitudes.get(7), 0b01111);
        assert_eq!(amplitudes.get(8), 0b100100);
        assert_eq!(amplitudes.get(9), 0b110101);
        assert_eq!(amplitudes.get(10), 0b10111);
        assert_eq!(amplitudes.get(11), 0b1101);
        assert_eq!(amplitudes.get(12), 0b1110);
        assert_eq!(amplitudes.get(13), 0b111);
        assert_eq!(amplitudes.get(14), 0b111);
        assert_eq!(amplitudes.get(15), 0b110);
        assert_eq!(amplitudes.get(16), 0b100);
        assert_eq!(amplitudes.get(17), 0b10);

        assert_eq!(voice.unvoiced_count(), 9);

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

        for l in 17..63 {
            assert!(!voice.is_voiced(l));
        }
    }

    #[test]
    fn descramble_with_10_harmonics() {
        let chunks = [
            0b000001010010,
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
        let (amplitudes, voice, gain) = descramble(&chunks, &params).unwrap();

        assert_eq!(params.harmonic_count, 10);
        assert_eq!(params.band_count, 4);
        assert_eq!(gain, 0b010011);

        assert_eq!(amplitudes.get(3), 0b010101011);
        assert_eq!(amplitudes.get(4), 0b110101101);
        assert_eq!(amplitudes.get(5), 0b01001011);
        assert_eq!(amplitudes.get(6), 0b01011000);
        assert_eq!(amplitudes.get(7), 0b10011101);
        assert_eq!(amplitudes.get(8), 0b010111011);
        assert_eq!(amplitudes.get(9), 0b1111110);
        assert_eq!(amplitudes.get(10), 0b110110);
        assert_eq!(amplitudes.get(11), 0b11100);

        assert_eq!(voice.unvoiced_count(), 3);

        assert!(voice.is_voiced(1));
        assert!(voice.is_voiced(2));
        assert!(voice.is_voiced(3));
        assert!(voice.is_voiced(4));
        assert!(voice.is_voiced(5));
        assert!(voice.is_voiced(6));
        assert!(!voice.is_voiced(7));
        assert!(!voice.is_voiced(8));
        assert!(!voice.is_voiced(9));
        assert!(voice.is_voiced(10));

        for l in 11..=63 {
            assert!(!voice.is_voiced(l));
        }
    }

    #[test]
    fn descramble_with_16_harmonics_variant() {
        let chunks = [
            0b001000010010,
            0b110011001100,
            0b111000111000,
            0b111111111111,
            0b10101110101,
            0b00101111010,
            0b01110111011,
            0b00001000,
        ];

        let bootstrap = Bootstrap::new(&chunks);
        let params = BaseParams::new(bootstrap.unwrap_period());
        let (_, voice, _) = descramble(&chunks, &params).unwrap();

        assert_eq!(params.harmonic_count, 16);
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
        assert!(voice.is_voiced(13));
        assert!(voice.is_voiced(14));
        assert!(voice.is_voiced(15));
        assert!(voice.is_voiced(16));

        for l in 17..=63 {
            assert!(!voice.is_voiced(l));
        }
    }

    #[test]
    fn voice_decisions_bitmap_generation() {
        // L=16, K=6, remainder = 16 % 3 = 1
        let params = BaseParams::new(32);
        assert_eq!(params.harmonic_count, 16);
        assert_eq!(params.band_count, 6);

        let v = VoiceDecisions::new(0b101011, &params);
        assert_eq!(v.unvoiced_count(), 6);
        assert_eq!(v.voiced, 0b1110001110001111);

        let v = VoiceDecisions::new(0b101010, &params);
        assert_eq!(v.unvoiced_count(), 7);
        assert_eq!(v.voiced, 0b1110001110001110);

        let v = VoiceDecisions::new(0b001001, &params);
        assert_eq!(v.unvoiced_count(), 12);
        assert_eq!(v.voiced, 0b0000001110000001);

        let v = VoiceDecisions::new(0b101000, &params);
        assert_eq!(v.unvoiced_count(), 10);
        assert_eq!(v.voiced, 0b1110001110000000);

        let v = VoiceDecisions::new(0b000000, &params);
        assert_eq!(v.unvoiced_count(), 16);
        assert_eq!(v.voiced, 0b0000000000000000);

        let v = VoiceDecisions::new(0b111111, &params);
        assert_eq!(v.unvoiced_count(), 0);
        assert_eq!(v.voiced, 0b1111111111111111);

        // L=17, K=6, remainder = 17 % 3 = 2
        let params = BaseParams::new(36);
        assert_eq!(params.harmonic_count, 17);
        assert_eq!(params.band_count, 6);

        let v = VoiceDecisions::new(0b101011, &params);
        assert_eq!(v.unvoiced_count(), 6);
        assert_eq!(v.voiced, 0b11100011100011111);

        let v = VoiceDecisions::new(0b101010, &params);
        assert_eq!(v.unvoiced_count(), 8);
        assert_eq!(v.voiced, 0b11100011100011100);

        let v = VoiceDecisions::new(0b001001, &params);
        assert_eq!(v.unvoiced_count(), 12);
        assert_eq!(v.voiced, 0b00000011100000011);

        let v = VoiceDecisions::new(0b101000, &params);
        assert_eq!(v.unvoiced_count(), 11);
        assert_eq!(v.voiced, 0b11100011100000000);

        let v = VoiceDecisions::new(0b000000, &params);
        assert_eq!(v.unvoiced_count(), 17);
        assert_eq!(v.voiced, 0b00000000000000000);

        let v = VoiceDecisions::new(0b111111, &params);
        assert_eq!(v.unvoiced_count(), 0);
        assert_eq!(v.voiced, 0b11111111111111111);

        // L=18, K=6, remainder = 18 % 3 = 0
        let params = BaseParams::new(40);
        assert_eq!(params.harmonic_count, 18);
        assert_eq!(params.band_count, 6);

        let v = VoiceDecisions::new(0b101011, &params);
        assert_eq!(v.unvoiced_count(), 6);
        assert_eq!(v.voiced, 0b111000111000111111);

        let v = VoiceDecisions::new(0b101010, &params);
        assert_eq!(v.unvoiced_count(), 9);
        assert_eq!(v.voiced, 0b111000111000111000);

        let v = VoiceDecisions::new(0b001001, &params);
        assert_eq!(v.unvoiced_count(), 12);
        assert_eq!(v.voiced, 0b000000111000000111);

        let v = VoiceDecisions::new(0b101000, &params);
        assert_eq!(v.unvoiced_count(), 12);
        assert_eq!(v.voiced, 0b111000111000000000);

        let v = VoiceDecisions::new(0b000000, &params);
        assert_eq!(v.unvoiced_count(), 18);
        assert_eq!(v.voiced, 0b000000000000000000);

        let v = VoiceDecisions::new(0b111111, &params);
        assert_eq!(v.unvoiced_count(), 0);
        assert_eq!(v.voiced, 0b111111111111111111);
    }

    #[test]
    #[should_panic]
    fn amplitude_out_of_bounds_panics_in_debug() {
        let chunks = [
            0b000001010010,
            0b110011001100,
            0b111000111000,
            0b111111111111,
            0b11110110101,
            0b00101111010,
            0b01110111011,
            0b00001000,
        ];

        let bootstrap = Bootstrap::new(&chunks);
        let params = BaseParams::new(bootstrap.unwrap_period());
        let (amplitudes, _, _) = descramble(&chunks, &params).unwrap();

        assert_eq!(params.harmonic_count, 10);
        // m=12 is out of range for L=10 (valid range: 3..=11)
        amplitudes.get(12);
    }
}

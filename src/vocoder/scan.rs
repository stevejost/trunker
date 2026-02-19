//! Scanning procedure used in IMBE descrambling.
//!
//! The scanning procedure extracts voiced/unvoiced decisions, the quantized
//! gain index fragment, and spectral amplitude bits from the prioritized
//! chunks u_0..u_7. See TIA-102.BABA section on scanning procedures.

use std::ops::Range;

use super::frame::Chunks;
use super::params::BaseParams;

/// Decoded separator fields from the boundary between the two scanning procedures.
///
/// Between the chunks covered by the two scanning procedures, there is a 22-bit
/// vector made up of u_4 and u_5 that contains the voiced/unvoiced vector,
/// part of the quantized gain index, and the initial bits in the second
/// scanning procedure.
#[derive(Debug, Copy, Clone)]
pub struct ScanSeparator {
    /// Voiced/unvoiced boolean bit vector b_1, with K bits (one per band).
    pub voiced: u32,
    /// Bits 1 and 2 of the 6-bit quantized gain index b_2.
    pub gain_index_part: u32,
    /// Remaining scanned bits from u_4/u_5 used in the scanning procedure.
    pub scanned: u32,
}

impl ScanSeparator {
    /// Create a new `ScanSeparator` from the given chunks and frame parameters.
    ///
    /// Concatenates u_4 (11 bits) and u_5 (11 bits) into a 22-bit vector,
    /// then splits out the voiced/unvoiced decisions, gain index bits, and
    /// scanned remainder based on the band count K.
    pub fn new(chunks: &Chunks, params: &BaseParams) -> Self {
        // Concatenate u_4 and u_5 into a 22-bit vector.
        let parts = chunks[4] << 11 | chunks[5];

        Self {
            // Take first K MSBs as the voiced/unvoiced vector.
            voiced: parts >> (22 - params.band_count),
            // Take next 2 bits as bits 1 and 2 of b_2.
            gain_index_part: parts >> (20 - params.band_count) & 0b11,
            // Take the remaining (20 - K) LSBs as part of the scanning procedure.
            // Since the underlying word is 32 bits, (32 - 22) + K + 2 = 12 + K.
            scanned: parts & !0 >> (12 + params.band_count),
        }
    }
}

/// Iterator over the chunk segments covered in the scanning procedure.
///
/// Yields 7 segments as (bits, width) tuples, walking through the prioritized
/// chunks in scan order: partial u_0, full u_1..u_3, separator remainder,
/// full u_6, and partial u_7.
pub struct ScanChunks<'a> {
    /// Chunks to scan.
    chunks: &'a Chunks,
    /// Separator chunk and its width in bits.
    separator: (u32, u8),
    /// Current position in the 7-step scan sequence.
    position: Range<u8>,
}

impl<'a> ScanChunks<'a> {
    /// Create a new `ScanChunks` iterator over the given chunks.
    ///
    /// The `scanned` value is the remainder bits from [`ScanSeparator`].
    /// The separator width is `20 - K` bits where K is the band count.
    pub fn new(chunks: &'a Chunks, scanned: u32, params: &BaseParams) -> Self {
        Self {
            chunks,
            // 22 - (K + 2) = 20 - K.
            separator: (scanned, (20 - params.band_count) as u8),
            // Each scan is made up of 7 chunks, each in whole or partial form.
            position: 0..7,
        }
    }
}

/// At each iteration, yields a chunk of bits along with the number of
/// LSBs to use from the chunk.
impl<'a> Iterator for ScanChunks<'a> {
    type Item = (u32, u8);

    fn next(&mut self) -> Option<Self::Item> {
        self.position.next().map(|step| match step {
            // Last 3 LSBs of u_0.
            0 => (self.chunks[0] & 0b111, 3),
            // All of u_1, u_2, and u_3 (12 bits each).
            1 => (self.chunks[1], 12),
            2 => (self.chunks[2], 12),
            3 => (self.chunks[3], 12),
            // Separator remainder from u_4/u_5.
            4 => self.separator,
            // All of u_6 (11 bits).
            5 => (self.chunks[6], 11),
            // First 3 MSBs of u_7.
            6 => (self.chunks[7] >> 4, 3),
            _ => unreachable!(),
        })
    }
}

/// Sequentially extracts individual bits from the scanning procedure.
///
/// Wraps a [`ScanChunks`] iterator and yields one bit at a time, MSB first
/// within each chunk segment.
pub struct ScanBits<'a> {
    /// Underlying chunk iterator.
    chunks: ScanChunks<'a>,
    /// Current chunk bits, stored starting at the MSB of a u32.
    current_chunk: u32,
    /// Bits remaining to yield from the current chunk.
    remaining: u8,
}

impl<'a> ScanBits<'a> {
    /// Create a new `ScanBits` iterator over the given scan chunks.
    pub fn new(chunks: ScanChunks<'a>) -> Self {
        Self {
            chunks,
            current_chunk: 0,
            remaining: 0,
        }
    }
}

/// At each iteration, yields the next single bit (0 or 1) in scan order.
impl<'a> Iterator for ScanBits<'a> {
    type Item = u32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            let (chunk, width) = self.chunks.next()?;
            self.current_chunk = chunk;
            self.remaining = width;
            // Move bits to MSB of 32-bit word.
            self.current_chunk <<= 32 - self.remaining;
        }

        // Extract the MSB and shift it off.
        let bit = self.current_chunk >> 31;
        self.current_chunk <<= 1;
        self.remaining -= 1;

        Some(bit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separator_with_16_harmonics() {
        let chunks = [
            0, 0, 0, 0,
            0b11110110101,
            0b00001111010,
            0, 0,
        ];
        let params = BaseParams::new(32);
        assert_eq!(params.harmonic_count, 16);
        assert_eq!(params.band_count, 6);

        let sep = ScanSeparator::new(&chunks, &params);
        assert_eq!(sep.voiced, 0b111101);
        assert_eq!(sep.gain_index_part, 0b10);
        assert_eq!(sep.scanned, 0b10100001111010);
    }

    #[test]
    fn separator_with_10_harmonics() {
        let chunks = [
            0, 0, 0, 0,
            0b11110110101,
            0b00001111010,
            0, 0,
        ];
        let params = BaseParams::new(4);
        assert_eq!(params.harmonic_count, 10);
        assert_eq!(params.band_count, 4);

        let sep = ScanSeparator::new(&chunks, &params);
        assert_eq!(sep.voiced, 0b1111);
        assert_eq!(sep.gain_index_part, 0b01);
        assert_eq!(sep.scanned, 0b1010100001111010);
    }

    #[test]
    fn scan_chunks_with_16_harmonics() {
        let chunks = [
            0b101,
            0b101010101010,
            0b101010101010,
            0b101010101010,
            0b11111111111,
            0b01010101010,
            0b10101010101,
            0b1010000,
        ];
        let params = BaseParams::new(32);
        assert_eq!(params.harmonic_count, 16);
        assert_eq!(params.band_count, 6);

        let sep = ScanSeparator::new(&chunks, &params);
        let mut iter = ScanChunks::new(&chunks, sep.scanned, &params);

        assert_eq!(iter.next().unwrap(), (0b101, 3));
        assert_eq!(iter.next().unwrap(), (0b101010101010, 12));
        assert_eq!(iter.next().unwrap(), (0b101010101010, 12));
        assert_eq!(iter.next().unwrap(), (0b101010101010, 12));
        assert_eq!(iter.next().unwrap(), (0b11101010101010, 14));
        assert_eq!(iter.next().unwrap(), (0b10101010101, 11));
        assert_eq!(iter.next().unwrap(), (0b101, 3));
        assert!(iter.next().is_none());
    }

    #[test]
    fn scan_chunks_with_10_harmonics() {
        let chunks = [
            0b111111111101,
            0b101010101010,
            0b101010101010,
            0b101010101010,
            0b11111111111,
            0b01010101010,
            0b10101010101,
            0b1010000,
        ];
        let params = BaseParams::new(4);
        assert_eq!(params.harmonic_count, 10);
        assert_eq!(params.band_count, 4);

        let sep = ScanSeparator::new(&chunks, &params);
        let mut iter = ScanChunks::new(&chunks, sep.scanned, &params);

        assert_eq!(iter.next().unwrap(), (0b101, 3));
        assert_eq!(iter.next().unwrap(), (0b101010101010, 12));
        assert_eq!(iter.next().unwrap(), (0b101010101010, 12));
        assert_eq!(iter.next().unwrap(), (0b101010101010, 12));
        assert_eq!(iter.next().unwrap(), (0b1111101010101010, 16));
        assert_eq!(iter.next().unwrap(), (0b10101010101, 11));
        assert_eq!(iter.next().unwrap(), (0b101, 3));
        assert!(iter.next().is_none());
    }

    #[test]
    fn scan_bits_with_16_harmonics() {
        let chunks = [
            0b111111111101,
            0b010101010101,
            0b010101010101,
            0b010101010101,
            0b11111111111,
            0b01010101010,
            0b10101010101,
            0b1010000,
        ];
        let params = BaseParams::new(32);
        assert_eq!(params.harmonic_count, 16);
        assert_eq!(params.band_count, 6);

        let sep = ScanSeparator::new(&chunks, &params);
        let bits: Vec<u32> = ScanBits::new(
            ScanChunks::new(&chunks, sep.scanned, &params),
        ).collect();

        // 3 + 12 + 12 + 12 + 14 + 11 + 3 = 67 bits total
        assert_eq!(bits.len(), 67);

        // u_0 last 3 bits: 101
        assert_eq!(&bits[0..3], &[1, 0, 1]);
        // u_1: 010101010101
        assert_eq!(&bits[3..15], &[0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1]);
        // u_2: 010101010101
        assert_eq!(&bits[15..27], &[0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1]);
        // u_3: 010101010101
        assert_eq!(&bits[27..39], &[0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1]);
        // separator (14 bits): 11101010101010
        assert_eq!(&bits[39..53], &[1, 1, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0]);
        // u_6: 10101010101
        assert_eq!(&bits[53..64], &[1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1]);
        // u_7 first 3 MSBs: 101
        assert_eq!(&bits[64..67], &[1, 0, 1]);
    }

    #[test]
    fn scan_bits_with_10_harmonics() {
        let chunks = [
            0b111111111101,
            0b010101010101,
            0b010101010101,
            0b010101010101,
            0b11111111111,
            0b01010101010,
            0b10101010101,
            0b1010000,
        ];
        let params = BaseParams::new(4);
        assert_eq!(params.harmonic_count, 10);
        assert_eq!(params.band_count, 4);

        let sep = ScanSeparator::new(&chunks, &params);
        let bits: Vec<u32> = ScanBits::new(
            ScanChunks::new(&chunks, sep.scanned, &params),
        ).collect();

        // 3 + 12 + 12 + 12 + 16 + 11 + 3 = 69 bits total
        assert_eq!(bits.len(), 69);

        // u_0 last 3 bits: 101
        assert_eq!(&bits[0..3], &[1, 0, 1]);
        // u_1: 010101010101
        assert_eq!(&bits[3..15], &[0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1]);
        // u_2: 010101010101
        assert_eq!(&bits[15..27], &[0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1]);
        // u_3: 010101010101
        assert_eq!(&bits[27..39], &[0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1]);
        // separator (16 bits): 1111101010101010
        assert_eq!(&bits[39..55], &[1, 1, 1, 1, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0]);
        // u_6: 10101010101
        assert_eq!(&bits[55..66], &[1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1]);
        // u_7 first 3 MSBs: 101
        assert_eq!(&bits[66..69], &[1, 0, 1]);
    }
}

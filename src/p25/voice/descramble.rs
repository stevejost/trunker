//! Voice frame deinterleaver (zigzag descrambler).
//!
//! An IMBE voice frame consists of 72 dibits (144 bits). The bits are
//! interleaved across 8 chunks (u_0..u_7) using a zigzag pattern that
//! alternates between the high (MSB) and low (LSB) bit of each dibit,
//! stepping 3 dibits forward at each position.
//!
//! Chunk sizes: u_0=23, u_1=23, u_2=23, u_3=23, u_4=15, u_5=15, u_6=15, u_7=7.
//! Total: 144 bits = 72 dibits x 2 bits.

use crate::p25::types::Dibit;

/// Number of dibits in a single IMBE voice frame.
pub const VOICE_FRAME_DIBITS: usize = 72;

/// Descramble chunk `index` (0..8) from the given voice frame dibits.
///
/// Returns the chunk as a right-aligned integer: u_0..u_3 are 23 bits,
/// u_4..u_6 are 15 bits, and u_7 is 7 bits.
pub fn descramble(dibits: &[Dibit; VOICE_FRAME_DIBITS], index: usize) -> u32 {
    debug_assert!(index < 8, "chunk index must be 0..8, got {index}");

    let segments = &CHUNKS[index];
    let mut word = 0u32;

    for segment in segments.iter() {
        if segment.count == 0 {
            break;
        }

        let mut dibit_index = segment.start;
        let mut use_high = segment.start_high;

        for _ in 0..segment.count {
            let dibit = dibits[dibit_index].bits();
            let bit = if use_high {
                (dibit >> 1) & 1
            } else {
                dibit & 1
            };
            word = (word << 1) | bit as u32;

            dibit_index += 3;
            use_high = !use_high;
        }
    }

    word
}

/// A zigzag segment: starting dibit index, whether to start with the high
/// bit, and how many bits to extract.
#[derive(Clone, Copy)]
struct Segment {
    start: usize,
    start_high: bool,
    count: usize,
}

impl Segment {
    const fn hi(start: usize, count: usize) -> Self {
        Self {
            start,
            start_high: true,
            count,
        }
    }

    const fn lo(start: usize, count: usize) -> Self {
        Self {
            start,
            start_high: false,
            count,
        }
    }

    const fn empty() -> Self {
        Self {
            start: 0,
            start_high: false,
            count: 0,
        }
    }
}

/// Zigzag segment definitions for each chunk u_0 through u_7.
///
/// Each chunk is composed of 1 or 2 zigzag segments. The segments are
/// concatenated to form the complete chunk bits (MSB first).
const CHUNKS: [[Segment; 2]; 8] = [
    // u_0: 23 bits starting at dibit 0, high bit
    [Segment::hi(0, 23), Segment::empty()],
    // u_1: 1 bit from dibit 69 low, then 22 bits from dibit 0 low
    [Segment::lo(69, 1), Segment::lo(0, 22)],
    // u_2: 2 bits from dibit 66 low, then 21 bits from dibit 1 high
    [Segment::lo(66, 2), Segment::hi(1, 21)],
    // u_3: 3 bits from dibit 64 low, then 20 bits from dibit 1 low
    [Segment::lo(64, 3), Segment::lo(1, 20)],
    // u_4: 4 bits from dibit 61 low, then 11 bits from dibit 2 high
    [Segment::lo(61, 4), Segment::hi(2, 11)],
    // u_5: 13 bits from dibit 35 low, then 2 bits from dibit 2 low
    [Segment::lo(35, 13), Segment::lo(2, 2)],
    // u_6: 15 bits from dibit 8 low
    [Segment::lo(8, 15), Segment::empty()],
    // u_7: 7 bits from dibit 53 high
    [Segment::hi(53, 7), Segment::empty()],
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_dibit_visited_exactly_twice() {
        // Each dibit has 2 bits (hi and lo). All 144 bits across 72 dibits
        // must be extracted exactly once across all 8 chunks.
        let mut visited = [0u32; VOICE_FRAME_DIBITS];

        for chunk_segments in &CHUNKS {
            for segment in chunk_segments {
                if segment.count == 0 {
                    continue;
                }
                let mut idx = segment.start;
                for _ in 0..segment.count {
                    visited[idx] += 1;
                    idx += 3;
                }
            }
        }

        for (i, &count) in visited.iter().enumerate() {
            assert_eq!(count, 2, "dibit {i} visited {count} times, expected 2");
        }
    }

    #[test]
    fn chunk_sizes_correct() {
        let expected_sizes: [usize; 8] = [23, 23, 23, 23, 15, 15, 15, 7];

        for (i, expected) in expected_sizes.iter().enumerate() {
            let total: usize = CHUNKS[i].iter().map(|s| s.count).sum();
            assert_eq!(total, *expected, "chunk u_{i} size mismatch");
        }
    }

    #[test]
    fn total_bits_equals_144() {
        let total: usize = CHUNKS
            .iter()
            .flat_map(|segs| segs.iter())
            .map(|s| s.count)
            .sum();
        assert_eq!(total, 144);
    }

    #[test]
    fn descramble_all_zeros() {
        let dibits = [Dibit::new(0); VOICE_FRAME_DIBITS];
        for i in 0..8 {
            assert_eq!(descramble(&dibits, i), 0, "chunk u_{i} should be 0");
        }
    }

    #[test]
    fn descramble_all_ones() {
        // All dibits = 0b11, so both hi and lo bits are 1.
        let dibits = [Dibit::new(3); VOICE_FRAME_DIBITS];
        let expected: [u32; 8] = [
            (1 << 23) - 1, // u_0: 23 ones
            (1 << 23) - 1, // u_1: 23 ones
            (1 << 23) - 1, // u_2: 23 ones
            (1 << 23) - 1, // u_3: 23 ones
            (1 << 15) - 1, // u_4: 15 ones
            (1 << 15) - 1, // u_5: 15 ones
            (1 << 15) - 1, // u_6: 15 ones
            (1 << 7) - 1,  // u_7: 7 ones
        ];

        for i in 0..8 {
            assert_eq!(
                descramble(&dibits, i),
                expected[i],
                "chunk u_{i} all-ones mismatch"
            );
        }
    }

    #[test]
    fn no_bit_overlap_between_chunks() {
        // Set each dibit to a unique pattern and verify chunks don't share
        // the same bit extraction.
        // We verify this indirectly: if we set a single dibit to 0b11 and
        // all others to 0b00, exactly 2 chunks should have non-zero output
        // (one chunk gets the hi bit, another gets the lo bit).
        for target in 0..VOICE_FRAME_DIBITS {
            let mut dibits = [Dibit::new(0); VOICE_FRAME_DIBITS];
            dibits[target] = Dibit::new(3);

            let mut nonzero_chunks = 0;
            for i in 0..8 {
                if descramble(&dibits, i) != 0 {
                    nonzero_chunks += 1;
                }
            }
            assert_eq!(
                nonzero_chunks, 2,
                "dibit {target} should affect exactly 2 chunks"
            );
        }
    }

    #[test]
    fn u0_extracts_high_bits_from_stride_3() {
        // u_0 starts at dibit 0, high bit, stride 3, for 23 bits.
        // Set dibit 0 to 0b10 (hi=1, lo=0), all others to 0.
        let mut dibits = [Dibit::new(0); VOICE_FRAME_DIBITS];
        dibits[0] = Dibit::new(0b10); // hi=1

        let u0 = descramble(&dibits, 0);
        // u_0 MSB should be 1, all others 0.
        assert_eq!(u0, 1 << 22);
    }
}

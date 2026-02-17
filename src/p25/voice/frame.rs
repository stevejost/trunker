//! IMBE voice frame decoder.
//!
//! Decodes a single IMBE voice frame from 72 interleaved, PN-scrambled
//! dibits into 8 data chunks (u_0..u_7) totaling 88 bits of IMBE data.
//!
//! The decoding pipeline for each frame:
//! 1. Zigzag deinterleave to extract coded chunks
//! 2. Golay(23,12) decode u_0 to recover the 12-bit PN seed
//! 3. Generate PN sequence and XOR-descramble u_1..u_6
//! 4. Golay(23,12) decode u_1..u_3 (higher-priority voice data)
//! 5. Hamming(15,11) decode u_4..u_6 (lower-priority voice data)
//! 6. u_7 passes through unprotected (7 bits)

use crate::p25::coding::{golay, hamming};
use crate::p25::error::P25Error;
use crate::p25::types::Dibit;
use super::descramble::{self, VOICE_FRAME_DIBITS};
use super::pn::PseudoRandom;

/// Decoded IMBE voice frame containing 88 bits of voice data.
#[derive(Debug)]
pub struct VoiceFrame {
    /// Data chunks u_0 through u_7.
    ///
    /// - u_0..u_3: 12 bits each (Golay-decoded)
    /// - u_4..u_6: 11 bits each (Hamming-decoded)
    /// - u_7: 7 bits (unprotected)
    pub chunks: [u32; 8],

    /// Number of FEC errors corrected in each chunk u_0 through u_6.
    ///
    /// u_7 has no FEC so it is not included.
    pub errors: [usize; 7],
}

impl VoiceFrame {
    /// Decode a voice frame from 72 coded, PN-scrambled, interleaved dibits.
    ///
    /// Returns the decoded frame with IMBE data chunks and error counts,
    /// or an error if any FEC decode fails unrecoverably.
    pub fn decode(dibits: &[Dibit; VOICE_FRAME_DIBITS]) -> Result<Self, P25Error> {
        let mut chunks = [0u32; 8];
        let mut errors = [0usize; 7];

        // Step 1: Decode u_0 (no PN scrambling) to get the 12-bit PN seed.
        let (seed, err) = golay::standard::decode(descramble::descramble(dibits, 0))
            .ok_or(P25Error::VoiceGolayDecode { chunk: 0 })?;

        chunks[0] = seed as u32;
        errors[0] = err;

        // Step 2: Initialize PN generator from the seed.
        let mut pn = PseudoRandom::new(seed);

        // Step 3: Decode u_1..u_3 (Golay-protected, PN-scrambled).
        for index in 1..=3 {
            let scrambled = descramble::descramble(dibits, index);
            let coded = scrambled ^ pn.next_23();

            let (data, err) = golay::standard::decode(coded)
                .ok_or(P25Error::VoiceGolayDecode { chunk: index })?;

            chunks[index] = data as u32;
            errors[index] = err;
        }

        // Step 4: Decode u_4..u_6 (Hamming-protected, PN-scrambled).
        for index in 4..=6 {
            let scrambled = descramble::descramble(dibits, index);
            let coded = scrambled ^ pn.next_15();

            let (data, err) = hamming::standard::decode(coded as u16)
                .ok_or(P25Error::VoiceHammingDecode { chunk: index })?;

            chunks[index] = data as u32;
            errors[index] = err;
        }

        // Step 5: u_7 is unprotected (7 bits, no FEC, no PN).
        chunks[7] = descramble::descramble(dibits, 7);

        Ok(Self { chunks, errors })
    }

    /// Total number of FEC errors corrected across all protected chunks.
    pub fn total_errors(&self) -> usize {
        self.errors.iter().sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p25::coding::{golay, hamming};
    use crate::p25::voice::pn::PseudoRandom;

    /// Build a clean (error-free) voice frame from known chunk values.
    ///
    /// This reverses the decode pipeline: encode chunks with FEC, apply PN
    /// scrambling, and interleave into 72 dibits.
    fn build_test_frame(data_chunks: &[u32; 8]) -> [Dibit; VOICE_FRAME_DIBITS] {
        let seed = data_chunks[0] as u16;
        let mut pn = PseudoRandom::new(seed);

        // Encode each chunk with FEC and apply PN scrambling.
        let mut coded = [0u32; 8];

        // u_0: Golay encode, no PN
        coded[0] = golay::standard::encode(seed);

        // u_1..u_3: Golay encode, then XOR with PN
        for i in 1..=3 {
            let golay_coded = golay::standard::encode(data_chunks[i] as u16);
            coded[i] = golay_coded ^ pn.next_23();
        }

        // u_4..u_6: Hamming encode, then XOR with PN
        for i in 4..=6 {
            let hamming_coded = hamming::standard::encode(data_chunks[i] as u16);
            coded[i] = hamming_coded as u32 ^ pn.next_15();
        }

        // u_7: no FEC, no PN
        coded[7] = data_chunks[7];

        // Reverse the zigzag deinterleave to produce dibits.
        scramble_into_dibits(&coded)
    }

    /// Reverse the zigzag interleave: place coded bits into dibit positions.
    fn scramble_into_dibits(coded: &[u32; 8]) -> [Dibit; VOICE_FRAME_DIBITS] {
        // Track hi and lo bits separately for each dibit position.
        let mut hi_bits = [0u8; VOICE_FRAME_DIBITS];
        let mut lo_bits = [0u8; VOICE_FRAME_DIBITS];

        // Chunk bit widths
        let widths = [23, 23, 23, 23, 15, 15, 15, 7];

        // Zigzag segments matching descramble.rs CHUNKS table
        let segments: [&[(usize, bool, usize)]; 8] = [
            &[(0, true, 23)],
            &[(69, false, 1), (0, false, 22)],
            &[(66, false, 2), (1, true, 21)],
            &[(64, false, 3), (1, false, 20)],
            &[(61, false, 4), (2, true, 11)],
            &[(35, false, 13), (2, false, 2)],
            &[(8, false, 15)],
            &[(53, true, 7)],
        ];

        for (chunk_idx, chunk_segments) in segments.iter().enumerate() {
            let chunk_val = coded[chunk_idx];
            let width = widths[chunk_idx];
            let mut bit_pos = width; // count down from MSB

            for &(start, start_high, count) in *chunk_segments {
                let mut dibit_idx = start;
                let mut use_high = start_high;

                for _ in 0..count {
                    bit_pos -= 1;
                    let bit = ((chunk_val >> bit_pos) & 1) as u8;

                    if use_high {
                        hi_bits[dibit_idx] = bit;
                    } else {
                        lo_bits[dibit_idx] = bit;
                    }

                    dibit_idx += 3;
                    use_high = !use_high;
                }
            }
        }

        // Combine hi and lo bits into dibits.
        let mut dibits = [Dibit::new(0); VOICE_FRAME_DIBITS];
        for i in 0..VOICE_FRAME_DIBITS {
            dibits[i] = Dibit::new((hi_bits[i] << 1) | lo_bits[i]);
        }
        dibits
    }

    #[test]
    fn decode_clean_frame_all_zeros() {
        let data = [0u32; 8];
        let dibits = build_test_frame(&data);
        let frame = VoiceFrame::decode(&dibits).unwrap();

        for i in 0..8 {
            assert_eq!(frame.chunks[i], 0, "chunk u_{i} should be 0");
        }
        assert_eq!(frame.total_errors(), 0);
    }

    #[test]
    fn decode_clean_frame_known_data() {
        let data = [
            0x123, // u_0: 12-bit seed (also PN seed)
            0x456, // u_1: 12-bit Golay data
            0x789, // u_2: 12-bit Golay data
            0xABC, // u_3: 12-bit Golay data
            0x3FF, // u_4: 11-bit Hamming data
            0x555, // u_5: 11-bit Hamming data
            0x000, // u_6: 11-bit Hamming data
            0x7F,  // u_7: 7-bit unprotected
        ];
        let dibits = build_test_frame(&data);
        let frame = VoiceFrame::decode(&dibits).unwrap();

        for i in 0..8 {
            assert_eq!(
                frame.chunks[i], data[i],
                "chunk u_{i} mismatch: got {:#X}, expected {:#X}",
                frame.chunks[i], data[i]
            );
        }
        assert_eq!(frame.total_errors(), 0);
    }

    #[test]
    fn decode_frame_with_single_bit_errors() {
        let data = [0x100, 0x200, 0x300, 0x400, 0x100, 0x200, 0x300, 0x55];
        let mut dibits = build_test_frame(&data);

        // Flip one bit in some dibits (simulating channel errors).
        // Golay and Hamming can correct 1-bit errors.
        dibits[0] = Dibit::new(dibits[0].bits() ^ 1);
        dibits[10] = Dibit::new(dibits[10].bits() ^ 2);

        let frame = VoiceFrame::decode(&dibits).unwrap();

        // Even with bit errors, the data should decode correctly.
        for i in 0..8 {
            assert_eq!(
                frame.chunks[i], data[i],
                "chunk u_{i} mismatch after error correction"
            );
        }
        // Should have some corrected errors
        assert!(frame.total_errors() > 0);
    }

    #[test]
    fn decode_preserves_u7_unprotected() {
        // u_7 has no FEC or PN, so it should pass through directly.
        for val in [0u32, 0x55, 0x7F] {
            let mut data = [0u32; 8];
            data[7] = val;
            let dibits = build_test_frame(&data);
            let frame = VoiceFrame::decode(&dibits).unwrap();
            assert_eq!(frame.chunks[7], val, "u_7 should be {val:#X}");
        }
    }

    #[test]
    fn total_errors_sums_all_chunks() {
        let data = [0u32; 8];
        let dibits = build_test_frame(&data);
        let frame = VoiceFrame::decode(&dibits).unwrap();
        assert_eq!(frame.total_errors(), 0);
        assert_eq!(
            frame.total_errors(),
            frame.errors.iter().sum::<usize>()
        );
    }

    #[test]
    fn seed_determines_pn_sequence() {
        // Two frames with different seeds should produce different PN masks,
        // so the same coded u_1 bits would decode to different data.
        let data_a = [0x001, 0x100, 0, 0, 0, 0, 0, 0];
        let data_b = [0x002, 0x100, 0, 0, 0, 0, 0, 0];

        let dibits_a = build_test_frame(&data_a);
        let dibits_b = build_test_frame(&data_b);

        let frame_a = VoiceFrame::decode(&dibits_a).unwrap();
        let frame_b = VoiceFrame::decode(&dibits_b).unwrap();

        assert_eq!(frame_a.chunks[0], 0x001);
        assert_eq!(frame_b.chunks[0], 0x002);
        assert_eq!(frame_a.chunks[1], 0x100);
        assert_eq!(frame_b.chunks[1], 0x100);
    }
}

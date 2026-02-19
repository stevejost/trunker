//! Received IMBE voice frame types for the vocoder.
//!
//! These types represent decoded IMBE voice data ready for vocoder processing.
//! The upstream P25 layer produces [`crate::p25::voice::frame::VoiceFrame`] from
//! the RF pipeline; a zero-cost [`From`] conversion bridges it into the vocoder's
//! [`ReceivedFrame`].

use super::consts::SAMPLES_PER_FRAME;

/// Prioritized bit vector chunks u_0 through u_7.
///
/// - u_0..u_3: 12 bits each (Golay-decoded)
/// - u_4..u_6: 11 bits each (Hamming-decoded)
/// - u_7: 7 bits (unprotected)
pub type Chunks = [u32; 8];

/// Error correction counts for chunks u_0 through u_6.
///
/// Each element is the number of bit errors detected and corrected by
/// Golay (u_0..u_3) or Hamming (u_4..u_6) decoding. u_7 has no FEC.
pub type Errors = [usize; 7];

/// Audio sample buffer for one decoded voice frame (160 samples at 8 kHz = 20 ms).
pub type AudioBuffer = [f32; SAMPLES_PER_FRAME];

/// A received IMBE voice frame ready for vocoder decoding.
///
/// Contains the 8 prioritized bit vector chunks extracted from the P25 voice
/// frame after FEC decoding, plus the per-chunk error counts.
#[derive(Debug, Clone)]
pub struct ReceivedFrame {
    /// Prioritized bit vector chunks u_0 through u_7.
    pub chunks: Chunks,
    /// FEC error correction counts for u_0 through u_6.
    pub errors: Errors,
}

impl ReceivedFrame {
    /// Create a new `ReceivedFrame` from decoded chunks and error counts.
    ///
    /// In debug builds, asserts that each chunk fits within its expected bit width:
    /// - u_0..u_3: 12 bits
    /// - u_4..u_6: 11 bits
    /// - u_7: 7 bits
    pub fn new(chunks: Chunks, errors: Errors) -> Self {
        for (i, &chunk) in chunks[..4].iter().enumerate() {
            debug_assert!(
                chunk >> 12 == 0,
                "chunk u_{i} exceeds 12 bits: {chunk:#X}",
            );
        }
        for (i, &chunk) in chunks[4..7].iter().enumerate() {
            debug_assert!(
                chunk >> 11 == 0,
                "chunk u_{} exceeds 11 bits: {chunk:#X}",
                i + 4,
            );
        }
        debug_assert!(
            chunks[7] >> 7 == 0,
            "chunk u_7 exceeds 7 bits: {:#X}",
            chunks[7]
        );

        Self { chunks, errors }
    }
}

impl From<&crate::p25::voice::frame::VoiceFrame> for ReceivedFrame {
    /// Convert a P25 voice frame into a vocoder received frame.
    ///
    /// This is a zero-cost conversion since both types share identical
    /// field layouts (`chunks: [u32; 8]`, `errors: [usize; 7]`).
    fn from(voice_frame: &crate::p25::voice::frame::VoiceFrame) -> Self {
        Self {
            chunks: voice_frame.chunks,
            errors: voice_frame.errors,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_accepts_valid_chunks() {
        let chunks: Chunks = [0x123, 0x456, 0x789, 0xABC, 0x3FF, 0x555, 0x000, 0x7F];
        let errors: Errors = [0; 7];
        let frame = ReceivedFrame::new(chunks, errors);
        assert_eq!(frame.chunks, chunks);
        assert_eq!(frame.errors, errors);
    }

    #[test]
    fn new_accepts_all_zeros() {
        let frame = ReceivedFrame::new([0; 8], [0; 7]);
        assert_eq!(frame.chunks, [0; 8]);
        assert_eq!(frame.errors, [0; 7]);
    }

    #[test]
    fn new_accepts_maximum_valid_values() {
        let chunks: Chunks = [
            0xFFF, // 12-bit max
            0xFFF, // 12-bit max
            0xFFF, // 12-bit max
            0xFFF, // 12-bit max
            0x7FF, // 11-bit max
            0x7FF, // 11-bit max
            0x7FF, // 11-bit max
            0x7F,  // 7-bit max
        ];
        let frame = ReceivedFrame::new(chunks, [0; 7]);
        assert_eq!(frame.chunks, chunks);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "exceeds 12 bits")]
    fn new_rejects_oversized_golay_chunk_in_debug() {
        ReceivedFrame::new([0x1000, 0, 0, 0, 0, 0, 0, 0], [0; 7]);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "exceeds 11 bits")]
    fn new_rejects_oversized_hamming_chunk_in_debug() {
        ReceivedFrame::new([0, 0, 0, 0, 0x800, 0, 0, 0], [0; 7]);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "exceeds 7 bits")]
    fn new_rejects_oversized_u7_chunk_in_debug() {
        ReceivedFrame::new([0, 0, 0, 0, 0, 0, 0, 0x80], [0; 7]);
    }

    #[test]
    fn from_voice_frame_copies_fields() {
        use crate::p25::voice::frame::VoiceFrame;

        let voice_frame = VoiceFrame {
            chunks: [0x100, 0x200, 0x300, 0x400, 0x100, 0x200, 0x300, 0x55],
            errors: [1, 0, 2, 0, 1, 0, 0],
        };
        let received = ReceivedFrame::from(&voice_frame);
        assert_eq!(received.chunks, voice_frame.chunks);
        assert_eq!(received.errors, voice_frame.errors);
    }

    #[test]
    fn audio_buffer_is_correct_size() {
        let buffer: AudioBuffer = [0.0; SAMPLES_PER_FRAME];
        assert_eq!(buffer.len(), 160);
    }
}

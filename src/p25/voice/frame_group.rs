//! LDU frame group receiver for P25 voice transmissions.
//!
//! Each frame group (LDU1 or LDU2) contains:
//! - 9 IMBE voice frames (72 dibits each)
//! - 6 "extra" pieces (20 dibits each, carrying LC or CC data)
//! - 1 low-speed data fragment (16 dibits = 2 cyclic code words)
//!
//! The structure within a frame group:
//! ```text
//! [frame 1] [frame 2] [extra 1] [frame 3] [extra 2] [frame 4] [extra 3]
//! [frame 5] [extra 4] [frame 6] [extra 5] [frame 7] [extra 6]
//! [frame 8] [data frag] [frame 9]
//! ```
//!
//! Extra pieces are Hamming(10,6,3)-decoded into hexbits, then the 24
//! hexbits are Reed-Solomon decoded into either Link Control (9 bytes,
//! RS short) or Crypto Control (12 bytes, RS medium).

use crate::p25::coding::{cyclic, hamming, reed_solomon};
use crate::p25::consts::{
    DATA_FRAG_DIBITS, EXTRA_HEXBITS, EXTRA_PIECE_DIBITS, EXTRA_WORD_DIBITS, VOICE_FRAME_DIBITS,
};
use crate::p25::error::P25Error;
use crate::p25::types::{Dibit, Hexbit};
use crate::p25::voice::control::{LinkControlFields, LINK_CONTROL_BYTES};
use crate::p25::voice::crypto::{CryptoControlFields, CRYPTO_CONTROL_BYTES};
use crate::p25::voice::frame::VoiceFrame;

// ---------------------------------------------------------------------------
// Frame group events
// ---------------------------------------------------------------------------

/// Events produced during frame group decoding.
#[derive(Debug)]
pub enum FrameGroupEvent {
    /// A decoded IMBE voice frame.
    VoiceFrame(VoiceFrame),
    /// A decoded Link Control word (from LDU1).
    LinkControl(LinkControlFields),
    /// A decoded Crypto Control word (from LDU2).
    CryptoControl(CryptoControlFields),
    /// A 16-bit low-speed data fragment.
    DataFragment(u16),
    /// A non-fatal decode error.
    Error(P25Error),
}

// ---------------------------------------------------------------------------
// Extra packet decoder (shared between LC and CC)
// ---------------------------------------------------------------------------

/// Accumulates extra pieces and decodes hexbits for the LC/CC payload.
///
/// Each of the 6 extra pieces is 20 dibits = 4 coded words of 5 dibits
/// each. Each 5-dibit word decodes via Hamming(10,6,3) to one hexbit.
/// After all 6 pieces (24 hexbits), Reed-Solomon decoding recovers the
/// payload bytes.
struct ExtraReceiver {
    /// Accumulated hexbits from all pieces so far.
    hexbits: [Hexbit; EXTRA_HEXBITS],
    /// Number of hexbits collected.
    hexbit_count: usize,
    /// Dibit accumulator for the current 5-dibit word.
    word_accumulator: u16,
    /// Number of dibits in the current word.
    word_dibit_count: usize,
}

impl ExtraReceiver {
    fn new() -> Self {
        Self {
            hexbits: [Hexbit::default(); EXTRA_HEXBITS],
            hexbit_count: 0,
            word_accumulator: 0,
            word_dibit_count: 0,
        }
    }

    /// Feed one dibit. Returns `true` when the current piece is complete
    /// (every 20 dibits).
    fn feed(&mut self, dibit: Dibit) -> bool {
        self.word_accumulator = (self.word_accumulator << 2) | u16::from(dibit.bits());
        self.word_dibit_count += 1;

        if self.word_dibit_count < EXTRA_WORD_DIBITS {
            return false;
        }

        // Complete 5-dibit word (10 bits) -- decode via Hamming.
        let word = self.word_accumulator & 0x03FF;
        self.word_accumulator = 0;
        self.word_dibit_count = 0;

        let hexbit_value = match hamming::shortened::decode(word) {
            Some((data, _errors)) => data,
            // Let RS attempt to fix this; store 0.
            None => 0,
        };

        if self.hexbit_count < EXTRA_HEXBITS {
            self.hexbits[self.hexbit_count] = Hexbit::new(hexbit_value);
            self.hexbit_count += 1;
        }

        // A piece is 20 dibits = 4 words. Check if we're at a piece boundary.
        self.hexbit_count.is_multiple_of(4)
    }

    /// Whether all 24 hexbits have been collected.
    fn complete(&self) -> bool {
        self.hexbit_count >= EXTRA_HEXBITS
    }

    /// Decode as Link Control (RS short). Consumes the accumulated hexbits.
    fn decode_link_control(&mut self) -> Result<LinkControlFields, P25Error> {
        let (_data, _errors) = reed_solomon::short::decode(&mut self.hexbits)
            .ok_or(P25Error::RsShortUnrecoverable)?;

        // Convert first 12 hexbits (72 bits) to 9 bytes.
        let mut bytes = [0u8; LINK_CONTROL_BYTES];
        hexbits_to_bytes(&self.hexbits[..12], &mut bytes);
        Ok(LinkControlFields::new(bytes))
    }

    /// Decode as Crypto Control (RS medium). Consumes the accumulated hexbits.
    fn decode_crypto_control(&mut self) -> Result<CryptoControlFields, P25Error> {
        let (_data, _errors) = reed_solomon::medium::decode(&mut self.hexbits)
            .ok_or(P25Error::RsMediumUnrecoverable)?;

        // Convert first 16 hexbits (96 bits) to 12 bytes.
        let mut bytes = [0u8; CRYPTO_CONTROL_BYTES];
        hexbits_to_bytes(&self.hexbits[..16], &mut bytes);
        Ok(CryptoControlFields::new(bytes))
    }
}

/// Convert a slice of hexbits (6-bit symbols) into packed bytes.
///
/// Processes hexbits in pairs: each pair of 6-bit values produces
/// 12 bits = 1.5 bytes. The overall conversion packs N hexbits into
/// ceil(N * 6 / 8) bytes.
fn hexbits_to_bytes(hexbits: &[Hexbit], bytes: &mut [u8]) {
    let mut bit_accumulator: u32 = 0;
    let mut bits_available: usize = 0;
    let mut byte_index = 0;

    for hexbit in hexbits {
        bit_accumulator = (bit_accumulator << 6) | u32::from(hexbit.bits());
        bits_available += 6;

        while bits_available >= 8 && byte_index < bytes.len() {
            bits_available -= 8;
            bytes[byte_index] = ((bit_accumulator >> bits_available) & 0xFF) as u8;
            byte_index += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Data fragment decoder
// ---------------------------------------------------------------------------

/// Accumulates the low-speed data fragment and decodes via cyclic code.
///
/// Per TIA-102.BAAA-B Figures 31/32: "LSD, 32 bits, 2 cyclic code words".
/// Each code word is 8 dibits (16 bits), decoded via cyclic(16,8,5) to one
/// data byte. The two bytes form a 16-bit low-speed data word.
struct DataFragmentReceiver {
    /// Dibit accumulator for the current coded word.
    word_accumulator: u16,
    /// Number of dibits in the current word.
    word_dibit_count: usize,
    /// First decoded byte (high byte of the result), if already decoded.
    first_byte: Option<u8>,
}

impl DataFragmentReceiver {
    fn new() -> Self {
        Self {
            word_accumulator: 0,
            word_dibit_count: 0,
            first_byte: None,
        }
    }

    /// Feed one dibit. Returns `Some(value)` when both cyclic code words
    /// have been decoded, or `Some(Err)` if cyclic decode fails.
    fn feed(&mut self, dibit: Dibit) -> Option<Result<u16, P25Error>> {
        self.word_accumulator = (self.word_accumulator << 2) | u16::from(dibit.bits());
        self.word_dibit_count += 1;

        if self.word_dibit_count < DATA_FRAG_DIBITS {
            return None;
        }

        // One complete 8-dibit (16-bit) cyclic codeword collected.
        let word = self.word_accumulator;
        self.word_accumulator = 0;
        self.word_dibit_count = 0;

        let data_byte = match cyclic::decode(word) {
            Some((data, _errors)) => data,
            None => return Some(Err(P25Error::CyclicUnrecoverable)),
        };

        match self.first_byte {
            None => {
                // First codeword decoded — store high byte, wait for second.
                self.first_byte = Some(data_byte);
                None
            }
            Some(hi) => {
                // Second codeword decoded — combine into 16-bit result.
                let result = (u16::from(hi) << 8) | u16::from(data_byte);
                Some(Ok(result))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Frame group state machine
// ---------------------------------------------------------------------------

/// Internal state of the frame group receiver.
enum State {
    /// Collecting dibits for a voice frame.
    VoiceFrame,
    /// Collecting dibits for an extra piece.
    Extra,
    /// Collecting dibits for the low-speed data fragment.
    DataFragment,
    /// Frame group decoding is complete.
    Done,
}

/// The type of extra data carried in this frame group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameGroupType {
    /// LDU1: carries Link Control (RS short).
    LinkControl,
    /// LDU2: carries Crypto Control (RS medium).
    CryptoControl,
}

/// Receives and decodes a complete LDU frame group (LDU1 or LDU2).
///
/// Feed dibits one at a time via [`feed()`](Self::feed). The receiver
/// tracks its position within the frame group structure and emits events
/// as voice frames, extra packets, and data fragments are decoded.
pub struct FrameGroupReceiver {
    /// What type of extra data we're decoding.
    group_type: FrameGroupType,
    /// Current state in the frame group structure.
    state: State,
    /// Current voice frame dibit buffer (fixed-size, indexed by `frame_buffer_count`).
    frame_buffer: [Dibit; VOICE_FRAME_DIBITS],
    /// Number of dibits collected in the current frame.
    frame_buffer_count: usize,
    /// Extra piece accumulator (persists across pieces).
    extra: ExtraReceiver,
    /// Data fragment decoder.
    data_fragment: DataFragmentReceiver,
    /// Dibit count within the current extra piece.
    extra_dibit_count: usize,
    /// Which voice frame we're on (0-indexed, 0..9).
    frame_index: usize,
}

impl FrameGroupReceiver {
    /// Create a new frame group receiver for the given type.
    pub fn new(group_type: FrameGroupType) -> Self {
        Self {
            group_type,
            state: State::VoiceFrame,
            frame_buffer: [Dibit::new(0); VOICE_FRAME_DIBITS],
            frame_buffer_count: 0,
            extra: ExtraReceiver::new(),
            data_fragment: DataFragmentReceiver::new(),
            extra_dibit_count: 0,
            frame_index: 0,
        }
    }

    /// Whether the full frame group (all 9 frames) has been received.
    pub fn is_done(&self) -> bool {
        matches!(self.state, State::Done)
    }

    /// Feed one data dibit into the receiver.
    ///
    /// Returns an event when a component is decoded, or `None` when more
    /// dibits are needed.
    pub fn feed(&mut self, dibit: Dibit) -> Option<FrameGroupEvent> {
        match self.state {
            State::VoiceFrame => self.feed_voice_frame(dibit),
            State::Extra => self.feed_extra(dibit),
            State::DataFragment => self.feed_data_fragment(dibit),
            State::Done => None,
        }
    }

    /// Collect a voice frame dibit.
    fn feed_voice_frame(&mut self, dibit: Dibit) -> Option<FrameGroupEvent> {
        self.frame_buffer[self.frame_buffer_count] = dibit;
        self.frame_buffer_count += 1;

        if self.frame_buffer_count < VOICE_FRAME_DIBITS {
            return None;
        }

        // Decode the complete voice frame.
        let frame_array = self.frame_buffer;
        self.frame_buffer_count = 0;

        self.frame_index += 1;

        // Determine next state based on frame position.
        self.state = match self.frame_index {
            1 => State::VoiceFrame,       // After frame 1: frame 2
            2..=7 => State::Extra,        // After frames 2-7: extra piece
            8 => State::DataFragment,     // After frame 8: data fragment
            9 => State::Done,             // After frame 9: done
            _ => State::Done,
        };

        match VoiceFrame::decode(&frame_array) {
            Ok(frame) => Some(FrameGroupEvent::VoiceFrame(frame)),
            Err(err) => Some(FrameGroupEvent::Error(err)),
        }
    }

    /// Collect an extra piece dibit.
    fn feed_extra(&mut self, dibit: Dibit) -> Option<FrameGroupEvent> {
        self.extra.feed(dibit);
        self.extra_dibit_count += 1;

        if self.extra_dibit_count >= EXTRA_PIECE_DIBITS {
            // Piece complete, transition to next voice frame.
            self.extra_dibit_count = 0;
            self.state = State::VoiceFrame;

            // If all 6 pieces collected, decode the extra payload.
            if self.extra.complete() {
                let event = match self.group_type {
                    FrameGroupType::LinkControl => match self.extra.decode_link_control() {
                        Ok(lc) => FrameGroupEvent::LinkControl(lc),
                        Err(err) => FrameGroupEvent::Error(err),
                    },
                    FrameGroupType::CryptoControl => {
                        match self.extra.decode_crypto_control() {
                            Ok(cc) => FrameGroupEvent::CryptoControl(cc),
                            Err(err) => FrameGroupEvent::Error(err),
                        }
                    }
                };
                return Some(event);
            }
        }

        None
    }

    /// Collect a data fragment dibit.
    fn feed_data_fragment(&mut self, dibit: Dibit) -> Option<FrameGroupEvent> {
        match self.data_fragment.feed(dibit) {
            Some(Ok(data)) => {
                self.state = State::VoiceFrame;
                Some(FrameGroupEvent::DataFragment(data))
            }
            Some(Err(err)) => {
                self.state = State::VoiceFrame;
                Some(FrameGroupEvent::Error(err))
            }
            None => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p25::coding::{golay, hamming as hamming_code};
    use crate::p25::voice::pn::PseudoRandom;

    /// Build a clean voice frame from all-zero IMBE data.
    fn build_zero_voice_frame() -> [Dibit; VOICE_FRAME_DIBITS] {
        build_voice_frame(&[0u32; 8])
    }

    /// Build a voice frame from given chunk values (same as frame.rs test helper).
    fn build_voice_frame(data_chunks: &[u32; 8]) -> [Dibit; VOICE_FRAME_DIBITS] {
        let seed = data_chunks[0] as u16;
        let mut pn = PseudoRandom::new(seed);

        let mut coded = [0u32; 8];
        coded[0] = golay::standard::encode(seed);

        for i in 1..=3 {
            coded[i] = golay::standard::encode(data_chunks[i] as u16) ^ pn.next_23();
        }
        for i in 4..=6 {
            coded[i] = hamming_code::standard::encode(data_chunks[i] as u16) as u32 ^ pn.next_15();
        }
        coded[7] = data_chunks[7];

        scramble_into_dibits(&coded)
    }

    /// Reverse the zigzag interleave (from frame.rs tests).
    fn scramble_into_dibits(coded: &[u32; 8]) -> [Dibit; VOICE_FRAME_DIBITS] {
        let mut hi_bits = [0u8; VOICE_FRAME_DIBITS];
        let mut lo_bits = [0u8; VOICE_FRAME_DIBITS];

        let widths = [23, 23, 23, 23, 15, 15, 15, 7];
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
            let mut bit_pos = width;

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

        let mut dibits = [Dibit::new(0); VOICE_FRAME_DIBITS];
        for i in 0..VOICE_FRAME_DIBITS {
            dibits[i] = Dibit::new((hi_bits[i] << 1) | lo_bits[i]);
        }
        dibits
    }

    /// Build an extra piece (20 dibits) containing 4 Hamming-encoded hexbits.
    fn build_extra_piece(hexbits: &[u8; 4]) -> Vec<Dibit> {
        let mut dibits = Vec::with_capacity(EXTRA_PIECE_DIBITS);
        for &hb in hexbits {
            let coded = hamming_code::shortened::encode(hb & 0x3F);
            // 10 bits -> 5 dibits
            for i in (0..5).rev() {
                dibits.push(Dibit::new(((coded >> (i * 2)) & 0x03) as u8));
            }
        }
        dibits
    }

    /// Build a data fragment (16 dibits = 2 cyclic code words) from two bytes.
    fn build_data_fragment(hi_byte: u8, lo_byte: u8) -> Vec<Dibit> {
        let mut dibits = Vec::with_capacity(DATA_FRAG_DIBITS * 2);
        for &byte_val in &[hi_byte, lo_byte] {
            let coded = cyclic::encode(byte_val);
            for i in (0..8).rev() {
                dibits.push(Dibit::new(((coded >> (i * 2)) & 0x03) as u8));
            }
        }
        dibits
    }

    #[test]
    fn hexbits_to_bytes_basic() {
        // 12 hexbits (72 bits) -> 9 bytes
        let hexbits: Vec<Hexbit> = (0..12).map(|i| Hexbit::new(i)).collect();
        let mut bytes = [0u8; 9];
        hexbits_to_bytes(&hexbits, &mut bytes);

        // hexbit 0 = 0b000000, hexbit 1 = 0b000001 -> 12 bits: 000000_000001
        //   byte 0 = 0b00000000 = 0x00, remaining 4 bits = 0001
        // hexbit 2 = 0b000010 -> 10 bits: 0001_000010
        //   byte 1 = 0b00010000 = 0x10, remaining 2 bits = 10
        // hexbit 3 = 0b000011 -> 8 bits: 10_000011
        //   byte 2 = 0b10000011 = 0x83
        assert_eq!(bytes[0], 0x00);
        assert_eq!(bytes[1], 0x10);
        assert_eq!(bytes[2], 0x83);
    }

    #[test]
    fn hexbits_to_bytes_all_ones() {
        let hexbits = [Hexbit::new(0x3F); 12];
        let mut bytes = [0u8; 9];
        hexbits_to_bytes(&hexbits, &mut bytes);
        // All bits set -> all bytes 0xFF
        assert_eq!(bytes, [0xFF; 9]);
    }

    #[test]
    fn extra_receiver_piece_boundary() {
        let mut extra = ExtraReceiver::new();
        let piece = build_extra_piece(&[0, 0, 0, 0]);

        let mut piece_done_count = 0;
        for dibit in &piece {
            if extra.feed(*dibit) {
                piece_done_count += 1;
            }
        }
        // One piece should trigger one piece_done.
        assert_eq!(piece_done_count, 1);
        assert_eq!(extra.hexbit_count, 4);
    }

    #[test]
    fn extra_receiver_full_lc_decode() {
        let mut extra = ExtraReceiver::new();

        // Build 24 hexbits: 12 data + 12 parity for RS short.
        let mut hexbit_buf = [Hexbit::default(); 24];
        // Set some data bytes that form a valid LC.
        let lc_bytes = [0x00u8, 0x00, 0xB5, 0x00, 0x00, 0x01, 0xDE, 0xAD, 0xBE];
        // Convert bytes to hexbits.
        bytes_to_hexbits(&lc_bytes, &mut hexbit_buf[..12]);
        // Encode RS parity.
        reed_solomon::short::encode(&mut hexbit_buf);

        // Build 6 extra pieces from the 24 hexbits.
        for piece_idx in 0..6 {
            let start = piece_idx * 4;
            let piece_hexbits = [
                hexbit_buf[start].bits(),
                hexbit_buf[start + 1].bits(),
                hexbit_buf[start + 2].bits(),
                hexbit_buf[start + 3].bits(),
            ];
            let piece_dibits = build_extra_piece(&piece_hexbits);

            for dibit in &piece_dibits {
                extra.feed(*dibit);
            }
        }

        assert!(extra.complete());
        let lc = extra.decode_link_control().unwrap();
        assert_eq!(lc.opcode(), crate::p25::voice::control::LinkControlOpcode::GroupVoiceTraffic);
    }

    #[test]
    fn data_fragment_decode() {
        let mut frag = DataFragmentReceiver::new();
        let dibits = build_data_fragment(0xAB, 0xCD);

        let mut result = None;
        for dibit in &dibits {
            if let Some(r) = frag.feed(*dibit) {
                result = Some(r);
            }
        }

        assert_eq!(result.unwrap().unwrap(), 0xABCD);
    }

    #[test]
    fn frame_group_receiver_counts_frames() {
        let mut recv = FrameGroupReceiver::new(FrameGroupType::LinkControl);

        let frame = build_zero_voice_frame();

        // Feed frame 1.
        let mut events = Vec::new();
        for dibit in &frame {
            if let Some(event) = recv.feed(*dibit) {
                events.push(event);
            }
        }
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], FrameGroupEvent::VoiceFrame(_)));
        assert_eq!(recv.frame_index, 1);
        assert!(!recv.is_done());
    }

    #[test]
    fn frame_group_receiver_full_ldu1() {
        let mut recv = FrameGroupReceiver::new(FrameGroupType::LinkControl);
        let mut events = Vec::new();

        let frame = build_zero_voice_frame();

        // Build valid LC extra data.
        let mut hexbit_buf = [Hexbit::default(); 24];
        let lc_bytes = [0x00u8, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x01];
        bytes_to_hexbits(&lc_bytes, &mut hexbit_buf[..12]);
        reed_solomon::short::encode(&mut hexbit_buf);

        let mut extra_pieces = Vec::new();
        for piece_idx in 0..6 {
            let start = piece_idx * 4;
            let piece_hexbits = [
                hexbit_buf[start].bits(),
                hexbit_buf[start + 1].bits(),
                hexbit_buf[start + 2].bits(),
                hexbit_buf[start + 3].bits(),
            ];
            extra_pieces.push(build_extra_piece(&piece_hexbits));
        }

        let data_frag = build_data_fragment(0x42, 0x37);

        // Frame 1
        for dibit in &frame {
            if let Some(e) = recv.feed(*dibit) { events.push(e); }
        }
        // Frame 2
        for dibit in &frame {
            if let Some(e) = recv.feed(*dibit) { events.push(e); }
        }
        // Extra 1
        for dibit in &extra_pieces[0] {
            if let Some(e) = recv.feed(*dibit) { events.push(e); }
        }
        // Frame 3
        for dibit in &frame {
            if let Some(e) = recv.feed(*dibit) { events.push(e); }
        }
        // Extra 2
        for dibit in &extra_pieces[1] {
            if let Some(e) = recv.feed(*dibit) { events.push(e); }
        }
        // Frame 4
        for dibit in &frame {
            if let Some(e) = recv.feed(*dibit) { events.push(e); }
        }
        // Extra 3
        for dibit in &extra_pieces[2] {
            if let Some(e) = recv.feed(*dibit) { events.push(e); }
        }
        // Frame 5
        for dibit in &frame {
            if let Some(e) = recv.feed(*dibit) { events.push(e); }
        }
        // Extra 4
        for dibit in &extra_pieces[3] {
            if let Some(e) = recv.feed(*dibit) { events.push(e); }
        }
        // Frame 6
        for dibit in &frame {
            if let Some(e) = recv.feed(*dibit) { events.push(e); }
        }
        // Extra 5
        for dibit in &extra_pieces[4] {
            if let Some(e) = recv.feed(*dibit) { events.push(e); }
        }
        // Frame 7
        for dibit in &frame {
            if let Some(e) = recv.feed(*dibit) { events.push(e); }
        }
        // Extra 6 (should produce LC event)
        for dibit in &extra_pieces[5] {
            if let Some(e) = recv.feed(*dibit) { events.push(e); }
        }
        // Frame 8
        for dibit in &frame {
            if let Some(e) = recv.feed(*dibit) { events.push(e); }
        }
        // Data fragment
        for dibit in &data_frag {
            if let Some(e) = recv.feed(*dibit) { events.push(e); }
        }
        // Frame 9
        for dibit in &frame {
            if let Some(e) = recv.feed(*dibit) { events.push(e); }
        }

        assert!(recv.is_done());

        // Count event types.
        let voice_frames = events.iter().filter(|e| matches!(e, FrameGroupEvent::VoiceFrame(_))).count();
        let lc_events = events.iter().filter(|e| matches!(e, FrameGroupEvent::LinkControl(_))).count();
        let data_events = events.iter().filter(|e| matches!(e, FrameGroupEvent::DataFragment(_))).count();
        let error_events = events.iter().filter(|e| matches!(e, FrameGroupEvent::Error(_))).count();

        assert_eq!(voice_frames, 9, "expected 9 voice frames");
        assert_eq!(lc_events, 1, "expected 1 link control event");
        assert_eq!(data_events, 1, "expected 1 data fragment event");
        assert_eq!(error_events, 0, "expected no errors, got: {:?}",
            events.iter().filter_map(|e| match e {
                FrameGroupEvent::Error(err) => Some(format!("{err}")),
                _ => None,
            }).collect::<Vec<_>>()
        );
    }

    /// Convert bytes to hexbits (6 bits each, MSB first).
    fn bytes_to_hexbits(bytes: &[u8], hexbits: &mut [Hexbit]) {
        let mut bit_accumulator: u32 = 0;
        let mut bits_available: usize = 0;
        let mut hexbit_index = 0;

        for &byte in bytes {
            bit_accumulator = (bit_accumulator << 8) | u32::from(byte);
            bits_available += 8;

            while bits_available >= 6 && hexbit_index < hexbits.len() {
                bits_available -= 6;
                hexbits[hexbit_index] =
                    Hexbit::new(((bit_accumulator >> bits_available) & 0x3F) as u8);
                hexbit_index += 1;
            }
        }
    }
}

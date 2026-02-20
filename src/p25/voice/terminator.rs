//! Voice LC terminator (TDULC) receiver for P25.
//!
//! The TDULC ends a voice transmission, carrying Link Control data.
//! Structure (after NID):
//! - 12 Golay-extended(24,12,8) words of 12 dibits each = 144 dibits
//! - Each word decodes to 12 data bits = 2 hexbits (high 6, low 6)
//! - 24 hexbits -> RS short(24,12,13) -> 12 data hexbits -> 9 bytes LC

use crate::p25::coding::{golay, reed_solomon};
use crate::p25::consts::{EXTRA_HEXBITS, LC_TERM_WORD_DIBITS};
use crate::p25::error::P25Error;
use crate::p25::types::{Dibit, Hexbit};
use crate::p25::voice::control::{LINK_CONTROL_BYTES, LinkControlFields};
use crate::p25::voice::frame_group::hexbits_to_bytes;

/// Streaming receiver for voice LC terminator (TDULC) data units.
///
/// Feed dibits one at a time. After every 12 dibits, decodes a
/// Golay-extended word to 12 data bits (2 hexbits). After all 12 words
/// (24 hexbits), RS short decodes to the 9-byte Link Control payload.
pub struct VoiceLcTerminatorReceiver {
    /// Accumulated hexbits from decoded Golay words.
    hexbits: [Hexbit; EXTRA_HEXBITS],
    /// Number of hexbits collected so far.
    hexbit_count: usize,
    /// Dibit accumulator for the current 12-dibit word.
    word_accumulator: u32,
    /// Number of dibits in the current word.
    word_dibit_count: usize,
}

impl VoiceLcTerminatorReceiver {
    /// Create a new receiver in its initial state.
    pub fn new() -> Self {
        Self {
            hexbits: [Hexbit::default(); EXTRA_HEXBITS],
            hexbit_count: 0,
            word_accumulator: 0,
            word_dibit_count: 0,
        }
    }

    /// Whether all 24 hexbits have been received.
    pub fn is_done(&self) -> bool {
        self.hexbit_count >= EXTRA_HEXBITS
    }

    /// Feed one dibit. Returns `Some(Ok(lc))` when the terminator is
    /// complete, `Some(Err(_))` on RS decode failure, or `None` when
    /// more dibits are needed.
    pub fn feed(&mut self, dibit: Dibit) -> Option<Result<LinkControlFields, P25Error>> {
        self.word_accumulator = (self.word_accumulator << 2) | u32::from(dibit.bits());
        self.word_dibit_count += 1;

        if self.word_dibit_count < LC_TERM_WORD_DIBITS {
            return None;
        }

        // Complete 12-dibit (24-bit) word -- decode via extended Golay.
        let word = self.word_accumulator & 0x00FFFFFF;
        self.word_accumulator = 0;
        self.word_dibit_count = 0;

        let data = match golay::extended::decode(word) {
            Some((data, _errors)) => data,
            // Let RS attempt to fix; store 0 for both hexbits.
            None => 0,
        };

        // Each 12-bit result splits into 2 hexbits (high 6, low 6).
        if self.hexbit_count < EXTRA_HEXBITS {
            self.hexbits[self.hexbit_count] = Hexbit::new((data >> 6) as u8);
            self.hexbit_count += 1;
        }
        if self.hexbit_count < EXTRA_HEXBITS {
            self.hexbits[self.hexbit_count] = Hexbit::new((data & 0x3F) as u8);
            self.hexbit_count += 1;
        }

        if self.hexbit_count < EXTRA_HEXBITS {
            return None;
        }

        // All 24 hexbits collected -- RS short decode.
        let (_data, _errors) = match reed_solomon::short::decode(&mut self.hexbits) {
            Some(result) => result,
            None => return Some(Err(P25Error::RsShortUnrecoverable)),
        };

        // Convert first 12 data hexbits (72 bits) to 9 bytes.
        let mut bytes = [0u8; LINK_CONTROL_BYTES];
        hexbits_to_bytes(&self.hexbits[..12], &mut bytes);
        Some(Ok(LinkControlFields::new(bytes)))
    }
}

impl Default for VoiceLcTerminatorReceiver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p25::coding::{golay, reed_solomon};
    use crate::p25::voice::control::LinkControlOpcode;

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

    /// Build all 144 dibits for a clean TDULC from given LC bytes.
    fn build_tdulc_dibits(lc_bytes: &[u8; LINK_CONTROL_BYTES]) -> Vec<Dibit> {
        // Convert 9 bytes to 12 data hexbits.
        let mut hexbit_buf = [Hexbit::default(); EXTRA_HEXBITS];
        bytes_to_hexbits(lc_bytes, &mut hexbit_buf[..12]);

        // RS short encode to fill parity.
        reed_solomon::short::encode(&mut hexbit_buf);

        // Group hexbits in pairs, extended Golay encode each pair.
        let mut dibits = Vec::with_capacity(12 * LC_TERM_WORD_DIBITS);
        for pair in hexbit_buf.chunks(2) {
            let data_12 = (u16::from(pair[0].bits()) << 6) | u16::from(pair[1].bits());
            let coded = golay::extended::encode(data_12);
            for i in (0..12).rev() {
                dibits.push(Dibit::new(((coded >> (i * 2)) & 0x03) as u8));
            }
        }
        dibits
    }

    #[test]
    fn decode_clean_tdulc() {
        let lc_bytes: [u8; LINK_CONTROL_BYTES] = [
            0x00, // opcode 0x00 = GroupVoiceTraffic
            0x00, // MFID
            0x00, // service options
            0x00, 0x00, 0x42, // talkgroup = 66
            0xDE, 0xAD, 0xBE, // source
        ];

        let dibits = build_tdulc_dibits(&lc_bytes);
        let mut recv = VoiceLcTerminatorReceiver::new();
        let mut result = None;

        for dibit in &dibits {
            if let Some(r) = recv.feed(*dibit) {
                result = Some(r);
            }
        }

        let lc = result.unwrap().unwrap();
        assert_eq!(lc.opcode(), LinkControlOpcode::GroupVoiceTraffic);
    }

    #[test]
    fn decode_tdulc_with_bit_errors() {
        let lc_bytes: [u8; LINK_CONTROL_BYTES] =
            [0x00, 0x00, 0x00, 0x00, 0x00, 0x42, 0xDE, 0xAD, 0xBE];

        let mut dibits = build_tdulc_dibits(&lc_bytes);

        // Flip bits in a couple of Golay words.
        dibits[0] = Dibit::new(dibits[0].bits() ^ 0x03);
        dibits[12] = Dibit::new(dibits[12].bits() ^ 0x03);

        let mut recv = VoiceLcTerminatorReceiver::new();
        let mut result = None;

        for dibit in &dibits {
            if let Some(r) = recv.feed(*dibit) {
                result = Some(r);
            }
        }

        let lc = result.unwrap().unwrap();
        assert_eq!(lc.opcode(), LinkControlOpcode::GroupVoiceTraffic);
    }

    #[test]
    fn is_done_after_all_words() {
        let lc_bytes = [0u8; LINK_CONTROL_BYTES];
        let dibits = build_tdulc_dibits(&lc_bytes);
        let mut recv = VoiceLcTerminatorReceiver::new();

        assert!(!recv.is_done());

        for dibit in &dibits {
            recv.feed(*dibit);
        }

        assert!(recv.is_done());
    }

    #[test]
    fn correct_dibit_count() {
        // 12 words x 12 dibits = 144 dibits total
        let lc_bytes = [0u8; LINK_CONTROL_BYTES];
        let dibits = build_tdulc_dibits(&lc_bytes);
        assert_eq!(dibits.len(), 144);
    }
}

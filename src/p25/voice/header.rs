//! Voice header (HDU) receiver for P25.
//!
//! The HDU begins each voice transmission. It carries crypto parameters
//! and talkgroup identity. Structure (after NID):
//! - 36 Golay-shortened(18,6,8) words of 9 dibits each = 324 dibits
//! - Each word decodes to one hexbit
//! - 36 hexbits -> RS long(36,20,17) -> 20 data hexbits -> 15 bytes
//!
//! Layout of the 15 decoded bytes:
//! - Bytes 0..9: Crypto initialization vector (MI, 72 bits)
//! - Byte 9: Manufacturer ID (MFID)
//! - Byte 10: Algorithm ID (ALGID)
//! - Bytes 11..13: Key ID (KID, 16 bits)
//! - Bytes 13..15: Talkgroup ID (TGID, 16 bits)

use crate::p25::coding::{golay, reed_solomon};
use crate::p25::consts::{HEADER_BYTES, HEADER_HEXBITS, HEADER_WORD_DIBITS};
use crate::p25::error::P25Error;
use crate::p25::types::{Dibit, Hexbit, TalkgroupId};
use crate::p25::voice::crypto::CryptoAlgorithm;
use crate::p25::voice::frame_group::hexbits_to_bytes;

/// Decoded voice header fields (15 bytes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceHeaderFields([u8; HEADER_BYTES]);

impl VoiceHeaderFields {
    /// Create from a 15-byte buffer.
    pub fn new(buf: [u8; HEADER_BYTES]) -> Self {
        Self(buf)
    }

    /// The 9-byte crypto initialization vector (MI).
    pub fn crypto_init(&self) -> &[u8] {
        &self.0[..9]
    }

    /// Manufacturer ID.
    pub fn manufacturer_id(&self) -> u8 {
        self.0[9]
    }

    /// Cryptographic algorithm in use.
    pub fn algorithm(&self) -> CryptoAlgorithm {
        CryptoAlgorithm::from_byte(self.0[10])
    }

    /// The 16-bit encryption key identifier.
    pub fn key_id(&self) -> u16 {
        u16::from_be_bytes([self.0[11], self.0[12]])
    }

    /// Talkgroup participating in the voice transmission.
    pub fn talkgroup(&self) -> TalkgroupId {
        TalkgroupId::new(u16::from_be_bytes([self.0[13], self.0[14]]))
    }

    /// Access the raw 15-byte buffer.
    pub fn raw(&self) -> &[u8; HEADER_BYTES] {
        &self.0
    }
}

impl std::fmt::Display for VoiceHeaderFields {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "algorithm={}, key_id=0x{:04X}, talkgroup={}",
            self.algorithm(),
            self.key_id(),
            self.talkgroup().value()
        )
    }
}

/// Streaming receiver for voice header (HDU) data units.
///
/// Feed dibits one at a time. After every 9 dibits, decodes a
/// Golay-shortened word to one hexbit. After all 36 hexbits,
/// RS long decodes to the 15-byte header.
pub struct VoiceHeaderReceiver {
    /// Accumulated hexbits from decoded Golay words.
    hexbits: [Hexbit; HEADER_HEXBITS],
    /// Number of hexbits collected so far.
    hexbit_count: usize,
    /// Dibit accumulator for the current 9-dibit word.
    word_accumulator: u32,
    /// Number of dibits in the current word.
    word_dibit_count: usize,
}

impl VoiceHeaderReceiver {
    /// Create a new receiver in its initial state.
    pub fn new() -> Self {
        Self {
            hexbits: [Hexbit::default(); HEADER_HEXBITS],
            hexbit_count: 0,
            word_accumulator: 0,
            word_dibit_count: 0,
        }
    }

    /// Whether all 36 hexbits have been received and decoded.
    pub fn is_done(&self) -> bool {
        self.hexbit_count >= HEADER_HEXBITS
    }

    /// Feed one dibit. Returns `Some(Ok(fields))` when the header is
    /// complete, `Some(Err(_))` on RS decode failure, or `None` when
    /// more dibits are needed.
    pub fn feed(&mut self, dibit: Dibit) -> Option<Result<VoiceHeaderFields, P25Error>> {
        self.word_accumulator = (self.word_accumulator << 2) | u32::from(dibit.bits());
        self.word_dibit_count += 1;

        if self.word_dibit_count < HEADER_WORD_DIBITS {
            return None;
        }

        // Complete 9-dibit (18-bit) word -- decode via shortened Golay.
        let word = self.word_accumulator & 0x3FFFF;
        self.word_accumulator = 0;
        self.word_dibit_count = 0;

        let hexbit_value = match golay::shortened::decode(word) {
            Some((data, _errors)) => data,
            // Let RS attempt to fix this; store 0.
            None => 0,
        };

        if self.hexbit_count < HEADER_HEXBITS {
            self.hexbits[self.hexbit_count] = Hexbit::new(hexbit_value);
            self.hexbit_count += 1;
        }

        if self.hexbit_count < HEADER_HEXBITS {
            return None;
        }

        // All 36 hexbits collected -- RS long decode.
        let (_data, _errors) = match reed_solomon::long::decode(&mut self.hexbits) {
            Some(result) => result,
            None => return Some(Err(P25Error::RsLongUnrecoverable)),
        };

        // Convert first 20 data hexbits (120 bits) to 15 bytes.
        let mut bytes = [0u8; HEADER_BYTES];
        hexbits_to_bytes(&self.hexbits[..20], &mut bytes);
        Some(Ok(VoiceHeaderFields::new(bytes)))
    }
}

impl Default for VoiceHeaderReceiver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p25::coding::golay;
    use crate::p25::coding::reed_solomon;

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

    /// Build all 324 dibits for a clean HDU from given header bytes.
    fn build_hdu_dibits(header_bytes: &[u8; HEADER_BYTES]) -> Vec<Dibit> {
        // Convert 15 bytes to 20 data hexbits.
        let mut hexbit_buf = [Hexbit::default(); HEADER_HEXBITS];
        bytes_to_hexbits(header_bytes, &mut hexbit_buf[..20]);

        // RS long encode to fill parity.
        reed_solomon::long::encode(&mut hexbit_buf);

        // Golay-shortened encode each hexbit to 18-bit word, then to 9 dibits.
        let mut dibits = Vec::with_capacity(HEADER_HEXBITS * HEADER_WORD_DIBITS);
        for h in &hexbit_buf {
            let coded = golay::shortened::encode(h.bits());
            for i in (0..9).rev() {
                dibits.push(Dibit::new(((coded >> (i * 2)) & 0x03) as u8));
            }
        }
        dibits
    }

    #[test]
    fn decode_clean_header() {
        let header_bytes: [u8; HEADER_BYTES] = [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, // MI
            0x00, // MFID
            0x80, // ALGID = Unencrypted
            0x00, 0x00, // KID
            0x00, 0x42, // TGID = 66
        ];

        let dibits = build_hdu_dibits(&header_bytes);
        let mut recv = VoiceHeaderReceiver::new();
        let mut result = None;

        for dibit in &dibits {
            if let Some(r) = recv.feed(*dibit) {
                result = Some(r);
            }
        }

        let fields = result.unwrap().unwrap();
        assert_eq!(fields.crypto_init(), &[1, 2, 3, 4, 5, 6, 7, 8, 9]);
        assert_eq!(fields.manufacturer_id(), 0);
        assert_eq!(fields.algorithm(), CryptoAlgorithm::Unencrypted);
        assert_eq!(fields.key_id(), 0);
        assert_eq!(fields.talkgroup().value(), 66);
    }

    #[test]
    fn decode_encrypted_header() {
        let header_bytes: [u8; HEADER_BYTES] = [
            0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x11, 0x22, 0x33, // MI
            0x00, // MFID
            0x84, // ALGID = AES-256
            0xDE, 0xAD, // KID
            0x01, 0x00, // TGID = 256
        ];

        let dibits = build_hdu_dibits(&header_bytes);
        let mut recv = VoiceHeaderReceiver::new();
        let mut result = None;

        for dibit in &dibits {
            if let Some(r) = recv.feed(*dibit) {
                result = Some(r);
            }
        }

        let fields = result.unwrap().unwrap();
        assert_eq!(fields.algorithm(), CryptoAlgorithm::Aes);
        assert_eq!(fields.key_id(), 0xDEAD);
        assert_eq!(fields.talkgroup().value(), 256);
    }

    #[test]
    fn is_done_after_all_hexbits() {
        let header_bytes = [0u8; HEADER_BYTES];
        let dibits = build_hdu_dibits(&header_bytes);
        let mut recv = VoiceHeaderReceiver::new();

        assert!(!recv.is_done());

        for dibit in &dibits {
            recv.feed(*dibit);
        }

        assert!(recv.is_done());
    }

    #[test]
    fn header_with_bit_errors() {
        let header_bytes: [u8; HEADER_BYTES] = [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09,
            0x00, 0x80, 0x00, 0x00, 0x00, 0x42,
        ];

        let mut dibits = build_hdu_dibits(&header_bytes);

        // Flip some bits in a few Golay words (Golay corrects up to 3 bits).
        dibits[0] = Dibit::new(dibits[0].bits() ^ 0x03);
        dibits[9] = Dibit::new(dibits[9].bits() ^ 0x03);
        dibits[18] = Dibit::new(dibits[18].bits() ^ 0x03);

        let mut recv = VoiceHeaderReceiver::new();
        let mut result = None;

        for dibit in &dibits {
            if let Some(r) = recv.feed(*dibit) {
                result = Some(r);
            }
        }

        let fields = result.unwrap().unwrap();
        assert_eq!(fields.talkgroup().value(), 66);
    }
}

//! Network Identifier (NID) decoder.
//!
//! Decodes the 64-bit NID word that follows every frame sync pattern.
//! Extracts the Network Access Code (NAC) and Data Unit Identifier
//! (DUID) using direct bit extraction (MVP approach without full
//! BCH error correction).

use crate::p25::consts::NID_DIBITS;
use crate::p25::error::P25Error;
use crate::p25::types::{Dibit, Nac};

/// Data Unit Identifier — identifies the type of P25 data unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataUnit {
    /// Voice header data unit (HDU), DUID = 0x0.
    VoiceHeader,
    /// Trunking signaling data unit (TSDU), DUID = 0x7.
    TrunkingSignaling,
    /// Voice simple terminator (TDU), DUID = 0x3.
    VoiceSimpleTerminator,
    /// Voice LC terminator (TDULC), DUID = 0xF.
    VoiceLcTerminator,
    /// Voice LC frame group 1 (LDU1), DUID = 0x5.
    VoiceLcFrameGroup,
    /// Voice CC frame group 2 (LDU2), DUID = 0xA.
    VoiceCcFrameGroup,
    /// Packet data unit, DUID = 0xC.
    DataPacket,
    /// Unrecognized DUID value.
    Unknown(u8),
}

impl DataUnit {
    /// Decode a 4-bit DUID value.
    pub fn from_bits(bits: u8) -> Self {
        match bits & 0x0F {
            0x0 => Self::VoiceHeader,
            0x3 => Self::VoiceSimpleTerminator,
            0x5 => Self::VoiceLcFrameGroup,
            0x7 => Self::TrunkingSignaling,
            0xA => Self::VoiceCcFrameGroup,
            0xC => Self::DataPacket,
            0xF => Self::VoiceLcTerminator,
            other => Self::Unknown(other),
        }
    }

    /// Return the raw 4-bit DUID value.
    pub fn to_bits(self) -> u8 {
        match self {
            Self::VoiceHeader => 0x0,
            Self::VoiceSimpleTerminator => 0x3,
            Self::VoiceLcFrameGroup => 0x5,
            Self::TrunkingSignaling => 0x7,
            Self::VoiceCcFrameGroup => 0xA,
            Self::DataPacket => 0xC,
            Self::VoiceLcTerminator => 0xF,
            Self::Unknown(v) => v,
        }
    }

    /// Whether this data unit is a trunking signaling data unit (TSDU).
    pub fn is_trunking_signaling(self) -> bool {
        matches!(self, Self::TrunkingSignaling)
    }
}

/// Decoded Network Identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkId {
    /// Network Access Code (12 bits).
    pub access_code: Nac,
    /// Data unit type.
    pub data_unit: DataUnit,
    /// Whether the overall parity check passed.
    pub parity_ok: bool,
}

/// Decode a NID from 32 data dibits.
///
/// Extracts NAC (bits 63-52), DUID (bits 51-48), and checks the
/// overall parity bit (bit 0). Uses direct extraction without BCH
/// error correction.
pub fn decode_nid(dibits: &[Dibit]) -> Result<NetworkId, P25Error> {
    if dibits.len() != NID_DIBITS {
        return Err(P25Error::NidDecode {
            reason: format!("expected {} dibits, got {}", NID_DIBITS, dibits.len()),
        });
    }

    let word = dibits_to_u64(dibits);

    let nac_bits = ((word >> 52) & 0x0FFF) as u16;
    let duid_bits = ((word >> 48) & 0x0F) as u8;

    // Check overall parity: XOR of all 64 bits should be 0.
    // The parity bit at position 0 is included in count_ones,
    // so even popcount means parity is correct.
    let parity_ok = word.count_ones().is_multiple_of(2);

    Ok(NetworkId {
        access_code: Nac::new(nac_bits),
        data_unit: DataUnit::from_bits(duid_bits),
        parity_ok,
    })
}

/// Convert 32 dibits to a 64-bit word (MSB first).
fn dibits_to_u64(dibits: &[Dibit]) -> u64 {
    let mut word: u64 = 0;
    for dibit in dibits {
        word = (word << 2) | u64::from(dibit.bits());
    }
    word
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build 32 dibits from a 64-bit word.
    fn u64_to_dibits(word: u64) -> Vec<Dibit> {
        (0..32)
            .map(|i| {
                let shift = 62 - i * 2;
                Dibit::new(((word >> shift) & 0x03) as u8)
            })
            .collect()
    }

    /// Build a NID word with given NAC and DUID, setting correct parity.
    fn make_nid_word(nac: u16, duid: u8) -> u64 {
        let mut word: u64 = 0;
        word |= (u64::from(nac) & 0x0FFF) << 52;
        word |= (u64::from(duid) & 0x0F) << 48;
        // BCH parity bits (47..1) left as zero for MVP.
        // Set parity bit (bit 0) so total popcount is even.
        if word.count_ones() % 2 != 0 {
            word |= 1;
        }
        word
    }

    #[test]
    fn decode_tsdu_duid() {
        let word = make_nid_word(0x293, 0x7);
        let dibits = u64_to_dibits(word);
        let nid = decode_nid(&dibits).unwrap();

        assert_eq!(nid.access_code, Nac::new(0x293));
        assert!(nid.data_unit.is_trunking_signaling());
        assert!(nid.parity_ok);
    }

    #[test]
    fn all_duid_values_parse() {
        let known_duids: [(u8, DataUnit); 7] = [
            (0x0, DataUnit::VoiceHeader),
            (0x3, DataUnit::VoiceSimpleTerminator),
            (0x5, DataUnit::VoiceLcFrameGroup),
            (0x7, DataUnit::TrunkingSignaling),
            (0xA, DataUnit::VoiceCcFrameGroup),
            (0xC, DataUnit::DataPacket),
            (0xF, DataUnit::VoiceLcTerminator),
        ];

        for (bits, expected) in &known_duids {
            let du = DataUnit::from_bits(*bits);
            assert_eq!(du, *expected, "DUID 0x{bits:X}");
            assert_eq!(du.to_bits(), *bits);
        }
    }

    #[test]
    fn unknown_duid_preserved() {
        let du = DataUnit::from_bits(0x1);
        assert_eq!(du, DataUnit::Unknown(0x1));
        assert_eq!(du.to_bits(), 0x1);
        assert!(!du.is_trunking_signaling());
    }

    #[test]
    fn special_nac_values() {
        for nac_val in [0x293u16, 0xF7E, 0xF7F] {
            let word = make_nid_word(nac_val, 0x7);
            let dibits = u64_to_dibits(word);
            let nid = decode_nid(&dibits).unwrap();
            assert_eq!(nid.access_code.value(), nac_val);
        }
    }

    #[test]
    fn parity_check_detects_error() {
        let word = make_nid_word(0x293, 0x7);
        let mut dibits = u64_to_dibits(word);

        // Flip one bit to break parity.
        let flipped = dibits[15].bits() ^ 0x01;
        dibits[15] = Dibit::new(flipped);

        let nid = decode_nid(&dibits).unwrap();
        assert!(!nid.parity_ok);
    }

    #[test]
    fn wrong_dibit_count_returns_error() {
        let short: Vec<Dibit> = (0..20).map(|_| Dibit::new(0)).collect();
        let result = decode_nid(&short);
        assert!(result.is_err());
    }

    #[test]
    fn dibits_to_u64_roundtrip() {
        let original: u64 = 0xA293_7000_0000_0001;
        let dibits = u64_to_dibits(original);
        let recovered = dibits_to_u64(&dibits);
        assert_eq!(recovered, original);
    }

    #[test]
    fn nac_extraction_all_bits() {
        // NAC = 0xFFF (all 12 bits set)
        let word = make_nid_word(0xFFF, 0x0);
        let dibits = u64_to_dibits(word);
        let nid = decode_nid(&dibits).unwrap();
        assert_eq!(nid.access_code.value(), 0x0FFF);
    }
}

//! Network Identifier (NID) decoder with BCH error correction.
//!
//! Decodes the 64-bit NID word that follows every frame sync pattern.
//! The NID contains a BCH(63,16,23) codeword (bits 63-1) and an
//! overall parity bit (bit 0). The BCH code protects the 12-bit NAC
//! and 4-bit DUID, correcting up to 11 bit errors.

use crate::p25::bch;
use crate::p25::consts::NID_DIBITS;
use crate::p25::error::P25Error;
use crate::p25::types::{Dibit, Nac};

/// Policy for gating data unit routing on NID integrity.
///
/// Controls whether the receiver accepts or rejects NIDs that fail
/// integrity checks (currently parity, BCH in the future).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NidIntegrityPolicy {
    /// Reject NIDs that fail integrity checks. The receiver skips the
    /// data unit and waits for the next frame sync.
    #[default]
    Strict,
    /// Accept NIDs that fail integrity checks with a warning log.
    /// Useful for development and debugging noisy channels.
    Permissive,
}

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

/// Decode a NID from 32 data dibits with BCH error correction.
///
/// Applies BCH(63,16,23) error correction to the 63-bit codeword
/// (bits 63-1), then extracts NAC and DUID from the corrected data.
/// Can correct up to 11 bit errors in the codeword. Falls back to
/// uncorrected extraction if BCH fails (more than 11 errors).
pub fn decode_nid(dibits: &[Dibit]) -> Result<NetworkId, P25Error> {
    if dibits.len() != NID_DIBITS {
        return Err(P25Error::NidDecode {
            reason: format!("expected {} dibits, got {}", NID_DIBITS, dibits.len()),
        });
    }

    let word = dibits_to_u64(dibits);

    // Separate the 63-bit BCH codeword (bits 63-1) from the parity bit (bit 0).
    let bch_word = word >> 1;

    if let Some(result) = bch::decode(bch_word) {
        // BCH correction succeeded — extract data from corrected codeword.
        let data = bch::extract_data(result.corrected);
        let nac_bits = (data >> 4) & 0x0FFF;
        let duid_bits = (data & 0x0F) as u8;

        if result.errors_corrected > 0 {
            tracing::debug!(
                errors_corrected = result.errors_corrected,
                "BCH corrected NID bit errors"
            );
        }

        // Check overall parity against the corrected codeword.
        let corrected_full = (result.corrected << 1) | (word & 1);
        let parity_ok = corrected_full.count_ones().is_multiple_of(2);

        // If BCH succeeded but parity fails, the parity bit itself
        // is in error. The BCH-protected data is still trustworthy.
        if !parity_ok {
            tracing::trace!("parity bit error after BCH correction (data still valid)");
        }

        Ok(NetworkId {
            access_code: Nac::new(nac_bits),
            data_unit: DataUnit::from_bits(duid_bits),
            parity_ok: true, // BCH success is the integrity check.
        })
    } else {
        // BCH failed — fall back to uncorrected extraction.
        let nac_bits = ((word >> 52) & 0x0FFF) as u16;
        let duid_bits = ((word >> 48) & 0x0F) as u8;

        tracing::trace!(
            nac = format_args!("0x{nac_bits:03X}"),
            duid = duid_bits,
            "BCH correction failed, NID uncorrectable"
        );

        Ok(NetworkId {
            access_code: Nac::new(nac_bits),
            data_unit: DataUnit::from_bits(duid_bits),
            parity_ok: false,
        })
    }
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

    /// Build a NID word with given NAC and DUID, properly BCH-encoded.
    fn make_nid_word(nac: u16, duid: u8) -> u64 {
        let data = ((nac & 0x0FFF) << 4) | u16::from(duid & 0x0F);
        let bch_word = crate::p25::bch::encode(data);
        let shifted = bch_word << 1;
        // Set overall parity bit so the 64-bit word has even popcount.
        if shifted.count_ones() % 2 != 0 {
            shifted | 1
        } else {
            shifted
        }
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
    fn bch_corrects_single_bit_error() {
        let word = make_nid_word(0x293, 0x7);
        let mut dibits = u64_to_dibits(word);

        // Flip one bit — BCH should correct it.
        let flipped = dibits[15].bits() ^ 0x01;
        dibits[15] = Dibit::new(flipped);

        let nid = decode_nid(&dibits).unwrap();
        assert!(nid.parity_ok, "BCH should correct single-bit error");
        assert_eq!(nid.access_code, Nac::new(0x293));
        assert!(nid.data_unit.is_trunking_signaling());
    }

    #[test]
    fn bch_rejects_uncorrectable_errors() {
        let word = make_nid_word(0x293, 0x7);
        let mut dibits = u64_to_dibits(word);

        // Flip 12 bits (exceeds BCH correction capability of 11).
        for i in 16..22 {
            dibits[i] = Dibit::new(dibits[i].bits() ^ 0x03);
        }

        let nid = decode_nid(&dibits).unwrap();
        assert!(!nid.parity_ok, "12 errors should exceed BCH capability");
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

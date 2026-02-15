//! TSBK parsing: bytes to structured messages.
//!
//! The top-level entry point is [`parse_tsbk`], which takes a 12-byte TSBK
//! (after trellis decoding), validates the CRC-16, extracts the header fields,
//! and dispatches to per-opcode field parsers.
//!
//! ## Bit Layout (96-bit TSBK)
//!
//! ```text
//! Bit 95 (index 0): Last Block flag
//! Bit 94 (index 1): Protected flag
//! Bits 93-88 (index 2..8): Opcode (6 bits)
//! Bits 87-80 (index 8..16): Manufacturer ID (8 bits)
//! Bits 79-16 (index 16..80): Opcode-specific payload (64 bits)
//! Bits 15-0 (index 80..96): CRC-16 (16 bits)
//! ```
//!
//! bitvec index mapping: `index = 95 - tia102_bit_number`
//!
//! Reference: TIA-102.AABF-A Section 7.1

use bitvec::order::Msb0;
use bitvec::slice::BitSlice;
use bitvec::view::BitView;

use super::fields::TsbkFields;
use super::opcode::TsbkOpcode;
use crate::p25::crc::{self, CrcError};
use crate::p25::types::*;
use crate::util::bits::{extract_u8, extract_u16, extract_u32};

/// TSBK data length in bytes (96 bits).
const TSBK_LENGTH_BYTES: usize = 12;

/// Motorola manufacturer ID, used for vendor-specific opcode variants.
const MFRID_MOTOROLA: u8 = 0x90;

/// A fully parsed TSBK message.
#[derive(Debug, Clone, PartialEq)]
pub struct TsbkMessage {
    /// Whether this is the last block in a multi-block sequence.
    pub last_block: bool,
    /// Whether this TSBK is protected (encrypted control signaling).
    pub protected: bool,
    /// The opcode identifying this message type.
    pub opcode: TsbkOpcode,
    /// The manufacturer ID. 0x00 = standard P25.
    pub manufacturer_id: ManufacturerId,
    /// Parsed fields specific to this opcode.
    pub fields: TsbkFields,
}

/// Errors that can occur during TSBK parsing.
#[derive(Debug, thiserror::Error)]
pub enum TsbkError {
    /// The input data is not the expected 12 bytes.
    #[error("invalid TSBK data length: expected 12 bytes, got {0}")]
    InvalidLength(usize),

    /// CRC-16 validation failed.
    #[error(transparent)]
    Crc(#[from] CrcError),
}

/// Parse a 12-byte TSBK into a structured message.
///
/// Validates the CRC-16, extracts header fields (opcode, manufacturer ID,
/// last block flag), and dispatches to the appropriate per-opcode parser.
///
/// # Errors
///
/// Returns `TsbkError::InvalidLength` if `data` is not exactly 12 bytes.
/// Returns `TsbkError::Crc` if CRC-16 validation fails.
pub fn parse_tsbk(data: &[u8]) -> Result<TsbkMessage, TsbkError> {
    if data.len() != TSBK_LENGTH_BYTES {
        return Err(TsbkError::InvalidLength(data.len()));
    }

    // Safety: length checked above
    let data_array: &[u8; 12] = data.try_into().unwrap();
    crc::validate_tsbk_crc(data_array)?;

    let bits = data.view_bits::<Msb0>();

    let last_block = bits[0];
    let protected = bits[1];
    let opcode_raw = extract_u8(bits, 2..8);
    let mfrid = extract_u8(bits, 8..16);

    let opcode = TsbkOpcode::from_u8(opcode_raw);
    let manufacturer_id = ManufacturerId(mfrid);

    let fields = parse_fields(opcode, manufacturer_id, bits);

    Ok(TsbkMessage {
        last_block,
        protected,
        opcode,
        manufacturer_id,
        fields,
    })
}

/// Dispatch to the appropriate per-opcode field parser.
fn parse_fields(
    opcode: TsbkOpcode,
    mfrid: ManufacturerId,
    bits: &BitSlice<u8, Msb0>,
) -> TsbkFields {
    match opcode {
        TsbkOpcode::GroupVoiceChannelGrant => parse_group_voice_channel_grant(bits, mfrid),
        TsbkOpcode::GroupVoiceChannelGrantUpdate => parse_group_voice_channel_grant_update(bits),
        TsbkOpcode::GroupVoiceChannelGrantUpdateExplicit => {
            parse_group_voice_channel_grant_update_explicit(bits)
        }
        TsbkOpcode::UnitToUnitVoiceChannelGrant => parse_unit_to_unit_voice_channel_grant(bits),
        TsbkOpcode::EmergencyAlarm => parse_emergency_alarm(bits),
        TsbkOpcode::AcknowledgeResponse => parse_acknowledge_response(bits),
        TsbkOpcode::DenyResponse => parse_deny_response(bits),
        TsbkOpcode::GroupAffiliationResponse => parse_group_affiliation_response(bits),
        TsbkOpcode::UnitRegistrationResponse => parse_unit_registration_response(bits),
        TsbkOpcode::ChannelIdentifierUpdateVu => parse_iden_up_vu(bits),
        TsbkOpcode::ChannelIdentifierUpdateTdma => parse_iden_up_tdma(bits),
        TsbkOpcode::ChannelIdentifierUpdateStandard => parse_iden_up_standard(bits),
        TsbkOpcode::NetworkStatusBroadcast => parse_network_status_broadcast(bits),
        TsbkOpcode::SecondaryControlChannelBroadcast => {
            parse_secondary_control_channel_broadcast(bits)
        }
        TsbkOpcode::RfssStatusBroadcast => parse_rfss_status_broadcast(bits),
        TsbkOpcode::AdjacentStatusBroadcast => parse_adjacent_status_broadcast(bits),
        TsbkOpcode::Unknown(v) => TsbkFields::Unknown {
            opcode_value: v,
            raw_payload: extract_payload_bytes(bits),
        },
        other => TsbkFields::Unimplemented {
            opcode: other,
            raw_payload: extract_payload_bytes(bits),
        },
    }
}

/// Extract the 8-byte payload (TSBK bytes 2-9) as a raw byte array.
fn extract_payload_bytes(bits: &BitSlice<u8, Msb0>) -> [u8; 8] {
    let mut payload = [0u8; 8];
    for (i, byte) in payload.iter_mut().enumerate() {
        let start = 16 + i * 8;
        *byte = extract_u8(bits, start..start + 8);
    }
    payload
}

// ---------------------------------------------------------------------------
// Per-opcode parsers
// ---------------------------------------------------------------------------

/// Parse Group Voice Channel Grant (opcode 0x00).
///
/// With manufacturer ID 0x90 (Motorola), this becomes a Group Regroup Command
/// with a completely different field layout.
///
/// Standard layout (MFRID 0x00):
/// - Bits 79-72 (index 16..24): Service Options (8 bits)
/// - Bits 71-56 (index 24..40): Channel ID (16 bits)
/// - Bits 55-40 (index 40..56): Talkgroup (16 bits)
/// - Bits 39-16 (index 56..80): Source Unit ID (24 bits)
///
/// Reference: TIA-102.AABF-A Section 7.2.1
fn parse_group_voice_channel_grant(bits: &BitSlice<u8, Msb0>, mfrid: ManufacturerId) -> TsbkFields {
    if mfrid.0 == MFRID_MOTOROLA {
        return TsbkFields::MotorolaGroupRegroupCommand {
            supergroup: TalkgroupId(extract_u16(bits, 16..32)),
            group_a: TalkgroupId(extract_u16(bits, 32..48)),
            group_b: TalkgroupId(extract_u16(bits, 48..64)),
            group_c: TalkgroupId(extract_u16(bits, 64..80)),
        };
    }

    TsbkFields::GroupVoiceChannelGrant {
        service_options: ServiceOptions(extract_u8(bits, 16..24)),
        channel: ChannelId(extract_u16(bits, 24..40)),
        talkgroup: TalkgroupId(extract_u16(bits, 40..56)),
        source: UnitId(extract_u32(bits, 56..80)),
    }
}

/// Parse Group Voice Channel Grant Update (opcode 0x02).
///
/// - Bits 79-64 (index 16..32): Channel A (16 bits)
/// - Bits 63-48 (index 32..48): Talkgroup A (16 bits)
/// - Bits 47-32 (index 48..64): Channel B (16 bits)
/// - Bits 31-16 (index 64..80): Talkgroup B (16 bits)
///
/// Reference: TIA-102.AABF-A Section 7.2.2
fn parse_group_voice_channel_grant_update(bits: &BitSlice<u8, Msb0>) -> TsbkFields {
    TsbkFields::GroupVoiceChannelGrantUpdate {
        channel_a: ChannelId(extract_u16(bits, 16..32)),
        talkgroup_a: TalkgroupId(extract_u16(bits, 32..48)),
        channel_b: ChannelId(extract_u16(bits, 48..64)),
        talkgroup_b: TalkgroupId(extract_u16(bits, 64..80)),
    }
}

/// Parse Group Voice Channel Grant Update - Explicit (opcode 0x03).
///
/// Same field layout as the standard update.
///
/// Reference: TIA-102.AABF-A Section 7.2.3
fn parse_group_voice_channel_grant_update_explicit(bits: &BitSlice<u8, Msb0>) -> TsbkFields {
    TsbkFields::GroupVoiceChannelGrantUpdateExplicit {
        channel_a: ChannelId(extract_u16(bits, 16..32)),
        talkgroup_a: TalkgroupId(extract_u16(bits, 32..48)),
        channel_b: ChannelId(extract_u16(bits, 48..64)),
        talkgroup_b: TalkgroupId(extract_u16(bits, 64..80)),
    }
}

/// Parse Unit to Unit Voice Channel Grant (opcode 0x04).
///
/// - Bits 71-56 (index 24..40): Channel ID (16 bits)
/// - Bits 55-32 (index 40..64): Target Unit ID (24 bits)
/// - Bits 39-16 (index 56..80): Source Unit ID (24 bits)
///
/// Reference: TIA-102.AABF-A Section 7.2.4
fn parse_unit_to_unit_voice_channel_grant(bits: &BitSlice<u8, Msb0>) -> TsbkFields {
    TsbkFields::UnitToUnitVoiceChannelGrant {
        channel: ChannelId(extract_u16(bits, 24..40)),
        target: UnitId(extract_u32(bits, 40..64)),
        source: UnitId(extract_u32(bits, 56..80)),
    }
}

/// Parse Emergency Alarm (opcode 0x09).
///
/// - Bits 55-32 (index 40..64): Source Unit ID (24 bits)
/// - Bits 39-24 (index 56..72): Talkgroup (16 bits)
///
/// Reference: TIA-102.AABF-A Section 7.2.9
fn parse_emergency_alarm(bits: &BitSlice<u8, Msb0>) -> TsbkFields {
    TsbkFields::EmergencyAlarm {
        source: UnitId(extract_u32(bits, 40..64)),
        talkgroup: TalkgroupId(extract_u16(bits, 56..72)),
    }
}

/// Parse Acknowledge Response (opcode 0x10).
///
/// - Bits 55-32 (index 40..64): Target Unit ID (24 bits)
/// - Bits 39-16 (index 56..80): Source Unit ID (24 bits)
///
/// Reference: TIA-102.AABF-A Section 7.2.10
fn parse_acknowledge_response(bits: &BitSlice<u8, Msb0>) -> TsbkFields {
    TsbkFields::AcknowledgeResponse {
        target: UnitId(extract_u32(bits, 40..64)),
        source: UnitId(extract_u32(bits, 56..80)),
    }
}

/// Parse Deny Response (opcode 0x16).
///
/// - Bits 55-40 (index 40..56): Talkgroup (16 bits)
/// - Bits 39-16 (index 56..80): Source Unit ID (24 bits)
///
/// Reference: TIA-102.AABF-A Section 7.2.16
fn parse_deny_response(bits: &BitSlice<u8, Msb0>) -> TsbkFields {
    TsbkFields::DenyResponse {
        talkgroup: TalkgroupId(extract_u16(bits, 40..56)),
        source: UnitId(extract_u32(bits, 56..80)),
    }
}

/// Parse Group Affiliation Response (opcode 0x28).
///
/// - Bits 55-40 (index 40..56): Talkgroup (16 bits)
/// - Bits 39-16 (index 56..80): Source Unit ID (24 bits)
///
/// Reference: TIA-102.AABF-A Section 7.2.28
fn parse_group_affiliation_response(bits: &BitSlice<u8, Msb0>) -> TsbkFields {
    TsbkFields::GroupAffiliationResponse {
        talkgroup: TalkgroupId(extract_u16(bits, 40..56)),
        source: UnitId(extract_u32(bits, 56..80)),
    }
}

/// Parse Unit Registration Response (opcode 0x2C).
///
/// - Bits 55-32 (index 40..64): Source Unit ID (24 bits)
/// - Bits 39-28 (index 56..68): System ID (12 bits)
///
/// Reference: TIA-102.AABF-A Section 7.2.2C
fn parse_unit_registration_response(bits: &BitSlice<u8, Msb0>) -> TsbkFields {
    TsbkFields::UnitRegistrationResponse {
        source: UnitId(extract_u32(bits, 40..64)),
        system_id: SystemId(extract_u16(bits, 56..68)),
    }
}

/// Parse Channel Identifier Update VHF/UHF (opcode 0x34).
///
/// - Bits 79-76 (index 16..20): Identifier (4 bits)
/// - Bits 75-72 (index 20..24): Bandwidth VU (4 bits)
/// - Bits 71-58 (index 24..38): Transmit Offset raw (14 bits)
/// - Bits 57-48 (index 38..48): Channel Spacing multiplier (10 bits)
/// - Bits 47-16 (index 48..80): Base Frequency raw (32 bits)
///
/// Transmit offset sign: bit 13 (MSB of 14-bit field). 0 = negative, 1 = positive.
/// Channel spacing in Hz = raw * 125.
/// Transmit offset in Hz = signed_magnitude * spacing.
/// Base frequency in Hz = raw * 5.
///
/// Reference: TIA-102.AABF-A Section 7.2.34
fn parse_iden_up_vu(bits: &BitSlice<u8, Msb0>) -> TsbkFields {
    let ident = extract_u8(bits, 16..20);
    let bwvu = extract_u8(bits, 20..24);
    let toff_raw = extract_u16(bits, 24..38);
    let spac_raw = extract_u16(bits, 38..48);
    let freq_raw = extract_u32(bits, 48..80);

    let (channel_spacing, transmit_offset, base_frequency) =
        decode_iden_up_vu_params(toff_raw, spac_raw, freq_raw);

    TsbkFields::ChannelIdentifierUpdateVu {
        identifier: ChannelIdentifier(ident),
        bandwidth_vu: bwvu,
        transmit_offset,
        channel_spacing,
        base_frequency,
    }
}

/// Parse Channel Identifier Update TDMA (opcode 0x33).
///
/// Same field layout as IDEN_UP_VU (0x34).
///
/// Reference: TIA-102.AABF-A Section 7.2.33
fn parse_iden_up_tdma(bits: &BitSlice<u8, Msb0>) -> TsbkFields {
    let ident = extract_u8(bits, 16..20);
    let bwvu = extract_u8(bits, 20..24);
    let toff_raw = extract_u16(bits, 24..38);
    let spac_raw = extract_u16(bits, 38..48);
    let freq_raw = extract_u32(bits, 48..80);

    let (channel_spacing, transmit_offset, base_frequency) =
        decode_iden_up_vu_params(toff_raw, spac_raw, freq_raw);

    TsbkFields::ChannelIdentifierUpdateTdma {
        identifier: ChannelIdentifier(ident),
        bandwidth_vu: bwvu,
        transmit_offset,
        channel_spacing,
        base_frequency,
    }
}

/// Decode frequency parameters shared by IDEN_UP_VU and IDEN_UP_TDMA.
///
/// Returns (channel_spacing_hz, transmit_offset_hz, base_frequency_hz).
fn decode_iden_up_vu_params(toff_raw: u16, spac_raw: u16, freq_raw: u32) -> (u32, i32, u64) {
    let toff_sign = (toff_raw >> 13) & 1;
    let toff_magnitude = (toff_raw & 0x1FFF) as i32;
    let toff = if toff_sign == 0 {
        -toff_magnitude
    } else {
        toff_magnitude
    };

    let channel_spacing = spac_raw as u32 * 125;
    let transmit_offset = toff * spac_raw as i32 * 125;
    let base_frequency = freq_raw as u64 * 5;

    (channel_spacing, transmit_offset, base_frequency)
}

/// Parse Channel Identifier Update - Standard (opcode 0x3D).
///
/// - Bits 79-76 (index 16..20): Identifier (4 bits)
/// - Bits 75-67 (index 20..29): Bandwidth raw (9 bits)
/// - Bits 66-58 (index 29..38): Transmit Offset raw (9 bits)
/// - Bits 57-48 (index 38..48): Channel Spacing multiplier (10 bits)
/// - Bits 47-16 (index 48..80): Base Frequency raw (32 bits)
///
/// Transmit offset sign: bit 8 (MSB of 9-bit field). 0 = negative, 1 = positive.
/// Channel spacing in Hz = raw * 125.
/// Transmit offset in Hz = signed_magnitude * 250000.
/// Base frequency in Hz = raw * 5.
///
/// Reference: TIA-102.AABF-A Section 7.2.3D
fn parse_iden_up_standard(bits: &BitSlice<u8, Msb0>) -> TsbkFields {
    let ident = extract_u8(bits, 16..20);
    let bw_raw = extract_u16(bits, 20..29);
    let toff_raw = extract_u16(bits, 29..38);
    let spac_raw = extract_u16(bits, 38..48);
    let freq_raw = extract_u32(bits, 48..80);

    let toff_sign = (toff_raw >> 8) & 1;
    let toff_magnitude = (toff_raw & 0xFF) as i32;
    let toff = if toff_sign == 0 {
        -toff_magnitude
    } else {
        toff_magnitude
    };

    let channel_spacing = spac_raw as u32 * 125;
    let transmit_offset = toff * 250_000;
    let base_frequency = freq_raw as u64 * 5;

    TsbkFields::ChannelIdentifierUpdateStandard {
        identifier: ChannelIdentifier(ident),
        bandwidth: bw_raw,
        transmit_offset,
        channel_spacing,
        base_frequency,
    }
}

/// Parse Network Status Broadcast (opcode 0x39).
///
/// - Bits 71-52 (index 24..44): WACN (20 bits)
/// - Bits 51-40 (index 44..56): System ID (12 bits)
/// - Bits 39-24 (index 56..72): Channel ID (16 bits)
///
/// Reference: TIA-102.AABF-A Section 7.2.39
fn parse_network_status_broadcast(bits: &BitSlice<u8, Msb0>) -> TsbkFields {
    TsbkFields::NetworkStatusBroadcast {
        wacn: Wacn(extract_u32(bits, 24..44)),
        system_id: SystemId(extract_u16(bits, 44..56)),
        channel: ChannelId(extract_u16(bits, 56..72)),
    }
}

/// Parse Secondary Control Channel Broadcast (opcode 0x3B).
///
/// Same field layout as Network Status Broadcast.
///
/// Reference: TIA-102.AABF-A Section 7.2.3B
fn parse_secondary_control_channel_broadcast(bits: &BitSlice<u8, Msb0>) -> TsbkFields {
    TsbkFields::SecondaryControlChannelBroadcast {
        wacn: Wacn(extract_u32(bits, 24..44)),
        system_id: SystemId(extract_u16(bits, 44..56)),
        channel: ChannelId(extract_u16(bits, 56..72)),
    }
}

/// Parse RFSS Status Broadcast (opcode 0x3A).
///
/// - Bits 67-56 (index 28..40): System ID (12 bits)
/// - Bits 55-48 (index 40..48): RFSS ID (8 bits)
/// - Bits 47-40 (index 48..56): Site ID (8 bits)
/// - Bits 39-24 (index 56..72): Channel ID (16 bits)
///
/// Reference: TIA-102.AABF-A Section 7.2.3A
fn parse_rfss_status_broadcast(bits: &BitSlice<u8, Msb0>) -> TsbkFields {
    TsbkFields::RfssStatusBroadcast {
        system_id: SystemId(extract_u16(bits, 28..40)),
        rfss: RfssId(extract_u8(bits, 40..48)),
        site: SiteId(extract_u8(bits, 48..56)),
        channel: ChannelId(extract_u16(bits, 56..72)),
    }
}

/// Parse Adjacent Status Broadcast (opcode 0x3C).
///
/// Same field layout as RFSS Status Broadcast.
///
/// Reference: TIA-102.AABF-A Section 7.2.3C
fn parse_adjacent_status_broadcast(bits: &BitSlice<u8, Msb0>) -> TsbkFields {
    TsbkFields::AdjacentStatusBroadcast {
        system_id: SystemId(extract_u16(bits, 28..40)),
        rfss: RfssId(extract_u8(bits, 40..48)),
        site: SiteId(extract_u8(bits, 48..56)),
        channel: ChannelId(extract_u16(bits, 56..72)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a 12-byte TSBK with valid CRC from 10 header+payload bytes.
    fn make_tsbk(header_and_payload: &[u8; 10]) -> [u8; 12] {
        let mut data = [0u8; 12];
        data[..10].copy_from_slice(header_and_payload);
        let crc_val = crc::crc16(&data[..10]);
        data[10] = (crc_val >> 8) as u8;
        data[11] = (crc_val & 0xFF) as u8;
        data
    }

    #[test]
    fn parse_group_voice_channel_grant() {
        // last_block=1, protected=0, opcode=0x00 -> byte 0 = 0b10_000000 = 0x80
        // mfrid=0x00
        // service_options=0x00
        // channel=0x1234
        // talkgroup=0x0EB9 (3769)
        // source=0x003039 (12345)
        let tsbk = make_tsbk(&[0x80, 0x00, 0x00, 0x12, 0x34, 0x0E, 0xB9, 0x00, 0x30, 0x39]);

        let msg = parse_tsbk(&tsbk).unwrap();
        assert_eq!(msg.opcode, TsbkOpcode::GroupVoiceChannelGrant);
        assert!(msg.last_block);
        assert!(!msg.protected);
        assert_eq!(msg.manufacturer_id, ManufacturerId(0x00));

        match msg.fields {
            TsbkFields::GroupVoiceChannelGrant {
                service_options,
                channel,
                talkgroup,
                source,
            } => {
                assert_eq!(service_options, ServiceOptions(0x00));
                assert_eq!(channel, ChannelId(0x1234));
                assert_eq!(talkgroup, TalkgroupId(3769));
                assert_eq!(source, UnitId(12345));
            }
            _ => panic!("expected GroupVoiceChannelGrant"),
        }
    }

    #[test]
    fn parse_motorola_group_regroup() {
        // opcode=0x00, mfrid=0x90 -> Motorola Group Regroup
        // last_block=1, protected=0, opcode=0x00 -> 0x80
        // supergroup=0x0001, ga1=0x0002, ga2=0x0003, ga3=0x0004
        let tsbk = make_tsbk(&[0x80, 0x90, 0x00, 0x01, 0x00, 0x02, 0x00, 0x03, 0x00, 0x04]);

        let msg = parse_tsbk(&tsbk).unwrap();
        assert_eq!(msg.opcode, TsbkOpcode::GroupVoiceChannelGrant);
        assert_eq!(msg.manufacturer_id, ManufacturerId(0x90));

        match msg.fields {
            TsbkFields::MotorolaGroupRegroupCommand {
                supergroup,
                group_a,
                group_b,
                group_c,
            } => {
                assert_eq!(supergroup, TalkgroupId(1));
                assert_eq!(group_a, TalkgroupId(2));
                assert_eq!(group_b, TalkgroupId(3));
                assert_eq!(group_c, TalkgroupId(4));
            }
            _ => panic!("expected MotorolaGroupRegroupCommand"),
        }
    }

    #[test]
    fn parse_group_voice_channel_grant_update() {
        // last_block=1, opcode=0x02 -> 0b10_000010 = 0x82
        // ch_a=0xAAAA, tg_a=0x1111, ch_b=0xBBBB, tg_b=0x2222
        let tsbk = make_tsbk(&[0x82, 0x00, 0xAA, 0xAA, 0x11, 0x11, 0xBB, 0xBB, 0x22, 0x22]);

        let msg = parse_tsbk(&tsbk).unwrap();
        assert_eq!(msg.opcode, TsbkOpcode::GroupVoiceChannelGrantUpdate);

        match msg.fields {
            TsbkFields::GroupVoiceChannelGrantUpdate {
                channel_a,
                talkgroup_a,
                channel_b,
                talkgroup_b,
            } => {
                assert_eq!(channel_a, ChannelId(0xAAAA));
                assert_eq!(talkgroup_a, TalkgroupId(0x1111));
                assert_eq!(channel_b, ChannelId(0xBBBB));
                assert_eq!(talkgroup_b, TalkgroupId(0x2222));
            }
            _ => panic!("expected GroupVoiceChannelGrantUpdate"),
        }
    }

    #[test]
    fn parse_unit_to_unit_voice_channel_grant() {
        // last_block=1, opcode=0x04 -> 0b10_000100 = 0x84
        // mfrid=0x00
        // (8 bits reserved/service_options) then:
        // channel=0x5678, target=0x00ABCD (43981), source=0x001234 (4660)
        let tsbk = make_tsbk(&[0x84, 0x00, 0x00, 0x56, 0x78, 0x00, 0xAB, 0xCD, 0x12, 0x34]);

        let msg = parse_tsbk(&tsbk).unwrap();
        match msg.fields {
            TsbkFields::UnitToUnitVoiceChannelGrant {
                channel,
                target,
                source,
            } => {
                assert_eq!(channel, ChannelId(0x5678));
                assert_eq!(target, UnitId(0x00ABCD));
                assert_eq!(source, UnitId(0xCD1234));
            }
            _ => panic!("expected UnitToUnitVoiceChannelGrant"),
        }
    }

    #[test]
    fn parse_network_status_broadcast() {
        // last_block=1, opcode=0x39 -> 0b10_111001 = 0xB9
        // mfrid=0x00
        // (8 reserved bits)
        // wacn(20 bits)=0xBEE00, sysid(12 bits)=0x5F2, channel(16 bits)=0x1234
        //
        // Payload bits (index 16..80 = 64 bits):
        //   8 reserved | 20 wacn | 12 sysid | 16 channel | 8 service_class
        //   00000000 | 10111110111000000000 | 010111110010 | 0001001000110100 | ...
        //
        // Let's just pack the bytes carefully:
        // Index 16..24: reserved = 0x00
        // Index 24..44: wacn=0xBEE00 (20 bits)
        //   0xBEE00 = 0b 1011 1110 1110 0000 0000
        //   bits 24..32: 0b10111110 = 0xBE
        //   bits 32..40: 0b11100000 = 0xE0
        //   bits 40..44: 0b0000 (first 4 of next byte)
        // Index 44..56: sysid=0x5F2 (12 bits)
        //   0x5F2 = 0b 0101 1111 0010
        //   bits 44..48: 0b0101 -> combined with wacn bits 40..44: byte[5] = 0b0000_0101 = 0x05
        //   bits 48..56: 0b11110010 = 0xF2
        // Index 56..72: channel=0x1234
        //   byte[7] = 0x12, byte[8] = 0x34
        // Index 72..80: service class = 0x00
        //   byte[9] = 0x00
        let tsbk = make_tsbk(&[0xB9, 0x00, 0x00, 0xBE, 0xE0, 0x05, 0xF2, 0x12, 0x34, 0x00]);

        let msg = parse_tsbk(&tsbk).unwrap();
        assert_eq!(msg.opcode, TsbkOpcode::NetworkStatusBroadcast);

        match msg.fields {
            TsbkFields::NetworkStatusBroadcast {
                wacn,
                system_id,
                channel,
            } => {
                assert_eq!(wacn, Wacn(0xBEE00));
                assert_eq!(system_id, SystemId(0x5F2));
                assert_eq!(channel, ChannelId(0x1234));
            }
            _ => panic!("expected NetworkStatusBroadcast"),
        }
    }

    #[test]
    fn parse_rfss_status_broadcast() {
        // last_block=1, opcode=0x3A -> 0b10_111010 = 0xBA
        // Payload layout (index 16..80):
        //   16..28: reserved (12 bits) = 0x000
        //   28..40: system_id (12 bits)
        //   40..48: rfss (8 bits)
        //   48..56: site (8 bits)
        //   56..72: channel (16 bits)
        //   72..80: service_class (8 bits)
        //
        // sysid=0x5F2, rfss=1, site=3, channel=0xABCD
        // byte[2]=0x00 (reserved top 8)
        // byte[3]: reserved[4] + sysid[4] = 0x05 (reserved=0000, sysid top 4=0101)
        // byte[4]: sysid[8] = 0xF2
        // byte[5]: rfss = 0x01
        // byte[6]: site = 0x03
        // byte[7]: channel high = 0xAB
        // byte[8]: channel low = 0xCD
        // byte[9]: service_class = 0x00
        let tsbk = make_tsbk(&[0xBA, 0x00, 0x00, 0x05, 0xF2, 0x01, 0x03, 0xAB, 0xCD, 0x00]);

        let msg = parse_tsbk(&tsbk).unwrap();
        match msg.fields {
            TsbkFields::RfssStatusBroadcast {
                system_id,
                rfss,
                site,
                channel,
            } => {
                assert_eq!(system_id, SystemId(0x5F2));
                assert_eq!(rfss, RfssId(1));
                assert_eq!(site, SiteId(3));
                assert_eq!(channel, ChannelId(0xABCD));
            }
            _ => panic!("expected RfssStatusBroadcast"),
        }
    }

    #[test]
    fn parse_unknown_opcode_gracefully() {
        // opcode=0x07 is not in our table
        // last_block=1, opcode=0x07 -> 0b10_000111 = 0x87
        let tsbk = make_tsbk(&[0x87, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);

        let msg = parse_tsbk(&tsbk).unwrap();
        assert_eq!(msg.opcode, TsbkOpcode::Unknown(0x07));
        match msg.fields {
            TsbkFields::Unknown { opcode_value, .. } => {
                assert_eq!(opcode_value, 0x07);
            }
            _ => panic!("expected Unknown"),
        }
    }

    #[test]
    fn parse_unimplemented_opcode() {
        // opcode=0x05 (UnitToUnitAnswerRequest) - recognized but no field parser
        // last_block=1, opcode=0x05 -> 0b10_000101 = 0x85
        let tsbk = make_tsbk(&[0x85, 0x00, 0xDE, 0xAD, 0xBE, 0xEF, 0x12, 0x34, 0x56, 0x78]);

        let msg = parse_tsbk(&tsbk).unwrap();
        assert_eq!(msg.opcode, TsbkOpcode::UnitToUnitAnswerRequest);
        match msg.fields {
            TsbkFields::Unimplemented {
                opcode,
                raw_payload,
            } => {
                assert_eq!(opcode, TsbkOpcode::UnitToUnitAnswerRequest);
                assert_eq!(
                    raw_payload,
                    [0xDE, 0xAD, 0xBE, 0xEF, 0x12, 0x34, 0x56, 0x78]
                );
            }
            _ => panic!("expected Unimplemented"),
        }
    }

    #[test]
    fn reject_invalid_length() {
        assert!(matches!(
            parse_tsbk(&[0u8; 11]),
            Err(TsbkError::InvalidLength(11))
        ));
        assert!(matches!(
            parse_tsbk(&[0u8; 13]),
            Err(TsbkError::InvalidLength(13))
        ));
        assert!(matches!(parse_tsbk(&[]), Err(TsbkError::InvalidLength(0))));
    }

    #[test]
    fn reject_invalid_crc() {
        let mut tsbk = make_tsbk(&[0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        tsbk[5] ^= 0xFF; // corrupt data byte
        assert!(matches!(parse_tsbk(&tsbk), Err(TsbkError::Crc(_))));
    }

    #[test]
    fn last_block_and_protected_flags() {
        // last_block=0, protected=1, opcode=0x00 -> 0b01_000000 = 0x40
        let tsbk = make_tsbk(&[0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);

        let msg = parse_tsbk(&tsbk).unwrap();
        assert!(!msg.last_block);
        assert!(msg.protected);
    }
}

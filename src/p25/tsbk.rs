//! Trunking Signaling Block (TSBK) packet parser.
//!
//! Parses decoded TSBK payloads (12 bytes) into structured fields based
//! on the opcode. Handles CRC-16 verification and opcode dispatch.

use crate::p25::crc;
use crate::p25::error::P25Error;
use crate::p25::types::{
    ChannelNumber, Dibit, RfssId, SiteId, SourceId, SystemId, TalkgroupId, Wacn,
};

/// TSBK opcode identifying the message type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TsbkOpcode {
    /// Group voice channel grant (0x00).
    GroupVoiceChannelGrant,
    /// Group voice channel grant update (0x02).
    GroupVoiceChannelGrantUpdate,
    /// Identifier update (0x20).
    IdentifierUpdate,
    /// Identifier update VHF/UHF (0x34).
    IdentifierUpdateVu,
    /// Network status broadcast (0x39).
    NetworkStatusBroadcast,
    /// RFSS status broadcast (0x3A).
    RfssStatusBroadcast,
    /// Network status broadcast (alternate) (0x3B).
    NetworkStatusBroadcastAlt,
    /// Adjacent site status broadcast (0x3C).
    AdjacentStatusBroadcast,
    /// Channel parameters update (0x3D).
    ChannelParametersUpdate,
    /// Unknown opcode.
    Unknown(u8),
}

impl TsbkOpcode {
    /// Parse an opcode from the 6-bit value in byte 0 of a TSBK.
    pub fn from_bits(bits: u8) -> Self {
        match bits & 0x3F {
            0x00 => Self::GroupVoiceChannelGrant,
            0x02 => Self::GroupVoiceChannelGrantUpdate,
            0x20 => Self::IdentifierUpdate,
            0x34 => Self::IdentifierUpdateVu,
            0x39 => Self::NetworkStatusBroadcast,
            0x3A => Self::RfssStatusBroadcast,
            0x3B => Self::NetworkStatusBroadcastAlt,
            0x3C => Self::AdjacentStatusBroadcast,
            0x3D => Self::ChannelParametersUpdate,
            other => Self::Unknown(other),
        }
    }

    /// Return the opcode name as a string for JSON output.
    pub fn name(&self) -> &'static str {
        match self {
            Self::GroupVoiceChannelGrant => "GRP_V_CH_GRANT",
            Self::GroupVoiceChannelGrantUpdate => "GRP_V_CH_GRANT_UPDT",
            Self::IdentifierUpdate => "IDENT_UP",
            Self::IdentifierUpdateVu => "IDEN_UP_VU",
            Self::NetworkStatusBroadcast => "NET_STS_BCST",
            Self::RfssStatusBroadcast => "RFSS_STS_BCST",
            Self::NetworkStatusBroadcastAlt => "NET_STS_BCST",
            Self::AdjacentStatusBroadcast => "ADJ_STS_BCST",
            Self::ChannelParametersUpdate => "IDENT_UP",
            Self::Unknown(_) => "UNKNOWN",
        }
    }

    /// Return the raw 6-bit opcode value.
    pub fn raw(&self) -> u8 {
        match self {
            Self::GroupVoiceChannelGrant => 0x00,
            Self::GroupVoiceChannelGrantUpdate => 0x02,
            Self::IdentifierUpdate => 0x20,
            Self::IdentifierUpdateVu => 0x34,
            Self::NetworkStatusBroadcast => 0x39,
            Self::RfssStatusBroadcast => 0x3A,
            Self::NetworkStatusBroadcastAlt => 0x3B,
            Self::AdjacentStatusBroadcast => 0x3C,
            Self::ChannelParametersUpdate => 0x3D,
            Self::Unknown(v) => *v,
        }
    }
}

/// Common TSBK header fields extracted from the first two bytes.
#[derive(Debug, Clone, Copy)]
pub struct TsbkHeader {
    /// Whether this is the last TSBK in the TSDU.
    pub last_block: bool,
    /// Whether the payload is encrypted.
    pub protected: bool,
    /// Message type.
    pub opcode: TsbkOpcode,
    /// Manufacturer ID (0x00 = standard).
    pub manufacturer_id: u8,
}

/// Parsed TSBK message with opcode-specific payload.
#[derive(Debug, Clone)]
pub struct Tsbk {
    /// Common header fields.
    pub header: TsbkHeader,
    /// Opcode-specific parsed payload.
    pub payload: TsbkPayload,
}

/// Opcode-specific TSBK payload data.
#[derive(Debug, Clone)]
pub enum TsbkPayload {
    /// Group voice channel grant.
    GroupVoiceChannelGrant {
        /// Channel to tune to.
        channel: ChannelNumber,
        /// Talkgroup for the conversation.
        talkgroup: TalkgroupId,
        /// Unit that initiated the conversation.
        source: SourceId,
    },
    /// Group voice channel grant update (two channel/talkgroup pairs).
    GroupVoiceChannelGrantUpdate {
        /// First channel.
        channel_a: ChannelNumber,
        /// First talkgroup.
        talkgroup_a: TalkgroupId,
        /// Second channel.
        channel_b: ChannelNumber,
        /// Second talkgroup.
        talkgroup_b: TalkgroupId,
    },
    /// Identifier table update.
    IdentifierUpdate {
        /// 4-bit identifier.
        identifier: u8,
        /// Channel bandwidth in hertz.
        bandwidth: u32,
        /// Transmit offset in hertz (signed).
        transmit_offset: i64,
        /// Channel spacing in hertz.
        channel_spacing: u32,
        /// Base frequency in hertz.
        base_frequency: u64,
    },
    /// Network status broadcast.
    NetworkStatusBroadcast {
        /// Wide Area Communication Network identifier.
        wacn: Wacn,
        /// System identifier.
        system_id: SystemId,
        /// Control channel.
        channel: ChannelNumber,
    },
    /// RFSS status broadcast.
    RfssStatusBroadcast {
        /// System identifier.
        system_id: SystemId,
        /// RF subsystem identifier.
        rfss_id: RfssId,
        /// Site identifier.
        site_id: SiteId,
        /// Control channel.
        channel: ChannelNumber,
    },
    /// Adjacent site status broadcast.
    AdjacentStatusBroadcast {
        /// System identifier.
        system_id: SystemId,
        /// RF subsystem identifier.
        rfss_id: RfssId,
        /// Site identifier.
        site_id: SiteId,
        /// Control channel.
        channel: ChannelNumber,
    },
    /// Unknown or unhandled opcode (raw payload bytes).
    Unknown {
        /// Raw payload bytes (2..10).
        data: [u8; 8],
    },
}

/// Parse a 12-byte TSBK, verifying CRC and extracting fields.
pub fn parse(data: &[u8; 12]) -> Result<Tsbk, P25Error> {
    let computed_crc = crc::crc16(&data[..10]);
    let stored_crc = u16::from_be_bytes([data[10], data[11]]);

    if computed_crc != stored_crc {
        return Err(P25Error::CrcMismatch {
            expected: stored_crc,
            actual: computed_crc,
        });
    }

    let header = parse_header(data);
    let payload = parse_payload(&header, data);

    Ok(Tsbk { header, payload })
}

/// Parse a 12-byte TSBK without CRC verification.
///
/// Useful when CRC has already been checked or for testing.
pub fn parse_unchecked(data: &[u8; 12]) -> Tsbk {
    let header = parse_header(data);
    let payload = parse_payload(&header, data);
    Tsbk { header, payload }
}

/// Extract header fields from the first two bytes.
fn parse_header(data: &[u8; 12]) -> TsbkHeader {
    TsbkHeader {
        last_block: data[0] & 0x80 != 0,
        protected: data[0] & 0x40 != 0,
        opcode: TsbkOpcode::from_bits(data[0] & 0x3F),
        manufacturer_id: data[1],
    }
}

/// Parse opcode-specific payload fields.
fn parse_payload(header: &TsbkHeader, data: &[u8; 12]) -> TsbkPayload {
    match header.opcode {
        TsbkOpcode::GroupVoiceChannelGrant => parse_group_voice_grant(data),
        TsbkOpcode::GroupVoiceChannelGrantUpdate => parse_group_voice_grant_update(data),
        TsbkOpcode::IdentifierUpdate | TsbkOpcode::ChannelParametersUpdate => {
            parse_identifier_update(data)
        }
        TsbkOpcode::IdentifierUpdateVu => parse_identifier_update(data),
        TsbkOpcode::NetworkStatusBroadcast | TsbkOpcode::NetworkStatusBroadcastAlt => {
            parse_network_status_broadcast(data)
        }
        TsbkOpcode::RfssStatusBroadcast => parse_rfss_status_broadcast(data),
        TsbkOpcode::AdjacentStatusBroadcast => parse_adjacent_status_broadcast(data),
        TsbkOpcode::Unknown(_) => parse_unknown(data),
    }
}

/// Parse group voice channel grant (opcode 0x00).
///
/// Layout: [opts(1), ch(2), tg(2), src(3)]
fn parse_group_voice_grant(data: &[u8; 12]) -> TsbkPayload {
    TsbkPayload::GroupVoiceChannelGrant {
        channel: ChannelNumber::new(u16::from_be_bytes([data[2], data[3]])),
        talkgroup: TalkgroupId::new(u16::from_be_bytes([data[4], data[5]])),
        source: SourceId::new(u32::from_be_bytes([0, data[6], data[7], data[8]])),
    }
}

/// Parse group voice channel grant update (opcode 0x02).
///
/// Layout: [ch_a(2), tg_a(2), ch_b(2), tg_b(2)]
fn parse_group_voice_grant_update(data: &[u8; 12]) -> TsbkPayload {
    TsbkPayload::GroupVoiceChannelGrantUpdate {
        channel_a: ChannelNumber::new(u16::from_be_bytes([data[2], data[3]])),
        talkgroup_a: TalkgroupId::new(u16::from_be_bytes([data[4], data[5]])),
        channel_b: ChannelNumber::new(u16::from_be_bytes([data[6], data[7]])),
        talkgroup_b: TalkgroupId::new(u16::from_be_bytes([data[8], data[9]])),
    }
}

/// Parse identifier update (opcodes 0x20, 0x34, 0x3D).
///
/// Layout (bytes 2-9, 64 bits total):
///   Identifier: 4 bits
///   Bandwidth: 9 bits (x 125 Hz)
///   Transmit Offset: 9 bits (x 250 kHz, MSB=sign)
///   Channel Spacing: 10 bits (x 125 Hz)
///   Base Frequency: 32 bits (x 5 Hz)
fn parse_identifier_update(data: &[u8; 12]) -> TsbkPayload {
    let word = u64::from_be_bytes([
        data[2], data[3], data[4], data[5], data[6], data[7], data[8], data[9],
    ]);

    let identifier = ((word >> 60) & 0x0F) as u8;
    let bandwidth = ((word >> 51) & 0x1FF) as u32 * 125;

    // Offset: 9 bits, MSB is sign bit, lower 8 bits are magnitude.
    // MSB=0 means negative offset, MSB=1 means positive (per p25.rs).
    let off_raw = ((word >> 42) & 0x1FF) as u16;
    let magnitude = (off_raw & 0xFF) as i64 * 250_000;
    let transmit_offset = if off_raw >> 8 == 0 {
        -magnitude
    } else {
        magnitude
    };

    let channel_spacing = ((word >> 32) & 0x3FF) as u32 * 125;
    let base_frequency = (word & 0xFFFF_FFFF) * 5;

    TsbkPayload::IdentifierUpdate {
        identifier,
        bandwidth,
        transmit_offset,
        channel_spacing,
        base_frequency,
    }
}

/// Parse network status broadcast (opcodes 0x39, 0x3B).
///
/// Layout bytes 2-9: [area(1), wacn_hi(1), wacn_lo_sys_hi(1), sys_lo(1), ch(2), svc(2)]
fn parse_network_status_broadcast(data: &[u8; 12]) -> TsbkPayload {
    // WACN is 20 bits: bytes 3-4 and top 4 of byte 5
    let wacn_raw = ((data[3] as u32) << 12) | ((data[4] as u32) << 4) | ((data[5] >> 4) as u32);

    // System ID is 12 bits: bottom 4 of byte 5 and byte 6
    let system_id_raw = (((data[5] & 0x0F) as u16) << 8) | data[6] as u16;

    let channel = u16::from_be_bytes([data[7], data[8]]);

    TsbkPayload::NetworkStatusBroadcast {
        wacn: Wacn::new(wacn_raw),
        system_id: SystemId::new(system_id_raw),
        channel: ChannelNumber::new(channel),
    }
}

/// Parse RFSS status broadcast (opcode 0x3A).
///
/// Layout bytes 2-9: [area(1), sys(1.5), rfss(1), site(1), ch(2), svc(1)]
fn parse_rfss_status_broadcast(data: &[u8; 12]) -> TsbkPayload {
    // System ID = bottom 4 of byte 3 + byte 4 = 12 bits
    let system_id_raw = (((data[3] & 0x0F) as u16) << 8) | data[4] as u16;

    TsbkPayload::RfssStatusBroadcast {
        system_id: SystemId::new(system_id_raw),
        rfss_id: RfssId::new(data[5]),
        site_id: SiteId::new(data[6]),
        channel: ChannelNumber::new(u16::from_be_bytes([data[7], data[8]])),
    }
}

/// Parse adjacent site status broadcast (opcode 0x3C).
///
/// Layout bytes 2-9: [area(1), sys_hi(0.5), sys_lo(1), rfss(1), site(1), ch(2), svc(1)]
fn parse_adjacent_status_broadcast(data: &[u8; 12]) -> TsbkPayload {
    let system_id_raw = (((data[3] & 0x0F) as u16) << 8) | data[4] as u16;

    TsbkPayload::AdjacentStatusBroadcast {
        system_id: SystemId::new(system_id_raw),
        rfss_id: RfssId::new(data[5]),
        site_id: SiteId::new(data[6]),
        channel: ChannelNumber::new(u16::from_be_bytes([data[7], data[8]])),
    }
}

/// Parse unknown or unhandled opcode.
fn parse_unknown(data: &[u8; 12]) -> TsbkPayload {
    let mut payload_data = [0u8; 8];
    payload_data.copy_from_slice(&data[2..10]);
    TsbkPayload::Unknown { data: payload_data }
}

/// Convert 48 decoded dibits into 12 bytes.
pub fn dibits_to_bytes(dibits: &[Dibit]) -> Result<[u8; 12], P25Error> {
    if dibits.len() != 48 {
        return Err(P25Error::PayloadTooShort {
            expected: 48,
            actual: dibits.len(),
        });
    }

    let mut bytes = [0u8; 12];
    for (i, chunk) in dibits.chunks_exact(4).enumerate() {
        bytes[i] = (chunk[0].bits() << 6)
            | (chunk[1].bits() << 4)
            | (chunk[2].bits() << 2)
            | chunk[3].bits();
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tsbk_with_crc(data: &[u8; 10]) -> [u8; 12] {
        let crc = crc::crc16(data);
        let mut tsbk = [0u8; 12];
        tsbk[..10].copy_from_slice(data);
        tsbk[10] = (crc >> 8) as u8;
        tsbk[11] = crc as u8;
        tsbk
    }

    #[test]
    fn parse_header_fields() {
        // From p25.rs test: [0xB9, 0x01, ...]
        // 0xB9 = 0b10111001: last_block=1, protected=0, opcode=0x39
        let tsbk =
            make_tsbk_with_crc(&[0xB9, 0x01, 0xF0, 0x0F, 0xAA, 0x55, 0x00, 0xFF, 0xCC, 0x33]);
        let parsed = parse(&tsbk).unwrap();
        assert!(parsed.header.last_block);
        assert!(!parsed.header.protected);
        assert_eq!(parsed.header.opcode, TsbkOpcode::NetworkStatusBroadcast);
        assert_eq!(parsed.header.manufacturer_id, 0x01);
    }

    #[test]
    fn parse_rejects_bad_crc() {
        let tsbk = [
            0xB9, 0x01, 0xF0, 0x0F, 0xAA, 0x55, 0x00, 0xFF, 0xCC, 0x33, 0xD7, 0xD7,
        ];
        let result = parse(&tsbk);
        assert!(result.is_err());
        match result.unwrap_err() {
            P25Error::CrcMismatch { expected, actual } => {
                assert_eq!(expected, 0xD7D7);
                assert_eq!(actual, crc::crc16(&tsbk[..10]));
            }
            other => panic!("expected CrcMismatch, got {other:?}"),
        }
    }

    #[test]
    fn parse_unchecked_skips_crc() {
        let tsbk = [
            0xB9, 0x01, 0xF0, 0x0F, 0xAA, 0x55, 0x00, 0xFF, 0xCC, 0x33, 0xD7, 0xD7,
        ];
        let parsed = parse_unchecked(&tsbk);
        assert!(parsed.header.last_block);
        assert_eq!(parsed.header.opcode, TsbkOpcode::NetworkStatusBroadcast);
    }

    #[test]
    fn opcode_from_bits_known_values() {
        assert_eq!(
            TsbkOpcode::from_bits(0x00),
            TsbkOpcode::GroupVoiceChannelGrant
        );
        assert_eq!(
            TsbkOpcode::from_bits(0x02),
            TsbkOpcode::GroupVoiceChannelGrantUpdate
        );
        assert_eq!(TsbkOpcode::from_bits(0x20), TsbkOpcode::IdentifierUpdate);
        assert_eq!(TsbkOpcode::from_bits(0x34), TsbkOpcode::IdentifierUpdateVu);
        assert_eq!(
            TsbkOpcode::from_bits(0x39),
            TsbkOpcode::NetworkStatusBroadcast
        );
        assert_eq!(TsbkOpcode::from_bits(0x3A), TsbkOpcode::RfssStatusBroadcast);
        assert_eq!(
            TsbkOpcode::from_bits(0x3C),
            TsbkOpcode::AdjacentStatusBroadcast
        );
        assert_eq!(
            TsbkOpcode::from_bits(0x3D),
            TsbkOpcode::ChannelParametersUpdate
        );
    }

    #[test]
    fn opcode_unknown_preserved() {
        assert_eq!(TsbkOpcode::from_bits(0x0F), TsbkOpcode::Unknown(0x0F));
        assert_eq!(TsbkOpcode::from_bits(0x0F).raw(), 0x0F);
    }

    #[test]
    fn opcode_raw_roundtrip() {
        for code in 0..=0x3Fu8 {
            let opcode = TsbkOpcode::from_bits(code);
            assert_eq!(opcode.raw(), code);
        }
    }

    #[test]
    fn parse_group_voice_channel_grant() {
        // opcode 0x00, mfid 0x00
        let data: [u8; 10] = [
            0x80, // last_block=1, protected=0, opcode=0x00
            0x00, // mfid
            0x61, 0x23, // channel 0x6123
            0x00, 0x42, // talkgroup 66
            0x01, 0x02, 0x03, // source 0x010203
            0x00, // padding (will be overwritten by CRC)
        ];
        let tsbk = make_tsbk_with_crc(&data);
        let parsed = parse(&tsbk).unwrap();

        assert_eq!(parsed.header.opcode, TsbkOpcode::GroupVoiceChannelGrant);
        match parsed.payload {
            TsbkPayload::GroupVoiceChannelGrant {
                channel,
                talkgroup,
                source,
            } => {
                assert_eq!(channel.value(), 0x6123);
                assert_eq!(channel.identifier(), 6);
                assert_eq!(channel.index(), 0x123);
                assert_eq!(talkgroup.value(), 66);
                assert_eq!(source.value(), 0x010203);
            }
            other => panic!("expected GroupVoiceChannelGrant, got {other:?}"),
        }
    }

    #[test]
    fn parse_group_voice_grant_update() {
        let data: [u8; 10] = [
            0x82, // last_block=1, protected=0, opcode=0x02
            0x00, 0x61, 0x23, // channel_a
            0x00, 0x42, // talkgroup_a
            0x71, 0x45, // channel_b
            0x00, 0x99, // talkgroup_b
        ];
        let tsbk = make_tsbk_with_crc(&data);
        let parsed = parse(&tsbk).unwrap();

        match parsed.payload {
            TsbkPayload::GroupVoiceChannelGrantUpdate {
                channel_a,
                talkgroup_a,
                channel_b,
                talkgroup_b,
            } => {
                assert_eq!(channel_a.value(), 0x6123);
                assert_eq!(talkgroup_a.value(), 66);
                assert_eq!(channel_b.value(), 0x7145);
                assert_eq!(talkgroup_b.value(), 0x0099);
            }
            other => panic!("expected GroupVoiceChannelGrantUpdate, got {other:?}"),
        }
    }

    #[test]
    fn parse_rfss_status_broadcast() {
        // From p25.rs test adjusted for our format
        let data: [u8; 10] = [
            0xBA, // last_block=1, protected=0, opcode=0x3A
            0x00, 0xCC, // area
            0x10, 0xAA, // system: 0x0AA
            0xE7, // rfss
            0x18, // site
            0xD5, 0x73, // channel
            0x51, // services
        ];
        let tsbk = make_tsbk_with_crc(&data);
        let parsed = parse(&tsbk).unwrap();

        match parsed.payload {
            TsbkPayload::RfssStatusBroadcast {
                system_id,
                rfss_id,
                site_id,
                channel,
            } => {
                assert_eq!(system_id.value(), 0x0AA);
                assert_eq!(rfss_id.value(), 0xE7);
                assert_eq!(site_id.value(), 0x18);
                assert_eq!(channel.value(), 0xD573);
                assert_eq!(channel.identifier(), 0xD);
                assert_eq!(channel.index(), 0x573);
            }
            other => panic!("expected RfssStatusBroadcast, got {other:?}"),
        }
    }

    #[test]
    fn parse_network_status_broadcast() {
        let data: [u8; 10] = [
            0xB9, // last_block=1, protected=0, opcode=0x39
            0x00, 0xCA, // area
            0xFC, 0x2B, // wacn hi
            0xCF, // wacn lo | sys hi
            0x5B, // sys lo
            0xDC, 0xE7, // channel
            0x51, // services
        ];
        let tsbk = make_tsbk_with_crc(&data);
        let parsed = parse(&tsbk).unwrap();

        match parsed.payload {
            TsbkPayload::NetworkStatusBroadcast {
                wacn,
                system_id,
                channel,
            } => {
                assert_eq!(wacn.value(), 0xFC2BC);
                assert_eq!(system_id.value(), 0xF5B);
                assert_eq!(channel.value(), 0xDCE7);
            }
            other => panic!("expected NetworkStatusBroadcast, got {other:?}"),
        }
    }

    #[test]
    fn parse_adjacent_status_broadcast() {
        let data: [u8; 10] = [
            0xBC, // last_block=1, protected=0, opcode=0x3C
            0x00, 0xAA, // area
            0x1B, 0xCD, // system
            0x05, // rfss
            0x0A, // site
            0x61, 0x23, // channel
            0xFF, // services
        ];
        let tsbk = make_tsbk_with_crc(&data);
        let parsed = parse(&tsbk).unwrap();

        match parsed.payload {
            TsbkPayload::AdjacentStatusBroadcast {
                system_id,
                rfss_id,
                site_id,
                channel,
            } => {
                assert_eq!(system_id.value(), 0xBCD);
                assert_eq!(rfss_id.value(), 5);
                assert_eq!(site_id.value(), 10);
                assert_eq!(channel.value(), 0x6123);
            }
            other => panic!("expected AdjacentStatusBroadcast, got {other:?}"),
        }
    }

    #[test]
    fn parse_unknown_opcode_captures_payload() {
        let data: [u8; 10] = [
            0x8F, // last_block=1, protected=0, opcode=0x0F (unknown)
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88,
        ];
        let tsbk = make_tsbk_with_crc(&data);
        let parsed = parse(&tsbk).unwrap();

        assert_eq!(parsed.header.opcode, TsbkOpcode::Unknown(0x0F));
        match parsed.payload {
            TsbkPayload::Unknown { data } => {
                assert_eq!(data, [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]);
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn parse_protected_flag() {
        // opcode 0x00 with protected=1: byte 0 = 0b11000000 = 0xC0
        let data: [u8; 10] = [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let tsbk = make_tsbk_with_crc(&data);
        let parsed = parse(&tsbk).unwrap();
        assert!(parsed.header.protected);
        assert!(parsed.header.last_block);
    }

    #[test]
    fn dibits_to_bytes_correct() {
        let dibits: Vec<Dibit> = [
            0b00, 0b11, 0b00, 0b11, // 0x33
            0b10, 0b01, 0b10, 0b01, // 0x99
            0b11, 0b11, 0b11, 0b11, // 0xFF
        ]
        .iter()
        .flat_map(|&b| std::iter::once(Dibit::new(b)))
        .collect();

        // Need 48 dibits, pad with zeros
        let mut full = dibits;
        while full.len() < 48 {
            full.push(Dibit::new(0));
        }

        let bytes = dibits_to_bytes(&full).unwrap();
        assert_eq!(bytes[0], 0b00110011);
        assert_eq!(bytes[1], 0b10011001);
        assert_eq!(bytes[2], 0b11111111);
    }

    #[test]
    fn dibits_to_bytes_wrong_length() {
        let short: Vec<Dibit> = vec![Dibit::new(0); 40];
        assert!(dibits_to_bytes(&short).is_err());
    }

    #[test]
    fn identifier_update_parsing() {
        // From p25.rs test vector for ChannelParamsUpdate (opcode 0x3D):
        // id=6, bw=12500 Hz, offset=-45 MHz, spacing=6250 Hz, base=851006250 Hz
        // rx_freq(9) = 851006250 + 6250*9 = 851062500
        let data: [u8; 10] = [
            0xBD, // last_block=1, opcode=0x3D
            0x00, 0x63, 0x22, 0xD0, 0x32, 0x0A, 0x25, 0x10, 0xA2,
        ];
        let tsbk = make_tsbk_with_crc(&data);
        let parsed = parse(&tsbk).unwrap();

        match parsed.payload {
            TsbkPayload::IdentifierUpdate {
                identifier,
                bandwidth,
                transmit_offset,
                channel_spacing,
                base_frequency,
            } => {
                assert_eq!(identifier, 6);
                assert_eq!(bandwidth, 12_500);
                assert_eq!(transmit_offset, -45_000_000);
                assert_eq!(channel_spacing, 6_250);
                assert_eq!(base_frequency, 851_006_250);
                // Verify frequency calculation:
                // freq(9) = base_frequency + channel_spacing * 9
                let freq = base_frequency + (channel_spacing as u64) * 9;
                assert_eq!(freq, 851_062_500);
            }
            other => panic!("expected IdentifierUpdate, got {other:?}"),
        }
    }
}

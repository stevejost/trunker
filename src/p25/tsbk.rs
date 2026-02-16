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
    /// Group voice channel grant update - explicit (0x03).
    GroupVoiceChannelGrantUpdateExplicit,
    /// Unit-to-unit answer request (0x05).
    UnitToUnitAnswerRequest,
    /// Emergency alarm (0x09).
    EmergencyAlarm,
    /// SNDCP data channel grant (0x14).
    SndcpDataChannelGrant,
    /// Deny response (0x16).
    DenyResponse,
    /// Identifier update (0x20).
    IdentifierUpdate,
    /// Group affiliation response (0x28).
    GroupAffiliationResponse,
    /// Unit registration response (0x2C).
    UnitRegistrationResponse,
    /// Unit deregistration acknowledgement (0x2F).
    UnitDeregistrationAck,
    /// Power control broadcast (0x30).
    PowerControlBroadcast,
    /// Identifier update TDMA (0x33).
    IdentifierUpdateTdma,
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
            0x03 => Self::GroupVoiceChannelGrantUpdateExplicit,
            0x05 => Self::UnitToUnitAnswerRequest,
            0x09 => Self::EmergencyAlarm,
            0x14 => Self::SndcpDataChannelGrant,
            0x16 => Self::DenyResponse,
            0x20 => Self::IdentifierUpdate,
            0x28 => Self::GroupAffiliationResponse,
            0x2C => Self::UnitRegistrationResponse,
            0x2F => Self::UnitDeregistrationAck,
            0x30 => Self::PowerControlBroadcast,
            0x33 => Self::IdentifierUpdateTdma,
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
            Self::GroupVoiceChannelGrantUpdateExplicit => "GRP_V_CH_GRANT_UPDT_EXP",
            Self::UnitToUnitAnswerRequest => "UNT_TO_UNT_ANS_REQ",
            Self::EmergencyAlarm => "EMERGENCY_ALRM",
            Self::SndcpDataChannelGrant => "SNDCP_DATA_CH_GRANT",
            Self::DenyResponse => "DENY_RSP",
            Self::IdentifierUpdate => "IDENT_UP",
            Self::GroupAffiliationResponse => "GRP_AFF_RSP",
            Self::UnitRegistrationResponse => "U_REG_RSP",
            Self::UnitDeregistrationAck => "U_DE_REG_ACK",
            Self::PowerControlBroadcast => "PWR_CTRL_BCST",
            Self::IdentifierUpdateTdma => "IDEN_UP_TDMA",
            Self::IdentifierUpdateVu => "IDEN_UP_VU",
            Self::NetworkStatusBroadcast => "NET_STS_BCST",
            Self::RfssStatusBroadcast => "RFSS_STS_BCST",
            Self::NetworkStatusBroadcastAlt => "NET2_STS_BCST",
            Self::AdjacentStatusBroadcast => "ADJ_STS_BCST",
            Self::ChannelParametersUpdate => "CH_PARAMS_UPDT",
            Self::Unknown(_) => "UNKNOWN",
        }
    }

    /// Return the raw 6-bit opcode value.
    pub fn raw(&self) -> u8 {
        match self {
            Self::GroupVoiceChannelGrant => 0x00,
            Self::GroupVoiceChannelGrantUpdate => 0x02,
            Self::GroupVoiceChannelGrantUpdateExplicit => 0x03,
            Self::UnitToUnitAnswerRequest => 0x05,
            Self::EmergencyAlarm => 0x09,
            Self::SndcpDataChannelGrant => 0x14,
            Self::DenyResponse => 0x16,
            Self::IdentifierUpdate => 0x20,
            Self::GroupAffiliationResponse => 0x28,
            Self::UnitRegistrationResponse => 0x2C,
            Self::UnitDeregistrationAck => 0x2F,
            Self::PowerControlBroadcast => 0x30,
            Self::IdentifierUpdateTdma => 0x33,
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
    /// Group voice channel grant update - explicit (transmit and receive channels).
    GroupVoiceChannelGrantUpdateExplicit {
        /// Service options.
        service_options: u8,
        /// Transmit channel.
        transmit_channel: ChannelNumber,
        /// Receive channel.
        receive_channel: ChannelNumber,
        /// Talkgroup.
        talkgroup: TalkgroupId,
    },
    /// Unit-to-unit answer request.
    UnitToUnitAnswerRequest {
        /// Service options.
        service_options: u8,
        /// Target (destination) unit.
        target: SourceId,
        /// Source unit.
        source: SourceId,
    },
    /// Emergency alarm from a unit.
    EmergencyAlarm {
        /// Target unit or talkgroup.
        target: SourceId,
        /// Source unit that triggered the alarm.
        source: SourceId,
    },
    /// SNDCP data channel grant.
    SndcpDataChannelGrant {
        /// Data channel.
        data_channel: ChannelNumber,
        /// Target unit.
        target: SourceId,
    },
    /// Deny response.
    DenyResponse {
        /// Service type being denied.
        service_type: u8,
        /// Deny reason.
        reason: u8,
        /// Additional information.
        additional: SourceId,
        /// Target unit.
        target: SourceId,
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
    /// Group affiliation response.
    GroupAffiliationResponse {
        /// Local/global flag (0=local, 1=global).
        local_global: u8,
        /// Group affiliation value.
        group_affiliation_value: u8,
        /// Announcement group address.
        announcement_group: TalkgroupId,
        /// Group address.
        group: TalkgroupId,
        /// Target unit.
        target: SourceId,
    },
    /// Unit registration response.
    UnitRegistrationResponse {
        /// Response value (0=accept, 1=fail, 2=deny, 3=refuse).
        response: u8,
        /// System identifier.
        system_id: SystemId,
        /// Source identifier.
        source_id: SourceId,
        /// Source address.
        source_address: SourceId,
    },
    /// Unit deregistration acknowledgement.
    UnitDeregistrationAck {
        /// WACN identifier.
        wacn: Wacn,
        /// System identifier.
        system_id: SystemId,
        /// Source unit.
        source: SourceId,
    },
    /// Power control broadcast.
    PowerControlBroadcast {
        /// Raw payload bytes (manufacturer-specific).
        data: [u8; 8],
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
        TsbkOpcode::GroupVoiceChannelGrantUpdateExplicit => {
            parse_group_voice_grant_update_explicit(data)
        }
        TsbkOpcode::UnitToUnitAnswerRequest => parse_unit_to_unit_answer_request(data),
        TsbkOpcode::EmergencyAlarm => parse_emergency_alarm(data),
        TsbkOpcode::SndcpDataChannelGrant => parse_sndcp_data_channel_grant(data),
        TsbkOpcode::DenyResponse => parse_deny_response(data),
        TsbkOpcode::IdentifierUpdate | TsbkOpcode::ChannelParametersUpdate => {
            parse_identifier_update(data)
        }
        TsbkOpcode::GroupAffiliationResponse => parse_group_affiliation_response(data),
        TsbkOpcode::UnitRegistrationResponse => parse_unit_registration_response(data),
        TsbkOpcode::UnitDeregistrationAck => parse_unit_deregistration_ack(data),
        TsbkOpcode::PowerControlBroadcast => parse_power_control_broadcast(data),
        TsbkOpcode::IdentifierUpdateTdma | TsbkOpcode::IdentifierUpdateVu => {
            parse_identifier_update_vu(data)
        }
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

/// Parse group voice channel grant update - explicit (opcode 0x03).
///
/// Layout (mfrid=0x00): [opts(1), reserved(1), ch_t(2), ch_r(2), tg(2)]
fn parse_group_voice_grant_update_explicit(data: &[u8; 12]) -> TsbkPayload {
    TsbkPayload::GroupVoiceChannelGrantUpdateExplicit {
        service_options: data[2],
        transmit_channel: ChannelNumber::new(u16::from_be_bytes([data[4], data[5]])),
        receive_channel: ChannelNumber::new(u16::from_be_bytes([data[6], data[7]])),
        talkgroup: TalkgroupId::new(u16::from_be_bytes([data[8], data[9]])),
    }
}

/// Parse unit-to-unit answer request (opcode 0x05).
///
/// Layout: [opts(1), reserved(1), target(3), source(3)]
fn parse_unit_to_unit_answer_request(data: &[u8; 12]) -> TsbkPayload {
    TsbkPayload::UnitToUnitAnswerRequest {
        service_options: data[2],
        target: SourceId::new(u32::from_be_bytes([0, data[4], data[5], data[6]])),
        source: SourceId::new(u32::from_be_bytes([0, data[7], data[8], data[9]])),
    }
}

/// Parse emergency alarm (opcode 0x09).
///
/// Layout: [reserved(2), target(3), source(3)]
fn parse_emergency_alarm(data: &[u8; 12]) -> TsbkPayload {
    TsbkPayload::EmergencyAlarm {
        target: SourceId::new(u32::from_be_bytes([0, data[4], data[5], data[6]])),
        source: SourceId::new(u32::from_be_bytes([0, data[7], data[8], data[9]])),
    }
}

/// Parse SNDCP data channel grant (opcode 0x14).
///
/// Layout: [opts(1), reserved(1), ch(2), reserved(2), target(3)]
fn parse_sndcp_data_channel_grant(data: &[u8; 12]) -> TsbkPayload {
    TsbkPayload::SndcpDataChannelGrant {
        data_channel: ChannelNumber::new(u16::from_be_bytes([data[4], data[5]])),
        target: SourceId::new(u32::from_be_bytes([0, data[7], data[8], data[9]])),
    }
}

/// Parse deny response (opcode 0x16).
///
/// Layout: [svc_type(1), reason(1), additional(3), target(3)]
fn parse_deny_response(data: &[u8; 12]) -> TsbkPayload {
    TsbkPayload::DenyResponse {
        service_type: data[2],
        reason: data[3],
        additional: SourceId::new(u32::from_be_bytes([0, data[4], data[5], data[6]])),
        target: SourceId::new(u32::from_be_bytes([0, data[7], data[8], data[9]])),
    }
}

/// Parse group affiliation response (opcode 0x28).
///
/// Layout: [lg(1 bit), gav(2 bits), reserved, aga(2), ga(2), ta(3)]
fn parse_group_affiliation_response(data: &[u8; 12]) -> TsbkPayload {
    let lg = (data[2] >> 7) & 0x01;
    let gav = (data[2] >> 4) & 0x03;

    TsbkPayload::GroupAffiliationResponse {
        local_global: lg,
        group_affiliation_value: gav,
        announcement_group: TalkgroupId::new(u16::from_be_bytes([data[3], data[4]])),
        group: TalkgroupId::new(u16::from_be_bytes([data[5], data[6]])),
        target: SourceId::new(u32::from_be_bytes([0, data[7], data[8], data[9]])),
    }
}

/// Parse unit registration response (opcode 0x2C).
///
/// Layout: [rv(2 bits) + syid(12 bits) + reserved(2 bits), sid(3), sa(3)]
fn parse_unit_registration_response(data: &[u8; 12]) -> TsbkPayload {
    let rv = (data[2] >> 4) & 0x03;
    let system_id_raw = (((data[2] & 0x0F) as u16) << 8) | data[3] as u16;

    TsbkPayload::UnitRegistrationResponse {
        response: rv,
        system_id: SystemId::new(system_id_raw),
        source_id: SourceId::new(u32::from_be_bytes([0, data[4], data[5], data[6]])),
        source_address: SourceId::new(u32::from_be_bytes([0, data[7], data[8], data[9]])),
    }
}

/// Parse unit deregistration acknowledgement (opcode 0x2F).
///
/// Layout: [reserved(1), wacn(2.5), syid(1.5), sid(3)]
fn parse_unit_deregistration_ack(data: &[u8; 12]) -> TsbkPayload {
    let wacn_raw =
        ((data[3] as u32) << 12) | ((data[4] as u32) << 4) | ((data[5] >> 4) as u32);
    let system_id_raw = (((data[5] & 0x0F) as u16) << 8) | data[6] as u16;

    TsbkPayload::UnitDeregistrationAck {
        wacn: Wacn::new(wacn_raw),
        system_id: SystemId::new(system_id_raw),
        source: SourceId::new(u32::from_be_bytes([0, data[7], data[8], data[9]])),
    }
}

/// Parse power control broadcast (opcode 0x30).
///
/// Manufacturer-specific; store raw payload.
fn parse_power_control_broadcast(data: &[u8; 12]) -> TsbkPayload {
    let mut payload_data = [0u8; 8];
    payload_data.copy_from_slice(&data[2..10]);
    TsbkPayload::PowerControlBroadcast { data: payload_data }
}

/// Parse identifier update VHF/UHF (opcodes 0x33, 0x34).
///
/// Layout (bytes 2-9, 64 bits total):
///   Identifier: 4 bits
///   Channel type/BW: 4 bits
///   Transmit Offset: 14 bits (MSB=sign, lower 13 = magnitude * spacing * 125 Hz)
///   Channel Spacing: 10 bits (x 125 Hz)
///   Base Frequency: 32 bits (x 5 Hz)
fn parse_identifier_update_vu(data: &[u8; 12]) -> TsbkPayload {
    let word = u64::from_be_bytes([
        data[2], data[3], data[4], data[5], data[6], data[7], data[8], data[9],
    ]);

    let identifier = ((word >> 60) & 0x0F) as u8;
    // 4-bit channel type (0x33) or bandwidth type (0x34) -- not used in frequency calc.
    let _channel_type = ((word >> 56) & 0x0F) as u8;

    // Offset: 14 bits. MSB is sign, lower 13 are magnitude.
    // toff_sign=0 means negative offset, toff_sign=1 means positive.
    let toff_raw = ((word >> 42) & 0x3FFF) as u16;
    let toff_sign = (toff_raw >> 13) & 1;
    let toff_magnitude = (toff_raw & 0x1FFF) as i64;

    let channel_spacing_raw = ((word >> 32) & 0x3FF) as u32;
    let channel_spacing = channel_spacing_raw * 125;

    // Offset = magnitude * spacing * 125 (already incorporated in channel_spacing).
    // Per OP25: self.freq_table[iden]['offset'] = toff * spac * 125
    let transmit_offset = if toff_sign == 0 {
        -(toff_magnitude * channel_spacing as i64)
    } else {
        toff_magnitude * channel_spacing as i64
    };

    let base_frequency = (word & 0xFFFF_FFFF) * 5;

    TsbkPayload::IdentifierUpdate {
        identifier,
        bandwidth: channel_spacing, // VU format doesn't have a separate bandwidth field
        transmit_offset,
        channel_spacing,
        base_frequency,
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
        assert_eq!(
            TsbkOpcode::from_bits(0x03),
            TsbkOpcode::GroupVoiceChannelGrantUpdateExplicit
        );
        assert_eq!(
            TsbkOpcode::from_bits(0x05),
            TsbkOpcode::UnitToUnitAnswerRequest
        );
        assert_eq!(TsbkOpcode::from_bits(0x09), TsbkOpcode::EmergencyAlarm);
        assert_eq!(
            TsbkOpcode::from_bits(0x14),
            TsbkOpcode::SndcpDataChannelGrant
        );
        assert_eq!(TsbkOpcode::from_bits(0x16), TsbkOpcode::DenyResponse);
        assert_eq!(TsbkOpcode::from_bits(0x20), TsbkOpcode::IdentifierUpdate);
        assert_eq!(
            TsbkOpcode::from_bits(0x28),
            TsbkOpcode::GroupAffiliationResponse
        );
        assert_eq!(
            TsbkOpcode::from_bits(0x2C),
            TsbkOpcode::UnitRegistrationResponse
        );
        assert_eq!(
            TsbkOpcode::from_bits(0x2F),
            TsbkOpcode::UnitDeregistrationAck
        );
        assert_eq!(
            TsbkOpcode::from_bits(0x30),
            TsbkOpcode::PowerControlBroadcast
        );
        assert_eq!(
            TsbkOpcode::from_bits(0x33),
            TsbkOpcode::IdentifierUpdateTdma
        );
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

    #[test]
    fn parse_emergency_alarm() {
        let data: [u8; 10] = [
            0x89, // last_block=1, opcode=0x09
            0x00, // mfid
            0x00, 0x00, // reserved
            0x00, 0x01, 0x00, // target 0x000100
            0x0A, 0x0B, 0x0C, // source 0x0A0B0C
        ];
        let tsbk = make_tsbk_with_crc(&data);
        let parsed = parse(&tsbk).unwrap();

        assert_eq!(parsed.header.opcode, TsbkOpcode::EmergencyAlarm);
        match parsed.payload {
            TsbkPayload::EmergencyAlarm { target, source } => {
                assert_eq!(target.value(), 0x000100);
                assert_eq!(source.value(), 0x0A0B0C);
            }
            other => panic!("expected EmergencyAlarm, got {other:?}"),
        }
    }

    #[test]
    fn parse_deny_response() {
        let data: [u8; 10] = [
            0x96, // last_block=1, opcode=0x16
            0x00, // mfid
            0x20, // service type
            0x05, // reason
            0x00, 0x01, 0x00, // additional
            0x0A, 0x0B, 0x0C, // target
        ];
        let tsbk = make_tsbk_with_crc(&data);
        let parsed = parse(&tsbk).unwrap();

        assert_eq!(parsed.header.opcode, TsbkOpcode::DenyResponse);
        match parsed.payload {
            TsbkPayload::DenyResponse {
                service_type,
                reason,
                additional,
                target,
            } => {
                assert_eq!(service_type, 0x20);
                assert_eq!(reason, 0x05);
                assert_eq!(additional.value(), 0x000100);
                assert_eq!(target.value(), 0x0A0B0C);
            }
            other => panic!("expected DenyResponse, got {other:?}"),
        }
    }

    #[test]
    fn parse_unit_to_unit_answer_request() {
        let data: [u8; 10] = [
            0x85, // last_block=1, opcode=0x05
            0x00, // mfid
            0xA3, // service options
            0x00, // reserved
            0x11, 0x22, 0x33, // target
            0x44, 0x55, 0x66, // source
        ];
        let tsbk = make_tsbk_with_crc(&data);
        let parsed = parse(&tsbk).unwrap();

        assert_eq!(parsed.header.opcode, TsbkOpcode::UnitToUnitAnswerRequest);
        match parsed.payload {
            TsbkPayload::UnitToUnitAnswerRequest {
                service_options,
                target,
                source,
            } => {
                assert_eq!(service_options, 0xA3);
                assert_eq!(target.value(), 0x112233);
                assert_eq!(source.value(), 0x445566);
            }
            other => panic!("expected UnitToUnitAnswerRequest, got {other:?}"),
        }
    }

    #[test]
    fn parse_group_voice_grant_update_explicit() {
        let data: [u8; 10] = [
            0x83, // last_block=1, opcode=0x03
            0x00, // mfid=0 (standard)
            0xA3, // service options
            0x00, // reserved
            0x61, 0x23, // transmit channel
            0x71, 0x45, // receive channel
            0x00, 0x42, // talkgroup 66
        ];
        let tsbk = make_tsbk_with_crc(&data);
        let parsed = parse(&tsbk).unwrap();

        assert_eq!(
            parsed.header.opcode,
            TsbkOpcode::GroupVoiceChannelGrantUpdateExplicit
        );
        match parsed.payload {
            TsbkPayload::GroupVoiceChannelGrantUpdateExplicit {
                service_options,
                transmit_channel,
                receive_channel,
                talkgroup,
            } => {
                assert_eq!(service_options, 0xA3);
                assert_eq!(transmit_channel.value(), 0x6123);
                assert_eq!(receive_channel.value(), 0x7145);
                assert_eq!(talkgroup.value(), 66);
            }
            other => panic!("expected GroupVoiceChannelGrantUpdateExplicit, got {other:?}"),
        }
    }

    #[test]
    fn parse_group_affiliation_response() {
        let data: [u8; 10] = [
            0xA8, // last_block=1, opcode=0x28
            0x00, // mfid
            0x90, // lg=1, reserved, gav=1
            0x01, 0x00, // announcement group 0x0100
            0x02, 0x00, // group 0x0200
            0xAA, 0xBB, 0xCC, // target
        ];
        let tsbk = make_tsbk_with_crc(&data);
        let parsed = parse(&tsbk).unwrap();

        assert_eq!(
            parsed.header.opcode,
            TsbkOpcode::GroupAffiliationResponse
        );
        match parsed.payload {
            TsbkPayload::GroupAffiliationResponse {
                local_global,
                group_affiliation_value,
                announcement_group,
                group,
                target,
            } => {
                assert_eq!(local_global, 1);
                assert_eq!(group_affiliation_value, 1);
                assert_eq!(announcement_group.value(), 0x0100);
                assert_eq!(group.value(), 0x0200);
                assert_eq!(target.value(), 0xAABBCC);
            }
            other => panic!("expected GroupAffiliationResponse, got {other:?}"),
        }
    }

    #[test]
    fn parse_unit_registration_response() {
        // rv=2 (deny), syid=0xABC
        // Byte 2: rv(2 bits)=10, syid_hi(4 bits)=1010 -> 0b10_0000_1010 but:
        // rv goes into bits 5:4, syid is lower 12 bits across bytes 2-3
        // rv = (data[2] >> 4) & 0x03, syid = (data[2] & 0x0F)<<8 | data[3]
        // For rv=2, syid=0xABC: data[2] = (2<<4) | 0x0A = 0x2A, data[3] = 0xBC
        let data: [u8; 10] = [
            0xAC, // last_block=1, opcode=0x2C
            0x00, // mfid
            0x2A, 0xBC, // rv=2, syid=0xABC
            0x11, 0x22, 0x33, // source_id
            0x44, 0x55, 0x66, // source_address
        ];
        let tsbk = make_tsbk_with_crc(&data);
        let parsed = parse(&tsbk).unwrap();

        assert_eq!(
            parsed.header.opcode,
            TsbkOpcode::UnitRegistrationResponse
        );
        match parsed.payload {
            TsbkPayload::UnitRegistrationResponse {
                response,
                system_id,
                source_id,
                source_address,
            } => {
                assert_eq!(response, 2);
                assert_eq!(system_id.value(), 0xABC);
                assert_eq!(source_id.value(), 0x112233);
                assert_eq!(source_address.value(), 0x445566);
            }
            other => panic!("expected UnitRegistrationResponse, got {other:?}"),
        }
    }

    #[test]
    fn parse_unit_deregistration_ack() {
        // wacn=0xFC2BC, syid=0xF5B (same as network status test)
        let data: [u8; 10] = [
            0xAF, // last_block=1, opcode=0x2F
            0x00, // mfid
            0xFF, // reserved
            0xFC, 0x2B, // wacn hi
            0xCF, // wacn lo | sys hi
            0x5B, // sys lo
            0xAA, 0xBB, 0xCC, // source
        ];
        let tsbk = make_tsbk_with_crc(&data);
        let parsed = parse(&tsbk).unwrap();

        assert_eq!(parsed.header.opcode, TsbkOpcode::UnitDeregistrationAck);
        match parsed.payload {
            TsbkPayload::UnitDeregistrationAck {
                wacn,
                system_id,
                source,
            } => {
                assert_eq!(wacn.value(), 0xFC2BC);
                assert_eq!(system_id.value(), 0xF5B);
                assert_eq!(source.value(), 0xAABBCC);
            }
            other => panic!("expected UnitDeregistrationAck, got {other:?}"),
        }
    }

    #[test]
    fn parse_identifier_update_tdma() {
        // opcode 0x33 should use VU-style parser (4+4+14+10+32 bit fields)
        // Same freq table entry as OP25: iden=6, spacing=6250, base=851006250
        // Construct word: iden(4)=6, ch_type(4)=3, toff(14)=0x00B4 sign=0 (negative),
        // spac(10)=50 (50*125=6250), freq(32)=170201250 (170201250*5=851006250)
        //
        // Actually let's match OP25 reference more carefully.
        // toff_sign=0 means negative. toff magnitude=180 (180*6250=1125000).
        // toff raw 14 bits: sign=0, mag=180=0x0B4 -> raw = 0x00B4
        // spac raw = 50 (50*125=6250)
        // base raw = 170201250 (x5 = 851006250)
        //
        // Word (64 bits):
        // [6:4][3:4][0x00B4:14][50:10][170201250:32]
        // iden=6 -> 0110
        // ch_type=3 -> 0011
        // toff=0x00B4=180 -> 00 0000 1011 0100
        // spac=50 -> 00 0011 0010
        // freq=170201250=0x0A251082 (but let's compute: 851006250/5=170201250=0x0A251082)

        // Build from bytes:
        // bits 63-60: iden=6 = 0110
        // bits 59-56: ch_type=3 = 0011
        // bits 55-42: toff=180 = 00_0000_1011_0100
        // bits 41-32: spac=50 = 00_0011_0010
        // bits 31-0: freq=0x0A251082

        // Byte 0 (bits 63-56): 0110_0011 = 0x63
        // Byte 1 (bits 55-48): 0000_0010 = 0x02
        //   wait, toff is 14 bits starting at bit 55:
        //   bits 55..42: 00_0000_1011_01|00
        //   Byte 1 = bits[55:48] = 0000_0010 = nope, let me be more careful.
        //
        // 64-bit word:
        //   [63:60] = iden = 0110
        //   [59:56] = ch_type = 0011
        //   [55:42] = toff = 00_0000_1011_0100  (14 bits)
        //   [41:32] = spac = 00_0011_0010       (10 bits)
        //   [31:0]  = freq = 0x0A251082         (32 bits)
        //
        // Binary layout:
        //   0110 0011 | 0000 0010 | 1101 0000 | 0011 0010 | 0000 1010 | 0010 0101 | 0001 0000 | 1000 0010
        //   0x63       0x02        0xD0        0x32        0x0A        0x25        0x10        0x82
        //
        // Hmm let me recompute. Actually this is getting complex. Let me use the known
        // OP25 test vector for opcode 0x3D which we already have, but change opcode to 0x33.
        // The VU parser should produce the same frequency calculation.
        //
        // Simpler: construct a known test vector manually.
        // iden=6, ch_type=3 (TDMA 2-slot), negative offset magnitude=0, spac=50, freq=170201250
        //
        // With zero offset:
        // toff raw = 0 (14 bits) = 0b00_0000_0000_0000
        //
        // Word = 0110_0011_0000_0000_0000_0000_0011_0010_0000_1010_0010_0101_0001_0000_1000_0010
        // Hmm that's hard to verify. Let me just use a simple approach:

        // 170201250 = 0x0A2510A2
        // Word layout: iden(4)=6, ch_type(4)=3, toff(14)=0, spac(10)=50, freq(32)=0x0A2510A2
        // Byte 0 (bits 63-56): 0110_0011 = 0x63
        // Byte 1 (bits 55-48): 0000_0000 = 0x00
        // Byte 2 (bits 47-40): 0000_0000 = 0x00
        // Byte 3 (bits 39-32): 0011_0010 = 0x32
        // Bytes 4-7 (bits 31-0): 0x0A, 0x25, 0x10, 0xA2
        let data: [u8; 10] = [
            0xB3, // last_block=1, opcode=0x33
            0x00, // mfid=0
            0x63, 0x00, 0x00, 0x32, 0x0A, 0x25, 0x10, 0xA2,
        ];
        let tsbk = make_tsbk_with_crc(&data);
        let parsed = parse(&tsbk).unwrap();

        assert_eq!(parsed.header.opcode, TsbkOpcode::IdentifierUpdateTdma);
        match parsed.payload {
            TsbkPayload::IdentifierUpdate {
                identifier,
                channel_spacing,
                base_frequency,
                transmit_offset,
                ..
            } => {
                assert_eq!(identifier, 6);
                assert_eq!(channel_spacing, 6250);
                assert_eq!(base_frequency, 851_006_250);
                assert_eq!(transmit_offset, 0); // zero offset
            }
            other => panic!("expected IdentifierUpdate, got {other:?}"),
        }
    }

    #[test]
    fn parse_power_control_broadcast() {
        let data: [u8; 10] = [
            0xB0, // last_block=1, opcode=0x30
            0x00, // mfid
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88,
        ];
        let tsbk = make_tsbk_with_crc(&data);
        let parsed = parse(&tsbk).unwrap();

        assert_eq!(parsed.header.opcode, TsbkOpcode::PowerControlBroadcast);
        match parsed.payload {
            TsbkPayload::PowerControlBroadcast { data } => {
                assert_eq!(data, [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]);
            }
            other => panic!("expected PowerControlBroadcast, got {other:?}"),
        }
    }

    #[test]
    fn parse_sndcp_data_channel_grant() {
        let data2: [u8; 10] = [
            0x94, // last_block=1, opcode=0x14
            0x00, // mfid
            0x00, // opts
            0x00, // reserved
            0x61, 0x23, // data channel 0x6123
            0x00, // reserved
            0xAA, 0xBB, 0xCC, // target 0xAABBCC
        ];
        let tsbk = make_tsbk_with_crc(&data2);
        let parsed = parse(&tsbk).unwrap();

        assert_eq!(parsed.header.opcode, TsbkOpcode::SndcpDataChannelGrant);
        match parsed.payload {
            TsbkPayload::SndcpDataChannelGrant {
                data_channel,
                target,
            } => {
                assert_eq!(data_channel.value(), 0x6123);
                assert_eq!(target.value(), 0xAABBCC);
            }
            other => panic!("expected SndcpDataChannelGrant, got {other:?}"),
        }
    }
}

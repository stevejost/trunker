//! Link Control fields from LDU1 voice frames and TDULC.
//!
//! Link Control (LC) carries identity and control information within voice
//! transmissions. The 9-byte LC payload appears in LDU1 frames (reassembled
//! from six "extra" pieces after Reed-Solomon correction).
//!
//! Layout (9 bytes, 72 bits):
//! - Bit 0: Protected flag (encrypted LC)
//! - Bit 1: Reserved
//! - Bits 2-7: LC Opcode (6 bits)
//! - Bytes 1-8: Opcode-specific payload

use crate::p25::types::{SourceId, TalkgroupId};
use serde::Serialize;
use std::fmt;

/// Number of bytes in a Link Control payload.
pub const LINK_CONTROL_BYTES: usize = 9;

// ---------------------------------------------------------------------------
// Link Control opcode
// ---------------------------------------------------------------------------

/// Link Control opcode identifying the type of LC payload.
///
/// Values from TIA-102.AABF Table 3.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum LinkControlOpcode {
    /// Group Voice Channel User (0x00).
    GroupVoiceTraffic,
    /// Group Voice Channel Update (0x02).
    GroupVoiceUpdate,
    /// Unit to Unit Voice Channel User (0x03).
    UnitVoiceTraffic,
    /// Group Voice Channel Update - Explicit (0x04).
    GroupVoiceUpdateExplicit,
    /// Unit to Unit Answer Request (0x05).
    UnitCallRequest,
    /// Telephone Interconnect Voice Channel User (0x06).
    PhoneTraffic,
    /// Telephone Interconnect Answer Request (0x07).
    PhoneAlert,
    /// Call Termination / Cancellation (0x0F).
    CallTermination,
    /// Group Affiliation Query (0x10).
    GroupAffiliationQuery,
    /// Unit Registration Command (0x11).
    UnitRegistrationRequest,
    /// Unit Authentication Command (0x12).
    UnitAuthenticationRequest,
    /// Status Query (0x13).
    UnitStatusRequest,
    /// Status Update (0x14).
    UnitStatusUpdate,
    /// Message Update (0x15).
    UnitShortMessage,
    /// Call Alert (0x16).
    UnitCallAlert,
    /// Extended Function Command (0x17).
    ExtendedFunction,
    /// Channel Identifier Update (0x18).
    ChannelParamsUpdate,
    /// Channel Identifier Update - Explicit (0x19).
    ChannelParamsExplicit,
    /// System Service Broadcast (0x20).
    SystemServiceBroadcast,
    /// Secondary Control Channel Broadcast (0x21).
    AltControlChannel,
    /// Adjacent Site Status Broadcast (0x22).
    AdjacentSite,
    /// RFSS Status Broadcast (0x23).
    RfssStatusBroadcast,
    /// Network Status Broadcast (0x24).
    NetworkStatusBroadcast,
    /// Protection Parameter Broadcast (0x25).
    ProtectionParamBroadcast,
    /// Secondary Control Channel Broadcast - Explicit (0x26).
    AltControlChannelExplicit,
    /// Adjacent Site Status Broadcast - Explicit (0x27).
    AdjacentSiteExplicit,
    /// RFSS Status Broadcast - Explicit (0x28).
    RfssStatusExplicit,
    /// Network Status Broadcast - Explicit (0x29).
    NetworkStatusExplicit,
    /// Unknown or vendor-specific opcode.
    Unknown(u8),
}

impl LinkControlOpcode {
    /// Parse an opcode from a 6-bit value.
    pub fn from_bits(bits: u8) -> Self {
        match bits & 0x3F {
            0x00 => Self::GroupVoiceTraffic,
            0x02 => Self::GroupVoiceUpdate,
            0x03 => Self::UnitVoiceTraffic,
            0x04 => Self::GroupVoiceUpdateExplicit,
            0x05 => Self::UnitCallRequest,
            0x06 => Self::PhoneTraffic,
            0x07 => Self::PhoneAlert,
            0x0F => Self::CallTermination,
            0x10 => Self::GroupAffiliationQuery,
            0x11 => Self::UnitRegistrationRequest,
            0x12 => Self::UnitAuthenticationRequest,
            0x13 => Self::UnitStatusRequest,
            0x14 => Self::UnitStatusUpdate,
            0x15 => Self::UnitShortMessage,
            0x16 => Self::UnitCallAlert,
            0x17 => Self::ExtendedFunction,
            0x18 => Self::ChannelParamsUpdate,
            0x19 => Self::ChannelParamsExplicit,
            0x20 => Self::SystemServiceBroadcast,
            0x21 => Self::AltControlChannel,
            0x22 => Self::AdjacentSite,
            0x23 => Self::RfssStatusBroadcast,
            0x24 => Self::NetworkStatusBroadcast,
            0x25 => Self::ProtectionParamBroadcast,
            0x26 => Self::AltControlChannelExplicit,
            0x27 => Self::AdjacentSiteExplicit,
            0x28 => Self::RfssStatusExplicit,
            0x29 => Self::NetworkStatusExplicit,
            b => Self::Unknown(b),
        }
    }
}

impl fmt::Display for LinkControlOpcode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GroupVoiceTraffic => write!(f, "GRP_V_CH_USR"),
            Self::GroupVoiceUpdate => write!(f, "GRP_V_CH_UPDT"),
            Self::UnitVoiceTraffic => write!(f, "UNT_V_CH_USR"),
            Self::GroupVoiceUpdateExplicit => write!(f, "GRP_V_CH_UPDT_EXP"),
            Self::UnitCallRequest => write!(f, "UNT_ANS_REQ"),
            Self::PhoneTraffic => write!(f, "TEL_INT_CH_USR"),
            Self::PhoneAlert => write!(f, "TEL_INT_ANS_REQ"),
            Self::CallTermination => write!(f, "CALL_TERM"),
            Self::Unknown(b) => write!(f, "UNKNOWN(0x{b:02X})"),
            _ => write!(f, "{self:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Link Control fields (base decoder)
// ---------------------------------------------------------------------------

/// Base Link Control decoder, common to all LC packets.
///
/// Provides access to the protected flag, opcode, and raw payload bytes.
/// Opcode-specific decoders (like `GroupVoiceTraffic`) wrap this type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkControlFields([u8; LINK_CONTROL_BYTES]);

impl LinkControlFields {
    /// Create from a 9-byte buffer.
    pub fn new(buf: [u8; LINK_CONTROL_BYTES]) -> Self {
        Self(buf)
    }

    /// Whether the LC payload is encrypted (protected).
    pub fn protected(&self) -> bool {
        self.0[0] >> 7 == 1
    }

    /// The LC opcode (6 bits from byte 0).
    pub fn opcode(&self) -> LinkControlOpcode {
        LinkControlOpcode::from_bits(self.0[0] & 0x3F)
    }

    /// The 8-byte opcode-specific payload (bytes 1 through 8).
    pub fn payload(&self) -> &[u8] {
        &self.0[1..=8]
    }

    /// Access the raw 9-byte buffer.
    pub fn raw(&self) -> &[u8; LINK_CONTROL_BYTES] {
        &self.0
    }
}

impl fmt::Display for LinkControlFields {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "opcode={}, protected={}",
            self.opcode(),
            self.protected()
        )
    }
}

// ---------------------------------------------------------------------------
// Group Voice Traffic (opcode 0x00)
// ---------------------------------------------------------------------------

/// Group Voice Channel User — identifies the unit transmitting on a
/// talkgroup traffic channel.
///
/// Layout of payload (bytes 1-8 of LC):
/// - Byte 1: Manufacturer ID (MFID)
/// - Byte 2: Service options
/// - Bytes 3-4: Reserved
/// - Bytes 4-5: Talkgroup ID (16 bits)
/// - Bytes 6-8: Source unit address (24 bits)
#[derive(Clone, Copy, Debug)]
pub struct GroupVoiceTraffic(LinkControlFields);

impl GroupVoiceTraffic {
    /// Create from a base LC decoder.
    pub fn new(lc: LinkControlFields) -> Self {
        Self(lc)
    }

    /// Manufacturer ID.
    pub fn manufacturer_id(&self) -> u8 {
        self.0.0[1]
    }

    /// Service options byte.
    pub fn service_options(&self) -> ServiceOptions {
        ServiceOptions(self.0.0[2])
    }

    /// Talkgroup on the traffic channel.
    pub fn talkgroup(&self) -> TalkgroupId {
        TalkgroupId::new(u16::from_be_bytes([self.0.0[4], self.0.0[5]]))
    }

    /// Source unit (subscriber radio) address.
    pub fn source_unit(&self) -> SourceId {
        let bytes = &self.0.0;
        SourceId::new(((bytes[6] as u32) << 16) | ((bytes[7] as u32) << 8) | (bytes[8] as u32))
    }
}

// ---------------------------------------------------------------------------
// Service Options
// ---------------------------------------------------------------------------

/// Service options byte from Link Control payloads.
///
/// Bit layout:
/// - Bit 7: Emergency
/// - Bit 6: Protected (encrypted voice)
/// - Bit 5: Full duplex
/// - Bit 4: Packet switched
/// - Bits 0-2: Priority (0-7)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServiceOptions(u8);

impl ServiceOptions {
    /// Create from a raw byte.
    pub fn new(byte: u8) -> Self {
        Self(byte)
    }

    /// Whether this is an emergency transmission.
    pub fn emergency(&self) -> bool {
        self.0 & 0x80 != 0
    }

    /// Whether voice is encrypted.
    pub fn protected(&self) -> bool {
        self.0 & 0x40 != 0
    }

    /// Whether the channel is full duplex.
    pub fn full_duplex(&self) -> bool {
        self.0 & 0x20 != 0
    }

    /// Whether the channel is packet switched.
    pub fn packet_switched(&self) -> bool {
        self.0 & 0x10 != 0
    }

    /// Priority level (0-7).
    pub fn priority(&self) -> u8 {
        self.0 & 0x07
    }
}

impl fmt::Display for ServiceOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut flags = Vec::new();
        if self.emergency() {
            flags.push("EMERG");
        }
        if self.protected() {
            flags.push("ENCR");
        }
        if self.full_duplex() {
            flags.push("DUPLEX");
        }
        if self.packet_switched() {
            flags.push("PKT");
        }
        write!(f, "{} pri={}", flags.join("+"), self.priority())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_control_basic_fields() {
        let lc = LinkControlFields::new([
            0b00000000, 0b00000000, 0b10110101, 0b00000000, 0b00000000, 0b00000001, 0xDE, 0xAD,
            0xBE,
        ]);

        assert_eq!(lc.opcode(), LinkControlOpcode::GroupVoiceTraffic);
        assert!(!lc.protected());
        assert_eq!(
            lc.payload(),
            &[
                0b00000000, 0b10110101, 0b00000000, 0b00000000, 0b00000001, 0xDE, 0xAD, 0xBE
            ]
        );
    }

    #[test]
    fn link_control_protected_flag() {
        let lc = LinkControlFields::new([0b10100010, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert!(lc.protected());
        assert_eq!(lc.opcode(), LinkControlOpcode::AdjacentSite);
    }

    #[test]
    fn link_control_unknown_opcode() {
        let lc = LinkControlFields::new([0b00001010, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(lc.opcode(), LinkControlOpcode::Unknown(0x0A));
    }

    #[test]
    fn group_voice_traffic_parsing() {
        let lc = LinkControlFields::new([
            0b00000000, 0b00000000, 0b10110101, 0b00000000, 0b00000000, 0b00000001, 0xDE, 0xAD,
            0xBE,
        ]);
        assert_eq!(lc.opcode(), LinkControlOpcode::GroupVoiceTraffic);
        let gvt = GroupVoiceTraffic::new(lc);

        assert_eq!(gvt.manufacturer_id(), 0);
        assert_eq!(gvt.talkgroup().value(), 1);
        assert_eq!(gvt.source_unit().value(), 0xDEADBE);

        let opts = gvt.service_options();
        assert!(opts.emergency());
        assert!(!opts.protected());
        assert!(opts.full_duplex());
        assert!(opts.packet_switched());
        assert_eq!(opts.priority(), 5);
    }

    #[test]
    fn service_options_all_set() {
        let opts = ServiceOptions::new(0xFF);
        assert!(opts.emergency());
        assert!(opts.protected());
        assert!(opts.full_duplex());
        assert!(opts.packet_switched());
        assert_eq!(opts.priority(), 7);
    }

    #[test]
    fn service_options_none_set() {
        let opts = ServiceOptions::new(0x00);
        assert!(!opts.emergency());
        assert!(!opts.protected());
        assert!(!opts.full_duplex());
        assert!(!opts.packet_switched());
        assert_eq!(opts.priority(), 0);
    }

    #[test]
    fn opcode_all_known_values() {
        let cases = [
            (0x00, LinkControlOpcode::GroupVoiceTraffic),
            (0x02, LinkControlOpcode::GroupVoiceUpdate),
            (0x03, LinkControlOpcode::UnitVoiceTraffic),
            (0x04, LinkControlOpcode::GroupVoiceUpdateExplicit),
            (0x05, LinkControlOpcode::UnitCallRequest),
            (0x06, LinkControlOpcode::PhoneTraffic),
            (0x07, LinkControlOpcode::PhoneAlert),
            (0x0F, LinkControlOpcode::CallTermination),
            (0x10, LinkControlOpcode::GroupAffiliationQuery),
            (0x11, LinkControlOpcode::UnitRegistrationRequest),
            (0x12, LinkControlOpcode::UnitAuthenticationRequest),
            (0x13, LinkControlOpcode::UnitStatusRequest),
            (0x14, LinkControlOpcode::UnitStatusUpdate),
            (0x15, LinkControlOpcode::UnitShortMessage),
            (0x16, LinkControlOpcode::UnitCallAlert),
            (0x17, LinkControlOpcode::ExtendedFunction),
            (0x18, LinkControlOpcode::ChannelParamsUpdate),
            (0x19, LinkControlOpcode::ChannelParamsExplicit),
            (0x20, LinkControlOpcode::SystemServiceBroadcast),
            (0x21, LinkControlOpcode::AltControlChannel),
            (0x22, LinkControlOpcode::AdjacentSite),
            (0x23, LinkControlOpcode::RfssStatusBroadcast),
            (0x24, LinkControlOpcode::NetworkStatusBroadcast),
            (0x25, LinkControlOpcode::ProtectionParamBroadcast),
            (0x26, LinkControlOpcode::AltControlChannelExplicit),
            (0x27, LinkControlOpcode::AdjacentSiteExplicit),
            (0x28, LinkControlOpcode::RfssStatusExplicit),
            (0x29, LinkControlOpcode::NetworkStatusExplicit),
        ];

        for (bits, expected) in cases {
            assert_eq!(
                LinkControlOpcode::from_bits(bits),
                expected,
                "opcode 0x{bits:02X}"
            );
        }
    }

    #[test]
    fn opcode_unknown_values() {
        assert_eq!(
            LinkControlOpcode::from_bits(0x01),
            LinkControlOpcode::Unknown(0x01)
        );
        assert_eq!(
            LinkControlOpcode::from_bits(0x3F),
            LinkControlOpcode::Unknown(0x3F)
        );
    }

    #[test]
    fn link_control_display() {
        let lc = LinkControlFields::new([0; 9]);
        assert_eq!(format!("{lc}"), "opcode=GRP_V_CH_USR, protected=false");
    }

    #[test]
    fn service_options_display() {
        let opts = ServiceOptions::new(0b10110101);
        assert_eq!(format!("{opts}"), "EMERG+DUPLEX+PKT pri=5");
    }
}

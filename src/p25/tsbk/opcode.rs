//! TSBK opcode definitions.
//!
//! Every P25 Trunking Signaling Block contains a 6-bit opcode identifying the
//! message type. This module maps raw opcode values to typed enum variants.
//!
//! Reference: TIA-102.AABF-A Table 7.1

use serde::Serialize;

/// TSBK opcode identifying the message type.
///
/// All known opcodes from TIA-102.AABF-A are represented as named variants.
/// Unknown opcodes are captured as `Unknown(u8)` rather than causing errors,
/// since RF noise may produce any opcode value.
///
/// Reference: TIA-102.AABF-A Table 7.1
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TsbkOpcode {
    /// Group Voice Channel Grant (0x00).
    GroupVoiceChannelGrant,
    /// Group Voice Channel Grant Update (0x02).
    GroupVoiceChannelGrantUpdate,
    /// Group Voice Channel Grant Update - Explicit (0x03).
    GroupVoiceChannelGrantUpdateExplicit,
    /// Unit to Unit Voice Channel Grant (0x04).
    UnitToUnitVoiceChannelGrant,
    /// Unit to Unit Answer Request (0x05).
    UnitToUnitAnswerRequest,
    /// Unit to Unit Voice Channel Grant Update (0x06).
    UnitToUnitVoiceChannelGrantUpdate,
    /// Telephone Interconnect Voice Channel Grant (0x08).
    TelephoneInterconnectVoiceChannelGrant,
    /// Emergency Alarm (0x09).
    EmergencyAlarm,
    /// Acknowledge Response (0x10).
    AcknowledgeResponse,
    /// SNDCP Data Channel Grant (0x14).
    SndcpDataChannelGrant,
    /// SNDCP Reconnect Request (0x15).
    SndcpReconnectRequest,
    /// Deny Response (0x16).
    DenyResponse,
    /// Group Affiliation Query (0x18).
    GroupAffiliationQuery,
    /// Location Registration Response (0x1A).
    LocationRegistrationResponse,
    /// Channel Identifier Update (0x20).
    ChannelIdentifierUpdate,
    /// Extended Function Command (0x24).
    ExtendedFunctionCommand,
    /// Group Affiliation Response (0x28).
    GroupAffiliationResponse,
    /// Group Affiliation Query Response (0x2A).
    GroupAffiliationQueryResponse,
    /// Unit Registration Response (0x2C).
    UnitRegistrationResponse,
    /// Unit Deregistration Acknowledge (0x2F).
    UnitDeregistrationAcknowledge,
    /// Power Control Broadcast (0x30).
    PowerControlBroadcast,
    /// Channel Identifier Update - TDMA (0x33).
    ChannelIdentifierUpdateTdma,
    /// Channel Identifier Update - VHF/UHF (0x34).
    ChannelIdentifierUpdateVu,
    /// Network Status Broadcast (0x39).
    NetworkStatusBroadcast,
    /// RFSS Status Broadcast (0x3A).
    RfssStatusBroadcast,
    /// Secondary Control Channel Broadcast (0x3B).
    SecondaryControlChannelBroadcast,
    /// Adjacent Status Broadcast (0x3C).
    AdjacentStatusBroadcast,
    /// Channel Identifier Update - Standard (0x3D).
    ChannelIdentifierUpdateStandard,
    /// Unknown or unrecognized opcode.
    Unknown(u8),
}

impl TsbkOpcode {
    /// Create a `TsbkOpcode` from a raw 6-bit opcode value.
    pub fn from_u8(value: u8) -> Self {
        match value {
            0x00 => Self::GroupVoiceChannelGrant,
            0x02 => Self::GroupVoiceChannelGrantUpdate,
            0x03 => Self::GroupVoiceChannelGrantUpdateExplicit,
            0x04 => Self::UnitToUnitVoiceChannelGrant,
            0x05 => Self::UnitToUnitAnswerRequest,
            0x06 => Self::UnitToUnitVoiceChannelGrantUpdate,
            0x08 => Self::TelephoneInterconnectVoiceChannelGrant,
            0x09 => Self::EmergencyAlarm,
            0x10 => Self::AcknowledgeResponse,
            0x14 => Self::SndcpDataChannelGrant,
            0x15 => Self::SndcpReconnectRequest,
            0x16 => Self::DenyResponse,
            0x18 => Self::GroupAffiliationQuery,
            0x1A => Self::LocationRegistrationResponse,
            0x20 => Self::ChannelIdentifierUpdate,
            0x24 => Self::ExtendedFunctionCommand,
            0x28 => Self::GroupAffiliationResponse,
            0x2A => Self::GroupAffiliationQueryResponse,
            0x2C => Self::UnitRegistrationResponse,
            0x2F => Self::UnitDeregistrationAcknowledge,
            0x30 => Self::PowerControlBroadcast,
            0x33 => Self::ChannelIdentifierUpdateTdma,
            0x34 => Self::ChannelIdentifierUpdateVu,
            0x39 => Self::NetworkStatusBroadcast,
            0x3A => Self::RfssStatusBroadcast,
            0x3B => Self::SecondaryControlChannelBroadcast,
            0x3C => Self::AdjacentStatusBroadcast,
            0x3D => Self::ChannelIdentifierUpdateStandard,
            other => Self::Unknown(other),
        }
    }

    /// Return the raw 6-bit opcode value.
    pub fn to_u8(&self) -> u8 {
        match self {
            Self::GroupVoiceChannelGrant => 0x00,
            Self::GroupVoiceChannelGrantUpdate => 0x02,
            Self::GroupVoiceChannelGrantUpdateExplicit => 0x03,
            Self::UnitToUnitVoiceChannelGrant => 0x04,
            Self::UnitToUnitAnswerRequest => 0x05,
            Self::UnitToUnitVoiceChannelGrantUpdate => 0x06,
            Self::TelephoneInterconnectVoiceChannelGrant => 0x08,
            Self::EmergencyAlarm => 0x09,
            Self::AcknowledgeResponse => 0x10,
            Self::SndcpDataChannelGrant => 0x14,
            Self::SndcpReconnectRequest => 0x15,
            Self::DenyResponse => 0x16,
            Self::GroupAffiliationQuery => 0x18,
            Self::LocationRegistrationResponse => 0x1A,
            Self::ChannelIdentifierUpdate => 0x20,
            Self::ExtendedFunctionCommand => 0x24,
            Self::GroupAffiliationResponse => 0x28,
            Self::GroupAffiliationQueryResponse => 0x2A,
            Self::UnitRegistrationResponse => 0x2C,
            Self::UnitDeregistrationAcknowledge => 0x2F,
            Self::PowerControlBroadcast => 0x30,
            Self::ChannelIdentifierUpdateTdma => 0x33,
            Self::ChannelIdentifierUpdateVu => 0x34,
            Self::NetworkStatusBroadcast => 0x39,
            Self::RfssStatusBroadcast => 0x3A,
            Self::SecondaryControlChannelBroadcast => 0x3B,
            Self::AdjacentStatusBroadcast => 0x3C,
            Self::ChannelIdentifierUpdateStandard => 0x3D,
            Self::Unknown(v) => *v,
        }
    }

    /// Return the abbreviated name used in JSON output.
    ///
    /// These match the standard P25 abbreviations from TIA-102.AABF-A.
    pub fn abbreviation(&self) -> &'static str {
        match self {
            Self::GroupVoiceChannelGrant => "GRP_V_CH_GRANT",
            Self::GroupVoiceChannelGrantUpdate => "GRP_V_CH_GRANT_UPDT",
            Self::GroupVoiceChannelGrantUpdateExplicit => "GRP_V_CH_GRANT_UPDT_EXP",
            Self::UnitToUnitVoiceChannelGrant => "UNT_TO_UNT_CH_GRANT",
            Self::UnitToUnitAnswerRequest => "UNT_TO_UNT_ANS_REQ",
            Self::UnitToUnitVoiceChannelGrantUpdate => "UNT_TO_UNT_CH_GRANT_UPDT",
            Self::TelephoneInterconnectVoiceChannelGrant => "TEL_INT_CH_GRANT",
            Self::EmergencyAlarm => "EMERGENCY_ALRM",
            Self::AcknowledgeResponse => "ACK_RSP",
            Self::SndcpDataChannelGrant => "SNDCP_DATA_CH_GRANT",
            Self::SndcpReconnectRequest => "SNDCP_RCN_REQ",
            Self::DenyResponse => "DENY_RSP",
            Self::GroupAffiliationQuery => "GRP_AFF_QRY",
            Self::LocationRegistrationResponse => "LOC_REG_RSP",
            Self::ChannelIdentifierUpdate => "IDEN_UP",
            Self::ExtendedFunctionCommand => "EXT_FNCT_CMD",
            Self::GroupAffiliationResponse => "GRP_AFF_RSP",
            Self::GroupAffiliationQueryResponse => "GRP_AFF_QRY_RSP",
            Self::UnitRegistrationResponse => "U_REG_RSP",
            Self::UnitDeregistrationAcknowledge => "U_DE_REG_ACK",
            Self::PowerControlBroadcast => "PWR_CTRL_BCST",
            Self::ChannelIdentifierUpdateTdma => "IDEN_UP_TDMA",
            Self::ChannelIdentifierUpdateVu => "IDEN_UP_VU",
            Self::NetworkStatusBroadcast => "NET_STS_BCST",
            Self::RfssStatusBroadcast => "RFSS_STS_BCST",
            Self::SecondaryControlChannelBroadcast => "SEC_CC_BCST",
            Self::AdjacentStatusBroadcast => "ADJ_STS_BCST",
            Self::ChannelIdentifierUpdateStandard => "IDEN_UP",
            Self::Unknown(_) => "UNKNOWN",
        }
    }
}

impl Serialize for TsbkOpcode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.abbreviation())
    }
}

impl std::fmt::Display for TsbkOpcode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (0x{:02X})", self.abbreviation(), self.to_u8())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_u8_known_opcodes() {
        assert_eq!(
            TsbkOpcode::from_u8(0x00),
            TsbkOpcode::GroupVoiceChannelGrant
        );
        assert_eq!(
            TsbkOpcode::from_u8(0x02),
            TsbkOpcode::GroupVoiceChannelGrantUpdate
        );
        assert_eq!(
            TsbkOpcode::from_u8(0x34),
            TsbkOpcode::ChannelIdentifierUpdateVu
        );
        assert_eq!(
            TsbkOpcode::from_u8(0x3C),
            TsbkOpcode::AdjacentStatusBroadcast
        );
    }

    #[test]
    fn from_u8_unknown_opcode() {
        assert_eq!(TsbkOpcode::from_u8(0x07), TsbkOpcode::Unknown(0x07));
        assert_eq!(TsbkOpcode::from_u8(0x3F), TsbkOpcode::Unknown(0x3F));
    }

    #[test]
    fn roundtrip_u8() {
        for value in 0..=0x3F {
            let opcode = TsbkOpcode::from_u8(value);
            assert_eq!(opcode.to_u8(), value);
        }
    }

    #[test]
    fn serialize_produces_abbreviation() {
        let json = serde_json::to_string(&TsbkOpcode::GroupVoiceChannelGrant).unwrap();
        assert_eq!(json, r#""GRP_V_CH_GRANT""#);
    }
}

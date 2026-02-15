//! TSBK message field definitions.
//!
//! Each TSBK opcode carries different fields in its 64-bit payload (bits 79-16
//! of the 96-bit TSBK). This enum has one variant per supported opcode, with
//! named fields matching the TIA-102 specification.
//!
//! Reference: TIA-102.AABF-A Section 7.2

use super::opcode::TsbkOpcode;
use crate::p25::types::*;

/// Parsed fields from a TSBK message, varying by opcode.
///
/// Each variant corresponds to a specific TSBK message type and contains
/// the decoded fields for that message. Unknown or unimplemented opcodes
/// capture the raw payload bytes for diagnostic purposes.
#[derive(Debug, Clone, PartialEq)]
pub enum TsbkFields {
    /// Group Voice Channel Grant.
    ///
    /// Reference: TIA-102.AABF-A Section 7.2.1
    GroupVoiceChannelGrant {
        /// Service options for this grant.
        service_options: ServiceOptions,
        /// Logical channel assigned for the voice call.
        channel: ChannelId,
        /// Talkgroup receiving the grant.
        talkgroup: TalkgroupId,
        /// Source unit initiating the call.
        source: UnitId,
    },

    /// Motorola Group Regroup Command (manufacturer ID 0x90 variant of opcode 0x00).
    ///
    /// Motorola vendor extension that reassigns talkgroups into a supergroup.
    MotorolaGroupRegroupCommand {
        /// Supergroup talkgroup ID.
        supergroup: TalkgroupId,
        /// First member talkgroup.
        group_a: TalkgroupId,
        /// Second member talkgroup.
        group_b: TalkgroupId,
        /// Third member talkgroup.
        group_c: TalkgroupId,
    },

    /// Group Voice Channel Grant Update.
    ///
    /// Reference: TIA-102.AABF-A Section 7.2.2
    GroupVoiceChannelGrantUpdate {
        /// First channel assignment.
        channel_a: ChannelId,
        /// First talkgroup.
        talkgroup_a: TalkgroupId,
        /// Second channel assignment.
        channel_b: ChannelId,
        /// Second talkgroup.
        talkgroup_b: TalkgroupId,
    },

    /// Group Voice Channel Grant Update - Explicit.
    ///
    /// Reference: TIA-102.AABF-A Section 7.2.3
    GroupVoiceChannelGrantUpdateExplicit {
        /// First channel assignment.
        channel_a: ChannelId,
        /// First talkgroup.
        talkgroup_a: TalkgroupId,
        /// Second channel assignment.
        channel_b: ChannelId,
        /// Second talkgroup.
        talkgroup_b: TalkgroupId,
    },

    /// Unit to Unit Voice Channel Grant.
    ///
    /// Reference: TIA-102.AABF-A Section 7.2.4
    UnitToUnitVoiceChannelGrant {
        /// Logical channel assigned for the private call.
        channel: ChannelId,
        /// Target unit being called.
        target: UnitId,
        /// Source unit initiating the call.
        source: UnitId,
    },

    /// Emergency Alarm.
    ///
    /// Reference: TIA-102.AABF-A Section 7.2.9
    EmergencyAlarm {
        /// Unit declaring the emergency.
        source: UnitId,
        /// Talkgroup associated with the emergency.
        talkgroup: TalkgroupId,
    },

    /// Acknowledge Response.
    ///
    /// Reference: TIA-102.AABF-A Section 7.2.10
    AcknowledgeResponse {
        /// Unit being acknowledged.
        target: UnitId,
        /// Unit sending the acknowledgment.
        source: UnitId,
    },

    /// Deny Response.
    ///
    /// Reference: TIA-102.AABF-A Section 7.2.16
    DenyResponse {
        /// Talkgroup associated with the denied request.
        talkgroup: TalkgroupId,
        /// Unit whose request was denied.
        source: UnitId,
    },

    /// Group Affiliation Response.
    ///
    /// Reference: TIA-102.AABF-A Section 7.2.28
    GroupAffiliationResponse {
        /// Talkgroup the unit is affiliating with.
        talkgroup: TalkgroupId,
        /// Unit affiliating.
        source: UnitId,
    },

    /// Unit Registration Response.
    ///
    /// Reference: TIA-102.AABF-A Section 7.2.2C
    UnitRegistrationResponse {
        /// Unit that registered.
        source: UnitId,
        /// System ID acknowledging the registration.
        system_id: SystemId,
    },

    /// Channel Identifier Update - VHF/UHF (0x34).
    ///
    /// Maps a 4-bit identifier prefix to frequency parameters for VHF/UHF bands.
    ///
    /// Reference: TIA-102.AABF-A Section 7.2.34
    ChannelIdentifierUpdateVu {
        /// 4-bit identifier prefix (0-15).
        identifier: ChannelIdentifier,
        /// Bandwidth indicator for VHF/UHF.
        bandwidth_vu: u8,
        /// Transmit offset in Hertz (signed).
        transmit_offset: i32,
        /// Channel spacing in Hertz.
        channel_spacing: u32,
        /// Base frequency in Hertz.
        base_frequency: u64,
    },

    /// Channel Identifier Update - TDMA (0x33).
    ///
    /// Maps a 4-bit identifier prefix to frequency parameters for TDMA channels.
    ///
    /// Reference: TIA-102.AABF-A Section 7.2.33
    ChannelIdentifierUpdateTdma {
        /// 4-bit identifier prefix (0-15).
        identifier: ChannelIdentifier,
        /// Bandwidth indicator.
        bandwidth_vu: u8,
        /// Transmit offset in Hertz (signed).
        transmit_offset: i32,
        /// Channel spacing in Hertz.
        channel_spacing: u32,
        /// Base frequency in Hertz.
        base_frequency: u64,
    },

    /// Channel Identifier Update - Standard (0x3D).
    ///
    /// Maps a 4-bit identifier prefix to frequency parameters (original format).
    ///
    /// Reference: TIA-102.AABF-A Section 7.2.3D
    ChannelIdentifierUpdateStandard {
        /// 4-bit identifier prefix (0-15).
        identifier: ChannelIdentifier,
        /// Channel bandwidth in Hz (raw 9-bit value * 125).
        bandwidth: u16,
        /// Transmit offset in Hertz (signed).
        transmit_offset: i32,
        /// Channel spacing in Hertz.
        channel_spacing: u32,
        /// Base frequency in Hertz.
        base_frequency: u64,
    },

    /// Network Status Broadcast (0x39).
    ///
    /// Reference: TIA-102.AABF-A Section 7.2.39
    NetworkStatusBroadcast {
        /// Wide Area Communications Network ID.
        wacn: Wacn,
        /// System ID.
        system_id: SystemId,
        /// Control channel logical channel ID.
        channel: ChannelId,
    },

    /// RFSS Status Broadcast (0x3A).
    ///
    /// Reference: TIA-102.AABF-A Section 7.2.3A
    RfssStatusBroadcast {
        /// System ID.
        system_id: SystemId,
        /// RF Subsystem ID.
        rfss: RfssId,
        /// Site ID within the RFSS.
        site: SiteId,
        /// Control channel logical channel ID.
        channel: ChannelId,
    },

    /// Secondary Control Channel Broadcast (0x3B).
    ///
    /// Reference: TIA-102.AABF-A Section 7.2.3B
    SecondaryControlChannelBroadcast {
        /// Wide Area Communications Network ID.
        wacn: Wacn,
        /// System ID.
        system_id: SystemId,
        /// Secondary control channel logical channel ID.
        channel: ChannelId,
    },

    /// Adjacent Status Broadcast (0x3C).
    ///
    /// Reference: TIA-102.AABF-A Section 7.2.3C
    AdjacentStatusBroadcast {
        /// Adjacent system ID.
        system_id: SystemId,
        /// Adjacent RFSS ID.
        rfss: RfssId,
        /// Adjacent site ID.
        site: SiteId,
        /// Adjacent site control channel.
        channel: ChannelId,
    },

    /// Opcode recognized but field extraction not yet implemented.
    Unimplemented {
        /// The recognized opcode.
        opcode: TsbkOpcode,
        /// Raw payload bytes (8 bytes: TSBK bytes 2-9).
        raw_payload: [u8; 8],
    },

    /// Completely unknown opcode.
    Unknown {
        /// The raw opcode value.
        opcode_value: u8,
        /// Raw payload bytes (8 bytes: TSBK bytes 2-9).
        raw_payload: [u8; 8],
    },
}

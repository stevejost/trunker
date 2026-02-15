//! JSON serialization for control channel events.
//!
//! This module converts parsed TSBK messages into compact JSON lines for
//! stdout output. Per project convention, JSON field names use abbreviated
//! forms (`tg`, `freq`, `src`) while internal Rust code uses full names.
//!
//! The conversion from `TsbkMessage` to `ControlChannelEvent` is the boundary
//! between the protocol decoder and the output layer.

use std::io::Write;

use serde::Serialize;

use crate::p25::channel::IdentifierTable;
use crate::p25::tsbk::fields::TsbkFields;
use crate::p25::tsbk::parser::TsbkMessage;
use crate::p25::types::*;

/// A JSON-serializable control channel event.
///
/// This is the output representation, separate from internal types.
/// Field names are abbreviated for compact JSON output per README.md spec.
/// Optional fields are omitted from output when `None`.
#[derive(Debug, Serialize, Default)]
pub struct ControlChannelEvent {
    /// Event type (e.g., "chan_grant", "chan_grant_updt", "affiliation",
    /// "ident_update", "sys_info", "adj_site").
    #[serde(rename = "type")]
    pub event_type: String,

    /// TSBK opcode abbreviation.
    pub opcode: String,

    /// Network Access Code (hex string, e.g., "0x5F2").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nac: Option<String>,

    /// Talkgroup ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tg: Option<u16>,

    /// Second talkgroup (for grant updates with two assignments).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tg2: Option<u16>,

    /// Frequency in Hertz.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub freq: Option<u64>,

    /// Second frequency (for grant updates with two assignments).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub freq2: Option<u64>,

    /// Source unit ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub src: Option<u32>,

    /// Target unit ID (for unit-to-unit calls).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<u32>,

    /// Logical channel ID (hex string).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,

    /// System ID (hex string).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sysid: Option<String>,

    /// WACN (hex string).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wacn: Option<String>,

    /// RFSS ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rfss: Option<u8>,

    /// Site ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site: Option<u8>,

    /// Channel identifier prefix (for ident_update events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ident: Option<u8>,

    /// Base frequency in Hertz (for ident_update events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_freq: Option<u64>,

    /// Channel spacing in Hertz (for ident_update events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spacing: Option<u32>,

    /// Transmit offset in Hertz (for ident_update events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<i32>,
}

/// Convert a parsed TSBK message into a JSON-serializable event.
///
/// Channel IDs are resolved to frequencies using the provided identifier table.
/// If the identifier table doesn't have an entry for a channel's prefix,
/// the `freq` field will be `None`.
pub fn to_event(
    message: &TsbkMessage,
    nac: Option<Nac>,
    ident_table: &IdentifierTable,
) -> ControlChannelEvent {
    let nac_str = nac.map(|n| format!("{n}"));
    let opcode_str = message.opcode.abbreviation().to_string();

    match &message.fields {
        TsbkFields::GroupVoiceChannelGrant {
            channel,
            talkgroup,
            source,
            ..
        } => ControlChannelEvent {
            event_type: "chan_grant".into(),
            opcode: opcode_str,
            nac: nac_str,
            tg: Some(talkgroup.0),
            freq: ident_table.resolve_frequency(*channel).map(|f| f.0),
            src: Some(source.0),
            channel: Some(format!("{channel}")),
            ..Default::default()
        },

        TsbkFields::MotorolaGroupRegroupCommand {
            supergroup,
            group_a,
            group_b,
            group_c,
        } => ControlChannelEvent {
            event_type: "mot_grg_cmd".into(),
            opcode: "MOT_GRG_CMD".into(),
            nac: nac_str,
            tg: Some(supergroup.0),
            // Encode member groups in a simple way
            src: Some(group_a.0 as u32),
            target: Some(group_b.0 as u32),
            tg2: Some(group_c.0),
            ..Default::default()
        },

        TsbkFields::GroupVoiceChannelGrantUpdate {
            channel_a,
            talkgroup_a,
            channel_b,
            talkgroup_b,
        } => ControlChannelEvent {
            event_type: "chan_grant_updt".into(),
            opcode: opcode_str,
            nac: nac_str,
            tg: Some(talkgroup_a.0),
            freq: ident_table.resolve_frequency(*channel_a).map(|f| f.0),
            tg2: Some(talkgroup_b.0),
            freq2: ident_table.resolve_frequency(*channel_b).map(|f| f.0),
            ..Default::default()
        },

        TsbkFields::GroupVoiceChannelGrantUpdateExplicit {
            channel_a,
            talkgroup_a,
            channel_b,
            talkgroup_b,
        } => ControlChannelEvent {
            event_type: "chan_grant_updt".into(),
            opcode: opcode_str,
            nac: nac_str,
            tg: Some(talkgroup_a.0),
            freq: ident_table.resolve_frequency(*channel_a).map(|f| f.0),
            tg2: Some(talkgroup_b.0),
            freq2: ident_table.resolve_frequency(*channel_b).map(|f| f.0),
            ..Default::default()
        },

        TsbkFields::UnitToUnitVoiceChannelGrant {
            channel,
            target,
            source,
        } => ControlChannelEvent {
            event_type: "unit_call".into(),
            opcode: opcode_str,
            nac: nac_str,
            freq: ident_table.resolve_frequency(*channel).map(|f| f.0),
            src: Some(source.0),
            target: Some(target.0),
            channel: Some(format!("{channel}")),
            ..Default::default()
        },

        TsbkFields::EmergencyAlarm { source, talkgroup } => ControlChannelEvent {
            event_type: "emergency".into(),
            opcode: opcode_str,
            nac: nac_str,
            tg: Some(talkgroup.0),
            src: Some(source.0),
            ..Default::default()
        },

        TsbkFields::AcknowledgeResponse { target, source } => ControlChannelEvent {
            event_type: "ack".into(),
            opcode: opcode_str,
            nac: nac_str,
            src: Some(source.0),
            target: Some(target.0),
            ..Default::default()
        },

        TsbkFields::DenyResponse { talkgroup, source } => ControlChannelEvent {
            event_type: "deny".into(),
            opcode: opcode_str,
            nac: nac_str,
            tg: Some(talkgroup.0),
            src: Some(source.0),
            ..Default::default()
        },

        TsbkFields::GroupAffiliationResponse { talkgroup, source } => ControlChannelEvent {
            event_type: "affiliation".into(),
            opcode: opcode_str,
            nac: nac_str,
            tg: Some(talkgroup.0),
            src: Some(source.0),
            ..Default::default()
        },

        TsbkFields::UnitRegistrationResponse {
            source, system_id, ..
        } => ControlChannelEvent {
            event_type: "registration".into(),
            opcode: opcode_str,
            nac: nac_str,
            src: Some(source.0),
            sysid: Some(format!("{system_id}")),
            ..Default::default()
        },

        TsbkFields::ChannelIdentifierUpdateVu {
            identifier,
            transmit_offset,
            channel_spacing,
            base_frequency,
            ..
        }
        | TsbkFields::ChannelIdentifierUpdateTdma {
            identifier,
            transmit_offset,
            channel_spacing,
            base_frequency,
            ..
        } => ControlChannelEvent {
            event_type: "ident_update".into(),
            opcode: opcode_str,
            nac: nac_str,
            ident: Some(identifier.0),
            base_freq: Some(*base_frequency),
            spacing: Some(*channel_spacing),
            offset: Some(*transmit_offset),
            ..Default::default()
        },

        TsbkFields::ChannelIdentifierUpdateStandard {
            identifier,
            transmit_offset,
            channel_spacing,
            base_frequency,
            ..
        } => ControlChannelEvent {
            event_type: "ident_update".into(),
            opcode: opcode_str,
            nac: nac_str,
            ident: Some(identifier.0),
            base_freq: Some(*base_frequency),
            spacing: Some(*channel_spacing),
            offset: Some(*transmit_offset),
            ..Default::default()
        },

        TsbkFields::NetworkStatusBroadcast {
            wacn,
            system_id,
            channel,
        } => ControlChannelEvent {
            event_type: "sys_info".into(),
            opcode: opcode_str,
            nac: nac_str,
            wacn: Some(format!("{wacn}")),
            sysid: Some(format!("{system_id}")),
            freq: ident_table.resolve_frequency(*channel).map(|f| f.0),
            channel: Some(format!("{channel}")),
            ..Default::default()
        },

        TsbkFields::SecondaryControlChannelBroadcast {
            wacn,
            system_id,
            channel,
        } => ControlChannelEvent {
            event_type: "sec_cc".into(),
            opcode: opcode_str,
            nac: nac_str,
            wacn: Some(format!("{wacn}")),
            sysid: Some(format!("{system_id}")),
            freq: ident_table.resolve_frequency(*channel).map(|f| f.0),
            channel: Some(format!("{channel}")),
            ..Default::default()
        },

        TsbkFields::RfssStatusBroadcast {
            system_id,
            rfss,
            site,
            channel,
        } => ControlChannelEvent {
            event_type: "sys_info".into(),
            opcode: opcode_str,
            nac: nac_str,
            sysid: Some(format!("{system_id}")),
            rfss: Some(rfss.0),
            site: Some(site.0),
            freq: ident_table.resolve_frequency(*channel).map(|f| f.0),
            channel: Some(format!("{channel}")),
            ..Default::default()
        },

        TsbkFields::AdjacentStatusBroadcast {
            system_id,
            rfss,
            site,
            channel,
        } => ControlChannelEvent {
            event_type: "adj_site".into(),
            opcode: opcode_str,
            nac: nac_str,
            sysid: Some(format!("{system_id}")),
            rfss: Some(rfss.0),
            site: Some(site.0),
            freq: ident_table.resolve_frequency(*channel).map(|f| f.0),
            channel: Some(format!("{channel}")),
            ..Default::default()
        },

        TsbkFields::Unimplemented { opcode, .. } => ControlChannelEvent {
            event_type: "unknown".into(),
            opcode: format!("{opcode}"),
            nac: nac_str,
            ..Default::default()
        },

        TsbkFields::Unknown { opcode_value, .. } => ControlChannelEvent {
            event_type: "unknown".into(),
            opcode: format!("0x{opcode_value:02X}"),
            nac: nac_str,
            ..Default::default()
        },
    }
}

/// Errors that can occur during output serialization.
#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    /// JSON serialization failed.
    #[error("JSON serialization failed: {0}")]
    SerializationFailed(#[from] serde_json::Error),

    /// I/O error writing output.
    #[error("I/O error writing output: {0}")]
    IoError(#[from] std::io::Error),
}

/// Write a control channel event as a JSON line to the given writer.
///
/// Each event is written as a single line of JSON followed by a newline,
/// suitable for piping to `jq` or other line-oriented JSON consumers.
pub fn write_json_line<W: Write>(
    writer: &mut W,
    event: &ControlChannelEvent,
) -> Result<(), OutputError> {
    serde_json::to_writer(&mut *writer, event)?;
    writeln!(writer)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p25::channel::{ChannelParams, IdentifierTable};
    use crate::p25::tsbk::opcode::TsbkOpcode;

    fn sample_ident_table() -> IdentifierTable {
        let mut table = IdentifierTable::new();
        table.update(
            ChannelIdentifier(1),
            ChannelParams {
                base_frequency: 851_012_500,
                channel_spacing: 12_500,
                transmit_offset: -45_000_000,
            },
        );
        table
    }

    #[test]
    fn channel_grant_to_json() {
        let msg = TsbkMessage {
            last_block: true,
            protected: false,
            opcode: TsbkOpcode::GroupVoiceChannelGrant,
            manufacturer_id: ManufacturerId(0),
            fields: TsbkFields::GroupVoiceChannelGrant {
                service_options: ServiceOptions(0),
                channel: ChannelId(0x1019),
                talkgroup: TalkgroupId(3769),
                source: UnitId(12345),
            },
        };

        let table = sample_ident_table();
        let event = to_event(&msg, Some(Nac(0x5F2)), &table);

        assert_eq!(event.event_type, "chan_grant");
        assert_eq!(event.tg, Some(3769));
        assert_eq!(event.src, Some(12345));
        assert_eq!(event.freq, Some(851_325_000));
        assert_eq!(event.nac, Some("0x5F2".to_string()));
    }

    #[test]
    fn json_line_output_format() {
        let event = ControlChannelEvent {
            event_type: "chan_grant".into(),
            opcode: "GRP_V_CH_GRANT".into(),
            tg: Some(3769),
            freq: Some(851_325_000),
            src: Some(12345),
            ..Default::default()
        };

        let mut buf = Vec::new();
        write_json_line(&mut buf, &event).unwrap();
        let output = String::from_utf8(buf).unwrap();

        assert!(output.ends_with('\n'));
        let parsed: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
        assert_eq!(parsed["type"], "chan_grant");
        assert_eq!(parsed["tg"], 3769);
        assert_eq!(parsed["freq"], 851_325_000);
        // Optional None fields should be absent
        assert!(parsed.get("wacn").is_none());
    }
}

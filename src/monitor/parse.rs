//! JSON line parser for P25 monitor.
//!
//! Parses JSON lines from the decoder's stdout into typed messages.
//! Dispatches on the `"name"` field to decouple the monitor from
//! decoder internals.

use serde_json::Value;

use crate::monitor::error::MonitorError;

/// A parsed TSBK message from the decoder's JSON output.
#[derive(Debug, PartialEq)]
pub enum ParsedMessage {
    /// Group voice channel grant (opcode 0x00).
    GroupVoiceGrant {
        /// Raw channel number.
        channel: u16,
        /// Frequency in MHz if resolved, or None.
        frequency: Option<f64>,
        /// Talkgroup ID.
        talkgroup: u16,
        /// Source unit ID.
        source: u32,
    },
    /// Group voice channel grant update (opcode 0x02).
    GroupVoiceGrantUpdate {
        /// First channel number.
        channel_a: u16,
        /// First frequency in MHz if resolved, or None.
        frequency_a: Option<f64>,
        /// First talkgroup ID.
        talkgroup_a: u16,
        /// Second channel number.
        channel_b: u16,
        /// Second frequency in MHz if resolved, or None.
        frequency_b: Option<f64>,
        /// Second talkgroup ID.
        talkgroup_b: u16,
    },
    /// Group voice channel grant update - explicit (opcode 0x03).
    GroupVoiceGrantUpdateExplicit {
        /// Receive channel number.
        receive_channel: u16,
        /// Receive frequency in MHz if resolved, or None.
        receive_frequency: Option<f64>,
        /// Talkgroup ID.
        talkgroup: u16,
    },
    /// Identifier update (opcode 0x20 or 0x3D).
    IdentifierUpdate {
        /// 4-bit band identifier.
        identifier: u8,
        /// Base frequency in hertz.
        base_frequency: u64,
        /// Channel spacing in hertz.
        channel_spacing: u32,
        /// Transmit offset in hertz.
        transmit_offset: i64,
    },
    /// Identifier update TDMA (opcode 0x33).
    IdentifierUpdateTdma {
        /// 4-bit band identifier.
        identifier: u8,
        /// Base frequency in hertz.
        base_frequency: u64,
        /// Channel spacing in hertz.
        channel_spacing: u32,
        /// Transmit offset in hertz.
        transmit_offset: i64,
    },
    /// Network status broadcast (opcode 0x39 or 0x3B).
    NetworkStatusBroadcast {
        /// Wide Area Communications Network ID (hex string).
        wacn: String,
        /// System ID (hex string).
        system_id: String,
        /// Control channel number.
        channel: u16,
        /// Control channel frequency in MHz if resolved, or None.
        frequency: Option<f64>,
    },
    /// RFSS status broadcast (opcode 0x3A).
    RfssStatusBroadcast {
        /// System ID (hex string).
        system_id: String,
        /// RFSS identifier.
        rfss_id: u8,
        /// Site identifier.
        site_id: u8,
        /// Control channel number.
        channel: u16,
        /// Control channel frequency in MHz if resolved, or None.
        frequency: Option<f64>,
    },
    /// Adjacent site status broadcast (opcode 0x3C).
    AdjacentStatusBroadcast {
        /// System ID (hex string).
        system_id: String,
        /// RFSS identifier.
        rfss_id: u8,
        /// Site identifier.
        site_id: u8,
        /// Control channel number.
        channel: u16,
        /// Control channel frequency in MHz if resolved, or None.
        frequency: Option<f64>,
    },
    /// Emergency alarm (opcode 0x09).
    EmergencyAlarm {
        /// Target unit or talkgroup ID.
        target: u32,
        /// Source unit ID.
        source: u32,
    },
    /// Unit registration response (opcode 0x2C).
    UnitRegistrationResponse {
        /// Response code.
        response: u8,
        /// Source address.
        source_address: u32,
    },
    /// Unit deregistration acknowledgement (opcode 0x2F).
    UnitDeregistrationAck {
        /// Source unit ID.
        source: u32,
    },
    /// Deny response (opcode 0x16).
    DenyResponse {
        /// Denied service type.
        service_type: u8,
        /// Deny reason code.
        reason: u8,
        /// Target unit ID.
        target: u32,
    },
    /// Group affiliation response (opcode 0x28).
    GroupAffiliationResponse {
        /// Group address (talkgroup).
        group: u16,
        /// Target unit ID.
        target: u32,
    },
    /// Any message type not specifically handled.
    Other {
        /// The `name` field from the JSON.
        name: String,
    },
}

/// Parse a JSON line from the decoder into a [`ParsedMessage`].
///
/// Dispatches on the `"name"` field. Unknown or unhandled message types
/// produce [`ParsedMessage::Other`] rather than an error.
pub fn parse_json_line(line: &str) -> Result<ParsedMessage, MonitorError> {
    let value: Value = serde_json::from_str(line)?;
    let name = value["name"].as_str().unwrap_or("");
    Ok(dispatch_message(name, &value))
}

/// Dispatch a parsed JSON value to the appropriate message variant.
fn dispatch_message(name: &str, value: &Value) -> ParsedMessage {
    match name {
        "GRP_V_CH_GRANT" => parse_group_voice_grant(value),
        "GRP_V_CH_GRANT_UPDT" => parse_group_voice_grant_update(value),
        "GRP_V_CH_GRANT_UPDT_EXP" | "GRP_V_CH_GRANT_UPDT_EXPLICIT" => {
            parse_group_voice_grant_update_explicit(value)
        }
        "IDENT_UP" | "CH_PARAMS_UPDT" | "IDEN_UP_VU" => parse_identifier_update(value),
        "IDEN_UP_TDMA" => parse_identifier_update_tdma(value),
        "NET_STS_BCST" | "NET2_STS_BCST" => parse_network_status_broadcast(value),
        "RFSS_STS_BCST" => parse_rfss_status_broadcast(value),
        "ADJ_STS_BCST" => parse_adjacent_status_broadcast(value),
        "EMERGENCY_ALRM" | "EMERGENCY_ALARM" => parse_emergency_alarm(value),
        "U_REG_RSP" | "UNIT_REG_RESP" => parse_unit_registration_response(value),
        "U_DE_REG_ACK" | "UNIT_DEREG_ACK" => parse_unit_deregistration_ack(value),
        "DENY_RSP" | "DENY_RESPONSE" => parse_deny_response(value),
        "GRP_AFF_RSP" | "GRP_AFF_RESP" => parse_group_affiliation_response(value),
        other => ParsedMessage::Other {
            name: other.to_string(),
        },
    }
}

/// Extract an optional f64 field (handles JSON null).
fn opt_f64(value: &Value, field: &str) -> Option<f64> {
    value[field].as_f64()
}

/// Extract a u16 field, defaulting to 0 if missing.
fn get_u16(value: &Value, field: &str) -> u16 {
    value[field].as_u64().unwrap_or(0) as u16
}

/// Extract a u32 field, defaulting to 0 if missing.
fn get_u32(value: &Value, field: &str) -> u32 {
    value[field].as_u64().unwrap_or(0) as u32
}

/// Extract a u8 field, defaulting to 0 if missing.
fn get_u8(value: &Value, field: &str) -> u8 {
    value[field].as_u64().unwrap_or(0) as u8
}

/// Extract a string field, defaulting to empty string.
fn get_string(value: &Value, field: &str) -> String {
    value[field]
        .as_str()
        .unwrap_or("")
        .to_string()
}

fn parse_group_voice_grant(value: &Value) -> ParsedMessage {
    ParsedMessage::GroupVoiceGrant {
        channel: get_u16(value, "channel"),
        frequency: opt_f64(value, "frequency"),
        talkgroup: get_u16(value, "talkgroup"),
        source: get_u32(value, "source"),
    }
}

fn parse_group_voice_grant_update(value: &Value) -> ParsedMessage {
    ParsedMessage::GroupVoiceGrantUpdate {
        channel_a: get_u16(value, "channel_a"),
        frequency_a: opt_f64(value, "frequency_a"),
        talkgroup_a: get_u16(value, "talkgroup_a"),
        channel_b: get_u16(value, "channel_b"),
        frequency_b: opt_f64(value, "frequency_b"),
        talkgroup_b: get_u16(value, "talkgroup_b"),
    }
}

fn parse_group_voice_grant_update_explicit(value: &Value) -> ParsedMessage {
    ParsedMessage::GroupVoiceGrantUpdateExplicit {
        receive_channel: get_u16(value, "receive_channel"),
        receive_frequency: opt_f64(value, "receive_frequency"),
        talkgroup: get_u16(value, "talkgroup"),
    }
}

fn parse_identifier_update(value: &Value) -> ParsedMessage {
    ParsedMessage::IdentifierUpdate {
        identifier: get_u8(value, "identifier"),
        base_frequency: value["base_frequency"].as_u64().unwrap_or(0),
        channel_spacing: get_u32(value, "channel_spacing"),
        transmit_offset: value["transmit_offset"].as_i64().unwrap_or(0),
    }
}

fn parse_identifier_update_tdma(value: &Value) -> ParsedMessage {
    ParsedMessage::IdentifierUpdateTdma {
        identifier: get_u8(value, "identifier"),
        base_frequency: value["base_frequency"].as_u64().unwrap_or(0),
        channel_spacing: get_u32(value, "channel_spacing"),
        transmit_offset: value["transmit_offset"].as_i64().unwrap_or(0),
    }
}

fn parse_network_status_broadcast(value: &Value) -> ParsedMessage {
    ParsedMessage::NetworkStatusBroadcast {
        wacn: get_string(value, "wacn"),
        system_id: get_string(value, "system_id"),
        channel: get_u16(value, "channel"),
        frequency: opt_f64(value, "frequency"),
    }
}

fn parse_rfss_status_broadcast(value: &Value) -> ParsedMessage {
    ParsedMessage::RfssStatusBroadcast {
        system_id: get_string(value, "system_id"),
        rfss_id: get_u8(value, "rfss_id"),
        site_id: get_u8(value, "site_id"),
        channel: get_u16(value, "channel"),
        frequency: opt_f64(value, "frequency"),
    }
}

fn parse_adjacent_status_broadcast(value: &Value) -> ParsedMessage {
    ParsedMessage::AdjacentStatusBroadcast {
        system_id: get_string(value, "system_id"),
        rfss_id: get_u8(value, "rfss_id"),
        site_id: get_u8(value, "site_id"),
        channel: get_u16(value, "channel"),
        frequency: opt_f64(value, "frequency"),
    }
}

fn parse_emergency_alarm(value: &Value) -> ParsedMessage {
    ParsedMessage::EmergencyAlarm {
        target: get_u32(value, "target"),
        source: get_u32(value, "source"),
    }
}

fn parse_unit_registration_response(value: &Value) -> ParsedMessage {
    ParsedMessage::UnitRegistrationResponse {
        response: get_u8(value, "response"),
        source_address: get_u32(value, "source_address"),
    }
}

fn parse_unit_deregistration_ack(value: &Value) -> ParsedMessage {
    ParsedMessage::UnitDeregistrationAck {
        source: get_u32(value, "source"),
    }
}

fn parse_deny_response(value: &Value) -> ParsedMessage {
    ParsedMessage::DenyResponse {
        service_type: get_u8(value, "service_type"),
        reason: get_u8(value, "reason"),
        target: get_u32(value, "target"),
    }
}

fn parse_group_affiliation_response(value: &Value) -> ParsedMessage {
    ParsedMessage::GroupAffiliationResponse {
        group: get_u16(value, "group"),
        target: get_u32(value, "target"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_group_voice_grant() {
        let json = r#"{"nac":"0x5FC","opcode":"0x00","name":"GRP_V_CH_GRANT","last_block":false,"manufacturer_id":144,"channel":3851,"frequency":null,"talkgroup":3851,"source":985871}"#;
        let msg = parse_json_line(json).unwrap();
        assert_eq!(
            msg,
            ParsedMessage::GroupVoiceGrant {
                channel: 3851,
                frequency: None,
                talkgroup: 3851,
                source: 985871,
            }
        );
    }

    #[test]
    fn parse_group_voice_grant_with_frequency() {
        let json = r#"{"name":"GRP_V_CH_GRANT","channel":100,"frequency":851.0625,"talkgroup":200,"source":12345}"#;
        let msg = parse_json_line(json).unwrap();
        match msg {
            ParsedMessage::GroupVoiceGrant { frequency, .. } => {
                assert!((frequency.unwrap() - 851.0625).abs() < 0.0001);
            }
            _ => panic!("expected GroupVoiceGrant"),
        }
    }

    #[test]
    fn parse_group_voice_grant_update() {
        let json = r#"{"nac":"0x5FC","opcode":"0x02","name":"GRP_V_CH_GRANT_UPDT","last_block":true,"manufacturer_id":0,"channel_a":433,"frequency_a":null,"talkgroup_a":2821,"channel_b":135,"frequency_b":null,"talkgroup_b":3769}"#;
        let msg = parse_json_line(json).unwrap();
        assert_eq!(
            msg,
            ParsedMessage::GroupVoiceGrantUpdate {
                channel_a: 433,
                frequency_a: None,
                talkgroup_a: 2821,
                channel_b: 135,
                frequency_b: None,
                talkgroup_b: 3769,
            }
        );
    }

    #[test]
    fn parse_grant_update_explicit() {
        let json = r#"{"name":"GRP_V_CH_GRANT_UPDT_EXP","receive_channel":500,"receive_frequency":852.5,"talkgroup":100}"#;
        let msg = parse_json_line(json).unwrap();
        assert_eq!(
            msg,
            ParsedMessage::GroupVoiceGrantUpdateExplicit {
                receive_channel: 500,
                receive_frequency: Some(852.5),
                talkgroup: 100,
            }
        );
    }

    #[test]
    fn parse_identifier_update() {
        let json = r#"{"nac":"0x5FC","opcode":"0x20","name":"IDENT_UP","identifier":6,"bandwidth":12500,"transmit_offset":-45000000,"channel_spacing":6250,"base_frequency":851006250}"#;
        let msg = parse_json_line(json).unwrap();
        assert_eq!(
            msg,
            ParsedMessage::IdentifierUpdate {
                identifier: 6,
                base_frequency: 851006250,
                channel_spacing: 6250,
                transmit_offset: -45000000,
            }
        );
    }

    #[test]
    fn parse_ch_params_updt_as_identifier_update() {
        let json = r#"{"name":"CH_PARAMS_UPDT","identifier":3,"bandwidth":12500,"transmit_offset":0,"channel_spacing":12500,"base_frequency":866000000}"#;
        let msg = parse_json_line(json).unwrap();
        assert!(matches!(msg, ParsedMessage::IdentifierUpdate { identifier: 3, .. }));
    }

    #[test]
    fn parse_identifier_update_tdma() {
        let json = r#"{"nac":"0x5FC","opcode":"0x3D","name":"IDEN_UP_TDMA","last_block":false,"manufacturer_id":0,"identifier":4,"bandwidth":12500,"transmit_offset":-39000000,"channel_spacing":12500,"base_frequency":935012500}"#;
        let msg = parse_json_line(json).unwrap();
        assert_eq!(
            msg,
            ParsedMessage::IdentifierUpdateTdma {
                identifier: 4,
                base_frequency: 935012500,
                channel_spacing: 12500,
                transmit_offset: -39000000,
            }
        );
    }

    #[test]
    fn parse_network_status_broadcast() {
        let json = r#"{"nac":"0x5FC","opcode":"0x39","name":"NET_STS_BCST","last_block":true,"manufacturer_id":0,"wacn":"0x0C018","system_id":"0x704","channel":459,"frequency":null}"#;
        let msg = parse_json_line(json).unwrap();
        assert_eq!(
            msg,
            ParsedMessage::NetworkStatusBroadcast {
                wacn: "0x0C018".to_string(),
                system_id: "0x704".to_string(),
                channel: 459,
                frequency: None,
            }
        );
    }

    #[test]
    fn parse_net2_sts_bcst_as_network_status() {
        let json = r#"{"name":"NET2_STS_BCST","wacn":"0xBEE00","system_id":"0x5F2","channel":215,"frequency":null}"#;
        let msg = parse_json_line(json).unwrap();
        assert!(matches!(msg, ParsedMessage::NetworkStatusBroadcast { .. }));
    }

    #[test]
    fn parse_rfss_status_broadcast() {
        let json = r#"{"nac":"0x5FC","opcode":"0x3A","name":"RFSS_STS_BCST","last_block":false,"manufacturer_id":0,"system_id":"0x5F2","rfss_id":1,"site_id":12,"channel":215,"frequency":null}"#;
        let msg = parse_json_line(json).unwrap();
        assert_eq!(
            msg,
            ParsedMessage::RfssStatusBroadcast {
                system_id: "0x5F2".to_string(),
                rfss_id: 1,
                site_id: 12,
                channel: 215,
                frequency: None,
            }
        );
    }

    #[test]
    fn parse_adjacent_status_broadcast() {
        let json = r#"{"nac":"0x5FC","opcode":"0x3C","name":"ADJ_STS_BCST","last_block":true,"manufacturer_id":0,"system_id":"0x5F2","rfss_id":1,"site_id":3,"channel":451,"frequency":null}"#;
        let msg = parse_json_line(json).unwrap();
        assert_eq!(
            msg,
            ParsedMessage::AdjacentStatusBroadcast {
                system_id: "0x5F2".to_string(),
                rfss_id: 1,
                site_id: 3,
                channel: 451,
                frequency: None,
            }
        );
    }

    #[test]
    fn parse_emergency_alarm() {
        let json = r#"{"name":"EMERGENCY_ALRM","target":5000,"source":12345}"#;
        let msg = parse_json_line(json).unwrap();
        assert_eq!(
            msg,
            ParsedMessage::EmergencyAlarm {
                target: 5000,
                source: 12345,
            }
        );
    }

    #[test]
    fn parse_unit_registration_response() {
        let json = r#"{"name":"U_REG_RSP","response":0,"system_id":"0x704","source_id":100,"source_address":54321}"#;
        let msg = parse_json_line(json).unwrap();
        assert_eq!(
            msg,
            ParsedMessage::UnitRegistrationResponse {
                response: 0,
                source_address: 54321,
            }
        );
    }

    #[test]
    fn parse_unit_deregistration_ack() {
        let json = r#"{"name":"U_DE_REG_ACK","wacn":"0x0C018","system_id":"0x704","source":99999}"#;
        let msg = parse_json_line(json).unwrap();
        assert_eq!(
            msg,
            ParsedMessage::UnitDeregistrationAck { source: 99999 }
        );
    }

    #[test]
    fn parse_deny_response() {
        let json = r#"{"name":"DENY_RSP","service_type":4,"reason":2,"additional":0,"target":12345}"#;
        let msg = parse_json_line(json).unwrap();
        assert_eq!(
            msg,
            ParsedMessage::DenyResponse {
                service_type: 4,
                reason: 2,
                target: 12345,
            }
        );
    }

    #[test]
    fn parse_group_affiliation_response() {
        let json = r#"{"name":"GRP_AFF_RSP","local_global":0,"group_affiliation_value":1,"announcement_group":0,"group":500,"target":12345}"#;
        let msg = parse_json_line(json).unwrap();
        assert_eq!(
            msg,
            ParsedMessage::GroupAffiliationResponse {
                group: 500,
                target: 12345,
            }
        );
    }

    #[test]
    fn parse_unknown_opcode_produces_other() {
        let json = r#"{"name":"PWR_CTRL_BCST","data":"1122334455667788"}"#;
        let msg = parse_json_line(json).unwrap();
        assert_eq!(
            msg,
            ParsedMessage::Other {
                name: "PWR_CTRL_BCST".to_string()
            }
        );
    }

    #[test]
    fn parse_missing_name_field_produces_other() {
        let json = r#"{"opcode":"0xFF"}"#;
        let msg = parse_json_line(json).unwrap();
        assert_eq!(
            msg,
            ParsedMessage::Other {
                name: String::new()
            }
        );
    }

    #[test]
    fn parse_malformed_json_returns_error() {
        let result = parse_json_line("not valid json {{{");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), MonitorError::JsonParse(_)));
    }

    #[test]
    fn parse_null_frequency_is_none() {
        let json = r#"{"name":"GRP_V_CH_GRANT","channel":100,"frequency":null,"talkgroup":200,"source":300}"#;
        let msg = parse_json_line(json).unwrap();
        match msg {
            ParsedMessage::GroupVoiceGrant { frequency, .. } => {
                assert!(frequency.is_none());
            }
            _ => panic!("expected GroupVoiceGrant"),
        }
    }

    #[test]
    fn parse_missing_frequency_field_is_none() {
        let json = r#"{"name":"GRP_V_CH_GRANT","channel":100,"talkgroup":200,"source":300}"#;
        let msg = parse_json_line(json).unwrap();
        match msg {
            ParsedMessage::GroupVoiceGrant { frequency, .. } => {
                assert!(frequency.is_none());
            }
            _ => panic!("expected GroupVoiceGrant"),
        }
    }
}

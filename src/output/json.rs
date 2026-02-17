//! JSON serialization of decoded P25 messages.
//!
//! Converts parsed TSBKs and voice events into single-line JSON for
//! stdout output. Channel numbers are resolved to frequencies when
//! an identifier table is available.

use serde::Serialize;

use crate::p25::ident::IdentTable;
use crate::p25::tsbk::{Tsbk, TsbkPayload};
use crate::p25::types::Nac;
use crate::p25::voice::control::LinkControlFields;
use crate::p25::voice::crypto::CryptoControlFields;
use crate::p25::voice::frame::VoiceFrame;
use crate::p25::voice::header::VoiceHeaderFields;

/// A TSBK serialized as a JSON object.
#[derive(Debug, Serialize)]
pub struct TsbkJson {
    /// Network access code (hex string).
    pub nac: String,
    /// Opcode as hex string.
    pub opcode: String,
    /// Human-readable opcode name.
    pub name: &'static str,
    /// Whether this is the last block in a TSDU.
    pub last_block: bool,
    /// Manufacturer ID.
    pub manufacturer_id: u8,
    /// Opcode-specific fields.
    #[serde(flatten)]
    pub fields: TsbkFields,
}

/// Opcode-specific fields for JSON output.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum TsbkFields {
    /// Group voice channel grant.
    GroupVoiceGrant {
        /// Channel number (raw 16-bit value).
        channel: u16,
        /// Receive frequency in MHz, or null if unresolvable.
        frequency: Option<f64>,
        /// Talkgroup ID.
        talkgroup: u16,
        /// Source unit ID.
        source: u32,
    },
    /// Group voice channel grant update.
    GroupVoiceGrantUpdate {
        /// First channel number.
        channel_a: u16,
        /// First receive frequency in MHz, or null.
        frequency_a: Option<f64>,
        /// First talkgroup.
        talkgroup_a: u16,
        /// Second channel number.
        channel_b: u16,
        /// Second receive frequency in MHz, or null.
        frequency_b: Option<f64>,
        /// Second talkgroup.
        talkgroup_b: u16,
    },
    /// Group voice channel grant update - explicit.
    GroupVoiceGrantUpdateExplicit {
        /// Service options byte.
        service_options: u8,
        /// Transmit channel number.
        transmit_channel: u16,
        /// Transmit frequency in MHz, or null.
        transmit_frequency: Option<f64>,
        /// Receive channel number.
        receive_channel: u16,
        /// Receive frequency in MHz, or null.
        receive_frequency: Option<f64>,
        /// Talkgroup ID.
        talkgroup: u16,
    },
    /// Unit-to-unit answer request.
    UnitToUnitAnswerRequest {
        /// Service options byte.
        service_options: u8,
        /// Target unit ID.
        target: u32,
        /// Source unit ID.
        source: u32,
    },
    /// Emergency alarm.
    EmergencyAlarm {
        /// Target unit or talkgroup ID.
        target: u32,
        /// Source unit ID.
        source: u32,
    },
    /// SNDCP data channel grant.
    SndcpDataChannelGrant {
        /// Data channel number.
        data_channel: u16,
        /// Data channel frequency in MHz, or null.
        frequency: Option<f64>,
        /// Target unit ID.
        target: u32,
    },
    /// Deny response.
    DenyResponse {
        /// Denied service type.
        service_type: u8,
        /// Deny reason code.
        reason: u8,
        /// Additional info (unit or talkgroup).
        additional: u32,
        /// Target unit ID.
        target: u32,
    },
    /// Identifier update.
    IdentifierUpdate {
        /// 4-bit identifier.
        identifier: u8,
        /// Channel bandwidth in hertz.
        bandwidth: u32,
        /// Transmit offset in hertz.
        transmit_offset: i64,
        /// Channel spacing in hertz.
        channel_spacing: u32,
        /// Base frequency in hertz.
        base_frequency: u64,
    },
    /// Group affiliation response.
    GroupAffiliationResponse {
        /// Local/global flag.
        local_global: u8,
        /// Group affiliation value.
        group_affiliation_value: u8,
        /// Announcement group.
        announcement_group: u16,
        /// Group address (talkgroup).
        group: u16,
        /// Target unit ID.
        target: u32,
    },
    /// Unit registration response.
    UnitRegistrationResponse {
        /// Response code (0=accept, 1=fail, 2=deny, 3=refuse).
        response: u8,
        /// System ID (hex string).
        system_id: String,
        /// Source identifier.
        source_id: u32,
        /// Source address.
        source_address: u32,
    },
    /// Unit deregistration acknowledgement.
    UnitDeregistrationAck {
        /// WACN (hex string).
        wacn: String,
        /// System ID (hex string).
        system_id: String,
        /// Source unit ID.
        source: u32,
    },
    /// Power control broadcast (raw payload).
    PowerControlBroadcast {
        /// Raw payload as hex string.
        data: String,
    },
    /// Network status broadcast.
    NetworkStatusBroadcast {
        /// WACN (hex string).
        wacn: String,
        /// System ID (hex string).
        system_id: String,
        /// Control channel number.
        channel: u16,
        /// Control channel frequency in MHz, or null.
        frequency: Option<f64>,
    },
    /// RFSS status broadcast.
    RfssStatusBroadcast {
        /// System ID (hex string).
        system_id: String,
        /// RFSS identifier.
        rfss_id: u8,
        /// Site identifier.
        site_id: u8,
        /// Control channel number.
        channel: u16,
        /// Control channel frequency in MHz, or null.
        frequency: Option<f64>,
    },
    /// Adjacent site status broadcast.
    AdjacentStatusBroadcast {
        /// System ID (hex string).
        system_id: String,
        /// RFSS identifier.
        rfss_id: u8,
        /// Site identifier.
        site_id: u8,
        /// Control channel number.
        channel: u16,
        /// Control channel frequency in MHz, or null.
        frequency: Option<f64>,
    },
    /// Unknown opcode with raw hex payload.
    Unknown {
        /// Raw payload bytes as hex string.
        data: String,
    },
}

/// Format a frequency in MHz with 5 decimal places, or None.
fn resolve_mhz(table: &IdentTable, channel: crate::p25::types::ChannelNumber) -> Option<f64> {
    let freq = table.resolve_frequency(channel)?;
    Some(format_mhz(freq.hz()))
}

/// Format Hz as MHz with 5 decimal places (truncated to f64 precision).
fn format_mhz(hz: u64) -> f64 {
    hz as f64 / 1_000_000.0
}

/// Build a JSON-serializable representation of a decoded TSBK.
pub fn to_json_value(nac: Nac, tsbk: &Tsbk, ident_table: &IdentTable) -> TsbkJson {
    let fields = match &tsbk.payload {
        TsbkPayload::GroupVoiceChannelGrant {
            channel,
            talkgroup,
            source,
        } => TsbkFields::GroupVoiceGrant {
            channel: channel.value(),
            frequency: resolve_mhz(ident_table, *channel),
            talkgroup: talkgroup.value(),
            source: source.value(),
        },

        TsbkPayload::GroupVoiceChannelGrantUpdate {
            channel_a,
            talkgroup_a,
            channel_b,
            talkgroup_b,
        } => TsbkFields::GroupVoiceGrantUpdate {
            channel_a: channel_a.value(),
            frequency_a: resolve_mhz(ident_table, *channel_a),
            talkgroup_a: talkgroup_a.value(),
            channel_b: channel_b.value(),
            frequency_b: resolve_mhz(ident_table, *channel_b),
            talkgroup_b: talkgroup_b.value(),
        },

        TsbkPayload::GroupVoiceChannelGrantUpdateExplicit {
            service_options,
            transmit_channel,
            receive_channel,
            talkgroup,
        } => TsbkFields::GroupVoiceGrantUpdateExplicit {
            service_options: *service_options,
            transmit_channel: transmit_channel.value(),
            transmit_frequency: resolve_mhz(ident_table, *transmit_channel),
            receive_channel: receive_channel.value(),
            receive_frequency: resolve_mhz(ident_table, *receive_channel),
            talkgroup: talkgroup.value(),
        },

        TsbkPayload::UnitToUnitAnswerRequest {
            service_options,
            target,
            source,
        } => TsbkFields::UnitToUnitAnswerRequest {
            service_options: *service_options,
            target: target.value(),
            source: source.value(),
        },

        TsbkPayload::EmergencyAlarm { target, source } => TsbkFields::EmergencyAlarm {
            target: target.value(),
            source: source.value(),
        },

        TsbkPayload::SndcpDataChannelGrant {
            data_channel,
            target,
        } => TsbkFields::SndcpDataChannelGrant {
            data_channel: data_channel.value(),
            frequency: resolve_mhz(ident_table, *data_channel),
            target: target.value(),
        },

        TsbkPayload::DenyResponse {
            service_type,
            reason,
            additional,
            target,
        } => TsbkFields::DenyResponse {
            service_type: *service_type,
            reason: *reason,
            additional: additional.value(),
            target: target.value(),
        },

        TsbkPayload::IdentifierUpdate {
            identifier,
            bandwidth,
            transmit_offset,
            channel_spacing,
            base_frequency,
        } => TsbkFields::IdentifierUpdate {
            identifier: *identifier,
            bandwidth: *bandwidth,
            transmit_offset: *transmit_offset,
            channel_spacing: *channel_spacing,
            base_frequency: *base_frequency,
        },

        TsbkPayload::GroupAffiliationResponse {
            local_global,
            group_affiliation_value,
            announcement_group,
            group,
            target,
        } => TsbkFields::GroupAffiliationResponse {
            local_global: *local_global,
            group_affiliation_value: *group_affiliation_value,
            announcement_group: announcement_group.value(),
            group: group.value(),
            target: target.value(),
        },

        TsbkPayload::UnitRegistrationResponse {
            response,
            system_id,
            source_id,
            source_address,
        } => TsbkFields::UnitRegistrationResponse {
            response: *response,
            system_id: format!("0x{:03X}", system_id.value()),
            source_id: source_id.value(),
            source_address: source_address.value(),
        },

        TsbkPayload::UnitDeregistrationAck {
            wacn,
            system_id,
            source,
        } => TsbkFields::UnitDeregistrationAck {
            wacn: format!("0x{:05X}", wacn.value()),
            system_id: format!("0x{:03X}", system_id.value()),
            source: source.value(),
        },

        TsbkPayload::PowerControlBroadcast { data } => TsbkFields::PowerControlBroadcast {
            data: data.iter().map(|b| format!("{b:02X}")).collect::<String>(),
        },

        TsbkPayload::NetworkStatusBroadcast {
            wacn,
            system_id,
            channel,
        } => TsbkFields::NetworkStatusBroadcast {
            wacn: format!("0x{:05X}", wacn.value()),
            system_id: format!("0x{:03X}", system_id.value()),
            channel: channel.value(),
            frequency: resolve_mhz(ident_table, *channel),
        },

        TsbkPayload::RfssStatusBroadcast {
            system_id,
            rfss_id,
            site_id,
            channel,
        } => TsbkFields::RfssStatusBroadcast {
            system_id: format!("0x{:03X}", system_id.value()),
            rfss_id: rfss_id.value(),
            site_id: site_id.value(),
            channel: channel.value(),
            frequency: resolve_mhz(ident_table, *channel),
        },

        TsbkPayload::AdjacentStatusBroadcast {
            system_id,
            rfss_id,
            site_id,
            channel,
        } => TsbkFields::AdjacentStatusBroadcast {
            system_id: format!("0x{:03X}", system_id.value()),
            rfss_id: rfss_id.value(),
            site_id: site_id.value(),
            channel: channel.value(),
            frequency: resolve_mhz(ident_table, *channel),
        },

        TsbkPayload::Unknown { data } => TsbkFields::Unknown {
            data: data.iter().map(|b| format!("{b:02X}")).collect::<String>(),
        },
    };

    TsbkJson {
        nac: format!("0x{:03X}", nac.value()),
        opcode: format!("0x{:02X}", tsbk.header.opcode.raw()),
        name: tsbk.header.opcode.name(),
        last_block: tsbk.header.last_block,
        manufacturer_id: tsbk.header.manufacturer_id,
        fields,
    }
}

/// Serialize a TSBK to a single JSON line.
pub fn to_json_line(nac: Nac, tsbk: &Tsbk, ident_table: &IdentTable) -> String {
    let value = to_json_value(nac, tsbk, ident_table);
    serde_json::to_string(&value).expect("TsbkJson serialization should not fail")
}

// ---------------------------------------------------------------------------
// Voice event JSON
// ---------------------------------------------------------------------------

/// A voice event serialized as a JSON object.
#[derive(Debug, Serialize)]
pub struct VoiceEventJson {
    /// Network access code (hex string).
    pub nac: String,
    /// Event type discriminator.
    #[serde(rename = "type")]
    pub event_type: &'static str,
    /// Event-specific fields.
    #[serde(flatten)]
    pub fields: VoiceEventFields,
}

/// Voice event-specific fields for JSON output.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum VoiceEventFields {
    /// IMBE voice frame.
    VoiceFrame {
        /// IMBE data as hex string (11 bytes = 88 bits).
        imbe: String,
        /// Total FEC errors corrected.
        errors: usize,
    },
    /// Link Control from LDU1.
    LinkControl {
        /// LC opcode name.
        lc_opcode: String,
        /// Talkgroup (if group voice traffic).
        #[serde(skip_serializing_if = "Option::is_none")]
        talkgroup: Option<u16>,
        /// Source unit ID (if group voice traffic).
        #[serde(skip_serializing_if = "Option::is_none")]
        source: Option<u32>,
        /// Raw LC payload as hex.
        lc_data: String,
    },
    /// Crypto Control from LDU2.
    CryptoControl {
        /// Algorithm name.
        algorithm: String,
        /// Key ID.
        key_id: u16,
        /// Initialization vector as hex string.
        initialization_vector: String,
    },
    /// Voice header from HDU.
    VoiceHeader {
        /// Algorithm name.
        algorithm: String,
        /// Key ID.
        key_id: u16,
        /// Talkgroup ID.
        talkgroup: u16,
        /// Manufacturer ID.
        manufacturer_id: u8,
        /// Crypto initialization vector as hex string.
        initialization_vector: String,
    },
    /// Low-speed data fragment.
    DataFragment {
        /// The 16-bit decoded low-speed data value (two cyclic codewords).
        data: u16,
    },
}

/// Pack IMBE voice frame chunks into a hex string.
///
/// The 88 bits are packed MSB-first from the 8 chunks:
/// u_0..u_3 (12 bits each), u_4..u_6 (11 bits each), u_7 (7 bits).
fn imbe_to_hex(frame: &VoiceFrame) -> String {
    let mut bits: u128 = 0;
    for i in 0..=3 {
        bits = (bits << 12) | u128::from(frame.chunks[i]);
    }
    for i in 4..=6 {
        bits = (bits << 11) | u128::from(frame.chunks[i]);
    }
    bits = (bits << 7) | u128::from(frame.chunks[7]);
    // 88 bits = 11 bytes
    let bytes = bits.to_be_bytes();
    // u128 is 16 bytes; our 11 bytes start at offset 5
    bytes[5..16]
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect()
}

/// Build a JSON line for a voice frame event.
pub fn voice_frame_json_line(nac: Nac, frame: &VoiceFrame) -> String {
    let event = VoiceEventJson {
        nac: format!("0x{:03X}", nac.value()),
        event_type: "voice_frame",
        fields: VoiceEventFields::VoiceFrame {
            imbe: imbe_to_hex(frame),
            errors: frame.total_errors(),
        },
    };
    serde_json::to_string(&event).expect("VoiceEventJson serialization should not fail")
}

/// Build a JSON line for a Link Control event.
pub fn link_control_json_line(nac: Nac, lc: &LinkControlFields) -> String {
    use crate::p25::voice::control::{GroupVoiceTraffic, LinkControlOpcode};

    let opcode = lc.opcode();
    let (talkgroup, source) = if opcode == LinkControlOpcode::GroupVoiceTraffic {
        let gvt = GroupVoiceTraffic::new(*lc);
        (Some(gvt.talkgroup().value()), Some(gvt.source_unit().value()))
    } else {
        (None, None)
    };

    let event = VoiceEventJson {
        nac: format!("0x{:03X}", nac.value()),
        event_type: "link_control",
        fields: VoiceEventFields::LinkControl {
            lc_opcode: format!("{opcode}"),
            talkgroup,
            source,
            lc_data: lc.raw().iter().map(|b| format!("{b:02X}")).collect(),
        },
    };
    serde_json::to_string(&event).expect("VoiceEventJson serialization should not fail")
}

/// Build a JSON line for a Crypto Control event.
pub fn crypto_control_json_line(nac: Nac, cc: &CryptoControlFields) -> String {
    let event = VoiceEventJson {
        nac: format!("0x{:03X}", nac.value()),
        event_type: "crypto_control",
        fields: VoiceEventFields::CryptoControl {
            algorithm: format!("{}", cc.algorithm()),
            key_id: cc.key_id(),
            initialization_vector: cc
                .initialization_vector()
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect(),
        },
    };
    serde_json::to_string(&event).expect("VoiceEventJson serialization should not fail")
}

/// Build a JSON line for a Voice Header event.
pub fn voice_header_json_line(nac: Nac, hdr: &VoiceHeaderFields) -> String {
    let event = VoiceEventJson {
        nac: format!("0x{:03X}", nac.value()),
        event_type: "voice_header",
        fields: VoiceEventFields::VoiceHeader {
            algorithm: format!("{}", hdr.algorithm()),
            key_id: hdr.key_id(),
            talkgroup: hdr.talkgroup().value(),
            manufacturer_id: hdr.manufacturer_id(),
            initialization_vector: hdr
                .crypto_init()
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect(),
        },
    };
    serde_json::to_string(&event).expect("VoiceEventJson serialization should not fail")
}

/// Build a JSON line for a data fragment event.
pub fn data_fragment_json_line(nac: Nac, data: u16) -> String {
    let event = VoiceEventJson {
        nac: format!("0x{:03X}", nac.value()),
        event_type: "data_fragment",
        fields: VoiceEventFields::DataFragment { data },
    };
    serde_json::to_string(&event).expect("VoiceEventJson serialization should not fail")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p25::crc;
    use crate::p25::tsbk;

    fn make_tsbk_with_crc(data: &[u8; 10]) -> [u8; 12] {
        let crc = crc::crc16(data);
        let mut tsbk_bytes = [0u8; 12];
        tsbk_bytes[..10].copy_from_slice(data);
        tsbk_bytes[10] = (crc >> 8) as u8;
        tsbk_bytes[11] = crc as u8;
        tsbk_bytes
    }

    #[test]
    fn json_group_voice_grant_without_ident_table() {
        let data: [u8; 10] = [0x80, 0x00, 0x61, 0x23, 0x00, 0x42, 0x01, 0x02, 0x03, 0x00];
        let tsbk_bytes = make_tsbk_with_crc(&data);
        let parsed = tsbk::parse(&tsbk_bytes).unwrap();
        let nac = Nac::new(0x293);
        let table = IdentTable::new();

        let json = to_json_line(nac, &parsed, &table);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(v["nac"], "0x293");
        assert_eq!(v["opcode"], "0x00");
        assert_eq!(v["name"], "GRP_V_CH_GRANT");
        assert_eq!(v["channel"], 0x6123);
        assert!(v["frequency"].is_null());
        assert_eq!(v["talkgroup"], 66);
        assert_eq!(v["source"], 0x010203);
    }

    #[test]
    fn json_group_voice_grant_with_resolved_frequency() {
        let data: [u8; 10] = [0x80, 0x00, 0x60, 0x09, 0x00, 0x42, 0x01, 0x02, 0x03, 0x00];
        let tsbk_bytes = make_tsbk_with_crc(&data);
        let parsed = tsbk::parse(&tsbk_bytes).unwrap();
        let nac = Nac::new(0x293);

        let mut table = IdentTable::new();
        let ident_tsbk = tsbk::Tsbk {
            header: tsbk::TsbkHeader {
                last_block: true,
                protected: false,
                opcode: tsbk::TsbkOpcode::IdentifierUpdate,
                manufacturer_id: 0,
            },
            payload: TsbkPayload::IdentifierUpdate {
                identifier: 6,
                bandwidth: 12_500,
                transmit_offset: -45_000_000,
                channel_spacing: 6_250,
                base_frequency: 851_006_250,
            },
        };
        table.update(&ident_tsbk);

        let json = to_json_line(nac, &parsed, &table);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(v["channel"], 0x6009);
        let freq = v["frequency"].as_f64().unwrap();
        assert!((freq - 851.0625).abs() < 0.0001);
    }

    #[test]
    fn json_identifier_update() {
        let data: [u8; 10] = [0xBD, 0x00, 0x63, 0x22, 0xD0, 0x32, 0x0A, 0x25, 0x10, 0xA2];
        let tsbk_bytes = make_tsbk_with_crc(&data);
        let parsed = tsbk::parse(&tsbk_bytes).unwrap();
        let nac = Nac::new(0x293);
        let table = IdentTable::new();

        let json = to_json_line(nac, &parsed, &table);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(v["name"], "CH_PARAMS_UPDT");
        assert_eq!(v["identifier"], 6);
        assert_eq!(v["bandwidth"], 12_500);
        assert_eq!(v["channel_spacing"], 6_250);
        assert_eq!(v["base_frequency"], 851_006_250u64);
    }

    #[test]
    fn json_network_status_broadcast() {
        let data: [u8; 10] = [0xB9, 0x00, 0xCA, 0xFC, 0x2B, 0xCF, 0x5B, 0xDC, 0xE7, 0x51];
        let tsbk_bytes = make_tsbk_with_crc(&data);
        let parsed = tsbk::parse(&tsbk_bytes).unwrap();
        let nac = Nac::new(0x293);
        let table = IdentTable::new();

        let json = to_json_line(nac, &parsed, &table);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(v["name"], "NET_STS_BCST");
        assert_eq!(v["wacn"], "0xFC2BC");
        assert_eq!(v["system_id"], "0xF5B");
        assert_eq!(v["channel"], 0xDCE7);
    }

    #[test]
    fn json_rfss_status_broadcast() {
        let data: [u8; 10] = [0xBA, 0x00, 0xCC, 0x10, 0xAA, 0xE7, 0x18, 0xD5, 0x73, 0x51];
        let tsbk_bytes = make_tsbk_with_crc(&data);
        let parsed = tsbk::parse(&tsbk_bytes).unwrap();
        let nac = Nac::new(0x293);
        let table = IdentTable::new();

        let json = to_json_line(nac, &parsed, &table);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(v["name"], "RFSS_STS_BCST");
        assert_eq!(v["system_id"], "0x0AA");
        assert_eq!(v["rfss_id"], 0xE7);
        assert_eq!(v["site_id"], 0x18);
    }

    #[test]
    fn json_unknown_opcode() {
        let data: [u8; 10] = [0x8F, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        let tsbk_bytes = make_tsbk_with_crc(&data);
        let parsed = tsbk::parse(&tsbk_bytes).unwrap();
        let nac = Nac::new(0x293);
        let table = IdentTable::new();

        let json = to_json_line(nac, &parsed, &table);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(v["name"], "UNKNOWN");
        assert_eq!(v["opcode"], "0x0F");
        assert_eq!(v["data"], "1122334455667788");
    }

    #[test]
    fn json_is_single_line() {
        let data: [u8; 10] = [0x80, 0x00, 0x61, 0x23, 0x00, 0x42, 0x01, 0x02, 0x03, 0x00];
        let tsbk_bytes = make_tsbk_with_crc(&data);
        let parsed = tsbk::parse(&tsbk_bytes).unwrap();
        let nac = Nac::new(0x293);
        let table = IdentTable::new();

        let json = to_json_line(nac, &parsed, &table);
        assert!(!json.contains('\n'));
    }

    #[test]
    fn json_is_valid_json() {
        let data: [u8; 10] = [0x80, 0x00, 0x61, 0x23, 0x00, 0x42, 0x01, 0x02, 0x03, 0x00];
        let tsbk_bytes = make_tsbk_with_crc(&data);
        let parsed = tsbk::parse(&tsbk_bytes).unwrap();
        let nac = Nac::new(0x293);
        let table = IdentTable::new();

        let json = to_json_line(nac, &parsed, &table);
        let result: Result<serde_json::Value, _> = serde_json::from_str(&json);
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // Voice event JSON tests
    // -----------------------------------------------------------------------

    #[test]
    fn json_voice_frame_output() {
        let frame = VoiceFrame {
            chunks: [0x123, 0x456, 0x789, 0xABC, 0x3FF, 0x555, 0x000, 0x7F],
            errors: [0, 1, 0, 0, 0, 2, 0],
        };
        let nac = Nac::new(0x5FC);
        let json = voice_frame_json_line(nac, &frame);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(v["nac"], "0x5FC");
        assert_eq!(v["type"], "voice_frame");
        assert_eq!(v["errors"], 3);
        // 88 bits = 11 bytes → 22 hex chars
        assert_eq!(v["imbe"].as_str().unwrap().len(), 22);
        assert!(!json.contains('\n'));
    }

    #[test]
    fn json_voice_frame_imbe_hex_all_zeros() {
        let frame = VoiceFrame {
            chunks: [0; 8],
            errors: [0; 7],
        };
        let nac = Nac::new(0x293);
        let json = voice_frame_json_line(nac, &frame);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(v["imbe"], "0000000000000000000000");
    }

    #[test]
    fn json_link_control_group_voice() {
        // Opcode 0x00 = GroupVoiceTraffic, talkgroup=0x0042, source=0x010203
        let lc = LinkControlFields::new([
            0x00, // opcode 0x00, not protected
            0x00, // MFID
            0x00, // service options
            0x00, // reserved
            0x00, 0x42, // talkgroup
            0x01, 0x02, 0x03, // source unit
        ]);
        let nac = Nac::new(0x5FC);
        let json = link_control_json_line(nac, &lc);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(v["nac"], "0x5FC");
        assert_eq!(v["type"], "link_control");
        assert_eq!(v["lc_opcode"], "GRP_V_CH_USR");
        assert_eq!(v["talkgroup"], 0x0042);
        assert_eq!(v["source"], 0x010203);
        assert!(v["lc_data"].is_string());
        assert!(!json.contains('\n'));
    }

    #[test]
    fn json_link_control_non_group_voice_omits_talkgroup() {
        // Opcode 0x0F = CallTermination — no talkgroup/source fields
        let lc = LinkControlFields::new([
            0x0F, // opcode 0x0F
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ]);
        let nac = Nac::new(0x293);
        let json = link_control_json_line(nac, &lc);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(v["lc_opcode"], "CALL_TERM");
        // talkgroup and source should be absent (skip_serializing_if = None)
        assert!(v.get("talkgroup").is_none() || v["talkgroup"].is_null());
        assert!(v.get("source").is_none() || v["source"].is_null());
    }

    #[test]
    fn json_crypto_control_aes() {
        let mut buf = [0u8; 12];
        buf[0] = 0xAA; // IV byte 0
        buf[8] = 0xBB; // IV byte 8
        buf[9] = 0x84; // AES-256
        buf[10] = 0xDE; // key_id high
        buf[11] = 0xAD; // key_id low
        let cc = CryptoControlFields::new(buf);
        let nac = Nac::new(0x5FC);
        let json = crypto_control_json_line(nac, &cc);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(v["nac"], "0x5FC");
        assert_eq!(v["type"], "crypto_control");
        assert_eq!(v["algorithm"], "AES-256");
        assert_eq!(v["key_id"], 0xDEAD);
        // IV is 9 bytes = 18 hex chars
        assert_eq!(v["initialization_vector"].as_str().unwrap().len(), 18);
        assert!(!json.contains('\n'));
    }

    #[test]
    fn json_crypto_control_unencrypted() {
        let mut buf = [0u8; 12];
        buf[9] = 0x80; // Unencrypted
        let cc = CryptoControlFields::new(buf);
        let nac = Nac::new(0x293);
        let json = crypto_control_json_line(nac, &cc);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(v["algorithm"], "Unencrypted");
        assert_eq!(v["key_id"], 0);
    }

    #[test]
    fn json_data_fragment() {
        let nac = Nac::new(0x5FC);
        let json = data_fragment_json_line(nac, 0xABCD);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(v["nac"], "0x5FC");
        assert_eq!(v["type"], "data_fragment");
        assert_eq!(v["data"], 0xABCD);
        assert!(!json.contains('\n'));
    }

    #[test]
    fn json_data_fragment_zero() {
        let nac = Nac::new(0x293);
        let json = data_fragment_json_line(nac, 0);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(v["data"], 0);
    }

    #[test]
    fn json_grant_update() {
        let data: [u8; 10] = [0x82, 0x00, 0x61, 0x23, 0x00, 0x42, 0x71, 0x45, 0x00, 0x99];
        let tsbk_bytes = make_tsbk_with_crc(&data);
        let parsed = tsbk::parse(&tsbk_bytes).unwrap();
        let nac = Nac::new(0x293);
        let table = IdentTable::new();

        let json = to_json_line(nac, &parsed, &table);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(v["name"], "GRP_V_CH_GRANT_UPDT");
        assert_eq!(v["channel_a"], 0x6123);
        assert_eq!(v["talkgroup_a"], 66);
        assert_eq!(v["channel_b"], 0x7145);
        assert_eq!(v["talkgroup_b"], 0x0099);
    }
}

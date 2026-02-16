//! JSON serialization of decoded P25 TSBK messages.
//!
//! Converts parsed TSBKs into single-line JSON for stdout output.
//! Channel numbers are resolved to frequencies when an identifier
//! table is available.

use serde::Serialize;

use crate::p25::ident::IdentTable;
use crate::p25::tsbk::{Tsbk, TsbkPayload};
use crate::p25::types::Nac;

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

        assert_eq!(v["name"], "IDEN_UP_TDMA");
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

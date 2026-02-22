//! API configuration types and mode detection for managed feeder startup.
//!
//! These types are consumed by the config fetch and mode detection logic
//! added in subsequent tasks. Allow dead_code until wired in.
#![allow(dead_code)]

use serde::Deserialize;
use uuid::Uuid;

/// Response from `GET /api/feeder/{id}/config`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeederConfig {
    /// LiveKit JWT token (24h TTL, publish-only).
    pub token: String,
    /// LiveKit server URL.
    pub url: String,
    /// LiveKit room name (system slug).
    pub room: String,
    /// System identification.
    pub system: SystemInfo,
    /// Opaque admin-editable config blob.
    pub config: serde_json::Value,
    /// SDR configuration (present only if feeder is assigned to a site).
    pub sdr: Option<SdrConfig>,
}

/// System identification from the API.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfo {
    /// System UUID.
    pub id: Uuid,
    /// Full system name.
    pub name: String,
    /// Short system name.
    pub short_name: String,
    /// P25 system ID (hex string like "0x5F2").
    pub system_id: String,
    /// WACN (hex string like "BEE00").
    pub wacn: String,
}

/// SDR configuration from the API.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SdrConfig {
    /// Unit for frequency values (always "Hz").
    pub unit: String,
    /// Top of the frequency range in Hz.
    pub freq_top: u64,
    /// Bottom of the frequency range in Hz.
    pub freq_bottom: u64,
    /// Total bandwidth in Hz (freq_top - freq_bottom).
    pub bandwidth: u64,
    /// Optimal center frequency in Hz.
    pub center_frequency: u64,
    /// List of candidate control channel frequencies in Hz.
    pub control_channels: Vec<u64>,
}

/// Operating mode for the server.
#[derive(Debug)]
pub enum ServerMode {
    /// Managed by trunker-web API.
    Managed {
        /// Feeder UUID for API registration.
        feeder_id: Uuid,
        /// API key for authentication.
        api_key: String,
        /// API base URL.
        api_url: String,
    },
    /// Standalone, all config from CLI/env.
    Standalone,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_CONFIG: &str = r#"{
        "token": "eyJhbGciOiJIUzI1NiJ9.test",
        "url": "wss://lk.example.com",
        "room": "srrcs",
        "system": {
            "id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
            "name": "Sacramento Regional Radio Communications System",
            "shortName": "SRRCS",
            "systemId": "0x5F2",
            "wacn": "BEE00"
        },
        "config": {},
        "sdr": {
            "unit": "Hz",
            "freqTop": 853900000,
            "freqBottom": 851050000,
            "bandwidth": 2850000,
            "centerFrequency": 852475000,
            "controlChannels": [852350000, 853187500, 853450000, 853875000]
        }
    }"#;

    #[test]
    fn deserialize_full_config() {
        let config: FeederConfig = serde_json::from_str(SAMPLE_CONFIG).unwrap();
        assert_eq!(config.room, "srrcs");
        assert_eq!(config.system.short_name, "SRRCS");
        assert_eq!(config.system.system_id, "0x5F2");
        let sdr = config.sdr.unwrap();
        assert_eq!(sdr.control_channels.len(), 4);
        assert_eq!(sdr.center_frequency, 852475000);
        assert_eq!(sdr.bandwidth, 2850000);
    }

    #[test]
    fn deserialize_config_without_sdr() {
        let json = r#"{
            "token": "test",
            "url": "wss://lk.example.com",
            "room": "srrcs",
            "system": {
                "id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
                "name": "Test System",
                "shortName": "TEST",
                "systemId": "0x001",
                "wacn": "00001"
            },
            "config": {}
        }"#;
        let config: FeederConfig = serde_json::from_str(json).unwrap();
        assert!(config.sdr.is_none());
    }

    #[test]
    fn sdr_config_frequencies_are_integers() {
        let config: FeederConfig = serde_json::from_str(SAMPLE_CONFIG).unwrap();
        let sdr = config.sdr.unwrap();
        assert_eq!(sdr.freq_top - sdr.freq_bottom, sdr.bandwidth);
    }
}

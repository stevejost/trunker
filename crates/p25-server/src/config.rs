//! API configuration types and mode detection for managed feeder startup.

use anyhow::{Result, bail};
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
    /// Opaque admin-editable config blob (reserved for future use).
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    pub unit: String,
    /// Top of the frequency range in Hz.
    #[allow(dead_code)]
    pub freq_top: u64,
    /// Bottom of the frequency range in Hz.
    #[allow(dead_code)]
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

impl ServerMode {
    /// Detect operating mode from CLI arguments.
    ///
    /// Managed mode requires both `feeder_id` and `api_key`.
    /// Having one without the other is an error.
    pub fn detect(feeder_id: Option<Uuid>, api_key: Option<&str>, api_url: &str) -> Result<Self> {
        match (feeder_id, api_key) {
            (Some(id), Some(key)) => Ok(Self::Managed {
                feeder_id: id,
                api_key: key.to_string(),
                api_url: api_url.to_string(),
            }),
            (None, None) => Ok(Self::Standalone),
            (Some(_), None) => bail!(
                "incomplete configuration: TRUNKER_FEEDER_ID is set but TRUNKER_API_KEY is missing\n  \
                 hint: set both for managed mode, or neither for standalone mode"
            ),
            (None, Some(_)) => bail!(
                "incomplete configuration: TRUNKER_API_KEY is set but TRUNKER_FEEDER_ID is missing\n  \
                 hint: set both for managed mode, or neither for standalone mode"
            ),
        }
    }
}

/// Parse a hex string (with or without `0x` prefix) into a `u32`.
pub fn parse_hex_string(s: &str) -> Option<u32> {
    let hex = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    u32::from_str_radix(hex, 16).ok()
}

/// Fetch feeder configuration from the trunker-web API.
///
/// Retries up to 3 times with exponential backoff (1s, 2s, 4s).
/// Returns the parsed config on success, or an error with actionable hints.
pub async fn fetch_config(api_url: &str, feeder_id: &Uuid, api_key: &str) -> Result<FeederConfig> {
    let url = format!(
        "{}/api/feeder/{}/config",
        api_url.trim_end_matches('/'),
        feeder_id
    );
    let client = reqwest::Client::new();

    let backoffs: &[u64] = &[1, 2, 4];
    let mut last_error = None;

    for (attempt, delay) in backoffs.iter().enumerate() {
        match client
            .get(&url)
            .bearer_auth(api_key)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
        {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    return response
                        .json::<FeederConfig>()
                        .await
                        .map_err(|e| anyhow::anyhow!("failed to parse API config response: {e}"));
                }
                let body = response.text().await.unwrap_or_default();
                match status.as_u16() {
                    401 => bail!(
                        "API returned 401 Unauthorized for feeder {feeder_id}\n  \
                         hint: check TRUNKER_API_KEY — regenerate from the trunker-web dashboard if needed"
                    ),
                    400 => bail!(
                        "API returned 400: {body}\n  \
                         hint: assign this feeder to a system in the trunker-web dashboard\n  \
                         hint: feeder ID: {feeder_id}"
                    ),
                    404 => bail!(
                        "API returned 404: system not found\n  \
                         hint: the system may have been deleted after feeder assignment"
                    ),
                    _ => {
                        last_error = Some(anyhow::anyhow!("API returned {status}: {body}"));
                    }
                }
            }
            Err(e) => {
                last_error = Some(anyhow::anyhow!("request failed: {e}"));
            }
        }

        if attempt < backoffs.len() - 1 {
            tracing::warn!(
                attempt = attempt + 1,
                retry_in_seconds = delay,
                "API request failed, retrying"
            );
            tokio::time::sleep(std::time::Duration::from_secs(*delay)).await;
        }
    }

    let err = last_error.unwrap_or_else(|| anyhow::anyhow!("unknown error"));
    bail!(
        "failed to fetch config from {url} after 3 attempts: {err}\n  \
         hint: check TRUNKER_API_URL and network connectivity\n  \
         hint: to run without the API, use standalone mode (--livekit-url, --frequency, etc.)"
    )
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

    #[test]
    fn detect_managed_mode() {
        let id = Uuid::new_v4();
        let mode = ServerMode::detect(Some(id), Some("key123"), "https://api.example.com").unwrap();
        match mode {
            ServerMode::Managed {
                feeder_id,
                api_key,
                api_url,
            } => {
                assert_eq!(feeder_id, id);
                assert_eq!(api_key, "key123");
                assert_eq!(api_url, "https://api.example.com");
            }
            ServerMode::Standalone => panic!("expected Managed mode"),
        }
    }

    #[test]
    fn detect_standalone_mode() {
        let mode = ServerMode::detect(None, None, "https://api.example.com").unwrap();
        assert!(matches!(mode, ServerMode::Standalone));
    }

    #[test]
    fn detect_feeder_id_without_api_key_is_error() {
        let id = Uuid::new_v4();
        let err = ServerMode::detect(Some(id), None, "https://api.example.com").unwrap_err();
        assert!(err.to_string().contains("TRUNKER_API_KEY is missing"));
    }

    #[test]
    fn detect_api_key_without_feeder_id_is_error() {
        let err = ServerMode::detect(None, Some("key"), "https://api.example.com").unwrap_err();
        assert!(err.to_string().contains("TRUNKER_FEEDER_ID is missing"));
    }

    #[test]
    fn parse_hex_string_with_prefix() {
        assert_eq!(parse_hex_string("0x5F2"), Some(0x5F2));
        assert_eq!(parse_hex_string("0XBEE00"), Some(0xBEE00));
    }

    #[test]
    fn parse_hex_string_without_prefix() {
        assert_eq!(parse_hex_string("BEE00"), Some(0xBEE00));
        assert_eq!(parse_hex_string("5F2"), Some(0x5F2));
    }

    #[test]
    fn parse_hex_string_invalid() {
        assert_eq!(parse_hex_string("ZZZZ"), None);
        assert_eq!(parse_hex_string(""), None);
    }
}

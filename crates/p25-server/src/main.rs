//! P25 LiveKit WebRTC server.
//!
//! Decodes a P25 trunked radio system and publishes audio via LiveKit
//! WebRTC, with JSON metadata on a data channel.
//!
//! Architecture:
//! - A sync decode thread runs the trunked decoder, producing events
//! - Events are broadcast to async consumers via `BroadcastSink`
//! - An audio publisher creates one LiveKit track per talkgroup
//! - A data channel publishes JSON metadata (grants, heartbeats)

mod bridge;
mod data_publisher;
mod livekit_publisher;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use clap::Parser;
use livekit::prelude::*;
use serde::Serialize;
use uuid::Uuid;

use bridge::BroadcastSink;
use data_publisher::DataPublisher;
use livekit_publisher::AudioPublisher;
use trunker::p25::nid::NidIntegrityPolicy;
use trunker::pipeline;
use trunker::sdr::sample_source::SampleSource;
use trunker::sdr::soapy_source::SoapySource;

/// P25 LiveKit WebRTC server.
#[derive(Parser)]
#[command(name = "p25-server", version, about)]
struct Cli {
    /// SoapySDR device argument string (e.g. "driver=sdrplay").
    #[arg(short, long)]
    device: String,

    /// Control channel frequency in Hz.
    #[arg(long)]
    frequency: u64,

    /// Center frequency of the capture in Hz.
    #[arg(short = 'f', long)]
    center_freq: u64,

    /// Sample rate in Hz.
    #[arg(short, long, default_value_t = 2_400_000)]
    sample_rate: u32,

    /// Manual gain in dB (mutually exclusive with --auto-gain).
    #[arg(long)]
    gain: Option<f64>,

    /// Enable automatic gain control.
    #[arg(long)]
    auto_gain: bool,

    /// Antenna port name.
    #[arg(long)]
    antenna: Option<String>,

    /// Device-specific setting as key=value (repeatable).
    #[arg(long = "setting", value_name = "KEY=VALUE")]
    settings: Vec<String>,

    /// SDR read buffer depth in milliseconds.
    #[arg(long, default_value_t = 500)]
    buffer_ms: u32,

    /// Seconds before tearing down an idle voice channel.
    #[arg(long, default_value_t = 3.0)]
    call_timeout: f64,

    /// Maximum simultaneous voice channel pipelines (0 = unlimited).
    #[arg(long, default_value_t = 10)]
    max_voices: usize,

    /// LiveKit server URL (e.g. "wss://my-server.livekit.cloud").
    #[arg(long, env = "LIVEKIT_URL")]
    livekit_url: String,

    /// LiveKit API key.
    #[arg(long, env = "LIVEKIT_API_KEY")]
    livekit_api_key: String,

    /// LiveKit API secret.
    #[arg(long, env = "LIVEKIT_API_SECRET")]
    livekit_api_secret: String,

    /// LiveKit room name to join (defaults to "trunker:{trunker_system_id}").
    #[arg(long)]
    livekit_room: Option<String>,

    /// Participant identity in the LiveKit room (defaults to "feeder:{trunker_feeder_id}").
    #[arg(long)]
    livekit_identity: Option<String>,

    /// Audio gain multiplier for LiveKit output (default: 16.0).
    #[arg(long, default_value_t = 16.0)]
    audio_gain: f32,

    /// P25 system ID.
    #[arg(long)]
    system_id: Option<u32>,

    /// Wide Area Communications Network ID.
    #[arg(long)]
    wacn: Option<u32>,

    /// P25 site identifier.
    #[arg(long)]
    site_id: Option<u32>,

    /// RadioReference.com database ID.
    #[arg(long)]
    radioreference_id: Option<u32>,

    /// Trunker system unique identifier (auto-generated if omitted).
    #[arg(long)]
    trunker_system_id: Option<Uuid>,

    /// Trunker feeder unique identifier (auto-generated if omitted).
    #[arg(long)]
    trunker_feeder_id: Option<Uuid>,
}

/// System identification metadata embedded in LiveKit participant info.
#[derive(Debug, Serialize)]
struct SystemMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    system_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    wacn: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    site_id: Option<u32>,
    /// RadioReference.com database identifier for this system.
    #[serde(skip_serializing_if = "Option::is_none")]
    radioreference_id: Option<u32>,
    trunker_system_id: Uuid,
    trunker_feeder_id: Uuid,
}

impl SystemMetadata {
    /// Convert to key-value pairs for LiveKit participant attributes.
    ///
    /// Skips `None` values so only populated fields appear in attributes.
    fn to_attributes(&self) -> HashMap<String, String> {
        // Serialize to a JSON Value and flatten into string key-value pairs,
        // so the attribute map stays in sync with the struct fields.
        let value = serde_json::to_value(self).expect("SystemMetadata is always serializable");
        value
            .as_object()
            .expect("SystemMetadata serializes as object")
            .iter()
            .map(|(k, v)| {
                let s = match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                (k.clone(), s)
            })
            .collect()
    }
}

#[tokio::main(worker_threads = 2)]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    // Build system identification metadata.
    let metadata = SystemMetadata {
        system_id: cli.system_id,
        wacn: cli.wacn,
        site_id: cli.site_id,
        radioreference_id: cli.radioreference_id,
        trunker_system_id: cli.trunker_system_id.unwrap_or_else(Uuid::new_v4),
        trunker_feeder_id: cli.trunker_feeder_id.unwrap_or_else(Uuid::new_v4),
    };
    let metadata_json = serde_json::to_string(&metadata)?;

    tracing::info!(
        trunker_system_id = %metadata.trunker_system_id,
        trunker_feeder_id = %metadata.trunker_feeder_id,
        system_id = ?metadata.system_id,
        wacn = ?metadata.wacn,
        site_id = ?metadata.site_id,
        "system metadata"
    );

    // Derive LiveKit room/identity defaults from system metadata.
    let livekit_room = cli
        .livekit_room
        .unwrap_or_else(|| format!("trunker:{}", metadata.trunker_system_id));
    let livekit_identity = cli
        .livekit_identity
        .unwrap_or_else(|| format!("feeder:{}", metadata.trunker_feeder_id));

    let running = Arc::new(AtomicBool::new(true));

    // Parse device settings.
    let settings: Vec<(String, String)> = cli
        .settings
        .iter()
        .map(|s| {
            let (key, value) = s
                .split_once('=')
                .ok_or_else(|| anyhow::anyhow!("invalid setting '{s}': expected key=value"))?;
            Ok((key.to_string(), value.to_string()))
        })
        .collect::<Result<Vec<_>>>()?;

    tracing::info!(
        room = livekit_room,
        identity = livekit_identity,
        "connecting to LiveKit server"
    );

    // Generate access token for LiveKit.
    let token = livekit_api::access_token::AccessToken::with_api_key(
        &cli.livekit_api_key,
        &cli.livekit_api_secret,
    )
    .with_identity(&livekit_identity)
    .with_metadata(&metadata_json)
    .with_grants(livekit_api::access_token::VideoGrants {
        room_join: true,
        room: livekit_room,
        can_publish: true,
        can_subscribe: false,
        ..Default::default()
    })
    .to_jwt()?;

    // Connect to LiveKit room.
    let mut room_options = RoomOptions::default();
    room_options.auto_subscribe = false;
    room_options.single_peer_connection = true;
    let (room, mut room_events) =
        Room::connect(&cli.livekit_url, &token, room_options).await?;
    let room = Arc::new(room);

    tracing::info!(room_name = room.name(), "connected to LiveKit room");

    // Publish system metadata on the local participant so subscribers
    // (and the future central server) can identify this feeder.
    room.local_participant()
        .set_metadata(metadata_json)
        .await?;
    room.local_participant()
        .set_attributes(metadata.to_attributes())
        .await?;

    // Install signal handler after LiveKit connects to avoid
    // interfering with libwebrtc's internal signal handling.
    let running_clone = running.clone();
    ctrlc::set_handler(move || {
        running_clone.store(false, Ordering::Relaxed);
    })?;

    // Set up the broadcast bridge.
    let mut sink = BroadcastSink::new(4096);
    let audio_rx = sink.subscribe();
    let data_rx = sink.subscribe();

    // Spawn LiveKit audio publisher.
    let publisher_room = Arc::clone(&room);
    let audio_gain = cli.audio_gain;
    let audio_handle = tokio::spawn(async move {
        let mut publisher = AudioPublisher::new(publisher_room, audio_gain);
        if let Err(e) = publisher.run(audio_rx).await {
            tracing::error!(error = %e, "audio publisher error");
        }
    });

    // Spawn LiveKit data channel publisher.
    let data_room = Arc::clone(&room);
    let data_handle = tokio::spawn(async move {
        let publisher = DataPublisher::new(data_room);
        publisher.run(data_rx).await;
    });

    // Spawn room event handler: log connection state changes and signal
    // shutdown on permanent disconnect.
    let room_running = running.clone();
    tokio::spawn(async move {
        while let Some(event) = room_events.recv().await {
            match event {
                RoomEvent::Reconnecting => {
                    tracing::warn!("LiveKit connection lost, reconnecting...");
                }
                RoomEvent::Reconnected => {
                    tracing::info!("LiveKit reconnected");
                }
                RoomEvent::Disconnected { reason } => {
                    tracing::error!(?reason, "LiveKit disconnected, shutting down");
                    room_running.store(false, Ordering::Relaxed);
                    break;
                }
                _ => {
                    tracing::debug!(?event, "LiveKit room event");
                }
            }
        }
    });

    // Compute CC offset.
    let cc_offset_hz = if cli.frequency != 0 {
        cli.frequency as f64 - cli.center_freq as f64
    } else {
        0.0
    };

    let max_voices = if cli.max_voices == 0 {
        None
    } else {
        Some(cli.max_voices)
    };

    // Open SDR device just before starting the decode thread so the reader
    // thread doesn't overflow while we wait for LiveKit to connect.
    let gain = if cli.auto_gain { None } else { cli.gain };
    let sample_source = SoapySource::open(
        &cli.device,
        cli.center_freq,
        cli.sample_rate,
        gain,
        cli.antenna.as_deref(),
        &settings,
        cli.buffer_ms,
        running.clone(),
    )?;
    let mut source = SampleSource::Soapy(sample_source);

    // Run decode on a dedicated OS thread so it gets consistent scheduling
    // and isn't throttled by Tokio's blocking thread pool.
    let decode_running = running.clone();
    let decode_handle = std::thread::spawn(move || {
        let config = trunker::decode::TrunkedDecoderConfig {
            sample_rate: cli.sample_rate,
            center_frequency: cli.center_freq,
            cc_offset_hz,
            modulation: pipeline::Modulation::Cqpsk,
            nid_integrity: NidIntegrityPolicy::Strict,
            call_timeout: cli.call_timeout,
            decode_audio: true,
            output_dir: None,
            audio_format: trunker::output::call_writer::AudioFormat::Wav,
            max_voices,
            json_output: false,
            heartbeat_seconds: 10,
        };
        trunker::decode::trunked::decode_trunked(
            &mut source,
            &config,
            &decode_running,
            Some(&mut sink),
        )
    });

    // Wait for decode thread without blocking the Tokio runtime.
    let decode_result = tokio::task::spawn_blocking(|| decode_handle.join()).await;
    match decode_result {
        Ok(Ok(Ok(()))) => tracing::info!("decode thread finished"),
        Ok(Ok(Err(e))) => tracing::error!(error = %e, "decode thread error"),
        Ok(Err(_)) => tracing::error!("decode thread panicked"),
        Err(e) => tracing::error!(error = %e, "failed to join decode thread"),
    }

    // Clean up.
    audio_handle.abort();
    data_handle.abort();
    let _ = room.close().await;

    tracing::info!("server shutdown complete");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    const SYSTEM_UUID: &str = "00000000-0000-0000-0000-000000000001";
    const FEEDER_UUID: &str = "00000000-0000-0000-0000-000000000002";

    fn metadata_all_fields() -> SystemMetadata {
        SystemMetadata {
            system_id: Some(42),
            wacn: Some(0xBEEF),
            site_id: Some(7),
            radioreference_id: Some(12345),
            trunker_system_id: Uuid::parse_str(SYSTEM_UUID).unwrap(),
            trunker_feeder_id: Uuid::parse_str(FEEDER_UUID).unwrap(),
        }
    }

    fn metadata_no_optional() -> SystemMetadata {
        SystemMetadata {
            system_id: None,
            wacn: None,
            site_id: None,
            radioreference_id: None,
            trunker_system_id: Uuid::parse_str(SYSTEM_UUID).unwrap(),
            trunker_feeder_id: Uuid::parse_str(FEEDER_UUID).unwrap(),
        }
    }

    #[test]
    fn test_json_serialization_all_fields() {
        let meta = metadata_all_fields();
        let json = serde_json::to_string(&meta).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["system_id"], 42);
        assert_eq!(parsed["wacn"], 0xBEEF_u32);
        assert_eq!(parsed["site_id"], 7);
        assert_eq!(parsed["radioreference_id"], 12345);
        assert_eq!(parsed["trunker_system_id"], SYSTEM_UUID);
        assert_eq!(parsed["trunker_feeder_id"], FEEDER_UUID);
    }

    #[test]
    fn test_json_serialization_optional_fields_skipped() {
        let meta = metadata_no_optional();
        let json = serde_json::to_string(&meta).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert!(parsed.get("system_id").is_none(), "system_id should be absent");
        assert!(parsed.get("wacn").is_none(), "wacn should be absent");
        assert!(parsed.get("site_id").is_none(), "site_id should be absent");
        assert!(
            parsed.get("radioreference_id").is_none(),
            "radioreference_id should be absent"
        );
        // UUID fields must still appear.
        assert_eq!(parsed["trunker_system_id"], SYSTEM_UUID);
        assert_eq!(parsed["trunker_feeder_id"], FEEDER_UUID);
    }

    #[test]
    fn test_to_attributes_all_fields() {
        let meta = metadata_all_fields();
        let attrs = meta.to_attributes();

        assert_eq!(attrs.get("system_id").map(String::as_str), Some("42"));
        assert_eq!(attrs.get("wacn").map(String::as_str), Some("48879"));
        assert_eq!(attrs.get("site_id").map(String::as_str), Some("7"));
        assert_eq!(
            attrs.get("radioreference_id").map(String::as_str),
            Some("12345")
        );
        assert_eq!(
            attrs.get("trunker_system_id").map(String::as_str),
            Some(SYSTEM_UUID)
        );
        assert_eq!(
            attrs.get("trunker_feeder_id").map(String::as_str),
            Some(FEEDER_UUID)
        );
    }

    #[test]
    fn test_to_attributes_none_fields_skipped() {
        let meta = metadata_no_optional();
        let attrs = meta.to_attributes();

        assert!(!attrs.contains_key("system_id"), "system_id should be absent");
        assert!(!attrs.contains_key("wacn"), "wacn should be absent");
        assert!(!attrs.contains_key("site_id"), "site_id should be absent");
        assert!(
            !attrs.contains_key("radioreference_id"),
            "radioreference_id should be absent"
        );
    }

    #[test]
    fn test_to_attributes_uuid_fields_always_present() {
        let meta = metadata_no_optional();
        let attrs = meta.to_attributes();

        assert!(
            attrs.contains_key("trunker_system_id"),
            "trunker_system_id must always be present"
        );
        assert!(
            attrs.contains_key("trunker_feeder_id"),
            "trunker_feeder_id must always be present"
        );
    }
}

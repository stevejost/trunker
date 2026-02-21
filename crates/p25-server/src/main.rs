//! P25 LiveKit WebRTC server.
//!
//! Decodes a P25 trunked radio system and publishes audio via LiveKit
//! WebRTC, with JSON metadata on a data channel.
//!
//! **Status: skeleton.** The LiveKit connection and audio publisher are
//! functional, but the `BroadcastSink` is not yet wired into the decode
//! loop. `decode_trunked()` does not accept an `EventSink` parameter,
//! so the audio publisher will never receive events until that is added.
//!
//! Architecture (target):
//! - A sync decode thread runs the trunked decoder, producing events
//! - Events are broadcast to async consumers via `BroadcastSink`
//! - An audio publisher creates one LiveKit track per talkgroup
//! - A data channel publishes JSON metadata (grants, heartbeats)

mod bridge;
mod livekit_publisher;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use clap::Parser;
use livekit::prelude::*;

use bridge::BroadcastSink;
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

    /// LiveKit room name to join.
    #[arg(long, default_value = "p25")]
    livekit_room: String,

    /// Participant identity in the LiveKit room.
    #[arg(long, default_value = "p25-decoder")]
    livekit_identity: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    // Signal handler for graceful shutdown.
    let running = Arc::new(AtomicBool::new(true));
    let running_clone = running.clone();
    ctrlc::set_handler(move || {
        running_clone.store(false, Ordering::Relaxed);
    })?;

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

    // Open SDR device.
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

    // Generate access token for LiveKit.
    let token = livekit_api::access_token::AccessToken::with_api_key(
        &cli.livekit_api_key,
        &cli.livekit_api_secret,
    )
    .with_identity(&cli.livekit_identity)
    .with_grants(livekit_api::access_token::VideoGrants {
        room_join: true,
        room: cli.livekit_room.clone(),
        can_publish: true,
        can_subscribe: false,
        ..Default::default()
    })
    .to_jwt()?;

    tracing::info!(
        room = cli.livekit_room,
        identity = cli.livekit_identity,
        "connecting to LiveKit server"
    );

    // Connect to LiveKit room.
    let (room, mut room_events) =
        Room::connect(&cli.livekit_url, &token, RoomOptions::default()).await?;

    tracing::info!(room_name = room.name(), "connected to LiveKit room");

    // Set up the broadcast bridge.
    // TODO: Pass `sink` to the decode loop once `decode_trunked` accepts
    // an `EventSink` parameter. Currently the sink receives no events.
    let sink = BroadcastSink::new(4096);
    let audio_rx = sink.subscribe();

    // Spawn LiveKit audio publisher.
    let room_clone = room.clone();
    let audio_handle = tokio::spawn(async move {
        let mut publisher = AudioPublisher::new(room_clone);
        if let Err(e) = publisher.run(audio_rx).await {
            tracing::error!(error = %e, "audio publisher error");
        }
    });

    // Spawn room event logger.
    tokio::spawn(async move {
        while let Some(event) = room_events.recv().await {
            tracing::debug!(?event, "LiveKit room event");
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

    // Run decode in a blocking thread so it doesn't block the Tokio runtime.
    // TODO: Wire `sink` into this call so decoded events reach the
    // audio publisher. Requires `decode_trunked` to accept an EventSink.
    let decode_running = running.clone();
    let decode_handle = tokio::task::spawn_blocking(move || {
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
        trunker::decode::trunked::decode_trunked(&mut source, &config, &decode_running)
    });

    // Wait for decode thread to finish.
    match decode_handle.await {
        Ok(Ok(())) => tracing::info!("decode thread finished"),
        Ok(Err(e)) => tracing::error!(error = %e, "decode thread error"),
        Err(e) => tracing::error!(error = %e, "decode thread panicked"),
    }

    // Clean up.
    audio_handle.abort();
    room.close().await;

    tracing::info!("server shutdown complete");
    Ok(())
}

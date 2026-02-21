//! Publish a 440Hz sine wave to LiveKit to test the audio pipeline.

use std::sync::Arc;

use anyhow::Result;
use livekit::options::TrackPublishOptions;
use livekit::prelude::*;
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::AudioSourceOptions;
use livekit::webrtc::audio_source::native::NativeAudioSource;

const SAMPLE_RATE: u32 = 48000;
const CHANNELS: u32 = 1;
const FRAME_DURATION_MS: u32 = 20;
const SAMPLES_PER_FRAME: u32 = SAMPLE_RATE * FRAME_DURATION_MS / 1000; // 960

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_writer(std::io::stderr)
        .init();

    let url = std::env::var("LIVEKIT_URL").unwrap_or_else(|_| "ws://127.0.0.1:7880".into());
    let api_key = std::env::var("LIVEKIT_API_KEY").unwrap_or_else(|_| "devkey".into());
    let api_secret =
        std::env::var("LIVEKIT_API_SECRET").unwrap_or_else(|_| "secret_that_you_should_change".into());

    let token = livekit_api::access_token::AccessToken::with_api_key(&api_key, &api_secret)
        .with_identity("sine-test")
        .with_grants(livekit_api::access_token::VideoGrants {
            room_join: true,
            room: "p25".to_string(),
            can_publish: true,
            can_subscribe: false,
            ..Default::default()
        })
        .to_jwt()?;

    let mut opts = RoomOptions::default();
    opts.auto_subscribe = false;
    opts.single_peer_connection = true;

    eprintln!("connecting...");
    let (room, _rx) = Room::connect(&url, &token, opts).await?;
    let room = Arc::new(room);
    eprintln!("connected! publishing 440Hz sine wave...");

    let source = NativeAudioSource::new(
        AudioSourceOptions {
            echo_cancellation: false,
            noise_suppression: false,
            auto_gain_control: false,
        },
        SAMPLE_RATE,
        CHANNELS,
        1000,
    );

    let track = LocalAudioTrack::create_audio_track(
        "sine-440",
        livekit::webrtc::audio_source::RtcAudioSource::Native(source.clone()),
    );

    room.local_participant()
        .publish_track(
            LocalTrack::Audio(track),
            TrackPublishOptions {
                source: TrackSource::Unknown,
                ..Default::default()
            },
        )
        .await?;

    eprintln!("track published. you should hear a tone. Ctrl+C to stop.");

    // Generate and send 440Hz sine wave continuously.
    let mut sample_idx: u64 = 0;
    let amplitude: f32 = 0.3; // -10 dB, moderate volume
    loop {
        let mut pcm = Vec::with_capacity(SAMPLES_PER_FRAME as usize);
        for _ in 0..SAMPLES_PER_FRAME {
            let t = sample_idx as f32 / SAMPLE_RATE as f32;
            let sample = amplitude * (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            pcm.push((sample * i16::MAX as f32) as i16);
            sample_idx += 1;
        }

        let frame = AudioFrame {
            data: pcm.into(),
            sample_rate: SAMPLE_RATE,
            num_channels: CHANNELS,
            samples_per_channel: SAMPLES_PER_FRAME,
        };

        source.capture_frame(&frame).await?;

        // Pace at real-time: 20ms per frame.
        tokio::time::sleep(std::time::Duration::from_millis(FRAME_DURATION_MS as u64)).await;
    }
}

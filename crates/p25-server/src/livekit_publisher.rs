//! LiveKit audio publishing for P25 voice channels.
//!
//! Manages one [`LocalAudioTrack`] per active talkgroup, publishing
//! decoded PCM audio frames as they arrive from the decode bridge.
//!
//! Each talkgroup gets its own track so that LiveKit clients can
//! subscribe to individual channels.
//!
//! **Status: skeleton.** The publisher is functional but will never
//! receive events until the `BroadcastSink` is wired into the decode
//! loop (see `bridge.rs` and `main.rs` TODOs).

use std::collections::HashMap;

use livekit::prelude::*;
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::AudioSourceOptions;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use tokio::sync::broadcast;

use crate::bridge::BroadcastEvent;

/// Sample rate for P25 IMBE decoded audio.
const AUDIO_SAMPLE_RATE: u32 = 8000;

/// Number of audio channels (mono).
const AUDIO_CHANNELS: u32 = 1;

/// State for a single published talkgroup audio track.
struct TalkgroupTrack {
    /// The LiveKit audio source that receives PCM frames.
    source: NativeAudioSource,
    /// The published track handle (kept alive to maintain publication).
    _publication: LocalTrackPublication,
}

/// Manages LiveKit audio tracks for all active talkgroups.
pub struct AudioPublisher {
    /// Room reference for publishing new tracks.
    room: Room,
    /// Active tracks keyed by talkgroup ID.
    tracks: HashMap<u16, TalkgroupTrack>,
}

impl AudioPublisher {
    /// Create a new publisher attached to the given room.
    pub fn new(room: Room) -> Self {
        Self {
            room,
            tracks: HashMap::new(),
        }
    }

    /// Run the publisher loop, consuming events from the broadcast channel.
    ///
    /// Blocks until the broadcast channel is closed or a fatal error occurs.
    pub async fn run(&mut self, mut rx: broadcast::Receiver<BroadcastEvent>) -> anyhow::Result<()> {
        loop {
            match rx.recv().await {
                Ok(BroadcastEvent::Audio {
                    talkgroup, samples, ..
                }) => {
                    if let Err(e) = self.publish_audio(talkgroup, &samples).await {
                        tracing::warn!(
                            talkgroup,
                            error = %e,
                            "failed to publish audio frame"
                        );
                    }
                }
                Ok(_) => {
                    // Metadata and other events handled elsewhere.
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "audio publisher lagged behind");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    tracing::info!("broadcast channel closed, audio publisher shutting down");
                    break;
                }
            }
        }
        Ok(())
    }

    /// Publish a single audio frame for a talkgroup.
    ///
    /// Creates the track on first call for a given talkgroup.
    async fn publish_audio(&mut self, talkgroup: u16, samples: &[f32]) -> anyhow::Result<()> {
        let track = self.get_or_create_track(talkgroup).await?;

        // Convert f32 samples to i16 for LiveKit.
        let pcm_i16: Vec<i16> = samples
            .iter()
            .map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .collect();

        let frame = AudioFrame {
            data: pcm_i16.into(),
            sample_rate: AUDIO_SAMPLE_RATE,
            num_channels: AUDIO_CHANNELS,
            samples_per_channel: samples.len() as u32,
        };

        track.source.capture_frame(&frame).await?;
        Ok(())
    }

    /// Get or create a LiveKit track for a talkgroup.
    async fn get_or_create_track(&mut self, talkgroup: u16) -> anyhow::Result<&mut TalkgroupTrack> {
        if !self.tracks.contains_key(&talkgroup) {
            let source = NativeAudioSource::new(
                AudioSourceOptions {
                    echo_cancellation: false,
                    noise_suppression: false,
                    auto_gain_control: false,
                },
                AUDIO_SAMPLE_RATE,
                AUDIO_CHANNELS,
                1000, // 1 second buffer
            );

            let track_name = format!("tg-{talkgroup}");
            let local_track = LocalAudioTrack::create_audio_track(
                &track_name,
                livekit::webrtc::audio_source::RtcAudioSource::Native(source.clone()),
            );

            let publication = self
                .room
                .local_participant()
                .publish_track(
                    LocalTrack::Audio(local_track),
                    TrackPublishOptions {
                        source: TrackSource::Unknown,
                        ..Default::default()
                    },
                )
                .await?;

            tracing::info!(talkgroup, track_name, "published audio track");

            self.tracks.insert(
                talkgroup,
                TalkgroupTrack {
                    source,
                    _publication: publication,
                },
            );
        }

        Ok(self.tracks.get_mut(&talkgroup).unwrap())
    }
}

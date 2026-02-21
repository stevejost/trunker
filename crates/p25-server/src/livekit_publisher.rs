//! LiveKit audio publishing for P25 voice channels.
//!
//! Manages one [`LocalAudioTrack`] per active talkgroup, publishing
//! decoded PCM audio frames as they arrive from the decode bridge.
//!
//! Each talkgroup gets its own track so that LiveKit clients can
//! subscribe to individual channels.

use std::collections::HashMap;
use std::sync::Arc;

use livekit::options::TrackPublishOptions;
use livekit::prelude::*;
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::AudioSourceOptions;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use tokio::sync::broadcast;
use tokio::time::{Duration, Instant};

use crate::bridge::BroadcastEvent;

/// Output sample rate for LiveKit (WebRTC standard).
const LIVEKIT_SAMPLE_RATE: u32 = 48000;

/// Upsample ratio (48000 / 8000).
const UPSAMPLE_RATIO: usize = 6;

/// Number of audio channels (mono).
const AUDIO_CHANNELS: u32 = 1;

/// Convert a raw vocoder f32 sample (after gain) to i16.
///
/// Values outside the i16 range are clamped.
fn vocoder_sample_to_i16(sample: f32) -> i16 {
    sample.clamp(i16::MIN as f32, i16::MAX as f32) as i16
}

/// Duration of one IMBE voice frame (160 samples at 8 kHz).
const FRAME_DURATION: Duration = Duration::from_millis(20);

/// State for a single published talkgroup audio track.
struct TalkgroupTrack {
    /// The LiveKit audio source that receives PCM frames.
    source: NativeAudioSource,
    /// The published track handle (kept alive to maintain publication).
    _publication: LocalTrackPublication,
    /// Earliest time the next frame should be sent (real-time pacing).
    next_send: Instant,
    /// Last sample from previous frame (pre-gain), for smooth interpolation
    /// across frame boundaries.
    last_sample: f32,
}

/// Manages LiveKit audio tracks for all active talkgroups.
pub struct AudioPublisher {
    /// Room reference for publishing new tracks.
    room: Arc<Room>,
    /// Active tracks keyed by talkgroup ID.
    tracks: HashMap<u16, TalkgroupTrack>,
    /// Gain multiplier applied to vocoder samples before publishing.
    gain: f32,
}

impl AudioPublisher {
    /// Create a new publisher attached to the given room.
    pub fn new(room: Arc<Room>, gain: f32) -> Self {
        Self {
            room,
            tracks: HashMap::new(),
            gain,
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
        let gain = self.gain;
        let track = self.get_or_create_track(talkgroup).await?;

        // Log peak level for debugging.
        let peak = samples.iter().copied().map(f32::abs).fold(0.0f32, f32::max);
        tracing::debug!(talkgroup, peak, "audio frame");

        // Pace frames at real-time to prevent buffer overrun.
        let now = Instant::now();
        if now < track.next_send {
            tokio::time::sleep_until(track.next_send).await;
        }
        track.next_send = Instant::now() + FRAME_DURATION;

        // Upsample 8kHz → 48kHz via linear interpolation, convert to i16.
        // Uses last_sample from the previous frame for a smooth transition
        // at frame boundaries (avoids click/crackle between frames).
        let upsampled_len = samples.len() * UPSAMPLE_RATIO;
        let mut pcm_i16 = Vec::with_capacity(upsampled_len);
        let mut prev = track.last_sample * gain;
        for &sample in samples {
            let current = sample * gain;
            for j in 0..UPSAMPLE_RATIO {
                let t = j as f32 / UPSAMPLE_RATIO as f32;
                let interpolated = prev + (current - prev) * t;
                pcm_i16.push(vocoder_sample_to_i16(interpolated));
            }
            prev = current;
        }
        track.last_sample = *samples.last().unwrap_or(&0.0);

        let frame = AudioFrame {
            data: pcm_i16.into(),
            sample_rate: LIVEKIT_SAMPLE_RATE,
            num_channels: AUDIO_CHANNELS,
            samples_per_channel: upsampled_len as u32,
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
                LIVEKIT_SAMPLE_RATE,
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
                    next_send: Instant::now(),
                    last_sample: 0.0,
                },
            );
        }

        Ok(self.tracks.get_mut(&talkgroup).unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vocoder_sample_zero() {
        assert_eq!(vocoder_sample_to_i16(0.0), 0);
    }

    #[test]
    fn vocoder_sample_typical_peak() {
        // Vocoder typically peaks around ~1000.
        assert_eq!(vocoder_sample_to_i16(1000.0), 1000);
        assert_eq!(vocoder_sample_to_i16(-1000.0), -1000);
    }

    #[test]
    fn vocoder_sample_clamps_at_i16_bounds() {
        assert_eq!(vocoder_sample_to_i16(40000.0), i16::MAX);
        assert_eq!(vocoder_sample_to_i16(-40000.0), i16::MIN);
    }

    #[test]
    fn vocoder_sample_small_values() {
        assert_eq!(vocoder_sample_to_i16(1.5), 1);
        assert_eq!(vocoder_sample_to_i16(-1.5), -1);
    }
}

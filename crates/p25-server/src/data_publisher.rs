//! LiveKit data channel publisher for P25 metadata.
//!
//! Consumes [`BroadcastEvent`] variants and publishes JSON messages
//! over LiveKit's data channel so web clients can display real-time
//! call activity (start/end) and heartbeats.
//!
//! Call boundaries are detected by audio frame timing: a call starts
//! on the first audio frame for a talkgroup and ends when no frames
//! arrive for [`CALL_TIMEOUT`].

use std::collections::HashMap;
use std::sync::Arc;

use livekit::prelude::*;
use serde::Serialize;
use tokio::sync::broadcast;
use tokio::time::{Duration, Instant};

use crate::bridge::BroadcastEvent;

/// How long after the last audio frame before we consider a call ended.
const CALL_TIMEOUT: Duration = Duration::from_secs(2);

/// How often we check for timed-out calls.
const SWEEP_INTERVAL: Duration = Duration::from_millis(500);

/// JSON message published on the data channel.
#[derive(Serialize)]
struct DataMessage<'a> {
    #[serde(rename = "e")]
    event: &'a str,
    #[serde(flatten)]
    payload: Payload,
}

#[derive(Serialize)]
#[serde(untagged)]
enum Payload {
    CallStart {
        #[serde(rename = "tg")]
        talkgroup: u16,
        #[serde(rename = "src")]
        source: u32,
        #[serde(rename = "freq")]
        frequency: u64,
    },
    CallEnd {
        #[serde(rename = "tg")]
        talkgroup: u16,
    },
    Heartbeat {
        #[serde(rename = "up")]
        uptime_seconds: u64,
        #[serde(rename = "tsbks")]
        tsbk_count: u64,
        #[serde(rename = "voices")]
        active_voice_channels: u32,
        cc_sync: bool,
    },
}

/// Per-talkgroup call state.
struct CallState {
    /// When we last received an audio frame.
    last_audio: Instant,
}

/// Publishes decode metadata to the LiveKit data channel.
pub struct DataPublisher {
    room: Arc<Room>,
}

impl DataPublisher {
    pub fn new(room: Arc<Room>) -> Self {
        Self { room }
    }

    pub async fn run(&self, mut rx: broadcast::Receiver<BroadcastEvent>) {
        // Talkgroups with an active call (have received audio recently).
        let mut active: HashMap<u16, CallState> = HashMap::new();
        let mut sweep_timer = tokio::time::interval(SWEEP_INTERVAL);

        loop {
            tokio::select! {
                result = rx.recv() => {
                    match result {
                        Ok(event) => self.handle_event(event, &mut active).await,
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(skipped = n, "data publisher lagged behind");
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            tracing::info!("broadcast channel closed, data publisher shutting down");
                            break;
                        }
                    }
                }
                _ = sweep_timer.tick() => {
                    self.sweep_expired(&mut active).await;
                }
            }
        }
    }

    async fn handle_event(&self, event: BroadcastEvent, active: &mut HashMap<u16, CallState>) {
        match event {
            BroadcastEvent::Audio {
                talkgroup,
                source,
                frequency,
                ..
            } => {
                let now = Instant::now();
                let is_new = !active.contains_key(&talkgroup);

                active.insert(talkgroup, CallState { last_audio: now });

                if is_new {
                    self.publish(&DataMessage {
                        event: "call_start",
                        payload: Payload::CallStart {
                            talkgroup,
                            source,
                            frequency,
                        },
                    })
                    .await;
                }
            }
            BroadcastEvent::Heartbeat {
                uptime_seconds,
                tsbk_count,
                active_voice_channels,
                cc_sync,
                ..
            } => {
                self.publish(&DataMessage {
                    event: "heartbeat",
                    payload: Payload::Heartbeat {
                        uptime_seconds,
                        tsbk_count,
                        active_voice_channels,
                        cc_sync,
                    },
                })
                .await;
            }
            BroadcastEvent::CcSync => {}
        }
    }

    /// Check for talkgroups that haven't received audio recently and emit call_end.
    async fn sweep_expired(&self, active: &mut HashMap<u16, CallState>) {
        let now = Instant::now();
        let expired: Vec<u16> = active
            .iter()
            .filter(|(_, state)| now.duration_since(state.last_audio) > CALL_TIMEOUT)
            .map(|(&tg, _)| tg)
            .collect();

        for tg in expired {
            active.remove(&tg);
            self.publish(&DataMessage {
                event: "call_end",
                payload: Payload::CallEnd { talkgroup: tg },
            })
            .await;
        }
    }

    async fn publish(&self, msg: &DataMessage<'_>) {
        let Ok(json) = serde_json::to_string(msg) else {
            return;
        };

        let packet = DataPacket {
            payload: json.into_bytes(),
            topic: Some("p25".to_string()),
            reliable: true,
            destination_identities: vec![],
        };

        if let Err(e) = self.room.local_participant().publish_data(packet).await {
            tracing::warn!(error = %e, "failed to publish data message");
        }
    }
}

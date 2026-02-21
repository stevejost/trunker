//! Bridge between the synchronous decode thread and async Tokio tasks.
//!
//! [`BroadcastSink`] implements [`EventSink`] and forwards events to a
//! `tokio::sync::broadcast` channel, allowing multiple async consumers
//! (LiveKit publisher, data channel, metrics) to receive decode events.

use tokio::sync::broadcast;
use trunker::decode::event::{DecoderEvent, EventSink};

/// Wrapper around [`DecoderEvent`] that is `Clone` for broadcast.
///
/// `DecoderEvent` contains types that are not all `Clone`, so we
/// extract the relevant fields into cloneable variants.
#[derive(Clone, Debug)]
pub enum BroadcastEvent {
    /// A voice frame with decoded PCM audio samples.
    Audio {
        /// Receive frequency in Hz.
        frequency: u64,
        /// Talkgroup ID.
        talkgroup: u16,
        /// Source unit ID.
        source: u32,
        /// PCM audio samples (8 kHz, f32, 160 samples per frame).
        samples: Vec<f32>,
    },

    /// A text-formatted metadata event (TSBK, heartbeat, etc.).
    Metadata(String),

    /// The CC pipeline acquired frame sync.
    CcSync,

    /// A heartbeat event with health metrics.
    Heartbeat {
        /// Decoder uptime in seconds.
        uptime_seconds: u64,
        /// Total TSBKs decoded.
        tsbk_count: u64,
        /// TSBK decode rate (per second).
        tsbk_rate: f64,
        /// Active voice channel count.
        active_voice_channels: u32,
        /// Whether CC has frame sync.
        cc_sync: bool,
    },
}

/// An [`EventSink`] that broadcasts events to async consumers.
///
/// Created on the main thread, handed to the sync decode loop.
/// The broadcast sender is cloned by async tasks via [`subscribe`].
pub struct BroadcastSink {
    sender: broadcast::Sender<BroadcastEvent>,
}

impl BroadcastSink {
    /// Create a new broadcast sink with the given channel capacity.
    ///
    /// Capacity should be large enough to absorb bursts without
    /// dropping events. 1024 is a reasonable default for real-time
    /// audio at ~50 voice frames/second per channel.
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Subscribe to the broadcast channel.
    ///
    /// Returns a receiver that will get all events sent after this call.
    /// Lagging receivers will get a `RecvError::Lagged` and miss events.
    pub fn subscribe(&self) -> broadcast::Receiver<BroadcastEvent> {
        self.sender.subscribe()
    }
}

impl EventSink for BroadcastSink {
    fn handle(&mut self, event: DecoderEvent) -> bool {
        match event {
            DecoderEvent::CcSync => {
                let _ = self.sender.send(BroadcastEvent::CcSync);
            }
            DecoderEvent::Heartbeat {
                uptime_seconds,
                tsbk_count,
                tsbk_rate,
                active_voice_channels,
                cc_sync,
                ..
            } => {
                let _ = self.sender.send(BroadcastEvent::Heartbeat {
                    uptime_seconds,
                    tsbk_count,
                    tsbk_rate,
                    active_voice_channels,
                    cc_sync,
                });
            }
            DecoderEvent::Tsbk(ref tsbk) => {
                let text = format!("{tsbk:?}");
                let _ = self.sender.send(BroadcastEvent::Metadata(text));
            }
            // Voice frames, link control, etc. are handled by the
            // trunked decoder's voice event path, not through EventSink.
            // The server will tap into voice events separately.
            _ => {}
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broadcast_sink_creates_with_capacity() {
        let sink = BroadcastSink::new(128);
        let _rx = sink.subscribe();
    }

    #[test]
    fn broadcast_sink_handle_returns_true() {
        let mut sink = BroadcastSink::new(128);
        assert!(sink.handle(DecoderEvent::CcSync));
    }

    #[test]
    fn broadcast_sink_sends_cc_sync() {
        let mut sink = BroadcastSink::new(128);
        let mut rx = sink.subscribe();
        sink.handle(DecoderEvent::CcSync);
        let event = rx.try_recv().unwrap();
        assert!(matches!(event, BroadcastEvent::CcSync));
    }

    #[test]
    fn broadcast_sink_sends_heartbeat() {
        let mut sink = BroadcastSink::new(128);
        let mut rx = sink.subscribe();
        sink.handle(DecoderEvent::Heartbeat {
            timestamp: std::time::SystemTime::UNIX_EPOCH,
            uptime_seconds: 42,
            tsbk_count: 100,
            tsbk_rate: 2.5,
            active_voice_channels: 3,
            cc_sync: true,
        });
        let event = rx.try_recv().unwrap();
        match event {
            BroadcastEvent::Heartbeat {
                uptime_seconds,
                tsbk_count,
                ..
            } => {
                assert_eq!(uptime_seconds, 42);
                assert_eq!(tsbk_count, 100);
            }
            _ => panic!("expected Heartbeat"),
        }
    }

    #[test]
    fn broadcast_sink_no_receivers_does_not_panic() {
        let mut sink = BroadcastSink::new(128);
        // No subscribers -- send should succeed without panic.
        sink.handle(DecoderEvent::CcSync);
    }

    #[test]
    fn multiple_subscribers_receive_same_event() {
        let mut sink = BroadcastSink::new(128);
        let mut rx1 = sink.subscribe();
        let mut rx2 = sink.subscribe();
        sink.handle(DecoderEvent::CcSync);
        assert!(matches!(rx1.try_recv().unwrap(), BroadcastEvent::CcSync));
        assert!(matches!(rx2.try_recv().unwrap(), BroadcastEvent::CcSync));
    }
}

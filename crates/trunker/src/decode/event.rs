//! Decoder events and the sink trait for consuming them.
//!
//! [`DecoderEvent`] is the unified event type emitted by the decode
//! pipeline. Consumers implement [`EventSink`] to handle events
//! (JSON output, WebSocket broadcast, audio recording, etc.).

use std::time::SystemTime;

use crate::output::call_recorder::CompletedRecording;
use crate::p25::error::P25Error;
use crate::p25::nid::NetworkId;
use crate::p25::tsbk::Tsbk;
use crate::p25::voice::control::LinkControlFields;
use crate::p25::voice::crypto::CryptoControlFields;
use crate::p25::voice::frame::VoiceFrame;
use crate::p25::voice::header::VoiceHeaderFields;

/// A decoded event from the P25 pipeline.
///
/// Each variant carries the minimum data needed by downstream consumers.
/// Events are produced by the decode loop and dispatched to one or more
/// [`EventSink`] implementations.
#[derive(Debug)]
pub enum DecoderEvent {
    /// A TSBK was successfully decoded and CRC-verified.
    Tsbk(Tsbk),

    /// The control channel pipeline acquired frame sync.
    CcSync,

    /// The identifier table was updated with new channel parameters.
    IdentTableUpdated,

    /// A decoded IMBE voice frame.
    VoiceFrame(VoiceFrame),

    /// A decoded Link Control word (from LDU1).
    LinkControl(LinkControlFields),

    /// A decoded Crypto Control word (from LDU2).
    CryptoControl(CryptoControlFields),

    /// A decoded voice header (from HDU).
    VoiceHeader(VoiceHeaderFields),

    /// A 16-bit low-speed data fragment from a voice frame group.
    DataFragment(u16),

    /// A NID decoded on a voice channel pipeline.
    VoiceChannelNid(NetworkId),

    /// A call recording has been finalized.
    RecordingCompleted(CompletedRecording),

    /// A periodic heartbeat for health monitoring.
    Heartbeat {
        /// UTC timestamp for this heartbeat.
        timestamp: SystemTime,
        /// Decoder uptime in seconds.
        uptime_seconds: u64,
        /// Total TSBKs decoded since startup.
        tsbk_count: u64,
        /// TSBK decode rate (per second) over the last interval.
        tsbk_rate: f64,
        /// Number of active voice channel pipelines.
        active_voice_channels: u32,
        /// Whether the CC pipeline currently has frame sync.
        cc_sync: bool,
    },

    /// A decode error occurred (non-fatal).
    Error(P25Error),
}

/// Trait for consuming decoder events.
///
/// Implementors process events as they arrive from the decode pipeline.
/// The return value signals whether the pipeline should continue running.
pub trait EventSink {
    /// Handle a single decoder event.
    ///
    /// Returns `true` if the pipeline should continue, or `false` to
    /// request a graceful shutdown.
    fn handle(&mut self, event: DecoderEvent) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p25::tsbk::{TsbkHeader, TsbkOpcode, TsbkPayload};

    /// A test sink that collects events.
    struct CollectingSink {
        events: Vec<String>,
    }

    impl CollectingSink {
        fn new() -> Self {
            Self { events: Vec::new() }
        }
    }

    impl EventSink for CollectingSink {
        fn handle(&mut self, event: DecoderEvent) -> bool {
            self.events.push(format!("{event:?}"));
            true
        }
    }

    #[test]
    fn sink_receives_tsbk_event() {
        let mut sink = CollectingSink::new();
        let tsbk = Tsbk {
            header: TsbkHeader {
                opcode: TsbkOpcode::NetworkStatusBroadcast,
                last_block: true,
                protected: false,
                manufacturer_id: 0x00,
            },
            payload: TsbkPayload::Unknown { data: [0; 8] },
        };
        assert!(sink.handle(DecoderEvent::Tsbk(tsbk)));
        assert_eq!(sink.events.len(), 1);
    }

    #[test]
    fn sink_receives_cc_sync_event() {
        let mut sink = CollectingSink::new();
        assert!(sink.handle(DecoderEvent::CcSync));
        assert_eq!(sink.events.len(), 1);
    }

    #[test]
    fn sink_can_request_shutdown() {
        struct ShutdownSink;
        impl EventSink for ShutdownSink {
            fn handle(&mut self, _event: DecoderEvent) -> bool {
                false
            }
        }
        let mut sink = ShutdownSink;
        assert!(!sink.handle(DecoderEvent::CcSync));
    }

    #[test]
    fn heartbeat_event_carries_fields() {
        let event = DecoderEvent::Heartbeat {
            timestamp: SystemTime::UNIX_EPOCH,
            uptime_seconds: 42,
            tsbk_count: 100,
            tsbk_rate: 2.5,
            active_voice_channels: 3,
            cc_sync: true,
        };
        assert!(matches!(
            event,
            DecoderEvent::Heartbeat {
                tsbk_count: 100,
                ..
            }
        ));
    }

    #[test]
    fn error_event_wraps_p25_error() {
        let error = P25Error::NoFrameSync;
        let event = DecoderEvent::Error(error);
        assert!(matches!(event, DecoderEvent::Error(P25Error::NoFrameSync)));
    }
}

//! Periodic heartbeat state tracking for the decode pipeline.
//!
//! Counts samples processed and produces [`DecoderEvent::Heartbeat`]
//! at a configurable interval. The caller decides how to emit the event
//! (JSON line to stdout, WebSocket push, etc.).

use std::time::{Instant, SystemTime};

use super::event::DecoderEvent;

/// Default heartbeat interval for live SDR mode (seconds).
pub const DEFAULT_HEARTBEAT_INTERVAL_SECONDS: u64 = 10;

/// Tracks state for periodic heartbeat emission.
pub struct HeartbeatState {
    /// Wall-clock start time for uptime calculation.
    start: Instant,
    /// Samples processed since last heartbeat emission.
    samples_in_interval: u64,
    /// TSBK count at the last heartbeat emission.
    prev_tsbk_count: u64,
    /// Whether CC sync was observed during the current interval.
    cc_sync_seen: bool,
}

impl HeartbeatState {
    /// Create a new heartbeat tracker starting from now.
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            samples_in_interval: 0,
            prev_tsbk_count: 0,
            cc_sync_seen: false,
        }
    }

    /// Record a CC sync (NID event) during this interval.
    pub fn record_cc_sync(&mut self) {
        self.cc_sync_seen = true;
    }

    /// Check if a heartbeat should be produced after processing one sample.
    ///
    /// Call this once per sample. Returns `Some(DecoderEvent::Heartbeat)`
    /// when the interval has elapsed, `None` otherwise.
    ///
    /// - `heartbeat_samples` — number of samples per heartbeat interval
    ///   (sample_rate * heartbeat_seconds). Pass 0 to disable.
    /// - `heartbeat_seconds` — the interval in seconds (for rate calculation).
    /// - `tsbk_count` — current total TSBK count.
    /// - `active_voice_channels` — current active voice channel count.
    pub fn maybe_produce(
        &mut self,
        heartbeat_samples: u64,
        heartbeat_seconds: u64,
        tsbk_count: u64,
        active_voice_channels: u32,
    ) -> Option<DecoderEvent> {
        self.samples_in_interval += 1;
        if heartbeat_samples == 0 || self.samples_in_interval < heartbeat_samples {
            return None;
        }

        let uptime = self.start.elapsed().as_secs();
        let tsbk_delta = tsbk_count - self.prev_tsbk_count;
        let tsbk_rate = if heartbeat_seconds > 0 {
            tsbk_delta as f64 / heartbeat_seconds as f64
        } else {
            0.0
        };

        let event = DecoderEvent::Heartbeat {
            timestamp: SystemTime::now(),
            uptime_seconds: uptime,
            tsbk_count,
            tsbk_rate,
            active_voice_channels,
            cc_sync: self.cc_sync_seen,
        };

        self.prev_tsbk_count = tsbk_count;
        self.samples_in_interval = 0;
        self.cc_sync_seen = false;

        Some(event)
    }
}

impl Default for HeartbeatState {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve the effective heartbeat interval in seconds.
///
/// Returns 0 (disabled) when `no_heartbeat` is true or when running from
/// a file without an explicit interval. Returns the user's explicit value
/// if provided, or the default (10s) for live SDR mode.
pub fn resolve_heartbeat_interval(
    explicit_interval: Option<u64>,
    no_heartbeat: bool,
    is_sdr: bool,
) -> u64 {
    if no_heartbeat {
        return 0;
    }
    if let Some(interval) = explicit_interval {
        return interval;
    }
    if is_sdr {
        DEFAULT_HEARTBEAT_INTERVAL_SECONDS
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_emission_before_interval() {
        let mut state = HeartbeatState::new();
        for _ in 0..9 {
            assert!(state.maybe_produce(10, 1, 0, 0).is_none());
        }
    }

    #[test]
    fn emits_after_interval() {
        let mut state = HeartbeatState::new();
        for _ in 0..9 {
            state.maybe_produce(10, 1, 5, 0);
        }
        let event = state.maybe_produce(10, 1, 5, 0);
        assert!(matches!(event, Some(DecoderEvent::Heartbeat { .. })));
    }

    #[test]
    fn disabled_when_zero() {
        let mut state = HeartbeatState::new();
        for _ in 0..100 {
            assert!(state.maybe_produce(0, 0, 10, 0).is_none());
        }
    }

    #[test]
    fn cc_sync_tracked() {
        let mut state = HeartbeatState::new();
        state.record_cc_sync();
        for _ in 0..9 {
            state.maybe_produce(10, 1, 0, 0);
        }
        let event = state.maybe_produce(10, 1, 0, 0);
        match event {
            Some(DecoderEvent::Heartbeat { cc_sync, .. }) => assert!(cc_sync),
            _ => panic!("expected Heartbeat event"),
        }
    }

    #[test]
    fn cc_sync_resets_after_emission() {
        let mut state = HeartbeatState::new();
        state.record_cc_sync();
        // Emit first heartbeat.
        for _ in 0..10 {
            state.maybe_produce(10, 1, 0, 0);
        }
        // Second heartbeat without recording cc_sync.
        for _ in 0..9 {
            state.maybe_produce(10, 1, 0, 0);
        }
        let event = state.maybe_produce(10, 1, 0, 0);
        match event {
            Some(DecoderEvent::Heartbeat { cc_sync, .. }) => assert!(!cc_sync),
            _ => panic!("expected Heartbeat event"),
        }
    }

    #[test]
    fn tsbk_rate_calculation() {
        let mut state = HeartbeatState::new();
        for _ in 0..9 {
            state.maybe_produce(10, 5, 20, 0);
        }
        let event = state.maybe_produce(10, 5, 20, 0);
        match event {
            Some(DecoderEvent::Heartbeat {
                tsbk_count,
                tsbk_rate,
                ..
            }) => {
                assert_eq!(tsbk_count, 20);
                assert!((tsbk_rate - 4.0).abs() < 0.001);
            }
            _ => panic!("expected Heartbeat event"),
        }
    }

    #[test]
    fn resolve_sdr_default() {
        assert_eq!(resolve_heartbeat_interval(None, false, true), 10);
    }

    #[test]
    fn resolve_file_default() {
        assert_eq!(resolve_heartbeat_interval(None, false, false), 0);
    }

    #[test]
    fn resolve_explicit_overrides_default() {
        assert_eq!(resolve_heartbeat_interval(Some(30), false, true), 30);
        assert_eq!(resolve_heartbeat_interval(Some(5), false, false), 5);
    }

    #[test]
    fn resolve_no_heartbeat_disables() {
        assert_eq!(resolve_heartbeat_interval(None, true, true), 0);
        assert_eq!(resolve_heartbeat_interval(Some(10), true, true), 0);
        assert_eq!(resolve_heartbeat_interval(Some(5), true, false), 0);
    }

    #[test]
    fn resolve_zero_explicit_disables() {
        assert_eq!(resolve_heartbeat_interval(Some(0), false, true), 0);
    }
}

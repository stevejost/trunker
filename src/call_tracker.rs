//! Voice call lifecycle tracking.
//!
//! Layers on top of [`ChannelManager`] to represent logical "calls" with
//! start/end boundaries. Each call transitions through three states:
//!
//! - **Pending**: grant received from CC, awaiting first voice frame.
//! - **Active**: voice frames are arriving.
//! - **Ended**: terminator received or grant timed out.
//!
//! Call end is detected by voice LC terminator (TDULC) NID, simple
//! terminator (TDU) NID, or grant timeout (fallback).

use std::collections::HashMap;
use std::time::Instant;

use crate::channel_manager::VoiceChannelEvent;
use crate::p25::nid::DataUnit;
use crate::p25::receiver::ReceiverEvent;
use crate::p25::types::{Frequency, SourceId, TalkgroupId};

/// Lifecycle state of a voice call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallState {
    /// Grant received, awaiting first voice frame.
    Pending,
    /// Voice frames are arriving.
    Active,
    /// Call has ended (terminator or timeout).
    Ended,
}

/// Reason a call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    /// Voice LC terminator (TDULC) received.
    LcTerminator,
    /// Simple terminator (TDU) received.
    SimpleTerminator,
    /// Grant timed out without terminator.
    Timeout,
}

/// Metadata for a single voice call.
#[derive(Debug, Clone)]
pub struct Call {
    /// Receive frequency of this call in hertz.
    pub frequency: Frequency,
    /// Talkgroup for this call.
    pub talkgroup: TalkgroupId,
    /// Source unit that initiated the call.
    pub source: SourceId,
    /// Wall-clock time when the grant was received.
    pub start_time: Instant,
    /// Number of decoded voice frames in this call.
    pub frame_count: u32,
    /// Current lifecycle state.
    pub state: CallState,
    /// Why the call ended (set when state transitions to Ended).
    pub end_reason: Option<EndReason>,
}

/// Tracks voice call lifecycles across frequencies.
///
/// Feed [`VoiceChannelEvent`]s from the [`ChannelManager`] to track
/// call start, activity, and termination. Completed calls are returned
/// to the caller for recording or logging.
pub struct CallTracker {
    /// Active calls keyed by receive frequency.
    calls: HashMap<Frequency, Call>,
}

impl CallTracker {
    /// Create a new call tracker with no active calls.
    pub fn new() -> Self {
        Self {
            calls: HashMap::new(),
        }
    }

    /// Start tracking a new call from a control channel grant.
    ///
    /// If a call already exists at this frequency, it is replaced.
    pub fn start_call(&mut self, frequency: Frequency, talkgroup: TalkgroupId, source: SourceId) {
        let call = Call {
            frequency,
            talkgroup,
            source,
            start_time: Instant::now(),
            frame_count: 0,
            state: CallState::Pending,
            end_reason: None,
        };
        self.calls.insert(frequency, call);
    }

    /// Process a voice channel event and update call state.
    ///
    /// Returns `Some(call)` when a call transitions to Ended,
    /// allowing the caller to finalize recording or logging.
    pub fn process_event(&mut self, event: &VoiceChannelEvent) -> Option<Call> {
        match &event.event {
            ReceiverEvent::VoiceFrame(_) => {
                self.mark_active(event.frequency);
                None
            }
            ReceiverEvent::Nid(nid) => match nid.data_unit {
                DataUnit::VoiceSimpleTerminator => {
                    self.end_call(event.frequency, EndReason::SimpleTerminator)
                }
                DataUnit::VoiceLcTerminator => {
                    self.end_call(event.frequency, EndReason::LcTerminator)
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// End a call due to grant timeout.
    ///
    /// Called by the channel manager when a voice channel expires.
    pub fn timeout_call(&mut self, frequency: Frequency) -> Option<Call> {
        self.end_call(frequency, EndReason::Timeout)
    }

    /// Return the number of currently tracked calls.
    pub fn active_call_count(&self) -> usize {
        self.calls.len()
    }

    /// Look up the current call at a frequency, if any.
    pub fn get_call(&self, frequency: &Frequency) -> Option<&Call> {
        self.calls.get(frequency)
    }

    /// Mark a call as active (voice frames arriving).
    fn mark_active(&mut self, frequency: Frequency) {
        if let Some(call) = self.calls.get_mut(&frequency) {
            if call.state == CallState::Pending {
                call.state = CallState::Active;
            }
            call.frame_count += 1;
        }
    }

    /// End a call and remove it from tracking.
    fn end_call(&mut self, frequency: Frequency, reason: EndReason) -> Option<Call> {
        self.calls.remove(&frequency).map(|mut call| {
            call.state = CallState::Ended;
            call.end_reason = Some(reason);
            call
        })
    }
}

impl Default for CallTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p25::nid::{DataUnit, NetworkId};
    use crate::p25::types::Nac;
    use crate::p25::voice::frame::VoiceFrame;

    fn test_frequency() -> Frequency {
        Frequency::from_hz(851_062_500)
    }

    fn test_talkgroup() -> TalkgroupId {
        TalkgroupId::new(100)
    }

    fn test_source() -> SourceId {
        SourceId::new(12345)
    }

    fn make_voice_frame_event(frequency: Frequency) -> VoiceChannelEvent {
        VoiceChannelEvent {
            frequency,
            talkgroup: test_talkgroup(),
            source: test_source(),
            nac: Nac::new(0x293),
            event: ReceiverEvent::VoiceFrame(VoiceFrame {
                chunks: [0; 8],
                errors: [0; 7],
            }),
            audio: None,
        }
    }

    fn make_nid_event(frequency: Frequency, data_unit: DataUnit) -> VoiceChannelEvent {
        VoiceChannelEvent {
            frequency,
            talkgroup: test_talkgroup(),
            source: test_source(),
            nac: Nac::new(0x293),
            event: ReceiverEvent::Nid(NetworkId {
                access_code: Nac::new(0x293),
                data_unit,
                parity_ok: true,
            }),
            audio: None,
        }
    }

    #[test]
    fn new_tracker_has_no_calls() {
        let tracker = CallTracker::new();
        assert_eq!(tracker.active_call_count(), 0);
    }

    #[test]
    fn start_call_creates_pending_call() {
        let mut tracker = CallTracker::new();
        tracker.start_call(test_frequency(), test_talkgroup(), test_source());

        assert_eq!(tracker.active_call_count(), 1);
        let call = tracker.get_call(&test_frequency()).unwrap();
        assert_eq!(call.state, CallState::Pending);
        assert_eq!(call.frame_count, 0);
        assert_eq!(call.talkgroup, test_talkgroup());
        assert_eq!(call.source, test_source());
        assert!(call.end_reason.is_none());
    }

    #[test]
    fn voice_frame_transitions_pending_to_active() {
        let mut tracker = CallTracker::new();
        let freq = test_frequency();
        tracker.start_call(freq, test_talkgroup(), test_source());

        let event = make_voice_frame_event(freq);
        let ended = tracker.process_event(&event);

        assert!(ended.is_none());
        let call = tracker.get_call(&freq).unwrap();
        assert_eq!(call.state, CallState::Active);
        assert_eq!(call.frame_count, 1);
    }

    #[test]
    fn voice_frames_increment_count() {
        let mut tracker = CallTracker::new();
        let freq = test_frequency();
        tracker.start_call(freq, test_talkgroup(), test_source());

        for _ in 0..5 {
            tracker.process_event(&make_voice_frame_event(freq));
        }

        let call = tracker.get_call(&freq).unwrap();
        assert_eq!(call.frame_count, 5);
        assert_eq!(call.state, CallState::Active);
    }

    #[test]
    fn lc_terminator_ends_call() {
        let mut tracker = CallTracker::new();
        let freq = test_frequency();
        tracker.start_call(freq, test_talkgroup(), test_source());
        tracker.process_event(&make_voice_frame_event(freq));

        let event = make_nid_event(freq, DataUnit::VoiceLcTerminator);
        let ended = tracker.process_event(&event);

        let call = ended.expect("call should have ended");
        assert_eq!(call.state, CallState::Ended);
        assert_eq!(call.end_reason, Some(EndReason::LcTerminator));
        assert_eq!(call.frame_count, 1);
        assert_eq!(tracker.active_call_count(), 0);
    }

    #[test]
    fn simple_terminator_ends_call() {
        let mut tracker = CallTracker::new();
        let freq = test_frequency();
        tracker.start_call(freq, test_talkgroup(), test_source());
        tracker.process_event(&make_voice_frame_event(freq));

        let event = make_nid_event(freq, DataUnit::VoiceSimpleTerminator);
        let ended = tracker.process_event(&event);

        let call = ended.expect("call should have ended");
        assert_eq!(call.end_reason, Some(EndReason::SimpleTerminator));
        assert_eq!(tracker.active_call_count(), 0);
    }

    #[test]
    fn timeout_ends_call() {
        let mut tracker = CallTracker::new();
        let freq = test_frequency();
        tracker.start_call(freq, test_talkgroup(), test_source());

        let ended = tracker.timeout_call(freq);

        let call = ended.expect("call should have ended");
        assert_eq!(call.state, CallState::Ended);
        assert_eq!(call.end_reason, Some(EndReason::Timeout));
        assert_eq!(tracker.active_call_count(), 0);
    }

    #[test]
    fn timeout_nonexistent_frequency_returns_none() {
        let mut tracker = CallTracker::new();
        let ended = tracker.timeout_call(test_frequency());
        assert!(ended.is_none());
    }

    #[test]
    fn event_for_untracked_frequency_is_ignored() {
        let mut tracker = CallTracker::new();
        let event = make_voice_frame_event(test_frequency());
        let ended = tracker.process_event(&event);
        assert!(ended.is_none());
        assert_eq!(tracker.active_call_count(), 0);
    }

    #[test]
    fn start_call_replaces_existing() {
        let mut tracker = CallTracker::new();
        let freq = test_frequency();

        tracker.start_call(freq, TalkgroupId::new(100), test_source());
        tracker.process_event(&make_voice_frame_event(freq));

        // Replace with new call on same frequency.
        tracker.start_call(freq, TalkgroupId::new(200), test_source());
        let call = tracker.get_call(&freq).unwrap();
        assert_eq!(call.talkgroup, TalkgroupId::new(200));
        assert_eq!(call.state, CallState::Pending);
        assert_eq!(call.frame_count, 0);
    }

    #[test]
    fn multiple_frequencies_tracked_independently() {
        let mut tracker = CallTracker::new();
        let freq_a = Frequency::from_hz(851_062_500);
        let freq_b = Frequency::from_hz(851_068_750);

        tracker.start_call(freq_a, TalkgroupId::new(100), test_source());
        tracker.start_call(freq_b, TalkgroupId::new(200), test_source());
        assert_eq!(tracker.active_call_count(), 2);

        // End one, the other persists.
        tracker.timeout_call(freq_a);
        assert_eq!(tracker.active_call_count(), 1);
        assert!(tracker.get_call(&freq_b).is_some());
    }

    #[test]
    fn non_terminator_nid_does_not_end_call() {
        let mut tracker = CallTracker::new();
        let freq = test_frequency();
        tracker.start_call(freq, test_talkgroup(), test_source());

        // LDU1 NID should not end the call.
        let event = make_nid_event(freq, DataUnit::VoiceLcFrameGroup);
        let ended = tracker.process_event(&event);
        assert!(ended.is_none());
        assert_eq!(tracker.active_call_count(), 1);
    }

    #[test]
    fn default_creates_empty_tracker() {
        let tracker = CallTracker::default();
        assert_eq!(tracker.active_call_count(), 0);
    }
}

//! Control channel event handler.
//!
//! Processes [`ReceiverEvent`] from the CC pipeline, updating the
//! identifier table and forwarding grant events to the channel manager.
//! Produces [`DecoderEvent`] values for downstream consumers.

use crate::channel_manager::ChannelManager;
use crate::output::call_recorder::CallRecorder;
use crate::p25::ident::IdentTable;
use crate::p25::receiver::ReceiverEvent;
use crate::p25::tsbk::{TsbkOpcode, TsbkPayload};
use crate::p25::types::SourceId;

use super::event::DecoderEvent;

/// Process a CC pipeline event, updating internal state.
///
/// Returns a `DecoderEvent` when the event should be forwarded to
/// consumers (e.g. a TSBK for JSON emission). Returns `None` for
/// events that are handled internally only (NID, errors).
///
/// Also updates the identifier table, forwards grants to the channel
/// manager, and starts recordings for new in-band channels.
pub fn handle_cc_event(
    event: ReceiverEvent,
    ident_table: &mut IdentTable,
    tsbk_count: &mut u64,
    channel_manager: Option<&mut ChannelManager>,
    recorder: Option<&mut CallRecorder>,
) -> Option<DecoderEvent> {
    match event {
        ReceiverEvent::Nid(_) => Some(DecoderEvent::CcSync),
        ReceiverEvent::Tsbk(tsbk) => {
            if matches!(tsbk.payload, TsbkPayload::IdentifierUpdate { .. }) {
                ident_table.update(&tsbk);
            }

            // Forward grant events to channel manager (trunk mode).
            if let Some(cm) = channel_manager {
                let is_grant = matches!(
                    tsbk.header.opcode,
                    TsbkOpcode::GroupVoiceChannelGrant
                        | TsbkOpcode::GroupVoiceChannelGrantUpdate
                        | TsbkOpcode::GroupVoiceChannelGrantUpdateExplicit
                );
                if is_grant {
                    cm.handle_grant(&tsbk, ident_table);
                    if let Some(rec) = recorder {
                        start_recording_for_grant(rec, &tsbk, ident_table, cm);
                    }
                }
            }

            *tsbk_count += 1;
            Some(DecoderEvent::Tsbk(tsbk))
        }
        ReceiverEvent::Error(err) => Some(DecoderEvent::Error(err)),
        _ => None,
    }
}

/// Start a call recording for a grant TSBK if it represents a new
/// channel activation and the frequency is within capture bandwidth.
fn start_recording_for_grant(
    recorder: &mut CallRecorder,
    tsbk: &crate::p25::tsbk::Tsbk,
    ident_table: &IdentTable,
    channel_manager: &ChannelManager,
) {
    match &tsbk.payload {
        TsbkPayload::GroupVoiceChannelGrant {
            channel,
            talkgroup,
            source,
        } => {
            if let Some(freq) = ident_table.resolve_frequency(*channel)
                && channel_manager.is_in_band(freq)
                && !recorder.has_active_recording(&freq)
            {
                recorder.start_call(freq, *talkgroup, *source);
            }
        }
        TsbkPayload::GroupVoiceChannelGrantUpdate {
            channel_a,
            talkgroup_a,
            channel_b,
            talkgroup_b,
        } => {
            let zero_source = SourceId::new(0);
            if let Some(freq) = ident_table.resolve_frequency(*channel_a)
                && channel_manager.is_in_band(freq)
                && !recorder.has_active_recording(&freq)
            {
                recorder.start_call(freq, *talkgroup_a, zero_source);
            }
            if let Some(freq) = ident_table.resolve_frequency(*channel_b)
                && channel_manager.is_in_band(freq)
                && !recorder.has_active_recording(&freq)
            {
                recorder.start_call(freq, *talkgroup_b, zero_source);
            }
        }
        TsbkPayload::GroupVoiceChannelGrantUpdateExplicit {
            receive_channel,
            talkgroup,
            ..
        } => {
            let zero_source = SourceId::new(0);
            if let Some(freq) = ident_table.resolve_frequency(*receive_channel)
                && channel_manager.is_in_band(freq)
                && !recorder.has_active_recording(&freq)
            {
                recorder.start_call(freq, *talkgroup, zero_source);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p25::tsbk::{Tsbk, TsbkHeader, TsbkOpcode, TsbkPayload};

    #[test]
    fn nid_produces_cc_sync() {
        let nid = crate::p25::nid::NetworkId {
            access_code: crate::p25::types::Nac::new(0),
            data_unit: crate::p25::nid::DataUnit::TrunkingSignaling,
            parity_ok: true,
        };
        let mut table = IdentTable::new();
        let mut count = 0;
        let result = handle_cc_event(ReceiverEvent::Nid(nid), &mut table, &mut count, None, None);
        assert!(matches!(result, Some(DecoderEvent::CcSync)));
    }

    #[test]
    fn tsbk_produces_event_and_increments_count() {
        let tsbk = Tsbk {
            header: TsbkHeader {
                opcode: TsbkOpcode::NetworkStatusBroadcast,
                last_block: true,
                protected: false,
                manufacturer_id: 0x00,
            },
            payload: TsbkPayload::Unknown { data: [0; 8] },
        };
        let mut table = IdentTable::new();
        let mut count = 0;
        let result = handle_cc_event(
            ReceiverEvent::Tsbk(tsbk),
            &mut table,
            &mut count,
            None,
            None,
        );
        assert!(matches!(result, Some(DecoderEvent::Tsbk(_))));
        assert_eq!(count, 1);
    }

    #[test]
    fn error_produces_event() {
        let err = crate::p25::error::P25Error::NoFrameSync;
        let mut table = IdentTable::new();
        let mut count = 0;
        let result = handle_cc_event(
            ReceiverEvent::Error(err),
            &mut table,
            &mut count,
            None,
            None,
        );
        assert!(matches!(result, Some(DecoderEvent::Error(_))));
        assert_eq!(count, 0);
    }
}

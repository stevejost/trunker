//! Voice channel event handler.
//!
//! Converts [`VoiceChannelEvent`] from the channel manager into
//! [`DecoderEvent`] values for downstream consumers.

use crate::channel_manager::VoiceChannelEvent;
use crate::p25::receiver::ReceiverEvent;

use super::event::DecoderEvent;

/// Convert a voice channel event into a `DecoderEvent`.
///
/// Returns `None` for events that have no downstream representation
/// (e.g. TSBKs on a voice channel, which shouldn't occur).
pub fn process_voice_event(voice_event: &VoiceChannelEvent) -> Option<DecoderEvent> {
    match &voice_event.event {
        ReceiverEvent::VoiceFrame(vf) => Some(DecoderEvent::VoiceFrame(vf.clone())),
        ReceiverEvent::LinkControl(lc) => Some(DecoderEvent::LinkControl(*lc)),
        ReceiverEvent::CryptoControl(cc) => Some(DecoderEvent::CryptoControl(*cc)),
        ReceiverEvent::VoiceHeader(hdr) => Some(DecoderEvent::VoiceHeader(hdr.clone())),
        ReceiverEvent::DataFragment(frag) => Some(DecoderEvent::DataFragment(*frag)),
        ReceiverEvent::Nid(nid) => Some(DecoderEvent::VoiceChannelNid(*nid)),
        ReceiverEvent::Error(err) => Some(DecoderEvent::Error(err.clone())),
        ReceiverEvent::Tsbk(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p25::nid::{DataUnit, NetworkId};
    use crate::p25::types::{Frequency, Nac, SourceId, TalkgroupId};
    use crate::p25::voice::frame::VoiceFrame;

    fn test_voice_event(event: ReceiverEvent) -> VoiceChannelEvent {
        VoiceChannelEvent {
            frequency: Frequency::from_hz(851_000_000),
            talkgroup: TalkgroupId::new(100),
            source: SourceId::new(1234),
            nac: Nac::new(0x5FC),
            event,
            audio: None,
        }
    }

    #[test]
    fn voice_frame_converts() {
        let vf = VoiceFrame {
            chunks: [0; 8],
            errors: [0; 7],
        };
        let ve = test_voice_event(ReceiverEvent::VoiceFrame(vf));
        assert!(matches!(
            process_voice_event(&ve),
            Some(DecoderEvent::VoiceFrame(_))
        ));
    }

    #[test]
    fn nid_converts_to_voice_channel_nid() {
        let nid = NetworkId {
            access_code: Nac::new(0x5FC),
            data_unit: DataUnit::VoiceLcFrameGroup,
            parity_ok: true,
        };
        let ve = test_voice_event(ReceiverEvent::Nid(nid));
        assert!(matches!(
            process_voice_event(&ve),
            Some(DecoderEvent::VoiceChannelNid(_))
        ));
    }

    #[test]
    fn error_converts() {
        let err = crate::p25::error::P25Error::NoFrameSync;
        let ve = test_voice_event(ReceiverEvent::Error(err));
        assert!(matches!(
            process_voice_event(&ve),
            Some(DecoderEvent::Error(_))
        ));
    }
}

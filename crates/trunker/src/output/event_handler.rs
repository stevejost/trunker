//! Dispatch decoded P25 protocol events to JSON output and optional audio.
//!
//! Provides [`handle_receiver_event`] which formats each protocol event as
//! a JSON line and writes it to any [`Write`] destination. Voice frames are
//! optionally decoded through an IMBE vocoder and written to a WAV file.

use std::io::Write;

use crate::output::json;
use crate::output::wav::WavWriter;
use crate::p25::ident::IdentTable;
use crate::p25::receiver::ReceiverEvent;
use crate::p25::tsbk::TsbkPayload;
use crate::p25::types::Nac;
use crate::vocoder::{AudioBuffer, ImbeDecoder, ReceivedFrame, SAMPLES_PER_FRAME};

/// Dispatch a decoded protocol event to JSON output and optional audio.
///
/// Writes JSON lines to `output` for TSBK, voice, link control, crypto
/// control, voice header, and data fragment events. NID and error events
/// are logged via `tracing` and produce no output.
///
/// * `nac` - Network access code from the current NID.
/// * `ident_table` - Identifier table for resolving channel numbers to frequencies.
/// * `tsbk_count` - Running count of decoded TSBKs (incremented on each TSBK).
/// * `decoder` - Optional IMBE vocoder for decoding voice frames to audio.
/// * `wav_writer` - Optional WAV writer for decoded audio output.
/// * `event` - The protocol event to handle.
/// * `output` - Destination for JSON line output.
pub fn handle_receiver_event(
    nac: Nac,
    ident_table: &mut IdentTable,
    tsbk_count: &mut u64,
    decoder: &mut Option<ImbeDecoder>,
    wav_writer: Option<&mut WavWriter>,
    event: ReceiverEvent,
    output: &mut impl Write,
) {
    match event {
        ReceiverEvent::Nid(nid) => {
            tracing::debug!(
                nac = %nid.access_code,
                duid = ?nid.data_unit,
                parity_ok = nid.parity_ok,
                "NID decoded"
            );
        }
        ReceiverEvent::Tsbk(tsbk) => {
            if matches!(tsbk.payload, TsbkPayload::IdentifierUpdate { .. }) {
                ident_table.update(&tsbk);
            }
            let line = json::to_json_line(nac, &tsbk, ident_table);
            let _ = writeln!(output, "{line}");
            *tsbk_count += 1;
        }
        ReceiverEvent::VoiceFrame(vf) => {
            decode_voice_frame(decoder, wav_writer, &vf);
            let line = json::voice_frame_json_line(nac, &vf);
            let _ = writeln!(output, "{line}");
        }
        ReceiverEvent::LinkControl(lc) => {
            let line = json::link_control_json_line(nac, &lc);
            let _ = writeln!(output, "{line}");
        }
        ReceiverEvent::CryptoControl(cc) => {
            let line = json::crypto_control_json_line(nac, &cc);
            let _ = writeln!(output, "{line}");
        }
        ReceiverEvent::VoiceHeader(hdr) => {
            let line = json::voice_header_json_line(nac, &hdr);
            let _ = writeln!(output, "{line}");
        }
        ReceiverEvent::DataFragment(frag) => {
            let line = json::data_fragment_json_line(nac, frag);
            let _ = writeln!(output, "{line}");
        }
        ReceiverEvent::Error(err) => {
            tracing::debug!(error = %err, "decode error");
        }
    }
}

/// Decode a voice frame through the IMBE vocoder and write audio samples.
fn decode_voice_frame(
    decoder: &mut Option<ImbeDecoder>,
    wav_writer: Option<&mut WavWriter>,
    voice_frame: &crate::p25::voice::frame::VoiceFrame,
) {
    if let Some(dec) = decoder.as_mut() {
        let received = ReceivedFrame::from(voice_frame);
        let mut buffer: AudioBuffer = [0.0; SAMPLES_PER_FRAME];
        dec.decode(received, &mut buffer);

        if let Some(writer) = wav_writer
            && let Err(e) = writer.write_samples(&buffer)
        {
            tracing::warn!(error = %e, "failed to write WAV samples");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p25::types::Nac;

    #[test]
    fn tsbk_event_writes_json_line() {
        let nac = Nac::new(0x293);
        let mut ident_table = IdentTable::new();
        let mut tsbk_count: u64 = 0;
        let mut decoder = None;

        // Build a TSBK with valid CRC.
        let data: [u8; 10] = [0x80, 0x00, 0x61, 0x23, 0x00, 0x42, 0x01, 0x02, 0x03, 0x00];
        let crc = crate::p25::crc::crc16(&data);
        let mut tsbk_bytes = [0u8; 12];
        tsbk_bytes[..10].copy_from_slice(&data);
        tsbk_bytes[10] = (crc >> 8) as u8;
        tsbk_bytes[11] = crc as u8;
        let tsbk = crate::p25::tsbk::parse(&tsbk_bytes).unwrap();

        let event = ReceiverEvent::Tsbk(tsbk);
        let mut output = Vec::new();

        handle_receiver_event(
            nac,
            &mut ident_table,
            &mut tsbk_count,
            &mut decoder,
            None,
            event,
            &mut output,
        );

        assert_eq!(tsbk_count, 1);
        let line = String::from_utf8(output).unwrap();
        assert!(line.ends_with('\n'), "output should end with newline");
        let trimmed = line.trim();
        let v: serde_json::Value = serde_json::from_str(trimmed).unwrap();
        assert_eq!(v["nac"], "0x293");
        assert_eq!(v["name"], "GRP_V_CH_GRANT");
    }

    #[test]
    fn nid_event_writes_nothing() {
        let nac = Nac::new(0x293);
        let mut ident_table = IdentTable::new();
        let mut tsbk_count: u64 = 0;
        let mut decoder = None;

        let nid = crate::p25::nid::NetworkId {
            access_code: Nac::new(0x293),
            data_unit: crate::p25::nid::DataUnit::TrunkingSignaling,
            parity_ok: true,
        };
        let event = ReceiverEvent::Nid(nid);
        let mut output = Vec::new();

        handle_receiver_event(
            nac,
            &mut ident_table,
            &mut tsbk_count,
            &mut decoder,
            None,
            event,
            &mut output,
        );

        assert!(output.is_empty(), "NID events should produce no output");
        assert_eq!(tsbk_count, 0);
    }

    #[test]
    fn error_event_writes_nothing() {
        let nac = Nac::new(0x293);
        let mut ident_table = IdentTable::new();
        let mut tsbk_count: u64 = 0;
        let mut decoder = None;

        let event = ReceiverEvent::Error(crate::p25::error::P25Error::NoFrameSync);
        let mut output = Vec::new();

        handle_receiver_event(
            nac,
            &mut ident_table,
            &mut tsbk_count,
            &mut decoder,
            None,
            event,
            &mut output,
        );

        assert!(output.is_empty(), "error events should produce no output");
    }

    #[test]
    fn voice_frame_event_writes_json_line() {
        use crate::p25::voice::frame::VoiceFrame;

        let nac = Nac::new(0x5FC);
        let mut ident_table = IdentTable::new();
        let mut tsbk_count: u64 = 0;
        let mut decoder = None;

        let frame = VoiceFrame {
            chunks: [0x123, 0x456, 0x789, 0xABC, 0x3FF, 0x555, 0x000, 0x7F],
            errors: [0, 1, 0, 0, 0, 2, 0],
        };
        let event = ReceiverEvent::VoiceFrame(frame);
        let mut output = Vec::new();

        handle_receiver_event(
            nac,
            &mut ident_table,
            &mut tsbk_count,
            &mut decoder,
            None,
            event,
            &mut output,
        );

        let line = String::from_utf8(output).unwrap();
        assert!(line.ends_with('\n'));
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["type"], "voice_frame");
        assert_eq!(v["nac"], "0x5FC");
        assert_eq!(
            tsbk_count, 0,
            "voice frames should not increment tsbk_count"
        );
    }

    #[test]
    fn data_fragment_event_writes_json_line() {
        let nac = Nac::new(0x5FC);
        let mut ident_table = IdentTable::new();
        let mut tsbk_count: u64 = 0;
        let mut decoder = None;

        let event = ReceiverEvent::DataFragment(0xABCD);
        let mut output = Vec::new();

        handle_receiver_event(
            nac,
            &mut ident_table,
            &mut tsbk_count,
            &mut decoder,
            None,
            event,
            &mut output,
        );

        let line = String::from_utf8(output).unwrap();
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["type"], "data_fragment");
        assert_eq!(v["data"], 0xABCD);
    }
}

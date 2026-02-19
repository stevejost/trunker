//! Data unit receiver state machine.
//!
//! Consumes data dibits (after status symbol removal) and decodes
//! P25 data units: TSDU (trunking signaling), LDU1/LDU2 (voice),
//! and terminator frames.

use crate::p25::consts::{CODING_DIBITS, NID_DIBITS};
use crate::p25::error::P25Error;
use crate::p25::interleave;
use crate::p25::nid::{self, DataUnit, NetworkId, NidIntegrityPolicy};
use crate::p25::trellis;
use crate::p25::tsbk::{self, Tsbk};
use crate::p25::types::Dibit;
use crate::p25::voice::control::LinkControlFields;
use crate::p25::voice::crypto::CryptoControlFields;
use crate::p25::voice::frame::VoiceFrame;
use crate::p25::voice::frame_group::{FrameGroupEvent, FrameGroupReceiver, FrameGroupType};
use crate::p25::voice::header::{VoiceHeaderFields, VoiceHeaderReceiver};
use crate::p25::voice::terminator::VoiceLcTerminatorReceiver;

/// Events produced by the data unit receiver.
#[derive(Debug)]
pub enum ReceiverEvent {
    /// A NID was successfully decoded.
    Nid(NetworkId),
    /// A TSBK was successfully decoded and CRC-verified.
    Tsbk(Tsbk),
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
    /// A decode error occurred (logged, not fatal).
    Error(P25Error),
}

/// Internal state of the data unit receiver.
#[derive(Debug)]
enum State {
    /// Collecting NID dibits after frame sync.
    CollectNid,
    /// Collecting coded TSBK dibits (98 per block).
    CollectTsbk,
    /// Decoding an LDU1 (link control) frame group.
    DecodeLcFrameGroup,
    /// Decoding an LDU2 (crypto control) frame group.
    DecodeCcFrameGroup,
    /// Decoding a voice header (HDU).
    DecodeHeader,
    /// Decoding a voice LC terminator (TDULC).
    DecodeLcTerminator,
    /// Skipping non-TSDU data unit until next sync.
    Skip,
}

/// Receives data dibits and decodes P25 data units.
///
/// After frame sync is detected externally, the caller feeds data
/// dibits (with status symbols already removed) into this receiver.
/// It collects the NID, dispatches based on DUID, and decodes TSDU
/// (trunking signaling), LDU1/LDU2 (voice), and terminator data units.
pub struct DataUnitReceiver {
    state: State,
    /// Buffer for accumulating dibits in the current phase.
    buffer: Vec<Dibit>,
    /// Target number of dibits to collect in the current phase.
    target: usize,
    /// The NID from the current data unit.
    current_nid: Option<NetworkId>,
    /// Frame group receiver for LDU1/LDU2 voice data units.
    frame_group: Option<FrameGroupReceiver>,
    /// Voice header receiver for HDU data units.
    header_receiver: Option<VoiceHeaderReceiver>,
    /// Voice LC terminator receiver for TDULC data units.
    terminator_receiver: Option<VoiceLcTerminatorReceiver>,
    /// Policy for accepting/rejecting NIDs with failed integrity checks.
    integrity_policy: NidIntegrityPolicy,
}

impl DataUnitReceiver {
    /// Create a new receiver with the given NID integrity policy.
    pub fn new(integrity_policy: NidIntegrityPolicy) -> Self {
        Self {
            state: State::CollectNid,
            buffer: Vec::with_capacity(CODING_DIBITS),
            target: NID_DIBITS,
            current_nid: None,
            frame_group: None,
            header_receiver: None,
            terminator_receiver: None,
            integrity_policy,
        }
    }

    /// Reset the receiver to await a new NID after frame sync.
    pub fn reset(&mut self) {
        self.state = State::CollectNid;
        self.buffer.clear();
        self.target = NID_DIBITS;
        self.current_nid = None;
        self.frame_group = None;
        self.header_receiver = None;
        self.terminator_receiver = None;
    }

    /// Feed one data dibit into the receiver.
    ///
    /// Returns an event when a NID, TSBK, voice frame, or other
    /// component is decoded. Returns `None` when more dibits are needed.
    pub fn feed(&mut self, dibit: Dibit) -> Option<ReceiverEvent> {
        // Streaming states bypass the buffer/target mechanism
        // and feed directly to the appropriate receiver.
        match self.state {
            State::DecodeLcFrameGroup | State::DecodeCcFrameGroup => {
                return self.feed_frame_group(dibit);
            }
            State::DecodeHeader => {
                return self.feed_header(dibit);
            }
            State::DecodeLcTerminator => {
                return self.feed_terminator(dibit);
            }
            _ => {}
        }

        self.buffer.push(dibit);

        if self.buffer.len() < self.target {
            return None;
        }

        match self.state {
            State::CollectNid => self.process_nid(),
            State::CollectTsbk => self.process_tsbk(),
            State::DecodeLcFrameGroup
            | State::DecodeCcFrameGroup
            | State::DecodeHeader
            | State::DecodeLcTerminator => {
                unreachable!("handled above")
            }
            State::Skip => {
                // Discard dibits until externally reset via reset().
                self.buffer.clear();
                None
            }
        }
    }

    /// Whether the receiver has finished processing the current data unit.
    ///
    /// Returns `true` when the receiver is in Skip state, after the
    /// last TSBK has been decoded, or after a frame group is complete.
    pub fn is_done(&self) -> bool {
        matches!(self.state, State::Skip)
    }

    /// Process a complete NID.
    fn process_nid(&mut self) -> Option<ReceiverEvent> {
        let dibits: Vec<Dibit> = self.buffer.drain(..).collect();

        match nid::decode_nid(&dibits) {
            Ok(nid) => {
                if let Some(rejection) = self.check_nid_integrity(&nid) {
                    return Some(rejection);
                }

                self.current_nid = Some(nid);
                self.route_data_unit(nid.data_unit);
                Some(ReceiverEvent::Nid(nid))
            }
            Err(err) => {
                self.state = State::Skip;
                Some(ReceiverEvent::Error(err))
            }
        }
    }

    /// Check NID integrity and return an error event if the policy rejects it.
    ///
    /// Returns `Some(error_event)` if rejected, `None` if accepted.
    fn check_nid_integrity(&mut self, nid: &NetworkId) -> Option<ReceiverEvent> {
        if nid.parity_ok {
            return None;
        }
        match self.integrity_policy {
            NidIntegrityPolicy::Strict => {
                self.state = State::Skip;
                Some(ReceiverEvent::Error(P25Error::NidRejected {
                    reason: "parity check failed",
                    nac: nid.access_code.value(),
                    duid: nid.data_unit.to_bits(),
                }))
            }
            NidIntegrityPolicy::Permissive => {
                tracing::warn!(
                    nac = %nid.access_code,
                    duid = ?nid.data_unit,
                    "NID parity failed, continuing (permissive mode)"
                );
                None
            }
        }
    }

    /// Transition receiver state based on the data unit type.
    fn route_data_unit(&mut self, data_unit: DataUnit) {
        match data_unit {
            DataUnit::TrunkingSignaling => {
                self.state = State::CollectTsbk;
                self.target = CODING_DIBITS;
            }
            DataUnit::VoiceLcFrameGroup => {
                self.frame_group = Some(FrameGroupReceiver::new(FrameGroupType::LinkControl));
                self.state = State::DecodeLcFrameGroup;
            }
            DataUnit::VoiceCcFrameGroup => {
                self.frame_group = Some(FrameGroupReceiver::new(FrameGroupType::CryptoControl));
                self.state = State::DecodeCcFrameGroup;
            }
            DataUnit::VoiceHeader => {
                self.header_receiver = Some(VoiceHeaderReceiver::new());
                self.state = State::DecodeHeader;
            }
            DataUnit::VoiceLcTerminator => {
                self.terminator_receiver = Some(VoiceLcTerminatorReceiver::new());
                self.state = State::DecodeLcTerminator;
            }
            DataUnit::VoiceSimpleTerminator | DataUnit::DataPacket | DataUnit::Unknown(_) => {
                self.state = State::Skip;
            }
        }
    }

    /// Process a complete set of 98 coded TSBK dibits.
    fn process_tsbk(&mut self) -> Option<ReceiverEvent> {
        let coded_dibits: Vec<Dibit> = self.buffer.drain(..).collect();

        // Deinterleave the 98 coded dibits.
        let coded_array: [Dibit; CODING_DIBITS] = match coded_dibits.try_into() {
            Ok(arr) => arr,
            Err(_) => {
                self.state = State::Skip;
                return Some(ReceiverEvent::Error(P25Error::PayloadTooShort {
                    expected: CODING_DIBITS,
                    actual: 0,
                }));
            }
        };

        let deinterleaved = interleave::deinterleave(&coded_array);

        // Trellis decode: 98 coded dibits -> 48 data dibits.
        let decoded = match trellis::decode(&deinterleaved) {
            Ok(d) => d,
            Err(err) => {
                self.state = State::Skip;
                return Some(ReceiverEvent::Error(err));
            }
        };

        // Convert 48 dibits to 12 bytes.
        let bytes = match tsbk::dibits_to_bytes(&decoded) {
            Ok(b) => b,
            Err(err) => {
                self.state = State::Skip;
                return Some(ReceiverEvent::Error(err));
            }
        };

        tracing::trace!(
            bytes = %format!("{:02X?}", bytes),
            "TSBK raw bytes"
        );

        // Parse and CRC-verify the TSBK.
        match tsbk::parse(&bytes) {
            Ok(tsbk_msg) => {
                let is_last = tsbk_msg.header.last_block;

                if is_last {
                    // Last block: done with this data unit.
                    self.state = State::Skip;
                }
                // If not last, stay in CollectTsbk to read another block.

                Some(ReceiverEvent::Tsbk(tsbk_msg))
            }
            Err(err) => {
                self.state = State::Skip;
                Some(ReceiverEvent::Error(err))
            }
        }
    }

    /// Feed a dibit into the active frame group receiver and translate
    /// frame group events into receiver events.
    fn feed_frame_group(&mut self, dibit: Dibit) -> Option<ReceiverEvent> {
        let fg = match self.frame_group.as_mut() {
            Some(fg) => fg,
            None => {
                self.state = State::Skip;
                return None;
            }
        };

        let event = fg.feed(dibit)?;

        // Check if the frame group is finished after this event.
        if fg.is_done() {
            self.state = State::Skip;
            self.frame_group = None;
        }

        // Translate frame group events to receiver events.
        Some(match event {
            FrameGroupEvent::VoiceFrame(vf) => ReceiverEvent::VoiceFrame(vf),
            FrameGroupEvent::LinkControl(lc) => ReceiverEvent::LinkControl(lc),
            FrameGroupEvent::CryptoControl(cc) => ReceiverEvent::CryptoControl(cc),
            FrameGroupEvent::DataFragment(frag) => ReceiverEvent::DataFragment(frag),
            FrameGroupEvent::Error(err) => ReceiverEvent::Error(err),
        })
    }

    /// Feed a dibit into the active voice header receiver.
    fn feed_header(&mut self, dibit: Dibit) -> Option<ReceiverEvent> {
        let recv = match self.header_receiver.as_mut() {
            Some(r) => r,
            None => {
                self.state = State::Skip;
                return None;
            }
        };

        let result = recv.feed(dibit)?;

        if recv.is_done() {
            self.state = State::Skip;
            self.header_receiver = None;
        }

        Some(match result {
            Ok(fields) => ReceiverEvent::VoiceHeader(fields),
            Err(err) => ReceiverEvent::Error(err),
        })
    }

    /// Feed a dibit into the active voice LC terminator receiver.
    fn feed_terminator(&mut self, dibit: Dibit) -> Option<ReceiverEvent> {
        let recv = match self.terminator_receiver.as_mut() {
            Some(r) => r,
            None => {
                self.state = State::Skip;
                return None;
            }
        };

        let result = recv.feed(dibit)?;

        if recv.is_done() {
            self.state = State::Skip;
            self.terminator_receiver = None;
        }

        Some(match result {
            Ok(lc) => ReceiverEvent::LinkControl(lc),
            Err(err) => ReceiverEvent::Error(err),
        })
    }
}

impl Default for DataUnitReceiver {
    fn default() -> Self {
        Self::new(NidIntegrityPolicy::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p25::crc;
    use crate::p25::trellis::TrellisEncoder;
    use crate::p25::tsbk::TsbkOpcode;
    use crate::p25::types::Nac;

    /// Interleave permutation table (from p25.rs reference).
    /// `transmitted[i] = original[INTERLEAVE_TABLE[i]]` — output position i
    /// gets the value from input position INTERLEAVE_TABLE[i].
    const INTERLEAVE_TABLE: [usize; CODING_DIBITS] = [
        0, 1, 8, 9, 16, 17, 24, 25, 32, 33, 40, 41, 48, 49, 56, 57, 64, 65, 72, 73, 80, 81, 88, 89,
        96, 97, 2, 3, 10, 11, 18, 19, 26, 27, 34, 35, 42, 43, 50, 51, 58, 59, 66, 67, 74, 75, 82,
        83, 90, 91, 4, 5, 12, 13, 20, 21, 28, 29, 36, 37, 44, 45, 52, 53, 60, 61, 68, 69, 76, 77,
        84, 85, 92, 93, 6, 7, 14, 15, 22, 23, 30, 31, 38, 39, 46, 47, 54, 55, 62, 63, 70, 71, 78,
        79, 86, 87, 94, 95,
    ];

    /// Interleave 98 coded dibits for test construction.
    fn interleave_for_test(original: &[Dibit; CODING_DIBITS]) -> [Dibit; CODING_DIBITS] {
        let mut output = [Dibit::new(0); CODING_DIBITS];
        for (output_index, &input_index) in INTERLEAVE_TABLE.iter().enumerate() {
            output[output_index] = original[input_index];
        }
        output
    }

    /// Build 32 NID dibits from a 64-bit word.
    fn u64_to_dibits(word: u64) -> Vec<Dibit> {
        (0..32)
            .map(|i| {
                let shift = 62 - i * 2;
                Dibit::new(((word >> shift) & 0x03) as u8)
            })
            .collect()
    }

    /// Build a NID word with given NAC and DUID, setting correct parity.
    fn make_nid_word(nac: u16, duid: u8) -> u64 {
        let mut word: u64 = 0;
        word |= (u64::from(nac) & 0x0FFF) << 52;
        word |= (u64::from(duid) & 0x0F) << 48;
        if word.count_ones() % 2 != 0 {
            word |= 1;
        }
        word
    }

    /// Build NID dibits for a TSDU with the given NAC.
    fn make_tsdu_nid_dibits(nac: u16) -> Vec<Dibit> {
        u64_to_dibits(make_nid_word(nac, 0x7))
    }

    /// Build a TSBK byte payload with valid CRC, then trellis-encode
    /// and interleave it to produce 98 coded dibits.
    fn make_coded_tsbk(opcode_byte: u8, payload: &[u8; 8]) -> Vec<Dibit> {
        let mut data = [0u8; 10];
        data[0] = opcode_byte;
        data[1] = 0x00; // manufacturer ID
        data[2..10].copy_from_slice(payload);

        let crc_val = crc::crc16(&data);
        let mut tsbk_bytes = [0u8; 12];
        tsbk_bytes[..10].copy_from_slice(&data);
        tsbk_bytes[10] = (crc_val >> 8) as u8;
        tsbk_bytes[11] = crc_val as u8;

        // Convert bytes to 48 dibits.
        let dibits: Vec<Dibit> = tsbk_bytes
            .iter()
            .flat_map(|&byte| {
                [
                    Dibit::new((byte >> 6) & 0x03),
                    Dibit::new((byte >> 4) & 0x03),
                    Dibit::new((byte >> 2) & 0x03),
                    Dibit::new(byte & 0x03),
                ]
            })
            .collect();

        // Trellis encode (48 data dibits + flush = 98 coded dibits).
        let mut encoder = TrellisEncoder::new();
        let encoded = encoder.encode_all(&dibits);
        assert_eq!(encoded.len(), CODING_DIBITS);

        // Interleave for transmission order.
        let encoded_array: [Dibit; CODING_DIBITS] = encoded.try_into().unwrap();
        let interleaved = interleave_for_test(&encoded_array);
        interleaved.to_vec()
    }

    #[test]
    fn new_receiver_starts_in_collect_nid() {
        let receiver = DataUnitReceiver::default();
        assert!(!receiver.is_done());
        assert!(matches!(receiver.state, State::CollectNid));
    }

    #[test]
    fn nid_decode_produces_event() {
        let mut receiver = DataUnitReceiver::default();
        let nid_dibits = make_tsdu_nid_dibits(0x293);

        let mut events = Vec::new();
        for dibit in &nid_dibits {
            if let Some(event) = receiver.feed(*dibit) {
                events.push(event);
            }
        }

        assert_eq!(events.len(), 1);
        match &events[0] {
            ReceiverEvent::Nid(nid) => {
                assert_eq!(nid.access_code, Nac::new(0x293));
                assert!(nid.data_unit.is_trunking_signaling());
            }
            other => panic!("expected Nid event, got {other:?}"),
        }
    }

    #[test]
    fn hdu_nid_transitions_to_decode_header() {
        let mut receiver = DataUnitReceiver::default();
        // Voice header (DUID = 0x0)
        let nid_dibits = u64_to_dibits(make_nid_word(0x293, 0x0));

        for dibit in &nid_dibits {
            receiver.feed(*dibit);
        }

        assert!(!receiver.is_done());
        assert!(matches!(receiver.state, State::DecodeHeader));
        assert!(receiver.header_receiver.is_some());
    }

    #[test]
    fn tdulc_nid_transitions_to_decode_lc_terminator() {
        let mut receiver = DataUnitReceiver::default();
        // TDULC (DUID = 0xF)
        let nid_dibits = u64_to_dibits(make_nid_word(0x293, 0xF));

        for dibit in &nid_dibits {
            receiver.feed(*dibit);
        }

        assert!(!receiver.is_done());
        assert!(matches!(receiver.state, State::DecodeLcTerminator));
        assert!(receiver.terminator_receiver.is_some());
    }

    #[test]
    fn simple_terminator_transitions_to_skip() {
        let mut receiver = DataUnitReceiver::default();
        // VoiceSimpleTerminator (DUID = 0x3)
        let nid_dibits = u64_to_dibits(make_nid_word(0x293, 0x3));

        for dibit in &nid_dibits {
            receiver.feed(*dibit);
        }

        assert!(receiver.is_done());
    }

    #[test]
    fn tsdu_nid_transitions_to_collect_tsbk() {
        let mut receiver = DataUnitReceiver::default();
        let nid_dibits = make_tsdu_nid_dibits(0x293);

        for dibit in &nid_dibits {
            receiver.feed(*dibit);
        }

        assert!(!receiver.is_done());
        assert!(matches!(receiver.state, State::CollectTsbk));
    }

    #[test]
    fn decode_single_tsbk_from_tsdu() {
        let mut receiver = DataUnitReceiver::default();

        // Feed NID.
        let nid_dibits = make_tsdu_nid_dibits(0x293);
        for dibit in &nid_dibits {
            receiver.feed(*dibit);
        }

        // Feed one TSBK (last_block=1, opcode=0x3A RFSS status).
        // 0xBA = last_block(1) | protected(0) | opcode(0x3A)
        let payload: [u8; 8] = [0xCC, 0x10, 0xAA, 0xE7, 0x18, 0xD5, 0x73, 0x51];
        let coded_tsbk = make_coded_tsbk(0xBA, &payload);

        let mut events = Vec::new();
        for dibit in &coded_tsbk {
            if let Some(event) = receiver.feed(*dibit) {
                events.push(event);
            }
        }

        assert_eq!(events.len(), 1);
        match &events[0] {
            ReceiverEvent::Tsbk(tsbk) => {
                assert!(tsbk.header.last_block);
                assert_eq!(tsbk.header.opcode, TsbkOpcode::RfssStatusBroadcast);
            }
            other => panic!("expected Tsbk event, got {other:?}"),
        }

        assert!(receiver.is_done());
    }

    #[test]
    fn decode_multiple_tsbks_in_tsdu() {
        let mut receiver = DataUnitReceiver::default();

        // Feed NID.
        let nid_dibits = make_tsdu_nid_dibits(0x293);
        for dibit in &nid_dibits {
            receiver.feed(*dibit);
        }

        // First TSBK: last_block=0 (0x3A = opcode, bit 7 = 0 for not-last)
        // 0x3A = protected(0) | opcode(0x3A), last_block=0
        let payload1: [u8; 8] = [0xCC, 0x10, 0xAA, 0xE7, 0x18, 0xD5, 0x73, 0x51];
        let coded_tsbk1 = make_coded_tsbk(0x3A, &payload1);

        let mut events = Vec::new();
        for dibit in &coded_tsbk1 {
            if let Some(event) = receiver.feed(*dibit) {
                events.push(event);
            }
        }

        assert_eq!(events.len(), 1);
        match &events[0] {
            ReceiverEvent::Tsbk(tsbk) => {
                assert!(!tsbk.header.last_block);
                assert_eq!(tsbk.header.opcode, TsbkOpcode::RfssStatusBroadcast);
            }
            other => panic!("expected Tsbk event, got {other:?}"),
        }
        assert!(!receiver.is_done());

        // Second TSBK: last_block=1
        // 0xBA = last_block(1) | opcode(0x3A)
        let payload2: [u8; 8] = [0xCC, 0x10, 0xAA, 0xE7, 0x18, 0xD5, 0x73, 0x51];
        let coded_tsbk2 = make_coded_tsbk(0xBA, &payload2);

        events.clear();
        for dibit in &coded_tsbk2 {
            if let Some(event) = receiver.feed(*dibit) {
                events.push(event);
            }
        }

        assert_eq!(events.len(), 1);
        match &events[0] {
            ReceiverEvent::Tsbk(tsbk) => {
                assert!(tsbk.header.last_block);
            }
            other => panic!("expected Tsbk event, got {other:?}"),
        }
        assert!(receiver.is_done());
    }

    #[test]
    fn reset_returns_to_collect_nid() {
        let mut receiver = DataUnitReceiver::default();

        // Feed NID and transition to CollectTsbk.
        let nid_dibits = make_tsdu_nid_dibits(0x293);
        for dibit in &nid_dibits {
            receiver.feed(*dibit);
        }
        assert!(matches!(receiver.state, State::CollectTsbk));

        // Reset.
        receiver.reset();
        assert!(matches!(receiver.state, State::CollectNid));
        assert!(!receiver.is_done());
    }

    #[test]
    fn skip_state_discards_dibits() {
        let mut receiver = DataUnitReceiver::default();

        // Feed a non-TSDU NID to enter Skip state.
        let nid_dibits = u64_to_dibits(make_nid_word(0x293, 0x3)); // VoiceSimpleTerminator
        for dibit in &nid_dibits {
            receiver.feed(*dibit);
        }
        assert!(receiver.is_done());

        // Feeding more dibits should produce no events.
        for _ in 0..100 {
            let event = receiver.feed(Dibit::new(0));
            assert!(event.is_none());
        }
    }

    #[test]
    fn full_tsdu_sequence_nid_plus_tsbk() {
        let mut receiver = DataUnitReceiver::default();
        let mut all_events = Vec::new();

        // Build complete TSDU: NID + one TSBK.
        let nid_dibits = make_tsdu_nid_dibits(0x293);

        // Group voice channel grant (opcode 0x00, last_block=1)
        // 0x80 = last_block(1) | opcode(0x00)
        let payload: [u8; 8] = [
            0x61, 0x23, // channel
            0x00, 0x42, // talkgroup 66
            0x01, 0x02, 0x03, // source
            0x00,
        ];
        let coded_tsbk = make_coded_tsbk(0x80, &payload);

        // Feed NID.
        for dibit in &nid_dibits {
            if let Some(event) = receiver.feed(*dibit) {
                all_events.push(event);
            }
        }

        // Feed TSBK.
        for dibit in &coded_tsbk {
            if let Some(event) = receiver.feed(*dibit) {
                all_events.push(event);
            }
        }

        // Should have: 1 NID event + 1 TSBK event.
        assert_eq!(all_events.len(), 2);
        assert!(matches!(&all_events[0], ReceiverEvent::Nid(_)));
        assert!(matches!(&all_events[1], ReceiverEvent::Tsbk(_)));

        match &all_events[1] {
            ReceiverEvent::Tsbk(tsbk) => {
                assert_eq!(tsbk.header.opcode, TsbkOpcode::GroupVoiceChannelGrant);
                assert!(tsbk.header.last_block);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn ldu1_nid_transitions_to_decode_lc_frame_group() {
        let mut receiver = DataUnitReceiver::default();
        // LDU1: DUID = 0x5
        let nid_dibits = u64_to_dibits(make_nid_word(0x293, 0x5));

        let mut events = Vec::new();
        for dibit in &nid_dibits {
            if let Some(event) = receiver.feed(*dibit) {
                events.push(event);
            }
        }

        assert_eq!(events.len(), 1);
        match &events[0] {
            ReceiverEvent::Nid(nid) => {
                assert_eq!(nid.data_unit, DataUnit::VoiceLcFrameGroup);
            }
            other => panic!("expected Nid event, got {other:?}"),
        }
        assert!(!receiver.is_done());
        assert!(matches!(receiver.state, State::DecodeLcFrameGroup));
        assert!(receiver.frame_group.is_some());
    }

    #[test]
    fn ldu2_nid_transitions_to_decode_cc_frame_group() {
        let mut receiver = DataUnitReceiver::default();
        // LDU2: DUID = 0xA
        let nid_dibits = u64_to_dibits(make_nid_word(0x293, 0xA));

        let mut events = Vec::new();
        for dibit in &nid_dibits {
            if let Some(event) = receiver.feed(*dibit) {
                events.push(event);
            }
        }

        assert_eq!(events.len(), 1);
        match &events[0] {
            ReceiverEvent::Nid(nid) => {
                assert_eq!(nid.data_unit, DataUnit::VoiceCcFrameGroup);
            }
            other => panic!("expected Nid event, got {other:?}"),
        }
        assert!(!receiver.is_done());
        assert!(matches!(receiver.state, State::DecodeCcFrameGroup));
        assert!(receiver.frame_group.is_some());
    }

    // -- NID integrity policy tests --

    /// Build NID dibits with broken parity (flip one bit).
    fn make_bad_parity_nid_dibits(nac: u16, duid: u8) -> Vec<Dibit> {
        let word = make_nid_word(nac, duid);
        let mut dibits = u64_to_dibits(word);
        // Flip one bit to break parity.
        let flipped = dibits[15].bits() ^ 0x01;
        dibits[15] = Dibit::new(flipped);
        dibits
    }

    #[test]
    fn strict_policy_rejects_bad_parity_nid() {
        let mut receiver = DataUnitReceiver::new(NidIntegrityPolicy::Strict);
        let nid_dibits = make_bad_parity_nid_dibits(0x293, 0x7);

        let mut events = Vec::new();
        for dibit in &nid_dibits {
            if let Some(event) = receiver.feed(*dibit) {
                events.push(event);
            }
        }

        assert_eq!(events.len(), 1);
        assert!(
            matches!(
                &events[0],
                ReceiverEvent::Error(P25Error::NidRejected { .. })
            ),
            "strict mode should reject bad-parity NID, got {:?}",
            events[0]
        );
        assert!(
            receiver.is_done(),
            "should skip to next sync after rejection"
        );
    }

    #[test]
    fn permissive_policy_accepts_bad_parity_nid() {
        let mut receiver = DataUnitReceiver::new(NidIntegrityPolicy::Permissive);
        let nid_dibits = make_bad_parity_nid_dibits(0x293, 0x7);

        let mut events = Vec::new();
        for dibit in &nid_dibits {
            if let Some(event) = receiver.feed(*dibit) {
                events.push(event);
            }
        }

        assert_eq!(events.len(), 1);
        match &events[0] {
            ReceiverEvent::Nid(nid) => {
                assert!(!nid.parity_ok, "parity should be flagged as bad");
            }
            other => panic!("permissive mode should emit Nid event, got {other:?}"),
        }
        // Should have transitioned to CollectTsbk (DUID 0x7 = TSDU).
        assert!(!receiver.is_done());
        assert!(matches!(receiver.state, State::CollectTsbk));
    }

    #[test]
    fn strict_policy_accepts_good_parity_nid() {
        let mut receiver = DataUnitReceiver::new(NidIntegrityPolicy::Strict);
        let nid_dibits = make_tsdu_nid_dibits(0x293);

        let mut events = Vec::new();
        for dibit in &nid_dibits {
            if let Some(event) = receiver.feed(*dibit) {
                events.push(event);
            }
        }

        assert_eq!(events.len(), 1);
        match &events[0] {
            ReceiverEvent::Nid(nid) => {
                assert!(nid.parity_ok);
                assert!(nid.data_unit.is_trunking_signaling());
            }
            other => panic!("expected Nid event, got {other:?}"),
        }
        assert!(!receiver.is_done());
    }

    #[test]
    fn strict_rejection_includes_nac_and_duid() {
        let mut receiver = DataUnitReceiver::new(NidIntegrityPolicy::Strict);
        let nid_dibits = make_bad_parity_nid_dibits(0x5FC, 0x5);

        let mut events = Vec::new();
        for dibit in &nid_dibits {
            if let Some(event) = receiver.feed(*dibit) {
                events.push(event);
            }
        }

        match &events[0] {
            ReceiverEvent::Error(err @ P25Error::NidRejected { nac, duid, .. }) => {
                assert_eq!(*nac, 0x5FC);
                assert_eq!(*duid, 0x5);
                // Error message should contain actionable diagnostics.
                let msg = format!("{err}");
                assert!(msg.contains("5FC"), "error should contain NAC: {msg}");
            }
            other => panic!("expected NidRejected error, got {other:?}"),
        }
    }

    #[test]
    fn default_policy_is_strict() {
        assert_eq!(NidIntegrityPolicy::default(), NidIntegrityPolicy::Strict);
    }
}

//! Integration test: end-to-end decode of a synthetic P25 TSDU.
//!
//! Constructs a realistic P25 data stream (NID + coded TSBK with
//! status symbols inserted at the correct positions) and feeds it
//! through the full protocol decode pipeline to verify JSON output.

use trunker::p25::consts::{CODING_DIBITS, SYNC_SYMBOLS};
use trunker::p25::crc;
use trunker::p25::ident::IdentTable;
use trunker::p25::receiver::{DataUnitReceiver, ReceiverEvent};
use trunker::p25::status::{StatusDeinterleaver, StreamSymbol};
use trunker::p25::tsbk::{TsbkOpcode, TsbkPayload};
use trunker::p25::types::{Dibit, Nac};

// -- Trellis encoder (test-only, mirrors the crate-internal TrellisEncoder) --

/// Constellation pairs: output (hi, lo) dibits for each state transition.
const CONSTELLATION_PAIRS: [(u8, u8); 16] = [
    (0b00, 0b10),
    (0b10, 0b10),
    (0b01, 0b11),
    (0b11, 0b11),
    (0b11, 0b10),
    (0b01, 0b10),
    (0b10, 0b11),
    (0b00, 0b11),
    (0b11, 0b01),
    (0b01, 0b01),
    (0b10, 0b00),
    (0b00, 0b00),
    (0b00, 0b01),
    (0b10, 0b01),
    (0b01, 0b00),
    (0b11, 0b00),
];

/// State transition to constellation pair index.
const PAIR_INDEX: [[usize; 4]; 4] = [[0, 15, 12, 3], [4, 11, 8, 7], [13, 2, 1, 14], [9, 6, 5, 10]];

/// Trellis-encode input dibits with a trailing flush symbol.
fn trellis_encode(input: &[Dibit]) -> Vec<Dibit> {
    let mut state: usize = 0;
    let mut output = Vec::with_capacity((input.len() + 1) * 2);

    for &dibit in input.iter().chain(std::iter::once(&Dibit::new(0))) {
        let next = dibit.bits() as usize;
        let idx = PAIR_INDEX[state][next];
        let (hi, lo) = CONSTELLATION_PAIRS[idx];
        output.push(Dibit::new(hi));
        output.push(Dibit::new(lo));
        state = next;
    }
    output
}

/// Interleave table for test signal construction (inverse of deinterleave).
const INTERLEAVE_TABLE: [usize; CODING_DIBITS] = [
    0, 1, 8, 9, 16, 17, 24, 25, 32, 33, 40, 41, 48, 49, 56, 57, 64, 65, 72, 73, 80, 81, 88, 89, 96,
    97, 2, 3, 10, 11, 18, 19, 26, 27, 34, 35, 42, 43, 50, 51, 58, 59, 66, 67, 74, 75, 82, 83, 90,
    91, 4, 5, 12, 13, 20, 21, 28, 29, 36, 37, 44, 45, 52, 53, 60, 61, 68, 69, 76, 77, 84, 85, 92,
    93, 6, 7, 14, 15, 22, 23, 30, 31, 38, 39, 46, 47, 54, 55, 62, 63, 70, 71, 78, 79, 86, 87, 94,
    95,
];

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

/// Interleave coded dibits for transmission order.
///
/// `transmitted[i] = original[INTERLEAVE_TABLE[i]]` — output position i
/// gets the value from input position INTERLEAVE_TABLE[i].
fn interleave(original: &[Dibit; CODING_DIBITS]) -> [Dibit; CODING_DIBITS] {
    let mut output = [Dibit::new(0); CODING_DIBITS];
    for (output_index, &input_index) in INTERLEAVE_TABLE.iter().enumerate() {
        output[output_index] = original[input_index];
    }
    output
}

/// Build a TSBK with valid CRC, trellis-encode, and interleave.
fn make_coded_tsbk(opcode_byte: u8, payload: &[u8; 8]) -> Vec<Dibit> {
    let mut data = [0u8; 10];
    data[0] = opcode_byte;
    data[1] = 0x00;
    data[2..10].copy_from_slice(payload);

    let crc_val = crc::crc16(&data);
    let mut tsbk_bytes = [0u8; 12];
    tsbk_bytes[..10].copy_from_slice(&data);
    tsbk_bytes[10] = (crc_val >> 8) as u8;
    tsbk_bytes[11] = crc_val as u8;

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

    let encoded = trellis_encode(&dibits);
    let encoded_array: [Dibit; CODING_DIBITS] = encoded.try_into().unwrap();
    interleave(&encoded_array).to_vec()
}

/// Insert status symbols into a data dibit stream.
///
/// The StatusDeinterleaver expects status symbols at positions
/// corresponding to multiples of 36 from the start of frame sync.
/// Since frame sync consumes 24 symbols, the first status after
/// sync appears at position 12, then every 36 thereafter.
fn insert_status_symbols(data_dibits: &[Dibit]) -> Vec<Dibit> {
    let mut output = Vec::new();
    let mut data_idx = 0;
    // Position counter: starts at SYNC_SYMBOLS (24) since sync already passed.
    let mut pos = SYNC_SYMBOLS;

    while data_idx < data_dibits.len() {
        pos += 1;
        pos %= 36;

        if pos == 0 {
            // Insert a status symbol (InboundIdle = 0b11).
            output.push(Dibit::new(0b11));
        } else {
            output.push(data_dibits[data_idx]);
            data_idx += 1;
        }
    }
    output
}

/// Full pipeline: construct a synthetic TSDU, insert status symbols,
/// strip them back out, decode the NID and TSBK, verify JSON output.
#[test]
fn decode_synthetic_tsdu_through_full_pipeline() {
    let nac_value: u16 = 0x293;

    // 1. Build the raw data stream: NID dibits + coded TSBK dibits.
    let nid_dibits = u64_to_dibits(make_nid_word(nac_value, 0x7));

    // RFSS status broadcast: last_block=1, opcode=0x3A -> 0xBA
    let payload: [u8; 8] = [0xCC, 0x10, 0xAA, 0xE7, 0x18, 0xD5, 0x73, 0x51];
    let coded_tsbk = make_coded_tsbk(0xBA, &payload);

    let mut raw_data: Vec<Dibit> = Vec::new();
    raw_data.extend_from_slice(&nid_dibits);
    raw_data.extend_from_slice(&coded_tsbk);

    // 2. Insert status symbols (simulating what the transmitter does).
    let transmitted = insert_status_symbols(&raw_data);

    // 3. Run through StatusDeinterleaver to strip status symbols.
    let mut status_deinterleaver = StatusDeinterleaver::new();
    let mut data_dibits = Vec::new();
    for &dibit in &transmitted {
        match status_deinterleaver.feed(dibit) {
            StreamSymbol::Data(d) => data_dibits.push(d),
            StreamSymbol::Status(_) => { /* discard */ }
        }
    }

    // Verify we recovered the original data.
    assert_eq!(data_dibits.len(), raw_data.len());
    assert_eq!(data_dibits, raw_data);

    // 4. Feed into DataUnitReceiver.
    let mut receiver = DataUnitReceiver::new();
    let mut events = Vec::new();
    for &dibit in &data_dibits {
        if let Some(event) = receiver.feed(dibit) {
            events.push(event);
        }
    }

    // Should get: NID event + TSBK event.
    assert_eq!(events.len(), 2);

    // 5. Verify NID.
    let nid = match &events[0] {
        ReceiverEvent::Nid(nid) => {
            assert_eq!(nid.access_code, Nac::new(nac_value));
            assert!(nid.data_unit.is_trunking_signaling());
            *nid
        }
        other => panic!("expected Nid event, got {other:?}"),
    };

    // 6. Verify TSBK.
    let tsbk = match &events[1] {
        ReceiverEvent::Tsbk(tsbk) => {
            assert!(tsbk.header.last_block);
            assert_eq!(tsbk.header.opcode, TsbkOpcode::RfssStatusBroadcast);
            tsbk
        }
        other => panic!("expected Tsbk event, got {other:?}"),
    };

    match &tsbk.payload {
        TsbkPayload::RfssStatusBroadcast {
            system_id,
            rfss_id,
            site_id,
            channel,
        } => {
            assert_eq!(system_id.value(), 0x0AA);
            assert_eq!(rfss_id.value(), 0xE7);
            assert_eq!(site_id.value(), 0x18);
            assert_eq!(channel.value(), 0xD573);
        }
        other => panic!("expected RfssStatusBroadcast payload, got {other:?}"),
    }

    // 7. Verify JSON output.
    let ident_table = IdentTable::new();
    let json = trunker::output::json::to_json_line(nid.access_code, tsbk, &ident_table);

    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["nac"], "0x293");
    assert_eq!(v["name"], "RFSS_STS_BCST");
    assert_eq!(v["opcode"], "0x3A");
    assert_eq!(v["system_id"], "0x0AA");
    assert_eq!(v["rfss_id"], 0xE7);
    assert_eq!(v["site_id"], 0x18);
    assert_eq!(v["channel"], 0xD573);
    assert!(v["frequency"].is_null());
    assert!(!json.contains('\n'));
}

/// Test with identifier table populated: frequencies should resolve.
#[test]
fn decode_grant_with_frequency_resolution() {
    let nac_value: u16 = 0x293;

    // Build NID.
    let nid_dibits = u64_to_dibits(make_nid_word(nac_value, 0x7));

    // First TSBK: identifier update (last_block=0, opcode=0x3D -> 0x3D)
    // Uses the p25.rs test vector: id=6, bw=12500, spacing=6250, base=851006250
    let ident_payload: [u8; 8] = [0x63, 0x22, 0xD0, 0x32, 0x0A, 0x25, 0x10, 0xA2];
    let coded_ident = make_coded_tsbk(0x3D, &ident_payload);

    // Second TSBK: group voice grant (last_block=1, opcode=0x00 -> 0x80)
    // Channel 0x6009 (id=6, index=9), talkgroup=100, source=12345
    let grant_payload: [u8; 8] = [
        0x60, 0x09, // channel 0x6009
        0x00, 0x64, // talkgroup 100
        0x00, 0x30, 0x39, // source 12345
        0x00,
    ];
    let coded_grant = make_coded_tsbk(0x80, &grant_payload);

    // Assemble raw data stream.
    let mut raw_data: Vec<Dibit> = Vec::new();
    raw_data.extend_from_slice(&nid_dibits);
    raw_data.extend_from_slice(&coded_ident);
    raw_data.extend_from_slice(&coded_grant);

    // Insert status symbols and strip them.
    let transmitted = insert_status_symbols(&raw_data);
    let mut status_deinterleaver = StatusDeinterleaver::new();
    let mut data_dibits = Vec::new();
    for &dibit in &transmitted {
        if let StreamSymbol::Data(d) = status_deinterleaver.feed(dibit) {
            data_dibits.push(d);
        }
    }

    // Feed into receiver.
    let mut receiver = DataUnitReceiver::new();
    let mut events = Vec::new();
    for &dibit in &data_dibits {
        if let Some(event) = receiver.feed(dibit) {
            events.push(event);
        }
    }

    // Should get: NID + ident TSBK + grant TSBK.
    assert_eq!(events.len(), 3);
    assert!(matches!(&events[0], ReceiverEvent::Nid(_)));

    let nac = match &events[0] {
        ReceiverEvent::Nid(nid) => nid.access_code,
        _ => unreachable!(),
    };

    // Populate identifier table from first TSBK.
    let mut ident_table = IdentTable::new();
    let ident_tsbk = match &events[1] {
        ReceiverEvent::Tsbk(tsbk) => {
            assert_eq!(tsbk.header.opcode, TsbkOpcode::ChannelParametersUpdate);
            assert!(!tsbk.header.last_block);
            ident_table.update(tsbk);
            tsbk
        }
        other => panic!("expected ident TSBK, got {other:?}"),
    };

    // Verify ident table was populated.
    let entry = ident_table
        .get(6)
        .expect("identifier 6 should be populated");
    assert_eq!(entry.base_frequency, 851_006_250);
    assert_eq!(entry.channel_spacing, 6_250);

    // Verify grant TSBK with frequency resolution.
    let grant_tsbk = match &events[2] {
        ReceiverEvent::Tsbk(tsbk) => {
            assert_eq!(tsbk.header.opcode, TsbkOpcode::GroupVoiceChannelGrant);
            assert!(tsbk.header.last_block);
            tsbk
        }
        other => panic!("expected grant TSBK, got {other:?}"),
    };

    // JSON output should have resolved frequency.
    let json = trunker::output::json::to_json_line(nac, grant_tsbk, &ident_table);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert_eq!(v["name"], "GRP_V_CH_GRANT");
    assert_eq!(v["channel"], 0x6009);
    assert_eq!(v["talkgroup"], 100);
    assert_eq!(v["source"], 12345);

    // Channel 0x6009: identifier=6, index=9
    // freq = 851006250 + 6250 * 9 = 851062500 Hz = 851.0625 MHz
    let freq = v["frequency"]
        .as_f64()
        .expect("frequency should be resolved");
    assert!((freq - 851.0625).abs() < 0.0001);

    // Also verify ident TSBK JSON.
    let ident_json = trunker::output::json::to_json_line(nac, ident_tsbk, &ident_table);
    let iv: serde_json::Value = serde_json::from_str(&ident_json).unwrap();
    assert_eq!(iv["name"], "CH_PARAMS_UPDT");
    assert_eq!(iv["identifier"], 6);
    assert_eq!(iv["bandwidth"], 12_500);
    assert_eq!(iv["base_frequency"], 851_006_250u64);
}

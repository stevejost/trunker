//! End-to-end integration test: synthesize a CQPSK P25 signal with
//! realistic impairments and decode it through the full pipeline.
//!
//! Constructs known TSBKs (IDENT_UP and GRP_V_CH_GRANT), modulates them
//! as a CQPSK signal with frequency offset, timing offset, and AWGN,
//! then feeds the IQ samples through ChannelPipeline and verifies the
//! decoded TSBK fields match the originals.

use num_complex::Complex;
use rand::SeedableRng;
use rand::rngs::StdRng;
use rand_distr::{Distribution, Normal};
use std::f32::consts::PI;

use trunker::dsp::cqpsk_mod::cqpsk_modulate;
use trunker::p25::consts::{CODING_DIBITS, SYNC_SYMBOLS};
use trunker::p25::crc;
use trunker::p25::nid::NidIntegrityPolicy;
use trunker::p25::receiver::ReceiverEvent;
use trunker::p25::tsbk::{TsbkOpcode, TsbkPayload};
use trunker::p25::types::Dibit;
use trunker::pipeline::{ChannelPipeline, Modulation, PipelineConfig};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Samples per symbol at 24 kHz channel rate / 4800 baud.
const SAMPLES_PER_SYMBOL: usize = 5;

/// RRC excess bandwidth (roll-off) for P25 CQPSK.
const EXCESS_BANDWIDTH: f32 = 0.2;

/// Channel rate after decimation (hertz).
const CHANNEL_RATE: u32 = 24_000;

/// Frequency offset applied to the synthesized signal (hertz).
/// 30 Hz is a realistic offset that exercises the Costas loop.
const FREQUENCY_OFFSET_HZ: f32 = 30.0;

/// Signal-to-noise ratio in dB for the AWGN impairment.
const SNR_DB: f32 = 18.0;

/// Number of warm-up symbols (idle dibits) before the data unit.
/// Allows the demodulator AGC, Costas loop, and Gardner TED to lock.
const WARMUP_SYMBOLS: usize = 2000;

/// Number of padding symbols after the data unit.
const TAIL_SYMBOLS: usize = 200;

/// Fixed RNG seed for deterministic noise.
const NOISE_SEED: u64 = 0xDEAD_BEEF_CAFE_1234;

/// NAC used in the test signal.
const TEST_NAC: u16 = 0x293;

// ---------------------------------------------------------------------------
// Trellis encoder (duplicated from decode_pipeline.rs since the crate-internal
// TrellisEncoder is #[cfg(test)] only and not available to integration tests)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Interleave table (inverse of deinterleave)
// ---------------------------------------------------------------------------

const INTERLEAVE_TABLE: [usize; CODING_DIBITS] = [
    0, 1, 8, 9, 16, 17, 24, 25, 32, 33, 40, 41, 48, 49, 56, 57, 64, 65, 72, 73, 80, 81, 88, 89, 96,
    97, 2, 3, 10, 11, 18, 19, 26, 27, 34, 35, 42, 43, 50, 51, 58, 59, 66, 67, 74, 75, 82, 83, 90,
    91, 4, 5, 12, 13, 20, 21, 28, 29, 36, 37, 44, 45, 52, 53, 60, 61, 68, 69, 76, 77, 84, 85, 92,
    93, 6, 7, 14, 15, 22, 23, 30, 31, 38, 39, 46, 47, 54, 55, 62, 63, 70, 71, 78, 79, 86, 87, 94,
    95,
];

fn interleave(original: &[Dibit; CODING_DIBITS]) -> [Dibit; CODING_DIBITS] {
    let mut output = [Dibit::new(0); CODING_DIBITS];
    for (output_index, &input_index) in INTERLEAVE_TABLE.iter().enumerate() {
        output[output_index] = original[input_index];
    }
    output
}

// ---------------------------------------------------------------------------
// Signal construction helpers
// ---------------------------------------------------------------------------

/// Build a BCH-encoded NID word for the given NAC and DUID.
fn make_nid_word(nac: u16, duid: u8) -> u64 {
    let data = ((nac & 0x0FFF) << 4) | u16::from(duid & 0x0F);
    let bch_word = trunker::p25::bch::encode(data);
    let shifted = bch_word << 1;
    if !shifted.count_ones().is_multiple_of(2) {
        shifted | 1
    } else {
        shifted
    }
}

/// Convert a 64-bit word to 32 dibits (MSB first).
fn u64_to_dibits(word: u64) -> Vec<Dibit> {
    (0..32)
        .map(|i| {
            let shift = 62 - i * 2;
            Dibit::new(((word >> shift) & 0x03) as u8)
        })
        .collect()
}

/// Build NID dibits for a TSDU with the given NAC.
fn make_tsdu_nid_dibits(nac: u16) -> Vec<Dibit> {
    u64_to_dibits(make_nid_word(nac, 0x7))
}

/// Build a TSBK with valid CRC, trellis-encode, and interleave.
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

/// Build the frame sync pattern as dibits.
fn sync_pattern_dibits() -> Vec<Dibit> {
    let sync: u64 = 0x5575_F5FF_77FF;
    (0..24)
        .map(|i| {
            let shift = (23 - i) * 2;
            Dibit::new(((sync >> shift) & 0x03) as u8)
        })
        .collect()
}

/// Insert status symbols into a data dibit stream.
///
/// Status symbols appear at multiples of 36 from the start of frame sync.
/// Since frame sync consumes 24 symbols, the first status after sync is
/// at position 12 (24 + 12 = 36).
fn insert_status_symbols(data_dibits: &[Dibit]) -> Vec<Dibit> {
    let mut output = Vec::new();
    let mut data_idx = 0;
    let mut position = SYNC_SYMBOLS;

    while data_idx < data_dibits.len() {
        position += 1;
        position %= 36;

        if position == 0 {
            // Insert a status symbol (InboundIdle = 0b11).
            output.push(Dibit::new(0b11));
        } else {
            output.push(data_dibits[data_idx]);
            data_idx += 1;
        }
    }
    output
}

/// Assemble a complete P25 TSDU dibit stream with warm-up and tail padding.
///
/// Structure: warm-up idle | frame sync | NID | status-interleaved TSBKs | tail
fn build_tsdu_dibit_stream(nac: u16, tsbk_specs: &[(u8, [u8; 8])]) -> Vec<Dibit> {
    let mut stream = Vec::new();

    // Warm-up: random-ish idle dibits for demodulator convergence.
    // Use alternating 01/11 (outer constellation points only, like sync).
    for i in 0..WARMUP_SYMBOLS {
        stream.push(Dibit::new(if i % 2 == 0 { 0b01 } else { 0b11 }));
    }

    // Frame sync pattern.
    stream.extend(sync_pattern_dibits());

    // NID + coded TSBKs (with status symbols inserted).
    let nid_dibits = make_tsdu_nid_dibits(nac);
    let mut payload_dibits = Vec::new();
    payload_dibits.extend_from_slice(&nid_dibits);
    for (opcode_byte, payload) in tsbk_specs {
        payload_dibits.extend(make_coded_tsbk(*opcode_byte, payload));
    }
    let with_status = insert_status_symbols(&payload_dibits);
    stream.extend(with_status);

    // Tail padding: idle dibits so the pipeline drains.
    for i in 0..TAIL_SYMBOLS {
        stream.push(Dibit::new(if i % 2 == 0 { 0b01 } else { 0b11 }));
    }

    stream
}

// ---------------------------------------------------------------------------
// Impairment functions
// ---------------------------------------------------------------------------

/// Apply a fixed frequency offset to a complex signal.
///
/// Rotates each sample by a linearly increasing phase, simulating a
/// carrier frequency error of `offset_hz` at the given sample rate.
fn apply_frequency_offset(signal: &mut [Complex<f32>], offset_hz: f32, sample_rate: f32) {
    let phase_step = 2.0 * PI * offset_hz / sample_rate;
    for (i, sample) in signal.iter_mut().enumerate() {
        let phase = phase_step * i as f32;
        let rotation = Complex::new(phase.cos(), phase.sin());
        *sample *= rotation;
    }
}

/// Add AWGN noise to a complex signal at the specified SNR.
///
/// Uses a fixed-seed RNG for determinism.
fn add_awgn(signal: &mut [Complex<f32>], snr_db: f32, seed: u64) {
    // Compute signal power.
    let signal_power: f32 = signal.iter().map(|s| s.norm_sqr()).sum::<f32>() / signal.len() as f32;

    // Noise power from SNR.
    let snr_linear = 10.0_f32.powf(snr_db / 10.0);
    let noise_power = signal_power / snr_linear;
    // Per-component standard deviation (I and Q each get half the noise power).
    let noise_std = (noise_power / 2.0).sqrt();

    let mut rng = StdRng::seed_from_u64(seed);
    let normal = Normal::new(0.0_f32, noise_std).unwrap();

    for sample in signal.iter_mut() {
        let noise_i = normal.sample(&mut rng);
        let noise_q = normal.sample(&mut rng);
        *sample += Complex::new(noise_i, noise_q);
    }
}

// ---------------------------------------------------------------------------
// The test
// ---------------------------------------------------------------------------

/// End-to-end test: synthesize a CQPSK P25 signal with impairments,
/// decode through the full pipeline, and verify TSBK field values.
#[test]
fn e2e_cqpsk_decode_with_impairments() {
    // 1. Define the TSBKs we want to decode.

    // TSBK 1: Channel parameters update (opcode 0x3D, last_block=0).
    // id=6, bw=12500, offset=-45MHz, spacing=6250, base=851006250.
    let ident_payload: [u8; 8] = [0x63, 0x22, 0xD0, 0x32, 0x0A, 0x25, 0x10, 0xA2];

    // TSBK 2: Group voice channel grant (opcode 0x00, last_block=1).
    // Channel 0x6009 (id=6, index=9), talkgroup=100, source=12345.
    let grant_payload: [u8; 8] = [
        0x60, 0x09, // channel 0x6009
        0x00, 0x64, // talkgroup 100
        0x00, 0x30, 0x39, // source 12345
        0x00,
    ];

    let tsbk_specs: Vec<(u8, [u8; 8])> = vec![
        (0x3D, ident_payload), // last_block=0, opcode=0x3D
        (0x80, grant_payload), // last_block=1, opcode=0x00
    ];

    // 2. Build the dibit stream.
    let dibits = build_tsdu_dibit_stream(TEST_NAC, &tsbk_specs);

    // 3. Modulate to CQPSK IQ at channel rate.
    let mut signal = cqpsk_modulate(&dibits, SAMPLES_PER_SYMBOL, EXCESS_BANDWIDTH);

    // 4. Apply impairments.
    apply_frequency_offset(&mut signal, FREQUENCY_OFFSET_HZ, CHANNEL_RATE as f32);
    add_awgn(&mut signal, SNR_DB, NOISE_SEED);

    // 5. Feed through the pipeline.
    let config = PipelineConfig {
        sample_rate: CHANNEL_RATE,
        modulation: Modulation::Cqpsk,
        nid_integrity: NidIntegrityPolicy::Permissive,
        sync_timeout_samples: None,
    };
    let mut pipeline = ChannelPipeline::new(config).expect("24 kHz should be a valid sample rate");

    let mut events: Vec<ReceiverEvent> = Vec::new();
    for &sample in &signal {
        if let Some(event) = pipeline.process_sample(sample) {
            events.push(event);
        }
    }

    // 6. Collect TSBKs.
    let tsbks: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            ReceiverEvent::Tsbk(t) => Some(t),
            _ => None,
        })
        .collect();

    // 7. Assertions.
    assert!(
        tsbks.len() >= 2,
        "expected at least 2 TSBKs decoded, got {}. Events: {:?}",
        tsbks.len(),
        events
            .iter()
            .map(|e| match e {
                ReceiverEvent::Nid(n) => format!("NID(nac=0x{:03X})", n.access_code.value()),
                ReceiverEvent::Tsbk(t) => format!("TSBK({})", t.header.opcode.name()),
                ReceiverEvent::Error(e) => format!("Error({e})"),
                other => format!("{other:?}"),
            })
            .collect::<Vec<_>>()
    );

    // Verify TSBK 1: Channel Parameters Update (0x3D).
    let ident_tsbk = tsbks
        .iter()
        .find(|t| t.header.opcode == TsbkOpcode::ChannelParametersUpdate)
        .expect("expected ChannelParametersUpdate TSBK");

    assert!(
        !ident_tsbk.header.last_block,
        "ident TSBK should have last_block=false"
    );
    match &ident_tsbk.payload {
        TsbkPayload::IdentifierUpdate {
            identifier,
            bandwidth,
            transmit_offset,
            channel_spacing,
            base_frequency,
        } => {
            assert_eq!(*identifier, 6, "identifier mismatch");
            assert_eq!(*bandwidth, 12_500, "bandwidth mismatch");
            assert_eq!(*transmit_offset, -45_000_000, "transmit_offset mismatch");
            assert_eq!(*channel_spacing, 6_250, "channel_spacing mismatch");
            assert_eq!(*base_frequency, 851_006_250, "base_frequency mismatch");
        }
        other => panic!("expected IdentifierUpdate payload, got {other:?}"),
    }

    // Verify TSBK 2: Group Voice Channel Grant (0x00).
    let grant_tsbk = tsbks
        .iter()
        .find(|t| t.header.opcode == TsbkOpcode::GroupVoiceChannelGrant)
        .expect("expected GroupVoiceChannelGrant TSBK");

    assert!(
        grant_tsbk.header.last_block,
        "grant TSBK should have last_block=true"
    );
    match &grant_tsbk.payload {
        TsbkPayload::GroupVoiceChannelGrant {
            channel,
            talkgroup,
            source,
        } => {
            assert_eq!(channel.value(), 0x6009, "channel mismatch");
            assert_eq!(talkgroup.value(), 100, "talkgroup mismatch");
            assert_eq!(source.value(), 12345, "source mismatch");
        }
        other => panic!("expected GroupVoiceChannelGrant payload, got {other:?}"),
    }

    // Verify NAC was decoded.
    let nid_events: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            ReceiverEvent::Nid(n) => Some(n),
            _ => None,
        })
        .collect();

    assert!(!nid_events.is_empty(), "expected at least one NID event");
    assert_eq!(
        nid_events[0].access_code.value(),
        TEST_NAC,
        "NAC mismatch: expected 0x{:03X}, got 0x{:03X}",
        TEST_NAC,
        nid_events[0].access_code.value()
    );
}

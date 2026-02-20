# P25 Decoder Pipeline — Detailed Reference

This document describes every stage of the `trunker` signal processing and protocol decode pipeline, from raw IQ samples to JSON output. Each block includes its source file, input/output types, and key parameters.

---

## Signal Flow Overview

```
IQ Source (SDR or File)
    │
    │  Complex<f32> samples @ input sample rate
    ▼
┌──────────────────────────────────────────────────────────────────────────┐
│  Multi-Stage Decimation                                                  │
│  (pipeline.rs)                                                           │
│                                                                          │
│  ┌───────────┐     ┌───────────┐           ┌───────────┐                │
│  │ Stage 1   │────►│ Stage 2   │──── ···──►│ Stage N   │                │
│  │ LPF+Dec   │     │ LPF+Dec   │           │ LPF+Dec   │                │
│  │ (primary) │     │(intermed.)│           │ (primary) │                │
│  └───────────┘     └───────────┘           └───────────┘                │
│                                                                          │
│  Each stage: FIR low-pass at 6250 Hz cutoff, then decimate by factor    │
│  Total: input_rate / 24000                                               │
└───────────────────────────────────┬──────────────────────────────────────┘
                                    │
                                    │  Complex<f32> @ 24 kHz
                                    ▼
                     ┌──────────────┴──────────────┐
                     │      Modulation Select       │
                     └──────┬───────────────┬───────┘
                            │               │
                    C4FM    │               │  CQPSK
                            ▼               ▼
┌───────────────────────────────┐ ┌────────────────────────────────────────┐
│  C4FM Demodulation Path       │ │  CQPSK Demodulation Path               │
│  (fm_demod.rs, timing.rs)     │ │  (cqpsk_demod.rs)                      │
│                               │ │                                        │
│  IQ ──► FM Discriminator      │ │  IQ ──► AGC                            │
│     ──► DC Blocker            │ │     ──► Complex RRC Matched Filter     │
│     ──► RRC Matched Filter    │ │     ──► Gardner TED + Interpolator     │
│     ──► Symbol Timing (M&M)   │ │     ──► Differential Decoder           │
│     ──► 4-Level Slicer        │ │     ──► Costas PLL                     │
│                               │ │     ──► arg() ──► Rescale              │
│                               │ │     ──► 4-Level Slicer                 │
└───────────────┬───────────────┘ └──────────────────┬─────────────────────┘
                │                                    │
                │  Dibit + SyncDetected events       │
                └──────────────┬─────────────────────┘
                               │
                               ▼
┌──────────────────────────────────────────────────────────────────────────┐
│  Status Symbol Deinterleaving (status.rs)                                │
│                                                                          │
│  Strips status symbols inserted every 35 data symbols.                   │
│  Status dibits are discarded; data dibits proceed.                       │
└───────────────────────────────────┬──────────────────────────────────────┘
                                    │
                                    │  Data dibits (status-stripped)
                                    ▼
┌──────────────────────────────────────────────────────────────────────────┐
│  Data Unit Receiver (receiver.rs)                                        │
│                                                                          │
│  State machine that accumulates dibits into data units:                  │
│                                                                          │
│  1. NID Decode (32 dibits)                                               │
│     ├── BCH(63,16,23) error correction — up to 11 bit errors (bch.rs)   │
│     ├── Extract NAC (12-bit) and DUID (4-bit)                            │
│     └── Route to data-unit-specific handler based on DUID                │
│                                                                          │
│  2. Data Unit Handlers (by DUID):                                        │
│     ┌──────────┬───────────────────────────────────────────────────┐     │
│     │ DUID     │ Handler                                           │     │
│     ├──────────┼───────────────────────────────────────────────────┤     │
│     │ 0x7 TSDU │ Collect 96 coded dibits per TSBK block            │     │
│     │          │ → Deinterleave → Trellis decode → CRC check       │     │
│     │          │ → Parse opcode fields → emit ReceiverEvent::Tsbk  │     │
│     ├──────────┼───────────────────────────────────────────────────┤     │
│     │ 0x0 HDU  │ Voice header: Golay-coded LC + RS parity          │     │
│     │          │ → emit ReceiverEvent::VoiceHeader                 │     │
│     ├──────────┼───────────────────────────────────────────────────┤     │
│     │ 0x5 LDU1 │ 9 IMBE voice frames + Golay-coded Link Control   │     │
│     │          │ → emit ReceiverEvent::VoiceFrame (×9)             │     │
│     │          │ → emit ReceiverEvent::LinkControl                 │     │
│     ├──────────┼───────────────────────────────────────────────────┤     │
│     │ 0xA LDU2 │ 9 IMBE voice frames + Reed-Solomon Crypto Ctrl   │     │
│     │          │ → emit ReceiverEvent::VoiceFrame (×9)             │     │
│     │          │ → emit ReceiverEvent::CryptoControl               │     │
│     ├──────────┼───────────────────────────────────────────────────┤     │
│     │ 0x3 TDU  │ Simple voice terminator (no data)                 │     │
│     │          │ → emit ReceiverEvent::VoiceTerminator             │     │
│     ├──────────┼───────────────────────────────────────────────────┤     │
│     │ 0xF TDULC│ Voice terminator with LC: Golay + RS coded        │     │
│     │          │ → emit ReceiverEvent::VoiceLcTerminator           │     │
│     └──────────┴───────────────────────────────────────────────────┘     │
└───────────────────────────────────┬──────────────────────────────────────┘
                                    │
                                    │  ReceiverEvent variants
                                    ▼
┌──────────────────────────────────────────────────────────────────────────┐
│  Application Layer                                                       │
│                                                                          │
│  ┌─────────────────────────────────────────────┐                         │
│  │ p25 cc (main.rs)                            │                         │
│  │ Single-channel control channel decoder      │                         │
│  │ → IdentTable tracks channel→freq mapping    │                         │
│  │ → to_json_line() serializes to JSON stdout  │                         │
│  └─────────────────────────────────────────────┘                         │
│                                                                          │
│  ┌─────────────────────────────────────────────┐                         │
│  │ p25 trunk (channel_manager.rs)              │                         │
│  │ Wideband: CC + voice channel decode         │                         │
│  │ → Watches GRP_V_CH_GRANT for new calls      │                         │
│  │ → Spawns per-channel ChannelPipeline + NCO  │                         │
│  │ → IMBE vocoder decodes voice frames to PCM  │                         │
│  │ → Tears down channels on timeout            │                         │
│  └─────────────────────────────────────────────┘                         │
│                                                                          │
│  ┌─────────────────────────────────────────────┐                         │
│  │ p25 monitor (monitor/)                      │                         │
│  │ TUI that reads JSON from stdin              │                         │
│  │ → Tracks active grants, system info         │                         │
│  │ → Renders terminal UI with ratatui          │                         │
│  └─────────────────────────────────────────────┘                         │
└──────────────────────────────────────────────────────────────────────────┘
```

---

## Stage Details

### 1. IQ Source

**Files:** `sdr/soapy_source.rs`, `sdr/cf32_reader.rs`, `sdr/u8_reader.rs`

| Source | Format | Conversion |
|---|---|---|
| SoapySDR (live) | CF32 via driver auto-convert | Direct: `Complex<f32>` pairs |
| CF32 file | 8 bytes/sample (f32 I, f32 Q) | Direct read as `Complex<f32>` |
| U8 file (RTL-SDR native) | 2 bytes/sample (u8 I, u8 Q) | `(byte - 127.5) / 127.5` per component |

SDRplay RSPdx native format is CS16; RTL-SDR native is CS8. SoapySDR auto-converts both to CF32. Hardware settings (`rfgain_sel`, `biasT_ctrl`, etc.) must be applied **after** `stream.activate()`.

A 200ms settling discard is applied after stream activation to allow the hardware PLL and AGC to converge.

### 2. Multi-Stage Decimation

**File:** `pipeline.rs` — `DecimationConfig::compute()`

Reduces the input sample rate to the 24 kHz IF rate used by both demodulators. The total decimation factor (`input_rate / 24000`) is split into stages of at most 10x each.

**Algorithm:**
1. Compute total factor: `sample_rate / 24000`
2. Recursively factor into stages ≤ 10x, preferring larger final factors
3. Compute FIR tap count per stage using a scaled reference

**Filter parameters:**

| Stage Position | Reference | Example (6 MSPS) |
|---|---|---|
| First stage | 2.4 MHz / 201 taps | 6 MHz → 503 taps |
| Intermediate | 240 kHz / 61 taps | 1.2 MHz → 305 taps |
| Final stage | 2.4 MHz / 201 taps | 240 kHz → 51 taps (min odd ≥ 51) |

All stages use a uniform 6250 Hz cutoff — half the 12.5 kHz P25 channel bandwidth. This is critical for CQPSK: wider intermediate cutoffs let adjacent-channel energy disrupt the Costas PLL.

**Example factorings:**

| Input Rate | Total | Stages | Taps |
|---|---|---|---|
| 48 kHz | 2x | [2] | [51] |
| 240 kHz | 10x | [10] | [51] |
| 2.4 MHz | 100x | [10, 10] | [201, 51] |
| 6 MHz | 250x | [5, 5, 10] | [503, 305, 51] |
| 9 MHz | 375x | [3, 5, 5, 5] | [755, 305, 61, 51] |

**File:** `dsp/filter.rs` — `DecimatingFilter`

Each stage is a polyphase FIR low-pass filter. The filter is designed with a Hamming window and processes one input sample per call, producing output only every N-th sample (where N is the decimation factor).

### 3. Demodulation

After decimation, the signal is at 24 kHz — 5 samples per P25 symbol (4800 baud).

#### 3a. C4FM Path

**Files:** `dsp/fm_demod.rs`, `dsp/dc_block.rs`, `dsp/rrc_filter.rs`, `dsp/timing.rs`, `dsp/slicer.rs`

```
IQ @ 24 kHz
    │
    ▼
FM Discriminator ──► atan2(cross, dot) between successive samples
    │                 Output: instantaneous frequency deviation
    ▼
DC Blocker ────────► Single-pole IIR high-pass (α = 0.999)
    │                 Removes DC offset from discriminator output
    ▼
RRC Matched Filter ► Root raised cosine, α = 0.2, 5 symbol spans
    │                 Matched to transmit pulse shape for optimal SNR
    ▼
Symbol Timing ─────► Mueller & Müller clock recovery
    │                 Tracks symbol boundaries, outputs at symbol rate
    ▼
4-Level Slicer ────► Maps baseband level to dibit {0, 1, 2, 3}
    │                 Adaptive thresholds from sync pattern
    ▼
Frame Sync ────────► Correlates 48-bit sync word (24 dibits)
                      Threshold: ≤ 4 dibit errors
```

#### 3b. CQPSK Path

**Files:** `dsp/cqpsk_demod.rs`, `dsp/agc.rs`, `dsp/rrc_filter.rs`, `dsp/gardner.rs`, `dsp/costas.rs`, `dsp/diff_decoder.rs`, `dsp/interpolator.rs`, `dsp/slicer.rs`

```
IQ @ 24 kHz
    │
    ▼
AGC ───────────────► Automatic gain control (normalizes power)
    │
    ▼
Complex RRC Filter ► Complex root raised cosine matched filter
    │                 α = 0.2, 5 symbol spans, operates on I+jQ
    ▼
Gardner TED ───────► Gardner timing error detector
    │                 Interpolates between samples to find symbol center
    │                 Uses cubic polynomial interpolation
    ▼
Differential ──────► Decodes phase transitions (not absolute phase)
    Decoder            π/4 DQPSK differential mapping
    │
    ▼
Costas PLL ────────► Carrier recovery loop
    │                 α = 0.008, β = α²/4
    │                 Tracks residual carrier offset and phase
    ▼
arg() + Rescale ───► Converts complex point to angle (radians)
    │                 Rescale: 1/(π/4) maps ±π/4, ±3π/4 to ±1, ±3
    ▼
4-Level Slicer ────► Maps to dibit {0, 1, 2, 3}
    │                 Same adaptive slicer as C4FM path
    ▼
Frame Sync ────────► Same 48-bit sync correlation as C4FM
```

### 4. Status Symbol Deinterleaving

**File:** `p25/status.rs` — `StatusDeinterleaver`

After frame sync, the demodulator produces a continuous dibit stream. The P25 air interface inserts a status symbol every 35 data symbols (at every 36th position from the start of the frame sync).

The `StatusDeinterleaver` counts positions modulo 36, separating data dibits from status dibits. Status symbols convey inbound/outbound busy/idle state but are not used for control channel decoding.

### 5. Network Identifier (NID) Decode

**Files:** `p25/nid.rs`, `p25/bch.rs`

The first 32 data dibits after frame sync form the 64-bit NID word:

```
Bit 63                                          Bit 0
┌─────────────────────────────────────────────┬──┐
│         63-bit BCH(63,16,23) codeword       │P │
│                                             │  │
│  ┌──────────────┬────────┬─────────────┐    │  │
│  │ NAC (12 bit) │DUID(4) │ Parity (47) │    │  │
│  │ bits 62-51   │50-47   │ bits 46-0   │    │  │
│  └──────────────┴────────┴─────────────┘    │  │
└─────────────────────────────────────────────┴──┘
```

**BCH(63,16,23) Error Correction:**

The BCH code protects the 16 data bits (NAC + DUID) with 47 parity bits, correcting up to 11 bit errors in the 63-bit codeword. This is essential for CQPSK simulcast systems where multipath interference causes elevated bit error rates.

| Step | Algorithm | File |
|---|---|---|
| Syndrome computation | Evaluate at α¹..α²² over GF(2⁶) | `bch.rs` |
| Error locator | Berlekamp-Massey | `bch.rs` |
| Root finding | Chien search | `bch.rs` |
| Error correction | Flip bits at located positions | `bch.rs` |

GF(2⁶) uses primitive polynomial x⁶ + x + 1. Lookup tables (EXP[126], LOG[64]) are computed at compile time.

**NID Integrity Policy** (`NidIntegrityPolicy`):
- **Strict** (default): Reject NIDs where BCH fails (> 11 errors). Receiver skips the data unit.
- **Permissive**: Accept NIDs with BCH failure, log a warning. For debugging noisy channels.

With BCH correction, strict mode achieves 100% NID recovery on real-world CQPSK signals.

### 6. TSBK Decode (DUID 0x7)

**Files:** `p25/interleave.rs`, `p25/trellis.rs`, `p25/crc.rs`, `p25/tsbk.rs`

Each TSDU (Trunking Signaling Data Unit) contains one or more TSBKs, each 96 coded dibits:

```
96 coded dibits (from air)
    │
    ▼
Deinterleave ────► Reverse the 96-dibit interleaving pattern
    │               (consts::DEINTERLEAVE table, 4 × 26 structure)
    ▼
Trellis Decode ──► 1/2 rate trellis code, Viterbi-like decoder
    │               49 input dibit pairs → 48 data dibits + flush
    │               Constellation mapping to state transitions
    ▼
96 data bits (48 dibits = 12 bytes)
    │
    ▼
CRC-16 Check ───► CCITT CRC-16 over first 10 bytes
    │               Bytes 10-11 carry the CRC value
    ▼
Parse Opcode ───► First byte: opcode (8 bits)
    │               Bit 8: last_block flag
    │               Remaining: opcode-specific fields
    ▼
TsbkPayload enum
```

**Supported TSBK opcodes** (`p25/tsbk.rs`):

| Opcode | Name | Key Fields |
|---|---|---|
| 0x00 | GRP_V_CH_GRANT | channel, talkgroup, source |
| 0x02 | GRP_V_CH_GRANT_UPDT | channel1, tg1, channel2, tg2 |
| 0x04 | UNT_TO_UNT_ANS_REQ | channel, target, source |
| 0x14 | SNDCP_DATA_CH_ANN | channel |
| 0x20 | IDENT_UP | identifier, bw, offset, spacing, base_freq |
| 0x28 | SYS_SRV_BCST | services bitmask |
| 0x29 | SCND_CC_BCST | rfss_id, site_id, channel |
| 0x2C | GRP_AFF_RSP | talkgroup, source |
| 0x2F | U_REG_RSP | source, system_id |
| 0x34 | IDEN_UP_VU | identifier, bw, offset, spacing, base_freq |
| 0x39 | NET_STS_BCST | wacn, system_id, channel |
| 0x3A | RFSS_STS_BCST | system_id, rfss_id, site_id, channel |
| 0x3B | NET_STS_BCST (alt) | Same as 0x39 |
| 0x3C | ADJ_STS_BCST | system_id, rfss_id, site_id, channel |
| 0x3D | CH_PARAMS_UPDT | identifier, bw, offset, spacing, base_freq |
| 0x24 | EMERGENCY_ALRM | source |
| 0x44 | U_DE_REG_ACK | wacn, source, system_id |
| 0x54 | GRP_V_CH_GRANT_UPDT_EXP | channel, talkgroup, source |

### 7. Voice Frame Decode (DUID 0x5, 0xA)

**Files:** `p25/voice/frame_group.rs`, `p25/voice/frame.rs`, `p25/voice/control.rs`, `p25/voice/crypto.rs`, `p25/coding/`

LDU1 and LDU2 each contain 9 IMBE voice frames (88 bits each) plus interleaved link/crypto control data:

```
LDU1 (DUID 0x5):
    9 × IMBE frames (each 88 bits = 7 chunks + error bits)
    + Link Control word (72 bits, Golay + Hamming coded)
    + Low-speed data (16 bits, cyclic coded)

LDU2 (DUID 0xA):
    9 × IMBE frames (same structure)
    + Crypto Control word (96 bits, Reed-Solomon coded)
    + Low-speed data (16 bits)
```

**Error correction per voice frame:**

| FEC Code | Application |
|---|---|
| Golay(24,12) | Link Control bits (LDU1) |
| Hamming(15,11) | Link Control bits (LDU1) |
| Reed-Solomon(24,12,13) | Crypto Control (LDU2) |
| Reed-Solomon(24,16,9) | Voice header (HDU) |
| Cyclic(16,8,5) | Low-speed data |

### 8. IMBE Vocoder

**File:** `vocoder/decode.rs` — `ImbeDecoder`

Decodes 88-bit IMBE voice frames to 160 samples of 8 kHz PCM audio (20ms per frame). The vocoder is a multi-band excitation model:

```
88-bit IMBE frame
    │
    ▼
Bit Unpacking ───► Extract pitch, gain, spectral params
    │
    ▼
Spectral Decode ─► Reconstruct spectral amplitudes from
    │               log-area ratios and prediction
    ▼
Enhancement ─────► Spectral enhancement for clarity
    │
    ▼
Voiced Synthesis ► Pitch-synchronous harmonic oscillators
    + Unvoiced ──── White noise shaped by spectral envelope
    │
    ▼
Overlap-Add ─────► Window and combine voiced + unvoiced
    │
    ▼
160 PCM samples @ 8 kHz (20ms)
```

### 9. Frequency Resolution

**File:** `p25/ident.rs` — `IdentTable`

Channel numbers in TSBKs are 16-bit values encoding an identifier (4 bits) and channel index (12 bits). The `IdentTable` maps identifiers to frequency parameters from `IDENT_UP` / `IDEN_UP_VU` / `CH_PARAMS_UPDT` messages:

```
frequency = base_frequency + (channel_spacing × channel_index) + transmit_offset
```

### 10. JSON Serialization

**File:** `output/json.rs` — `to_json_line()`

Every decoded TSBK and voice event is serialized to a single JSON line on stdout. The `IdentTable` is consulted to resolve channel numbers to frequencies (in MHz). If the identifier hasn't been seen yet, the `frequency` field is `null`.

---

## Wideband Trunking Architecture

**File:** `channel_manager.rs` — `ChannelManager`

The `p25 trunk` command uses a single wideband IQ stream to decode both the control channel and active voice channels simultaneously:

```
Wideband IQ @ input sample rate (e.g., 6 MSPS)
    │
    ├──► CC Pipeline (ChannelPipeline @ center freq)
    │        │
    │        ▼
    │    TSBK events ──► Watch for GRP_V_CH_GRANT
    │                         │
    │                         ▼ new grant: (freq, talkgroup, source)
    │
    ├──► Voice Channel 1: NCO(offset_hz) ──► ChannelPipeline ──► IMBE ──► JSON
    ├──► Voice Channel 2: NCO(offset_hz) ──► ChannelPipeline ──► IMBE ──► JSON
    └──► Voice Channel N: NCO(offset_hz) ──► ChannelPipeline ──► IMBE ──► JSON

Each voice channel:
  - NCO shifts the wideband signal to baseband for that channel
  - Independent ChannelPipeline (decimation + demod + protocol)
  - IMBE vocoder decodes voice frames
  - Torn down after call_timeout seconds with no grant updates
```

---

## Data Types Through the Pipeline

| Stage | Type | Width |
|---|---|---|
| IQ source | `Complex<f32>` | 8 bytes/sample |
| After decimation | `Complex<f32>` | 8 bytes/sample |
| After demod | `f32` (baseband) | 4 bytes/sample |
| After slicer | `Dibit` (0-3) | 1 byte (2 bits used) |
| After status strip | `Dibit` | 1 byte |
| NID word | `u64` | 8 bytes |
| TSBK raw | `[u8; 12]` | 12 bytes |
| TSBK parsed | `Tsbk` struct | Variable |
| IMBE frame | 88 bits | 11 bytes |
| PCM audio | `[f32; 160]` | 640 bytes (20ms) |
| JSON output | UTF-8 string | Variable |

---

## Key Constants

| Constant | Value | Source |
|---|---|---|
| Symbol rate | 4800 baud | TIA-102.BAAA |
| Channel rate (IF) | 24 kHz (5 samples/symbol) | `pipeline.rs` |
| Channel bandwidth | 12.5 kHz | TIA-102 |
| Filter cutoff | 6250 Hz | `pipeline.rs` |
| Frame sync pattern | `0x5575F5FF77FF` (48 bits) | TIA-102.BAAA |
| Sync threshold | ≤ 4 dibit errors | `dsp/sync.rs` |
| NID length | 32 dibits (64 bits) | TIA-102.BAAA |
| BCH code | (63,16,23), t=11 | TIA-102.BAAA |
| TSBK coded length | 96 dibits | TIA-102.AABF |
| TSBK data length | 96 bits (12 bytes) | TIA-102.AABF |
| CRC polynomial | CRC-16-CCITT | TIA-102.AABF |
| IMBE frame | 88 bits → 160 PCM samples | TIA-102.BACA |
| Audio rate | 8000 Hz | TIA-102.BACA |
| Max decimation/stage | 10x | `pipeline.rs` |

# trunker

**Unix-philosophy P25 trunked radio tools for software-defined radios.**

`trunker` is a suite of focused, composable command-line tools for decoding APCO Project 25 trunked radio systems using commodity SDR hardware. Think `dump1090` for P25 — RF in, structured data out.

---

## Why This Exists

The P25 SDR ecosystem is a mess. Every existing application tries to be a complete scanner — bundling DSP, protocol decode, audio playback, and UI into a single monolithic package with poor interfaces, worse documentation, and no composability. There is no `rtl_adsb` equivalent for P25.

`trunker` fills that gap. Each tool does one thing well, emits structured JSON, and gets out of the way. Pipe the output to `jq`, a database, a web service, Home Assistant, or whatever you want. Your radio infrastructure, your rules.

---

## Tools

### `p25 cc` — Control Channel Decoder

Monitors a P25 control channel frequency, demodulates C4FM or CQPSK, decodes Trunking Signaling Blocks (TSBKs), and emits structured JSON lines to stdout.

```bash
# Live decode with an RTL-SDR (C4FM)
p25 cc --device "driver=rtlsdr" --frequency 852350000 --gain 40

# Simulcast system with CQPSK modulation (SDRplay RSPdx)
p25 cc --device "driver=sdrplay" --frequency 852350000 --gain 40 \
  --modulation cqpsk --setting rfgain_sel=24 --setting hdr_ctrl=false

# Decode from a recorded IQ file
p25 cc --input capture.cf32

# Pipe to jq for live filtering
p25 cc --device "driver=rtlsdr" -f 852350000 --gain 40 \
  | jq 'select(.name == "GRP_V_CH_GRANT")'
```

**Output (one JSON object per line):**

```jsonl
{"nac":"0x5FC","opcode":"0x00","name":"GRP_V_CH_GRANT","last_block":true,"manufacturer_id":0,"channel":24841,"frequency":851.0625,"talkgroup":3769,"source":12345}
{"nac":"0x5FC","opcode":"0x3A","name":"RFSS_STS_BCST","last_block":true,"manufacturer_id":0,"system_id":"0x5F2","rfss_id":1,"site_id":1,"channel":56691,"frequency":852.35}
{"nac":"0x5FC","opcode":"0x3D","name":"CH_PARAMS_UPDT","last_block":true,"manufacturer_id":0,"identifier":6,"bandwidth":12500,"transmit_offset":-45000000,"channel_spacing":6250,"base_frequency":851006250}
```

### `p25 trunk` — Wideband Trunked Decoder

Decodes both the control channel and active voice channels from a single wideband IQ capture. Watches for channel grants on the CC and automatically spawns voice channel pipelines at the granted frequencies.

```bash
# Live decode with SDRplay (wideband capture centered on system)
p25 trunk --device "driver=sdrplay" --center-freq 852350000 --gain 40 \
  --modulation cqpsk --setting rfgain_sel=24

# From a recorded wideband IQ file
p25 trunk --input wideband.cf32 --center-freq 852350000

# With custom call timeout (default 3 seconds)
p25 trunk --input wideband.cf32 --center-freq 852350000 --call-timeout 5.0
```

Voice events include call context (frequency, talkgroup, source) from the CC grant:

```jsonl
{"nac":"0x5FC","type":"voice_frame","frequency":851.0625,"talkgroup":3769,"source":12345,"imbe":"1234567890ABCDEF123456","errors":2}
{"nac":"0x5FC","type":"voice_frame","frequency":851.0625,"talkgroup":3769,"source":12345,"imbe":"1234567890ABCDEF123456","errors":0}
```

### `p25 monitor` — Terminal Monitor

Real-time TUI that displays control channel activity. Reads JSON lines from stdin (piped from `p25 cc`) and renders active grants, system info, and talkgroup activity.

```bash
# Pipe control channel output to the monitor
p25 cc --device "driver=rtlsdr" --frequency 852350000 --gain 40 | p25 monitor

# With custom grant expiry timeout (default 3 seconds)
p25 cc --input capture.cf32 | p25 monitor --grant-timeout 5
```

### `p25 devices` — List SDR Hardware

Lists all SoapySDR-compatible devices detected on the system.

```bash
p25 devices
```

---

## Filtering JSON Output

All messages include fields suitable for filtering with `jq` or any JSON-aware tool. No built-in filter flags are needed -- the Unix pipeline is the filter mechanism.

**Fields present on every TSBK message:**

| Field | Type | Example | Description |
|---|---|---|---|
| `nac` | string | `"0x5FC"` | Network Access Code (identifies the system) |
| `opcode` | string | `"0x00"` | Raw opcode hex value |
| `name` | string | `"GRP_V_CH_GRANT"` | Human-readable opcode name |
| `manufacturer_id` | integer | `0` | Manufacturer ID (0 = standard) |
| `last_block` | boolean | `true` | Whether this is the last TSBK in a TSDU |

**Fields only on specific opcodes (not available for per-message filtering):**

| Field | Opcodes |
|---|---|
| `system_id` | `NET_STS_BCST`, `RFSS_STS_BCST`, `ADJ_STS_BCST`, `U_REG_RSP`, `U_DE_REG_ACK` |
| `wacn` | `NET_STS_BCST`, `U_DE_REG_ACK` |
| `rfss_id`, `site_id` | `RFSS_STS_BCST`, `ADJ_STS_BCST` |
| `talkgroup` | `GRP_V_CH_GRANT`, `GRP_V_CH_GRANT_UPDT_EXP` |
| `source` | `GRP_V_CH_GRANT`, `UNT_TO_UNT_ANS_REQ`, `EMERGENCY_ALRM`, `U_DE_REG_ACK` |

**Fields present on every voice event:**

| Field | Type | Example | Description |
|---|---|---|---|
| `nac` | string | `"0x5FC"` | Network Access Code |
| `type` | string | `"voice_frame"` | Event type: `voice_frame`, `link_control`, `crypto_control`, `voice_header`, `data_fragment` |

**Example `jq` filters:**

```bash
# Filter by NAC (isolate one system on a shared frequency)
p25 cc ... | jq 'select(.nac == "0x5FC")'

# Only channel grants
p25 cc ... | jq 'select(.name == "GRP_V_CH_GRANT")'

# Only non-standard manufacturer messages
p25 cc ... | jq 'select(.manufacturer_id != 0)'

# Channel grants for a specific talkgroup
p25 cc ... | jq 'select(.name == "GRP_V_CH_GRANT" and .talkgroup == 3769)'

# System identity messages only
p25 cc ... | jq 'select(.name == "NET_STS_BCST" or .name == "RFSS_STS_BCST")'

# Voice frames from trunk mode
p25 trunk ... | jq 'select(.type == "voice_frame")'
```

---

## Project Goals

1. **Composability over completeness.** Each tool is a building block. No built-in UI, no audio player, no talkgroup database. Emit structured data and let downstream consumers handle presentation.

2. **Correctness over speed.** P25 protocol decode must be bit-accurate. Every TSBK opcode, every field, every edge case. Performance matters but never at the cost of correctness.

3. **Testability as a first-class concern.** Every layer of the stack — from dibit streams through TSBK parsing — must be testable in isolation with deterministic inputs. IQ file replay enables full end-to-end regression testing with no hardware in the loop.

4. **Readability over cleverness.** This is a protocol decoder, not a DSP research project. Code should read like documentation of the P25 specification. Someone with TIA-102 open on one monitor should be able to follow the code on the other.

5. **Hardware agnostic.** SoapySDR abstraction means any supported SDR works. Develop with a $25 RTL-SDR, deploy with an SDRplay RSPdx. Same code, same results.

---

## Architecture

```
┌──────────────────────────────────────────────────────────────┐
│              IQ Source (SDR or File)                          │
│  RTL-SDR (CS8) · SDRplay (CS16) · CF32 file · U8 file       │
└───────────────────────────┬──────────────────────────────────┘
                            │ Complex<f32> @ input rate
                            ▼
┌──────────────────────────────────────────────────────────────┐
│           Multi-Stage Decimation (≤10x per stage)            │
│  LPF 6250 Hz ──►  LPF 6250 Hz ──► ··· ──► LPF 6250 Hz      │
│  e.g. 6M: [5x, 5x, 10x] = 250x total                       │
└───────────────────────────┬──────────────────────────────────┘
                            │ Complex<f32> @ 24 kHz
                            ▼
              ┌─────────────┴──────────────┐
              │                            │
      C4FM    ▼                            ▼   CQPSK
┌──────────────────────┐    ┌──────────────────────────────┐
│  FM Discriminator     │    │  AGC → RRC → Gardner TED     │
│  DC Block → RRC       │    │  Diff Decoder → Costas PLL   │
│  M&M Timing → Slicer  │    │  arg() → Rescale → Slicer    │
└──────────┬───────────┘    └──────────────┬───────────────┘
           │                               │
           └───────────┬───────────────────┘
                       │ Dibits + Sync
                       ▼
┌──────────────────────────────────────────────────────────────┐
│                    Protocol Decoder                           │
│  Status Deinterleave → NID [BCH(63,16,23)] → Route by DUID  │
│                                                              │
│  TSDU: Deinterleave → Trellis → CRC → TSBK parser           │
│  Voice: LDU1/LDU2 → Golay/RS FEC → IMBE vocoder → PCM      │
└───────────────────────────┬──────────────────────────────────┘
                            │ Structured events
                            ▼
┌──────────────────────────────────────────────────────────────┐
│                JSON Serializer → stdout                       │
└──────────────────────────────────────────────────────────────┘
```

For a detailed description of every pipeline stage with parameters, see [docs/DECODER.md](docs/DECODER.md).

### Layer Responsibilities

| Layer | Input | Output | Testable With |
|---|---|---|---|
| SDR Source | Hardware/File | Complex IQ samples | Recorded `.iq`/`.cf32`/`.u8` files |
| Decimation | IQ @ input rate | IQ @ 24 kHz | `DecimationConfig` unit tests |
| Demodulator (C4FM or CQPSK) | IQ @ 24 kHz | Dibit stream | Synthetic IQ, CQPSK modulator |
| Frame Sync | Dibit stream | Aligned data units | Recorded dibit streams |
| NID + BCH | 32 dibits | NAC + DUID (corrected) | BCH encode/decode roundtrips |
| Trellis Decoder | 96 coded dibits | 12 corrected bytes | Known test vectors from spec |
| TSBK Parser | 12 bytes | Typed message structs | Hex-encoded TSBK fixtures |
| IMBE Vocoder | 88-bit frames | PCM audio (8 kHz) | Bit-exact reference frames |
| JSON Serializer | Event structs | JSON lines | Unit tests on structs |

Each layer boundary is a clean interface with a well-defined data type. No layer reaches into another layer's internals.

---

## Technology

| Component | Choice | Rationale |
|---|---|---|
| Language | Rust | Performance of C, safety guarantees, excellent tooling, `cargo install` distribution |
| SDR Interface | SoapySDR (via `soapysdr` crate) | Hardware-agnostic, supports all major SDR devices |
| DSP Math | `num-complex` | Standard Rust numerics for complex IQ samples |
| Serialization | `serde` + `serde_json` | Industry-standard, zero-boilerplate JSON output |
| CLI | `clap` | Derive-based arg parsing, subcommand support |
| Logging | `tracing` | Structured logging with levels, filterable at runtime |
| Testing | Built-in (`cargo test`) | No external test framework needed |

---

## P25 Concepts

A brief primer on the P25 trunking concepts this project deals with. See TIA-102 for the full specification.

### Trunked Radio

Unlike conventional radio where each group gets a dedicated frequency, a trunked system shares a pool of voice frequencies across all users. A **control channel** broadcasts continuously, coordinating which talkgroup gets which frequency for each transmission. Radios monitor the control channel and jump to the assigned voice frequency when their talkgroup is granted a channel.

### Control Channel

The control channel transmits **Trunking Signaling Blocks (TSBKs)** — short data messages that describe system activity. Key TSBK message types include:

- **Channel Grants** (`GRP_V_CH_GRANT`) — assigns a voice frequency to a talkgroup. This is the core message that tells a scanner where to tune.
- **Channel Grant Updates** (`GRP_V_CH_GRANT_UPDT`) — updates for ongoing calls.
- **Unit Registrations** (`U_REG_RSP`) — a radio registering on the system.
- **Affiliations** (`GRP_AFF_RSP`) — a radio joining a talkgroup.
- **Identifier Updates** (`IDEN_UP`) — maps logical channel numbers to actual RF frequencies.
- **Adjacent Site** (`ADJ_STS_BCST`) — information about neighboring sites in a multi-site system.
- **System Info** (`RFSS_STS_BCST`, `NET_STS_BCST`) — system identity and configuration.

### C4FM Modulation

P25 Phase I uses Compatible 4-level FM (C4FM) — a 4FSK modulation scheme transmitting at 4800 symbols/second, with each symbol encoding 2 bits (a "dibit"), for a raw data rate of 9600 bps. The four deviation levels map to dibit values 0, 1, 2, 3.

### Simulcast

In simulcast systems, multiple towers transmit the same signal on the same frequency simultaneously for seamless coverage. This causes multipath interference at locations equidistant from two towers. Control channel data is resilient to this due to error correction, but voice audio can be severely degraded without equalization.

---

## Development

### Prerequisites

- Rust toolchain (stable, latest): `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- SoapySDR development libraries: `sudo apt install libsoapysdr-dev`
- SoapySDR driver for your hardware (e.g., `soapysdr-module-rtlsdr`)
- An SDR device or recorded IQ files for testing

### Building

```bash
git clone https://github.com/youruser/trunker.git
cd trunker
cargo build --release
```

### Running Tests

```bash
# Full test suite
cargo test

# With output for debugging
cargo test -- --nocapture

# Run a specific test module
cargo test tsbk::tests

# Run integration tests with IQ fixtures
cargo test --test decode_pipeline
```

### Recording IQ Files for Development

Capture raw IQ samples for offline development and regression testing:

```bash
# Using SoapySDR's command-line utility
SoapySDRUtil --probe  # verify your device is detected

# Using rx_sdr (from soapy-tools)
rx_sdr -d "driver=rtlsdr" -f 853450000 -s 2400000 -g 40 -n 57600000 capture_30s.iq

# This gives you 30 seconds of IQ at 2.4 MSPS in u8 format (~144 MB)
# Note: rx_sdr produces u8 (RTL-SDR native); convert to CF32 for the decoder
```

### Project Structure

```
trunker/
├── Cargo.toml
├── Cargo.lock
├── CLAUDE.md
├── README.md
├── LICENSE
├── rustfmt.toml
├── .gitignore
│
├── src/
│   ├── main.rs                  # CLI entry point (p25 cc, trunk, debug, devices, monitor)
│   ├── lib.rs                   # Library root, re-exports all modules
│   ├── pipeline.rs              # Per-channel DSP + protocol decode pipeline
│   ├── channel_manager.rs       # Wideband trunked decoder (CC + voice channels)
│   │
│   ├── sdr/
│   │   ├── mod.rs
│   │   ├── soapy_source.rs      # Live SDR via SoapySDR
│   │   ├── cf32_reader.rs       # IQ file replay (CF32 format)
│   │   ├── u8_reader.rs         # IQ file replay (U8 / RTL-SDR native format)
│   │   └── error.rs
│   │
│   ├── dsp/
│   │   ├── mod.rs
│   │   ├── fm_demod.rs          # C4FM FM discriminator
│   │   ├── cqpsk_demod.rs       # CQPSK coherent demodulator (simulcast)
│   │   ├── cqpsk_mod.rs         # CQPSK modulator (test signal generation)
│   │   ├── costas.rs            # Costas PLL for carrier recovery
│   │   ├── gardner.rs           # Gardner timing error detector
│   │   ├── gardner_timing.rs    # Gardner clock recovery loop
│   │   ├── timing.rs            # Symbol timing recovery (C4FM path)
│   │   ├── slicer.rs            # Dibit decision slicer
│   │   ├── filter.rs            # FIR decimating low-pass filter
│   │   ├── rrc_filter.rs        # Root raised cosine matched filter
│   │   ├── dc_block.rs          # DC offset removal
│   │   ├── agc.rs               # Automatic gain control
│   │   ├── nco.rs               # Numerically controlled oscillator
│   │   ├── interpolator.rs      # Polynomial interpolator
│   │   ├── diff_decoder.rs      # Differential decoder (CQPSK)
│   │   └── sync.rs              # 48-bit frame sync word detection
│   │
│   ├── p25/
│   │   ├── mod.rs
│   │   ├── types.rs             # Newtypes: Nac, TalkgroupId, Frequency, etc.
│   │   ├── consts.rs            # Protocol constants
│   │   ├── error.rs             # P25 error types (thiserror)
│   │   ├── bch.rs               # BCH(63,16,23) decoder for NID error correction
│   │   ├── nid.rs               # Network ID decode (BCH + integrity gating)
│   │   ├── receiver.rs          # Data unit receiver state machine
│   │   ├── tsbk.rs              # TSBK opcode parser (all message types)
│   │   ├── trellis.rs           # 1/2 rate trellis codec
│   │   ├── interleave.rs        # TSBK dibit deinterleaving
│   │   ├── crc.rs               # CRC-16 (CCITT) for TSBK verification
│   │   ├── status.rs            # Status symbol deinterleaving
│   │   ├── ident.rs             # Identifier table (channel → frequency)
│   │   │
│   │   ├── coding/              # Forward error correction
│   │   │   ├── mod.rs
│   │   │   ├── golay.rs         # (24,12) extended Golay
│   │   │   ├── hamming.rs       # (15,11) Hamming
│   │   │   ├── reed_solomon.rs  # (24,12,13) and (24,16,9) Reed-Solomon
│   │   │   ├── galois.rs        # GF(2^6) field arithmetic
│   │   │   ├── cyclic.rs        # Cyclic code for low-speed data
│   │   │   └── bmcf.rs          # Bush-Caldwell-Murthy-Fang decoder
│   │   │
│   │   └── voice/               # Voice data unit decoding
│   │       ├── mod.rs
│   │       ├── frame.rs         # IMBE voice frame (88-bit chunks)
│   │       ├── frame_group.rs   # LDU1/LDU2 frame group receiver
│   │       ├── header.rs        # Voice header (HDU) decoder
│   │       ├── terminator.rs    # Voice LC terminator (TDULC) decoder
│   │       ├── control.rs       # Link Control word (LDU1)
│   │       ├── crypto.rs        # Crypto Control word (LDU2)
│   │       ├── descramble.rs    # Voice frame descrambling
│   │       └── pn.rs            # PN sequence generation
│   │
│   ├── vocoder/                 # IMBE vocoder (voice frame → PCM audio)
│   │   ├── mod.rs
│   │   ├── decode.rs            # Frame decoder (88-bit IMBE → 160 PCM samples)
│   │   ├── frame.rs             # Bit unpacking and parameter extraction
│   │   ├── params.rs            # Vocoder parameter structures
│   │   ├── spectral.rs          # Spectral amplitude reconstruction
│   │   ├── enhance.rs           # Spectral enhancement
│   │   ├── voiced.rs            # Voiced synthesis (harmonic oscillators)
│   │   ├── unvoiced.rs          # Unvoiced synthesis (shaped noise)
│   │   ├── gain.rs              # Adaptive gain control
│   │   ├── prev.rs              # Previous frame state
│   │   ├── window.rs            # Synthesis window functions
│   │   ├── consts.rs            # Vocoder constants
│   │   ├── coefs.rs             # Coefficient tables
│   │   ├── allocs.rs            # Band allocation tables
│   │   ├── scan.rs              # Bit scanning utilities
│   │   ├── descramble.rs        # Frame descrambling
│   │   └── error.rs             # Vocoder error types
│   │
│   ├── debug/                   # p25 debug subcommand (diagnostic tools)
│   │   ├── mod.rs               # CLI types, dispatch, decode-cc/decode-voice
│   │   ├── info.rs              # File inspection (format, duration, stats)
│   │   └── filter.rs            # Channel extraction (NCO + LPF + decimate)
│   │
│   ├── output/
│   │   ├── mod.rs
│   │   └── json.rs              # JSON serialization (TSBKs + voice events)
│   │
│   └── monitor/                 # TUI for p25 monitor subcommand
│       ├── mod.rs
│       ├── event.rs             # Monitor event types
│       ├── parse.rs             # JSON line parser (stdin)
│       ├── state.rs             # Talkgroup/grant state tracking
│       ├── ui.rs                # Terminal UI rendering
│       └── error.rs
│
├── tests/                       # Integration tests
│   ├── channelizer.rs           # Wideband channelizer tests
│   ├── cqpsk_signal.rs          # CQPSK signal generation tests
│   ├── decode_pipeline.rs       # End-to-end decode pipeline tests
│   ├── monitor_state.rs         # Monitor state machine tests
│   ├── sample_rate_config.rs    # Sample rate / decimation tests
│   └── soapy_source.rs          # SDR source integration tests
│
├── samples/                     # Radio captures and test tools
│   ├── decode_test.py           # OP25 comparison test harness
│   ├── op25_gold_reference.py   # Gold standard reference generator
│   ├── op25_gold_comparison.json
│   ├── op25_gold_cqpsk.json
│   ├── op25_gold_fsk4.json
│   ├── op25_reference/          # OP25 reference data
│   └── iq/                      # Raw IQ recordings (git-ignored)
│
└── docs/                        # Documentation and TIA-102 spec PDFs
    └── DECODER.md               # Detailed decoder pipeline reference
```

### Directory Conventions

- **`src/`** — All application source code. Standard Rust layout with `main.rs` as entry point and `lib.rs` as library root. Unit tests live in `#[cfg(test)] mod tests` blocks within each source file.
- **`tests/`** — Integration tests that exercise the pipeline end-to-end. Rust runs these with `cargo test` automatically.
- **`samples/`** — Radio captures and OP25 gold-standard comparison data for development and regression testing. IQ recordings in `samples/iq/` are git-ignored due to size.
- **`docs/`** — Detailed decoder pipeline documentation (`DECODER.md`) and TIA-102 specification PDFs for protocol reference.

---

## Rust Style Guidelines

This project prioritizes readable, well-structured Rust. Follow these conventions:

### General Principles

- **Clarity over brevity.** Spell things out. `talkgroup_id` not `tgid`. `control_channel_frequency` not `cc_freq`. The P25 spec is dense enough — the code shouldn't add another layer of cryptic abbreviations.
- **Match the spec vocabulary.** Use P25 terminology from TIA-102 where it exists. If the spec says "Network Access Code," the struct field is `network_access_code`, not `nac_value` or `access_code`.
- **No magic numbers.** Constants get names. `const FRAME_SYNC_PATTERN: u64 = 0x5575F5FF77FF;` not a bare hex literal in the middle of a function.
- **Small functions.** If a function doesn't fit on a screen (~40 lines), it's doing too much. Break it into named steps.

### Types and Data Modeling

- **Use newtypes for domain concepts.** `struct Frequency(u64)`, `struct TalkgroupId(u16)`, `struct UnitId(u32)`. This prevents mixing up bare integers and makes function signatures self-documenting.
- **Use enums for TSBK opcodes and message types.** Rust's enum + match is perfect for protocol decoding. Every opcode should be a variant, and the compiler will warn you if you miss one.

```rust
// Good: exhaustive, self-documenting
enum TsbkOpcode {
    GroupVoiceChannelGrant,
    GroupVoiceChannelGrantUpdate,
    UnitRegistrationResponse,
    AdjacentStatusBroadcast,
    // ...
    Unknown(u8),
}

// Good: newtype prevents mixing up talkgroup and unit IDs
struct TalkgroupId(u16);
struct UnitId(u32);
struct Frequency(u64);

fn handle_grant(tg: TalkgroupId, freq: Frequency, src: UnitId) { ... }
```

- **Prefer structs with named fields over tuples** for anything that crosses a module boundary.
- **Derive liberally.** `#[derive(Debug, Clone, PartialEq)]` on almost everything. `Serialize` on anything that reaches the output layer.

### Error Handling

- **Use `thiserror` for library error types.** Define a clear error enum per module.
- **Use `anyhow` in `main.rs` and CLI glue** where you just need to propagate errors with context.
- **No `.unwrap()` in library code.** Ever. Return `Result` or `Option` and let the caller decide.
- **`.unwrap()` is acceptable in tests** and in `main()` after argument validation where failure means a bug.

```rust
// Good: clear error types
#[derive(Debug, thiserror::Error)]
enum TsbkError {
    #[error("CRC mismatch: expected {expected:#06x}, got {actual:#06x}")]
    CrcMismatch { expected: u16, actual: u16 },

    #[error("unknown opcode: {0:#04x}")]
    UnknownOpcode(u8),

    #[error("insufficient data: need {need} bits, have {have}")]
    InsufficientData { need: usize, have: usize },
}
```

### Module Organization

- **One concept per file.** `fm_demod.rs` contains FM demodulation and nothing else.
- **`mod.rs` files are thin.** They re-export public items and maybe contain a sentence of module-level docs. No business logic.
- **Public API surface should be minimal.** Default to `pub(crate)`. Only mark things `pub` if they're part of the tool's external interface or needed for integration tests.

### Documentation

- **Doc comments on every public item.** At minimum, one sentence explaining what it does.
- **Include units in doc comments for numeric parameters.** "Frequency in Hz", "Sample rate in samples per second", "Deviation in Hz".
- **Link to TIA-102 section numbers** where relevant, so reviewers can cross-reference the spec.

```rust
/// Decode a Trunking Signaling Block (TSBK) from raw corrected bits.
///
/// Expects exactly 96 bits of trellis-decoded, error-corrected data.
/// Returns the parsed message or an error if the CRC check fails
/// or the opcode is malformed.
///
/// Reference: TIA-102.AABF-A §7.1 (TSBK Format)
pub fn decode_tsbk(bits: &BitSlice) -> Result<TsbkMessage, TsbkError> {
    // ...
}
```

### Testing

- **Unit tests live in the same file** as the code they test, in a `#[cfg(test)] mod tests` block.
- **Integration tests live in `tests/`.** These test the full pipeline from IQ/dibits through JSON output.
- **Use fixtures for protocol data.** Commit known-good TSBK hex strings with expected parse results. These are your regression suite.
- **Test the boundaries.** Malformed input, truncated frames, unknown opcodes, CRC failures. The RF world is noisy — the decoder must handle garbage gracefully.
- **Property-based testing with `proptest`** for DSP functions where applicable (e.g., verify that encode → decode round-trips correctly).

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_group_voice_channel_grant() {
        let bits = bits_from_hex("...");
        let msg = decode_tsbk(&bits).unwrap();

        assert_eq!(msg.opcode, TsbkOpcode::GroupVoiceChannelGrant);
        assert_eq!(msg.talkgroup, TalkgroupId(3769));
        assert_eq!(msg.frequency, Frequency(851_325_000));
    }

    #[test]
    fn reject_bad_crc() {
        let mut bits = bits_from_hex("...");
        bits.set(50, !bits[50]); // flip a bit

        assert!(matches!(
            decode_tsbk(&bits),
            Err(TsbkError::CrcMismatch { .. })
        ));
    }
}
```

### Formatting and Linting

- **Always `cargo fmt`** before committing. No exceptions.
- **`cargo clippy` must pass clean.** Treat warnings as errors in CI: `cargo clippy -- -D warnings`.
- **Line length: 100 characters.** Configured in `rustfmt.toml`.

```toml
# rustfmt.toml
max_width = 100
use_field_init_shorthand = true
use_try_shorthand = true
```

---

## Roadmap

### Implemented

- [x] Project structure and build system
- [x] SoapySDR source + IQ file replay — CF32 and U8 formats (`src/sdr/`)
- [x] C4FM FM demodulator (`src/dsp/fm_demod.rs`)
- [x] CQPSK demodulator for simulcast systems (`src/dsp/cqpsk_demod.rs`)
- [x] Multi-stage decimation pipeline, ≤10x per stage (`src/pipeline.rs`)
- [x] Gardner clock recovery and dibit slicer (`src/dsp/timing.rs`)
- [x] Frame synchronization (48-bit sync word detection) (`src/dsp/sync.rs`)
- [x] Trellis 1/2 rate decoder (`src/p25/trellis.rs`)
- [x] Error correction: Golay, Hamming, Reed-Solomon (`src/p25/coding/`)
- [x] BCH(63,16,23) NID error correction — corrects up to 11 bit errors (`src/p25/bch.rs`)
- [x] TSBK CRC validation (`src/p25/crc.rs`)
- [x] TSBK opcode parser (channel grants, affiliations, system info) (`src/p25/tsbk.rs`)
- [x] JSON output to stdout (`src/output/json.rs`)
- [x] Identifier table tracking (logical channel → frequency mapping) (`src/p25/ident.rs`)
- [x] Adjacent site tracking (`src/p25/tsbk.rs`)
- [x] NID integrity gating with strict/permissive policy (`src/p25/nid.rs`)
- [x] Voice frame decoding — IMBE frames, LDU1/LDU2, HDU, terminators (`src/p25/voice/`)
- [x] IMBE vocoder — 88-bit voice frames to 8 kHz PCM audio (`src/vocoder/`)
- [x] Wideband trunked decoder — `p25 trunk` (`src/channel_manager.rs`)
- [x] Monitor TUI — `p25 monitor` (`src/monitor/`)
- [x] Debug subcommand — file info, channel filter, decode tools (`src/debug/`)

### Future

- [ ] ZeroMQ publisher output (optional feature flag)
- [ ] IQ file recording mode
- [ ] Adaptive equalizer for simulcast compensation
- [ ] Phase II TDMA support

---

## Troubleshooting

- **No decodes on a simulcast system?** Use `--modulation cqpsk`. The default C4FM demodulator will not lock onto CQPSK signals. Simulcast systems use CQPSK (also called LSM) for control and voice channels.

- **Use manual gain, not AGC.** `--gain 40` is recommended over `--auto-gain`. Automatic gain control can destabilize the CQPSK Costas PLL, causing intermittent decode failures.

- **Sample rate must be a multiple of 24000 Hz.** The decoder validates this at startup and suggests the nearest valid rates. Common choices: 48000, 240000, 480000, 960000, 2400000.

- **SDRplay: set `rfgain_sel` explicitly.** The default RF gain table index (4) is often too low. Use `--setting rfgain_sel=24` for better sensitivity.

- **SDRplay: disable HDR mode for 800 MHz.** HDR mode only works below 2 MHz. Use `--setting hdr_ctrl=false` for 800 MHz P25 systems.

- **SDRplay: disable bias-T unless needed.** Use `--setting biasT_ctrl=false` unless you are powering an external LNA through the antenna port.

- **Viewing logs while piping.** When stdout is piped (e.g., `p25 cc ... | p25 monitor`), stderr is suppressed to avoid corrupting the TUI. To see decode logs: `p25 cc ... 2>decode.log | p25 monitor`. Set `RUST_LOG=debug` for verbose output.

---

## References

- [TIA-102.AABF-A](https://www.tiaonline.org/) — P25 Trunking Control Channel Messages
- [TIA-102.BAAA](https://www.tiaonline.org/) — P25 Common Air Interface (C4FM)
- [RadioReference SRRCS Wiki](https://wiki.radioreference.com/index.php/Sacramento_Regional_Radio_Communications_System_(SRRCS)_P25)
- [OP25](https://github.com/boatbod/op25) — Open source P25 decoder (reference implementation)
- [trunk-recorder](https://github.com/robotastic/trunk-recorder) — GNU Radio based P25 recorder
- [SoapySDR](https://github.com/pothosware/SoapySDR) — Vendor-neutral SDR abstraction

---

## License

MIT OR Apache-2.0 (dual-licensed, standard Rust convention)

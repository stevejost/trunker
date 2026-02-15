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

Tunes to a P25 control channel frequency, demodulates C4FM, decodes Trunking Signaling Blocks (TSBKs), and emits structured JSON to stdout.

```bash
# Monitor a control channel
p25 cc --device "driver=rtlsdr" -f 853.450e6

# With an SDRplay RSPdx
p25 cc --device "driver=sdrplay" -f 853.450e6

# Decode from a recorded IQ file (development/testing)
p25 cc --file capture.iq --sample-rate 2.4e6

# Pipe to jq for live filtering
p25 cc -f 853.450e6 | jq 'select(.type == "chan_grant")'
```

**Output:**

```jsonl
{"ts":1739500000.123,"type":"chan_grant","tg":3769,"freq":851325000,"src":12345}
{"ts":1739500001.456,"type":"chan_grant","tg":3325,"freq":852075000,"src":67890}
{"ts":1739500002.789,"type":"affiliation","unit":54321,"tg":3847}
{"ts":1739500003.012,"type":"ident_update","channel":1,"freq":851050000,"bandwidth":12500}
{"ts":1739500004.345,"type":"adj_site","site":3,"rfss":1,"freq":852225000}
{"ts":1739500005.678,"type":"sys_info","sysid":"5F2","wacn":"BEE00","nac":"5F2"}
```

### `p25 voice` — Voice Channel Decoder *(future)*

Tunes to a P25 voice channel frequency, demodulates C4FM, decodes IMBE voice frames, and outputs audio. Can follow channel grants from `p25 cc`.

```bash
# Follow grants from the control channel decoder
p25 cc -f 853.450e6 | p25 voice --follow-grants --device "driver=rtlsdr,serial=00000002"

# Decode a single talkgroup
p25 cc -f 853.450e6 | p25 voice --follow-grants --talkgroup 3769

# Multiple simultaneous talkgroups with multiple SDRs
p25 cc -f 853.450e6 | p25 voice --follow-grants --talkgroup 3769 --device-index 1 &
p25 cc -f 853.450e6 | p25 voice --follow-grants --talkgroup 3847 --device-index 2 &
```

### `p25 scan` — System Scanner *(future)*

Higher-level orchestrator that manages multiple voice decoders and SDR devices for a complete scanning experience.

```bash
# Scan a system using a config file
p25 scan --config srrcs.toml
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
┌─────────────────────────────────────────────────┐
│                  SDR Hardware                    │
│         (RTL-SDR, SDRplay, Airspy, ...)         │
└──────────────────────┬──────────────────────────┘
                       │ IQ samples
                       ▼
┌─────────────────────────────────────────────────┐
│              SoapySDR Abstraction                │
└──────────────────────┬──────────────────────────┘
                       │ Complex f32 samples
                       ▼
┌─────────────────────────────────────────────────┐
│                 DSP Pipeline                     │
│  ┌───────────┐  ┌──────────┐  ┌──────────────┐ │
│  │ FM Demod  ├─►│  Clock   ├─►│    Dibit     │ │
│  │ (C4FM)    │  │ Recovery │  │   Slicer     │ │
│  └───────────┘  └──────────┘  └──────┬───────┘ │
└──────────────────────────────────────┼──────────┘
                                       │ Dibit stream
                                       ▼
┌─────────────────────────────────────────────────┐
│              Protocol Decoder                    │
│  ┌───────────┐  ┌──────────┐  ┌──────────────┐ │
│  │  Frame    ├─►│ Trellis  ├─►│    TSBK      │ │
│  │  Sync     │  │ Decode   │  │   Parser     │ │
│  └───────────┘  └──────────┘  └──────┬───────┘ │
└──────────────────────────────────────┼──────────┘
                                       │ Structured messages
                                       ▼
┌─────────────────────────────────────────────────┐
│                JSON Serializer                   │
│              (stdout / file / ZMQ)               │
└─────────────────────────────────────────────────┘
```

### Layer Responsibilities

| Layer | Input | Output | Testable With |
|---|---|---|---|
| SDR Source | Hardware/File | Complex IQ samples | Recorded `.iq` files |
| FM Demodulator | IQ samples | Baseband signal | Synthetic IQ test vectors |
| Clock Recovery | Baseband signal | Symbol stream | Synthetic baseband signals |
| Dibit Slicer | Symbol stream | Dibit stream (0-3) | Raw symbol arrays |
| Frame Sync | Dibit stream | Aligned data units | Recorded dibit streams |
| Trellis Decoder | Raw data unit bits | Corrected data bits | Known test vectors from spec |
| TSBK Parser | Corrected bits | Typed message structs | Hex-encoded TSBK fixtures |
| JSON Serializer | Message structs | JSON lines | Unit tests on structs |

Each layer boundary is a clean interface with a well-defined data type. No layer reaches into another layer's internals.

---

## Technology

| Component | Choice | Rationale |
|---|---|---|
| Language | Rust | Performance of C, safety guarantees, excellent tooling, `cargo install` distribution |
| SDR Interface | SoapySDR (via `soapysdr` crate) | Hardware-agnostic, supports all major SDR devices |
| DSP Math | `num-complex`, `rustfft` | Standard Rust numerics, proven FFT implementation |
| Bit Manipulation | `bitvec` | Ergonomic bitfield access, critical for protocol parsing |
| Serialization | `serde` + `serde_json` | Industry-standard, zero-boilerplate JSON output |
| CLI | `clap` | Derive-based arg parsing, subcommand support |
| Logging | `tracing` | Structured logging with levels, filterable at runtime |
| Testing | Built-in (`cargo test`) | No external test framework needed |
| Async (future) | `tokio` | For ZMQ/network output and multi-device orchestration |

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
- **Affiliations** (`U_REG_RSP`, `GRP_AFF_RSP`) — a radio joining a talkgroup.
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
cargo test tsbk::parser::tests

# Run integration tests with IQ fixtures
cargo test --test integration
```

### Recording IQ Files for Development

Capture raw IQ samples for offline development and regression testing:

```bash
# Using SoapySDR's command-line utility
SoapySDRUtil --probe  # verify your device is detected

# Using rx_sdr (from soapy-tools)
rx_sdr -d "driver=rtlsdr" -f 853450000 -s 2400000 -g 40 -n 57600000 capture_30s.iq

# This gives you 30 seconds of IQ at 2.4 MSPS (2 bytes/sample I+Q = ~144 MB)
```

### Project Structure

```
trunker/
├── Cargo.toml
├── Cargo.lock
├── CLAUDE.md
├── README.md
├── LICENSE-MIT
├── LICENSE-APACHE
├── rustfmt.toml
├── .gitignore
│
├── src/                         # Application source (standard Rust convention)
│   ├── main.rs                  # CLI entry point and subcommand dispatch
│   ├── cli.rs                   # Clap argument definitions
│   ├── sdr/
│   │   ├── mod.rs
│   │   ├── source.rs            # SoapySDR + file source abstraction
│   │   └── config.rs            # Device configuration
│   ├── dsp/
│   │   ├── mod.rs
│   │   ├── fm_demod.rs          # C4FM demodulation
│   │   ├── clock_recovery.rs    # Gardner timing error detector
│   │   ├── slicer.rs            # Dibit decision slicer
│   │   └── filter.rs            # FIR / RRC filter implementations
│   ├── p25/
│   │   ├── mod.rs
│   │   ├── sync.rs              # Frame synchronization
│   │   ├── trellis.rs           # 1/2 rate trellis decoding
│   │   ├── error_correction.rs  # Golay, Hamming, Reed-Solomon, CRC
│   │   ├── tsbk/
│   │   │   ├── mod.rs
│   │   │   ├── opcode.rs        # TSBK opcode enum definitions
│   │   │   ├── parser.rs        # TSBK field extraction
│   │   │   └── messages.rs      # Typed message structs
│   │   ├── nid.rs               # Network ID decoding
│   │   └── data_unit.rs         # Data unit type handling
│   ├── output/
│   │   ├── mod.rs
│   │   ├── json.rs              # JSON serialization
│   │   └── zmq.rs               # ZeroMQ publisher (optional feature)
│   └── util/
│       ├── mod.rs
│       └── bits.rs              # Bit manipulation helpers
│
├── tests/                       # Integration tests (standard Rust convention)
│   ├── integration/
│   │   └── cc_decode.rs         # End-to-end control channel tests
│   └── fixtures/
│       ├── tsbk_vectors.json    # Known TSBK hex → expected parse results
│       └── dibit_stream.bin     # Recorded dibit sequences with known content
│
├── samples/                     # Radio captures for development and testing
│   ├── README.md                # Documents each capture (frequency, SDR, settings)
│   └── iq/                      # Raw IQ baseband recordings
│       └── srrcs_cc_852350_2400k_i16.wav  # SRRCS control channel capture
│
└── docs/                        # Project documentation and references
    ├── p25_quick_reference.md   # Field-level TSBK format reference
    └── dsp_notes.md             # Design notes on demod/clock recovery
```

### Directory Conventions

- **`src/`** — All application source code. Standard Rust layout with `main.rs` as entry point. Unit tests live in `#[cfg(test)] mod tests` blocks within each source file.
- **`tests/`** — Integration tests that exercise the pipeline end-to-end. Rust runs these with `cargo test` automatically. Fixtures in `tests/fixtures/` are small, deterministic data files (hex vectors, short dibit sequences) committed to the repo.
- **`samples/`** — Real radio captures used during development. These are large binary files (IQ recordings from SDR++ or similar). Use Git LFS for files over a few MB, or `.gitignore` them and document how to recreate. Each file should be named descriptively: `{system}_{type}_{freq}_{samplerate}_{format}.wav` (e.g., `srrcs_cc_852350_2400k_i16.wav`). The `samples/README.md` should document the capture conditions for each file (date, SDR hardware, gain, antenna, location).
- **`docs/`** — Long-form documentation, protocol references, and design notes. Not API docs (those live in doc comments and are generated by `cargo doc`).

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

- [x] Project structure and build system
- [ ] SoapySDR source + IQ file replay
- [ ] C4FM FM demodulator
- [ ] Gardner clock recovery and dibit slicer
- [ ] Frame synchronization (48-bit sync word detection)
- [ ] Trellis 1/2 rate decoder
- [ ] TSBK CRC validation
- [ ] TSBK opcode parser (channel grants, affiliations, system info)
- [ ] JSON output to stdout
- [ ] Identifier table tracking (logical channel → frequency mapping)
- [ ] Adjacent site tracking
- [ ] ZeroMQ publisher output (optional feature flag)
- [ ] IQ file recording mode
- [ ] Voice channel decoder (`p25 voice`)
- [ ] Adaptive equalizer for simulcast compensation
- [ ] Phase II TDMA support

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

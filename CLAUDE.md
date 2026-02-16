# CLAUDE.md

Project guidance for AI assistants working on trunker.

---

## Team Structure

This project uses Agent Teams. Each team member has a defined role and responsibilities. Team members should stay in their lane and defer to the appropriate expert when questions fall outside their domain.

### 1. Project Owner / Stakeholder

**Focus:** MVP. The reason this project exists is that there is no simple, Unix-y P25 decoder. The PO's job is to keep the team focused on getting a working decoder that reads IQ and emits JSON. The PO should actively question developers and the PM: "Does this feature need to exist before MVP is complete?" If the answer is no, it gets deferred. MVP = successfully decode TSBKs from a real IQ recording and emit correct JSON.

### 2. Development Project Manager

**Focus:** Task breakdown and sequencing. Expert at splitting work into right-sized tasks and evaluating complexity. If a task seems non-trivial or touches RF/DSP domain knowledge, the PM must consult the RF Expert before sizing or assigning it. The PM maintains the task list and ensures developers always have clear, well-scoped work items.

### 3. QA Expert

**Focus:** Testing is non-negotiable. The QA expert's job is to force unit testing throughout the entire application. Every module gets tests. Every new function gets a test. The QA expert should actively search for demo P25 payloads, known-good test vectors, and help capture real P25 data for regression testing. QA is a stick-in-the-mud: no feature ships without tests, no "we'll add tests later." Correct RF decoding is the highest priority — a decoder that produces wrong output is worse than one that produces no output.

### 4. RF Expert

**Focus:** Theory and correctness of the RF/DSP pipeline. The RF expert understands APCO P25 at the signal level: C4FM modulation, symbol timing, frame sync correlation, BCH error correction, trellis coding, channel filtering, FM demodulation — all of it. This expert does NOT write code. They explain what each stage of the decode pipeline should do, what the expected signal characteristics are, what the correct algorithms are, and review whether the implementation matches the theory. When something doesn't decode correctly, the RF expert diagnoses whether it's a DSP problem or a protocol problem.

### 5. Rust Graybeard

**Focus:** Code quality gate. Reviews code post-implementation (after each task/feature is complete). The graybeard rejects code that is not: (a) simple to understand, (b) idiomatically correct Rust, and (c) the simplest form the code can take. No clever tricks, no over-engineering, no premature abstractions. If there's a simpler way to express something, the graybeard will find it and demand the change. The graybeard's approval is required before moving to the next feature.

### 6. Staff Developers (x2)

**Focus:** Implementation. Pull tasks from the PM's task list and write code. Follow the architecture rules, write tests (enforced by QA), and submit work for graybeard review. When stuck on RF/DSP questions, escalate to the RF Expert. When unsure about scope, check with the PM or PO.

### Team Interaction Rules

- **PM** assigns tasks. Developers don't self-assign without PM approval.
- **RF Expert** is consulted before any DSP/protocol task is sized or started.
- **QA** reviews test coverage on every deliverable. No exceptions.
- **Graybeard** reviews code post-implementation. Approval required before moving on.
- **PO** has veto power on scope. If PO says "not MVP," it's deferred.
- When in doubt, ask the relevant expert. Don't guess.

---

## Project Overview

`trunker` is a Rust CLI application for decoding APCO Project 25 (P25) trunked radio control channels using software-defined radios. It follows Unix philosophy: RF in, structured JSON out, composable with pipes.

The primary tool is `p25 cc` — a control channel decoder that continuously monitors a P25 control channel frequency, demodulates C4FM, decodes Trunking Signaling Blocks (TSBKs), and emits JSON lines to stdout.

**This is not a scanner application.** There is no UI, no audio player, no talkgroup database. It is a protocol decoder that emits structured data for downstream consumers.

---

## Repository Structure

```
trunker/
├── Cargo.toml              # Workspace and dependency definitions
├── CLAUDE.md               # This file — AI assistant guidance
├── README.md               # Project overview, goals, style guide
├── LICENSE-MIT
├── LICENSE-APACHE
├── rustfmt.toml            # Rust formatting config
├── .gitignore
├── src/                    # All application source code
│   ├── main.rs             # CLI entry point
│   └── ...                 # See README.md for full tree
├── tests/                  # Integration tests (cargo test runs these)
│   ├── integration/        # End-to-end pipeline tests
│   └── fixtures/           # Small deterministic test data (committed)
├── samples/                # Radio captures for development
│   ├── README.md           # Documents each capture file
│   └── iq/                 # IQ baseband recordings (may be git-ignored)
└── docs/                   # Protocol references and design notes
```

### Directory Roles

- **`src/`** — Standard Rust source layout. Unit tests are inline (`#[cfg(test)] mod tests`).
- **`tests/`** — Integration tests and small fixture files. Everything here is committed to the repo and runs in CI. Keep fixtures small (hex vectors, short dibit sequences).
- **`samples/`** — Real IQ recordings from SDR hardware. These are working files for development, not test fixtures. They may be large (hundreds of MB) and should use Git LFS or be `.gitignore`d. The `samples/README.md` must document each file's capture conditions (frequency, sample rate, format, gain, SDR hardware, antenna, date, location). Naming convention: `{system}_{type}_{freq}_{samplerate}_{format}.wav` — e.g., `srrcs_cc_852350_2400k_i16.wav`.
- **`docs/`** — Long-form documentation. Protocol quick references, DSP design notes, architecture decisions. Not generated API docs (those come from `cargo doc`).

---

## Build and Test

```bash
# Build
cargo build
cargo build --release

# Run all tests
cargo test

# Run tests with stdout visible
cargo test -- --nocapture

# Run a specific test module
cargo test tsbk::parser::tests

# Lint (must pass clean, warnings are errors in CI)
cargo clippy -- -D warnings

# Format (required before all commits)
cargo fmt

# Check formatting without modifying
cargo fmt -- --check
```

### Optional feature flags

```bash
# Build with ZeroMQ output support
cargo build --features zmq
```

---

## Architecture Rules

### Layer Separation

The codebase has four distinct layers. Do not violate these boundaries:

1. **SDR Source** (`src/sdr/`) — Reads IQ samples from hardware via SoapySDR or from files. Knows nothing about P25.
2. **DSP Pipeline** (`src/dsp/`) — FM demodulation, clock recovery, dibit slicing. Knows nothing about P25 framing.
3. **Protocol Decoder** (`src/p25/`) — Frame sync, trellis decode, TSBK parsing. Operates on dibit streams. Knows nothing about RF or SDR hardware.
4. **Output** (`src/output/`) — Serializes decoded messages to JSON, ZMQ, etc. Knows nothing about how messages were decoded.

**Each layer only depends on the layer above it.** The protocol decoder never imports from `sdr::`. The DSP layer never imports from `p25::`. This separation is what makes the project testable without hardware.

### Data Flow

```
IQ samples (Complex<f32>) → DSP → Dibits (u8: 0-3) → Protocol → Messages → Output → JSON
```

Each arrow is a function boundary that can be tested independently.

---

## Key Design Decisions

### Newtypes for Domain Concepts

Always use newtypes, never bare primitives for domain values:

```rust
struct Frequency(u64);      // Hz
struct TalkgroupId(u16);
struct UnitId(u32);
struct Nac(u16);            // Network Access Code
struct SystemId(u16);
struct Wacn(u32);           // Wide Area Communications Network
```

This is a hard rule. If you're writing a function that takes a `u16` that represents a talkgroup, wrap it.

### TSBK Opcodes as Enums

Every TSBK opcode must be a variant in the `TsbkOpcode` enum. Unknown opcodes get `Unknown(u8)`, not a panic. The RF world is noisy and the decoder will encounter garbage.

### No Unwrap in Library Code

`src/sdr/`, `src/dsp/`, `src/p25/`, and `src/output/` must never use `.unwrap()` or `.expect()` on `Result` or `Option`. Return errors to the caller. The only exceptions are:

- `main.rs` and CLI glue code
- Test code (`#[cfg(test)]`)
- Proven-safe cases with a comment explaining why (e.g., regex compilation of a constant pattern)

### Error Types

- Each module defines its own error enum using `thiserror`.
- `main.rs` uses `anyhow` for top-level error propagation.
- Error messages must be actionable. Include values: "CRC mismatch: expected 0x1A3F, got 0x0000", not "CRC error".

### Constants, Not Magic Numbers

Every protocol-defined value gets a named constant:

```rust
const FRAME_SYNC_PATTERN: u64 = 0x5575F5FF77FF;
const TSBK_LENGTH_BITS: usize = 96;
const SYMBOL_RATE: u32 = 4800;
const C4FM_DEVIATION: f32 = 1800.0; // Hz, nominal
```

---

## Code Style

### Naming

- Use full words: `talkgroup_id` not `tgid`, `control_channel` not `cc`, `frequency` not `freq`.
- Exception: In JSON output, use abbreviated field names for compactness (`tg`, `freq`, `src`). The mapping between internal names and output names happens in the serialization layer.
- Match P25/TIA-102 terminology exactly. If the spec says "Group Voice Channel Grant", the enum variant is `GroupVoiceChannelGrant`.

### Functions

- Max ~40 lines per function. If it's longer, extract named helper functions.
- Pure functions are preferred. Take inputs, return outputs. Minimize mutation.
- DSP functions that process sample-by-sample may carry state in a struct. That's fine — but isolate the stateful processing from pure computation.

### Documentation

- Every public function, struct, and enum gets a doc comment.
- Include units for numeric parameters: `/// Sample rate in samples per second`.
- Reference TIA-102 section numbers where applicable: `/// Reference: TIA-102.AABF-A §7.1`.
- Module-level doc comments should describe the module's role in the pipeline.

### Tests

- Unit tests go in `#[cfg(test)] mod tests` in the same file.
- Integration tests go in `tests/`.
- Fixture data (known TSBK hex strings, recorded dibit streams) goes in `tests/fixtures/`.
- Test both happy path and error cases. Malformed input, truncated data, bad CRC, unknown opcodes.
- When adding a new TSBK opcode, always add a corresponding test with a known-good hex vector.

---

## Working with P25 Protocol Data

### TSBK Structure (96 bits after trellis decode)

```
Bits 0-7:    Opcode (8 bits)
Bit 8:       Last Block Flag
Bits 9-95:   Opcode-specific fields (varies by message type)
             Includes 16-bit CRC at the end
```

### Common TSBK Opcodes to Prioritize

| Hex | Name | Priority | Notes |
|-----|------|----------|-------|
| 0x00 | GRP_V_CH_GRANT | Critical | Channel grant — the core message |
| 0x02 | GRP_V_CH_GRANT_UPDT | Critical | Update to ongoing grant |
| 0x04 | UNT_TO_UNT_CH_GRANT | High | Unit-to-unit private call |
| 0x20 | IDENT_UP | Critical | Maps channel IDs to frequencies |
| 0x28 | GRP_AFF_RSP | Medium | Unit affiliating with talkgroup |
| 0x2C | U_REG_RSP | Medium | Unit registration |
| 0x34 | IDEN_UP_VU | Critical | Identifier update (VHF/UHF bands) |
| 0x39 | NET_STS_BCST | High | Network status broadcast |
| 0x3A | RFSS_STS_BCST | High | RFSS status broadcast |
| 0x3C | ADJ_STS_BCST | Medium | Adjacent site info |

### Frequency Calculation

P25 uses logical channel numbers that must be mapped to actual RF frequencies using IDENT_UP messages. The formula is:

```
frequency = base_freq + (channel_spacing * channel_number) + transmit_offset
```

The decoder must maintain an identifier table (populated from IDENT_UP messages) and use it to resolve channel grants to actual frequencies.

---

## Common Tasks

### Adding a New TSBK Opcode

1. Add the variant to `TsbkOpcode` enum in `src/p25/tsbk/opcode.rs`
2. Add the message struct in `src/p25/tsbk/messages.rs`
3. Add the parsing logic in `src/p25/tsbk/parser.rs`
4. Add serialization in `src/output/json.rs`
5. Add a test with a known hex vector in the parser test module
6. Update `tests/fixtures/tsbk_vectors.json` with the new test case

### Adding a New SDR Backend

SoapySDR handles this. If the device has a SoapySDR driver module installed, it should work with no code changes. If custom configuration is needed:

1. Add device-specific config handling in `src/sdr/config.rs`
2. Add a CLI flag in `src/cli.rs` if needed
3. Test with the actual hardware

### Debugging Decode Failures

1. Record an IQ file of the problematic signal
2. Run `p25 cc --file problem.iq --sample-rate 2.4e6 --verbose`
3. The `--verbose` flag enables `tracing` output at debug level, showing frame sync hits, CRC pass/fail, and raw hex of decoded TSBKs
4. Compare against OP25's output on the same IQ file to identify divergence

---

## Testing Strategy

### Unit Tests (per-module)

Every parsing function and DSP function gets unit tests with known inputs and expected outputs. For protocol parsing, use hex-encoded TSBK data with verified-correct field values.

### Fixture-Based Tests

`tests/fixtures/tsbk_vectors.json` contains an array of test vectors:

```json
[
  {
    "name": "group_voice_channel_grant_spd_north",
    "hex": "0080...",
    "expected": {
      "opcode": "GroupVoiceChannelGrant",
      "talkgroup": 3769,
      "frequency": 851325000,
      "source": 12345
    }
  }
]
```

The integration test loads all vectors and verifies they parse correctly. Adding new fixtures automatically expands test coverage.

### IQ Replay Tests

Short IQ recordings (5-10 seconds) committed to `tests/fixtures/iq/` enable full-pipeline regression testing. These are large files — use Git LFS or keep them small.

### What to Test for New Code

- Happy path with valid input
- Malformed input (wrong length, bad magic bytes)
- CRC failures (flip a bit and verify rejection)
- Unknown/reserved values (verify graceful handling, not panic)
- Boundary conditions (empty input, maximum values)

---

## External References

- **TIA-102.AABF-A** — TSBK message formats and opcodes (the primary spec)
- **TIA-102.BAAA** — C4FM common air interface specification
- **OP25 source** (github.com/boatbod/op25) — Reference implementation for decode behavior
- **RadioReference wiki** — System-specific data (frequencies, talkgroups, site info)
- **GNU Radio** — DSP block implementations for algorithm reference

When implementing a protocol feature, always cross-reference the TIA-102 spec first, then check how OP25 handles it for practical edge cases the spec doesn't cover.

---

## Environment Notes

- Minimum Rust edition: 2021
- Target platforms: Linux (primary), macOS (secondary)
- SoapySDR must be installed system-wide (not a Cargo dependency, linked at build time)
- IQ files from SDR++ are WAV format with interleaved `i16` I/Q samples. Use `hound` crate to read, or skip the 44-byte WAV header and read raw. Normalize to `f32` with `sample as f32 / 32768.0`.
- IQ files may also be raw interleaved `u8` format (RTL-SDR native via `rtl_sdr`) or interleaved `f32` (SoapySDR CF32). The file source abstraction should support all three.
- Sample files live in `samples/iq/`. These may be git-ignored due to size — see `samples/README.md` for how to recreate them.
- The project uses a Cargo workspace if/when `p25 voice` becomes a separate binary

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

## Build and Test

```bash
cargo build
cargo test
cargo test -- --nocapture
cargo clippy -- -D warnings
cargo fmt
```

---

## Key Design Decisions

### Newtypes for Domain Concepts

Always use newtypes, never bare primitives for domain values:

```rust
struct Frequency(u64);      // Hz
struct TalkgroupId(u16);
struct Nac(u16);            // Network Access Code
```

### No Unwrap in Library Code

Library code must never use `.unwrap()` or `.expect()`. Return errors to the caller. Exceptions: `main.rs`, test code, proven-safe cases with a comment.

### Error Types

- Each module defines its own error enum using `thiserror`.
- `main.rs` uses `anyhow` for top-level error propagation.
- Error messages must be actionable. Include values: "CRC mismatch: expected 0x1A3F, got 0x0000", not "CRC error".

### Constants, Not Magic Numbers

Every protocol-defined value gets a named constant.

### TSBK Opcodes as Enums

Unknown opcodes get `Unknown(u8)`, not a panic. The RF world is noisy.

---

## Code Style

- Use full words: `talkgroup_id` not `tgid`, `frequency` not `freq`.
- Match P25/TIA-102 terminology exactly.
- Max ~40 lines per function.
- Every public item gets a doc comment with units where applicable.
- Unit tests go in `#[cfg(test)] mod tests` in the same file.
- Test both happy path and error cases.

---

## P25 Protocol Reference

### TSBK Structure (96 bits after trellis decode)

```
Bits 0-7:    Opcode (8 bits)
Bit 8:       Last Block Flag
Bits 9-95:   Opcode-specific fields (varies by message type)
             Includes 16-bit CRC at the end
```

### Key TSBK Opcodes

| Hex  | Name                    | Notes                          |
|------|-------------------------|--------------------------------|
| 0x00 | GRP_V_CH_GRANT          | Channel grant (core message)   |
| 0x02 | GRP_V_CH_GRANT_UPDT     | Update to ongoing grant        |
| 0x20 | IDENT_UP                | Maps channel IDs to frequencies|
| 0x34 | IDEN_UP_VU              | Identifier update (VHF/UHF)    |
| 0x39 | NET_STS_BCST            | Network status broadcast       |
| 0x3A | RFSS_STS_BCST           | RFSS status broadcast          |

### Frequency Calculation

```
frequency = base_freq + (channel_spacing * channel_number) + transmit_offset
```

---

## External References

- **TIA-102.AABF-A** — TSBK message formats and opcodes
- **TIA-102.BAAA** — C4FM common air interface specification
- **OP25 source** (github.com/boatbod/op25) — Reference implementation
- **kchmck/p25.rs** — Rust reference implementation (local at `~/source/p25.rs`)

---

## Environment Notes

- Rust edition: 2024
- Target platforms: Linux (primary), macOS (secondary)
- IQ sample files live in `samples/iq/` (git-ignored due to size)
- Sample formats: CF32 (interleaved f32 I/Q), i16 WAV, u8 (RTL-SDR native)

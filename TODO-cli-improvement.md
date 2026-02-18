# CLI Redesign Plan for `p25`

## Context

The `p25` CLI works well for its current fixed-frequency, single-CC use case. But real P25 systems can hop their control channel between frequencies (failover, maintenance, load shedding), and the wideband `trunk` command assumes the CC is always at the capture center. This plan redesigns the CLI to handle CC hopping, flexible wideband captures, and future input sources — while keeping the tool simple and Unix-y.

Three experts contributed: RF Expert (protocol realities), Rust Graybeard (architecture), CLI Usability Expert (interface design).

---

## Expert Consensus

All three experts agree on:
- **Keep `SampleSource` as an enum**, not a trait (2-3 variants don't justify vtable dispatch)
- **Add `retune()` to `SoapySource`** by storing the `Device` handle (currently dropped after open)
- **Extract shared CLI args** into a `CommonArgs` struct (4 duplicated fields across `cc`/`trunk`)
- **Human-friendly frequency input** (`852.35M` alongside `852350000`)
- **CC frequency list** for hopping support
- **Defer config files and output format flags** -- not needed yet
- **No new traits or abstractions** -- extend existing patterns (`CcTracker` struct like `ChannelManager`)

---

## Decisions

- **Rename `trunk` to `trunking`** -- reads as an operating mode. Keep `trunk` as hidden alias.
- **Use `--freq` (not `--frequency`)** -- SDR convention, saves keystrokes. Keep `--frequency` as hidden alias.
- **Both repeatable AND comma-delimited** -- `-f 852.35M -f 853.35M` and `-f 852.35M,853.35M` both work (clap `value_delimiter`).
- **Include CC-at-offset** -- trunking mode takes `--center-freq` (capture center) + `-f` (CC frequencies). Real-world need.

---

## CLI Changes

### 1. Human-Friendly Frequency Parser

Add a `parse_frequency` value parser that accepts suffixes:

| Input | Parsed |
|-------|--------|
| `852350000` | 852,350,000 Hz |
| `852.35M` | 852,350,000 Hz |
| `852350k` | 852,350,000 Hz |

Case-insensitive suffixes: `k`/`K` = 1e3, `M` = 1e6, `G` = 1e9. No suffix = Hz. Lives near the CLI definitions in `main.rs` or a small `src/cli.rs` module.

Same parser reused for `--sample-rate` (`2.4M` instead of `2400000`).

### 2. File Path as Positional Argument

Change from `--input file.iq` to a positional argument. Keep `--device` as a flag:

```
p25 cc recording.iq                          # file (common case)
p25 cc --device "driver=rtlsdr" -f 852.35M   # live SDR
```

`--input` becomes a hidden alias for backward compatibility. Clap enforces mutual exclusivity between the positional arg and `--device`.

### 3. Unified `--freq` for CC Frequencies

`--freq` / `-f` consistently means "control channel frequency" across both subcommands. Repeatable and comma-delimited:

**`p25 cc`**: `--freq` is the CC frequency (repeatable for hopping). First is the initial tune target.
```
p25 cc -d "driver=rtlsdr" -f 852.35M --gain 40                     # single CC
p25 cc -d "driver=rtlsdr" -f 852.35M -f 853.35M --gain 40          # CC hopping
p25 cc -d "driver=rtlsdr" -f 852.35M,853.35M --gain 40             # same, comma form
```

**`p25 trunking`**: `--center-freq` is the capture center (physical SDR tuning). `--freq` is where the CC lives within the capture. If `--freq` omitted, defaults to `--center-freq`.
```
p25 trunking wideband.iq --center-freq 852.35M                       # CC at center
p25 trunking wideband.iq --center-freq 852M -f 852.35M               # CC at offset
p25 trunking wideband.iq --center-freq 852M -f 852.35M -f 853.19M   # CC hopping within capture
```

Internally: `Vec<Frequency>`, first element is the initial CC.

### 4. Shared `CommonArgs` Struct

Extract duplicated fields from `Cc` and `Trunking`:

```rust
#[derive(Args)]
struct CommonArgs {
    #[command(flatten)]
    source: InputSource,
    #[command(flatten)]
    gain_control: GainControl,
    #[arg(short, long, default_value = "2.4M", value_parser = parse_frequency_u32)]
    sample_rate: u32,
    #[arg(short, long)]
    modulation: Option<CliModulation>,  // default differs per subcommand
}
```

Subcommand-specific defaults: `cc` defaults to `c4fm`, `trunking` defaults to `cqpsk` (resolved at runtime, not in clap).

### 5. System ID Filter

Add `--system-id` to filter by SysID on multi-system sites:

```
p25 cc recording.iq --system-id 0x5F2
p25 trunking wideband.iq --center-freq 852.35M --system-id 0x5F2
```

Optional. When present, ignore grants/events from non-matching systems. The Sacramento site broadcasts grants for SysIDs 0x704/0xB04/0xD04 that currently pollute output.

### 6. Global Flags

Add `--verbose` / `--quiet` to the top-level `Cli` struct:

```
p25 -v cc recording.iq          # debug logging
p25 -vv cc recording.iq         # trace logging
p25 --quiet cc recording.iq     # suppress stderr
```

Uses `global = true` so flags work before or after the subcommand.

### 7. Rename `trunk` to `trunking`

Rename the subcommand. Keep `trunk` as a hidden alias (`#[command(alias = "trunk")]`) so existing scripts don't break.

---

## Resulting CLI Surface

```bash
# -- File decode (most common) --
p25 cc recording.iq
p25 cc recording.iq -m cqpsk
p25 cc recording.iq --system-id 0x5F2

# -- Live SDR --
p25 cc -d "driver=rtlsdr" -f 852.35M --gain 40
p25 cc -d "driver=rtlsdr" -f 852.35M -f 853.35M --auto-gain    # CC hopping
p25 cc -d "driver=rtlsdr" -f 852.35M,853.35M --auto-gain       # same, comma form

# -- Wideband trunking --
p25 trunking recording.iq --center-freq 852.35M                  # CC at center
p25 trunking recording.iq --center-freq 852M -f 852.35M          # CC at offset
p25 trunking recording.iq --center-freq 852M -f 852.35M,853.19M  # CC hopping
p25 trunking -d "driver=rtlsdr" --center-freq 852.35M --gain 40

# -- Pipeline composition --
p25 cc -d "driver=rtlsdr" -f 852.35M --gain 40 | p25 monitor
p25 trunking recording.iq --center-freq 852.35M | tee output.jsonl | p25 monitor

# -- Utility --
p25 devices
p25 --version
```

---

## Architecture Changes

### SoapySource Retune

Store `Device` in `SoapySource` (currently dropped after `open()`). Add:

```rust
impl SoapySource {
    pub fn retune(&mut self, frequency: Frequency) -> Result<(), SdrError> { ... }
}
```

Add matching method on `SampleSource` enum (file variant returns error).

**Files:** `src/sdr/soapy_source.rs`, `src/main.rs`

### CC Tracker (`src/cc_tracker.rs`)

Small struct that manages CC hopping logic, following the same pattern as `ChannelManager`:

- Owns the CC frequency list (`Vec<Frequency>`)
- Dynamically adds frequencies learned from SCCB (opcode 0x38), RFSS_STS_BCST (0x3A)
- Tracks time since last valid TSBK
- On timeout (configurable, default 5s): returns next frequency to try
- Emits JSON events: `cc_lost`, `cc_retune`, `cc_acquired`

Used by `decode_control_channel()` as a collaborator. For file input, it emits `cc_lost` events but cannot retune. For live SDR, it triggers `SampleSource::retune()`.

**Files:** new `src/cc_tracker.rs`, modified `src/main.rs`

### Wideband CC-at-Offset

In `p25 trunking`, when `--freq` differs from `--center-freq`, the CC pipeline uses an NCO to shift the CC frequency to baseband -- same mechanism already used for voice channels. The `ChannelManager` spawns the CC pipeline at the offset frequency instead of assuming DC.

For CC hopping in wideband mode: if the first CC goes silent and a second frequency in the `--freq` list is within the capture bandwidth, the `CcTracker` can switch to it via NCO re-channelization -- no physical retune needed. This is a major advantage of wideband mode (RF Expert: "strongly prefer wideband for CC resilience").

**Files:** `src/channel_manager.rs`, `src/main.rs`

---

## RF Protocol Details (from RF Expert)

### How CC Hopping Works in Practice

1. **Site failover**: System controller moves CC to a different frequency. Announced via RFSS_STS_BCST (0x3A) showing new CC channel number before switchover.
2. **SCCB (opcode 0x38)**: Broadcasts alternate CC frequencies on the primary CC. Subscriber units cache these for fallback.
3. **Voice LC 0x21/0x26**: Alternate CC info carried in LDU1 frames during active calls, so units on voice channels know where to go when the call ends.
4. **Subscriber fallback sequence**: Last-known SCCB frequencies -> ADJ_STS_BCST neighbors -> programmed scan list -> full-band scan.

### What We Need to Parse

- **Opcode 0x38 (SCCB)**: Not yet parsed. Carries alternate CC channel+services pairs. Add to `tsbk.rs`.
- **LC 0x21/0x26**: Enum variants exist in `voice/control.rs` but payloads not decoded. Lower priority -- these are in voice frames, not the CC stream.

### Pipeline Reset on Retune

After retuning, all DSP state (FIR filter history, Costas loop, Gardner timing) is invalid. Create a fresh `ChannelPipeline` instance. Re-lock takes ~50-100ms at 4800 sym/s -- well within the 2-second dwell time per frequency.

---

## Implementation Phases

### Phase 1: CLI Polish (no behavioral changes)
1. Add `parse_frequency` value parser with suffix support (k/M/G)
2. Make file path positional (keep `--input` as hidden alias)
3. Rename `trunk` to `trunking` (keep `trunk` as hidden alias)
4. Unify `--freq` / `-f` across both subcommands (repeatable + comma-delimited)
5. Extract `CommonArgs` struct
6. Add `-v`/`--quiet` global flags
7. Add examples to help text (`after_help`)
8. Ensure all existing behavior is preserved (regression: same TSBK counts, same JSON)

### Phase 2: CC Hopping Infrastructure
9. Store `Device` in `SoapySource`, add `retune()` method
10. Add `retune()` to `SampleSource` enum
11. Build `CcTracker` struct (frequency list, timeout, rotation)
12. Wire `CcTracker` into `decode_control_channel()` loop
13. Emit `cc_lost`/`cc_retune`/`cc_acquired` JSON events
14. Parse SCCB (opcode 0x38) for dynamic CC frequency discovery

### Phase 3: Wideband Flexibility
15. Support CC-at-offset in `ChannelManager` (NCO shift for CC pipeline)
16. Wire `--freq` in trunking mode to specify CC offset from `--center-freq`
17. Add `--system-id` filter for multi-system sites
18. CC hopping within wideband capture via NCO re-channelization (no retune needed)

### Future (not in this plan)
- Config file support (`--config system.toml`)
- Remote SDR (rtl_tcp / SoapyRemote)
- Input format auto-detection (`.u8`, `.wav`, `.sigmf-data`)
- Output format flag (`--output-format json|csv|table`)
- `p25 scan` / `p25 record` subcommands

---

## Verification

### Phase 1
- `cargo test` -- all existing tests pass
- `p25 cc samples/iq/srrcs_cc_852350000_2400k_cf32.iq` -- positional arg works
- `p25 cc samples/iq/srrcs_cc_852350000_2400k_cf32.iq -m cqpsk` -- same 1223 TSBKs
- `p25 trunking samples/iq/srrcs_wideband_852350000_2400k_cf32.iq --center-freq 852.35M` -- same output as `trunk`
- `p25 trunk ...` -- hidden alias still works
- `p25 cc --help` -- shows examples, accepts `852.35M` format
- `p25 -v cc ...` -- debug logging to stderr
- `p25 cc --input samples/iq/srrcs_cc_852350000_2400k_cf32.iq` -- hidden alias still works

### Phase 2
- Unit tests for `CcTracker` (timeout detection, frequency rotation, dynamic add)
- Integration test: file input with no TSBKs emits `cc_lost` event
- Manual test with live SDR: force CC loss by detuning, verify retune cycle

### Phase 3
- `p25 trunking ... --center-freq 852.35M` -- same results as before (CC at center, --freq omitted)
- `p25 trunking ... --center-freq 852M -f 852.35M` -- CC decoded via NCO offset, same TSBK count
- `p25 trunking ... --system-id 0x5F2` -- filters out SysID 0x704/0xB04/0xD04 grants

---

## Key Files

| File | Changes |
|------|---------|
| `src/main.rs` | CLI restructure (CommonArgs, positional file, global flags, freq parser, `trunking` rename) |
| `src/sdr/soapy_source.rs` | Store `Device`, add `retune()` |
| `src/cc_tracker.rs` | **New** -- CC hopping state machine |
| `src/channel_manager.rs` | Support CC pipeline at offset frequency, CC hopping within wideband |
| `src/p25/tsbk.rs` | Parse SCCB (opcode 0x38) |
| `src/p25/types.rs` | No changes needed |
| `src/output/json.rs` | Add `cc_lost`/`cc_retune`/`cc_acquired` event types, `--system-id` filter field |

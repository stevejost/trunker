# Managed Feeder Startup Design

Date: 2026-02-22

## Problem

p25-server currently takes all configuration via CLI arguments. Integrating with
trunker-web requires fetching system config (frequencies, LiveKit tokens, system
metadata) from an API at startup, and automatically hunting for the active
control channel from a list of candidates.

Additionally, the long CLI invocation is unwieldy for service deployments. An
`.env` file should hold stable per-installation secrets and hardware config.

## Goals

1. Fetch system configuration from the trunker-web API on startup (managed mode).
2. Automatically find and lock onto the active control channel from a candidate list.
3. Support `.env` files for secrets and hardware config.
4. Preserve existing standalone mode (all CLI args, no API dependency).
5. Backward compatible: existing invocations work unchanged.

## Non-Goals (Deferred)

- Full reconnect orchestration (log + exit for now; let systemd restart).
- Hot config refresh.
- API-driven sample rate recommendation.
- Config file formats (TOML/YAML). `.env` is sufficient.

---

## Operating Modes

Two modes, implicitly detected based on which arguments are present:

```
Managed mode:    TRUNKER_FEEDER_ID + TRUNKER_API_KEY both set
Standalone mode: everything else (current behavior, unchanged)
```

No `--mode` flag. No subcommands. Detection logic:

```rust
match (&cli.feeder_id, &cli.api_key) {
    (Some(_), Some(_)) => ServerMode::Managed,
    (None, None)       => ServerMode::Standalone,
    _                  => bail!("set both TRUNKER_FEEDER_ID and TRUNKER_API_KEY, or neither"),
}
```

---

## Configuration

### Precedence (highest wins)

```
CLI flag  >  environment variable  >  .env file  >  API response  >  compiled default
```

### .env File

Loaded via `dotenvy` before clap parses. Clap's `env = "..."` attributes pick
up values automatically. No custom merge logic needed.

Default location: `./.env` (current working directory).
Override via: `TRUNKER_ENV_FILE` environment variable (avoids two-pass CLI parse).

**IMPORTANT:** SoapySDR device setting keys (`rfgain_sel`, `hdr_ctrl`,
`biasT_ctrl`) have specific mixed casing that the driver requires. The
`TRUNKER_SETTINGS` value must preserve key casing exactly. No case
normalization anywhere in the parse chain.

### Sample .env

```bash
# === Managed mode (trunker-web API) ===
TRUNKER_FEEDER_ID=a1b2c3d4-e5f6-7890-abcd-ef1234567890
TRUNKER_API_KEY=tk_live_abc123def456...
TRUNKER_API_URL=https://trunker.example.com

# === SDR hardware (API cannot know local hardware) ===
TRUNKER_DEVICE=driver=sdrplay
TRUNKER_ANTENNA=Antenna C
TRUNKER_AUTO_GAIN=true
TRUNKER_SAMPLE_RATE=3600000
TRUNKER_BUFFER_MS=500
TRUNKER_AUDIO_GAIN=16.0

# === Device-specific pass-through settings ===
# Comma-separated key=value pairs. Casing is preserved exactly.
TRUNKER_SETTINGS=rfgain_sel=4,hdr_ctrl=false,biasT_ctrl=false
```

### Parameter Sources

| Parameter | Managed mode source | Override |
|-----------|-------------------|----------|
| center_frequency | API `sdr.centerFrequency` | `--center-freq` / `TRUNKER_CENTER_FREQ` |
| sample_rate | Operator (must cover API `sdr.bandwidth`) | `--sample-rate` / `TRUNKER_SAMPLE_RATE` |
| control_channels | API `sdr.controlChannels` | None (API is authoritative) |
| device, antenna, gain, settings | Operator only | `.env` / CLI |
| system_id, wacn, site_id | API `system.*` | CLI override |
| livekit_url, livekit_token, room | API response | None in managed mode |

### Sample Rate Validation

The SDR sample rate must cover the full network frequency spread (voice channels
included, not just control channels). The API provides `sdr.bandwidth` (Hz)
which equals `freqTop - freqBottom`. The sample rate is rounded up to a value
that divides cleanly through the decimation stages and is supported by the SDR
hardware.

```
if sample_rate < api.sdr.bandwidth:
    ERROR: sample rate {sample_rate} Hz < network bandwidth {bandwidth} Hz
    hint: set TRUNKER_SAMPLE_RATE >= {bandwidth}
```

### New CLI Arguments

Added to the existing flat `Cli` struct:

```
--feeder-id <UUID>     env: TRUNKER_FEEDER_ID
--api-key <TOKEN>      env: TRUNKER_API_KEY
--api-url <URL>        env: TRUNKER_API_URL    default: https://trunker.app
```

All existing arguments remain unchanged. In managed mode, arguments that the API
provides (`--livekit-url`, `--frequency`, `--center-freq`, `--system-id`, etc.)
become optional — the API fills them in. CLI values still override API values.

---

## Managed Startup Sequence

```
 1. Load .env (dotenvy, from TRUNKER_ENV_FILE or ./.env)
 2. Parse CLI (clap)
 3. Detect mode (managed vs standalone)
 4. GET /api/feeder/{id}/config
    - 3 retries with backoff: 1s, 2s, 4s
    - Fatal exit if all retries fail
 5. Merge config: CLI overrides > API values (fill in Options still None)
 6. Validate: sample_rate >= network bandwidth, all required fields present
 7. Open SDR (existing init sequence in soapy_source.rs, unchanged)
 8. Hunt for active control channel (parallel NCO scan)
 9. Lock CC, create ChannelManager
10. Connect to LiveKit (using API-provided token + url)
11. Open WebSocket command channel
12. Start heartbeat loop (POST every 30s)
13. Begin trunked decode
```

### API Failure Handling

- 3 retries with 1s/2s/4s backoff, then fatal exit.
- Clear error message with hints for each failure mode:
  - Connection refused: check TRUNKER_API_URL and network
  - 401: check TRUNKER_API_KEY, regenerate from dashboard
  - 400 "not assigned": assign feeder in dashboard
  - 404: system was deleted

---

## Control Channel Hunting

### Approach

Parallel NCO scanning of all CC candidates simultaneously on the main decode
thread. The SDR captures wideband IQ (sample rate covers full network spread),
and all CC candidates are within that bandwidth.

One lightweight `ChannelPipeline` per candidate frequency, NCO-shifted from the
center frequency. Same pattern as voice channels in `channel_manager.rs`.

### Detection Threshold

**2 valid TSBKs with CRC pass.** A P25 control channel emits ~25-30 TSBKs/sec
continuously. Two valid TSBKs with DUID=0x7 (Trunking Signaling Data Unit) is
definitive CC identification. If NAC is known (from the API's `systemId`), only
count TSBKs with matching NAC.

### Timing

| Phase | Duration |
|-------|----------|
| CQPSK carrier acquisition (Costas loop) | 200-500 ms |
| First valid TSBK after acquisition | 5-35 ms |
| Second valid TSBK (lock confirmed) | +35 ms |
| **Total cold-start to CC lock** | **300-600 ms** |

### State Machine

```
HUNTING ──(2 valid TSBKs on a candidate)──> LOCKED
   ^                                           |
   |                                           v
   |                                      MONITORING
   |                                      (normal trunked decode)
   |                                           |
   +──────(no TSBKs for 10 seconds)────────────+
```

**HUNTING:**
- Create N candidate pipelines (NCO + ChannelPipeline each).
- Feed every IQ sample to all N candidates simultaneously.
- Track `tsbk_count`, `last_tsbk_sample`, `nac` per candidate.
- First to reach 2 valid TSBKs wins.
- Hunt timeout: 5 seconds with no winner, log warning, reset, retry.

**LOCKED (transient):**
- Winner's pipeline becomes primary CC.
- All other candidate pipelines destroyed.
- `ChannelManager` created for voice channel tracking.
- IDENT_UP TSBKs decoded during hunting seed the ident table.

**MONITORING:**
- Normal `decode_trunked` loop.
- Track `last_tsbk_timestamp` via sample counter.
- Warning at 3 seconds without TSBKs ("CC degraded").
- CC lost at 10 seconds, trigger re-hunt.

**RE-HUNT:**
- Tear down `ChannelManager` (all voice channels).
- Destroy CC pipeline.
- Re-enter HUNTING with all candidates.
- Seed new candidate Costas loops from last-known state.

### Bandwidth Validation

All CC candidates must fit within the SDR capture bandwidth:

```
for each candidate in control_channels:
    offset = abs(candidate - center_frequency)
    if offset > sample_rate / 2:
        WARN: candidate {freq} outside capture bandwidth, skipping
```

If zero candidates are within bandwidth, fatal error.

### New Module

`crates/trunker/src/decode/cc_hunter.rs` (~200 lines):

```rust
struct CcHunter {
    candidates: Vec<CandidateChannel>,
    state: HuntState,
    config: CcHunterConfig,
}

struct CandidateChannel {
    frequency: Frequency,
    offset_hz: f64,
    nco: Nco,
    pipeline: ChannelPipeline,
    tsbk_count: u32,
    last_tsbk_sample: u64,
    nac: Option<Nac>,
}

enum HuntState {
    Hunting,
    Locked { winner_index: usize },
    Lost,
}
```

---

## Runtime Services

### Heartbeat

Tokio task. `POST /api/feeder/{id}/heartbeat` every 30 seconds with Bearer
token auth. Log warnings on failure, never crash. Respects `running` AtomicBool
for clean shutdown.

### WebSocket Command Channel

Connect to `WS /api/feeder/{id}/ws?token={api_key}` after LiveKit connects.
Auto-reconnect on disconnect (backoff: 1s, 2s, 4s, cap 30s).

Currently one command: `{"type": "reconnect"}`. For MVP, log the command and
exit cleanly (let systemd restart). Full reconnect orchestration deferred.

---

## New Dependencies

| Crate | Purpose |
|-------|---------|
| `dotenvy` | Load .env files into process environment |
| `reqwest` (rustls-tls) | HTTP client for config fetch + heartbeat |
| `tokio-tungstenite` | WebSocket command channel |

`reqwest` chosen over `ureq` because the heartbeat loop needs async HTTP, and
tokio is already a dependency via LiveKit.

## New and Modified Files

**New modules:**

| Module | Location | ~Lines |
|--------|----------|--------|
| `config.rs` | `crates/p25-server/src/` | ~80 |
| `heartbeat.rs` | `crates/p25-server/src/` | ~40 |
| `command.rs` | `crates/p25-server/src/` | ~60 |
| `cc_hunter.rs` | `crates/trunker/src/decode/` | ~200 |

**Modified files:**

| File | Changes |
|------|---------|
| `p25-server/src/main.rs` | .env loading, mode detection, API fetch, startup sequencing |
| `p25-server/Cargo.toml` | New deps: dotenvy, reqwest, tokio-tungstenite |
| `trunker/src/decode/trunked.rs` | Accept `Vec<Frequency>` for CC candidates, integrate CcHunter |

**Unchanged:**
- `soapy_source.rs` (no SDR code changes)
- `pipeline.rs` (CC hunter reuses existing ChannelPipeline)
- `channel_manager.rs` (voice channel logic untouched)
- `p25` CLI binary (completely unaffected)
- All DSP and protocol code

## MVP Task List

1. Add .env loading + new CLI args [S]
2. Define API response types in config.rs [S]
3. Config fetch + mode detection + merge [M]
4. Heartbeat loop [S]
5. WebSocket command channel [M]
6. CC hunter module [L]
7. Integrate CC hunter into trunked.rs [M]
8. Integrate managed startup into p25-server main.rs [M]

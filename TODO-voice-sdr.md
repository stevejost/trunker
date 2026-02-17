# Voice Decode + Streaming Architecture Brainstorm

## Context

Trunker currently decodes P25 control channel TSBKs (1223 via CQPSK, beating OP25 by 23%) and emits JSON. The next major feature is **voice decoding** — capturing voice channel audio and streaming it to browsers in real-time. This document synthesizes analysis from RF Engineering, Backend/RTC, and Fullstack/PO perspectives.

---

## The Vision

1. **N+1 SDR setup**: 1 RTL-SDR for CC + N voice SDR(s) (RSPdx, multiple RTL-SDRs, or Airspy)
2. Software channelizes individual 12.5 kHz voice channels from each voice SDR
3. Custom IMBE vocoder (built from TIA-102.BABA spec + p25.rs reference) decodes voice frames to PCM
4. Each discovered talkgroup gets a **persistent WebRTC stream** via Janus Gateway
5. Browser UI lets users subscribe to talkgroup streams, "hold" on departments, priority-based playback

---

## RF Engineering Assessment

### N+1 SDR Architecture (User's Choice)

**1 dedicated CC dongle + N voice SDR(s)**, flexible hardware:

| Config | Hardware | Coverage | Cost |
|--------|----------|----------|------|
| 1+1 RSPdx-R2 | RTL-SDR (CC) + RSPdx-R2 (voice) | Full 3 MHz, 14-bit, preselection filters | ~$280 |
| 1+1 RSP1B | RTL-SDR (CC) + RSP1B (voice) | Full 3 MHz, 10-bit (consider ext. bandpass) | ~$140 |
| 1+1 Airspy Mini | RTL-SDR (CC) + Airspy Mini (voice) | Full 6 MHz, 12-bit | ~$130 |
| 1+3 RTL-SDR | RTL-SDR (CC) + 3x RTL-SDR (voice) | ~90-95% coverage, overlapping | ~$120 |

**Multi-RTL-SDR details** (the budget-friendly option):
- Each RTL-SDR: ~2.0-2.2 MHz usable bandwidth at 2.4 MS/s (8-bit)
- 3 dongles at 1 MHz center spacing covers 851-854 MHz with ~700 kHz overlap zones
- Example: V1=851.5, V2=852.5, V3=853.5 MHz centers
- TCXO required (RTL-SDR Blog V3/V4) — without TCXO, 60 ppm = ±51 kHz drift at 852 MHz, which falls outside the 6.25 kHz decimation filter passband
- Channels in overlap zones: pick the dongle with closest center frequency

**USB bandwidth**: 4.58 MB/s per RTL-SDR dongle at 2.4 MS/s. 4 dongles = 18.3 MB/s total. USB 2.0 practical max = 35-40 MB/s. Safe, but use a powered USB 3.0 hub and spread across controllers if possible. Max ~3-4 RTL-SDRs per USB 2.0 controller before scheduling jitter causes sample drops.

### Channelization: Per-Channel DDC (Start Here)

**Two approaches exist:**

| Approach | How | Cost | When |
|----------|-----|------|------|
| **Per-channel DDC** | NCO frequency shift + existing 2-stage decimation per active channel | ~127 MFLOPS/channel, linear scaling | **Start here** — reuses existing `DecimatingFilter` |
| **Polyphase filterbank** | Single FFT-based channelizer splits all channels simultaneously | ~102 MMAC/s total for ALL channels | Optimization if >20 channels needed |

**Why DDC first**: We only channelize active channels (from CC grants, typically 8-15 simultaneous). The existing filter chain is tested and working. P25 channels don't always align to a uniform grid (SRRCS uses 6250 Hz spacing), which complicates PFB. The NCO (numerically controlled oscillator) is ~10 lines of new DSP code.

**Per-channel resource cost**: ~4 KB memory. 20 channels = 2.54 GFLOPS total — well within a modern multi-core CPU.

### Voice Channel DSP: 100% Reusable

The existing DSP pipeline works unchanged for voice channels:
- **Same modulation**: C4FM/CQPSK at 4800 sym/s, 12.5 kHz bandwidth
- **Same frame sync**: 48-bit pattern `0x5575F5FF77FF` before every data unit
- **Same status symbols**: Every 36 dibits, same `StatusDeinterleaver`
- **Same symbol timing**: Gardner, Costas, all CQPSK components identical

**What changes is the protocol layer only** — voice data units (HDU, LDU1, LDU2, TDU) use different FEC coding (Golay/Hamming instead of trellis) and carry IMBE voice frames instead of TSBKs.

### Voice Frame Structure

```
Voice call on traffic channel:
  HDU -> LDU1 -> LDU2 -> LDU1 -> LDU2 -> ... -> TDU

Each LDU: 1728 dibits containing:
  - 9 IMBE voice frames (each 72 dibits = 144 coded bits -> 88 data bits -> 20ms audio)
  - Link Control (LDU1) or Crypto Control (LDU2) — 72 bits spread across 6 chunks
  - Low-speed data — 16 bits

One LDU = 180ms of audio (9 frames x 20ms)
```

### Key Risks

1. **Late entry**: When tuning to a call in progress, you miss the HDU. Not a problem for unencrypted traffic (LC in LDU1 has talkgroup/source). Fatal for encrypted traffic (no key material without HDU).
2. **Simulcast multipath**: SRRCS is simulcast. Voice LDUs are much longer than TSBKs (1728 vs 130 dibits), so more vulnerable to fading. IMBE's FEC (Golay + Hamming) provides tolerance up to ~3% BER. Start without equalizer, measure, add if needed.
3. **IMBE vocoder**: Building our own from TIA-102.BABA spec + p25.rs reference (same approach as the rest of the project). Not using mbelib/codec2 — own implementation, own quality.

### SDR Strategy: N+1 with SDR Registry

CC dongle is always dedicated and uninterrupted. Voice SDR(s) are registered with their center frequency and usable bandwidth. Grant tracker maps voice channel frequencies to the appropriate SDR.

**Software design**: An `SdrRegistry` holds all voice SDR devices. When a `GRP_V_CH_GRANT` arrives, the registry answers "which SDR covers frequency X?" and the channelizer on that SDR extracts the channel. If no SDR covers the frequency, the grant is logged as unserviceable.

---

## RTC/Backend Architecture

### Protocol: WebRTC + Janus SFU (with WebSocket signaling)

**RTSP is wrong for browsers** — no browser supports it natively.

**WebRTC is the right choice** under a persistent per-TGID stream model:

| Protocol | Latency | Browser Support | Bursty Audio | Verdict |
|----------|---------|-----------------|-------------|---------|
| **WebRTC (persistent streams)** | 60-150ms | Universal | Excellent (Opus DTX) | **Chosen** |
| WebSocket + Opus | 100-400ms | Universal | Good but TCP HOL blocking | Fallback option |
| HLS/DASH LL | 1-5 seconds | Universal | Poor | Too slow for radio |
| RTSP | N/A | None | N/A | Dead on arrival |

### Stream Model: Persistent Per-Talkgroup Streams

- When the CC first discovers a TGID, a **permanent Janus mountpoint** is created for it
- Streams are **never torn down** — they persist through silence, frequency changes, etc.
- SRRCS system: ~60 talkgroups = ~60 persistent mountpoints
- Users subscribe to streams of interest via **single PeerConnection, multiple audio tracks**
- During active calls: Opus audio flows through the already-established track
- During silence: No packets sent (Opus DTX). Track stays "live" but silent. Zero bandwidth.

**Connection topology: Single PeerConnection per user, multiple audio tracks.**
- ICE negotiation happens once at connect time (not per talkgroup)
- One DTLS session, shared congestion control
- Subscribing to a new TGID = renegotiate (add track) on existing PeerConnection — signaling only, no new ICE round
- Janus "multistream" mode: one Janus session subscribes to multiple mountpoints, each becomes a separate audio track
- 60 idle tracks on one PeerConnection: ~200 KB memory, negligible CPU/bandwidth

**Why this works**: WebRTC's main cost (ICE negotiation, 1-3 sec) is amortized to zero because streams are persistent. Audio latency is just encode + jitter buffer = ~60-150ms.

### Idle Stream Cost (Negligible)

| Resource | Per idle track | 60 tracks/user | 10 users x 60 tracks |
|----------|---------------|-----------------|----------------------|
| Memory | 2-4 KB | ~200 KB | ~2 MB |
| CPU | ~0 | ~0 | ~0 |
| Network | ~200 B/5s (RTCP) | ~2.4 KB/s | ~24 KB/s |

### WebRTC Server: Janus Gateway + Streaming Plugin

**Janus Streaming Plugin** is purpose-built for this exact pattern: external source pushes RTP, Janus relays to WebRTC subscribers.

```
[Rust voice decoder] --Opus/RTP over local UDP--> [Janus Streaming Plugin] --WebRTC--> [Browsers]
```

Integration:
1. Rust process encodes IMBE -> PCM -> Opus, packetizes as RTP
2. Sends RTP packets to Janus via localhost UDP (one port per TGID mountpoint)
3. Janus handles ICE, DTLS-SRTP, and fan-out to all subscribers
4. Janus admin API (HTTP/WS) for creating/destroying mountpoints when TGIDs are discovered

Per-mountpoint Janus config:
```
rtp-audio-tg100: {
    type = "rtp", id = 100, description = "Sac Sheriff Dispatch"
    audio = true, audioport = 15100, audiopt = 111
    audiortpmap = "opus/48000/2"
}
```

**Why Janus over alternatives:**
- **mediasoup**: Node.js-centric, optimized for bidirectional conferencing (overkill here)
- **Pion (Go)**: Toolkit, not product — you'd build your own SFU from scratch
- **webrtc-rs**: Early stage, architecture in flux — not production-ready yet

### Hybrid Architecture: WebRTC Audio + WebSocket Signaling

This is the standard pattern for WebRTC deployments:

**WebSocket channel** (reliable, ordered):
- Janus signaling (subscribe/unsubscribe to mountpoints)
- TSBK metadata events (the JSON that `p25 cc` already emits)
- Call start/end events (from GRP_V_CH_GRANT)
- System status, emergency alerts, talkgroup discovery

**WebRTC channel** (low-latency, UDP):
- Decoded P25 voice audio per talkgroup
- One audio track per subscribed talkgroup
- Opus encoded, bursty (active during calls, silent between via DTX)

### Audio Pipeline

```
IMBE 88-bit frame -> IMBE vocoder -> 160 samples @ 8kHz (20ms PCM)
                                          |
                                    Opus encode (VOIP mode, 8kHz mono, ~16kbps)
                                          |
                                    RTP packetize -> UDP to Janus (localhost)
                                          |
                                    Janus SFU -> WebRTC -> Browser
```

Opus frame size = 20ms at 8kHz = 160 samples. **Matches IMBE frame boundaries exactly.** Zero resampling needed.

### Fan-out

1 Rust decoder -> 1 RTP stream to Janus -> N subscribers via SFU forwarding. Per subscriber cost: one SRTP re-encrypt + one UDP sendto per packet. At Opus 32kbps = ~50 packets/sec per stream. 5 users on same TG = 250 SRTP ops/sec. Trivial.

### Thread Architecture

```
[OS Thread per SDR] SDR reader + DSP pipeline (CPU-bound, must not block async executor)
    | mpsc channels
[Tokio Runtime] Grant tracker, SDR registry, Opus encoder, RTP sender, Axum WS signaling
    | UDP localhost (one port per TGID mountpoint)
[Janus Gateway] SFU fan-out, ICE/DTLS/SRTP, WebRTC delivery
    | Single PeerConnection per user, multiple audio tracks
[Browser Client] Native WebRTC audio pipeline (jitter buffer, playback per track)
```

---

## Product Owner: Phasing & Scope

### The Unix Philosophy Tension

CLAUDE.md says: *"This is not a scanner application."* Adding a web UI with audio streaming IS a scanner application.

**Resolution: Separate binaries that compose.**

```
p25 cc        -- control channel decoder (EXISTS, emits JSON)
p25 voice     -- voice channel decoder (NEW, emits PCM/IMBE)
p25 trunk     -- trunking follow orchestrator (NEW, multi-channel)
p25 serve     -- web server wrapping trunk (NEW, the scanner app)
p25 monitor   -- TUI monitor (EXISTS, reads JSON from stdin)
```

First three maintain Unix philosophy. `p25 serve` is the explicit departure point.

### Phased Delivery

| Phase | Deliverable | Effort | Dependencies |
|-------|------------|--------|-------------|
| **0** | Voice frame extraction -> IMBE hex JSON on stdout | 2-4 wk | Extend receiver, port Golay/Hamming/RS from p25.rs |
| **1** | Single-channel voice -> WAV file | 3-5 wk | Custom IMBE vocoder (from TIA-102.BABA spec + p25.rs ref) |
| **2** | N+1 SDR trunking follow (multi-channel, multi-dongle) | 6-10 wk | SDR registry, channelizer (NCO + DDC), trunking state machine |
| **3** | WebRTC streaming via Janus + signaling server | 8-12 wk | Axum WS signaling, Opus/RTP to Janus, Janus admin API |
| **4** | Full UI with hold/priority/PWA | 4-6 wk | Svelte SPA, talkgroup management, client-side mixing |

**Phase 0 is the gate.** If we can't extract IMBE frames correctly, nothing else matters.

### What Already Exists (Reusable)

**DSP (no changes needed):**
- `src/dsp/filter.rs` -- `DecimatingFilter` (one instance per DDC channel)
- `src/dsp/cqpsk_demod.rs` -- `CqpskDemodulator` (one per voice channel)
- `src/dsp/fm_demod.rs`, `slicer.rs`, `agc.rs`, `costas.rs`, `gardner.rs`, `diff_decoder.rs`, `rrc_filter.rs`
- `src/dsp/sync.rs` -- `SyncDetector` (frame sync identical for voice)
- `src/p25/status.rs` -- `StatusDeinterleaver` (same status symbol removal)
- `src/p25/nid.rs` -- Already recognizes all voice DUIDs (HDU=0x0, LDU1=0x5, LDU2=0xA, TDU=0x3, TDULC=0xF)

**Protocol (extend, don't replace):**
- `src/p25/receiver.rs` -- Currently `State::Skip` for voice DUIDs (line 116). Add `CollectLdu1`, `CollectLdu2`, `CollectHdu` states.

**Reference implementation to port from (`~/source/p25.rs/`):**
- `src/voice/frame_group.rs` -- LDU receiver state machine (~400 lines)
- `src/voice/frame.rs` -- IMBE frame decoder (Golay, Hamming, PN descramble)
- `src/voice/descramble.rs` -- PN sequence descrambling
- `src/voice/rand.rs` -- Pseudo-random sequence generator
- `src/voice/control.rs` -- Link Control fields (LDU1)
- `src/voice/crypto.rs` -- Crypto Control fields (LDU2)
- `src/coding/golay.rs`, `hamming.rs`, `reed_solomon.rs`, `cyclic.rs` -- FEC decoders

### New Code Needed

| Component | Complexity | Notes |
|-----------|-----------|-------|
| NCO (frequency shift) | Trivial | ~10 lines: complex multiply with rotating phasor |
| Golay(23,12) decoder | Medium | Port from p25.rs `src/coding/golay.rs` |
| Hamming decoders | Medium | Port from p25.rs `src/coding/hamming.rs` |
| Reed-Solomon (short+medium) | Hard | Port from p25.rs `src/coding/reed_solomon.rs` |
| LDU frame group receiver | Medium | Port/adapt from p25.rs `src/voice/frame_group.rs` |
| IMBE vocoder (custom) | Hard | Build from TIA-102.BABA spec + p25.rs reference. Pitch estimation, spectral envelope, excitation modeling. |
| Trunking state machine | Hard | Grant tracking, channel alloc, late entry handling |
| Wideband channelizer | Medium | NCO + existing filter chain, one per active channel |
| Opus encoder | Easy | audiopus crate wraps libopus |
| Axum WebSocket server | Easy | Standard Axum + tokio::sync::broadcast |
| Svelte browser UI | Medium | AudioWorklet playback, talkgroup subscription |

---

## Fullstack: Browser UI

### Framework: Svelte + Vite (static SPA)

- No SSR needed (all data is real-time WebSocket)
- Smallest bundle size, best reactive performance
- Ship as static files embedded in Rust binary via `rust-embed`
- PWA for mobile support (people use scanners on phones)

### "Listen to Sac Sheriff" = Client-Side Mixing

**Server pushes all subscribed TG streams. Client decides what to play.**

- Emergency calls preempt everything
- User-configured priority per TG (drag to reorder)
- Currently-active call holds priority (no flip-flopping)
- Visual indicators for all active TGs even when muted

### Talkgroup Metadata

1. Local TOML/CSV config file (Unix way)
2. RadioReference.com CSV import
3. Auto-discovery from control channel grants (show numeric TGID, user labels in UI)

### API Design

```
WS  /api/v1/janus                          -- Janus WebSocket signaling (subscribe to audio mountpoints)
WS  /api/v1/systems/:system_id/events      -- TSBK metadata stream (call events, system status)
GET /api/v1/systems                         -- discovered systems
GET /api/v1/systems/:sid/talkgroups         -- known talkgroups + activity status
GET /api/v1/systems/:sid/talkgroups/:tgid   -- talkgroup detail + Janus mountpoint ID
```

Audio delivery via WebRTC (Janus). Metadata/signaling via WebSocket.

---

## Critical Risks & Open Questions

1. **IMBE vocoder complexity**: Building from spec is the hardest single component. IMBE involves pitch estimation, spectral envelope modeling, multi-band excitation synthesis. TIA-102.BABA spec + p25.rs `imbe.rs` crate are references. Expect 4-6 weeks of focused work.
2. **CPU budget**: Per-channel DDC at ~500M MACs/sec. 8-10 simultaneous voice channels = ~5 GFLOPS. One modern x86 core handles ~8-10 GFLOPS with SIMD. 4-core system is comfortable. Profile early.
3. **Encrypted traffic**: Many P25 systems use AES-256. Without keys, encrypted calls produce silence. Detect and indicate gracefully (ALGID in HDU/LDU2 crypto control).
4. **Wideband test recording needed**: The existing 852.350 MHz capture is CC-only (narrowband). Need a wideband recording of SRRCS that includes voice channels for integration testing.
5. **p25.rs code age**: The reference uses older Rust idioms (range patterns `2...7`, custom buffer types). Port requires modernization, not copy-paste.
6. **Multi-SDR sample drops**: 3-4 RTL-SDRs on one USB controller can cause scheduling jitter. Mitigate with dedicated reader threads, large USB buffers, and SCHED_FIFO priority.
7. **Janus deployment**: Adds an external dependency (C process). Needs packaging/deployment story. Docker? System package?

---

## Verification Plan

### Pre-Phase 0: Capture Test Data
- Record a fresh 2.4 MS/s CF32 sample centered on 852.350 MHz using the existing RTL-SDR
- Duration: 2-5 minutes (capture during peak radio activity for best chance of voice calls)
- Voice channels within ~1.1 MHz of CC (851.15 - 853.55 MHz) will be in the recording
- Run `p25 cc` on the capture first to identify which voice grants have frequencies in range
- This single recording serves as test data for Phase 0 and Phase 1

### Phase 0 (Voice Frame Extraction)
- Extract voice channels from the test recording via DDC (frequency shift + decimate)
- Decode with OP25 as reference, compare IMBE frame hex output
- Unit tests for Golay, Hamming, RS decoders with known test vectors from p25.rs

### Phase 1 (Single-Channel Voice)
- `p25 voice --input voice_channel.iq > audio.wav`
- Listen to WAV -- intelligible speech = pass
- Compare with OP25 decoded audio as reference

### Phase 2 (Wideband Trunking)
- Record 30 seconds of wideband IQ with known voice activity
- Verify all granted voice channels are captured and decoded
- Count decoded vs missed calls

### Phase 3+ (Web Streaming)
- `p25 serve --device driver=sdrplay` -> open browser -> hear live audio
- Multiple browser tabs subscribing to different TGs simultaneously
- Latency measurement: grant timestamp vs audio playback < 500ms

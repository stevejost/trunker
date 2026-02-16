# Simulcast / CQPSK Support TODO

## Motivation

User is physically located between 3 simulcast transmitters on SRRCS. Real-world
experience confirms the need: upgrading from BC395 (FM discriminator only) to SDS100
(simulcast-aware / CQPSK) dramatically improved decode quality.

Current pipeline on simulcast IQ recording (`samples/iq/srrcs_cc_852350000_2400k_cf32.iq`,
captured from same location): 479 TSBKs, 81.5% CRC pass rate. The missing 18.5% is
likely multipath-induced symbol timing errors and ISI.

## Background

- **CQPSK = LSM = pi/4 DQPSK** -- all the same modulation, different names
- C4FM and CQPSK are both members of the QPSK-c family (TIA-102.BAAA-B Section 8)
- C4FM = frequency modulation (constant envelope), CQPSK = I/Q amplitude modulation (non-constant envelope)
- At symbol decision instants, both produce the same 4-level deviation -- that's the "Compatible" part
- FM discriminator CAN decode CQPSK (per standard Section 8.6) but with degraded performance
- TIA-102.CAAB-C Table 3-5: C4FM delay spread tolerance = 50 us, Simulcast = 80 us (~38% of 208 us symbol period)
- **No existing open-source P25 tool has an adaptive equalizer** (not OP25, not p25.rs, not p25rx)

## Task Sequence

### Task 1: Adaptive Timing Recovery (Gardner or Mueller-Muller)

**Blocks:** Task 2

Replace fixed-phase + parabolic interpolation with an adaptive timing loop.
This improves C4FM decode AND is required infrastructure for the CQPSK path.

Current: Fixed-phase timing from sync lock + parabolic interpolation at decision point.
Target: Adaptive loop that tracks symbol timing continuously.

Reference implementation:
- OP25 `gardner_cc_impl.cc` (`~/source/op25/op25/gr-op25_repeater/lib/gardner_cc_impl.cc`)
- Uses MMSE FIR interpolator for sub-sample timing
- Parameters: `gain_mu=0.025`, `gain_omega=0.1 * gain_mu * gain_mu`
- Lock detection based on 480-symbol error accumulator

Expected improvement: Higher CRC pass rate on existing simulcast IQ recording.

### Task 2: CQPSK Coherent Demodulation Path

**Blocked by:** Task 1
**Blocks:** Tasks 3 and 4

Add a second demodulation path operating on complex I/Q samples directly
(not FM-discriminated output).

Components needed:
1. AGC (RMS-based, feedforward)
2. Costas loop for carrier/phase recovery (QPSK order=4)
   - OP25: `costas_loop_cc_impl.cc`, alpha=0.008, damping=sqrt(2)/2
3. Gardner timing recovery on complex samples (from Task 1)
4. Differential phase decoder (extract phase change between successive symbols)
5. Phase-to-symbol mapping (rescale to [-3,-1,+1,+3])
6. FSK4 slicer (existing, reuse)

OP25 CQPSK chain reference (`p25_demodulator.py` line ~503):
```
if_out -> cutoff -> agc -> fll -> clock -> diffdec -> costas -> to_float -> rescale -> slicer
```

Standards references:
- TIA-102.BAAA-B Section 8.5 (CQPSK modulator, Figure 37)
- TIA-102.BAAA-B Table 20 (dibit symbol mapping for both modulations)
- TIA-102.BAAA-B Table 21 (CQPSK I/Q lookup table, 8 phase states x 4 dibits)
- TIA-102.BAAA-B Section 8.3 (Nyquist Raised Cosine Filter spec)

### Task 3: Adaptive Equalizer for Simulcast Delay Spread

**Blocked by:** Task 2

Add an LMS decision-feedback equalizer (DFE) to handle multipath.

- 3-5 taps should cover 80 us delay spread requirement
- Train on known frame sync sequence (48 bits, known pattern)
- Adapt continuously using decided symbols
- Place after timing recovery, before slicer

This would be novel -- no open-source P25 tool has this. Start simple, validate
against the real simulcast IQ recording.

### Task 4: C4FM vs CQPSK Mode Selection

**Blocked by:** Task 2

Allow user to select demodulation mode:
- CLI flag: `p25 cc --modulation cqpsk` (or `c4fm`, default)
- Optional: auto-detect by measuring envelope variance (CQPSK has amplitude
  variation, C4FM has constant envelope)

Most users know whether their system is simulcast -- it's on RadioReference.

## Test Plan

Use the existing simulcast IQ recording as the benchmark at each stage:

| Stage | Expected TSBKs | Expected CRC Rate |
|-------|---------------|-------------------|
| Current (FM discriminator, fixed timing) | 479 | 81.5% |
| After Task 1 (adaptive timing) | ~500+ | ~85-90% |
| After Task 2 (CQPSK demod) | ~550+ | ~90-95% |
| After Task 3 (equalizer) | ~600+ | ~95%+ |

## Key Reference Files

- OP25 CQPSK demod: `~/source/op25/op25/gr-op25_repeater/apps/p25_demodulator.py`
- OP25 Gardner timing: `~/source/op25/op25/gr-op25_repeater/lib/gardner_cc_impl.cc`
- OP25 Costas loop: `~/source/op25/op25/gr-op25_repeater/lib/costas_loop_cc_impl.cc`
- OP25 CQPSK test script: `~/source/op25/op25/gr-op25_repeater/apps/util/cqpsk-demod-file.py`
- Standard (modulation): `docs/8.15_TIA-102.BAAA-B_Final Published Document_2017_06_22.pdf` Section 8
- Standard (performance): `docs/TIA-102.CAAB-C-2010.pdf` Section 3.1.6, Table 3-5

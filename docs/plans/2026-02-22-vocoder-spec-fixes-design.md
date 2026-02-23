# Vocoder TIA-102.BABA Spec Compliance Fixes

**Date:** 2026-02-22
**Branch:** `fix/voiced-smooth-interpolation`
**Spec:** TIA-102.BABA (December 2003)

## Background

A full audit of the IMBE vocoder implementation against TIA-102.BABA found four discrepancies. All tables, FEC codes, and other equations verified correct.

## Fixes (in implementation order)

### Fix 1: Frame repeat state update (MEDIUM) — `decode.rs`

**Spec:** Section 7.7 (p46), Eqs 99-104, 139
**Problem:** `repeat()` synthesizes audio using prev params but never updates `self.prev`. Phase base (Eq 139) must accumulate every frame.
**Fix:** Change `repeat(&self)` → `repeat(&mut self)`. After synthesis, update:
- `self.prev.phase_base`
- `self.prev.phase`
- `self.prev.unvoiced`

### Fix 2: Frame muting comfort noise + state update (LOW) — `decode.rs`

**Spec:** Section 7.8 (p47)
**Problem:** `silence()` outputs zeros and doesn't update state. Spec says: perform repeat-style state update, then output uniform random noise on [-5, 5].
**Fix:** Change `silence(&self)` → `silence(&mut self)`. Call `self.repeat(buffer)` for state update, then overwrite buffer with noise.

### Fix 3: Smooth voiced interpolation Eqs 134-138 (HIGH) — `voiced.rs`

**Spec:** Section 11.3 (p60-61), Eqs 134-138
**Problem:** `get_pair()` always uses Eq 133 for voiced-voiced case. Missing smooth interpolation for l < 8 with stable pitch.
**Condition:** Both current and previous harmonic l voiced, AND `l < 8`, AND `|w0(0) - w0(-1)| < 0.1 * w0(0)`.
**Fix:** Add fields to `Voiced` struct:
- `smooth_eligible: bool`
- `delta_omega: f32`
- `smooth_delta_phi: [f32; 7]`

Add `signal_smooth(l, n)` method implementing:
- Eq 134: `s_v,l(n) = a_l(n) * cos(theta_l(n))`
- Eq 135: `a_l(n) = M_l(-1) + (n/N) * [M_l(0) - M_l(-1)]`
- Eq 136: `theta_l(n) = phi_l(-1) + [w0(-1)*l + delta_omega_l]*n + [w0(0)-w0(-1)]*l*n^2/(2N)`
- Eq 137: `delta_phi_l = phi_l(0) - phi_l(-1) - [w0(-1) + w0(0)]*l*N/2`
- Eq 138: `delta_omega_l = (1/N) * [delta_phi_l - 2*pi*floor((delta_phi_l + pi)/(2*pi))]`

Phase wrapping uses spec's explicit floor formula, NOT `rem_euclid`.

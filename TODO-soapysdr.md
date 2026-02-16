# TODO: Live SDR Support via SoapySDR

## Task 1: Define `IqSource` trait
**File:** `src/sdr/mod.rs`
- Add trait: `pub trait IqSource: Iterator<Item = Complex<f32>> { fn sample_rate(&self) -> u32; }`
- Implement `IqSource` for `Cf32Reader` (already has both methods)

## Task 2: Update pipeline to use trait
**File:** `src/main.rs`
- Change `decode_control_channel` to accept `impl IqSource` instead of hardcoded `Cf32Reader`
- Remove the `sample_rate` parameter (get it from `source.sample_rate()`)

## Task 3: Add SoapySDR source (behind `soapy` feature flag)
**New file:** `src/sdr/soapy_source.rs`
- `SoapySource` struct: opens device, configures freq/rate/gain, sets up RX stream
- Request CF32 format from SoapySDR (handles u8->f32 conversion internally)
- Internal buffer + cursor pattern (like `Cf32Reader`)
- Implements `Iterator<Item = Complex<f32>>` and `IqSource`

**Cargo.toml:**
```toml
[features]
default = []
soapy = ["dep:soapysdr"]

[dependencies]
soapysdr = { version = "0.4", optional = true }
```

**`src/sdr/mod.rs`:** conditionally expose module with `#[cfg(feature = "soapy")]`

## Task 4: Extend `SdrError`
**File:** `src/sdr/error.rs`
- Add `#[cfg(feature = "soapy")]` variants: `DeviceNotFound { driver: String }`, `StreamFailed { source: String }`

## Task 5: Extend CLI
**File:** `src/main.rs`
- `--frequency` (required for live mode, mutually exclusive with `--input`)
- `--gain` (optional, default AGC)
- `--device` (optional, default first available)
- When `--frequency` provided: use `SoapySource`. When `--input` provided: use `Cf32Reader`.

## Verification
1. `cargo test` passes with and without `--features soapy`
2. `cargo clippy --all-features -- -D warnings` clean
3. File mode unchanged: `p25 cc --input file.iq` still works
4. Live mode: `p25 cc --frequency 852350000` decodes with RTL-SDR

## System dependencies
- `libsoapysdr-dev` (apt) or `soapysdr` (brew)
- `soapy-sdr-module-rtlsdr` (apt) for RTL-SDR support

//! Integration tests for SoapySDR hardware source.
//!
//! These tests require a real SoapySDR device (e.g. RTL-SDR) and are
//! skipped in normal `cargo test` runs. Run with:
//!
//!   cargo test -- --ignored
//!
//! Ensure the device is plugged in and SoapySDR drivers are installed.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use trunker::sdr::soapy_source::SoapySource;

/// Open an RTL-SDR device, read a few thousand samples, and verify
/// that at least some are non-zero (i.e. the device is producing data).
#[test]
#[ignore]
fn rtlsdr_reads_nonzero_samples() {
    let running = Arc::new(AtomicBool::new(true));
    let source = SoapySource::open("driver=rtlsdr", 852_350_000, 2_400_000, Some(40.0), None, &[], running)
        .expect("failed to open RTL-SDR device");

    let samples: Vec<_> = source.take(8192).collect();

    assert_eq!(samples.len(), 8192, "should read 8192 samples");

    let nonzero = samples.iter().any(|s| s.re != 0.0 || s.im != 0.0);
    assert!(nonzero, "at least some samples should be non-zero");
}

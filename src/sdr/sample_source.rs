//! Unified IQ sample source abstraction over files and SDR hardware.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use num_complex::Complex;

use super::cf32_reader::Cf32Reader;
use super::error::SdrError;
use super::soapy_source::SoapySource;
use super::u8_reader::U8Reader;

/// IQ sample source: file or live SDR hardware.
pub enum SampleSource {
    /// Read from a CF32 IQ file.
    Cf32(Cf32Reader),
    /// Read from a U8 IQ file (RTL-SDR native / .cu8).
    U8(U8Reader),
    /// Stream from a SoapySDR device.
    Soapy(SoapySource),
}

impl SampleSource {
    /// Returns shared SDR reader stats handles, or `None` for file-based sources.
    ///
    /// Call this before the source is consumed by a `for` loop so that stats
    /// remain accessible during iteration.
    pub fn sdr_stats_handles(&self) -> Option<SdrStatsHandles> {
        match self {
            SampleSource::Soapy(source) => Some(SdrStatsHandles {
                chunk_count: source.chunk_count_handle(),
                overflow_count: source.overflow_count_handle(),
            }),
            _ => None,
        }
    }

    /// Return the device error if the stream ended due to a hardware failure.
    ///
    /// Returns `None` for file-based sources or when the SDR stream ended
    /// normally. Check this after iteration completes to distinguish
    /// graceful shutdown from device errors.
    pub fn device_error(&self) -> Option<&SdrError> {
        match self {
            SampleSource::Soapy(source) => source.device_error(),
            _ => None,
        }
    }
}

/// Shared atomic counters from the SDR reader thread.
pub struct SdrStatsHandles {
    /// Number of chunks read from the SDR device.
    pub chunk_count: Arc<AtomicU64>,
    /// Number of overflow events detected.
    pub overflow_count: Arc<AtomicU64>,
}

impl SdrStatsHandles {
    /// Read the current chunk and overflow counts.
    pub fn snapshot(&self) -> (u64, u64) {
        (
            self.chunk_count.load(Ordering::Relaxed),
            self.overflow_count.load(Ordering::Relaxed),
        )
    }
}

impl Iterator for SampleSource {
    type Item = Complex<f32>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            SampleSource::Cf32(reader) => reader.next(),
            SampleSource::U8(reader) => reader.next(),
            SampleSource::Soapy(source) => source.next(),
        }
    }
}

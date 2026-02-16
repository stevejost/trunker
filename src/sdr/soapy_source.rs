//! SoapySDR hardware device source for live IQ sample streaming.
//!
//! Wraps a SoapySDR `Device` and `RxStream` to provide an `Iterator`
//! over `Complex<f32>` samples from real SDR hardware.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use num_complex::Complex;
use soapysdr::{Device, Direction, ErrorCode, RxStream};

use super::error::SdrError;

/// Receive channel index (single-channel devices).
const RX_CHANNEL: usize = 0;

/// Stream read timeout in microseconds (1 second).
const READ_TIMEOUT_US: i64 = 1_000_000;

/// A live IQ sample source backed by a SoapySDR device.
///
/// Opens an SDR device, configures frequency/rate/gain, and yields
/// `Complex<f32>` samples via the `Iterator` trait. The stream stops
/// when `running` is set to `false` (e.g. from a Ctrl-C handler).
pub struct SoapySource {
    stream: RxStream<Complex<f32>>,
    buffer: Vec<Complex<f32>>,
    position: usize,
    valid_count: usize,
    running: Arc<AtomicBool>,
}

impl std::fmt::Debug for SoapySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SoapySource")
            .field("position", &self.position)
            .field("valid_count", &self.valid_count)
            .field("buffer_len", &self.buffer.len())
            .finish()
    }
}

impl SoapySource {
    /// Open a SoapySDR device and configure it for receiving.
    ///
    /// # Arguments
    /// * `device_args` -- device filter string (e.g. `"driver=rtlsdr"`)
    /// * `frequency_hz` -- center frequency in hertz
    /// * `sample_rate_hz` -- sample rate in hertz
    /// * `gain` -- manual gain in dB, or `None` for automatic gain control
    /// * `running` -- shared flag checked before each read; set to `false` to stop
    pub fn open(
        device_args: &str,
        frequency_hz: u64,
        sample_rate_hz: u32,
        gain: Option<f64>,
        running: Arc<AtomicBool>,
    ) -> Result<Self, SdrError> {
        let device = open_device(device_args)?;
        configure_frequency(&device, frequency_hz)?;
        configure_sample_rate(&device, sample_rate_hz)?;
        configure_gain(&device, gain)?;
        let mut stream = create_stream(&device)?;
        let mtu = stream
            .mtu()
            .map_err(|e| SdrError::StreamCreate(e.message))?;

        stream
            .activate(None)
            .map_err(|e| SdrError::StreamActivate(e.message))?;

        tracing::info!(
            device_args,
            frequency_hz,
            sample_rate_hz,
            ?gain,
            mtu,
            "SoapySDR device opened"
        );

        Ok(Self {
            stream,
            buffer: vec![Complex::new(0.0, 0.0); mtu],
            position: 0,
            valid_count: 0,
            running,
        })
    }

    /// Fill the internal buffer with the next chunk of samples from the device.
    ///
    /// Returns the number of samples read, or `None` if the stream should stop.
    fn fill_buffer(&mut self) -> Option<usize> {
        if !self.running.load(Ordering::Relaxed) {
            return None;
        }

        loop {
            match self.stream.read(&mut [&mut self.buffer], READ_TIMEOUT_US) {
                Ok(count) => {
                    self.position = 0;
                    self.valid_count = count;
                    return Some(count);
                }
                Err(e) if e.code == ErrorCode::Timeout => {
                    if !self.running.load(Ordering::Relaxed) {
                        return None;
                    }
                    continue;
                }
                Err(e) if e.code == ErrorCode::Overflow => {
                    tracing::warn!("SoapySDR overflow (samples lost)");
                    if !self.running.load(Ordering::Relaxed) {
                        return None;
                    }
                    continue;
                }
                Err(e) => {
                    tracing::warn!(error = %e.message, "SoapySDR stream read error");
                    return None;
                }
            }
        }
    }
}

impl Iterator for SoapySource {
    type Item = Complex<f32>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.position >= self.valid_count {
            self.fill_buffer()?;
        }
        let sample = self.buffer[self.position];
        self.position += 1;
        Some(sample)
    }
}

impl Drop for SoapySource {
    fn drop(&mut self) {
        if let Err(e) = self.stream.deactivate(None) {
            tracing::warn!(error = %e.message, "failed to deactivate SoapySDR stream");
        }
    }
}

/// Open a SoapySDR device with the given argument string.
fn open_device(device_args: &str) -> Result<Device, SdrError> {
    Device::new(device_args).map_err(|e| SdrError::DeviceOpen {
        args: device_args.to_string(),
        detail: e.message,
    })
}

/// Set the center frequency on the device's RX channel.
fn configure_frequency(device: &Device, frequency_hz: u64) -> Result<(), SdrError> {
    device
        .set_frequency(Direction::Rx, RX_CHANNEL, frequency_hz as f64, ())
        .map_err(|e| SdrError::Configure {
            parameter: "center_frequency".to_string(),
            detail: e.message,
        })
}

/// Set the sample rate on the device's RX channel.
///
/// Warns if the actual rate differs from the requested rate.
fn configure_sample_rate(device: &Device, sample_rate_hz: u32) -> Result<(), SdrError> {
    device
        .set_sample_rate(Direction::Rx, RX_CHANNEL, sample_rate_hz as f64)
        .map_err(|e| SdrError::Configure {
            parameter: "sample_rate".to_string(),
            detail: e.message,
        })?;

    let actual =
        device
            .sample_rate(Direction::Rx, RX_CHANNEL)
            .map_err(|e| SdrError::Configure {
                parameter: "sample_rate".to_string(),
                detail: e.message,
            })?;

    if (actual - sample_rate_hz as f64).abs() > 1.0 {
        tracing::warn!(
            requested = sample_rate_hz,
            actual = actual,
            "sample rate differs from requested value"
        );
    }
    Ok(())
}

/// Configure gain: manual (specific dB) or AGC.
fn configure_gain(device: &Device, gain: Option<f64>) -> Result<(), SdrError> {
    match gain {
        Some(gain_db) => {
            device
                .set_gain_mode(Direction::Rx, RX_CHANNEL, false)
                .map_err(|e| SdrError::Configure {
                    parameter: "gain_mode".to_string(),
                    detail: e.message,
                })?;
            device
                .set_gain(Direction::Rx, RX_CHANNEL, gain_db)
                .map_err(|e| SdrError::Configure {
                    parameter: "gain".to_string(),
                    detail: e.message,
                })?;
        }
        None => {
            device
                .set_gain_mode(Direction::Rx, RX_CHANNEL, true)
                .map_err(|e| SdrError::Configure {
                    parameter: "gain_mode".to_string(),
                    detail: e.message,
                })?;
        }
    }
    Ok(())
}

/// Create an RX stream for Complex<f32> samples on channel 0.
fn create_stream(device: &Device) -> Result<RxStream<Complex<f32>>, SdrError> {
    device
        .rx_stream::<Complex<f32>>(&[RX_CHANNEL])
        .map_err(|e| SdrError::StreamCreate(e.message))
}

/// List all SoapySDR devices visible on the system.
///
/// Prints device properties to stdout. If no devices are found,
/// prints a troubleshooting message to stderr.
pub fn list_devices() {
    match soapysdr::enumerate("") {
        Ok(devices) if devices.is_empty() => {
            eprintln!("No SoapySDR devices found.");
            eprintln!();
            eprintln!("Troubleshooting:");
            eprintln!("  - Is the SDR device plugged in?");
            eprintln!("  - Is the SoapySDR module installed? (e.g. soapysdr-module-rtlsdr)");
            eprintln!("  - Try: SoapySDRUtil --find");
        }
        Ok(devices) => {
            println!("Found {} SoapySDR device(s):", devices.len());
            for (index, device) in devices.iter().enumerate() {
                println!("  [{index}] {device}");
            }
        }
        Err(e) => {
            eprintln!("Error enumerating SoapySDR devices: {}", e.message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_nonexistent_device_returns_error() {
        let running = Arc::new(AtomicBool::new(true));
        let result = SoapySource::open(
            "driver=nonexistent_xyz_12345",
            852_350_000,
            2_400_000,
            None,
            running,
        );
        match result {
            Err(err) => {
                let msg = format!("{err}");
                assert!(msg.contains("nonexistent_xyz_12345"));
            }
            Ok(_) => panic!("expected error for nonexistent device"),
        }
    }
}

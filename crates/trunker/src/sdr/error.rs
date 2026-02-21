//! Error types for SDR source operations.

use std::path::PathBuf;

/// Errors that can occur when reading IQ sample data.
#[derive(Debug, thiserror::Error)]
pub enum SdrError {
    /// Failed to open an IQ sample file.
    #[error("failed to open IQ file {path}: {source}")]
    OpenFile {
        /// Path to the file that could not be opened.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// Failed to read from an IQ sample file.
    #[error("read error on IQ file: {0}")]
    Read(#[from] std::io::Error),

    /// Failed to open a SoapySDR device.
    #[error("failed to open SoapySDR device with args '{args}': {detail}")]
    DeviceOpen {
        /// Device arguments string (e.g. "driver=rtlsdr").
        args: String,
        /// Description of the failure.
        detail: String,
    },

    /// Failed to configure a SoapySDR device parameter.
    #[error("failed to configure SoapySDR {parameter}: {detail}")]
    Configure {
        /// Name of the parameter that failed (e.g. "center_frequency").
        parameter: String,
        /// Description of the failure.
        detail: String,
    },

    /// Failed to create an RX stream on a SoapySDR device.
    #[error("failed to create SoapySDR RX stream: {0}")]
    StreamCreate(String),

    /// Failed to activate an RX stream on a SoapySDR device.
    #[error("failed to activate SoapySDR RX stream: {0}")]
    StreamActivate(String),

    /// Error reading samples from a SoapySDR RX stream.
    #[error("SoapySDR stream read error: {0}")]
    StreamRead(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_file_error_display() {
        let err = SdrError::OpenFile {
            path: PathBuf::from("/tmp/test.iq"),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "not found"),
        };
        let msg = format!("{err}");
        assert!(msg.contains("/tmp/test.iq"));
        assert!(msg.contains("not found"));
    }

    #[test]
    fn device_open_error_display() {
        let err = SdrError::DeviceOpen {
            args: "driver=rtlsdr".to_string(),
            detail: "no matching device".to_string(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("driver=rtlsdr"));
        assert!(msg.contains("no matching device"));
    }

    #[test]
    fn configure_error_display() {
        let err = SdrError::Configure {
            parameter: "center_frequency".to_string(),
            detail: "value out of range".to_string(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("center_frequency"));
        assert!(msg.contains("value out of range"));
    }

    #[test]
    fn stream_create_error_display() {
        let err = SdrError::StreamCreate("format not supported".to_string());
        let msg = format!("{err}");
        assert!(msg.contains("create"));
        assert!(msg.contains("format not supported"));
    }

    #[test]
    fn stream_activate_error_display() {
        let err = SdrError::StreamActivate("device busy".to_string());
        let msg = format!("{err}");
        assert!(msg.contains("activate"));
        assert!(msg.contains("device busy"));
    }

    #[test]
    fn stream_read_error_display() {
        let err = SdrError::StreamRead("timeout".to_string());
        let msg = format!("{err}");
        assert!(msg.contains("read error"));
        assert!(msg.contains("timeout"));
    }
}

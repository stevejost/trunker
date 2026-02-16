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
}

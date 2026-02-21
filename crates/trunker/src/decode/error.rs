//! Error types for the decode event layer.

use thiserror::Error;

/// Errors that can occur in the decode event layer.
#[derive(Debug, Error)]
pub enum DecodeError {
    /// An I/O error occurred while writing output.
    #[error("output write error: {0}")]
    OutputWrite(#[from] std::io::Error),
}

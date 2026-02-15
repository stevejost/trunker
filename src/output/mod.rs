//! Output serialization layer.
//!
//! Converts decoded P25 messages into output formats (JSON lines, etc.).
//! This layer knows nothing about how messages were decoded.

pub mod json;

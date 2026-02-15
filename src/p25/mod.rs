//! P25 protocol decoder layer.
//!
//! This module handles all P25 protocol-specific logic: TSBK parsing,
//! opcode handling, and channel identifier resolution. It operates on
//! decoded byte arrays and knows nothing about RF or SDR hardware.

pub mod channel;
pub mod crc;
pub mod tsbk;
pub mod types;

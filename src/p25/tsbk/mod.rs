//! Trunking Signaling Block (TSBK) parsing.
//!
//! TSBKs are 96-bit messages broadcast on P25 control channels.
//! This module handles opcode identification, field extraction,
//! and CRC validation.
//!
//! Reference: TIA-102.AABF-A Section 7

pub mod fields;
pub mod opcode;
pub mod parser;

pub use fields::TsbkFields;
pub use opcode::TsbkOpcode;
pub use parser::{TsbkError, TsbkMessage, parse_tsbk};

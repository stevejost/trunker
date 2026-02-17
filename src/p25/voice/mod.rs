//! Voice channel data types for P25 data units.
//!
//! Contains decoders for:
//! - Link Control (from LDU1 and TDULC)
//! - Crypto Control (from LDU2)
//! - Voice Header (from HDU)

pub mod control;
pub mod crypto;
pub mod descramble;
pub mod frame;
pub mod frame_group;
pub mod header;
pub mod pn;
pub mod terminator;

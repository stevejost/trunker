//! Voice channel data types for P25 LDU1/LDU2 frames.
//!
//! Contains Link Control (from LDU1) and Crypto Control (from LDU2)
//! payload decoders.

pub mod control;
pub mod crypto;
pub mod descramble;
pub mod frame;
pub mod pn;

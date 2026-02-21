//! Forward error correction coding for P25 voice and data channel decoding.
//!
//! This module contains the error correction codes used by the P25 air
//! interface for voice frames, link control, and header data blocks.

pub mod bmcf;
pub mod cyclic;
pub mod galois;
pub mod golay;
pub mod hamming;
pub mod reed_solomon;

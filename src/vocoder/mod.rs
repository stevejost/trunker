//! IMBE vocoder for P25 voice decoding.
//!
//! Decodes IMBE voice frames (88-bit chunks from the P25 FEC layer) into
//! 8 kHz PCM audio samples. The pipeline:
//!
//! 1. Extract voice parameters from the bit vector chunks
//! 2. Perform spectral amplitude enhancement
//! 3. Synthesize audio via overlap-add harmonic synthesis

// TODO: remove #[allow(dead_code)] once vocoder has internal consumers
#[allow(dead_code)]
pub(crate) mod error;
#[allow(dead_code)]
pub(crate) mod consts;
#[allow(dead_code)]
pub(crate) mod frame;
#[allow(dead_code)]
pub(crate) mod params;
#[allow(dead_code)]
pub(crate) mod allocs;
#[allow(dead_code)]
pub(crate) mod scan;
#[allow(dead_code)]
pub(crate) mod descramble;
#[allow(dead_code)]
pub(crate) mod gain;
#[allow(dead_code)]
pub(crate) mod coefs;
#[allow(dead_code)]
pub(crate) mod spectral;
#[allow(dead_code)]
pub(crate) mod prev;
#[allow(dead_code)]
pub(crate) mod enhance;
#[allow(dead_code)]
pub(crate) mod window;

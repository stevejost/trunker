//! IMBE vocoder for P25 voice decoding.
//!
//! Decodes IMBE voice frames (88-bit chunks from the P25 FEC layer) into
//! 8 kHz PCM audio samples. The pipeline:
//!
//! 1. Extract voice parameters from the bit vector chunks
//! 2. Perform spectral amplitude enhancement
//! 3. Synthesize audio via overlap-add harmonic synthesis

pub(crate) mod allocs;
pub(crate) mod coefs;
pub(crate) mod consts;
pub(crate) mod decode;
pub(crate) mod descramble;
pub(crate) mod enhance;
pub(crate) mod error;
pub(crate) mod frame;
pub(crate) mod gain;
pub(crate) mod params;
pub(crate) mod prev;
pub(crate) mod scan;
pub(crate) mod spectral;
pub(crate) mod unvoiced;
pub(crate) mod voiced;
pub(crate) mod window;

pub use consts::SAMPLES_PER_FRAME;
pub use decode::ImbeDecoder;
pub use frame::{AudioBuffer, ReceivedFrame};

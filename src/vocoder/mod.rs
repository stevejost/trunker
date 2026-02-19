//! IMBE vocoder for P25 voice decoding.
//!
//! Decodes IMBE voice frames (88-bit chunks from the P25 FEC layer) into
//! 8 kHz PCM audio samples. The pipeline:
//!
//! 1. Extract voice parameters from the bit vector chunks
//! 2. Perform spectral amplitude enhancement
//! 3. Synthesize audio via overlap-add harmonic synthesis

// TODO: remove once vocoder has internal consumers (issue #41+)
#[allow(dead_code)]
pub(crate) mod consts;
#[allow(dead_code)]
pub(crate) mod frame;

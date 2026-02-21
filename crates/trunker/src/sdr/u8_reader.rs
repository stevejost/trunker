//! Reader for unsigned 8-bit IQ sample files (RTL-SDR native format).
//!
//! U8 format stores interleaved unsigned 8-bit pairs (I, Q, I, Q, ...)
//! at 2 bytes per complex sample. Each byte is an unsigned value in
//! [0, 255] representing one component of the IQ pair:
//!
//! - `0` maps to `-1.0`
//! - `128` maps to approximately zero (true zero is not representable)
//! - `255` maps to `+1.0`
//!
//! Conversion formula: `(byte - 127.5) / 127.5`
//!
//! This is the native output format of RTL-SDR hardware and tools like
//! `rtl_sdr` and `rx_sdr`. For comparison, CF32 format uses 8 bytes
//! per complex sample (interleaved f32 pairs in native byte order).

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use num_complex::Complex;

use super::error::SdrError;

/// Number of IQ samples to buffer per read operation.
const BUFFER_SAMPLES: usize = 8192;

/// Number of bytes per complex sample (1 byte I + 1 byte Q).
const BYTES_PER_SAMPLE: usize = 2;

/// Convert an unsigned 8-bit sample to a float in the range [-1.0, +1.0].
///
/// Uses the standard RTL-SDR conversion: `(byte - 127.5) / 127.5`.
fn u8_to_f32(byte: u8) -> f32 {
    (byte as f32 - 127.5) / 127.5
}

/// A buffered reader for unsigned 8-bit IQ sample files.
///
/// Reads interleaved unsigned 8-bit I/Q pairs and yields `Complex<f32>`
/// samples via the `Iterator` trait. Each byte is converted from the
/// [0, 255] range to [-1.0, +1.0].
#[derive(Debug)]
pub struct U8Reader {
    reader: BufReader<File>,
    /// Sample rate of the IQ recording in hertz.
    sample_rate: u32,
    buffer: Vec<Complex<f32>>,
    position: usize,
}

impl U8Reader {
    /// Open a U8 IQ file at the given path.
    ///
    /// `sample_rate` is the recording's sample rate in hertz, stored as
    /// metadata for downstream DSP stages.
    pub fn open(path: &Path, sample_rate: u32) -> Result<Self, SdrError> {
        let file = File::open(path).map_err(|source| SdrError::OpenFile {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(Self {
            reader: BufReader::new(file),
            sample_rate,
            buffer: Vec::new(),
            position: 0,
        })
    }

    /// Return the sample rate in hertz.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Fill the internal buffer with the next chunk of samples.
    ///
    /// Returns the number of complete samples read, or 0 at EOF.
    fn fill_buffer(&mut self) -> Result<usize, SdrError> {
        let mut raw = vec![0u8; BUFFER_SAMPLES * BYTES_PER_SAMPLE];
        let mut total_read = 0;

        while total_read < raw.len() {
            match self.reader.read(&mut raw[total_read..]) {
                Ok(0) => break,
                Ok(n) => total_read += n,
                Err(e) => return Err(SdrError::Read(e)),
            }
        }

        let complete_samples = total_read / BYTES_PER_SAMPLE;
        self.buffer.clear();
        self.buffer.reserve(complete_samples);

        for i in 0..complete_samples {
            let offset = i * BYTES_PER_SAMPLE;
            let real = u8_to_f32(raw[offset]);
            let imag = u8_to_f32(raw[offset + 1]);
            self.buffer.push(Complex::new(real, imag));
        }

        self.position = 0;
        Ok(complete_samples)
    }
}

impl Iterator for U8Reader {
    type Item = Complex<f32>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.position >= self.buffer.len() {
            match self.fill_buffer() {
                Ok(0) => return None,
                Ok(_) => {}
                Err(e) => {
                    tracing::error!("IQ read error: {e}");
                    return None;
                }
            }
        }
        let sample = self.buffer[self.position];
        self.position += 1;
        Some(sample)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// Write raw U8 IQ byte pairs to a temp file.
    fn write_test_u8(byte_pairs: &[[u8; 2]]) -> NamedTempFile {
        let mut file = NamedTempFile::new().expect("create temp file");
        for pair in byte_pairs {
            file.write_all(pair).expect("write byte pair");
        }
        file.flush().expect("flush");
        file
    }

    #[test]
    fn read_known_samples() {
        // Boundary values: 0 -> -1.0, 128 -> 0.003937..., 255 -> +1.0
        let file = write_test_u8(&[
            [0, 0],     // I=-1.0, Q=-1.0
            [128, 128], // I=0.003937, Q=0.003937
            [255, 255], // I=+1.0, Q=+1.0
        ]);

        let reader = U8Reader::open(file.path(), 2_400_000).unwrap();
        let samples: Vec<_> = reader.collect();

        assert_eq!(samples.len(), 3);

        // 0 -> (0 - 127.5) / 127.5 = -1.0
        assert_eq!(samples[0].re, -1.0);
        assert_eq!(samples[0].im, -1.0);

        // 128 -> (128 - 127.5) / 127.5 = 0.5 / 127.5 = 0.003921569...
        let expected_128 = (128.0_f32 - 127.5) / 127.5;
        assert_eq!(samples[1].re, expected_128);
        assert_eq!(samples[1].im, expected_128);

        // 255 -> (255 - 127.5) / 127.5 = 1.0
        assert_eq!(samples[2].re, 1.0);
        assert_eq!(samples[2].im, 1.0);
    }

    #[test]
    fn empty_file_yields_no_samples() {
        let file = write_test_u8(&[]);
        let reader = U8Reader::open(file.path(), 48_000).unwrap();
        let samples: Vec<_> = reader.collect();
        assert!(samples.is_empty());
    }

    #[test]
    fn partial_sample_at_eof_is_discarded() {
        let mut file = NamedTempFile::new().expect("create temp file");
        // Write one complete sample (2 bytes) plus one trailing byte.
        file.write_all(&[100, 200, 0xAA]).unwrap();
        file.flush().unwrap();

        let reader = U8Reader::open(file.path(), 48_000).unwrap();
        let samples: Vec<_> = reader.collect();
        assert_eq!(samples.len(), 1);

        let expected_re = u8_to_f32(100);
        let expected_im = u8_to_f32(200);
        assert_eq!(samples[0].re, expected_re);
        assert_eq!(samples[0].im, expected_im);
    }

    #[test]
    fn sample_rate_is_stored() {
        let file = write_test_u8(&[]);
        let reader = U8Reader::open(file.path(), 2_400_000).unwrap();
        assert_eq!(reader.sample_rate(), 2_400_000);
    }

    #[test]
    fn open_nonexistent_file_returns_error() {
        let result = U8Reader::open(Path::new("/nonexistent/file.u8"), 48_000);
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("/nonexistent/file.u8"));
    }

    #[test]
    fn iq_interleaving_is_correct() {
        // First byte is I, second byte is Q.
        let file = write_test_u8(&[
            [64, 192], // I = (64-127.5)/127.5, Q = (192-127.5)/127.5
            [0, 255],  // I = -1.0, Q = +1.0
        ]);

        let reader = U8Reader::open(file.path(), 48_000).unwrap();
        let samples: Vec<_> = reader.collect();

        assert_eq!(samples.len(), 2);

        assert_eq!(samples[0].re, u8_to_f32(64));
        assert_eq!(samples[0].im, u8_to_f32(192));
        assert_eq!(samples[1].re, -1.0);
        assert_eq!(samples[1].im, 1.0);
    }
}

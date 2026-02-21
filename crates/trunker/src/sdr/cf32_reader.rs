//! Reader for CF32 (complex float32) IQ sample files.
//!
//! CF32 format stores interleaved f32 pairs (I, Q, I, Q, ...) in
//! native byte order at 8 bytes per complex sample (4 bytes per
//! component). Values are IEEE 754 single-precision floats, typically
//! in the range [-1.0, +1.0]. This is the internal working format
//! used throughout the DSP pipeline.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use num_complex::Complex;

use super::error::SdrError;

/// Number of IQ samples to buffer per read operation.
const BUFFER_SAMPLES: usize = 8192;

/// Number of bytes per complex sample (2 x f32).
const BYTES_PER_SAMPLE: usize = 8;

/// A buffered reader for CF32 IQ sample files.
///
/// Reads interleaved f32 I/Q pairs and yields `Complex<f32>` samples
/// via the `Iterator` trait.
#[derive(Debug)]
pub struct Cf32Reader {
    reader: BufReader<File>,
    /// Sample rate of the IQ recording in hertz.
    sample_rate: u32,
    buffer: Vec<Complex<f32>>,
    position: usize,
}

impl Cf32Reader {
    /// Open a CF32 file at the given path.
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
            let real = f32::from_ne_bytes([
                raw[offset],
                raw[offset + 1],
                raw[offset + 2],
                raw[offset + 3],
            ]);
            let imag = f32::from_ne_bytes([
                raw[offset + 4],
                raw[offset + 5],
                raw[offset + 6],
                raw[offset + 7],
            ]);
            self.buffer.push(Complex::new(real, imag));
        }

        self.position = 0;
        Ok(complete_samples)
    }
}

impl Iterator for Cf32Reader {
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

    fn write_test_cf32(samples: &[Complex<f32>]) -> NamedTempFile {
        let mut file = NamedTempFile::new().expect("create temp file");
        for s in samples {
            file.write_all(&s.re.to_ne_bytes())
                .expect("write real part");
            file.write_all(&s.im.to_ne_bytes())
                .expect("write imag part");
        }
        file.flush().expect("flush");
        file
    }

    #[test]
    fn read_known_samples() {
        let expected = vec![
            Complex::new(1.0, 2.0),
            Complex::new(3.0, 4.0),
            Complex::new(-1.0, -0.5),
        ];
        let file = write_test_cf32(&expected);

        let reader = Cf32Reader::open(file.path(), 2_400_000).unwrap();
        let samples: Vec<_> = reader.collect();

        assert_eq!(samples.len(), 3);
        for (got, want) in samples.iter().zip(expected.iter()) {
            assert_eq!(got.re, want.re);
            assert_eq!(got.im, want.im);
        }
    }

    #[test]
    fn empty_file_yields_no_samples() {
        let file = write_test_cf32(&[]);
        let reader = Cf32Reader::open(file.path(), 48_000).unwrap();
        let samples: Vec<_> = reader.collect();
        assert!(samples.is_empty());
    }

    #[test]
    fn partial_sample_at_eof_is_discarded() {
        let mut file = NamedTempFile::new().expect("create temp file");
        let s = Complex::new(1.0f32, 2.0f32);
        file.write_all(&s.re.to_ne_bytes()).unwrap();
        file.write_all(&s.im.to_ne_bytes()).unwrap();
        file.write_all(&[0xAA, 0xBB, 0xCC]).unwrap();
        file.flush().unwrap();

        let reader = Cf32Reader::open(file.path(), 48_000).unwrap();
        let samples: Vec<_> = reader.collect();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0], Complex::new(1.0, 2.0));
    }

    #[test]
    fn sample_rate_is_stored() {
        let file = write_test_cf32(&[]);
        let reader = Cf32Reader::open(file.path(), 2_400_000).unwrap();
        assert_eq!(reader.sample_rate(), 2_400_000);
    }

    #[test]
    fn open_nonexistent_file_returns_error() {
        let result = Cf32Reader::open(Path::new("/nonexistent/file.iq"), 48_000);
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("/nonexistent/file.iq"));
    }

    #[test]
    fn iq_interleaving_is_correct() {
        let samples = vec![Complex::new(0.25, 0.75), Complex::new(-0.5, 0.0)];
        let file = write_test_cf32(&samples);

        let reader = Cf32Reader::open(file.path(), 48_000).unwrap();
        let read_back: Vec<_> = reader.collect();

        assert_eq!(read_back[0].re, 0.25);
        assert_eq!(read_back[0].im, 0.75);
        assert_eq!(read_back[1].re, -0.5);
        assert_eq!(read_back[1].im, 0.0);
    }
}

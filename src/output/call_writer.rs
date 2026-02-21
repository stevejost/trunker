//! Audio format selection and unified writer for [`CallRecorder`].
//!
//! Provides [`AudioFormat`] for choosing between WAV and Opus output,
//! and [`AudioWriter`] which dispatches to the correct encoder. Uses
//! enum dispatch since there are only two formats with no extensibility
//! requirement.

use std::path::Path;

use super::opus_writer::OpusWriter;
use super::wav::WavWriter;

/// Audio file format selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFormat {
    /// WAV: 8 kHz, 16-bit signed PCM, mono.
    Wav,
    /// Opus: 8 kHz, mono, VOIP mode, 16 kbps, OGG container.
    Opus,
}

impl AudioFormat {
    /// File extension for this format (without leading dot).
    pub fn extension(&self) -> &'static str {
        match self {
            AudioFormat::Wav => "wav",
            AudioFormat::Opus => "opus",
        }
    }
}

/// Unified audio writer that dispatches to WAV or Opus encoders.
///
/// Wraps either a [`WavWriter`] or an [`OpusWriter`] and delegates
/// `write_samples` and `finalize` calls to the underlying encoder.
pub enum AudioWriter {
    /// WAV format writer.
    Wav(WavWriter),
    /// OGG/Opus format writer.
    Opus(OpusWriter),
}

impl AudioWriter {
    /// Create a new audio writer for the given format and output path.
    pub fn create(format: AudioFormat, path: &Path) -> std::io::Result<Self> {
        match format {
            AudioFormat::Wav => Ok(AudioWriter::Wav(WavWriter::create(path)?)),
            AudioFormat::Opus => Ok(AudioWriter::Opus(OpusWriter::create(path)?)),
        }
    }

    /// Write f32 audio samples to the underlying encoder.
    ///
    /// Samples are 8 kHz mono, produced by the IMBE vocoder.
    /// Typical frame size is 160 samples (20 ms).
    pub fn write_samples(&mut self, samples: &[f32]) -> std::io::Result<()> {
        match self {
            AudioWriter::Wav(w) => w.write_samples(samples),
            AudioWriter::Opus(w) => w.write_samples(samples),
        }
    }

    /// Finalize the file (patch headers, flush buffers, close).
    ///
    /// Consumes the writer.
    pub fn finalize(self) -> std::io::Result<()> {
        match self {
            AudioWriter::Wav(w) => w.finalize(),
            AudioWriter::Opus(w) => w.finalize(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_extension() {
        assert_eq!(AudioFormat::Wav.extension(), "wav");
    }

    #[test]
    fn opus_extension() {
        assert_eq!(AudioFormat::Opus.extension(), "opus");
    }

    #[test]
    fn create_wav_writer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.wav");
        let writer = AudioWriter::create(AudioFormat::Wav, &path).unwrap();
        writer.finalize().unwrap();

        let data = std::fs::read(&path).unwrap();
        assert_eq!(&data[0..4], b"RIFF");
    }

    #[test]
    fn create_opus_writer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.opus");
        let writer = AudioWriter::create(AudioFormat::Opus, &path).unwrap();
        writer.finalize().unwrap();

        let data = std::fs::read(&path).unwrap();
        assert_eq!(&data[0..4], b"OggS");
    }

    #[test]
    fn write_samples_wav() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.wav");

        let mut writer = AudioWriter::create(AudioFormat::Wav, &path).unwrap();
        writer.write_samples(&[100.0; 160]).unwrap();
        writer.finalize().unwrap();

        let size = std::fs::metadata(&path).unwrap().len();
        assert_eq!(size, 44 + 160 * 2, "WAV: header + 160 i16 samples");
    }

    #[test]
    fn write_samples_opus() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.opus");

        let mut writer = AudioWriter::create(AudioFormat::Opus, &path).unwrap();
        writer.write_samples(&[100.0; 160]).unwrap();
        writer.finalize().unwrap();

        let data = std::fs::read(&path).unwrap();
        assert_eq!(&data[0..4], b"OggS");
        // Opus file with audio should be larger than just headers.
        assert!(data.len() > 100);
    }
}

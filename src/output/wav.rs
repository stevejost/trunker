//! Minimal WAV file writer for 8 kHz, 16-bit signed PCM, mono audio.
//!
//! Writes a placeholder header on creation, appends samples incrementally,
//! and patches the RIFF/data chunk sizes on [`WavWriter::finalize`].

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

/// WAV file writer that streams 16-bit PCM samples incrementally.
pub struct WavWriter {
    writer: BufWriter<File>,
    /// Number of audio data bytes written (excluding header).
    data_bytes: u32,
}

/// Sample rate for IMBE decoded audio (8 kHz).
const SAMPLE_RATE: u32 = 8000;
/// Mono channel count.
const CHANNELS: u16 = 1;
/// Bits per sample (16-bit signed PCM).
const BITS_PER_SAMPLE: u16 = 16;
/// Size of the WAV header in bytes.
const HEADER_SIZE: u32 = 44;

impl WavWriter {
    /// Create a new WAV file at the given path.
    ///
    /// Writes a placeholder 44-byte header. Call [`finalize`](WavWriter::finalize)
    /// when done to patch the data sizes.
    pub fn create(path: &Path) -> std::io::Result<Self> {
        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);
        write_header(&mut writer, 0)?;
        Ok(Self {
            writer,
            data_bytes: 0,
        })
    }

    /// Write f32 audio samples as 16-bit signed PCM.
    ///
    /// Samples are clamped to the i16 range (-32768, 32767). The IMBE
    /// vocoder outputs raw synthesis values (typical peak ~1000) which
    /// fit within i16 range without additional scaling.
    pub fn write_samples(&mut self, samples: &[f32]) -> std::io::Result<()> {
        for &sample in samples {
            let clamped = sample.clamp(i16::MIN as f32, i16::MAX as f32);
            let pcm = clamped as i16;
            self.writer.write_all(&pcm.to_le_bytes())?;
        }
        self.data_bytes += (samples.len() as u32) * 2;
        Ok(())
    }

    /// Finalize the WAV file by patching the header with correct sizes.
    pub fn finalize(mut self) -> std::io::Result<()> {
        self.writer.flush()?;
        self.writer.seek(SeekFrom::Start(0))?;
        write_header(&mut self.writer, self.data_bytes)?;
        self.writer.flush()?;
        Ok(())
    }
}

/// Write the 44-byte WAV header with the given data size.
fn write_header(w: &mut impl Write, data_bytes: u32) -> std::io::Result<()> {
    let byte_rate = SAMPLE_RATE * u32::from(CHANNELS) * u32::from(BITS_PER_SAMPLE) / 8;
    let block_align = CHANNELS * BITS_PER_SAMPLE / 8;

    // RIFF chunk
    w.write_all(b"RIFF")?;
    w.write_all(&(data_bytes + HEADER_SIZE - 8).to_le_bytes())?;
    w.write_all(b"WAVE")?;

    // fmt sub-chunk
    w.write_all(b"fmt ")?;
    w.write_all(&16_u32.to_le_bytes())?; // PCM format chunk size
    w.write_all(&1_u16.to_le_bytes())?; // PCM format tag
    w.write_all(&CHANNELS.to_le_bytes())?;
    w.write_all(&SAMPLE_RATE.to_le_bytes())?;
    w.write_all(&byte_rate.to_le_bytes())?;
    w.write_all(&block_align.to_le_bytes())?;
    w.write_all(&BITS_PER_SAMPLE.to_le_bytes())?;

    // data sub-chunk
    w.write_all(b"data")?;
    w.write_all(&data_bytes.to_le_bytes())?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn creates_valid_wav_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.wav");

        let writer = WavWriter::create(&path).unwrap();
        writer.finalize().unwrap();

        let mut data = Vec::new();
        File::open(&path).unwrap().read_to_end(&mut data).unwrap();

        assert_eq!(data.len(), 44, "empty WAV should be 44 bytes");
        assert_eq!(&data[0..4], b"RIFF");
        assert_eq!(&data[8..12], b"WAVE");
        assert_eq!(&data[12..16], b"fmt ");
        assert_eq!(&data[36..40], b"data");

        // Data size should be 0.
        let data_size = u32::from_le_bytes([data[40], data[41], data[42], data[43]]);
        assert_eq!(data_size, 0);

        // RIFF size should be 36 (44 - 8).
        let riff_size = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        assert_eq!(riff_size, 36);
    }

    #[test]
    fn writes_samples_and_patches_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.wav");

        let mut writer = WavWriter::create(&path).unwrap();
        // Write 160 samples (one IMBE frame) with typical vocoder-range values.
        let samples: Vec<f32> = (0..160).map(|i| (i as f32 - 80.0) * 5.0).collect();
        writer.write_samples(&samples).unwrap();
        writer.finalize().unwrap();

        let mut data = Vec::new();
        File::open(&path).unwrap().read_to_end(&mut data).unwrap();

        // 44 header + 160 samples * 2 bytes = 364 bytes.
        assert_eq!(data.len(), 364);

        // Data chunk size = 320.
        let data_size = u32::from_le_bytes([data[40], data[41], data[42], data[43]]);
        assert_eq!(data_size, 320);

        // RIFF size = 364 - 8 = 356.
        let riff_size = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        assert_eq!(riff_size, 356);

        // Sample rate = 8000.
        let sample_rate = u32::from_le_bytes([data[24], data[25], data[26], data[27]]);
        assert_eq!(sample_rate, 8000);

        // Channels = 1.
        let channels = u16::from_le_bytes([data[22], data[23]]);
        assert_eq!(channels, 1);

        // Bits per sample = 16.
        let bits = u16::from_le_bytes([data[34], data[35]]);
        assert_eq!(bits, 16);
    }

    #[test]
    fn clamps_out_of_range_samples() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.wav");

        let mut writer = WavWriter::create(&path).unwrap();
        writer.write_samples(&[50000.0, -50000.0, 500.0]).unwrap();
        writer.finalize().unwrap();

        let mut data = Vec::new();
        File::open(&path).unwrap().read_to_end(&mut data).unwrap();

        // Read back the 3 i16 samples.
        let s0 = i16::from_le_bytes([data[44], data[45]]);
        let s1 = i16::from_le_bytes([data[46], data[47]]);
        let s2 = i16::from_le_bytes([data[48], data[49]]);

        assert_eq!(s0, 32767, "50000.0 should clamp to i16::MAX");
        assert_eq!(s1, -32768, "-50000.0 should clamp to i16::MIN");
        assert_eq!(s2, 500, "500.0 should pass through as 500");
    }
}

//! OGG/Opus audio file writer for 8 kHz, mono, VOIP-mode audio.
//!
//! Encodes f32 audio samples (from the IMBE vocoder) into an OGG/Opus
//! container. Designed to match the [`WavWriter`](super::wav::WavWriter)
//! interface for use as an alternative audio output format.
//!
//! Requires system `libopus-dev` (apt) or `opus` (brew).

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use ogg::writing::{PacketWriteEndInfo, PacketWriter};

/// Sample rate for IMBE decoded audio in hertz.
const SAMPLE_RATE: u32 = 8000;
/// Number of audio channels (mono).
const CHANNEL_COUNT: u8 = 1;
/// Opus VOIP application bitrate in bits per second.
const BITRATE: i32 = 16_000;
/// Number of samples per IMBE frame (20 ms at 8 kHz).
const FRAME_SIZE: usize = 160;
/// Maximum encoded Opus packet size in bytes.
const MAX_PACKET_SIZE: usize = 4000;
/// Opus pre-skip in 48 kHz samples (standard value per RFC 7845).
const PRE_SKIP: u16 = 3840;
/// OGG stream serial number.
const STREAM_SERIAL: u32 = 1;
/// Opus internal sample rate for granule positions in hertz.
const OPUS_INTERNAL_RATE: u64 = 48000;
/// Granule position increment per frame (960 at 48 kHz for 20 ms).
const GRANULE_INCREMENT: u64 = (FRAME_SIZE as u64) * OPUS_INTERNAL_RATE / (SAMPLE_RATE as u64);
/// Vendor string for the OpusTags header.
const VENDOR: &[u8] = b"trunker";

/// OGG/Opus file writer that streams IMBE vocoder output to disk.
///
/// Encodes 8 kHz mono f32 samples at 16 kbps using the Opus VOIP
/// application mode. Samples are converted to i16 internally (matching
/// the same clamping path used by [`WavWriter`](super::wav::WavWriter)).
///
/// Call [`finalize`](OpusWriter::finalize) when done to write the
/// end-of-stream marker and flush the OGG container.
pub struct OpusWriter {
    encoder: opus::Encoder,
    packet_writer: PacketWriter<'static, BufWriter<File>>,
    /// Running granule position in 48 kHz samples.
    granule_position: u64,
    /// Reusable buffer for encoded Opus packets.
    encode_buffer: Vec<u8>,
    /// Reusable buffer for f32-to-i16 conversion.
    i16_buffer: Vec<i16>,
    /// Whether any audio frames have been written.
    has_audio: bool,
}

impl OpusWriter {
    /// Create a new OGG/Opus file at the given path.
    ///
    /// Writes the OpusHead and OpusTags header packets immediately.
    /// Call [`finalize`](OpusWriter::finalize) when done.
    pub fn create(path: &Path) -> std::io::Result<Self> {
        let encoder = create_encoder().map_err(std::io::Error::other)?;

        let file = File::create(path)?;
        let buf_writer = BufWriter::new(file);
        let mut packet_writer = PacketWriter::new(buf_writer);

        write_opus_head(&mut packet_writer)?;
        write_opus_tags(&mut packet_writer)?;

        Ok(Self {
            encoder,
            packet_writer,
            granule_position: 0,
            encode_buffer: vec![0u8; MAX_PACKET_SIZE],
            i16_buffer: Vec::with_capacity(FRAME_SIZE),
            has_audio: false,
        })
    }

    /// Write f32 audio samples as Opus-encoded packets.
    ///
    /// Samples are clamped to the i16 range and converted to i16 for the
    /// Opus encoder. The IMBE vocoder outputs raw synthesis values (typical
    /// peak ~1000) which fit within i16 without additional scaling.
    ///
    /// Input should contain exactly 160 samples (one IMBE frame) per call.
    pub fn write_samples(&mut self, samples: &[f32]) -> std::io::Result<()> {
        self.i16_buffer.clear();
        self.i16_buffer.extend(
            samples
                .iter()
                .map(|&s| s.clamp(i16::MIN as f32, i16::MAX as f32) as i16),
        );

        let encoded_len = self
            .encoder
            .encode(&self.i16_buffer, &mut self.encode_buffer)
            .map_err(std::io::Error::other)?;

        self.granule_position += GRANULE_INCREMENT;

        self.packet_writer.write_packet(
            self.encode_buffer[..encoded_len].to_vec(),
            STREAM_SERIAL,
            PacketWriteEndInfo::NormalPacket,
            self.granule_position,
        )?;

        self.has_audio = true;
        Ok(())
    }

    /// Finalize the OGG/Opus file by marking the end of stream.
    ///
    /// Writes a final silent frame with the EndStream flag and flushes
    /// the underlying writer. Consumes the writer.
    pub fn finalize(mut self) -> std::io::Result<()> {
        if self.has_audio {
            // Encode a silent frame for proper stream termination.
            let silence = vec![0_i16; FRAME_SIZE];
            let encoded_len = self
                .encoder
                .encode(&silence, &mut self.encode_buffer)
                .map_err(std::io::Error::other)?;

            self.granule_position += GRANULE_INCREMENT;

            self.packet_writer.write_packet(
                self.encode_buffer[..encoded_len].to_vec(),
                STREAM_SERIAL,
                PacketWriteEndInfo::EndStream,
                self.granule_position,
            )?;
        }

        self.packet_writer.inner_mut().flush()?;
        Ok(())
    }
}

/// Create and configure an Opus encoder for 8 kHz mono VOIP.
fn create_encoder() -> Result<opus::Encoder, opus::Error> {
    let mut encoder =
        opus::Encoder::new(SAMPLE_RATE, opus::Channels::Mono, opus::Application::Voip)?;
    encoder.set_bitrate(opus::Bitrate::Bits(BITRATE))?;
    Ok(encoder)
}

/// Build and write the OpusHead header packet (RFC 7845 Section 5.1).
fn write_opus_head(writer: &mut PacketWriter<'static, BufWriter<File>>) -> std::io::Result<()> {
    let mut head = Vec::with_capacity(19);
    head.extend_from_slice(b"OpusHead");
    head.push(1); // version
    head.push(CHANNEL_COUNT);
    head.extend_from_slice(&PRE_SKIP.to_le_bytes());
    head.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    head.extend_from_slice(&0_i16.to_le_bytes()); // output gain
    head.push(0); // channel mapping family (mono)

    writer.write_packet(head, STREAM_SERIAL, PacketWriteEndInfo::EndPage, 0)?;
    Ok(())
}

/// Build and write the OpusTags comment header packet (RFC 7845 Section 5.2).
fn write_opus_tags(writer: &mut PacketWriter<'static, BufWriter<File>>) -> std::io::Result<()> {
    let mut tags = Vec::new();
    tags.extend_from_slice(b"OpusTags");
    tags.extend_from_slice(&(VENDOR.len() as u32).to_le_bytes());
    tags.extend_from_slice(VENDOR);
    tags.extend_from_slice(&0_u32.to_le_bytes()); // no user comments

    writer.write_packet(tags, STREAM_SERIAL, PacketWriteEndInfo::EndPage, 0)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_valid_ogg_opus_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.opus");

        let writer = OpusWriter::create(&path).unwrap();
        writer.finalize().unwrap();

        let data = std::fs::read(&path).unwrap();
        assert!(data.len() >= 4, "file should not be empty");
        assert_eq!(&data[0..4], b"OggS", "file should start with OggS magic");
    }

    #[test]
    fn opus_head_has_correct_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.opus");

        let writer = OpusWriter::create(&path).unwrap();
        writer.finalize().unwrap();

        let data = std::fs::read(&path).unwrap();
        let head_pos = find_subsequence(&data, b"OpusHead").expect("OpusHead should be present");

        assert_eq!(data[head_pos + 8], 1, "version should be 1");
        assert_eq!(data[head_pos + 9], 1, "channels should be 1");

        let rate = u32::from_le_bytes([
            data[head_pos + 12],
            data[head_pos + 13],
            data[head_pos + 14],
            data[head_pos + 15],
        ]);
        assert_eq!(rate, 8000, "sample rate should be 8000");
    }

    #[test]
    fn opus_tags_contains_vendor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.opus");

        let writer = OpusWriter::create(&path).unwrap();
        writer.finalize().unwrap();

        let data = std::fs::read(&path).unwrap();
        let tags_pos = find_subsequence(&data, b"OpusTags").expect("OpusTags should be present");

        let vendor_len = u32::from_le_bytes([
            data[tags_pos + 8],
            data[tags_pos + 9],
            data[tags_pos + 10],
            data[tags_pos + 11],
        ]);
        assert_eq!(vendor_len, 7);
        assert_eq!(&data[tags_pos + 12..tags_pos + 19], b"trunker");
    }

    #[test]
    fn writes_audio_smaller_than_wav() {
        let dir = tempfile::tempdir().unwrap();
        let opus_path = dir.path().join("test.opus");
        let wav_path = dir.path().join("test.wav");

        // Generate 250 frames (5 seconds) of a 300 Hz sine wave.
        let mut opus_writer = OpusWriter::create(&opus_path).unwrap();
        let mut wav_writer = super::super::wav::WavWriter::create(&wav_path).unwrap();

        for frame_idx in 0..250 {
            let mut samples = [0.0_f32; FRAME_SIZE];
            for (i, sample) in samples.iter_mut().enumerate() {
                let t = (frame_idx * FRAME_SIZE + i) as f32 / SAMPLE_RATE as f32;
                *sample = (2.0 * std::f32::consts::PI * 300.0 * t).sin() * 500.0;
            }
            opus_writer.write_samples(&samples).unwrap();
            wav_writer.write_samples(&samples).unwrap();
        }
        opus_writer.finalize().unwrap();
        wav_writer.finalize().unwrap();

        let opus_size = std::fs::metadata(&opus_path).unwrap().len();
        let wav_size = std::fs::metadata(&wav_path).unwrap().len();

        assert!(
            opus_size < wav_size / 2,
            "Opus ({opus_size} bytes) should be less than half of WAV ({wav_size} bytes)"
        );
    }

    #[test]
    fn round_trip_correlation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.opus");

        // Encode a 300 Hz sine wave (50 frames = 1 second).
        let num_frames = 50;
        let mut all_i16 = Vec::with_capacity(num_frames * FRAME_SIZE);
        {
            let mut writer = OpusWriter::create(&path).unwrap();
            for frame_idx in 0..num_frames {
                let mut samples = [0.0_f32; FRAME_SIZE];
                for (i, sample) in samples.iter_mut().enumerate() {
                    let t = (frame_idx * FRAME_SIZE + i) as f32 / SAMPLE_RATE as f32;
                    *sample = (2.0 * std::f32::consts::PI * 300.0 * t).sin() * 500.0;
                }
                all_i16.extend(
                    samples
                        .iter()
                        .map(|&s| s.clamp(i16::MIN as f32, i16::MAX as f32) as i16),
                );
                writer.write_samples(&samples).unwrap();
            }
            writer.finalize().unwrap();
        }

        // Decode the file.
        let data = std::fs::read(&path).unwrap();
        let cursor = std::io::Cursor::new(&data);
        let mut reader = ogg::PacketReader::new(cursor);

        // Skip OpusHead and OpusTags.
        reader.read_packet().unwrap().unwrap();
        reader.read_packet().unwrap().unwrap();

        let mut decoder = opus::Decoder::new(SAMPLE_RATE, opus::Channels::Mono).unwrap();
        let mut decoded = Vec::new();
        let mut buf = vec![0_i16; FRAME_SIZE];
        while let Some(packet) = reader.read_packet().unwrap() {
            let len = decoder.decode(&packet.data, &mut buf, false).unwrap();
            decoded.extend_from_slice(&buf[..len]);
        }

        // Find best alignment (Opus has algorithmic delay).
        let window = FRAME_SIZE * 30;
        let max_delay = FRAME_SIZE * 15;
        let mut best_corr = 0.0_f64;
        for delay in 0..max_delay {
            let len = window.min(all_i16.len()).min(decoded.len() - delay);
            if len == 0 {
                continue;
            }
            let corr = pearson_i16(&all_i16[..len], &decoded[delay..delay + len]);
            if corr > best_corr {
                best_corr = corr;
            }
        }

        assert!(
            best_corr > 0.9,
            "round-trip correlation should be > 0.9, got {best_corr:.4}"
        );
    }

    #[test]
    fn error_on_invalid_path() {
        let result = OpusWriter::create(Path::new("/nonexistent/dir/test.opus"));
        assert!(result.is_err(), "should fail on invalid path");
    }

    #[test]
    fn empty_writer_produces_no_audio_packets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.opus");

        let writer = OpusWriter::create(&path).unwrap();
        writer.finalize().unwrap();

        let data = std::fs::read(&path).unwrap();
        let cursor = std::io::Cursor::new(&data);
        let mut reader = ogg::PacketReader::new(cursor);

        // OpusHead.
        let p1 = reader.read_packet().unwrap().unwrap();
        assert_eq!(&p1.data[..8], b"OpusHead");

        // OpusTags.
        let p2 = reader.read_packet().unwrap().unwrap();
        assert_eq!(&p2.data[..8], b"OpusTags");

        // No more packets.
        assert!(
            reader.read_packet().unwrap().is_none(),
            "empty writer should produce no audio packets"
        );
    }

    /// Find the first occurrence of `needle` in `haystack`.
    fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    /// Pearson correlation coefficient for two i16 slices.
    fn pearson_i16(a: &[i16], b: &[i16]) -> f64 {
        let n = a.len().min(b.len());
        let mean_a = a[..n].iter().map(|&x| x as f64).sum::<f64>() / n as f64;
        let mean_b = b[..n].iter().map(|&x| x as f64).sum::<f64>() / n as f64;

        let mut cross = 0.0_f64;
        let mut sq_a = 0.0_f64;
        let mut sq_b = 0.0_f64;

        for (&va, &vb) in a[..n].iter().zip(b[..n].iter()) {
            let da = va as f64 - mean_a;
            let db = vb as f64 - mean_b;
            cross += da * db;
            sq_a += da * da;
            sq_b += db * db;
        }

        if sq_a == 0.0 || sq_b == 0.0 {
            return 0.0;
        }
        cross / (sq_a.sqrt() * sq_b.sqrt())
    }
}

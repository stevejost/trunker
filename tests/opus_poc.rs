//! Proof-of-concept: encode PCM audio to OGG/Opus and decode back.
//!
//! Validates that the `opus` + `ogg` crate combination supports 8 kHz
//! mono encoding in VOIP mode with 20 ms frames (160 samples), matching
//! the IMBE vocoder output characteristics.

use ogg::PacketReader;
use ogg::writing::PacketWriteEndInfo;

/// Sample rate for IMBE decoded audio (8 kHz).
const SAMPLE_RATE: u32 = 8000;
/// Number of channels (mono).
const CHANNELS: u32 = 1;
/// Opus VOIP application bitrate in bits per second.
const BITRATE: i32 = 16_000;
/// Number of samples per IMBE frame (20 ms at 8 kHz).
const FRAME_SIZE: usize = 160;
/// Maximum encoded packet size in bytes.
const MAX_PACKET_SIZE: usize = 4000;
/// Opus pre-skip value in samples at 48 kHz (standard value).
const PRE_SKIP: u16 = 3840;
/// Serial number for the OGG stream.
const SERIAL: u32 = 1;

/// Build the OpusHead header packet (RFC 7845 Section 5.1).
fn build_opus_head() -> Vec<u8> {
    let mut head = Vec::with_capacity(19);
    head.extend_from_slice(b"OpusHead");
    head.push(1); // version
    head.push(CHANNELS as u8); // channel count
    head.extend_from_slice(&PRE_SKIP.to_le_bytes()); // pre-skip
    head.extend_from_slice(&SAMPLE_RATE.to_le_bytes()); // original sample rate
    head.extend_from_slice(&0_i16.to_le_bytes()); // output gain
    head.push(0); // channel mapping family
    head
}

/// Build the OpusTags comment header packet (RFC 7845 Section 5.2).
fn build_opus_tags() -> Vec<u8> {
    let vendor = b"trunker";
    let mut tags = Vec::new();
    tags.extend_from_slice(b"OpusTags");
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor);
    tags.extend_from_slice(&0_u32.to_le_bytes()); // no user comments
    tags
}

/// Convert f32 samples (IMBE vocoder range ~[-1000, 1000]) to i16.
///
/// Matches the same conversion path used by WavWriter.
fn f32_to_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&s| s.clamp(i16::MIN as f32, i16::MAX as f32) as i16)
        .collect()
}

/// Compute Pearson correlation between two i16 slices.
fn pearson_correlation(a: &[i16], b: &[i16]) -> f64 {
    let n = a.len().min(b.len());
    let a = &a[..n];
    let b = &b[..n];

    let mean_a = a.iter().map(|&x| x as f64).sum::<f64>() / n as f64;
    let mean_b = b.iter().map(|&x| x as f64).sum::<f64>() / n as f64;

    let mut cross = 0.0_f64;
    let mut sq_a = 0.0_f64;
    let mut sq_b = 0.0_f64;

    for (&va, &vb) in a.iter().zip(b.iter()) {
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

#[test]
fn round_trip_ogg_opus_8khz_mono() {
    // Generate a 300 Hz sine wave (50 frames = 1 second).
    // 300 Hz is well within the VOIP band (200-3500 Hz) for 8 kHz Opus.
    // Amplitude 500 matches typical IMBE vocoder output range.
    let num_frames = 50;
    let total_samples = num_frames * FRAME_SIZE;
    let mut pcm_f32 = vec![0.0_f32; total_samples];
    for (i, sample) in pcm_f32.iter_mut().enumerate() {
        let t = i as f32 / SAMPLE_RATE as f32;
        *sample = (2.0 * std::f32::consts::PI * 300.0 * t).sin() * 500.0;
    }

    // Convert to i16 (same path as WavWriter uses).
    let pcm_i16 = f32_to_i16(&pcm_f32);

    // --- Encode to OGG/Opus ---
    let mut encoder =
        opus::Encoder::new(SAMPLE_RATE, opus::Channels::Mono, opus::Application::Voip)
            .expect("encoder creation should succeed at 8 kHz");
    encoder
        .set_bitrate(opus::Bitrate::Bits(BITRATE))
        .expect("set bitrate");

    let mut ogg_buf = Vec::new();
    {
        let mut packet_writer = ogg::writing::PacketWriter::new(&mut ogg_buf);

        // Write OpusHead.
        packet_writer
            .write_packet(build_opus_head(), SERIAL, PacketWriteEndInfo::EndPage, 0)
            .expect("write OpusHead");

        // Write OpusTags.
        packet_writer
            .write_packet(build_opus_tags(), SERIAL, PacketWriteEndInfo::EndPage, 0)
            .expect("write OpusTags");

        // Encode and write audio frames.
        // Granule position is in 48 kHz samples (Opus internal rate).
        let granule_increment = (FRAME_SIZE as u64) * 48000 / (SAMPLE_RATE as u64);
        let mut granule_position: u64 = 0;
        let mut output = vec![0u8; MAX_PACKET_SIZE];

        for frame_idx in 0..num_frames {
            let start = frame_idx * FRAME_SIZE;
            let end = start + FRAME_SIZE;
            let encoded_len = encoder
                .encode(&pcm_i16[start..end], &mut output)
                .expect("encode should succeed");

            granule_position += granule_increment;

            let end_info = if frame_idx == num_frames - 1 {
                PacketWriteEndInfo::EndStream
            } else {
                PacketWriteEndInfo::NormalPacket
            };

            packet_writer
                .write_packet(
                    output[..encoded_len].to_vec(),
                    SERIAL,
                    end_info,
                    granule_position,
                )
                .expect("write audio packet");
        }
    }

    // Verify OGG magic bytes.
    assert_eq!(
        &ogg_buf[0..4],
        b"OggS",
        "OGG container should start with OggS magic"
    );

    // --- Decode from OGG/Opus ---
    let mut decoder =
        opus::Decoder::new(SAMPLE_RATE, opus::Channels::Mono).expect("decoder creation");

    let cursor = std::io::Cursor::new(&ogg_buf);
    let mut packet_reader = PacketReader::new(cursor);

    // Read and verify OpusHead.
    let head_packet = packet_reader
        .read_packet()
        .expect("read")
        .expect("OpusHead");
    assert_eq!(&head_packet.data[..8], b"OpusHead");
    assert_eq!(head_packet.data[8], 1, "OpusHead version should be 1");
    assert_eq!(head_packet.data[9], 1, "OpusHead channels should be 1");
    let rate = u32::from_le_bytes([
        head_packet.data[12],
        head_packet.data[13],
        head_packet.data[14],
        head_packet.data[15],
    ]);
    assert_eq!(rate, 8000, "OpusHead sample rate should be 8000");

    // Read and verify OpusTags.
    let tags_packet = packet_reader
        .read_packet()
        .expect("read")
        .expect("OpusTags");
    assert_eq!(&tags_packet.data[..8], b"OpusTags");

    // Decode audio packets back to i16.
    let mut decoded_i16 = Vec::new();
    let mut decode_buf = vec![0_i16; FRAME_SIZE];
    while let Some(packet) = packet_reader.read_packet().expect("read packet") {
        let decoded_len = decoder
            .decode(&packet.data, &mut decode_buf, false)
            .expect("decode should succeed");
        decoded_i16.extend_from_slice(&decode_buf[..decoded_len]);
    }

    assert_eq!(
        decoded_i16.len(),
        total_samples,
        "decoded sample count should match original"
    );

    // --- Verify correlation > 0.9 ---
    // Opus introduces algorithmic delay (pre-skip). When using the raw
    // encoder/decoder without OGG pre-skip handling, the decoded signal
    // is shifted in time. Find the best alignment by scanning offsets.
    let window = FRAME_SIZE * 30; // compare 600ms of aligned audio
    let max_delay = FRAME_SIZE * 10; // search up to 200ms delay
    let mut best_corr = 0.0_f64;
    let mut best_delay = 0_usize;

    for delay in 0..max_delay {
        let orig_start = 0;
        let dec_start = delay;
        let len = window.min(total_samples - delay);
        let corr = pearson_correlation(
            &pcm_i16[orig_start..orig_start + len],
            &decoded_i16[dec_start..dec_start + len],
        );
        if corr > best_corr {
            best_corr = corr;
            best_delay = delay;
        }
    }

    println!(
        "best correlation: {best_corr:.4} at delay {best_delay} samples ({:.1}ms)",
        best_delay as f64 / SAMPLE_RATE as f64 * 1000.0
    );

    assert!(
        best_corr > 0.9,
        "round-trip correlation should be > 0.9, got {best_corr:.4}"
    );

    // Verify the encoded OGG data is significantly smaller than raw PCM.
    let raw_pcm_size = total_samples * 2; // i16 PCM
    assert!(
        ogg_buf.len() < raw_pcm_size,
        "OGG/Opus ({} bytes) should be smaller than raw PCM ({} bytes)",
        ogg_buf.len(),
        raw_pcm_size
    );
}

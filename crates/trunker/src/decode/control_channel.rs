//! Control channel decode loop.
//!
//! Extracts the `decode_control_channel` function from main.rs into the
//! library so it can be reused by multiple frontends (CLI, LiveKit server).

use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::decode::DecoderEvent;
use crate::decode::heartbeat::HeartbeatState;
use crate::dsp::nco::Nco;
use crate::output::event_handler;
use crate::output::json;
use crate::output::time::format_utc_timestamp;
use crate::output::wav::WavWriter;
use crate::p25::ident::IdentTable;
use crate::p25::nid::NidIntegrityPolicy;
use crate::p25::receiver::ReceiverEvent;
use crate::pipeline::{self, ChannelPipeline, PipelineConfig};
use crate::sdr::sample_source::SampleSource;
use crate::vocoder::ImbeDecoder;

/// Configuration for the control channel decoder.
pub struct ControlChannelConfig {
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// NCO frequency offset in hertz (0.0 = no shift).
    pub offset_hz: f64,
    /// Modulation type (C4FM or CQPSK).
    pub modulation: pipeline::Modulation,
    /// NID integrity policy.
    pub nid_integrity: NidIntegrityPolicy,
    /// Whether to decode IMBE voice frames into audio.
    pub decode_audio: bool,
    /// Optional path to write decoded audio as a WAV file.
    pub audio_file: Option<String>,
    /// Heartbeat interval in seconds (0 = disabled).
    pub heartbeat_seconds: u64,
}

/// Run the control channel decode pipeline.
///
/// Reads IQ samples from `source`, demodulates, decodes TSBKs, and
/// writes JSON lines to stdout. Stops when `running` is set to `false`
/// or the sample source is exhausted.
pub fn decode_control_channel(
    source: &mut SampleSource,
    config: &ControlChannelConfig,
    running: &Arc<AtomicBool>,
) -> anyhow::Result<()> {
    let pipeline_config = PipelineConfig {
        sample_rate: config.sample_rate,
        modulation: config.modulation,
        nid_integrity: config.nid_integrity,
        sync_timeout_samples: Some(pipeline::CC_SYNC_TIMEOUT_SAMPLES),
    };
    let mut pipeline = ChannelPipeline::new(pipeline_config)?;
    let mut ident_table = IdentTable::new();
    let mut tsbk_count: u64 = 0;
    let mut decoder = if config.decode_audio {
        Some(ImbeDecoder::new())
    } else {
        None
    };

    let mut wav_writer = if let Some(path) = &config.audio_file {
        let writer = WavWriter::create(Path::new(path))
            .map_err(|e| anyhow::anyhow!("failed to create WAV file '{path}': {e}"))?;
        tracing::info!(path, "writing decoded audio to WAV file");
        Some(writer)
    } else {
        None
    };

    let mut nco = if config.offset_hz != 0.0 {
        tracing::info!(
            offset_hz = config.offset_hz,
            "applying NCO shift from center"
        );
        Some(Nco::new(config.offset_hz, config.sample_rate as f64))
    } else {
        None
    };

    let heartbeat_samples = u64::from(config.sample_rate) * config.heartbeat_seconds;
    let mut heartbeat = HeartbeatState::new();

    let mut stdout = std::io::LineWriter::new(std::io::stdout().lock());

    for iq_sample in source.by_ref() {
        if !running.load(Ordering::Relaxed) {
            tracing::info!("interrupted by signal");
            break;
        }

        let sample = match nco.as_mut() {
            Some(nco) => nco.shift(iq_sample),
            None => iq_sample,
        };

        if let Some(event) = pipeline.process_sample(sample) {
            if matches!(event, ReceiverEvent::Nid(_)) {
                heartbeat.record_cc_sync();
            }
            let nac = pipeline.current_nac();
            event_handler::handle_receiver_event(
                nac,
                &mut ident_table,
                &mut tsbk_count,
                &mut decoder,
                wav_writer.as_mut(),
                event,
                &mut stdout,
            );
        }

        // CC mode has no voice channels.
        if let Some(event) =
            heartbeat.maybe_produce(heartbeat_samples, config.heartbeat_seconds, tsbk_count, 0)
        {
            emit_heartbeat_event(&event, &mut stdout);
        }
    }

    if let Some(writer) = wav_writer {
        writer
            .finalize()
            .map_err(|e| anyhow::anyhow!("failed to finalize WAV file: {e}"))?;
        tracing::info!("WAV file finalized");
    }

    tracing::info!(
        samples = pipeline.sample_count(),
        tsbks = tsbk_count,
        "decode complete"
    );
    Ok(())
}

/// Emit a heartbeat event as a JSON line.
pub(crate) fn emit_heartbeat_event(event: &DecoderEvent, out: &mut impl Write) {
    if let DecoderEvent::Heartbeat {
        timestamp,
        uptime_seconds,
        tsbk_count,
        tsbk_rate,
        active_voice_channels,
        cc_sync,
    } = event
    {
        let heartbeat = json::HeartbeatJson {
            event_type: "heartbeat",
            timestamp: format_utc_timestamp(*timestamp),
            uptime_seconds: *uptime_seconds,
            tsbk_count: *tsbk_count,
            tsbk_rate: *tsbk_rate,
            active_voice_channels: *active_voice_channels,
            cc_sync: *cc_sync,
        };
        let line = json::heartbeat_line(&heartbeat);
        let _ = writeln!(out, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_are_sensible() {
        let config = ControlChannelConfig {
            sample_rate: 2_400_000,
            offset_hz: 0.0,
            modulation: pipeline::Modulation::C4fm,
            nid_integrity: NidIntegrityPolicy::Strict,
            decode_audio: false,
            audio_file: None,
            heartbeat_seconds: 10,
        };
        assert_eq!(config.sample_rate, 2_400_000);
        assert_eq!(config.heartbeat_seconds, 10);
        assert!(!config.decode_audio);
    }

    #[test]
    fn emit_heartbeat_writes_json_line() {
        let event = DecoderEvent::Heartbeat {
            timestamp: std::time::SystemTime::UNIX_EPOCH,
            uptime_seconds: 42,
            tsbk_count: 100,
            tsbk_rate: 2.5,
            active_voice_channels: 0,
            cc_sync: true,
        };
        let mut buf = Vec::new();
        emit_heartbeat_event(&event, &mut buf);
        let line = String::from_utf8(buf).unwrap();
        assert!(line.contains("\"type\":\"heartbeat\""));
        assert!(line.ends_with('\n'));
    }

    #[test]
    fn emit_heartbeat_ignores_non_heartbeat_events() {
        let event = DecoderEvent::CcSync;
        let mut buf = Vec::new();
        emit_heartbeat_event(&event, &mut buf);
        assert!(buf.is_empty());
    }
}

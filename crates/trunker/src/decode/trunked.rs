//! Wideband trunked decoder loop (CC + voice channels).
//!
//! Extracts `decode_trunked` from main.rs into the library so it can be
//! reused by multiple frontends (CLI, LiveKit server).

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::channel_manager::{ChannelManager, ChannelManagerConfig, VoiceChannelEvent};
use crate::decode::control_channel::emit_heartbeat_event;
use crate::decode::event::{DecoderEvent, EventSink};
use crate::decode::heartbeat::HeartbeatState;
use crate::dsp::nco::Nco;
use crate::output::call_recorder::{CallRecorder, CompletedRecording};
use crate::output::call_writer::AudioFormat;
use crate::output::json;
use crate::p25::ident::IdentTable;
use crate::p25::nid::NidIntegrityPolicy;
use crate::p25::receiver::ReceiverEvent;
use crate::p25::tsbk::{Tsbk, TsbkOpcode, TsbkPayload};
use crate::p25::types::{Frequency, SourceId};
use crate::pipeline::{self, ChannelPipeline, PipelineConfig};
use crate::sdr::sample_source::{SampleSource, SdrStatsHandles};

/// Configuration for the wideband trunked decoder.
pub struct TrunkedDecoderConfig {
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Center frequency of the capture in Hz.
    pub center_frequency: u64,
    /// NCO offset to shift CC to baseband (Hz).
    pub cc_offset_hz: f64,
    /// Modulation type (C4FM or CQPSK).
    pub modulation: pipeline::Modulation,
    /// NID integrity policy.
    pub nid_integrity: NidIntegrityPolicy,
    /// Seconds before tearing down an idle voice channel.
    pub call_timeout: f64,
    /// Whether to decode IMBE voice frames into audio.
    pub decode_audio: bool,
    /// Output directory for per-call audio recordings.
    pub output_dir: Option<PathBuf>,
    /// Audio file format for call recordings.
    pub audio_format: AudioFormat,
    /// Maximum simultaneous voice channel pipelines (None = unlimited).
    pub max_voices: Option<usize>,
    /// Emit full JSON lines instead of periodic stats.
    pub json_output: bool,
    /// Heartbeat interval in seconds (0 = disabled).
    pub heartbeat_seconds: u64,
}

/// Run the wideband trunked decoder (CC + voice channels).
///
/// If `event_sink` is provided, decoded events (TSBKs, heartbeats,
/// CC sync, voice events) are forwarded to it in addition to the
/// normal JSON/stats output.
pub fn decode_trunked(
    source: &mut SampleSource,
    config: &TrunkedDecoderConfig,
    running: &Arc<AtomicBool>,
    mut event_sink: Option<&mut dyn EventSink>,
) -> anyhow::Result<()> {
    let pipeline_config = PipelineConfig {
        sample_rate: config.sample_rate,
        modulation: config.modulation,
        nid_integrity: config.nid_integrity,
        sync_timeout_samples: Some(pipeline::CC_SYNC_TIMEOUT_SAMPLES),
    };
    let mut cc_pipeline = ChannelPipeline::new(pipeline_config)?;
    let mut ident_table = IdentTable::new();
    let mut tsbk_count: u64 = 0;

    // NCO to shift the CC to baseband when it's not at DC.
    let mut cc_nco = if config.cc_offset_hz.abs() > 0.1 {
        Some(Nco::new(config.cc_offset_hz, f64::from(config.sample_rate)))
    } else {
        None
    };

    let mut channel_manager = ChannelManager::new(ChannelManagerConfig {
        center_frequency: Frequency::from_hz(config.center_frequency),
        sample_rate: config.sample_rate,
        call_timeout_seconds: config.call_timeout,
        nid_integrity: config.nid_integrity,
        modulation: config.modulation,
        decode_audio: config.decode_audio,
        max_channels: config.max_voices,
    });

    let mut recorder = config
        .output_dir
        .as_deref()
        .map(|dir| CallRecorder::new(dir, config.audio_format));
    let mut voice_events = Vec::new();
    let mut stdout = std::io::LineWriter::new(std::io::stdout().lock());

    let sdr_handles = source.sdr_stats_handles();

    let heartbeat_samples = u64::from(config.sample_rate) * config.heartbeat_seconds;
    let mut heartbeat = HeartbeatState::new();

    // Stats mode: report every 10 seconds instead of per-event JSON.
    let stats_interval_samples = u64::from(config.sample_rate) * 10;
    let mut stats = TrunkStats::new();
    let mut samples_since_report: u64 = 0;
    let mut total_elapsed_seconds: u64 = 0;

    for iq_sample in source.by_ref() {
        if !running.load(Ordering::Relaxed) {
            tracing::info!("interrupted by signal");
            break;
        }

        // Accumulate raw IQ power for diagnostics.
        stats.iq_power_sum += (iq_sample.re * iq_sample.re + iq_sample.im * iq_sample.im) as f64;
        stats.iq_power_count += 1;

        // Shift CC to baseband if needed, then feed to CC pipeline.
        let cc_sample = match &mut cc_nco {
            Some(nco) => nco.shift(iq_sample),
            None => iq_sample,
        };
        if let Some(event) = cc_pipeline.process_sample(cc_sample) {
            let nac = cc_pipeline.current_nac();

            // Keep voice channel Costas seed in sync with CC's locked state.
            channel_manager.update_costas_seed(&cc_pipeline);

            // Count CC syncs for stats mode and heartbeat.
            if matches!(event, ReceiverEvent::Nid(_)) {
                stats.cc_syncs += 1;
                heartbeat.record_cc_sync();
                if let Some(sink) = &mut event_sink {
                    sink.handle(DecoderEvent::CcSync);
                }
            }

            // Forward TSBK events to the event sink.
            if let ReceiverEvent::Tsbk(ref tsbk) = event
                && let Some(sink) = &mut event_sink
            {
                sink.handle(DecoderEvent::Tsbk(tsbk.clone()));
            }

            if config.json_output {
                handle_cc_event(
                    nac,
                    &mut ident_table,
                    &mut tsbk_count,
                    &mut channel_manager,
                    recorder.as_mut(),
                    event,
                    &mut stdout,
                );
            } else {
                // Stats mode: process the event without JSON output.
                handle_cc_event_quiet(
                    &mut ident_table,
                    &mut tsbk_count,
                    &mut channel_manager,
                    recorder.as_mut(),
                    event,
                );
            }
        }

        // Feed to all active voice channel pipelines.
        voice_events.clear();
        channel_manager.process_sample(iq_sample, &mut voice_events);
        process_voice_events(
            &voice_events,
            recorder.as_mut(),
            &mut stats,
            config.json_output,
            &mut stdout,
        );
        // Forward voice frames to the event sink.
        if let Some(sink) = &mut event_sink {
            for voice_event in &voice_events {
                if let ReceiverEvent::VoiceFrame(ref vf) = voice_event.event {
                    sink.handle(DecoderEvent::VoiceFrame(vf.clone()));
                }
            }
        }

        // Periodic heartbeat (independent of stats mode).
        if let Some(hb_event) = heartbeat.maybe_produce(
            heartbeat_samples,
            config.heartbeat_seconds,
            tsbk_count,
            channel_manager.active_channel_count() as u32,
        ) {
            emit_heartbeat_event(&hb_event, &mut stdout);
            if let Some(sink) = &mut event_sink {
                sink.handle(hb_event);
            }
        }

        // Periodic stats reporting.
        if !config.json_output {
            samples_since_report += 1;
            if samples_since_report >= stats_interval_samples {
                total_elapsed_seconds += 10;
                report_stats(
                    &mut stats,
                    total_elapsed_seconds,
                    tsbk_count,
                    &channel_manager,
                    &cc_pipeline,
                    sdr_handles.as_ref(),
                );
                samples_since_report = 0;
            }
        }
    }

    // Finalize any open recordings on shutdown.
    if let Some(recorder) = recorder.as_mut() {
        let recordings = recorder.finalize_all();
        for recording in &recordings {
            log_completed_recording(recording);
        }
        tracing::info!(
            finalized = recordings.len(),
            "finalized open recordings on shutdown"
        );
    }

    tracing::info!(
        samples = cc_pipeline.sample_count(),
        tsbks = tsbk_count,
        active_voice_channels = channel_manager.active_channel_count(),
        "trunked decode complete"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Voice event processing
// ---------------------------------------------------------------------------

/// Process voice channel events: update recorder, count frames, emit JSON.
fn process_voice_events(
    voice_events: &[VoiceChannelEvent],
    mut recorder: Option<&mut CallRecorder>,
    stats: &mut TrunkStats,
    json_output: bool,
    out: &mut impl Write,
) {
    for voice_event in voice_events {
        if let Some(recorder) = recorder.as_mut()
            && let Some(completed) = recorder.process_event(voice_event)
        {
            log_completed_recording(&completed);
        }
        if matches!(voice_event.event, ReceiverEvent::VoiceFrame(_)) {
            stats.voice_frames += 1;
        }
        if json_output {
            emit_voice_event(voice_event, out);
        }
    }
}

// ---------------------------------------------------------------------------
// Stats reporting
// ---------------------------------------------------------------------------

/// Compute IQ power, build a stats snapshot, and print a report line.
fn report_stats(
    stats: &mut TrunkStats,
    elapsed_seconds: u64,
    tsbk_count: u64,
    channel_manager: &ChannelManager,
    cc_pipeline: &ChannelPipeline,
    sdr_handles: Option<&SdrStatsHandles>,
) {
    let avg_power = if stats.iq_power_count > 0 {
        (stats.iq_power_sum / stats.iq_power_count as f64) as f32
    } else {
        0.0
    };
    let iq_power_db = if avg_power > 0.0 {
        10.0 * avg_power.log10()
    } else {
        -100.0
    };
    let snap = StatsSnapshot {
        elapsed_seconds,
        tsbk_count,
        active_voices: channel_manager.active_channel_count(),
        costas_freq: cc_pipeline.costas_frequency(),
        expired_timeout: channel_manager.expired_timeout(),
        expired_no_carrier: channel_manager.expired_no_carrier(),
        sdr_stats: sdr_handles.map(|h| h.snapshot()),
        iq_power_db,
    };
    stats.iq_power_sum = 0.0;
    stats.iq_power_count = 0;
    stats.report(&snap, &mut std::io::stderr());
}

// ---------------------------------------------------------------------------
// CC event handling (trunked mode)
// ---------------------------------------------------------------------------

/// Process a TSBK: update ident table, forward grants to the channel
/// manager, and start recordings. Returns `true` if it was a TSBK event.
fn process_tsbk(
    ident_table: &mut IdentTable,
    tsbk_count: &mut u64,
    channel_manager: &mut ChannelManager,
    recorder: Option<&mut CallRecorder>,
    tsbk: &Tsbk,
) {
    if matches!(tsbk.payload, TsbkPayload::IdentifierUpdate { .. }) {
        ident_table.update(tsbk);
    }

    let is_grant = matches!(
        tsbk.header.opcode,
        TsbkOpcode::GroupVoiceChannelGrant
            | TsbkOpcode::GroupVoiceChannelGrantUpdate
            | TsbkOpcode::GroupVoiceChannelGrantUpdateExplicit
    );
    if is_grant {
        channel_manager.handle_grant(tsbk, ident_table);
        if let Some(recorder) = recorder {
            start_recording_for_grant(recorder, tsbk, ident_table, channel_manager);
        }
    }

    *tsbk_count += 1;
}

/// Handle a CC pipeline event: print JSON, update ident table, forward
/// grant events to the channel manager, and start recordings.
fn handle_cc_event(
    nac: crate::p25::types::Nac,
    ident_table: &mut IdentTable,
    tsbk_count: &mut u64,
    channel_manager: &mut ChannelManager,
    recorder: Option<&mut CallRecorder>,
    event: ReceiverEvent,
    out: &mut impl Write,
) {
    match event {
        ReceiverEvent::Nid(nid) => {
            tracing::debug!(
                nac = %nid.access_code,
                duid = ?nid.data_unit,
                parity_ok = nid.parity_ok,
                "CC NID decoded"
            );
        }
        ReceiverEvent::Tsbk(tsbk) => {
            process_tsbk(ident_table, tsbk_count, channel_manager, recorder, &tsbk);
            let line = json::to_json_line(nac, &tsbk, ident_table);
            let _ = writeln!(out, "{line}");
        }
        ReceiverEvent::Error(err) => {
            tracing::debug!(error = %err, "CC decode error");
        }
        // CC shouldn't produce voice events, but handle gracefully.
        _ => {}
    }
}

/// Handle a CC pipeline event in stats mode: update ident table, forward
/// grant events, and bump counters -- but skip JSON output.
fn handle_cc_event_quiet(
    ident_table: &mut IdentTable,
    tsbk_count: &mut u64,
    channel_manager: &mut ChannelManager,
    recorder: Option<&mut CallRecorder>,
    event: ReceiverEvent,
) {
    match event {
        ReceiverEvent::Tsbk(tsbk) => {
            process_tsbk(ident_table, tsbk_count, channel_manager, recorder, &tsbk);
        }
        ReceiverEvent::Error(err) => {
            tracing::debug!(error = %err, "CC decode error");
        }
        _ => {}
    }
}

/// Start a call recording for a grant TSBK if it represents a new channel
/// activation and the frequency is within capture bandwidth.
fn start_recording_for_grant(
    recorder: &mut CallRecorder,
    tsbk: &Tsbk,
    ident_table: &IdentTable,
    channel_manager: &ChannelManager,
) {
    match &tsbk.payload {
        TsbkPayload::GroupVoiceChannelGrant {
            channel,
            talkgroup,
            source,
        } => {
            if let Some(freq) = ident_table.resolve_frequency(*channel)
                && channel_manager.is_in_band(freq)
                && !recorder.has_active_recording(&freq)
                && let Some(displaced) = recorder.start_call(freq, *talkgroup, *source)
            {
                log_completed_recording(&displaced);
            }
        }
        TsbkPayload::GroupVoiceChannelGrantUpdate {
            channel_a,
            talkgroup_a,
            channel_b,
            talkgroup_b,
        } => {
            let zero_source = SourceId::new(0);
            if let Some(freq) = ident_table.resolve_frequency(*channel_a)
                && channel_manager.is_in_band(freq)
                && !recorder.has_active_recording(&freq)
                && let Some(displaced) = recorder.start_call(freq, *talkgroup_a, zero_source)
            {
                log_completed_recording(&displaced);
            }
            if let Some(freq) = ident_table.resolve_frequency(*channel_b)
                && channel_manager.is_in_band(freq)
                && !recorder.has_active_recording(&freq)
                && let Some(displaced) = recorder.start_call(freq, *talkgroup_b, zero_source)
            {
                log_completed_recording(&displaced);
            }
        }
        TsbkPayload::GroupVoiceChannelGrantUpdateExplicit {
            receive_channel,
            talkgroup,
            ..
        } => {
            let zero_source = SourceId::new(0);
            if let Some(freq) = ident_table.resolve_frequency(*receive_channel)
                && channel_manager.is_in_band(freq)
                && !recorder.has_active_recording(&freq)
                && let Some(displaced) = recorder.start_call(freq, *talkgroup, zero_source)
            {
                log_completed_recording(&displaced);
            }
        }
        _ => {}
    }
}

/// Log metadata for a completed call recording.
pub(crate) fn log_completed_recording(recording: &CompletedRecording) {
    tracing::info!(
        talkgroup = %recording.talkgroup,
        source = %recording.source,
        frequency = %recording.frequency,
        frame_count = recording.frame_count,
        end_reason = ?recording.end_reason,
        audio_file = ?recording.audio_file,
        "call recording completed"
    );
}

/// Emit a voice channel event as a JSON line with call context.
fn emit_voice_event(voice_event: &VoiceChannelEvent, out: &mut impl Write) {
    let nac = voice_event.nac;
    let freq = voice_event.frequency;
    let tg = voice_event.talkgroup;
    let src = voice_event.source;

    match &voice_event.event {
        ReceiverEvent::VoiceFrame(vf) => {
            let line = json::voice_frame_with_context(nac, vf, freq, tg, src);
            let _ = writeln!(out, "{line}");
        }
        ReceiverEvent::LinkControl(lc) => {
            let line = json::link_control_with_context(nac, lc, freq, tg, src);
            let _ = writeln!(out, "{line}");
        }
        ReceiverEvent::CryptoControl(cc) => {
            let line = json::crypto_control_with_context(nac, cc, freq, tg, src);
            let _ = writeln!(out, "{line}");
        }
        ReceiverEvent::VoiceHeader(hdr) => {
            let line = json::voice_header_with_context(nac, hdr, freq, tg, src);
            let _ = writeln!(out, "{line}");
        }
        ReceiverEvent::DataFragment(frag) => {
            let line = json::data_fragment_with_context(nac, *frag, freq, tg, src);
            let _ = writeln!(out, "{line}");
        }
        ReceiverEvent::Nid(nid) => {
            tracing::debug!(
                frequency = %voice_event.frequency,
                talkgroup = %voice_event.talkgroup,
                nac = %nid.access_code,
                duid = ?nid.data_unit,
                "voice channel NID"
            );
        }
        ReceiverEvent::Tsbk(_) => {
            // Voice channels shouldn't produce TSBKs; ignore.
        }
        ReceiverEvent::Error(err) => {
            tracing::debug!(
                frequency = %voice_event.frequency,
                error = %err,
                "voice channel decode error"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Stats reporting
// ---------------------------------------------------------------------------

/// Periodic stats counters for the trunked decoder.
///
/// Accumulates event counts between stats report intervals. The `prev_*`
/// fields track totals at the last report so deltas can be computed.
struct TrunkStats {
    /// CC sync detections since last report.
    cc_syncs: u64,
    /// Voice frames decoded since last report.
    voice_frames: u64,
    /// Total TSBKs at last report (for computing delta).
    prev_tsbk_count: u64,
    /// Total expired-timeout at last report.
    prev_expired_timeout: u64,
    /// Total expired-no-carrier at last report.
    prev_expired_no_carrier: u64,
    /// Total SDR chunks at last report (for computing delta).
    prev_sdr_chunks: u64,
    /// Accumulated raw IQ power (|sample|^2) over interval.
    iq_power_sum: f64,
    /// Number of samples accumulated for power measurement.
    iq_power_count: u64,
}

/// Snapshot of current decoder state for stats reporting.
struct StatsSnapshot {
    elapsed_seconds: u64,
    tsbk_count: u64,
    active_voices: usize,
    costas_freq: f32,
    expired_timeout: u64,
    expired_no_carrier: u64,
    /// `(chunk_count, overflow_count)` from the SDR reader thread, if live.
    sdr_stats: Option<(u64, u64)>,
    /// Average raw IQ power (|sample|^2) over this interval.
    iq_power_db: f32,
}

impl TrunkStats {
    fn new() -> Self {
        Self {
            cc_syncs: 0,
            voice_frames: 0,
            prev_tsbk_count: 0,
            prev_expired_timeout: 0,
            prev_expired_no_carrier: 0,
            prev_sdr_chunks: 0,
            iq_power_sum: 0.0,
            iq_power_count: 0,
        }
    }

    /// Print a stats line and reset interval counters.
    fn report(&mut self, snap: &StatsSnapshot, out: &mut impl Write) {
        let tsbk_delta = snap.tsbk_count - self.prev_tsbk_count;
        let expired_delta = (snap.expired_timeout - self.prev_expired_timeout)
            + (snap.expired_no_carrier - self.prev_expired_no_carrier);
        let no_carrier_delta = snap.expired_no_carrier - self.prev_expired_no_carrier;

        // Base stats line.
        let _ = write!(
            out,
            "[{:>4}s] tsbks={} (+{}) voices={} frames={} cc_syncs={} \
             costas_freq={:.4} iq_power={:.1}dB expired={} (no_carrier={})",
            snap.elapsed_seconds,
            snap.tsbk_count,
            tsbk_delta,
            snap.active_voices,
            self.voice_frames,
            self.cc_syncs,
            snap.costas_freq,
            snap.iq_power_db,
            expired_delta,
            no_carrier_delta,
        );

        // Append SDR reader thread stats when running live.
        if let Some((chunks, overflows)) = snap.sdr_stats {
            let chunk_delta = chunks - self.prev_sdr_chunks;
            let _ = write!(
                out,
                " sdr_chunks={chunks} (+{chunk_delta}) overflows={overflows}"
            );
            self.prev_sdr_chunks = chunks;
        }

        let _ = writeln!(out);

        // Reset interval counters.
        self.cc_syncs = 0;
        self.voice_frames = 0;
        self.prev_tsbk_count = snap.tsbk_count;
        self.prev_expired_timeout = snap.expired_timeout;
        self.prev_expired_no_carrier = snap.expired_no_carrier;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_are_sensible() {
        let config = TrunkedDecoderConfig {
            sample_rate: 2_400_000,
            center_frequency: 852_350_000,
            cc_offset_hz: 0.0,
            modulation: pipeline::Modulation::Cqpsk,
            nid_integrity: NidIntegrityPolicy::Strict,
            call_timeout: 3.0,
            decode_audio: false,
            output_dir: None,
            audio_format: AudioFormat::Wav,
            max_voices: Some(10),
            json_output: false,
            heartbeat_seconds: 10,
        };
        assert_eq!(config.sample_rate, 2_400_000);
        assert_eq!(config.center_frequency, 852_350_000);
        assert!(!config.json_output);
    }

    #[test]
    fn trunk_stats_report_writes_output() {
        let mut stats = TrunkStats::new();
        let snap = StatsSnapshot {
            elapsed_seconds: 10,
            tsbk_count: 42,
            active_voices: 2,
            costas_freq: 0.001,
            expired_timeout: 1,
            expired_no_carrier: 0,
            sdr_stats: None,
            iq_power_db: -30.0,
        };
        let mut buf = Vec::new();
        stats.report(&snap, &mut buf);
        let line = String::from_utf8(buf).unwrap();
        assert!(line.contains("tsbks=42"));
        assert!(line.contains("voices=2"));
        assert!(line.ends_with('\n'));
    }

    #[test]
    fn trunk_stats_report_resets_interval_counters() {
        let mut stats = TrunkStats::new();
        stats.cc_syncs = 5;
        stats.voice_frames = 10;
        let snap = StatsSnapshot {
            elapsed_seconds: 10,
            tsbk_count: 42,
            active_voices: 0,
            costas_freq: 0.0,
            expired_timeout: 0,
            expired_no_carrier: 0,
            sdr_stats: None,
            iq_power_db: -100.0,
        };
        let mut buf = Vec::new();
        stats.report(&snap, &mut buf);
        assert_eq!(stats.cc_syncs, 0);
        assert_eq!(stats.voice_frames, 0);
        assert_eq!(stats.prev_tsbk_count, 42);
    }

    #[test]
    fn trunk_stats_report_includes_sdr_stats() {
        let mut stats = TrunkStats::new();
        let snap = StatsSnapshot {
            elapsed_seconds: 10,
            tsbk_count: 0,
            active_voices: 0,
            costas_freq: 0.0,
            expired_timeout: 0,
            expired_no_carrier: 0,
            sdr_stats: Some((100, 2)),
            iq_power_db: -30.0,
        };
        let mut buf = Vec::new();
        stats.report(&snap, &mut buf);
        let line = String::from_utf8(buf).unwrap();
        assert!(line.contains("sdr_chunks=100"));
        assert!(line.contains("overflows=2"));
    }
}

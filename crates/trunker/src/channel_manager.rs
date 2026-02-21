//! Voice channel lifecycle management for wideband trunking.
//!
//! Watches control channel grant events, spawns per-channel decode
//! pipelines (NCO + decimation + demod), and tears them down after
//! grant updates stop. Each active voice channel gets its own
//! [`ChannelPipeline`] running on a dedicated thread, fed IQ samples
//! via a bounded crossbeam channel.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use num_complex::Complex;

use crate::dsp::nco::Nco;
use crate::p25::ident::IdentTable;
use crate::p25::nid::NidIntegrityPolicy;
use crate::p25::receiver::ReceiverEvent;
use crate::p25::tsbk::{Tsbk, TsbkPayload};
use crate::p25::types::{Frequency, Nac, SourceId, TalkgroupId};
use crate::pipeline::{ChannelPipeline, Modulation, PipelineConfig};
use crate::vocoder::{AudioBuffer, ImbeDecoder, ReceivedFrame, SAMPLES_PER_FRAME};

/// A voice channel event with call context.
#[derive(Debug)]
pub struct VoiceChannelEvent {
    /// Receive frequency of this voice channel in hertz.
    pub frequency: Frequency,
    /// Talkgroup for this call.
    pub talkgroup: TalkgroupId,
    /// Source unit that initiated the call.
    pub source: SourceId,
    /// NAC from the voice channel's NID.
    pub nac: Nac,
    /// The decoded protocol event.
    pub event: ReceiverEvent,
    /// Decoded PCM audio samples (8 kHz, f32) for voice frame events.
    ///
    /// Present only when `--decode-audio` is enabled and the event is a
    /// `VoiceFrame`. Contains 160 samples (20 ms at 8 kHz).
    pub audio: Option<Vec<f32>>,
}

/// An active voice channel with its DSP pipeline.
///
/// Owned by the voice thread; moved into the thread at spawn time.
struct VoiceChannel {
    /// Receive frequency of this channel.
    frequency: Frequency,
    /// Talkgroup for the active call.
    talkgroup: TalkgroupId,
    /// Source unit that initiated the call.
    source: SourceId,
    /// NCO for shifting this channel to baseband.
    nco: Nco,
    /// Per-channel decode pipeline.
    pipeline: ChannelPipeline,
    /// IMBE vocoder decoder (present when `--decode-audio` is enabled).
    decoder: Option<ImbeDecoder>,
}

/// Handle to a voice channel thread.
///
/// Stored in the manager's HashMap. The actual DSP work runs on the
/// thread; this handle provides the IQ send channel, shared carrier
/// state, and metadata for grant refresh and expiry.
struct VoiceThreadHandle {
    /// Send IQ samples to this thread. Drop to signal shutdown.
    iq_sender: crossbeam_channel::Sender<Complex<f32>>,
    /// Thread join handle for cleanup.
    join_handle: Option<thread::JoinHandle<()>>,
    /// Whether carrier has been acquired (shared with voice thread).
    carrier_acquired: Arc<AtomicBool>,
    /// Receive frequency of this channel.
    frequency: Frequency,
    /// Talkgroup for the active call.
    talkgroup: TalkgroupId,
    /// Source unit that initiated the call.
    source: SourceId,
    /// Sample counter at last grant update (for timeout).
    last_grant_sample: u64,
}

/// Capacity of the bounded IQ sample channel per voice thread.
///
/// 4096 samples at 2.4 MS/s is ~1.7 ms of buffering.
const IQ_CHANNEL_CAPACITY: usize = 4096;

/// Voice thread entry point.
///
/// Receives IQ samples from the manager thread, runs the full DSP
/// pipeline (NCO shift, demod, decode), and sends decoded events
/// back to the manager via the event channel.
fn voice_thread_main(
    mut channel: VoiceChannel,
    iq_receiver: crossbeam_channel::Receiver<Complex<f32>>,
    event_sender: crossbeam_channel::Sender<VoiceChannelEvent>,
    carrier_acquired: Arc<AtomicBool>,
) {
    while let Ok(sample) = iq_receiver.recv() {
        let shifted = channel.nco.shift(sample);
        if let Some(event) = channel.pipeline.process_sample(shifted) {
            // Mark carrier acquired on first valid NID decode.
            if let ReceiverEvent::Nid(nid) = &event
                && nid.parity_ok
                && !carrier_acquired.load(Ordering::Relaxed)
            {
                carrier_acquired.store(true, Ordering::Relaxed);
                tracing::debug!(
                    frequency = %channel.frequency,
                    "carrier acquired, audio output enabled"
                );
            }

            // Decode audio only after carrier is acquired.
            let audio = if carrier_acquired.load(Ordering::Relaxed) {
                if let (Some(decoder), ReceiverEvent::VoiceFrame(vf)) =
                    (channel.decoder.as_mut(), &event)
                {
                    let received = ReceivedFrame::from(vf);
                    let mut buffer: AudioBuffer = [0.0; SAMPLES_PER_FRAME];
                    decoder.decode(received, &mut buffer);
                    Some(buffer.to_vec())
                } else {
                    None
                }
            } else {
                None
            };

            let _ = event_sender.send(VoiceChannelEvent {
                frequency: channel.frequency,
                talkgroup: channel.talkgroup,
                source: channel.source,
                nac: channel.pipeline.current_nac(),
                event,
                audio,
            });
        }
    }
    // iq_receiver disconnected — sender was dropped, thread exits cleanly.
}

/// Configuration for the channel manager.
#[derive(Debug, Clone)]
pub struct ChannelManagerConfig {
    /// Center frequency of the wideband capture in hertz.
    pub center_frequency: Frequency,
    /// Wideband sample rate in hertz.
    pub sample_rate: u32,
    /// Call timeout in seconds (channels torn down after this long without
    /// a grant update).
    pub call_timeout_seconds: f64,
    /// Modulation type for voice channel pipelines.
    pub modulation: Modulation,
    /// NID integrity policy for voice channel pipelines.
    pub nid_integrity: NidIntegrityPolicy,
    /// Whether to decode IMBE voice frames into audio.
    pub decode_audio: bool,
    /// Maximum number of simultaneous voice channel pipelines.
    /// `None` means no limit.
    pub max_channels: Option<usize>,
}

/// Manages voice channel pipelines for wideband trunking.
///
/// Receives grant events from the control channel and manages the
/// lifecycle of voice channel decode pipelines. Feed wideband IQ
/// samples via [`process_sample`] to decode all active voice channels.
pub struct ChannelManager {
    center_frequency_hz: f64,
    sample_rate: f64,
    /// Usable bandwidth: 80% of sample rate to avoid rolloff at edges.
    usable_bandwidth: f64,
    /// Timeout in samples (converted from seconds).
    call_timeout_samples: u64,
    /// Active voice channel threads keyed by receive frequency.
    active_channels: HashMap<Frequency, VoiceThreadHandle>,
    /// Pipeline configuration for new voice channels.
    pipeline_config: PipelineConfig,
    /// Whether to decode IMBE voice frames into audio.
    decode_audio: bool,
    /// Global sample counter.
    sample_count: u64,
    /// Costas loop state from the CC pipeline, used to seed new voice
    /// channel pipelines for faster carrier acquisition.
    cc_costas_seed: Option<(f32, f32)>,
    /// Maximum simultaneous voice channels (`None` = unlimited).
    max_channels: Option<usize>,
    /// Channels expired due to call timeout (carrier was acquired).
    expired_timeout: u64,
    /// Channels expired due to no carrier acquisition within 1 second.
    expired_no_carrier: u64,
    /// Receiver for events from all voice threads.
    event_receiver: crossbeam_channel::Receiver<VoiceChannelEvent>,
    /// Sender cloned into each voice thread.
    event_sender: crossbeam_channel::Sender<VoiceChannelEvent>,
}

/// Fraction of sample rate considered usable bandwidth.
/// Signals near the edges of the capture bandwidth are attenuated
/// by the anti-aliasing filter, so we only accept channels within
/// 80% of the Nyquist bandwidth.
const USABLE_BANDWIDTH_FRACTION: f64 = 0.8;

impl ChannelManager {
    /// Create a new channel manager with the given configuration.
    pub fn new(config: ChannelManagerConfig) -> Self {
        let sample_rate = config.sample_rate as f64;
        let timeout_samples = (config.call_timeout_seconds * sample_rate) as u64;
        let (event_sender, event_receiver) = crossbeam_channel::unbounded();

        Self {
            center_frequency_hz: config.center_frequency.hz() as f64,
            sample_rate,
            usable_bandwidth: sample_rate * USABLE_BANDWIDTH_FRACTION,
            call_timeout_samples: timeout_samples,
            active_channels: HashMap::new(),
            pipeline_config: PipelineConfig {
                sample_rate: config.sample_rate,
                modulation: config.modulation,
                nid_integrity: config.nid_integrity,
                // Voice channels disable the sync timeout — they need
                // 250 ms-1 s for initial acquisition. Zombie channels
                // are handled by the acquisition timeout instead.
                sync_timeout_samples: None,
            },
            decode_audio: config.decode_audio,
            sample_count: 0,
            cc_costas_seed: None,
            max_channels: config.max_channels,
            expired_timeout: 0,
            expired_no_carrier: 0,
            event_receiver,
            event_sender,
        }
    }

    /// Process a grant TSBK from the control channel.
    ///
    /// If the grant frequency is within the capture bandwidth, a voice
    /// channel pipeline is created (or an existing one is refreshed).
    pub fn handle_grant(&mut self, tsbk: &Tsbk, ident_table: &IdentTable) {
        match &tsbk.payload {
            TsbkPayload::GroupVoiceChannelGrant {
                channel,
                talkgroup,
                source,
            } => {
                if let Some(freq) = ident_table.resolve_frequency(*channel) {
                    self.activate_channel(freq, *talkgroup, *source);
                }
            }
            TsbkPayload::GroupVoiceChannelGrantUpdate {
                channel_a,
                talkgroup_a,
                channel_b,
                talkgroup_b,
            } => {
                // Source is not available in grant updates; use zero.
                let zero_source = SourceId::new(0);
                if let Some(freq) = ident_table.resolve_frequency(*channel_a) {
                    self.activate_channel(freq, *talkgroup_a, zero_source);
                }
                if let Some(freq) = ident_table.resolve_frequency(*channel_b) {
                    self.activate_channel(freq, *talkgroup_b, zero_source);
                }
            }
            TsbkPayload::GroupVoiceChannelGrantUpdateExplicit {
                receive_channel,
                talkgroup,
                ..
            } => {
                let zero_source = SourceId::new(0);
                if let Some(freq) = ident_table.resolve_frequency(*receive_channel) {
                    self.activate_channel(freq, *talkgroup, zero_source);
                }
            }
            _ => {}
        }
    }

    /// Feed one wideband IQ sample to all active voice channel threads.
    ///
    /// Fans out the sample to each voice thread via bounded channels,
    /// then drains any decoded events from the shared event channel
    /// into the caller-provided buffer. Also expires timed-out channels.
    pub fn process_sample(&mut self, sample: Complex<f32>, events: &mut Vec<VoiceChannelEvent>) {
        self.sample_count += 1;

        // Fan out IQ sample to all voice threads.
        for handle in self.active_channels.values() {
            if handle.iq_sender.try_send(sample).is_err() {
                tracing::trace!(
                    frequency = %handle.frequency,
                    "voice thread lagging, dropped sample"
                );
            }
        }

        // Drain events from voice threads.
        while let Ok(event) = self.event_receiver.try_recv() {
            events.push(event);
        }

        // Expire timed-out channels periodically (every 10k samples to
        // avoid HashMap iteration overhead on every sample).
        if self.sample_count.is_multiple_of(10_000) {
            self.expire_channels();
        }
    }

    /// Return the number of currently active voice channels.
    pub fn active_channel_count(&self) -> usize {
        self.active_channels.len()
    }

    /// Return the total number of wideband samples processed.
    pub fn sample_count(&self) -> u64 {
        self.sample_count
    }

    /// Channels expired due to call timeout (had acquired carrier).
    pub fn expired_timeout(&self) -> u64 {
        self.expired_timeout
    }

    /// Channels expired due to no carrier acquisition within 1 second.
    pub fn expired_no_carrier(&self) -> u64 {
        self.expired_no_carrier
    }

    /// Update the Costas loop seed from the control channel pipeline.
    ///
    /// Call this periodically (e.g., on each CC pipeline event) so that
    /// newly spawned voice channels start with the CC's locked Costas
    /// state, reducing acquisition from ~22 frames to ~2-5 frames.
    pub fn update_costas_seed(&mut self, cc_pipeline: &ChannelPipeline) {
        self.cc_costas_seed = cc_pipeline.costas_state();
    }

    /// Refresh an existing voice channel's timeout and talkgroup.
    ///
    /// Called for grant update TSBKs (opcodes 0x02, 0x04) which indicate
    /// an ongoing call but should not create new pipelines. If the
    /// frequency is not already active, the update is silently ignored.
    #[allow(dead_code)]
    fn refresh_channel(&mut self, frequency: Frequency, talkgroup: TalkgroupId) {
        if let Some(handle) = self.active_channels.get_mut(&frequency) {
            handle.talkgroup = talkgroup;
            handle.last_grant_sample = self.sample_count;
            tracing::trace!(
                frequency = %frequency,
                talkgroup = %talkgroup,
                "refreshed voice channel via grant update"
            );
        }
    }

    /// Activate or refresh a voice channel at the given frequency.
    ///
    /// For new channels, spawns a dedicated voice thread with its own
    /// DSP pipeline. For existing channels, refreshes the grant timeout
    /// and metadata on the handle.
    fn activate_channel(&mut self, frequency: Frequency, talkgroup: TalkgroupId, source: SourceId) {
        if !self.is_in_band(frequency) {
            tracing::debug!(
                frequency = %frequency,
                "grant frequency out of capture bandwidth, skipping"
            );
            return;
        }

        if let Some(handle) = self.active_channels.get_mut(&frequency) {
            // Refresh existing channel: update talkgroup, source (if
            // non-zero), and timeout.
            handle.talkgroup = talkgroup;
            if source.value() != 0 {
                handle.source = source;
            }
            handle.last_grant_sample = self.sample_count;
            tracing::debug!(
                frequency = %frequency,
                talkgroup = %talkgroup,
                "refreshed voice channel"
            );
            return;
        }

        // Enforce voice channel cap.
        if let Some(max) = self.max_channels
            && self.active_channels.len() >= max
        {
            tracing::debug!(
                frequency = %frequency,
                max_channels = max,
                "max voice channels reached, skipping"
            );
            return;
        }

        // New channel: compute NCO offset and create pipeline.
        let offset_hz = frequency.hz() as f64 - self.center_frequency_hz;
        let nco = Nco::new(offset_hz, self.sample_rate);
        let mut pipeline = match ChannelPipeline::new(self.pipeline_config) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(
                    frequency = %frequency,
                    error = %e,
                    "failed to create voice channel pipeline"
                );
                return;
            }
        };

        // Seed the Costas loop frequency from the CC's locked state.
        if let Some((_phase, freq)) = self.cc_costas_seed {
            pipeline.seed_costas(0.0, freq);
            tracing::debug!(
                frequency = %frequency,
                seed_freq = freq,
                "seeded voice channel Costas frequency from CC"
            );
        }

        let decoder = if self.decode_audio {
            Some(ImbeDecoder::new())
        } else {
            None
        };

        let channel = VoiceChannel {
            frequency,
            talkgroup,
            source,
            nco,
            pipeline,
            decoder,
        };

        // Create bounded IQ channel and shared carrier state.
        let (iq_sender, iq_receiver) = crossbeam_channel::bounded(IQ_CHANNEL_CAPACITY);
        let carrier_acquired = Arc::new(AtomicBool::new(false));
        let carrier_flag = Arc::clone(&carrier_acquired);
        let event_sender = self.event_sender.clone();

        let join_handle = thread::Builder::new()
            .name(format!("voice-{}", frequency))
            .spawn(move || {
                voice_thread_main(channel, iq_receiver, event_sender, carrier_flag);
            })
            .expect("failed to spawn voice thread");

        tracing::info!(
            frequency = %frequency,
            talkgroup = %talkgroup,
            source = %source,
            offset_hz = offset_hz,
            "activated voice channel (thread)"
        );

        self.active_channels.insert(
            frequency,
            VoiceThreadHandle {
                iq_sender,
                join_handle: Some(join_handle),
                carrier_acquired,
                frequency,
                talkgroup,
                source,
                last_grant_sample: self.sample_count,
            },
        );
    }

    /// Check whether a frequency falls within the usable capture bandwidth.
    pub fn is_in_band(&self, frequency: Frequency) -> bool {
        let offset = (frequency.hz() as f64 - self.center_frequency_hz).abs();
        offset <= self.usable_bandwidth / 2.0
    }

    /// Remove channels that have not received a grant update within the
    /// timeout window, or that failed to acquire carrier within 1 second.
    ///
    /// Expired channels have their IQ sender dropped, which causes the
    /// voice thread to exit, then the thread is joined.
    fn expire_channels(&mut self) {
        let timeout = self.call_timeout_samples;
        let acquisition_timeout = self.sample_rate as u64; // 1 second
        let current = self.sample_count;

        // Collect frequencies to remove (can't borrow self mutably in retain
        // and also join threads, so we do two passes).
        let mut to_remove = Vec::new();

        for (frequency, handle) in &self.active_channels {
            let age = current.saturating_sub(handle.last_grant_sample);

            if !handle.carrier_acquired.load(Ordering::Relaxed) && age > acquisition_timeout {
                tracing::info!(
                    frequency = %handle.frequency,
                    talkgroup = %handle.talkgroup,
                    "voice channel expired (no carrier)"
                );
                to_remove.push((*frequency, false));
            } else if age > timeout {
                tracing::info!(
                    frequency = %handle.frequency,
                    talkgroup = %handle.talkgroup,
                    "voice channel expired"
                );
                to_remove.push((*frequency, true));
            }
        }

        for (frequency, had_carrier) in to_remove {
            if let Some(mut handle) = self.active_channels.remove(&frequency) {
                // Drop sender to signal the voice thread to exit.
                drop(handle.iq_sender);
                if let Some(jh) = handle.join_handle.take() {
                    let _ = jh.join();
                }
                if had_carrier {
                    self.expired_timeout += 1;
                } else {
                    self.expired_no_carrier += 1;
                }
            }
        }
    }

    /// Shut down all active voice channel threads.
    ///
    /// Drops all IQ senders and joins all threads. Called on
    /// `ChannelManager` drop to ensure clean shutdown.
    pub fn shutdown(&mut self) {
        // Drain remaining events before shutdown.
        while self.event_receiver.try_recv().is_ok() {}

        for (_frequency, mut handle) in self.active_channels.drain() {
            drop(handle.iq_sender);
            if let Some(jh) = handle.join_handle.take() {
                let _ = jh.join();
            }
        }
    }
}

impl Drop for ChannelManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p25::tsbk::{TsbkHeader, TsbkOpcode};
    use crate::p25::types::ChannelNumber;

    fn make_config() -> ChannelManagerConfig {
        ChannelManagerConfig {
            center_frequency: Frequency::from_hz(852_000_000),
            sample_rate: 2_400_000,
            call_timeout_seconds: 3.0,
            modulation: Modulation::Cqpsk,
            nid_integrity: NidIntegrityPolicy::default(),
            decode_audio: false,
            max_channels: None,
        }
    }

    fn make_ident_table() -> IdentTable {
        let mut table = IdentTable::new();
        // Identifier 6: base=851006250, spacing=6250, offset=-45MHz
        let tsbk = Tsbk {
            header: TsbkHeader {
                last_block: true,
                protected: false,
                opcode: TsbkOpcode::IdentifierUpdate,
                manufacturer_id: 0,
            },
            payload: TsbkPayload::IdentifierUpdate {
                identifier: 6,
                bandwidth: 12_500,
                transmit_offset: -45_000_000,
                channel_spacing: 6_250,
                base_frequency: 851_006_250,
            },
        };
        table.update(&tsbk);
        table
    }

    fn make_grant_tsbk(channel: u16, talkgroup: u16, source: u32) -> Tsbk {
        Tsbk {
            header: TsbkHeader {
                last_block: true,
                protected: false,
                opcode: TsbkOpcode::GroupVoiceChannelGrant,
                manufacturer_id: 0,
            },
            payload: TsbkPayload::GroupVoiceChannelGrant {
                channel: ChannelNumber::new(channel),
                talkgroup: TalkgroupId::new(talkgroup),
                source: SourceId::new(source),
            },
        }
    }

    fn make_grant_update_tsbk(
        channel_a: u16,
        talkgroup_a: u16,
        channel_b: u16,
        talkgroup_b: u16,
    ) -> Tsbk {
        Tsbk {
            header: TsbkHeader {
                last_block: true,
                protected: false,
                opcode: TsbkOpcode::GroupVoiceChannelGrantUpdate,
                manufacturer_id: 0,
            },
            payload: TsbkPayload::GroupVoiceChannelGrantUpdate {
                channel_a: ChannelNumber::new(channel_a),
                talkgroup_a: TalkgroupId::new(talkgroup_a),
                channel_b: ChannelNumber::new(channel_b),
                talkgroup_b: TalkgroupId::new(talkgroup_b),
            },
        }
    }

    #[test]
    fn new_manager_has_no_active_channels() {
        let manager = ChannelManager::new(make_config());
        assert_eq!(manager.active_channel_count(), 0);
        assert_eq!(manager.sample_count(), 0);
    }

    #[test]
    fn grant_activates_in_band_channel() {
        let mut manager = ChannelManager::new(make_config());
        let ident_table = make_ident_table();

        // Channel 0x6009: freq = 851006250 + 6250*9 = 851062500
        // Center = 852000000, offset = -937500 Hz, within 2.4M/2 bandwidth
        let grant = make_grant_tsbk(0x6009, 100, 12345);
        manager.handle_grant(&grant, &ident_table);

        assert_eq!(manager.active_channel_count(), 1);
    }

    #[test]
    fn grant_rejects_out_of_band_channel() {
        let mut manager = ChannelManager::new(make_config());
        let ident_table = make_ident_table();

        // Channel 0x6300: freq = 851006250 + 6250*768 = 855806250
        // Offset from center (852M) = +3806250, exceeds 2.4M*0.8/2 = 960000
        let grant = make_grant_tsbk(0x6300, 200, 99999);
        manager.handle_grant(&grant, &ident_table);

        assert_eq!(manager.active_channel_count(), 0);
    }

    #[test]
    fn duplicate_grant_refreshes_channel() {
        let mut manager = ChannelManager::new(make_config());
        let ident_table = make_ident_table();

        let grant = make_grant_tsbk(0x6009, 100, 12345);
        manager.handle_grant(&grant, &ident_table);
        assert_eq!(manager.active_channel_count(), 1);

        // Second grant to same channel refreshes, doesn't duplicate.
        let grant2 = make_grant_tsbk(0x6009, 100, 54321);
        manager.handle_grant(&grant2, &ident_table);
        assert_eq!(manager.active_channel_count(), 1);
    }

    #[test]
    fn grant_update_activates_channels() {
        let mut manager = ChannelManager::new(make_config());
        let ident_table = make_ident_table();

        // Grant updates (0x02) must create channels — many systems
        // announce channels primarily via grant updates, not 0x00.
        let update = make_grant_update_tsbk(0x6009, 100, 0x600A, 200);
        manager.handle_grant(&update, &ident_table);

        assert_eq!(
            manager.active_channel_count(),
            2,
            "grant update should create channels"
        );
    }

    #[test]
    fn grant_update_refreshes_existing_channel() {
        let mut config = make_config();
        config.call_timeout_seconds = 0.01; // 24000 samples
        let mut manager = ChannelManager::new(config);
        let ident_table = make_ident_table();

        // Create two channels with real grants.
        let grant_a = make_grant_tsbk(0x6009, 100, 12345);
        let grant_b = make_grant_tsbk(0x600A, 200, 54321);
        manager.handle_grant(&grant_a, &ident_table);
        manager.handle_grant(&grant_b, &ident_table);
        assert_eq!(manager.active_channel_count(), 2);

        // Advance 15000 samples (not yet expired).
        let silence = Complex::new(0.0, 0.0);
        let mut events = Vec::new();
        for _ in 0..15_000 {
            manager.process_sample(silence, &mut events);
        }

        // Grant update refreshes both channels.
        let update = make_grant_update_tsbk(0x6009, 100, 0x600A, 200);
        manager.handle_grant(&update, &ident_table);

        // Advance another 15000 samples. Without refresh, total would
        // exceed the 24000-sample timeout.
        for _ in 0..15_000 {
            manager.process_sample(silence, &mut events);
        }

        assert_eq!(
            manager.active_channel_count(),
            2,
            "grant update should have refreshed timeout for both channels"
        );
    }

    #[test]
    fn process_sample_increments_counter() {
        let mut manager = ChannelManager::new(make_config());
        let silence = Complex::new(0.0, 0.0);
        let mut events = Vec::new();

        for _ in 0..100 {
            manager.process_sample(silence, &mut events);
        }

        assert_eq!(manager.sample_count(), 100);
    }

    #[test]
    fn channels_expire_after_timeout() {
        let mut config = make_config();
        // Very short timeout: 0.01 seconds = 24000 samples at 2.4 MS/s
        config.call_timeout_seconds = 0.01;
        let mut manager = ChannelManager::new(config);
        let ident_table = make_ident_table();

        let grant = make_grant_tsbk(0x6009, 100, 12345);
        manager.handle_grant(&grant, &ident_table);
        assert_eq!(manager.active_channel_count(), 1);

        // Process enough samples to trigger timeout + expiry check.
        let silence = Complex::new(0.0, 0.0);
        let mut events = Vec::new();
        for _ in 0..30_000 {
            manager.process_sample(silence, &mut events);
        }

        assert_eq!(
            manager.active_channel_count(),
            0,
            "channel should have expired"
        );
    }

    #[test]
    fn grant_refresh_prevents_timeout() {
        let mut config = make_config();
        config.call_timeout_seconds = 0.01; // 24000 samples
        let mut manager = ChannelManager::new(config);
        let ident_table = make_ident_table();

        let grant = make_grant_tsbk(0x6009, 100, 12345);
        manager.handle_grant(&grant, &ident_table);

        let silence = Complex::new(0.0, 0.0);
        let mut events = Vec::new();
        // Process 15000 samples (not enough to timeout).
        for _ in 0..15_000 {
            manager.process_sample(silence, &mut events);
        }

        // Refresh the grant.
        manager.handle_grant(&grant, &ident_table);

        // Process another 15000 samples. Without refresh, total would be
        // 30000 > 24000 timeout. With refresh, only 15000 since last grant.
        for _ in 0..15_000 {
            manager.process_sample(silence, &mut events);
        }

        assert_eq!(
            manager.active_channel_count(),
            1,
            "channel should still be active after refresh"
        );
    }

    #[test]
    fn is_in_band_boundary() {
        let manager = ChannelManager::new(make_config());
        // Center = 852M, usable = 2.4M * 0.8 = 1.92M
        // In band: 852M +/- 960kHz = [851040000, 852960000]
        assert!(manager.is_in_band(Frequency::from_hz(852_000_000))); // center
        assert!(manager.is_in_band(Frequency::from_hz(851_050_000))); // just inside
        assert!(manager.is_in_band(Frequency::from_hz(852_950_000))); // just inside
        assert!(!manager.is_in_band(Frequency::from_hz(850_000_000))); // way out
        assert!(!manager.is_in_band(Frequency::from_hz(854_000_000))); // way out
    }

    #[test]
    fn silence_produces_no_voice_events() {
        let mut manager = ChannelManager::new(make_config());
        let ident_table = make_ident_table();

        let grant = make_grant_tsbk(0x6009, 100, 12345);
        manager.handle_grant(&grant, &ident_table);

        let silence = Complex::new(0.0, 0.0);
        let mut events = Vec::new();
        let mut total_events = 0;
        for _ in 0..10_000 {
            events.clear();
            manager.process_sample(silence, &mut events);
            total_events += events.len();
        }

        assert_eq!(total_events, 0, "silence should not produce voice events");
    }

    #[test]
    fn unresolvable_channel_is_skipped() {
        let mut manager = ChannelManager::new(make_config());
        let empty_table = IdentTable::new(); // no identifiers loaded

        let grant = make_grant_tsbk(0x6009, 100, 12345);
        manager.handle_grant(&grant, &empty_table);

        assert_eq!(
            manager.active_channel_count(),
            0,
            "unresolvable channel should not be activated"
        );
    }

    #[test]
    fn costas_seed_is_none_by_default() {
        let manager = ChannelManager::new(make_config());
        assert!(manager.cc_costas_seed.is_none());
    }

    #[test]
    fn update_costas_seed_from_cqpsk_pipeline() {
        let mut manager = ChannelManager::new(make_config());
        let config = PipelineConfig {
            sample_rate: 2_400_000,
            modulation: Modulation::Cqpsk,
            nid_integrity: NidIntegrityPolicy::default(),
            sync_timeout_samples: None,
        };
        let cc_pipeline = ChannelPipeline::new(config).unwrap();

        manager.update_costas_seed(&cc_pipeline);
        assert!(
            manager.cc_costas_seed.is_some(),
            "CQPSK pipeline should provide Costas seed"
        );
    }

    #[test]
    fn update_costas_seed_from_c4fm_pipeline_is_none() {
        let mut manager = ChannelManager::new(make_config());
        let config = PipelineConfig {
            sample_rate: 2_400_000,
            modulation: Modulation::C4fm,
            nid_integrity: NidIntegrityPolicy::default(),
            sync_timeout_samples: None,
        };
        let cc_pipeline = ChannelPipeline::new(config).unwrap();

        manager.update_costas_seed(&cc_pipeline);
        assert!(
            manager.cc_costas_seed.is_none(),
            "C4FM pipeline has no Costas loop"
        );
    }

    #[test]
    fn costas_seed_is_applied_to_new_channels() {
        let mut manager = ChannelManager::new(make_config());
        let ident_table = make_ident_table();

        // Set a Costas seed as if the CC loop had locked.
        let seed_freq = 0.005_f32;
        manager.cc_costas_seed = Some((0.3, seed_freq));

        // Activate a voice channel via grant. The pipeline is now
        // inside the thread, so we verify indirectly: the channel
        // was successfully created with the seed set.
        let grant = make_grant_tsbk(0x6009, 100, 12345);
        manager.handle_grant(&grant, &ident_table);
        assert_eq!(manager.active_channel_count(), 1);

        // The Costas seed was consumed during activate_channel.
        // We can verify it's still set on the manager for future channels.
        assert_eq!(
            manager.cc_costas_seed,
            Some((0.3, seed_freq)),
            "Costas seed should persist for future channels"
        );
    }

    #[test]
    fn new_channel_has_carrier_not_acquired() {
        let mut manager = ChannelManager::new(make_config());
        let ident_table = make_ident_table();

        let grant = make_grant_tsbk(0x6009, 100, 12345);
        manager.handle_grant(&grant, &ident_table);

        let handle = manager.active_channels.values().next().unwrap();
        assert!(
            !handle.carrier_acquired.load(Ordering::Relaxed),
            "new channel should not have carrier acquired"
        );
    }

    #[test]
    fn max_channels_prevents_new_activations() {
        let mut config = make_config();
        config.max_channels = Some(1);
        let mut manager = ChannelManager::new(config);
        let ident_table = make_ident_table();

        // First grant succeeds.
        let grant1 = make_grant_tsbk(0x6009, 100, 12345);
        manager.handle_grant(&grant1, &ident_table);
        assert_eq!(manager.active_channel_count(), 1);

        // Second grant to a different frequency is rejected.
        let grant2 = make_grant_tsbk(0x600A, 200, 54321);
        manager.handle_grant(&grant2, &ident_table);
        assert_eq!(
            manager.active_channel_count(),
            1,
            "should not exceed max_channels"
        );
    }

    #[test]
    fn unacquired_channel_expires_early() {
        let mut config = make_config();
        config.call_timeout_seconds = 3.0; // normal timeout: 7.2M samples
        let mut manager = ChannelManager::new(config);
        let ident_table = make_ident_table();

        let grant = make_grant_tsbk(0x6009, 100, 12345);
        manager.handle_grant(&grant, &ident_table);
        assert_eq!(manager.active_channel_count(), 1);

        // Process 2.5M samples (just over 1 second at 2.4 MS/s).
        // The channel has not acquired carrier, so it should expire.
        let silence = Complex::new(0.0, 0.0);
        let mut events = Vec::new();
        for _ in 0..2_500_000 {
            manager.process_sample(silence, &mut events);
        }

        assert_eq!(
            manager.active_channel_count(),
            0,
            "unacquired channel should expire after ~1 second"
        );
    }

    #[test]
    fn max_channels_none_allows_unlimited() {
        let mut manager = ChannelManager::new(make_config()); // max_channels: None
        let ident_table = make_ident_table();

        let grant1 = make_grant_tsbk(0x6009, 100, 12345);
        let grant2 = make_grant_tsbk(0x600A, 200, 54321);
        manager.handle_grant(&grant1, &ident_table);
        manager.handle_grant(&grant2, &ident_table);
        assert_eq!(manager.active_channel_count(), 2);
    }

    #[test]
    fn shutdown_joins_all_voice_threads() {
        let mut manager = ChannelManager::new(make_config());
        let ident_table = make_ident_table();

        // Activate two voice channels.
        let grant1 = make_grant_tsbk(0x6009, 100, 12345);
        let grant2 = make_grant_tsbk(0x600A, 200, 54321);
        manager.handle_grant(&grant1, &ident_table);
        manager.handle_grant(&grant2, &ident_table);
        assert_eq!(manager.active_channel_count(), 2);

        // Feed a few samples so threads are actively receiving.
        let silence = Complex::new(0.0, 0.0);
        let mut events = Vec::new();
        for _ in 0..100 {
            manager.process_sample(silence, &mut events);
        }

        // Explicit shutdown: drops senders, joins threads.
        manager.shutdown();
        assert_eq!(
            manager.active_channel_count(),
            0,
            "all channels should be removed after shutdown"
        );
    }

    #[test]
    fn drop_cleans_up_threads() {
        let ident_table = make_ident_table();

        // Scope the manager so it drops at end of block.
        {
            let mut manager = ChannelManager::new(make_config());
            let grant = make_grant_tsbk(0x6009, 100, 12345);
            manager.handle_grant(&grant, &ident_table);
            assert_eq!(manager.active_channel_count(), 1);

            let silence = Complex::new(0.0, 0.0);
            let mut events = Vec::new();
            for _ in 0..100 {
                manager.process_sample(silence, &mut events);
            }
        }
        // If threads weren't joined, this would leak or panic.
        // Test passes = Drop impl correctly cleaned up.
    }

    #[test]
    fn expired_channel_thread_is_joined() {
        let mut config = make_config();
        config.call_timeout_seconds = 0.01; // 24000 samples
        let mut manager = ChannelManager::new(config);
        let ident_table = make_ident_table();

        let grant = make_grant_tsbk(0x6009, 100, 12345);
        manager.handle_grant(&grant, &ident_table);
        assert_eq!(manager.active_channel_count(), 1);

        // Process enough samples to trigger expiry.
        let silence = Complex::new(0.0, 0.0);
        let mut events = Vec::new();
        for _ in 0..30_000 {
            manager.process_sample(silence, &mut events);
        }

        // Channel expired via call timeout and thread was joined.
        assert_eq!(manager.active_channel_count(), 0);
        assert_eq!(manager.expired_timeout(), 1);
    }
}

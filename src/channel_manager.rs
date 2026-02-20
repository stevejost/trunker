//! Voice channel lifecycle management for wideband trunking.
//!
//! Watches control channel grant events, spawns per-channel decode
//! pipelines (NCO + decimation + demod), and tears them down after
//! grant updates stop. Each active voice channel gets its own
//! [`ChannelPipeline`] fed by an NCO-shifted copy of the wideband IQ stream.

use std::collections::HashMap;

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
    /// Sample counter at last grant update (for timeout).
    last_grant_sample: u64,
    /// Whether the Costas loop has acquired carrier lock.
    ///
    /// Set to `true` after the first NID with valid parity is decoded.
    /// Audio output is suppressed until acquisition to avoid digital
    /// noise artifacts from corrupted IMBE frames.
    carrier_acquired: bool,
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
    /// Active voice channels keyed by receive frequency.
    active_channels: HashMap<Frequency, VoiceChannel>,
    /// Pipeline configuration for new voice channels.
    pipeline_config: PipelineConfig,
    /// Whether to decode IMBE voice frames into audio.
    decode_audio: bool,
    /// Global sample counter.
    sample_count: u64,
    /// Costas loop state from the CC pipeline, used to seed new voice
    /// channel pipelines for faster carrier acquisition.
    cc_costas_seed: Option<(f32, f32)>,
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
            },
            decode_audio: config.decode_audio,
            sample_count: 0,
            cc_costas_seed: None,
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
                // Grant updates don't carry a source; use a zero source
                // to refresh the channel. If the channel already exists,
                // the talkgroup and last_grant timestamp are updated.
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

    /// Feed one wideband IQ sample to all active voice channels.
    ///
    /// Returns events from any voice channels that produced decoded data.
    /// Also expires timed-out channels.
    pub fn process_sample(&mut self, sample: Complex<f32>) -> Vec<VoiceChannelEvent> {
        self.sample_count += 1;
        let mut events = Vec::new();

        for channel in self.active_channels.values_mut() {
            let shifted = channel.nco.shift(sample);
            if let Some(event) = channel.pipeline.process_sample(shifted) {
                // Mark carrier as acquired on first valid NID decode.
                if let ReceiverEvent::Nid(nid) = &event
                    && nid.parity_ok
                    && !channel.carrier_acquired
                {
                    channel.carrier_acquired = true;
                    tracing::debug!(
                        frequency = %channel.frequency,
                        "carrier acquired, audio output enabled"
                    );
                }

                // Suppress audio until carrier is acquired to avoid
                // digital noise from corrupted IMBE frames during
                // Costas loop convergence.
                let audio = if channel.carrier_acquired {
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

                events.push(VoiceChannelEvent {
                    frequency: channel.frequency,
                    talkgroup: channel.talkgroup,
                    source: channel.source,
                    nac: channel.pipeline.current_nac(),
                    event,
                    audio,
                });
            }
        }

        // Expire timed-out channels periodically (every 10k samples to
        // avoid HashMap iteration overhead on every sample).
        if self.sample_count.is_multiple_of(10_000) {
            self.expire_channels();
        }

        events
    }

    /// Return the number of currently active voice channels.
    pub fn active_channel_count(&self) -> usize {
        self.active_channels.len()
    }

    /// Return the total number of wideband samples processed.
    pub fn sample_count(&self) -> u64 {
        self.sample_count
    }

    /// Update the Costas loop seed from the control channel pipeline.
    ///
    /// Call this periodically (e.g., on each CC pipeline event) so that
    /// newly spawned voice channels start with the CC's locked Costas
    /// state, reducing acquisition from ~22 frames to ~2-5 frames.
    pub fn update_costas_seed(&mut self, cc_pipeline: &ChannelPipeline) {
        self.cc_costas_seed = cc_pipeline.costas_state();
    }

    /// Activate or refresh a voice channel at the given frequency.
    fn activate_channel(&mut self, frequency: Frequency, talkgroup: TalkgroupId, source: SourceId) {
        if !self.is_in_band(frequency.hz()) {
            tracing::debug!(
                frequency = %frequency,
                "grant frequency out of capture bandwidth, skipping"
            );
            return;
        }

        if let Some(existing) = self.active_channels.get_mut(&frequency) {
            // Refresh existing channel: update talkgroup, source (if
            // non-zero), and timeout.
            existing.talkgroup = talkgroup;
            if source.value() != 0 {
                existing.source = source;
            }
            existing.last_grant_sample = self.sample_count;
            tracing::debug!(
                frequency = %frequency,
                talkgroup = %talkgroup,
                "refreshed voice channel"
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
        // The CC's frequency estimate reflects the SDR's LO error, which
        // is the same for all channels. Phase is NOT transferred because
        // the differential decoder makes absolute phase non-transferable
        // between independent signal paths.
        if let Some((_phase, freq)) = self.cc_costas_seed {
            pipeline.seed_costas(0.0, freq);
            tracing::debug!(
                frequency = %frequency,
                seed_freq = freq,
                "seeded voice channel Costas frequency from CC"
            );
        }

        tracing::info!(
            frequency = %frequency,
            talkgroup = %talkgroup,
            source = %source,
            offset_hz = offset_hz,
            "activated voice channel"
        );

        let decoder = if self.decode_audio {
            Some(ImbeDecoder::new())
        } else {
            None
        };

        self.active_channels.insert(
            frequency,
            VoiceChannel {
                frequency,
                talkgroup,
                source,
                nco,
                pipeline,
                decoder,
                last_grant_sample: self.sample_count,
                carrier_acquired: false,
            },
        );
    }

    /// Check whether a frequency falls within the usable capture bandwidth.
    fn is_in_band(&self, frequency_hz: u64) -> bool {
        let offset = (frequency_hz as f64 - self.center_frequency_hz).abs();
        offset <= self.usable_bandwidth / 2.0
    }

    /// Remove channels that have not received a grant update within the
    /// timeout window.
    fn expire_channels(&mut self) {
        let timeout = self.call_timeout_samples;
        let current = self.sample_count;

        self.active_channels.retain(|_frequency, channel| {
            let age = current.saturating_sub(channel.last_grant_sample);
            if age > timeout {
                tracing::info!(
                    frequency = %channel.frequency,
                    talkgroup = %channel.talkgroup,
                    "voice channel expired"
                );
                false
            } else {
                true
            }
        });
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
    fn grant_update_activates_two_channels() {
        let mut manager = ChannelManager::new(make_config());
        let ident_table = make_ident_table();

        // Two channels near center frequency
        // 0x6009: 851062500 (offset -937500)
        // 0x600A: 851068750 (offset -931250)
        let update = make_grant_update_tsbk(0x6009, 100, 0x600A, 200);
        manager.handle_grant(&update, &ident_table);

        assert_eq!(manager.active_channel_count(), 2);
    }

    #[test]
    fn process_sample_increments_counter() {
        let mut manager = ChannelManager::new(make_config());
        let silence = Complex::new(0.0, 0.0);

        for _ in 0..100 {
            manager.process_sample(silence);
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
        for _ in 0..30_000 {
            manager.process_sample(silence);
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
        // Process 15000 samples (not enough to timeout).
        for _ in 0..15_000 {
            manager.process_sample(silence);
        }

        // Refresh the grant.
        manager.handle_grant(&grant, &ident_table);

        // Process another 15000 samples. Without refresh, total would be
        // 30000 > 24000 timeout. With refresh, only 15000 since last grant.
        for _ in 0..15_000 {
            manager.process_sample(silence);
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
        assert!(manager.is_in_band(852_000_000)); // center
        assert!(manager.is_in_band(851_050_000)); // just inside
        assert!(manager.is_in_band(852_950_000)); // just inside
        assert!(!manager.is_in_band(850_000_000)); // way out
        assert!(!manager.is_in_band(854_000_000)); // way out
    }

    #[test]
    fn silence_produces_no_voice_events() {
        let mut manager = ChannelManager::new(make_config());
        let ident_table = make_ident_table();

        let grant = make_grant_tsbk(0x6009, 100, 12345);
        manager.handle_grant(&grant, &ident_table);

        let silence = Complex::new(0.0, 0.0);
        let mut total_events = 0;
        for _ in 0..10_000 {
            total_events += manager.process_sample(silence).len();
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
        };
        let cc_pipeline = ChannelPipeline::new(config).unwrap();

        manager.update_costas_seed(&cc_pipeline);
        assert!(
            manager.cc_costas_seed.is_none(),
            "C4FM pipeline has no Costas loop"
        );
    }

    #[test]
    fn activated_channel_receives_costas_frequency_seed() {
        let mut manager = ChannelManager::new(make_config());
        let ident_table = make_ident_table();

        // Manually set a Costas seed as if the CC loop had locked.
        let seed_freq = 0.005_f32;
        manager.cc_costas_seed = Some((0.3, seed_freq));

        // Activate a voice channel via grant.
        let grant = make_grant_tsbk(0x6009, 100, 12345);
        manager.handle_grant(&grant, &ident_table);
        assert_eq!(manager.active_channel_count(), 1);

        // The activated pipeline should have been seeded with frequency
        // only (phase=0.0 per RF expert recommendation).
        let channel = manager.active_channels.values().next().unwrap();
        let costas_state = channel.pipeline.costas_state();
        let (phase, freq) = costas_state.expect("CQPSK pipeline should have Costas state");

        // Phase should be near zero (not the CC's phase).
        assert!(
            phase.abs() < 0.01,
            "seeded phase should be ~0.0, got {phase}"
        );
        // Frequency should match the seed.
        assert!(
            (freq - seed_freq).abs() < 1e-6,
            "seeded frequency should be {seed_freq}, got {freq}"
        );
    }

    #[test]
    fn new_channel_has_carrier_not_acquired() {
        let mut manager = ChannelManager::new(make_config());
        let ident_table = make_ident_table();

        let grant = make_grant_tsbk(0x6009, 100, 12345);
        manager.handle_grant(&grant, &ident_table);

        let channel = manager.active_channels.values().next().unwrap();
        assert!(
            !channel.carrier_acquired,
            "new channel should not have carrier acquired"
        );
    }
}

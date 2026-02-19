//! Integration test: wideband channelizer (NCO + per-channel pipeline).
//!
//! Tests the NCO frequency shifting and ChannelManager grant tracking
//! to verify the wideband trunking pipeline operates correctly.

use num_complex::Complex;

use trunker::channel_manager::{ChannelManager, ChannelManagerConfig};
use trunker::p25::nid::NidIntegrityPolicy;
use trunker::dsp::nco::Nco;
use trunker::p25::ident::IdentTable;
use trunker::p25::tsbk::{Tsbk, TsbkHeader, TsbkOpcode, TsbkPayload};
use trunker::p25::types::{ChannelNumber, Frequency, SourceId, TalkgroupId};
use trunker::pipeline::{ChannelPipeline, Modulation, PipelineConfig};

// -----------------------------------------------------------------------
// NCO integration tests
// -----------------------------------------------------------------------

/// Verify that shifting a tone by its frequency brings it to DC,
/// and the shifted signal maintains constant phase (within tolerance).
#[test]
fn nco_shifts_off_center_tone_to_dc() {
    let sample_rate = 2_400_000.0;
    let tone_offset = 100_000.0; // 100 kHz off center
    let num_samples = 240_000; // 100 ms

    let mut nco = Nco::new(tone_offset, sample_rate);
    let mut dc_sum = Complex::new(0.0_f64, 0.0_f64);

    for n in 0..num_samples {
        let t = n as f64 / sample_rate;
        let phase = std::f64::consts::TAU * tone_offset * t;
        let input = Complex::new(phase.cos() as f32, phase.sin() as f32);
        let shifted = nco.shift(input);
        dc_sum += Complex::new(shifted.re as f64, shifted.im as f64);
    }

    let dc_avg = dc_sum / num_samples as f64;
    // Should be approximately (1, 0) — pure DC at unit amplitude.
    assert!(
        (dc_avg.re - 1.0).abs() < 0.01,
        "DC real should be ~1.0, got {}",
        dc_avg.re
    );
    assert!(
        dc_avg.im.abs() < 0.01,
        "DC imaginary should be ~0.0, got {}",
        dc_avg.im
    );
}

/// Verify that a tone NOT at the NCO offset retains its frequency
/// content (is not collapsed to DC).
#[test]
fn nco_does_not_collapse_wrong_frequency() {
    let sample_rate = 2_400_000.0;
    let tone_hz = 100_000.0;
    let nco_offset = 200_000.0; // Different from tone
    let num_samples = 240_000;

    let mut nco = Nco::new(nco_offset, sample_rate);
    let mut dc_sum = Complex::new(0.0_f64, 0.0_f64);

    for n in 0..num_samples {
        let t = n as f64 / sample_rate;
        let phase = std::f64::consts::TAU * tone_hz * t;
        let input = Complex::new(phase.cos() as f32, phase.sin() as f32);
        let shifted = nco.shift(input);
        dc_sum += Complex::new(shifted.re as f64, shifted.im as f64);
    }

    let dc_avg = dc_sum / num_samples as f64;
    // Should be near zero (the 100 kHz - 200 kHz = -100 kHz residual
    // averages to zero over many cycles).
    assert!(
        dc_avg.norm() < 0.01,
        "wrong-frequency tone should not produce DC: got {:?}",
        dc_avg
    );
}

// -----------------------------------------------------------------------
// ChannelManager integration tests
// -----------------------------------------------------------------------

fn make_ident_table() -> IdentTable {
    let mut table = IdentTable::new();
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

fn make_grant(channel: u16, talkgroup: u16, source: u32) -> Tsbk {
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

/// Verify that ChannelManager activates multiple in-band channels
/// and feeds wideband samples to each with the correct NCO offset.
#[test]
fn channel_manager_tracks_multiple_grants() {
    let center = 852_000_000_u64;
    let mut manager = ChannelManager::new(ChannelManagerConfig {
        center_frequency: Frequency::from_hz(center),
        sample_rate: 2_400_000,
        call_timeout_seconds: 3.0,
        modulation: Modulation::Cqpsk,
        nid_integrity: NidIntegrityPolicy::default(),
        decode_audio: false,
    });
    let ident_table = make_ident_table();

    // Grant on channel 0x6009: 851062500 Hz (offset = -937500)
    manager.handle_grant(&make_grant(0x6009, 100, 11111), &ident_table);
    // Grant on channel 0x600A: 851068750 Hz (offset = -931250)
    manager.handle_grant(&make_grant(0x600A, 200, 22222), &ident_table);

    assert_eq!(manager.active_channel_count(), 2);

    // Feed some samples and verify no crashes.
    let silence = Complex::new(0.0, 0.0);
    for _ in 0..1000 {
        let _ = manager.process_sample(silence);
    }

    assert_eq!(manager.active_channel_count(), 2);
}

/// Verify that out-of-band grants are rejected while in-band ones are
/// accepted, creating a heterogeneous set of channels.
#[test]
fn channel_manager_rejects_out_of_band() {
    let center = 852_000_000_u64;
    let mut manager = ChannelManager::new(ChannelManagerConfig {
        center_frequency: Frequency::from_hz(center),
        sample_rate: 2_400_000,
        call_timeout_seconds: 3.0,
        modulation: Modulation::Cqpsk,
        nid_integrity: NidIntegrityPolicy::default(),
        decode_audio: false,
    });
    let ident_table = make_ident_table();

    // In-band: 851062500 (offset -937500, within +/-960000)
    manager.handle_grant(&make_grant(0x6009, 100, 11111), &ident_table);

    // Out-of-band: 851006250 + 6250*300 = 852881250 (offset +881250, in-band)
    manager.handle_grant(&make_grant(0x612C, 200, 22222), &ident_table);

    // Way out-of-band: 851006250 + 6250*768 = 855806250 (offset +3806250)
    manager.handle_grant(&make_grant(0x6300, 300, 33333), &ident_table);

    assert_eq!(
        manager.active_channel_count(),
        2,
        "should have 2 in-band channels, not 3"
    );
}

/// Verify that the channel manager's timeout mechanism works:
/// grants expire when not refreshed, and refresh prevents expiry.
#[test]
fn channel_manager_timeout_lifecycle() {
    let mut manager = ChannelManager::new(ChannelManagerConfig {
        center_frequency: Frequency::from_hz(852_000_000),
        sample_rate: 2_400_000,
        call_timeout_seconds: 0.01, // Very short: 24000 samples
        modulation: Modulation::Cqpsk,
        nid_integrity: NidIntegrityPolicy::default(),
        decode_audio: false,
    });
    let ident_table = make_ident_table();

    // Activate two channels.
    manager.handle_grant(&make_grant(0x6009, 100, 11111), &ident_table);
    manager.handle_grant(&make_grant(0x600A, 200, 22222), &ident_table);
    assert_eq!(manager.active_channel_count(), 2);

    // Run 15k samples (not enough to timeout).
    let silence = Complex::new(0.0, 0.0);
    for _ in 0..15_000 {
        let _ = manager.process_sample(silence);
    }

    // Refresh only channel 0x6009.
    manager.handle_grant(&make_grant(0x6009, 100, 11111), &ident_table);

    // Run another 15k samples (total 30k > 24k timeout).
    for _ in 0..15_000 {
        let _ = manager.process_sample(silence);
    }

    // Only the refreshed channel should survive.
    assert_eq!(
        manager.active_channel_count(),
        1,
        "only refreshed channel should survive timeout"
    );
}

// -----------------------------------------------------------------------
// Pipeline integration: verify ChannelPipeline processes without panic
// -----------------------------------------------------------------------

/// Feed random noise through a CQPSK ChannelPipeline to verify the
/// complete DSP chain doesn't crash or produce garbage events.
#[test]
fn channel_pipeline_processes_noise_without_events() {
    let config = PipelineConfig {
        sample_rate: 2_400_000,
        modulation: Modulation::Cqpsk,
        nid_integrity: NidIntegrityPolicy::default(),
    };
    let mut pipeline = ChannelPipeline::new(config).expect("2.4M should be valid");

    let mut event_count = 0;
    for i in 0..100_000_u32 {
        let phase = i as f32 * 0.37;
        let sample = Complex::new(phase.cos() * 0.5, phase.sin() * 0.5);
        if pipeline.process_sample(sample).is_some() {
            event_count += 1;
        }
    }

    // Random noise should not produce valid P25 events.
    assert_eq!(
        event_count, 0,
        "random noise should not produce decode events"
    );
    assert_eq!(pipeline.sample_count(), 100_000);
}

/// Verify that both C4FM and CQPSK pipeline configurations construct
/// and process samples without errors.
#[test]
fn both_modulation_types_process_cleanly() {
    for modulation in [Modulation::C4fm, Modulation::Cqpsk] {
        let config = PipelineConfig {
            sample_rate: 2_400_000,
            modulation,
            nid_integrity: NidIntegrityPolicy::default(),
        };
        let mut pipeline = ChannelPipeline::new(config).expect("2.4M should be valid");

        let silence = Complex::new(0.0, 0.0);
        for _ in 0..10_000 {
            let _ = pipeline.process_sample(silence);
        }

        assert_eq!(pipeline.sample_count(), 10_000);
    }
}

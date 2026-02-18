//! Tests for flexible sample rate decimation configuration.
//!
//! Verifies that `DecimationConfig` correctly computes decimation stages,
//! filter parameters, and rejects invalid sample rates. Includes a
//! regression test ensuring the 2.4 MS/s path matches the original
//! hardcoded values exactly.

use num_complex::Complex;

use trunker::pipeline::{
    ChannelPipeline, DecimationConfig, DecimationError, Modulation, PipelineConfig,
};

/// Target channel rate: always 24 kHz (5 samples/symbol at 4800 baud).
const CHANNEL_RATE: u32 = 24_000;

/// Helper: first stage of a multi-stage config.
fn first_stage(config: &DecimationConfig) -> &trunker::pipeline::DecimationStage {
    &config.stages[0]
}

/// Helper: last stage of a multi-stage config.
fn last_stage(config: &DecimationConfig) -> &trunker::pipeline::DecimationStage {
    config.stages.last().expect("stages should not be empty in tests")
}

// ---------------------------------------------------------------------------
// Regression: 2.4 MS/s must produce exact original hardcoded values
// ---------------------------------------------------------------------------

#[test]
fn regression_2400k_matches_original_constants() {
    let config = DecimationConfig::compute(2_400_000).expect("2.4M should be valid");

    assert_eq!(config.total_decimation(), 100, "total decimation");
    assert_eq!(first_stage(&config).decimation_factor, 10, "stage 1 decimation");
    assert_eq!(last_stage(&config).decimation_factor, 10, "stage 2 decimation");
    assert_eq!(first_stage(&config).num_taps, 201, "stage 1 taps (regression)");
    assert_eq!(last_stage(&config).num_taps, 61, "stage 2 taps (regression)");
    assert!(
        (first_stage(&config).cutoff_hz - 12_000.0).abs() < 0.01,
        "stage 1 cutoff: got {}",
        first_stage(&config).cutoff_hz
    );
    assert!(
        (last_stage(&config).cutoff_hz - 6_250.0).abs() < 0.01,
        "stage 2 cutoff: got {}",
        last_stage(&config).cutoff_hz
    );
}

// ---------------------------------------------------------------------------
// Valid sample rates: verify decimation stages are computed correctly
// ---------------------------------------------------------------------------

#[test]
fn valid_rate_2880k() {
    let config = DecimationConfig::compute(2_880_000).expect("2.88M should be valid");
    assert_eq!(config.total_decimation(), 120);
    assert_eq!(
        first_stage(&config).decimation_factor * last_stage(&config).decimation_factor,
        120
    );
    assert!(first_stage(&config).decimation_factor <= 25, "stage 1 factor too large");
    assert!(last_stage(&config).decimation_factor <= 25, "stage 2 factor too large");
}

#[test]
fn valid_rate_3000k() {
    let config = DecimationConfig::compute(3_000_000).expect("3M should be valid");
    assert_eq!(config.total_decimation(), 125);
    assert_eq!(
        first_stage(&config).decimation_factor * last_stage(&config).decimation_factor,
        125
    );
    assert!(first_stage(&config).decimation_factor <= 25);
    assert!(last_stage(&config).decimation_factor <= 25);
}

#[test]
fn valid_rate_4800k() {
    let config = DecimationConfig::compute(4_800_000).expect("4.8M should be valid");
    assert_eq!(config.total_decimation(), 200);
    assert_eq!(
        first_stage(&config).decimation_factor * last_stage(&config).decimation_factor,
        200
    );
    assert!(first_stage(&config).decimation_factor <= 25);
    assert!(last_stage(&config).decimation_factor <= 25);
}

#[test]
fn valid_rate_6000k() {
    let config = DecimationConfig::compute(6_000_000).expect("6M should be valid");
    assert_eq!(config.total_decimation(), 250);
    assert_eq!(
        first_stage(&config).decimation_factor * last_stage(&config).decimation_factor,
        250
    );
    assert!(first_stage(&config).decimation_factor <= 25);
    assert!(last_stage(&config).decimation_factor <= 25);
}

#[test]
fn valid_rate_9600k() {
    let config = DecimationConfig::compute(9_600_000).expect("9.6M should be valid");
    assert_eq!(config.total_decimation(), 400);
    assert_eq!(
        first_stage(&config).decimation_factor * last_stage(&config).decimation_factor,
        400
    );
    assert!(first_stage(&config).decimation_factor <= 25);
    assert!(last_stage(&config).decimation_factor <= 25);
}

// ---------------------------------------------------------------------------
// Property: all valid configs produce correct total decimation
// ---------------------------------------------------------------------------

#[test]
fn all_valid_rates_have_correct_total_decimation() {
    let valid_rates: Vec<u32> = vec![
        2_400_000, 2_880_000, 3_000_000, 3_360_000, 3_840_000, 4_320_000, 4_800_000, 5_760_000,
        6_000_000, 7_200_000, 9_600_000,
    ];

    for rate in valid_rates {
        let config = DecimationConfig::compute(rate)
            .unwrap_or_else(|e| panic!("{rate} should be valid: {e}"));

        let expected_total = rate / CHANNEL_RATE;
        assert_eq!(
            config.total_decimation(),
            expected_total as usize,
            "total decimation for {rate}"
        );

        assert_eq!(
            first_stage(&config).decimation_factor * last_stage(&config).decimation_factor,
            expected_total as usize,
            "stage product for {rate}"
        );
    }
}

// ---------------------------------------------------------------------------
// Property: filter parameters are sane for all valid configs
// ---------------------------------------------------------------------------

#[test]
fn filter_cutoffs_are_correct_for_all_valid_rates() {
    let valid_rates: Vec<u32> = vec![
        2_400_000, 2_880_000, 3_000_000, 4_800_000, 6_000_000, 9_600_000,
    ];

    for rate in valid_rates {
        let config = DecimationConfig::compute(rate).unwrap();

        // Stage 1 cutoff: always 12 kHz (protects P25 12.5 kHz channel).
        assert!(
            (first_stage(&config).cutoff_hz - 12_000.0).abs() < 0.01,
            "stage 1 cutoff for {rate}: got {}",
            first_stage(&config).cutoff_hz
        );

        // Last stage cutoff: always 6.25 kHz (half of 12.5 kHz channel BW).
        assert!(
            (last_stage(&config).cutoff_hz - 6_250.0).abs() < 0.01,
            "last stage cutoff for {rate}: got {}",
            last_stage(&config).cutoff_hz
        );
    }
}

#[test]
fn filter_taps_are_odd_for_all_valid_rates() {
    let valid_rates: Vec<u32> = vec![
        2_400_000, 2_880_000, 3_000_000, 4_800_000, 6_000_000, 9_600_000,
    ];

    for rate in valid_rates {
        let config = DecimationConfig::compute(rate).unwrap();

        assert!(
            first_stage(&config).num_taps % 2 == 1,
            "stage 1 taps for {rate} should be odd: got {}",
            first_stage(&config).num_taps
        );
        assert!(
            last_stage(&config).num_taps % 2 == 1,
            "last stage taps for {rate} should be odd: got {}",
            last_stage(&config).num_taps
        );
    }
}

#[test]
fn filter_taps_are_at_least_51() {
    let valid_rates: Vec<u32> = vec![
        2_400_000, 2_880_000, 3_000_000, 4_800_000, 6_000_000, 9_600_000,
    ];

    for rate in valid_rates {
        let config = DecimationConfig::compute(rate).unwrap();

        assert!(
            first_stage(&config).num_taps >= 51,
            "stage 1 taps for {rate} should be >= 51: got {}",
            first_stage(&config).num_taps
        );
        assert!(
            last_stage(&config).num_taps >= 51,
            "last stage taps for {rate} should be >= 51: got {}",
            last_stage(&config).num_taps
        );
    }
}

#[test]
fn higher_sample_rates_need_more_stage1_taps() {
    let low = DecimationConfig::compute(2_400_000).unwrap();
    let high = DecimationConfig::compute(9_600_000).unwrap();

    assert!(
        first_stage(&high).num_taps > first_stage(&low).num_taps,
        "9.6M should need more stage 1 taps than 2.4M: {} vs {}",
        first_stage(&high).num_taps,
        first_stage(&low).num_taps
    );
}

// ---------------------------------------------------------------------------
// Error cases: invalid sample rates are rejected
// ---------------------------------------------------------------------------

#[test]
fn rejects_rate_not_divisible_by_24k() {
    assert!(
        DecimationConfig::compute(2_000_000).is_err(),
        "2M is not a multiple of 24k"
    );
    assert!(
        DecimationConfig::compute(4_000_000).is_err(),
        "4M is not a multiple of 24k"
    );
    assert!(
        DecimationConfig::compute(8_000_000).is_err(),
        "8M is not a multiple of 24k"
    );
    assert!(
        DecimationConfig::compute(10_000_000).is_err(),
        "10M is not a multiple of 24k"
    );
    assert!(
        DecimationConfig::compute(3_200_000).is_err(),
        "3.2M is not a multiple of 24k"
    );
}

#[test]
fn rejects_zero_rate() {
    assert!(
        DecimationConfig::compute(0).is_err(),
        "zero sample rate should be rejected"
    );
}

#[test]
fn rejects_rate_below_channel_rate() {
    assert!(
        DecimationConfig::compute(12_000).is_err(),
        "12 kHz is below channel rate"
    );
}

#[test]
fn rejects_channel_rate_itself() {
    // 24000 / 24000 = 1, which means no decimation needed.
    // This would be a single-channel-rate input, which doesn't make sense
    // for an SDR capture. The implementation may accept or reject this;
    // if it accepts, total_decimation should be 1.
    let result = DecimationConfig::compute(24_000);
    if let Ok(config) = result {
        assert_eq!(config.total_decimation(), 1);
    }
    // Either accepting (total=1) or rejecting is fine.
}

#[test]
fn error_message_is_actionable() {
    let err = DecimationConfig::compute(2_000_000).unwrap_err();
    let msg = format!("{err}");
    // Error should mention that the rate is not a multiple of 24000.
    assert!(
        msg.contains("24") || msg.contains("divisible") || msg.contains("multiple"),
        "error message should explain the constraint: got '{msg}'"
    );
}

// ---------------------------------------------------------------------------
// Pipeline construction: verify pipelines build successfully with various rates
// ---------------------------------------------------------------------------

#[test]
fn pipeline_constructs_with_4800k_c4fm() {
    let config = PipelineConfig {
        sample_rate: 4_800_000,
        modulation: Modulation::C4fm,
    };
    let pipeline = ChannelPipeline::new(config).expect("4.8M should be valid");
    assert_eq!(pipeline.sample_count(), 0);
}

#[test]
fn pipeline_constructs_with_6000k_cqpsk() {
    let config = PipelineConfig {
        sample_rate: 6_000_000,
        modulation: Modulation::Cqpsk,
    };
    let pipeline = ChannelPipeline::new(config).expect("6M should be valid");
    assert_eq!(pipeline.sample_count(), 0);
}

#[test]
fn pipeline_constructs_with_2880k_cqpsk() {
    let config = PipelineConfig {
        sample_rate: 2_880_000,
        modulation: Modulation::Cqpsk,
    };
    let pipeline = ChannelPipeline::new(config).expect("2.88M should be valid");
    assert_eq!(pipeline.sample_count(), 0);
}

#[test]
fn pipeline_constructs_with_9600k_c4fm() {
    let config = PipelineConfig {
        sample_rate: 9_600_000,
        modulation: Modulation::C4fm,
    };
    let pipeline = ChannelPipeline::new(config).expect("9.6M should be valid");
    assert_eq!(pipeline.sample_count(), 0);
}

// ---------------------------------------------------------------------------
// Functional: non-2.4M pipelines process silence without crashing
// ---------------------------------------------------------------------------

#[test]
fn pipeline_4800k_processes_silence() {
    let config = PipelineConfig {
        sample_rate: 4_800_000,
        modulation: Modulation::Cqpsk,
    };
    let mut pipeline = ChannelPipeline::new(config).expect("4.8M should be valid");
    let silence = Complex::new(0.0, 0.0);

    let mut event_count = 0;
    for _ in 0..20_000 {
        if pipeline.process_sample(silence).is_some() {
            event_count += 1;
        }
    }

    assert_eq!(event_count, 0, "silence should not produce events");
    assert_eq!(pipeline.sample_count(), 20_000);
}

#[test]
fn pipeline_6000k_processes_noise_without_panic() {
    let config = PipelineConfig {
        sample_rate: 6_000_000,
        modulation: Modulation::C4fm,
    };
    let mut pipeline = ChannelPipeline::new(config).expect("6M should be valid");

    for i in 0..50_000u32 {
        let phase = i as f32 * 0.73;
        let sample = Complex::new(phase.cos() * 0.1, phase.sin() * 0.1);
        let _ = pipeline.process_sample(sample);
    }

    assert_eq!(pipeline.sample_count(), 50_000);
}

// ---------------------------------------------------------------------------
// Regression: 2.4M pipeline still produces zero events from silence
// (same behavior as before the refactor)
// ---------------------------------------------------------------------------

#[test]
fn regression_2400k_pipeline_silence() {
    let config = PipelineConfig {
        sample_rate: 2_400_000,
        modulation: Modulation::Cqpsk,
    };
    let mut pipeline = ChannelPipeline::new(config).expect("2.4M should be valid");
    let silence = Complex::new(0.0, 0.0);

    let mut event_count = 0;
    for _ in 0..10_000 {
        if pipeline.process_sample(silence).is_some() {
            event_count += 1;
        }
    }

    assert_eq!(event_count, 0, "silence should not produce events");
    assert_eq!(pipeline.sample_count(), 10_000);
}

// ---------------------------------------------------------------------------
// Pipeline constructor error propagation
// ---------------------------------------------------------------------------

#[test]
fn pipeline_rejects_invalid_sample_rate() {
    let config = PipelineConfig {
        sample_rate: 2_000_000,
        modulation: Modulation::Cqpsk,
    };
    assert!(
        ChannelPipeline::new(config).is_err(),
        "pipeline should reject 2M (not a multiple of 24k)"
    );
}

// ---------------------------------------------------------------------------
// Error details: nearest valid rates are correct
// ---------------------------------------------------------------------------

#[test]
fn error_suggests_nearest_valid_rates() {
    let err = DecimationConfig::compute(2_000_000).unwrap_err();
    match err {
        DecimationError::NotDivisible {
            sample_rate,
            nearest_lower,
            nearest_higher,
        } => {
            assert_eq!(sample_rate, 2_000_000);
            // 2_000_000 / 24_000 = 83.33 -> floor = 83 * 24000 = 1_992_000
            assert_eq!(nearest_lower, 1_992_000);
            assert_eq!(nearest_higher, 2_016_000);
        }
        _ => panic!("expected NotDivisible error"),
    }
}

#[test]
fn zero_rate_error_suggests_channel_rate() {
    let err = DecimationConfig::compute(0).unwrap_err();
    match err {
        DecimationError::NotDivisible {
            nearest_lower,
            nearest_higher,
            ..
        } => {
            assert_eq!(nearest_lower, 24_000, "zero rate should suggest 24k as nearest");
            assert_eq!(nearest_higher, 24_000);
        }
        _ => panic!("expected NotDivisible error"),
    }
}

// ---------------------------------------------------------------------------
// Input rate cascading: verify stage input rates chain correctly
// ---------------------------------------------------------------------------

#[test]
fn stage_input_rates_chain_correctly() {
    let valid_rates: Vec<u32> = vec![
        2_400_000, 2_880_000, 3_000_000, 4_800_000, 6_000_000, 9_600_000,
    ];

    for rate in valid_rates {
        let config = DecimationConfig::compute(rate).unwrap();
        let mut expected_input = rate as f32;

        for (i, stage) in config.stages.iter().enumerate() {
            assert!(
                (stage.input_rate - expected_input).abs() < 1.0,
                "stage {i} input_rate for {rate}: expected {expected_input}, got {}",
                stage.input_rate
            );
            expected_input /= stage.decimation_factor as f32;
        }

        // After all stages, should be at channel rate (24 kHz).
        assert!(
            (expected_input - CHANNEL_RATE as f32).abs() < 1.0,
            "final rate for {rate}: expected {CHANNEL_RATE}, got {expected_input}"
        );
    }
}

// ---------------------------------------------------------------------------
// Single-stage config for small decimation factors
// ---------------------------------------------------------------------------

#[test]
fn single_stage_for_small_decimation() {
    // 48000 / 24000 = 2x, should be single stage.
    let config = DecimationConfig::compute(48_000).expect("48k should be valid");
    assert_eq!(config.stages.len(), 1);
    assert_eq!(config.total_decimation(), 2);
    assert_eq!(config.stages[0].decimation_factor, 2);
    // Single stage should use final cutoff.
    assert!(
        (config.stages[0].cutoff_hz - 6_250.0).abs() < 0.01,
        "single stage should use 6250 Hz cutoff"
    );
}

#[test]
fn single_stage_at_max_factor() {
    // 600000 / 24000 = 25x, exactly at max single-stage factor.
    let config = DecimationConfig::compute(600_000).expect("600k should be valid");
    assert_eq!(config.stages.len(), 1);
    assert_eq!(config.total_decimation(), 25);
}

#[test]
fn two_stages_just_above_max_single_factor() {
    // 624000 / 24000 = 26x, needs two stages.
    let config = DecimationConfig::compute(624_000).expect("624k should be valid");
    assert!(config.stages.len() >= 2, "26x needs multi-stage");
    assert_eq!(config.total_decimation(), 26);
}

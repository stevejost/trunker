//! Benchmark: Costas loop acquisition time with and without frequency seeding.
//!
//! Generates a synthetic CQPSK P25 signal with a frequency offset (simulating
//! SDR LO error), feeds it through the CQPSK demodulator, and measures how
//! many symbols the Costas loop takes to converge.
//!
//! The CC's locked frequency estimate captures the LO error. Seeding this
//! into a new voice channel's Costas loop skips the slow frequency ramp-up
//! (the integrator path, beta=0.000253, takes many symbols cold).

use std::f32::consts::PI;
use trunker::dsp::costas::CostasLoop;
use num_complex::Complex;

/// Generate a QPSK signal with a frequency offset, simulating SDR LO error.
fn generate_offset_signal(
    num_symbols: usize,
    samples_per_symbol: usize,
    freq_offset_hz: f32,
    sample_rate: f32,
) -> Vec<Complex<f32>> {
    let freq_offset_rad = 2.0 * PI * freq_offset_hz / sample_rate;
    let phases = [
        PI / 4.0,
        3.0 * PI / 4.0,
        -3.0 * PI / 4.0,
        -PI / 4.0,
    ];

    let mut samples = Vec::with_capacity(num_symbols * samples_per_symbol);
    for i in 0..num_symbols {
        let constellation_phase = phases[i % 4];
        for j in 0..samples_per_symbol {
            let sample_idx = (i * samples_per_symbol + j) as f32;
            let phase = constellation_phase + freq_offset_rad * sample_idx;
            samples.push(Complex::from_polar(1.0, phase));
        }
    }
    samples
}

/// Measure how many symbols it takes for a Costas loop to converge
/// (mean phase error over a window drops below threshold).
fn measure_acquisition(
    costas: &mut CostasLoop,
    signal: &[Complex<f32>],
    samples_per_symbol: usize,
) -> usize {
    let threshold = 0.15;
    let window = 10;
    let mut errors: Vec<f32> = Vec::new();
    let mut symbol_count = 0;

    for (i, &sample) in signal.iter().enumerate() {
        let corrected = costas.process(sample);
        if i % samples_per_symbol == samples_per_symbol / 2 {
            let error = phase_error(corrected);
            errors.push(error.abs());
            symbol_count += 1;

            if errors.len() >= window {
                let recent: f32 =
                    errors[errors.len() - window..].iter().sum::<f32>() / window as f32;
                if recent < threshold {
                    return symbol_count;
                }
            }
        }
    }
    symbol_count // didn't converge
}

/// QPSK phase error: sgn(I)*Q - sgn(Q)*I.
fn phase_error(sample: Complex<f32>) -> f32 {
    let i = sample.re;
    let q = sample.im;
    let si = if i >= 0.0 { 1.0_f32 } else { -1.0 };
    let sq = if q >= 0.0 { 1.0_f32 } else { -1.0 };
    si * q - sq * i
}

/// Cold-start vs frequency-seeded acquisition with a 30 Hz LO offset.
///
/// This simulates the real scenario: the SDR's crystal oscillator has a
/// fixed error (~30 Hz), the CC Costas loop measures it, and we seed
/// new voice channels with that frequency estimate.
#[test]
fn frequency_seed_reduces_acquisition_time() {
    let sample_rate = 24_000.0;
    let freq_offset_hz = 30.0; // typical SDR LO error
    let samples_per_symbol = 5;
    let signal = generate_offset_signal(400, samples_per_symbol, freq_offset_hz, sample_rate);

    // Cold start: Costas loop starts from zero.
    let mut cold = CostasLoop::default();
    let cold_symbols = measure_acquisition(&mut cold, &signal, samples_per_symbol);

    // Seeded: pre-load the frequency estimate from the CC's locked state.
    let expected_freq_rad = 2.0 * PI * freq_offset_hz / sample_rate;
    let mut seeded = CostasLoop::default();
    seeded.seed(0.0, expected_freq_rad); // phase=0.0, frequency from CC
    let seeded_symbols = measure_acquisition(&mut seeded, &signal, samples_per_symbol);

    println!("Costas frequency seed benchmark (30 Hz LO offset):");
    println!("  Cold-start acquisition:  {cold_symbols} symbols");
    println!("  Seeded acquisition:      {seeded_symbols} symbols");
    println!(
        "  Improvement: {} symbols faster ({:.0}%)",
        cold_symbols.saturating_sub(seeded_symbols),
        if cold_symbols > 0 {
            (1.0 - seeded_symbols as f64 / cold_symbols as f64) * 100.0
        } else {
            0.0
        }
    );

    assert!(
        seeded_symbols < cold_symbols,
        "seeded ({seeded_symbols}) should acquire faster than cold ({cold_symbols})"
    );
}

/// With a larger frequency offset (100 Hz), the seed benefit is even more
/// pronounced since the cold-start integrator takes longer to ramp up.
#[test]
fn frequency_seed_benefit_increases_with_offset() {
    let sample_rate = 24_000.0;
    let samples_per_symbol = 5;

    let mut results = Vec::new();
    for &offset_hz in &[10.0_f32, 30.0, 60.0, 100.0] {
        let signal = generate_offset_signal(600, samples_per_symbol, offset_hz, sample_rate);
        let expected_freq_rad = 2.0 * PI * offset_hz / sample_rate;

        let mut cold = CostasLoop::default();
        let cold_symbols = measure_acquisition(&mut cold, &signal, samples_per_symbol);

        let mut seeded = CostasLoop::default();
        seeded.seed(0.0, expected_freq_rad);
        let seeded_symbols = measure_acquisition(&mut seeded, &signal, samples_per_symbol);

        results.push((offset_hz, cold_symbols, seeded_symbols));
    }

    println!("\nCostas seed benchmark across frequency offsets:");
    println!("  {:>10} {:>12} {:>12} {:>10}", "Offset Hz", "Cold (sym)", "Seeded (sym)", "Saved");
    for &(offset, cold, seeded) in &results {
        println!(
            "  {:>10.0} {:>12} {:>12} {:>10}",
            offset,
            cold,
            seeded,
            cold.saturating_sub(seeded)
        );
    }

    // At every offset, seeded should be equal or faster.
    for &(offset, cold, seeded) in &results {
        assert!(
            seeded <= cold,
            "seeded ({seeded}) should not be slower than cold ({cold}) at {offset} Hz"
        );
    }

    // At the larger offsets, seeded should be strictly faster.
    let (_, cold_100, seeded_100) = results.last().unwrap();
    assert!(
        seeded_100 < cold_100,
        "seeded ({seeded_100}) must be faster than cold ({cold_100}) at 100 Hz offset"
    );
}

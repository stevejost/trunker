//! Control channel hunting via parallel NCO scanning.
//!
//! Given a list of candidate control channel frequencies, creates one
//! [`ChannelPipeline`] per candidate, feeds wideband IQ to all simultaneously,
//! and locks onto the first candidate that produces the configured number
//! of valid TSBKs (default: 2).

use num_complex::Complex;

use crate::dsp::nco::Nco;
use crate::p25::nid::NidIntegrityPolicy;
use crate::p25::receiver::ReceiverEvent;
use crate::p25::tsbk::Tsbk;
use crate::p25::types::{Frequency, Nac};
use crate::pipeline::{self, ChannelPipeline, PipelineConfig};

/// Configuration for control channel hunting.
pub struct CcHunterConfig {
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Center frequency of the SDR capture in Hz.
    pub center_frequency: u64,
    /// Candidate CC frequencies in Hz.
    pub control_channels: Vec<u64>,
    /// Modulation type.
    pub modulation: pipeline::Modulation,
    /// NID integrity policy.
    pub nid_integrity: NidIntegrityPolicy,
    /// Number of valid TSBKs required to declare a CC found.
    pub lock_threshold: u32,
    /// Optional NAC filter -- only count TSBKs with matching NAC.
    pub expected_nac: Option<Nac>,
    /// Hunt timeout in samples. If no CC found after this many samples,
    /// return `HuntResult::Timeout`.
    pub hunt_timeout_samples: u64,
}

/// Per-candidate tracking state.
struct CandidateChannel {
    /// Frequency of this candidate.
    frequency: Frequency,
    /// NCO offset from center frequency in Hz.
    offset_hz: f64,
    /// NCO for frequency shifting.
    nco: Nco,
    /// Decode pipeline (`None` after the winner is extracted).
    pipeline: Option<ChannelPipeline>,
    /// Number of valid TSBKs decoded so far.
    tsbk_count: u32,
    /// NAC seen in decoded TSBKs.
    observed_nac: Option<Nac>,
    /// TSBKs collected during hunting (for ident table seeding).
    collected_tsbks: Vec<Tsbk>,
}

/// Result of processing one IQ sample during hunting.
pub enum HuntResult {
    /// Still scanning, no CC found yet.
    Scanning,
    /// A CC has been found. Returns the frequency, offset, pipeline,
    /// and any TSBKs collected during hunting.
    Locked {
        /// Frequency of the locked control channel.
        frequency: Frequency,
        /// NCO offset in Hz from center frequency.
        offset_hz: f64,
        /// The decode pipeline for the locked channel (boxed to reduce enum size).
        pipeline: Box<ChannelPipeline>,
        /// TSBKs decoded during hunting (for ident table seeding).
        collected_tsbks: Vec<Tsbk>,
    },
    /// Hunt timed out without finding a CC.
    Timeout,
}

/// Control channel hunter that scans multiple candidates in parallel.
pub struct CcHunter {
    candidates: Vec<CandidateChannel>,
    config: CcHunterConfig,
    samples_processed: u64,
}

impl CcHunter {
    /// Create a new CC hunter from configuration.
    ///
    /// Filters out candidates that are outside the capture bandwidth
    /// and creates a pipeline for each valid candidate.
    pub fn new(config: CcHunterConfig) -> anyhow::Result<Self> {
        let half_bw = f64::from(config.sample_rate) / 2.0;
        let center = config.center_frequency as f64;

        let mut candidates = Vec::new();
        for &freq_hz in &config.control_channels {
            let offset = freq_hz as f64 - center;
            if offset.abs() > half_bw {
                tracing::warn!(
                    frequency = freq_hz,
                    "CC candidate outside capture bandwidth, skipping"
                );
                continue;
            }

            let pipeline_config = PipelineConfig {
                sample_rate: config.sample_rate,
                modulation: config.modulation,
                nid_integrity: config.nid_integrity,
                sync_timeout_samples: Some(pipeline::CC_SYNC_TIMEOUT_SAMPLES),
            };
            let pipeline = ChannelPipeline::new(pipeline_config)?;
            let nco = Nco::new(offset, f64::from(config.sample_rate));

            candidates.push(CandidateChannel {
                frequency: Frequency::from_hz(freq_hz),
                offset_hz: offset,
                nco,
                pipeline: Some(pipeline),
                tsbk_count: 0,
                observed_nac: None,
                collected_tsbks: Vec::new(),
            });
        }

        if candidates.is_empty() {
            anyhow::bail!(
                "no CC candidates within capture bandwidth ({} Hz centered at {} Hz)",
                config.sample_rate,
                config.center_frequency
            );
        }

        tracing::info!(
            candidates = candidates.len(),
            "CC hunter initialized, scanning {} frequencies",
            candidates.len()
        );

        Ok(Self {
            candidates,
            config,
            samples_processed: 0,
        })
    }

    /// Feed one IQ sample to all candidate pipelines.
    ///
    /// Returns `HuntResult::Locked` when a candidate reaches the lock
    /// threshold. The winning candidate's pipeline is extracted via
    /// `Option::take`.
    pub fn process_sample(&mut self, sample: Complex<f32>) -> HuntResult {
        self.samples_processed += 1;

        if self.samples_processed >= self.config.hunt_timeout_samples {
            return HuntResult::Timeout;
        }

        for candidate in &mut self.candidates {
            let pipeline = match candidate.pipeline.as_mut() {
                Some(p) => p,
                None => continue,
            };
            let shifted = candidate.nco.shift(sample);
            if let Some(ReceiverEvent::Tsbk(ref tsbk)) = pipeline.process_sample(shifted) {
                let nac = pipeline.current_nac();

                let nac_ok = match self.config.expected_nac {
                    Some(expected) => nac == expected,
                    None => true,
                };

                if nac_ok {
                    candidate.tsbk_count += 1;
                    candidate.observed_nac = Some(nac);
                    candidate.collected_tsbks.push(tsbk.clone());

                    tracing::debug!(
                        frequency = candidate.frequency.hz(),
                        tsbk_count = candidate.tsbk_count,
                        nac = %nac,
                        "CC candidate decoded TSBK"
                    );

                    if candidate.tsbk_count >= self.config.lock_threshold {
                        tracing::info!(
                            frequency = %candidate.frequency,
                            tsbk_count = candidate.tsbk_count,
                            nac = %nac,
                            "locked onto control channel"
                        );

                        let winner = candidate.pipeline.take().ok_or_else(|| {
                            anyhow::anyhow!(
                                "pipeline already extracted for {}",
                                candidate.frequency
                            )
                        });
                        // Safety: we just matched Some(p) above, so take() always succeeds.
                        // Use ok_or_else for library-safe error handling per CLAUDE.md.
                        let collected_tsbks = std::mem::take(&mut candidate.collected_tsbks);

                        return match winner {
                            Ok(pipeline) => HuntResult::Locked {
                                frequency: candidate.frequency,
                                offset_hz: candidate.offset_hz,
                                pipeline: Box::new(pipeline),
                                collected_tsbks,
                            },
                            Err(_) => HuntResult::Timeout,
                        };
                    }
                }
            }
        }

        HuntResult::Scanning
    }

    /// Get the number of candidates being scanned.
    pub fn candidate_count(&self) -> usize {
        self.candidates.len()
    }

    /// Get the number of samples processed so far.
    pub fn samples_processed(&self) -> u64 {
        self.samples_processed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(channels: Vec<u64>) -> CcHunterConfig {
        CcHunterConfig {
            sample_rate: 3_600_000,
            center_frequency: 852_475_000,
            control_channels: channels,
            modulation: pipeline::Modulation::Cqpsk,
            nid_integrity: NidIntegrityPolicy::Strict,
            lock_threshold: 2,
            expected_nac: None,
            hunt_timeout_samples: 18_000_000,
        }
    }

    #[test]
    fn rejects_empty_candidate_list() {
        let config = make_config(vec![]);
        assert!(CcHunter::new(config).is_err());
    }

    #[test]
    fn filters_out_of_band_candidates() {
        // center=852.475 MHz, sr=3.6 MHz, half_bw=1.8 MHz
        // Valid range: 850.675 - 854.275 MHz
        let config = make_config(vec![
            852_350_000, // in band: offset = -125 kHz
            860_000_000, // out of band: offset = +7.525 MHz
        ]);
        let hunter = CcHunter::new(config).unwrap();
        assert_eq!(hunter.candidate_count(), 1);
    }

    #[test]
    fn all_candidates_out_of_band_is_error() {
        let config = make_config(vec![800_000_000, 900_000_000]);
        assert!(CcHunter::new(config).is_err());
    }

    #[test]
    fn creates_pipeline_per_candidate() {
        let config = make_config(vec![852_350_000, 853_187_500, 853_450_000, 853_875_000]);
        let hunter = CcHunter::new(config).unwrap();
        assert_eq!(hunter.candidate_count(), 4);
    }

    #[test]
    fn initial_state_is_scanning() {
        let config = make_config(vec![852_350_000]);
        let mut hunter = CcHunter::new(config).unwrap();
        let result = hunter.process_sample(Complex::new(0.0, 0.0));
        assert!(matches!(result, HuntResult::Scanning));
    }

    #[test]
    fn timeout_after_configured_samples() {
        let mut config = make_config(vec![852_350_000]);
        config.hunt_timeout_samples = 10;
        let mut hunter = CcHunter::new(config).unwrap();
        let mut result = HuntResult::Scanning;
        for _ in 0..11 {
            result = hunter.process_sample(Complex::new(0.001, 0.0));
        }
        assert!(matches!(result, HuntResult::Timeout));
    }

    #[test]
    fn samples_processed_tracks_correctly() {
        let config = make_config(vec![852_350_000]);
        let mut hunter = CcHunter::new(config).unwrap();
        assert_eq!(hunter.samples_processed(), 0);
        for _ in 0..100 {
            hunter.process_sample(Complex::new(0.0, 0.0));
        }
        assert_eq!(hunter.samples_processed(), 100);
    }
}

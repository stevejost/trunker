//! Symbol timing recovery via frame sync locking.
//!
//! For MVP, uses frame sync detection to establish symbol timing
//! rather than a PLL. Once the sync pattern is found at a particular
//! sample offset, symbols are extracted every 5 samples from that point.

use crate::p25::consts::SYMBOL_RATE;
use crate::p25::types::Dibit;

use super::slicer::DibitSlicer;
use super::sync::SyncDetector;

/// Number of baseband samples per symbol at 24 kHz output rate.
const SAMPLES_PER_SYMBOL: usize = 5; // 24000 / 4800

/// Number of dibits in the frame sync pattern.
const SYNC_DIBITS: usize = 24;

/// Indices of dibit 01 (+3 deviation) positions in the sync pattern.
const SYNC_POSITIVE_INDICES: [usize; 11] = [0, 1, 2, 3, 4, 6, 7, 10, 11, 16, 18];

/// Indices of dibit 11 (-3 deviation) positions in the sync pattern.
const SYNC_NEGATIVE_INDICES: [usize; 13] = [5, 8, 9, 12, 13, 14, 15, 17, 19, 20, 21, 22, 23];

/// Maximum symbols to try at one timing offset before rotating.
const SEARCH_WINDOW_SYMBOLS: usize = SYNC_DIBITS * 4;

/// Symbol timing recovery and frame synchronization.
///
/// Processes baseband samples (at 24 kHz, 5 samples per symbol) and
/// produces synchronized dibits. Uses frame sync detection to lock
/// timing and calibrate the dibit slicer.
#[derive(Debug)]
pub struct SymbolTiming {
    slicer: DibitSlicer,
    sync_detector: SyncDetector,
    sample_count: usize,
    /// Sample offset within the symbol period (0..4).
    timing_offset: usize,
    /// Whether we've locked to a frame sync.
    locked: bool,
    /// Symbols tested at current offset without finding sync.
    search_symbols: usize,
    /// Buffer of symbol-rate samples for threshold calibration.
    sync_samples: Vec<f32>,
}

impl SymbolTiming {
    /// Create a new symbol timing recovery block.
    pub fn new() -> Self {
        Self {
            slicer: DibitSlicer::new(),
            sync_detector: SyncDetector::new(),
            sample_count: 0,
            timing_offset: 0,
            locked: false,
            search_symbols: 0,
            sync_samples: Vec::with_capacity(SYNC_DIBITS),
        }
    }

    /// Return the output symbol rate in hertz.
    pub fn symbol_rate() -> u32 {
        SYMBOL_RATE
    }

    /// Set initial slicer thresholds before sync is acquired.
    ///
    /// Use estimated outer deviation levels to bootstrap the slicer.
    /// Once a frame sync is found, thresholds are recalibrated
    /// automatically from the measured sync symbol amplitudes.
    pub fn set_initial_thresholds(&mut self, positive_avg: f32, negative_avg: f32) {
        self.slicer.calibrate(positive_avg, negative_avg);
    }

    /// Whether timing is currently locked to a frame sync.
    pub fn is_locked(&self) -> bool {
        self.locked
    }

    /// Process one baseband sample.
    ///
    /// Returns `Some(SymbolEvent)` when a symbol is extracted or
    /// frame sync is detected. Returns `None` for inter-symbol samples.
    pub fn process(&mut self, sample: f32) -> Option<SymbolEvent> {
        self.sample_count += 1;

        // Only process samples at the current timing offset.
        if (self.sample_count % SAMPLES_PER_SYMBOL) != self.timing_offset {
            return None;
        }

        let dibit = self.slicer.slice(sample);
        self.sync_samples.push(sample);

        if self.sync_detector.feed(dibit) {
            self.locked = true;
            self.calibrate_from_sync();
            return Some(SymbolEvent::SyncDetected);
        }

        if self.locked {
            // Keep sync_samples bounded while tracking.
            if self.sync_samples.len() > SYNC_DIBITS * 2 {
                self.sync_samples
                    .drain(..self.sync_samples.len() - SYNC_DIBITS);
            }
            return Some(SymbolEvent::Symbol(dibit));
        }

        // Search mode: rotate through timing offsets.
        self.search_symbols += 1;
        if self.search_symbols >= SEARCH_WINDOW_SYMBOLS {
            self.timing_offset = (self.timing_offset + 1) % SAMPLES_PER_SYMBOL;
            self.search_symbols = 0;
            self.sync_detector.reset();
            self.sync_samples.clear();
        }

        None
    }

    /// Calibrate slicer thresholds from the sync pattern samples.
    fn calibrate_from_sync(&mut self) {
        if self.sync_samples.len() < SYNC_DIBITS {
            return;
        }

        let start = self.sync_samples.len() - SYNC_DIBITS;
        let sync_slice = &self.sync_samples[start..];

        let positive_sum: f32 = SYNC_POSITIVE_INDICES
            .iter()
            .map(|&i| sync_slice[i])
            .sum();
        let negative_sum: f32 = SYNC_NEGATIVE_INDICES
            .iter()
            .map(|&i| sync_slice[i])
            .sum();

        let positive_avg = positive_sum / SYNC_POSITIVE_INDICES.len() as f32;
        let negative_avg = negative_sum / SYNC_NEGATIVE_INDICES.len() as f32;

        self.slicer.calibrate(positive_avg, negative_avg);
    }
}

impl Default for SymbolTiming {
    fn default() -> Self {
        Self::new()
    }
}

/// Events produced by the symbol timing recovery block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolEvent {
    /// Frame sync pattern detected. Subsequent symbols belong
    /// to the new data unit.
    SyncDetected,
    /// A symbol was extracted at the correct timing point.
    Symbol(Dibit),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a baseband signal from a dibit sequence at 5 samples/symbol.
    fn dibits_to_baseband(dibits: &[Dibit]) -> Vec<f32> {
        dibits
            .iter()
            .flat_map(|d| {
                let level = match d.bits() {
                    0b01 => 3.0,
                    0b00 => 1.0,
                    0b10 => -1.0,
                    0b11 => -3.0,
                    _ => 0.0,
                };
                std::iter::repeat_n(level, SAMPLES_PER_SYMBOL)
            })
            .collect()
    }

    fn sync_dibits() -> Vec<Dibit> {
        use super::super::sync::sync_pattern_to_dibits;
        sync_pattern_to_dibits().to_vec()
    }

    #[test]
    fn detects_sync_in_clean_signal() {
        let mut timing = SymbolTiming::new();
        // Calibrate with known levels so slicing works.
        timing.set_initial_thresholds(3.0, -3.0);

        let pattern = sync_dibits();
        let baseband = dibits_to_baseband(&pattern);

        let mut found_sync = false;
        for sample in baseband {
            if let Some(SymbolEvent::SyncDetected) = timing.process(sample) {
                found_sync = true;
            }
        }
        assert!(found_sync, "should detect sync in clean signal");
    }

    #[test]
    fn extracts_symbols_after_sync() {
        let mut timing = SymbolTiming::new();
        timing.set_initial_thresholds(3.0, -3.0);

        let sync = sync_dibits();
        let data = vec![
            Dibit::new(0b01),
            Dibit::new(0b00),
            Dibit::new(0b10),
            Dibit::new(0b11),
        ];

        let mut all_dibits: Vec<Dibit> = sync;
        all_dibits.extend_from_slice(&data);
        let baseband = dibits_to_baseband(&all_dibits);

        let mut symbols = Vec::new();
        for sample in baseband {
            match timing.process(sample) {
                Some(SymbolEvent::Symbol(d)) => symbols.push(d),
                _ => {}
            }
        }

        // After sync, we should get our 4 data dibits.
        assert!(
            symbols.len() >= 4,
            "expected >= 4 symbols, got {}",
            symbols.len()
        );

        let last_four: Vec<_> = symbols.iter().rev().take(4).rev().collect();
        assert_eq!(last_four[0].bits(), 0b01);
        assert_eq!(last_four[1].bits(), 0b00);
        assert_eq!(last_four[2].bits(), 0b10);
        assert_eq!(last_four[3].bits(), 0b11);
    }

    #[test]
    fn locks_timing_after_sync() {
        let mut timing = SymbolTiming::new();
        timing.set_initial_thresholds(3.0, -3.0);

        let pattern = sync_dibits();
        let baseband = dibits_to_baseband(&pattern);

        for sample in baseband {
            timing.process(sample);
        }

        assert!(timing.is_locked(), "should be locked after sync");
    }

    #[test]
    fn symbol_rate_is_4800() {
        assert_eq!(SymbolTiming::symbol_rate(), 4800);
    }

    #[test]
    fn sync_positive_and_negative_indices_are_complete() {
        let mut all_indices: Vec<usize> = Vec::new();
        all_indices.extend_from_slice(&SYNC_POSITIVE_INDICES);
        all_indices.extend_from_slice(&SYNC_NEGATIVE_INDICES);
        all_indices.sort();
        let expected: Vec<usize> = (0..SYNC_DIBITS).collect();
        assert_eq!(all_indices, expected);
    }
}

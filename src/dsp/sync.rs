//! Frame sync detector for P25 data unit boundaries.
//!
//! Detects the 48-bit (24-dibit) frame sync pattern in a stream of
//! dibits using a sliding window with configurable error tolerance.

use crate::p25::consts::FRAME_SYNC;
use crate::p25::types::Dibit;

/// Number of dibits in the frame sync pattern.
const SYNC_DIBITS: usize = 24;

/// Maximum number of dibit errors allowed for a valid sync detection.
const MAX_SYNC_ERRORS: usize = 3;

/// The frame sync pattern as an array of dibits.
///
/// Derived from FRAME_SYNC (0x5575F5FF77FF):
/// 01 01 01 01 01 11 01 01 11 11 01 01 11 11 11 11 01 11 01 11 11 11 11 11
const SYNC_DIBITS_PATTERN: [u8; SYNC_DIBITS] = [
    0b01, 0b01, 0b01, 0b01, 0b01, 0b11, 0b01, 0b01, 0b11, 0b11, 0b01, 0b01, 0b11, 0b11, 0b11, 0b11,
    0b01, 0b11, 0b01, 0b11, 0b11, 0b11, 0b11, 0b11,
];

/// Frame sync detector using a dibit sliding window.
///
/// Feeds dibits one at a time and reports when the frame sync pattern
/// is detected (within the error tolerance).
#[derive(Debug)]
pub struct SyncDetector {
    window: [u8; SYNC_DIBITS],
    position: usize,
    filled: bool,
}

impl SyncDetector {
    /// Create a new sync detector with an empty window.
    pub fn new() -> Self {
        Self {
            window: [0; SYNC_DIBITS],
            position: 0,
            filled: false,
        }
    }

    /// Feed one dibit into the detector.
    ///
    /// Returns `true` if the sliding window now matches the frame
    /// sync pattern within the error tolerance.
    pub fn feed(&mut self, dibit: Dibit) -> bool {
        self.window[self.position] = dibit.bits();
        self.position = (self.position + 1) % SYNC_DIBITS;
        if self.position == 0 {
            self.filled = true;
        }
        if !self.filled {
            return false;
        }
        self.check_sync()
    }

    /// Count mismatches between the current window and the sync pattern.
    fn error_count(&self) -> usize {
        SYNC_DIBITS_PATTERN
            .iter()
            .enumerate()
            .filter(|(i, expected)| {
                let window_idx = (self.position + i) % SYNC_DIBITS;
                self.window[window_idx] != **expected
            })
            .count()
    }

    /// Check if the current window matches the sync pattern.
    fn check_sync(&self) -> bool {
        self.error_count() <= MAX_SYNC_ERRORS
    }

    /// Reset the detector state.
    pub fn reset(&mut self) {
        self.window = [0; SYNC_DIBITS];
        self.position = 0;
        self.filled = false;
    }
}

impl Default for SyncDetector {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert the FRAME_SYNC constant to a dibit array for verification.
pub fn sync_pattern_to_dibits() -> [Dibit; SYNC_DIBITS] {
    let mut dibits = [Dibit::new(0); SYNC_DIBITS];
    for (i, dibit) in dibits.iter_mut().enumerate() {
        let shift = (SYNC_DIBITS - 1 - i) * 2;
        let bits = ((FRAME_SYNC >> shift) & 0x03) as u8;
        *dibit = Dibit::new(bits);
    }
    dibits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sync_dibits() -> Vec<Dibit> {
        SYNC_DIBITS_PATTERN.iter().map(|&b| Dibit::new(b)).collect()
    }

    #[test]
    fn sync_pattern_matches_hex_constant() {
        let from_hex = sync_pattern_to_dibits();
        for (i, (hex_dibit, &table_val)) in
            from_hex.iter().zip(SYNC_DIBITS_PATTERN.iter()).enumerate()
        {
            assert_eq!(
                hex_dibit.bits(),
                table_val,
                "mismatch at dibit {i}: hex={} table={}",
                hex_dibit.bits(),
                table_val
            );
        }
    }

    #[test]
    fn sync_pattern_contains_only_outer_dibits() {
        for (i, &val) in SYNC_DIBITS_PATTERN.iter().enumerate() {
            assert!(
                val == 0b01 || val == 0b11,
                "dibit {i} is {val:02b}, expected 01 or 11"
            );
        }
    }

    #[test]
    fn sync_pattern_dibit_counts() {
        let count_01 = SYNC_DIBITS_PATTERN.iter().filter(|&&v| v == 0b01).count();
        let count_11 = SYNC_DIBITS_PATTERN.iter().filter(|&&v| v == 0b11).count();
        assert_eq!(count_01, 11, "expected 11 dibit-01 (+3 dev)");
        assert_eq!(count_11, 13, "expected 13 dibit-11 (-3 dev)");
        assert_eq!(count_01 + count_11, SYNC_DIBITS);
    }

    #[test]
    fn exact_match_detects_sync() {
        let mut detector = SyncDetector::new();
        let pattern = sync_dibits();

        for (i, &dibit) in pattern.iter().enumerate() {
            let found = detector.feed(dibit);
            if i < SYNC_DIBITS - 1 {
                assert!(!found, "false positive at dibit {i}");
            } else {
                assert!(found, "should detect sync after all 24 dibits");
            }
        }
    }

    #[test]
    fn detect_with_1_error() {
        let mut detector = SyncDetector::new();
        let mut pattern = sync_dibits();
        // Corrupt one dibit.
        pattern[5] = Dibit::new(pattern[5].bits() ^ 0x02);

        for &dibit in &pattern {
            detector.feed(dibit);
        }
        assert!(detector.check_sync());
    }

    #[test]
    fn detect_with_3_errors() {
        let mut detector = SyncDetector::new();
        let mut pattern = sync_dibits();
        pattern[0] = Dibit::new(pattern[0].bits() ^ 0x02);
        pattern[10] = Dibit::new(pattern[10].bits() ^ 0x02);
        pattern[20] = Dibit::new(pattern[20].bits() ^ 0x02);

        for &dibit in &pattern {
            detector.feed(dibit);
        }
        assert!(detector.check_sync());
    }

    #[test]
    fn reject_with_4_errors() {
        let mut detector = SyncDetector::new();
        let mut pattern = sync_dibits();
        pattern[0] = Dibit::new(pattern[0].bits() ^ 0x02);
        pattern[5] = Dibit::new(pattern[5].bits() ^ 0x02);
        pattern[10] = Dibit::new(pattern[10].bits() ^ 0x02);
        pattern[15] = Dibit::new(pattern[15].bits() ^ 0x02);

        for &dibit in &pattern {
            detector.feed(dibit);
        }
        assert!(!detector.check_sync());
    }

    #[test]
    fn detect_after_garbage_prefix() {
        let mut detector = SyncDetector::new();

        // Feed 50 garbage dibits first.
        for i in 0..50 {
            detector.feed(Dibit::new((i % 4) as u8));
        }

        // Now feed the real sync pattern.
        let pattern = sync_dibits();
        let mut detected = false;
        for &dibit in &pattern {
            if detector.feed(dibit) {
                detected = true;
            }
        }
        assert!(detected, "should detect sync after garbage prefix");
    }

    #[test]
    fn reset_clears_state() {
        let mut detector = SyncDetector::new();
        let pattern = sync_dibits();

        // Feed half the pattern.
        for &dibit in &pattern[..12] {
            detector.feed(dibit);
        }

        detector.reset();

        // Now feed full pattern from scratch — should detect.
        let mut detected = false;
        for &dibit in &pattern {
            if detector.feed(dibit) {
                detected = true;
            }
        }
        assert!(detected);
    }
}

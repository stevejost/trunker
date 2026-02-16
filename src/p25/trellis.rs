//! Half-rate trellis (convolutional) coding for P25 TSBK payloads.
//!
//! Encoding maps each input dibit to a pair of output dibits using a 4-state
//! finite state machine. Decoding uses the Viterbi algorithm with truncated
//! history to find the most likely input sequence.

use crate::p25::error::P25Error;
use crate::p25::types::Dibit;

/// Number of states in the half-rate trellis FSM.
const NUM_STATES: usize = 4;

/// Constellation pairs table (16 entries).
///
/// Each entry is `(hi_dibit, lo_dibit)` forming the output pair for a
/// state transition. Indexed by `PAIR_INDEX[current_state][next_state]`.
const CONSTELLATION_PAIRS: [(u8, u8); 16] = [
    (0b00, 0b10), // 0
    (0b10, 0b10), // 1
    (0b01, 0b11), // 2
    (0b11, 0b11), // 3
    (0b11, 0b10), // 4
    (0b01, 0b10), // 5
    (0b10, 0b11), // 6
    (0b00, 0b11), // 7
    (0b11, 0b01), // 8
    (0b01, 0b01), // 9
    (0b10, 0b00), // 10
    (0b00, 0b00), // 11
    (0b00, 0b01), // 12
    (0b10, 0b01), // 13
    (0b01, 0b00), // 14
    (0b11, 0b00), // 15
];

/// State transition to constellation pair index mapping.
///
/// `PAIR_INDEX[current_state][next_state]` gives the index into
/// `CONSTELLATION_PAIRS` for that transition.
const PAIR_INDEX: [[usize; NUM_STATES]; NUM_STATES] =
    [[0, 15, 12, 3], [4, 11, 8, 7], [13, 2, 1, 14], [9, 6, 5, 10]];

/// Half-rate trellis encoder (test-only).
///
/// Encodes a sequence of input dibits into constellation pairs using a
/// 4-state finite state machine. Only needed for constructing test vectors.
#[cfg(test)]
#[derive(Debug, Default)]
pub struct TrellisEncoder {
    /// Current FSM state.
    state: usize,
}

#[cfg(test)]
impl TrellisEncoder {
    /// Create a new encoder at the initial state (0).
    pub fn new() -> Self {
        Self { state: 0 }
    }

    /// Encode one input dibit, returning the output pair `(hi, lo)`.
    pub fn encode(&mut self, input: Dibit) -> (Dibit, Dibit) {
        let next = input.bits() as usize;
        let idx = PAIR_INDEX[self.state][next];
        let (hi, lo) = CONSTELLATION_PAIRS[idx];
        self.state = next;
        (Dibit::new(hi), Dibit::new(lo))
    }

    /// Flush the encoder with dibit 00, returning the final pair.
    pub fn flush(&mut self) -> (Dibit, Dibit) {
        self.encode(Dibit::new(0b00))
    }

    /// Encode a slice of input dibits, appending a flush symbol.
    ///
    /// Returns all output dibits (2 per input dibit, plus 2 for flush).
    pub fn encode_all(&mut self, input: &[Dibit]) -> Vec<Dibit> {
        let mut output = Vec::with_capacity((input.len() + 1) * 2);
        for &dibit in input {
            let (hi, lo) = self.encode(dibit);
            output.push(hi);
            output.push(lo);
        }
        let (hi, lo) = self.flush();
        output.push(hi);
        output.push(lo);
        output
    }
}

/// Compute the Hamming distance between two 4-bit edge values.
fn hamming_distance(a: u8, b: u8) -> usize {
    (a ^ b).count_ones() as usize
}

/// Pack a dibit pair into a 4-bit edge value for distance computation.
fn pack_pair(hi: Dibit, lo: Dibit) -> u8 {
    (hi.bits() << 2) | lo.bits()
}

/// A Viterbi path through the trellis.
#[derive(Clone)]
struct Path {
    /// Accumulated Hamming distance (metric).
    distance: usize,
    /// History of states visited (oldest first).
    history: Vec<usize>,
}

impl Path {
    fn new(initial_state: usize, initial_distance: usize) -> Self {
        Self {
            distance: initial_distance,
            history: vec![initial_state],
        }
    }
}

/// Half-rate trellis decoder using the Viterbi algorithm.
///
/// Decodes 98 received dibits (49 pairs) into 48 data dibits plus one
/// flushing dibit (which is discarded).
pub fn decode(received: &[Dibit]) -> Result<Vec<Dibit>, P25Error> {
    if received.len() != 98 {
        return Err(P25Error::TrellisDecode {
            reason: format!("expected 98 dibits, got {}", received.len()),
        });
    }

    // Initialize paths: state 0 starts with distance 0, others with max
    let mut paths: Vec<Path> = (0..NUM_STATES)
        .map(|s| Path::new(s, if s == 0 { 0 } else { usize::MAX / 2 }))
        .collect();

    // Process 49 pairs of received dibits
    for pair_idx in 0..49 {
        let received_edge = pack_pair(received[pair_idx * 2], received[pair_idx * 2 + 1]);

        let mut new_paths: Vec<Path> = Vec::with_capacity(NUM_STATES);

        for next_state in 0..NUM_STATES {
            let best = find_best_predecessor(&paths, next_state, received_edge);
            new_paths.push(best);
        }

        paths = new_paths;
    }

    // Find the path with minimum distance
    let best_path =
        paths
            .iter()
            .min_by_key(|p| p.distance)
            .ok_or_else(|| P25Error::TrellisDecode {
                reason: "no valid path found".to_string(),
            })?;

    // Extract decoded dibits from state transitions in history.
    // The history contains states [s0, s1, s2, ...].
    // The input dibit at step i is the state s_{i+1} (since input = next_state).
    // Skip the first entry (initial state) and the last (flush).
    let history = &best_path.history;
    if history.len() < 2 {
        return Err(P25Error::TrellisDecode {
            reason: "path history too short".to_string(),
        });
    }

    // history[1..] gives us 49 decoded symbols (the next_state at each step).
    // The last one is the flush symbol (always 0), so take history[1..49].
    let decoded: Vec<Dibit> = history[1..history.len() - 1]
        .iter()
        .map(|&state| Dibit::new(state as u8))
        .collect();

    if decoded.len() != 48 {
        return Err(P25Error::TrellisDecode {
            reason: format!("expected 48 decoded dibits, got {}", decoded.len()),
        });
    }

    Ok(decoded)
}

/// Find the best predecessor state for a transition to `next_state`.
fn find_best_predecessor(paths: &[Path], next_state: usize, received_edge: u8) -> Path {
    let mut best_distance = usize::MAX;
    let mut best_prev = 0;

    for prev_state in 0..NUM_STATES {
        let idx = PAIR_INDEX[prev_state][next_state];
        let (hi, lo) = CONSTELLATION_PAIRS[idx];
        let expected_edge = (hi << 2) | lo;
        let dist = hamming_distance(received_edge, expected_edge);

        let total = paths[prev_state].distance.saturating_add(dist);
        if total < best_distance {
            best_distance = total;
            best_prev = prev_state;
        }
    }

    let mut new_path = paths[best_prev].clone();
    new_path.distance = best_distance;
    new_path.history.push(next_state);
    new_path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constellation_pairs_table_has_16_entries() {
        assert_eq!(CONSTELLATION_PAIRS.len(), 16);
    }

    #[test]
    fn pair_index_table_has_valid_indices() {
        for row in &PAIR_INDEX {
            for &idx in row {
                assert!(idx < 16, "pair index {idx} out of range");
            }
        }
    }

    #[test]
    fn encoder_known_sequence() {
        // Test vector from p25.rs reference: encode [0,0,1,1,2,2,3,3]
        let mut encoder = TrellisEncoder::new();

        // State 0 -> 0: PAIR_INDEX[0][0]=0, pair=(00,10)
        assert_eq!(
            encoder.encode(Dibit::new(0b00)),
            (Dibit::new(0b00), Dibit::new(0b10))
        );
        // State 0 -> 0: PAIR_INDEX[0][0]=0, pair=(00,10)
        assert_eq!(
            encoder.encode(Dibit::new(0b00)),
            (Dibit::new(0b00), Dibit::new(0b10))
        );
        // State 0 -> 1: PAIR_INDEX[0][1]=15, pair=(11,00)
        assert_eq!(
            encoder.encode(Dibit::new(0b01)),
            (Dibit::new(0b11), Dibit::new(0b00))
        );
        // State 1 -> 1: PAIR_INDEX[1][1]=11, pair=(00,00)
        assert_eq!(
            encoder.encode(Dibit::new(0b01)),
            (Dibit::new(0b00), Dibit::new(0b00))
        );
        // State 1 -> 2: PAIR_INDEX[1][2]=8, pair=(11,01)
        assert_eq!(
            encoder.encode(Dibit::new(0b10)),
            (Dibit::new(0b11), Dibit::new(0b01))
        );
        // State 2 -> 2: PAIR_INDEX[2][2]=1, pair=(10,10)
        assert_eq!(
            encoder.encode(Dibit::new(0b10)),
            (Dibit::new(0b10), Dibit::new(0b10))
        );
        // State 2 -> 3: PAIR_INDEX[2][3]=14, pair=(01,00)
        assert_eq!(
            encoder.encode(Dibit::new(0b11)),
            (Dibit::new(0b01), Dibit::new(0b00))
        );
        // State 3 -> 3: PAIR_INDEX[3][3]=10, pair=(10,00)
        assert_eq!(
            encoder.encode(Dibit::new(0b11)),
            (Dibit::new(0b10), Dibit::new(0b00))
        );
    }

    #[test]
    fn encode_then_decode_clean_signal() {
        let input: Vec<Dibit> = [1, 2, 2, 2, 2, 1, 3, 3, 0, 2]
            .iter()
            .map(|&b| Dibit::new(b))
            .collect();

        let mut encoder = TrellisEncoder::new();
        let encoded = encoder.encode_all(&input);

        // 10 input + 1 flush = 11 pairs = 22 dibits
        assert_eq!(encoded.len(), 22);

        // Pad to 98 dibits for the decoder (fill remaining with
        // encoded zeros from state 0)
        let mut padded = encoded.clone();
        let mut pad_encoder = TrellisEncoder::new();
        // Encoder state after encoding input+flush is 0 (flush goes to 0)
        // We need 49 - 11 = 38 more pairs = 38 input dibits + 0 extra flush
        // But total must be exactly 98 dibits = 49 pairs
        // We already have 11 pairs (22 dibits), need 38 more pairs
        for _ in 0..37 {
            let (hi, lo) = pad_encoder.encode(Dibit::new(0b00));
            padded.push(hi);
            padded.push(lo);
        }
        let (hi, lo) = pad_encoder.flush();
        padded.push(hi);
        padded.push(lo);

        assert_eq!(padded.len(), 98);

        let decoded = decode(&padded).unwrap();
        assert_eq!(decoded.len(), 48);

        // First 10 should match our input
        for (i, &d) in input.iter().enumerate() {
            assert_eq!(
                decoded[i].bits(),
                d.bits(),
                "mismatch at position {i}: expected {}, got {}",
                d.bits(),
                decoded[i].bits()
            );
        }
    }

    #[test]
    fn encode_decode_roundtrip_48_dibits() {
        // Create a full 48-dibit payload
        let input: Vec<Dibit> = (0..48).map(|i| Dibit::new((i % 4) as u8)).collect();

        let mut encoder = TrellisEncoder::new();
        let encoded = encoder.encode_all(&input);

        // 48 + 1 flush = 49 pairs = 98 dibits
        assert_eq!(encoded.len(), 98);

        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded.len(), 48);

        for (i, (&orig, &dec)) in input.iter().zip(decoded.iter()).enumerate() {
            assert_eq!(orig.bits(), dec.bits(), "mismatch at position {i}");
        }
    }

    #[test]
    fn decode_with_single_bit_errors() {
        let input: Vec<Dibit> = (0..48).map(|i| Dibit::new((i % 4) as u8)).collect();

        let mut encoder = TrellisEncoder::new();
        let mut encoded = encoder.encode_all(&input);

        // Inject single-bit errors in a few pairs
        encoded[2] = Dibit::new(encoded[2].bits() ^ 0b01);
        encoded[10] = Dibit::new(encoded[10].bits() ^ 0b10);
        encoded[20] = Dibit::new(encoded[20].bits() ^ 0b01);

        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded.len(), 48);

        for (i, (&orig, &dec)) in input.iter().zip(decoded.iter()).enumerate() {
            assert_eq!(
                orig.bits(),
                dec.bits(),
                "mismatch at position {i} after error injection"
            );
        }
    }

    #[test]
    fn decode_wrong_length_returns_error() {
        let short: Vec<Dibit> = (0..96).map(|_| Dibit::new(0)).collect();
        assert!(decode(&short).is_err());

        let long: Vec<Dibit> = (0..100).map(|_| Dibit::new(0)).collect();
        assert!(decode(&long).is_err());
    }

    #[test]
    fn hamming_distance_correctness() {
        assert_eq!(hamming_distance(0b0000, 0b0000), 0);
        assert_eq!(hamming_distance(0b1111, 0b0000), 4);
        assert_eq!(hamming_distance(0b1010, 0b0101), 4);
        assert_eq!(hamming_distance(0b1100, 0b1010), 2);
        assert_eq!(hamming_distance(0b0001, 0b0000), 1);
    }

    #[test]
    fn pack_pair_correctness() {
        assert_eq!(pack_pair(Dibit::new(0b11), Dibit::new(0b01)), 0b1101);
        assert_eq!(pack_pair(Dibit::new(0b00), Dibit::new(0b00)), 0b0000);
        assert_eq!(pack_pair(Dibit::new(0b10), Dibit::new(0b10)), 0b1010);
    }

    #[test]
    fn encode_decode_all_zeros() {
        let input: Vec<Dibit> = vec![Dibit::new(0); 48];
        let mut encoder = TrellisEncoder::new();
        let encoded = encoder.encode_all(&input);
        assert_eq!(encoded.len(), 98);

        let decoded = decode(&encoded).unwrap();
        for (i, d) in decoded.iter().enumerate() {
            assert_eq!(d.bits(), 0, "non-zero at position {i}");
        }
    }

    #[test]
    fn encode_decode_all_threes() {
        let input: Vec<Dibit> = vec![Dibit::new(3); 48];
        let mut encoder = TrellisEncoder::new();
        let encoded = encoder.encode_all(&input);
        assert_eq!(encoded.len(), 98);

        let decoded = decode(&encoded).unwrap();
        for (i, d) in decoded.iter().enumerate() {
            assert_eq!(d.bits(), 3, "mismatch at position {i}");
        }
    }
}

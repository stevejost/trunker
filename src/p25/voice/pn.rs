//! Pseudo-random (PN) sequence generator for voice frame descrambling.
//!
//! The P25 IMBE voice codec scrambles voice data using a linear congruential
//! generator (LCG): `state = 173 * state + 13849 (mod 65536)`. The random
//! bit at each step is the MSB (bit 15) of the state.
//!
//! The 12-bit seed comes from the u_0 chunk of the first voice frame in
//! each superframe. Methods `next_23()` and `next_15()` produce scrambling
//! masks for Golay-protected and Hamming-protected chunks respectively.

/// P25 PN sequence generator for voice frame scrambling/descrambling.
///
/// Generates scrambling words by extracting the MSB of a 16-bit LCG state
/// at each step.
pub struct PseudoRandom {
    /// Current LCG state (p_n in the standard).
    state: u16,
}

impl PseudoRandom {
    /// Create a new generator with the given 12-bit seed.
    ///
    /// The seed is typically extracted from the u_0 chunk of the first IMBE
    /// voice frame. It occupies bits 15..4 of the initial state.
    pub fn new(seed: u16) -> Self {
        debug_assert!(seed >> 12 == 0, "seed must be 12 bits, got {seed:#X}");
        Self {
            state: seed << 4,
        }
    }

    /// Generate the next 23-bit scrambling word for Golay-protected chunks.
    ///
    /// The MSB of the returned value corresponds to the first generated bit.
    pub fn next_23(&mut self) -> u32 {
        self.next_bits(23)
    }

    /// Generate the next 15-bit scrambling word for Hamming-protected chunks.
    ///
    /// The MSB of the returned value corresponds to the first generated bit.
    pub fn next_15(&mut self) -> u32 {
        self.next_bits(15)
    }

    /// Generate a scrambling word of the given bit width.
    fn next_bits(&mut self, count: usize) -> u32 {
        debug_assert!(count <= 32);
        (0..count).fold(0u32, |word, _| {
            (word << 1) | self.advance() as u32
        })
    }

    /// Step the LCG and return the next random bit (MSB of new state).
    fn advance(&mut self) -> u16 {
        self.state = self.state.wrapping_mul(173).wrapping_add(13849);
        self.state >> 15
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_state_sequence() {
        // Verify the LCG state transitions match the reference implementation.
        // Seed 0xABC -> initial state = 0xABC0 = 43968.
        let mut pn = PseudoRandom::new(0xABC);
        assert_eq!(pn.state, 0xABC0);

        // Each advance() steps state and returns MSB.
        // Expected states after each step (from p25.rs reference):
        let expected: [(u16, u16); 15] = [
            (18137, 0), // state after step 1, bit
            (5822, 0),
            (38015, 1),
            (36844, 1),
            (30869, 0),
            (45770, 1),
            (2203, 0),
            (1752, 0),
            (54801, 1),
            (57238, 1),
            (20087, 0),
            (15492, 0),
            (6989, 0),
            (43298, 1),
            (33299, 1),
        ];

        for (i, &(expected_state, expected_bit)) in expected.iter().enumerate() {
            let bit = pn.advance();
            assert_eq!(
                pn.state, expected_state,
                "state mismatch at step {i}"
            );
            assert_eq!(
                bit, expected_bit,
                "bit mismatch at step {i}"
            );
        }
    }

    #[test]
    fn next_15_known_vector() {
        let mut pn = PseudoRandom::new(0xABC);
        assert_eq!(pn.next_15(), 0b001101001100011);
    }

    #[test]
    fn next_23_produces_23_bits() {
        let mut pn = PseudoRandom::new(0xABC);
        let word = pn.next_23();
        assert!(word < (1 << 23), "next_23 must fit in 23 bits");
    }

    #[test]
    fn zero_seed() {
        let mut pn = PseudoRandom::new(0);
        assert_eq!(pn.state, 0);
        // First advance: 0 * 173 + 13849 = 13849
        let bit = pn.advance();
        assert_eq!(pn.state, 13849);
        assert_eq!(bit, 0); // 13849 < 32768, MSB = 0
    }

    #[test]
    fn max_seed() {
        let mut pn = PseudoRandom::new(0xFFF);
        assert_eq!(pn.state, 0xFFF0);
        // Verify it doesn't panic and produces valid output.
        let _word = pn.next_23();
    }

    #[test]
    fn deterministic_output() {
        // Same seed always produces same sequence.
        let mut pn1 = PseudoRandom::new(0x123);
        let mut pn2 = PseudoRandom::new(0x123);
        assert_eq!(pn1.next_23(), pn2.next_23());
        assert_eq!(pn1.next_15(), pn2.next_15());
        assert_eq!(pn1.next_23(), pn2.next_23());
    }

    #[test]
    fn different_seeds_different_output() {
        let mut pn1 = PseudoRandom::new(0x000);
        let mut pn2 = PseudoRandom::new(0x001);
        // Different seeds must produce different first words.
        assert_ne!(pn1.next_23(), pn2.next_23());
    }
}

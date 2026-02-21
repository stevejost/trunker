//! Error types for the IMBE vocoder.

/// Errors that can occur during IMBE vocoder decoding.
#[derive(Debug, thiserror::Error)]
pub enum VocoderError {
    /// Scan bit stream exhausted before all amplitude bits were extracted.
    ///
    /// This indicates a logic bug: the bit allocation table and scan procedure
    /// should produce exactly the right number of bits for a given harmonic
    /// count L. RF noise corrupts bit values but not bit count.
    #[error("scan exhausted early: expected more bits for L={harmonic_count}")]
    ScanExhausted {
        /// The harmonic count L that was being decoded.
        harmonic_count: u32,
    },

    /// Scan bit stream had leftover bits after amplitude extraction.
    ///
    /// Like `ScanExhausted`, this indicates a logic bug in the bit allocation
    /// or scanning procedure.
    #[error("scan not exhausted: leftover bits after decoding L={harmonic_count}")]
    ScanNotExhausted {
        /// The harmonic count L that was being decoded.
        harmonic_count: u32,
    },
}

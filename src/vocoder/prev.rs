//! Previous frame saved parameters for inter-frame prediction.
//!
//! Various parameters from the previous frame are needed when decoding
//! the current frame. This struct accumulates those fields as vocoder
//! modules are ported.

use super::descramble::VoiceDecisions;
use super::enhance::EnhancedSpectrals;
use super::params::BaseParams;
use super::spectral::Spectrals;
use super::voiced::{Phase, PhaseBase};

/// Parameters saved from the previous frame, used when constructing the
/// current frame.
///
/// Additional fields (error rate, energy, unvoiced DFT, etc.) will be
/// added as the corresponding modules are ported.
pub(crate) struct PrevFrame {
    /// Base parameters from the previous frame.
    pub(crate) params: BaseParams,
    /// Spectral amplitudes M_l from the previous frame.
    pub(crate) spectrals: Spectrals,
    /// Enhanced spectral amplitudes from the previous frame.
    pub(crate) enhanced: EnhancedSpectrals,
    /// Per-harmonic voiced/unvoiced decisions from the previous frame.
    pub(crate) voice: VoiceDecisions,
    /// Base phase offsets from the previous frame.
    pub(crate) phase_base: PhaseBase,
    /// Random phase terms from the previous frame.
    pub(crate) phase: Phase,
}

impl Default for PrevFrame {
    /// Create a new `PrevFrame` suitable for decoding the very first IMBE
    /// frame in a stream per TIA-102.BABA [p64].
    fn default() -> Self {
        Self {
            params: BaseParams::default(),
            spectrals: Spectrals::default(),
            enhanced: EnhancedSpectrals::default(),
            voice: VoiceDecisions::default(),
            phase_base: PhaseBase::default(),
            phase: Phase::default(),
        }
    }
}

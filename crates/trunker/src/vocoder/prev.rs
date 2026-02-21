//! Previous frame saved parameters for inter-frame prediction.
//!
//! Various parameters from the previous frame are needed when decoding
//! the current frame. This struct holds all 10 saved fields required by
//! the full decode pipeline.

use super::descramble::VoiceDecisions;
use super::enhance::{EnhancedSpectrals, FrameEnergy};
use super::params::BaseParams;
use super::spectral::Spectrals;
use super::unvoiced::UnvoicedDft;
use super::voiced::{Phase, PhaseBase};

/// Parameters saved from the previous frame, used when constructing the
/// current frame.
pub(crate) struct PrevFrame {
    /// Base parameters from the previous frame.
    pub(crate) params: BaseParams,
    /// Spectral amplitudes M_l from the previous frame.
    pub(crate) spectrals: Spectrals,
    /// Enhanced spectral amplitudes from the previous frame.
    pub(crate) enhanced: EnhancedSpectrals,
    /// Per-harmonic voiced/unvoiced decisions from the previous frame.
    pub(crate) voice: VoiceDecisions,
    /// Error rate EWMA from the previous frame, epsilon_R.
    pub(crate) error_rate: f32,
    /// Energy values from the previous frame.
    pub(crate) energy: FrameEnergy,
    /// Spectral amplitude threshold tau_M from the previous frame.
    pub(crate) amplitude_threshold: f32,
    /// Unvoiced DFT from the previous frame.
    pub(crate) unvoiced: UnvoicedDft,
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
            error_rate: 0.0,
            energy: FrameEnergy::default(),
            amplitude_threshold: 0.0,
            unvoiced: UnvoicedDft::default(),
            phase_base: PhaseBase::default(),
            phase: Phase::default(),
        }
    }
}

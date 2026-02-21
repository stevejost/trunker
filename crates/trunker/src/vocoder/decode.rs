//! Top-level IMBE frame decoder.
//!
//! Decodes a stream of IMBE voice frames into 8 kHz PCM audio samples.
//! The decoder maintains inter-frame state for spectral prediction,
//! phase continuity, and error rate tracking.

use super::coefs::Coefficients;
use super::descramble::{Bootstrap, descramble};
use super::enhance::{self, EnhanceErrors, EnhancedSpectrals, FrameEnergy};
use super::frame::{AudioBuffer, ReceivedFrame};
use super::gain::Gains;
use super::params::BaseParams;
use super::prev::PrevFrame;
use super::spectral::Spectrals;
use super::unvoiced::{Unvoiced, UnvoicedDft};
use super::voiced::{Phase, PhaseBase, Voiced};

/// Decodes a stream of IMBE frames into PCM audio.
///
/// Call [`decode`](ImbeDecoder::decode) once per received voice frame.
/// The decoder tracks inter-frame state (spectral prediction, phase
/// accumulation, error rate) internally.
#[derive(Default)]
pub struct ImbeDecoder {
    /// Saved parameters from the previous frame.
    prev: PrevFrame,
}

impl ImbeDecoder {
    /// Create a new `ImbeDecoder` in the default state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Decode the given frame into the given audio sample buffer.
    ///
    /// On success, fills `buffer` with 160 PCM samples. On invalid input
    /// or high error rates, either repeats the previous frame or outputs
    /// silence per TIA-102.BABA error handling rules.
    pub fn decode(&mut self, frame: ReceivedFrame, buffer: &mut AudioBuffer) {
        let period = match Bootstrap::new(&frame.chunks) {
            Bootstrap::Period(p) => p,
            Bootstrap::Invalid => {
                // Repeat previous frame on invalid period [p46].
                self.repeat(buffer);
                return;
            }
            Bootstrap::Silence => {
                self.silence(buffer);
                return;
            }
        };

        let errors = EnhanceErrors::new(&frame.errors, self.prev.error_rate);

        if enhance::should_repeat(&errors) {
            self.repeat(buffer);
            return;
        }

        if enhance::should_mute(&errors) {
            self.silence(buffer);
            return;
        }

        let params = BaseParams::new(period);

        // Descramble chunks into quantized amplitudes, voice decisions, and gain index.
        // A scan exhaustion error indicates a logic bug, not an RF condition.
        // Repeat the previous frame rather than panicking on corrupted data.
        let (amplitudes, mut voice, gain_index) = match descramble(&frame.chunks, &params) {
            Ok(result) => result,
            Err(_) => {
                self.repeat(buffer);
                return;
            }
        };

        let gains = Gains::new(gain_index, &amplitudes, &params);
        let coefficients = Coefficients::new(&gains, &amplitudes, &params);
        let spectrals = Spectrals::new(&coefficients, &params, &self.prev);
        let energy = FrameEnergy::new(&spectrals, &self.prev.energy, &params);

        let mut enhanced = EnhancedSpectrals::new(&spectrals, &energy, &params);
        let threshold = enhance::amplitude_threshold(&errors, self.prev.amplitude_threshold);
        enhance::smooth(&mut enhanced, &mut voice, &errors, &energy, threshold);

        let unvoiced_dft = UnvoicedDft::new(&params, &voice, &enhanced, rand::rng());
        let phase_base = PhaseBase::new(&params, &self.prev);
        let phase = Phase::new(&phase_base, &params, &self.prev, &voice, rand::rng());

        // Synthesize audio samples (Eq 142): s(n) = s_v(n) + s_uv(n).
        let unvoiced = Unvoiced::new(&unvoiced_dft, &self.prev.unvoiced);
        let voiced = Voiced::new(&params, &self.prev, &phase, &enhanced, &voice);

        for (n, sample) in buffer.iter_mut().enumerate() {
            *sample = unvoiced.get(n) + voiced.get(n);
        }

        // Save current parameters for next frame.
        self.prev = PrevFrame {
            params,
            spectrals,
            enhanced,
            voice,
            error_rate: errors.rate,
            energy,
            amplitude_threshold: threshold,
            unvoiced: unvoiced_dft,
            phase_base,
            phase,
        };
    }

    /// Fill the given audio buffer with silence.
    ///
    /// Does not update saved state — the next frame will still reference
    /// the parameters from before the silence frame.
    fn silence(&self, buffer: &mut AudioBuffer) {
        buffer.fill(0.0);
    }

    /// Repeat the previous frame into the given audio buffer.
    ///
    /// Reuses the previous frame's parameters, voice decisions, and enhanced
    /// spectrals, but generates fresh randomness for the unvoiced DFT and
    /// phase terms (Eqs 99-104). Does not update saved state.
    fn repeat(&self, buffer: &mut AudioBuffer) {
        let unvoiced_dft = UnvoicedDft::new(
            &self.prev.params,
            &self.prev.voice,
            &self.prev.enhanced,
            rand::rng(),
        );
        let phase_base = PhaseBase::new(&self.prev.params, &self.prev);
        let phase = Phase::new(
            &phase_base,
            &self.prev.params,
            &self.prev,
            &self.prev.voice,
            rand::rng(),
        );

        let unvoiced = Unvoiced::new(&unvoiced_dft, &self.prev.unvoiced);
        let voiced = Voiced::new(
            &self.prev.params,
            &self.prev,
            &phase,
            &self.prev.enhanced,
            &self.prev.voice,
        );

        for (n, sample) in buffer.iter_mut().enumerate() {
            *sample = unvoiced.get(n) + voiced.get(n);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vocoder::consts::SAMPLES_PER_FRAME;

    /// Standard test chunks that produce L=16, valid voice frame.
    fn test_frame() -> ReceivedFrame {
        ReceivedFrame::new(
            [
                0b001000010010,
                0b110011001100,
                0b111000111000,
                0b111111111111,
                0b10100110101,
                0b00101111010,
                0b01110111011,
                0b00001000,
            ],
            [0; 7],
        )
    }

    #[test]
    fn single_frame_produces_valid_audio() {
        let mut decoder = ImbeDecoder::new();
        let mut buffer = [0.0f32; SAMPLES_PER_FRAME];

        decoder.decode(test_frame(), &mut buffer);

        // All samples should be finite (no NaN or infinity).
        for (n, &sample) in buffer.iter().enumerate() {
            assert!(sample.is_finite(), "sample {n} is not finite: {sample}");
        }

        // At least some samples should be non-zero.
        let has_nonzero = buffer.iter().any(|&s| s != 0.0);
        assert!(has_nonzero, "expected non-zero audio output");
    }

    #[test]
    fn two_frames_differ() {
        let mut decoder = ImbeDecoder::new();
        let mut buffer1 = [0.0f32; SAMPLES_PER_FRAME];
        let mut buffer2 = [0.0f32; SAMPLES_PER_FRAME];

        decoder.decode(test_frame(), &mut buffer1);
        decoder.decode(test_frame(), &mut buffer2);

        // Second frame should differ from first due to inter-frame
        // prediction and phase accumulation.
        let differs = buffer1
            .iter()
            .zip(buffer2.iter())
            .any(|(&a, &b)| (a - b).abs() > 1e-6);
        assert!(differs, "second frame should differ from first");
    }

    #[test]
    fn invalid_period_triggers_repeat() {
        let mut decoder = ImbeDecoder::new();
        let mut buffer = [0.0f32; SAMPLES_PER_FRAME];

        // First decode a valid frame to populate state.
        decoder.decode(test_frame(), &mut buffer);

        // Invalid period value (b_0 = 208..=215 is invalid).
        // u_0 MSBs = 110100 (0xD0 >> 2 = 52), u_7 bits = 00 -> period = 0b11010000 = 208
        let invalid_frame =
            ReceivedFrame::new([0b110100000000, 0, 0, 0, 0, 0, 0, 0b0000000], [0; 7]);

        decoder.decode(invalid_frame, &mut buffer);

        // Should not panic and should produce finite output.
        for (n, &sample) in buffer.iter().enumerate() {
            assert!(sample.is_finite(), "sample {n} is not finite: {sample}");
        }
    }

    #[test]
    fn silence_frame_produces_zeros() {
        let mut decoder = ImbeDecoder::new();
        let mut buffer = [1.0f32; SAMPLES_PER_FRAME]; // pre-fill with non-zero

        // Silence period: b_0 = 216..=219.
        // u_0 MSBs = 110110 (0xD8 >> 2 = 54), u_7 bits = 00 -> period = 0b11011000 = 216
        let silence_frame =
            ReceivedFrame::new([0b110110000000, 0, 0, 0, 0, 0, 0, 0b0000000], [0; 7]);

        decoder.decode(silence_frame, &mut buffer);

        for (n, &sample) in buffer.iter().enumerate() {
            assert_eq!(sample, 0.0, "silence should produce zero at sample {n}");
        }
    }

    #[test]
    fn silence_preserves_state_for_next_frame() {
        let mut decoder = ImbeDecoder::new();
        let mut buffer1 = [0.0f32; SAMPLES_PER_FRAME];
        let mut buffer2 = [0.0f32; SAMPLES_PER_FRAME];
        let mut buffer3 = [0.0f32; SAMPLES_PER_FRAME];

        // Decode a valid frame.
        decoder.decode(test_frame(), &mut buffer1);

        // Decode silence — should not update state.
        let silence_frame =
            ReceivedFrame::new([0b110110000000, 0, 0, 0, 0, 0, 0, 0b0000000], [0; 7]);
        decoder.decode(silence_frame, &mut buffer2);
        assert!(buffer2.iter().all(|&s| s == 0.0));

        // Decode another valid frame — should still work (state preserved).
        decoder.decode(test_frame(), &mut buffer3);

        for (n, &sample) in buffer3.iter().enumerate() {
            assert!(sample.is_finite(), "sample {n} is not finite after silence");
        }
        let has_nonzero = buffer3.iter().any(|&s| s != 0.0);
        assert!(has_nonzero, "expected non-zero output after silence");
    }
}

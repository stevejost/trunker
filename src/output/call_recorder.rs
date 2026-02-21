//! Per-call audio recording tied to voice call lifecycle.
//!
//! Combines [`CallTracker`] with [`WavWriter`] to automatically record
//! each voice call to a separate WAV file. Files are placed in the
//! output directory following the `{date}/{talkgroup}_{timestamp}.wav`
//! convention from [`call_audio`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::call_tracker::{Call, CallTracker, EndReason};
use crate::channel_manager::VoiceChannelEvent;
use crate::output::call_audio;
use crate::output::wav::WavWriter;
use crate::p25::types::{Frequency, SourceId, TalkgroupId};

/// Metadata emitted when a call recording completes.
#[derive(Debug, Clone)]
pub struct CompletedRecording {
    /// Talkgroup for this call.
    pub talkgroup: TalkgroupId,
    /// Source unit that initiated the call.
    pub source: SourceId,
    /// Receive frequency in hertz.
    pub frequency: Frequency,
    /// Number of decoded voice frames.
    pub frame_count: u32,
    /// Why the call ended.
    pub end_reason: EndReason,
    /// Path to the recorded WAV file, if audio was captured.
    pub audio_file: Option<PathBuf>,
}

/// Records voice calls to per-call WAV files.
///
/// Wraps a [`CallTracker`] and manages one [`WavWriter`] per active
/// call. Audio samples from [`VoiceChannelEvent`]s are written to
/// the appropriate WAV file. When a call ends, the WAV header is
/// finalized and a [`CompletedRecording`] is returned.
pub struct CallRecorder {
    tracker: CallTracker,
    /// Output directory root for WAV files.
    output_dir: PathBuf,
    /// Active WAV writers keyed by frequency.
    writers: HashMap<Frequency, WavWriter>,
    /// Paths of active WAV files keyed by frequency.
    paths: HashMap<Frequency, PathBuf>,
}

impl CallRecorder {
    /// Create a new call recorder writing to the given output directory.
    pub fn new(output_dir: &Path) -> Self {
        Self {
            tracker: CallTracker::new(),
            output_dir: output_dir.to_path_buf(),
            writers: HashMap::new(),
            paths: HashMap::new(),
        }
    }

    /// Start recording a new call.
    ///
    /// Prepares a WAV file path but defers file creation until the first
    /// audio data arrives via [`process_event`]. If a recording already
    /// exists at this frequency, it is finalized and returned as a
    /// completed recording.
    pub fn start_call(
        &mut self,
        frequency: Frequency,
        talkgroup: TalkgroupId,
        source: SourceId,
    ) -> Option<CompletedRecording> {
        // Finalize any existing WAV writer at this frequency.
        let audio_file = if self.writers.contains_key(&frequency) {
            self.finalize_writer(frequency)
        } else {
            // Clean up any pending path that never got audio.
            self.paths.remove(&frequency);
            None
        };

        // Start tracking; get the displaced call if one existed.
        let completed = self
            .tracker
            .start_call(frequency, talkgroup, source)
            .map(|displaced| CompletedRecording {
                talkgroup: displaced.talkgroup,
                source: displaced.source,
                frequency: displaced.frequency,
                frame_count: displaced.frame_count,
                end_reason: displaced.end_reason.unwrap_or_else(|| {
                    tracing::warn!("displaced call missing end_reason, defaulting to Timeout");
                    EndReason::Timeout
                }),
                audio_file,
            });

        // Store the intended path; the file is created lazily on first audio.
        let timestamp = SystemTime::now();
        match call_audio::call_audio_path(&self.output_dir, talkgroup, timestamp) {
            Ok(path) => {
                self.paths.insert(frequency, path);
            }
            Err(e) => {
                tracing::warn!(
                    talkgroup = %talkgroup,
                    error = %e,
                    "failed to create output directory"
                );
            }
        }

        completed
    }

    /// Process a voice channel event, writing audio and tracking state.
    ///
    /// The WAV file is created lazily on the first event that carries
    /// audio data, avoiding empty 44-byte header-only files for calls
    /// that are terminated before any voice frames arrive.
    ///
    /// Returns `Some(recording)` when a call ends.
    pub fn process_event(&mut self, event: &VoiceChannelEvent) -> Option<CompletedRecording> {
        // Write audio samples if available.
        if let Some(audio) = &event.audio {
            // Lazily create the WAV writer on first audio.
            if !self.writers.contains_key(&event.frequency) {
                if let Some(path) = self.paths.get(&event.frequency) {
                    match WavWriter::create(path) {
                        Ok(writer) => {
                            tracing::info!(
                                frequency = %event.frequency,
                                path = %path.display(),
                                "recording started"
                            );
                            self.writers.insert(event.frequency, writer);
                        }
                        Err(e) => {
                            tracing::warn!(
                                path = %path.display(),
                                error = %e,
                                "failed to create WAV file"
                            );
                        }
                    }
                }
            }

            if let Some(writer) = self.writers.get_mut(&event.frequency)
                && let Err(e) = writer.write_samples(audio)
            {
                tracing::warn!(
                    frequency = %event.frequency,
                    error = %e,
                    "failed to write audio samples"
                );
            }
        }

        // Update call state and check for termination.
        let ended_call = self.tracker.process_event(event)?;
        Some(self.complete_recording(ended_call))
    }

    /// End a call due to grant timeout.
    ///
    /// Returns the completed recording metadata.
    pub fn timeout_call(&mut self, frequency: Frequency) -> Option<CompletedRecording> {
        let ended_call = self.tracker.timeout_call(frequency)?;
        Some(self.complete_recording(ended_call))
    }

    /// Finalize all open recordings (for graceful shutdown).
    ///
    /// Returns metadata for all recordings that were in progress.
    pub fn finalize_all(&mut self) -> Vec<CompletedRecording> {
        let frequencies: Vec<Frequency> = self.writers.keys().copied().collect();
        let mut recordings = Vec::new();

        for frequency in frequencies {
            let audio_file = self.finalize_writer(frequency);
            // Build a recording from whatever call state we have.
            if let Some(call) = self.tracker.timeout_call(frequency) {
                recordings.push(CompletedRecording {
                    talkgroup: call.talkgroup,
                    source: call.source,
                    frequency: call.frequency,
                    frame_count: call.frame_count,
                    end_reason: call.end_reason.unwrap_or_else(|| {
                        tracing::warn!("finalized call missing end_reason, defaulting to Timeout");
                        EndReason::Timeout
                    }),
                    audio_file,
                });
            }
        }

        recordings
    }

    /// Return the number of currently active recordings.
    pub fn active_recording_count(&self) -> usize {
        self.writers.len()
    }

    /// Check whether a recording is active at the given frequency.
    pub fn has_active_recording(&self, frequency: &Frequency) -> bool {
        self.writers.contains_key(frequency)
    }

    /// Finalize a WAV writer and return the file path.
    ///
    /// Returns `None` if no audio was ever written (no WAV file was created).
    fn finalize_writer(&mut self, frequency: Frequency) -> Option<PathBuf> {
        let path = self.paths.remove(&frequency);
        if let Some(writer) = self.writers.remove(&frequency) {
            if let Err(e) = writer.finalize() {
                tracing::warn!(
                    frequency = %frequency,
                    error = %e,
                    "failed to finalize WAV file"
                );
            }
            path
        } else {
            // No writer was created — no audio was received. No file exists.
            None
        }
    }

    /// Build a CompletedRecording from an ended call.
    fn complete_recording(&mut self, call: Call) -> CompletedRecording {
        let audio_file = self.finalize_writer(call.frequency);
        CompletedRecording {
            talkgroup: call.talkgroup,
            source: call.source,
            frequency: call.frequency,
            frame_count: call.frame_count,
            end_reason: call.end_reason.unwrap_or_else(|| {
                tracing::warn!("completed call missing end_reason, defaulting to Timeout");
                EndReason::Timeout
            }),
            audio_file,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p25::nid::{DataUnit, NetworkId};
    use crate::p25::receiver::ReceiverEvent;
    use crate::p25::types::Nac;
    use crate::p25::voice::frame::VoiceFrame;
    use crate::vocoder::{AudioBuffer, ImbeDecoder, ReceivedFrame, SAMPLES_PER_FRAME};

    fn test_frequency() -> Frequency {
        Frequency::from_hz(851_062_500)
    }

    fn test_talkgroup() -> TalkgroupId {
        TalkgroupId::new(100)
    }

    fn test_source() -> SourceId {
        SourceId::new(12345)
    }

    fn make_voice_event_with_audio(frequency: Frequency) -> VoiceChannelEvent {
        VoiceChannelEvent {
            frequency,
            talkgroup: test_talkgroup(),
            source: test_source(),
            nac: Nac::new(0x293),
            event: ReceiverEvent::VoiceFrame(VoiceFrame {
                chunks: [0; 8],
                errors: [0; 7],
            }),
            audio: Some(vec![0.0; 160]),
        }
    }

    fn make_terminator_event(frequency: Frequency) -> VoiceChannelEvent {
        VoiceChannelEvent {
            frequency,
            talkgroup: test_talkgroup(),
            source: test_source(),
            nac: Nac::new(0x293),
            event: ReceiverEvent::Nid(NetworkId {
                access_code: Nac::new(0x293),
                data_unit: DataUnit::VoiceLcTerminator,
                parity_ok: true,
            }),
            audio: None,
        }
    }

    #[test]
    fn new_recorder_has_no_active_recordings() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = CallRecorder::new(dir.path());
        assert_eq!(recorder.active_recording_count(), 0);
    }

    #[test]
    fn start_call_defers_wav_creation() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = CallRecorder::new(dir.path());

        recorder.start_call(test_frequency(), test_talkgroup(), test_source());

        // Path is prepared but file is NOT created until audio arrives.
        assert_eq!(recorder.active_recording_count(), 0);
        let path = recorder.paths.get(&test_frequency()).unwrap();
        assert!(!path.exists(), "WAV file should not exist until audio arrives");
        assert!(
            path.extension().is_some_and(|ext| ext == "wav"),
            "path should have .wav extension"
        );
    }

    #[test]
    fn voice_event_writes_audio_to_wav() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = CallRecorder::new(dir.path());
        let freq = test_frequency();

        recorder.start_call(freq, test_talkgroup(), test_source());
        recorder.process_event(&make_voice_event_with_audio(freq));

        // Finalize by ending the call (flushes BufWriter).
        let completed = recorder.timeout_call(freq).unwrap();
        let path = completed.audio_file.unwrap();

        // File should contain 44-byte header + 160 samples * 2 bytes = 364 bytes.
        let metadata = std::fs::metadata(&path).unwrap();
        assert_eq!(
            metadata.len(),
            364,
            "WAV file should be 364 bytes (header + 160 PCM samples)"
        );
    }

    #[test]
    fn terminator_ends_recording() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = CallRecorder::new(dir.path());
        let freq = test_frequency();

        recorder.start_call(freq, test_talkgroup(), test_source());
        recorder.process_event(&make_voice_event_with_audio(freq));
        let completed = recorder.process_event(&make_terminator_event(freq));

        let recording = completed.expect("should return completed recording");
        assert_eq!(recording.talkgroup, test_talkgroup());
        assert_eq!(recording.end_reason, EndReason::LcTerminator);
        assert!(recording.audio_file.is_some());
        assert_eq!(recorder.active_recording_count(), 0);

        // WAV file should still exist on disk.
        let path = recording.audio_file.unwrap();
        assert!(path.exists(), "finalized WAV should exist");
    }

    #[test]
    fn timeout_without_audio_produces_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = CallRecorder::new(dir.path());
        let freq = test_frequency();

        recorder.start_call(freq, test_talkgroup(), test_source());
        let completed = recorder.timeout_call(freq);

        let recording = completed.expect("should return completed recording");
        assert_eq!(recording.end_reason, EndReason::Timeout);
        assert!(
            recording.audio_file.is_none(),
            "no audio was written, so no file should exist"
        );
        assert_eq!(recorder.active_recording_count(), 0);
    }

    #[test]
    fn finalize_all_closes_open_recordings() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = CallRecorder::new(dir.path());

        let freq_a = Frequency::from_hz(851_062_500);
        let freq_b = Frequency::from_hz(851_068_750);

        recorder.start_call(freq_a, TalkgroupId::new(100), test_source());
        recorder.start_call(freq_b, TalkgroupId::new(200), test_source());

        // Send audio to both so writers are created.
        recorder.process_event(&make_voice_event_with_audio(freq_a));
        recorder.process_event(&make_voice_event_with_audio(freq_b));
        assert_eq!(recorder.active_recording_count(), 2);

        let recordings = recorder.finalize_all();
        assert_eq!(recordings.len(), 2);
        assert_eq!(recorder.active_recording_count(), 0);

        for recording in &recordings {
            assert!(recording.audio_file.is_some());
            assert!(recording.audio_file.as_ref().unwrap().exists());
        }
    }

    #[test]
    fn start_call_returns_displaced_recording() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = CallRecorder::new(dir.path());
        let freq = test_frequency();

        // First call: no displacement.
        let displaced = recorder.start_call(freq, TalkgroupId::new(100), test_source());
        assert!(
            displaced.is_none(),
            "first call should not displace anything"
        );

        // Write some audio so the first call has a file.
        recorder.process_event(&make_voice_event_with_audio(freq));
        let first_path = recorder.paths.get(&freq).unwrap().clone();
        assert!(first_path.exists(), "first WAV should exist after audio");

        // Starting a new call at the same frequency finalizes the old one.
        let displaced = recorder.start_call(freq, TalkgroupId::new(200), test_source());
        let recording = displaced.expect("should return displaced recording");
        assert_eq!(recording.talkgroup, TalkgroupId::new(100));
        assert_eq!(recording.end_reason, EndReason::Timeout);
        assert!(recording.audio_file.is_some());

        let second_path = recorder.paths.get(&freq).unwrap().clone();
        assert_eq!(recorder.active_recording_count(), 0, "no audio yet for new call");
        assert!(first_path.exists(), "first WAV should be finalized on disk");
        assert!(!second_path.exists(), "second WAV should not exist until audio arrives");
    }

    #[test]
    fn event_without_audio_does_not_create_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = CallRecorder::new(dir.path());
        let freq = test_frequency();

        recorder.start_call(freq, test_talkgroup(), test_source());
        let path = recorder.paths.get(&freq).unwrap().clone();

        // Event with no audio (e.g., NID event) should not create the file.
        let event = VoiceChannelEvent {
            frequency: freq,
            talkgroup: test_talkgroup(),
            source: test_source(),
            nac: Nac::new(0x293),
            event: ReceiverEvent::Nid(NetworkId {
                access_code: Nac::new(0x293),
                data_unit: DataUnit::VoiceLcFrameGroup,
                parity_ok: true,
            }),
            audio: None,
        };
        recorder.process_event(&event);

        assert!(!path.exists(), "file should not be created without audio");
    }

    #[test]
    fn vocoder_decoded_audio_produces_non_silent_wav() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = CallRecorder::new(dir.path());
        let freq = test_frequency();

        recorder.start_call(freq, test_talkgroup(), test_source());

        // Decode a real IMBE voice frame through the vocoder.
        let mut decoder = ImbeDecoder::new();
        let frame = ReceivedFrame::new(
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
        );
        let mut buffer: AudioBuffer = [0.0; SAMPLES_PER_FRAME];
        decoder.decode(frame, &mut buffer);

        // Feed vocoder output through the recorder.
        let event = VoiceChannelEvent {
            frequency: freq,
            talkgroup: test_talkgroup(),
            source: test_source(),
            nac: Nac::new(0x293),
            event: ReceiverEvent::VoiceFrame(VoiceFrame {
                chunks: [0; 8],
                errors: [0; 7],
            }),
            audio: Some(buffer.to_vec()),
        };
        recorder.process_event(&event);

        // Finalize the recording.
        let completed = recorder.timeout_call(freq).unwrap();
        let path = completed.audio_file.unwrap();

        // WAV should contain header + audio data.
        let file_size = std::fs::metadata(&path).unwrap().len();
        assert_eq!(
            file_size, 364,
            "WAV file should be 44-byte header + 160 samples * 2 bytes"
        );

        // Read the PCM samples from the WAV file (skip 44-byte header).
        let file_data = std::fs::read(&path).unwrap();
        let pcm_data = &file_data[44..];
        let samples: Vec<i16> = pcm_data
            .chunks_exact(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();

        assert_eq!(samples.len(), 160);

        // At least some samples should be non-zero (real IMBE produces audio).
        let has_nonzero = samples.iter().any(|&s| s != 0);
        assert!(
            has_nonzero,
            "vocoder-decoded audio should produce non-silent WAV output"
        );
    }
}

//! Per-call audio recording tied to voice call lifecycle.
//!
//! Combines [`CallTracker`] with [`AudioWriter`] to automatically record
//! each voice call to a separate audio file. Files are placed in the
//! output directory following the `{date}/{talkgroup}_{timestamp}.{ext}`
//! convention from [`call_audio`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::call_tracker::{Call, CallTracker, EndReason};
use crate::channel_manager::VoiceChannelEvent;
use crate::output::call_audio;
use crate::output::call_writer::{AudioFormat, AudioWriter};
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
    /// Path to the recorded audio file, if audio was captured.
    pub audio_file: Option<PathBuf>,
}

/// State of a recording at a given frequency.
///
/// Transitions from `Pending` to `Active` on first audio data.
/// Both states count as "active" for duplicate-start suppression.
enum RecordingState {
    /// Path allocated, waiting for first audio data.
    Pending(PathBuf),
    /// Audio file open and being written.
    Active(PathBuf, AudioWriter),
}

/// Records voice calls to per-call audio files.
///
/// Wraps a [`CallTracker`] and manages one [`RecordingState`] per active
/// call. Audio samples from [`VoiceChannelEvent`]s are written to
/// the appropriate audio file. When a call ends, the file is
/// finalized and a [`CompletedRecording`] is returned.
pub struct CallRecorder {
    tracker: CallTracker,
    /// Output directory root for audio files.
    output_dir: PathBuf,
    /// Audio encoding format (WAV or Opus).
    audio_format: AudioFormat,
    /// Recording state per frequency.
    recordings: HashMap<Frequency, RecordingState>,
}

impl CallRecorder {
    /// Create a new call recorder writing to the given output directory.
    pub fn new(output_dir: &Path, audio_format: AudioFormat) -> Self {
        Self {
            tracker: CallTracker::new(),
            output_dir: output_dir.to_path_buf(),
            audio_format,
            recordings: HashMap::new(),
        }
    }

    /// Start recording a new call.
    ///
    /// Prepares an audio file path but defers file creation until the first
    /// audio data arrives via [`process_event`]. If a recording already
    /// exists at this frequency, it is finalized and returned as a
    /// completed recording.
    pub fn start_call(
        &mut self,
        frequency: Frequency,
        talkgroup: TalkgroupId,
        source: SourceId,
    ) -> Option<CompletedRecording> {
        // Finalize any existing recording at this frequency.
        let audio_file = self.finalize_recording(frequency);

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
        let extension = self.audio_format.extension();
        match call_audio::call_audio_path(&self.output_dir, talkgroup, timestamp, extension) {
            Ok(path) => {
                self.recordings
                    .insert(frequency, RecordingState::Pending(path));
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
    /// The audio file is created lazily on the first event that carries
    /// audio data, avoiding empty files for calls that are terminated
    /// before any voice frames arrive.
    ///
    /// Returns `Some(recording)` when a call ends.
    pub fn process_event(&mut self, event: &VoiceChannelEvent) -> Option<CompletedRecording> {
        // Write audio samples if available.
        if let Some(audio) = &event.audio {
            // Transition Pending → Active on first audio.
            if let Some(RecordingState::Pending(_)) = self.recordings.get(&event.frequency) {
                // Take ownership to transition state.
                if let Some(RecordingState::Pending(path)) =
                    self.recordings.remove(&event.frequency)
                {
                    match AudioWriter::create(self.audio_format, &path) {
                        Ok(writer) => {
                            tracing::info!(
                                frequency = %event.frequency,
                                path = %path.display(),
                                format = ?self.audio_format,
                                "recording started"
                            );
                            self.recordings
                                .insert(event.frequency, RecordingState::Active(path, writer));
                        }
                        Err(e) => {
                            tracing::warn!(
                                path = %path.display(),
                                error = %e,
                                "failed to create audio file"
                            );
                        }
                    }
                }
            }

            if let Some(RecordingState::Active(_, writer)) =
                self.recordings.get_mut(&event.frequency)
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
    /// Returns metadata for all recordings that were in progress,
    /// including pending recordings that never received audio.
    pub fn finalize_all(&mut self) -> Vec<CompletedRecording> {
        let frequencies: Vec<Frequency> = self.recordings.keys().copied().collect();
        let mut completed = Vec::new();

        for frequency in frequencies {
            let audio_file = self.finalize_recording(frequency);
            if let Some(call) = self.tracker.timeout_call(frequency) {
                completed.push(CompletedRecording {
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

        completed
    }

    /// Return the number of currently active recordings (pending or active).
    pub fn active_recording_count(&self) -> usize {
        self.recordings.len()
    }

    /// Check whether a recording exists at the given frequency.
    pub fn has_active_recording(&self, frequency: &Frequency) -> bool {
        self.recordings.contains_key(frequency)
    }

    /// Finalize a recording and return the file path if audio was written.
    ///
    /// Returns `None` if the recording was still pending (no audio received).
    fn finalize_recording(&mut self, frequency: Frequency) -> Option<PathBuf> {
        match self.recordings.remove(&frequency) {
            Some(RecordingState::Active(path, writer)) => {
                if let Err(e) = writer.finalize() {
                    tracing::warn!(
                        frequency = %frequency,
                        error = %e,
                        "failed to finalize audio file"
                    );
                }
                Some(path)
            }
            Some(RecordingState::Pending(_)) => {
                // No writer was created -- no audio was received. No file exists.
                None
            }
            None => None,
        }
    }

    /// Build a CompletedRecording from an ended call.
    fn complete_recording(&mut self, call: Call) -> CompletedRecording {
        let audio_file = self.finalize_recording(call.frequency);
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
        let recorder = CallRecorder::new(dir.path(), AudioFormat::Wav);
        assert_eq!(recorder.active_recording_count(), 0);
    }

    /// Extract the path from a RecordingState.
    fn recording_path(state: &RecordingState) -> &Path {
        match state {
            RecordingState::Pending(p) | RecordingState::Active(p, _) => p,
        }
    }

    #[test]
    fn start_call_defers_wav_creation() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = CallRecorder::new(dir.path(), AudioFormat::Wav);

        recorder.start_call(test_frequency(), test_talkgroup(), test_source());

        // Recording is pending (counted as active) but file is NOT created yet.
        assert_eq!(recorder.active_recording_count(), 1);
        assert!(recorder.has_active_recording(&test_frequency()));
        let state = recorder.recordings.get(&test_frequency()).unwrap();
        assert!(matches!(state, RecordingState::Pending(_)));
        let path = recording_path(state);
        assert!(
            !path.exists(),
            "audio file should not exist until audio arrives"
        );
        assert!(
            path.extension().is_some_and(|ext| ext == "wav"),
            "path should have .wav extension"
        );
    }

    #[test]
    fn start_call_defers_opus_creation() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = CallRecorder::new(dir.path(), AudioFormat::Opus);

        recorder.start_call(test_frequency(), test_talkgroup(), test_source());

        assert_eq!(recorder.active_recording_count(), 1);
        assert!(recorder.has_active_recording(&test_frequency()));
        let state = recorder.recordings.get(&test_frequency()).unwrap();
        assert!(matches!(state, RecordingState::Pending(_)));
        let path = recording_path(state);
        assert!(
            !path.exists(),
            "audio file should not exist until audio arrives"
        );
        assert!(
            path.extension().is_some_and(|ext| ext == "opus"),
            "path should have .opus extension"
        );
    }

    #[test]
    fn voice_event_writes_audio_to_wav() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = CallRecorder::new(dir.path(), AudioFormat::Wav);
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
    fn voice_event_writes_audio_to_opus() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = CallRecorder::new(dir.path(), AudioFormat::Opus);
        let freq = test_frequency();

        recorder.start_call(freq, test_talkgroup(), test_source());
        recorder.process_event(&make_voice_event_with_audio(freq));

        let completed = recorder.timeout_call(freq).unwrap();
        let path = completed.audio_file.unwrap();

        let data = std::fs::read(&path).unwrap();
        assert_eq!(&data[0..4], b"OggS", "Opus file should start with OggS");
        assert!(data.len() > 100, "Opus file with audio should not be tiny");
        assert!(
            path.extension().is_some_and(|ext| ext == "opus"),
            "file should have .opus extension"
        );
    }

    #[test]
    fn terminator_ends_recording() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = CallRecorder::new(dir.path(), AudioFormat::Wav);
        let freq = test_frequency();

        recorder.start_call(freq, test_talkgroup(), test_source());
        recorder.process_event(&make_voice_event_with_audio(freq));
        let completed = recorder.process_event(&make_terminator_event(freq));

        let recording = completed.expect("should return completed recording");
        assert_eq!(recording.talkgroup, test_talkgroup());
        assert_eq!(recording.end_reason, EndReason::LcTerminator);
        assert!(recording.audio_file.is_some());
        assert_eq!(recorder.active_recording_count(), 0);

        // Audio file should still exist on disk.
        let path = recording.audio_file.unwrap();
        assert!(path.exists(), "finalized audio file should exist");
    }

    #[test]
    fn timeout_without_audio_produces_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = CallRecorder::new(dir.path(), AudioFormat::Wav);
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
        let mut recorder = CallRecorder::new(dir.path(), AudioFormat::Wav);

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
        let mut recorder = CallRecorder::new(dir.path(), AudioFormat::Wav);
        let freq = test_frequency();

        // First call: no displacement.
        let displaced = recorder.start_call(freq, TalkgroupId::new(100), test_source());
        assert!(
            displaced.is_none(),
            "first call should not displace anything"
        );

        // Write some audio so the first call has a file.
        recorder.process_event(&make_voice_event_with_audio(freq));
        let first_path = recording_path(recorder.recordings.get(&freq).unwrap()).to_path_buf();
        assert!(first_path.exists(), "first file should exist after audio");

        // Starting a new call at the same frequency finalizes the old one.
        let displaced = recorder.start_call(freq, TalkgroupId::new(200), test_source());
        let recording = displaced.expect("should return displaced recording");
        assert_eq!(recording.talkgroup, TalkgroupId::new(100));
        assert_eq!(recording.end_reason, EndReason::Timeout);
        assert!(recording.audio_file.is_some());

        let second_path =
            recording_path(recorder.recordings.get(&freq).unwrap()).to_path_buf();
        assert_eq!(
            recorder.active_recording_count(),
            1,
            "new call is pending"
        );
        assert!(
            first_path.exists(),
            "first file should be finalized on disk"
        );
        assert!(
            !second_path.exists(),
            "second file should not exist until audio arrives"
        );
    }

    #[test]
    fn event_without_audio_does_not_create_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = CallRecorder::new(dir.path(), AudioFormat::Wav);
        let freq = test_frequency();

        recorder.start_call(freq, test_talkgroup(), test_source());
        let path = recording_path(recorder.recordings.get(&freq).unwrap()).to_path_buf();

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
        let mut recorder = CallRecorder::new(dir.path(), AudioFormat::Wav);
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

    #[test]
    fn grant_update_before_audio_does_not_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = CallRecorder::new(dir.path(), AudioFormat::Wav);
        let freq = test_frequency();

        // Initial grant starts the recording (pending state).
        recorder.start_call(freq, test_talkgroup(), test_source());
        assert!(recorder.has_active_recording(&freq));
        assert_eq!(recorder.active_recording_count(), 1);
        let original_path =
            recording_path(recorder.recordings.get(&freq).unwrap()).to_path_buf();

        // Simulate a grant update arriving before first audio — the caller
        // checks has_active_recording() and should NOT call start_call again.
        assert!(
            recorder.has_active_recording(&freq),
            "pending recording must block duplicate start"
        );

        // Now audio arrives and transitions Pending → Active.
        recorder.process_event(&make_voice_event_with_audio(freq));
        assert!(matches!(
            recorder.recordings.get(&freq),
            Some(RecordingState::Active(_, _))
        ));
        let active_path =
            recording_path(recorder.recordings.get(&freq).unwrap()).to_path_buf();
        assert_eq!(original_path, active_path, "path should not change");
    }

    #[test]
    fn finalize_all_includes_pending_recordings() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = CallRecorder::new(dir.path(), AudioFormat::Wav);

        let freq_a = Frequency::from_hz(851_062_500);
        let freq_b = Frequency::from_hz(851_068_750);

        recorder.start_call(freq_a, TalkgroupId::new(100), test_source());
        recorder.start_call(freq_b, TalkgroupId::new(200), test_source());

        // Only send audio to freq_a; freq_b stays pending.
        recorder.process_event(&make_voice_event_with_audio(freq_a));
        assert_eq!(recorder.active_recording_count(), 2);

        let recordings = recorder.finalize_all();
        // Both should be finalized: one with audio, one without.
        assert_eq!(recordings.len(), 2);
        assert_eq!(recorder.active_recording_count(), 0);

        let with_audio = recordings.iter().find(|r| r.talkgroup == TalkgroupId::new(100)).unwrap();
        assert!(with_audio.audio_file.is_some(), "active recording should have a file");

        let without_audio = recordings.iter().find(|r| r.talkgroup == TalkgroupId::new(200)).unwrap();
        assert!(without_audio.audio_file.is_none(), "pending recording should have no file");
    }
}

//! Meeting Notes AI: import of audio files (e.g. iPhone/Mac voice memos) as
//! regular meetings.
//!
//! The file is decoded (`mna_core::audio_decode`: m4a/AAC, mp3, wav, flac,
//! ogg), transcribed offline with the transcription model the user chose,
//! run through the same text filters as live meetings, and stored as a
//! normal meeting with one recording session. The audio is kept as the
//! usual Ogg Opus recording when meeting audio retention is on, so playback,
//! waveform and audio export work like on recorded meetings. Summary and
//! cleanup then run from the meeting page like for any other meeting.

use std::path::Path;

use chrono::{DateTime, Utc};
use mna_core::audio_decode::{DecodedAudio, decode_file_to_mono};

use crate::constants::{SAMPLE_RATE_F64, SILENCE_SUFFIX_SAMPLES};
use crate::engine::{TranscriptionEngine, TranscriptionProfile, TranscriptionSegment};
use crate::filter::TextFilterChain;
use crate::settings::{AppSettings, MeetingAudioRetention};
use crate::state::AppState;
use crate::transcript::{MeetingRecordingSession, MeetingTranscript};

pub use mna_core::audio_decode::{SUPPORTED_EXTENSIONS, is_supported_extension};

/// Error text when the user cancels an import.
pub const CANCELED: &str = "Import canceled";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportStage {
    Decoding,
    LoadingModel,
    Transcribing,
    Saving,
}

/// Progress of one file; `fraction` (0..=1) is only meaningful while
/// transcribing.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct ImportProgress {
    pub stage: ImportStage,
    pub fraction: f32,
}

/// Meeting title from the file name ("Neue Aufnahme 12.m4a" -> "Neue
/// Aufnahme 12").
pub fn title_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::trim)
        .filter(|stem| !stem.is_empty())
        .unwrap_or("Imported recording")
        .to_string()
}

/// When the recording started: the file's creation time (voice memos keep
/// the recording time there), else its modification time minus the
/// duration, else now minus the duration.
pub fn recording_start(path: &Path, duration_seconds: f64) -> DateTime<Utc> {
    let duration = chrono::Duration::milliseconds((duration_seconds.max(0.0) * 1000.0) as i64);
    let metadata = std::fs::metadata(path).ok();
    if let Some(created) = metadata.as_ref().and_then(|m| m.created().ok()) {
        return DateTime::<Utc>::from(created);
    }
    if let Some(modified) = metadata.as_ref().and_then(|m| m.modified().ok()) {
        return DateTime::<Utc>::from(modified) - duration;
    }
    Utc::now() - duration
}

/// Resample in one-second blocks with the rubato resampler the live
/// capture uses (no gain: file audio is not systematically quiet).
fn resample(samples: &[f32], source_rate: u32, target_rate: u32) -> Vec<f32> {
    if source_rate == target_rate || samples.is_empty() {
        return samples.to_vec();
    }
    let mut resampler = crate::audio::Resampler::new(source_rate, 1, target_rate, 1.0);
    let block = source_rate.max(1) as usize;
    let mut out = Vec::with_capacity(
        (samples.len() as f64 * f64::from(target_rate) / f64::from(source_rate)) as usize + block,
    );
    for chunk in samples.chunks(block) {
        out.extend(resampler.process(chunk));
    }
    out.extend(resampler.flush());
    out
}

/// Feed `pcm` (already at the engine's rate) to `engine` in its native chunk
/// size, flush, then normalize and filter every segment like the live
/// pipeline (`emit_filtered`). Only final segments are kept when the engine
/// produced any (batch engines with previews off only emit finals).
pub fn transcribe_pcm(
    engine: &mut dyn TranscriptionEngine,
    pcm: &[f32],
    filters: &TextFilterChain,
    should_cancel: &dyn Fn() -> bool,
    on_fraction: &dyn Fn(f32),
) -> Result<Vec<TranscriptionSegment>, String> {
    let requirements = engine.audio_requirements();
    let chunk_size = requirements.chunk_size_samples.max(1) as usize;
    let suffix = (SILENCE_SUFFIX_SAMPLES as f64 / SAMPLE_RATE_F64
        * f64::from(requirements.sample_rate_hz))
    .round() as usize;

    let total_chunks = pcm.len().div_ceil(chunk_size).max(1);
    let mut raw: Vec<TranscriptionSegment> = Vec::new();
    let mut frame = vec![0.0f32; chunk_size];
    let padded_len = pcm.len() + suffix;
    let mut position = 0usize;
    let mut chunk_index = 0usize;
    while position < padded_len {
        if should_cancel() {
            return Err(CANCELED.into());
        }
        frame.fill(0.0);
        let end = (position + chunk_size).min(padded_len);
        if position < pcm.len() {
            let available = end.min(pcm.len());
            frame[..available - position].copy_from_slice(&pcm[position..available]);
        }
        let mut produced = engine
            .transcribe(&frame, None)
            .map_err(|e| format!("Transcription failed: {e}"))?;
        raw.append(&mut produced);
        position = end;
        chunk_index += 1;
        if chunk_index.is_multiple_of(20) {
            on_fraction((chunk_index as f32 / total_chunks as f32).min(1.0));
        }
    }
    let mut flushed = engine
        .flush()
        .map_err(|e| format!("Transcription failed: {e}"))?;
    raw.append(&mut flushed);
    on_fraction(1.0);

    let keep_only_final = raw.iter().any(|segment| segment.is_final);
    Ok(raw
        .into_iter()
        .filter(|segment| !keep_only_final || segment.is_final)
        .filter_map(|mut segment| {
            segment.text = filters.apply(&engine.normalize_text(&segment.text));
            if segment.text.trim().is_empty() {
                return None;
            }
            // Imports have one lane: no Me/Them speaker.
            segment.speaker = None;
            Some(segment)
        })
        .collect())
}

/// Build the meeting for an imported file.
pub fn build_meeting(
    title: String,
    started_at: DateTime<Utc>,
    duration_seconds: f64,
    profile: TranscriptionProfile,
    segments: Vec<TranscriptionSegment>,
) -> MeetingTranscript {
    let id = uuid::Uuid::new_v4().to_string();
    let ended_at =
        started_at + chrono::Duration::milliseconds((duration_seconds.max(0.0) * 1000.0) as i64);
    let session = MeetingRecordingSession {
        id: uuid::Uuid::new_v4().to_string(),
        started_at,
        ended_at,
        duration_seconds: duration_seconds.max(0.0),
        start_segment_index: 0,
        end_segment_index: segments.len() as u64,
    };
    MeetingTranscript {
        id,
        title,
        started_at,
        ended_at: Some(ended_at),
        duration_seconds: duration_seconds.max(0.0),
        transcription_profile: profile,
        recording_sessions: vec![session],
        segments,
        summary: None,
        summary_is_stale: false,
        summary_model: None,
        summary_generated_at: None,
        structured_summary: None,
        edited_transcript: None,
        notes: None,
        calendar_event_id: None,
        participants: Vec::new(),
        system_audio: None,
    }
}

/// The profile to transcribe with: the one the app has active, else the one
/// saved in the settings.
fn import_profile(
    state: &AppState,
    settings: &AppSettings,
) -> Result<TranscriptionProfile, String> {
    if let Some(profile) = state
        .current_machine_state()
        .ok()
        .and_then(|machine| machine.active_profile().cloned())
    {
        return Ok(profile);
    }
    crate::engine::resolve_transcription_profile(
        Some(&settings.transcription_engine_id),
        Some(&settings.transcription_model_id),
        Some(&settings.transcription_backend_id),
    )
}

fn load_engine(profile: &TranscriptionProfile) -> Result<Box<dyn TranscriptionEngine>, String> {
    if !crate::models::model_exists(profile) {
        return Err(format!(
            "The transcription model '{}' is not downloaded. Download it in Settings first.",
            profile.model_label
        ));
    }
    let mut engine = crate::engine::create_engine(profile)?;
    let load = match crate::models::model_location(profile)? {
        crate::engine::ModelLocation::Files(path) => engine.load_model(&path),
        crate::engine::ModelLocation::SystemSpeech { locale } => engine.load_system_assets(&locale),
    };
    load.map_err(|e| format!("Could not load the transcription model: {e}"))?;
    engine.set_preview_enabled(false);
    Ok(engine)
}

/// Import one audio file as a meeting and return it (already saved).
/// Blocking: run it on a worker thread. `should_cancel` is polled while
/// decoding and transcribing.
pub fn import_audio_file(
    state: &AppState,
    path: &Path,
    should_cancel: &dyn Fn() -> bool,
    on_progress: &dyn Fn(ImportProgress),
) -> Result<MeetingTranscript, String> {
    let machine = state.current_machine_state()?;
    if machine.is_recording() {
        return Err("Stop the current recording before importing audio.".into());
    }

    on_progress(ImportProgress {
        stage: ImportStage::Decoding,
        fraction: 0.0,
    });
    let decoded: DecodedAudio = decode_file_to_mono(path, should_cancel).map_err(|e| {
        if should_cancel() {
            CANCELED.to_string()
        } else {
            e.to_string()
        }
    })?;
    let duration_seconds = decoded.duration_ms() as f64 / 1000.0;

    let settings = AppSettings::load(&state.db)?;
    let profile = import_profile(state, &settings)?;
    on_progress(ImportProgress {
        stage: ImportStage::LoadingModel,
        fraction: 0.0,
    });
    let mut engine = load_engine(&profile)?;
    engine.set_meeting_language_prior(settings.meeting_transcription_language);
    engine
        .reset_state()
        .map_err(|e| format!("Could not prepare the transcription model: {e}"))?;

    let dictionary = state.db.list_dictionary_entries().unwrap_or_default();
    let filters =
        crate::filter::build_text_filters(&settings.pipeline_config(), dictionary, &[], &[]);
    let target_rate = engine.audio_requirements().sample_rate_hz;
    let pcm = resample(&decoded.samples, decoded.sample_rate, target_rate);
    let segments = transcribe_pcm(
        engine.as_mut(),
        &pcm,
        &filters,
        should_cancel,
        &|fraction| {
            on_progress(ImportProgress {
                stage: ImportStage::Transcribing,
                fraction,
            })
        },
    )?;
    drop(pcm);
    drop(engine);
    if segments.is_empty() {
        return Err("No speech was recognized in this file.".into());
    }

    on_progress(ImportProgress {
        stage: ImportStage::Saving,
        fraction: 1.0,
    });
    let meeting = build_meeting(
        title_from_path(path),
        recording_start(path, duration_seconds),
        duration_seconds,
        profile,
        segments,
    );
    if settings.meeting_audio_retention != MeetingAudioRetention::Off {
        let audio_path = crate::audio::recorder::session_path(&meeting.id, 0);
        if let Err(error) = crate::audio::recorder::write_mono_ogg_opus(
            &audio_path,
            &decoded.samples,
            decoded.sample_rate,
        ) {
            // The transcript is what matters; a missing player is not fatal.
            tracing::warn!(%error, "Could not store the imported audio");
        }
    }
    state.db.save_meeting(&meeting)?;
    tracing::info!(
        segments = meeting.segments.len(),
        duration_seconds,
        "Imported audio file as meeting"
    );
    Ok(meeting)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::mock::MockEngine;
    use crate::filter::TextFilterChain;

    fn segment(text: &str, start: f64, is_final: bool) -> TranscriptionSegment {
        TranscriptionSegment {
            text: text.to_string(),
            start_time: start,
            end_time: start + 1.0,
            is_final,
            language: None,
            confidence: None,
            speaker: Some(souffle_schema::Speaker::Me),
        }
    }

    #[test]
    fn title_comes_from_the_file_name() {
        assert_eq!(
            title_from_path(Path::new("/tmp/Neue Aufnahme 12.m4a")),
            "Neue Aufnahme 12"
        );
        assert_eq!(title_from_path(Path::new("/")), "Imported recording");
    }

    #[test]
    fn transcribes_keeps_finals_and_drops_empty_text() {
        let mut engine = MockEngine::new()
            .with_transcribe_response(Ok(vec![segment("Hallo", 0.0, true)]), 1)
            .with_transcribe_response(Ok(vec![segment("Hal", 0.5, false)]), 1)
            .with_transcribe_response(Ok(vec![segment("   ", 1.0, true)]), 1);
        engine
            .flush_responses
            .push_back(Ok(vec![segment("Welt", 2.0, true)]));
        let requirements = engine.audio_requirements();
        let pcm = vec![0.0f32; requirements.chunk_size_samples as usize * 3];
        let filters = TextFilterChain::new(vec![]);

        let fractions = std::cell::RefCell::new(Vec::new());
        let segments = transcribe_pcm(&mut engine, &pcm, &filters, &|| false, &|f| {
            fractions.borrow_mut().push(f)
        })
        .unwrap();
        let texts: Vec<&str> = segments.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["Hallo", "Welt"]);
        assert!(segments.iter().all(|s| s.speaker.is_none()));
        assert_eq!(fractions.borrow().last().copied(), Some(1.0));
    }

    #[test]
    fn cancel_stops_the_transcription() {
        let mut engine = MockEngine::new();
        let pcm = vec![0.0f32; 16_000];
        let filters = TextFilterChain::new(vec![]);
        let error = transcribe_pcm(&mut engine, &pcm, &filters, &|| true, &|_| {}).unwrap_err();
        assert_eq!(error, CANCELED);
    }

    #[test]
    fn imported_meeting_is_a_regular_meeting() {
        let (db, _dir) = crate::test_helpers::fixtures::test_db();
        let started = Utc::now() - chrono::Duration::minutes(5);
        let meeting = build_meeting(
            "Sprachmemo".into(),
            started,
            61.5,
            crate::engine::TranscriptionProfile::default(),
            vec![segment("Hallo", 0.0, true)],
        );
        assert_eq!(meeting.recording_sessions.len(), 1);
        assert_eq!(meeting.recording_sessions[0].end_segment_index, 1);
        assert_eq!(meeting.duration_seconds, 61.5);
        db.save_meeting(&meeting).unwrap();
        let loaded = db.load_meeting(&meeting.id).unwrap();
        assert_eq!(loaded.title, "Sprachmemo");
        assert_eq!(loaded.segments.len(), 1);
        assert!(loaded.ended_at.is_some());
    }

    #[test]
    fn resample_changes_the_rate() {
        let samples = vec![0.1f32; 48_000];
        let out = resample(&samples, 48_000, 16_000);
        let ratio = out.len() as f64 / 16_000.0;
        assert!((ratio - 1.0).abs() < 0.05, "got {} samples", out.len());
        assert_eq!(resample(&samples, 16_000, 16_000).len(), 48_000);
    }
}

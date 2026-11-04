//! Meeting Notes AI: AI cleanup of a meeting transcript, segment by segment.
//!
//! The AI fixes spelling, grammar and filler words of every segment; the
//! numbering (`mna_core::cleanup`) keeps the link to the original segment.
//! Segments the AI drops or garbles keep their original text and are
//! flagged. Runs on the same providers as the summary (local Ollama, Ollama
//! Cloud, Apple Intelligence).

use mna_core::cleanup::{
    CLEANUP_SYSTEM_PROMPT, CleanupFlag, DEFAULT_CHUNK_CHARS, NumberedSegment, chunk_segments,
    format_segments, merge_cleanup, parse_cleanup_response,
};

use crate::db::cleaned::CleanedSegmentRecord;
use crate::engine::TranscriptionSegment;

/// Low temperature: the cleanup must stay close to what was said.
const CLEANUP_TEMPERATURE: f32 = 0.1;

/// Progress of a running cleanup: `done` of `total` requests finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct CleanupProgress {
    pub done: usize,
    pub total: usize,
}

fn speaker_label(segment: &TranscriptionSegment) -> String {
    match segment.speaker {
        Some(souffle_schema::Speaker::Me) => "Me".to_string(),
        Some(souffle_schema::Speaker::Them) => "Them".to_string(),
        None => String::new(),
    }
}

/// Numbered input for the AI; numbers are 1-based positions in `segments`.
pub fn numbered_segments(segments: &[TranscriptionSegment]) -> Vec<NumberedSegment> {
    segments
        .iter()
        .enumerate()
        .map(|(index, segment)| NumberedSegment {
            number: index + 1,
            start_ms: (segment.start_time.max(0.0) * 1000.0) as u64,
            speaker: speaker_label(segment),
            text: segment.text.trim().to_string(),
        })
        .collect()
}

/// Turn the merged result into stored records (index = 0-based position).
pub fn records_from_cleanup(
    segments: &[TranscriptionSegment],
    cleaned: &[mna_core::cleanup::CleanedSegment],
) -> Vec<CleanedSegmentRecord> {
    cleaned
        .iter()
        .filter_map(|item| {
            let index = item.number.checked_sub(1)?;
            let source = segments.get(index)?;
            Some(CleanedSegmentRecord {
                index,
                start_time: source.start_time,
                speaker: source.speaker.map(|speaker| speaker.as_str().to_string()),
                original: item.original.clone(),
                text: item.text.clone(),
                flag: item.flag.as_str().to_string(),
                edited: false,
            })
        })
        .collect()
}

/// Number of requests a cleanup of `segments` takes (for progress display).
pub fn cleanup_request_count(segments: &[TranscriptionSegment]) -> usize {
    chunk_segments(&numbered_segments(segments), DEFAULT_CHUNK_CHARS).len()
}

/// Clean up all segments with `model` (resolved like summary models).
pub async fn clean_transcript_segments(
    segments: &[TranscriptionSegment],
    model: &str,
    ollama_url: &str,
    on_progress: impl Fn(CleanupProgress),
) -> Result<Vec<CleanedSegmentRecord>, String> {
    let provider = super::resolve_provider(model)?;
    let numbered = numbered_segments(segments);
    let chunks = chunk_segments(&numbered, DEFAULT_CHUNK_CHARS);
    if chunks.is_empty() {
        return Err("Transcript has no text".into());
    }

    let total = chunks.len();
    on_progress(CleanupProgress { done: 0, total });
    let mut answers: Vec<(usize, String)> = Vec::new();
    for (done, chunk) in chunks.iter().enumerate() {
        let prompt = format_segments(chunk);
        let budget = super::ollama::polish_budget(super::estimate_tokens(&prompt));
        let answer = super::generate_with_provider(
            provider,
            model,
            ollama_url,
            CLEANUP_SYSTEM_PROMPT,
            prompt,
            CLEANUP_TEMPERATURE,
            budget,
            &|_progress| {},
            true,
        )
        .await?;
        answers.extend(parse_cleanup_response(&answer));
        on_progress(CleanupProgress {
            done: done + 1,
            total,
        });
    }

    let merged = merge_cleanup(&numbered, &answers);
    let kept = merged
        .iter()
        .filter(|item| item.flag != CleanupFlag::None)
        .count();
    if kept > 0 {
        tracing::info!(
            kept,
            total = merged.len(),
            "Transcript cleanup kept original text for unreliable segments"
        );
    }
    Ok(records_from_cleanup(segments, &merged))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::fixtures::sample_segment;

    #[test]
    fn numbers_segments_and_maps_records_back() {
        let mut first = sample_segment(" Äh also hallo ", 1.5, 2.0);
        first.speaker = Some(souffle_schema::Speaker::Me);
        let second = sample_segment("zweites Segment", 3.0, 4.0);
        let segments = vec![first, second];

        let numbered = numbered_segments(&segments);
        assert_eq!(numbered[0].number, 1);
        assert_eq!(numbered[0].start_ms, 1500);
        assert_eq!(numbered[0].speaker, "Me");
        assert_eq!(numbered[0].text, "Äh also hallo");
        assert_eq!(numbered[1].speaker, "");

        let answers = vec![(1, "Hallo.".to_string())];
        let merged = merge_cleanup(&numbered, &answers);
        let records = records_from_cleanup(&segments, &merged);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].index, 0);
        assert_eq!(records[0].text, "Hallo.");
        assert_eq!(records[0].speaker.as_deref(), Some("me"));
        assert_eq!(records[1].text, "zweites Segment");
        assert_eq!(records[1].flag, "missing");
        assert_eq!(cleanup_request_count(&segments), 1);
    }
}

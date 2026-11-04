//! Meeting Notes AI: AI cleanup of a meeting transcript, paragraph by
//! paragraph.
//!
//! The cleanup works on the same paragraphs the meeting page shows
//! (`souffle_schema::paragraphs`): some engines emit one segment per word,
//! and a single word gives the AI nothing to correct. The AI fixes spelling,
//! grammar and filler words of every paragraph; the numbering
//! (`mna_core::cleanup`) keeps the link to the original paragraph.
//! Paragraphs the AI drops or garbles keep their original text and are
//! flagged. Runs on the same providers as the summary (local Ollama, Ollama
//! Cloud, Apple Intelligence).

use mna_core::cleanup::{
    CLEANUP_SYSTEM_PROMPT, CleanupFlag, DEFAULT_CHUNK_CHARS, NumberedSegment, chunk_segments,
    format_segments, merge_cleanup, parse_cleanup_response,
};
use souffle_schema::paragraphs::{PAUSE_THRESHOLD_SECONDS, Paragraph, group_into_paragraphs};

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

/// The transcript as the paragraphs the meeting page shows.
pub fn transcript_paragraphs(segments: &[TranscriptionSegment]) -> Vec<Paragraph> {
    group_into_paragraphs(segments, PAUSE_THRESHOLD_SECONDS)
        .into_iter()
        .filter(|paragraph| !paragraph.text.trim().is_empty())
        .collect()
}

fn speaker_label(paragraph: &Paragraph) -> String {
    match paragraph.speaker {
        Some(souffle_schema::Speaker::Me) => "Me".to_string(),
        Some(souffle_schema::Speaker::Them) => "Them".to_string(),
        None => String::new(),
    }
}

/// Numbered input for the AI; numbers are 1-based positions in `paragraphs`.
pub fn numbered_paragraphs(paragraphs: &[Paragraph]) -> Vec<NumberedSegment> {
    paragraphs
        .iter()
        .enumerate()
        .map(|(index, paragraph)| NumberedSegment {
            number: index + 1,
            start_ms: (paragraph.start_time.max(0.0) * 1000.0) as u64,
            speaker: speaker_label(paragraph),
            text: paragraph.text.trim().to_string(),
        })
        .collect()
}

/// Turn the merged result into stored records (index = 0-based paragraph).
pub fn records_from_cleanup(
    paragraphs: &[Paragraph],
    cleaned: &[mna_core::cleanup::CleanedSegment],
) -> Vec<CleanedSegmentRecord> {
    cleaned
        .iter()
        .filter_map(|item| {
            let index = item.number.checked_sub(1)?;
            let source = paragraphs.get(index)?;
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
    let paragraphs = transcript_paragraphs(segments);
    chunk_segments(&numbered_paragraphs(&paragraphs), DEFAULT_CHUNK_CHARS).len()
}

/// Clean up the whole transcript with `model` (resolved like summary models).
pub async fn clean_transcript_segments(
    segments: &[TranscriptionSegment],
    model: &str,
    ollama_url: &str,
    on_progress: impl Fn(CleanupProgress),
) -> Result<Vec<CleanedSegmentRecord>, String> {
    let provider = super::resolve_provider(model)?;
    let paragraphs = transcript_paragraphs(segments);
    let numbered = numbered_paragraphs(&paragraphs);
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
            "Transcript cleanup kept original text for unreliable paragraphs"
        );
    }
    Ok(records_from_cleanup(&paragraphs, &merged))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::fixtures::sample_segment;

    #[test]
    fn numbers_paragraphs_and_maps_records_back() {
        // Word level segments without a pause form one paragraph.
        let words = vec![
            sample_segment("Äh", 1.5, 1.7),
            sample_segment("also", 1.8, 2.0),
            sample_segment("hallo.", 2.1, 2.4),
            // Long pause: a new paragraph.
            sample_segment("Zweiter", 10.0, 10.4),
            sample_segment("Absatz", 10.5, 11.0),
        ];
        let paragraphs = transcript_paragraphs(&words);
        assert_eq!(paragraphs.len(), 2);

        let numbered = numbered_paragraphs(&paragraphs);
        assert_eq!(numbered[0].number, 1);
        assert_eq!(numbered[0].start_ms, 1500);
        assert_eq!(numbered[0].speaker, "");
        assert_eq!(numbered[0].text, "Äh also hallo.");
        assert_eq!(numbered[1].text, "Zweiter Absatz");

        let answers = vec![(1, "Hallo.".to_string())];
        let merged = merge_cleanup(&numbered, &answers);
        let records = records_from_cleanup(&paragraphs, &merged);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].index, 0);
        assert_eq!(records[0].text, "Hallo.");
        assert_eq!(records[0].original, "Äh also hallo.");
        assert_eq!(records[1].index, 1);
        assert_eq!(records[1].start_time, 10.0);
        assert_eq!(records[1].text, "Zweiter Absatz");
        assert_eq!(records[1].flag, "missing");
        assert_eq!(cleanup_request_count(&words), 1);
    }

    #[test]
    fn keeps_the_speaker_of_diarized_paragraphs() {
        let mut me = sample_segment("Hallo zusammen.", 0.0, 1.0);
        me.speaker = Some(souffle_schema::Speaker::Me);
        let mut them = sample_segment("Guten Morgen.", 5.0, 6.0);
        them.speaker = Some(souffle_schema::Speaker::Them);
        let paragraphs = transcript_paragraphs(&[me, them]);
        let numbered = numbered_paragraphs(&paragraphs);
        assert_eq!(numbered[0].speaker, "Me");
        assert_eq!(numbered[1].speaker, "Them");
        let merged = merge_cleanup(&numbered, &[]);
        let records = records_from_cleanup(&paragraphs, &merged);
        assert_eq!(records[1].speaker.as_deref(), Some("them"));
    }
}

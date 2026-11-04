//! Transcript cleanup: the AI corrects every segment of a meeting transcript
//! (spelling, grammar, filler words) while the original stays untouched.
//!
//! Segments are numbered `[1]`, `[2]`, ... across the transcript so every
//! cleaned text can be matched to its original segment and timestamps. A
//! segment is never split across requests. Answers that are missing or look
//! wrong keep the original text and are flagged, so no content is lost.

use serde::{Deserialize, Serialize};

/// Fixed rules, always part of the system prompt (not user editable).
pub const CLEANUP_SYSTEM_PROMPT: &str = "You are a careful editor of meeting transcripts.
These rules always apply and take precedence over everything else:
- Never invent information. Use only what is in the transcript.
- Do not change what was said: no new facts, no opinions, no interpretation.
- Keep numbers, measurements, quantities, amounts, names, dates and technical terms exactly.
- Keep the assignment of statements to speakers.
- Do not guess unclear passages; mark them as [unclear] in the transcript's language.
- Write in the same language as the transcript.

Task: clean up every numbered segment on its own.
- Fix spelling, grammar and punctuation.
- Remove filler words (e.g. uh, um, \"äh\", \"ähm\", \"also\", \"halt\"), false starts and needless repetitions.
- Rephrase colloquial wording so it reads clearly, without changing the meaning.
- Do not merge or split segments. Every segment keeps its number.
- Do not repeat the timestamp or the speaker name in the text.
- A segment that only consists of filler words may become empty (\"\").
Answer only with JSON in exactly this format:
{\"segments\": [{\"n\": 1, \"text\": \"cleaned text\"}]}";

/// Roughly 3-4 characters per token: requests stay well inside the context
/// window of small local models as well.
pub const DEFAULT_CHUNK_CHARS: usize = 8000;

/// A cleaned segment shorter than this share of the original is suspicious,
/// unless the original is short (e.g. "Äh, ja, genau.").
const MIN_LENGTH_RATIO: f32 = 0.3;
const MIN_LENGTH_CHECK_CHARS: usize = 25;
const MAX_LENGTH_RATIO: f32 = 2.0;
const MAX_LENGTH_SLACK_CHARS: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumberedSegment {
    /// 1-based, unique within the transcript.
    pub number: usize,
    pub start_ms: u64,
    pub speaker: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CleanupFlag {
    #[default]
    None,
    /// The AI did not return this segment; the original text was kept.
    Missing,
    /// The AI result looked wrong (much shorter/longer); the original was kept.
    Suspicious,
}

impl CleanupFlag {
    pub fn as_str(self) -> &'static str {
        match self {
            CleanupFlag::None => "",
            CleanupFlag::Missing => "missing",
            CleanupFlag::Suspicious => "suspicious",
        }
    }

    pub fn from_stored(value: &str) -> Self {
        match value {
            "missing" => CleanupFlag::Missing,
            "suspicious" => CleanupFlag::Suspicious,
            _ => CleanupFlag::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanedSegment {
    pub number: usize,
    pub original: String,
    pub text: String,
    pub flag: CleanupFlag,
}

pub fn format_timestamp(ms: u64) -> String {
    let seconds = ms / 1000;
    let (hours, rest) = (seconds / 3600, seconds % 3600);
    let (minutes, seconds) = (rest / 60, rest % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

pub fn format_segment_line(segment: &NumberedSegment) -> String {
    let speaker = segment.speaker.trim();
    let speaker = if speaker.is_empty() {
        String::new()
    } else {
        format!(" {speaker}:")
    };
    format!(
        "[{}] ({}){} {}",
        segment.number,
        format_timestamp(segment.start_ms),
        speaker,
        segment.text.trim()
    )
}

/// The user message for one request.
pub fn format_segments(segments: &[NumberedSegment]) -> String {
    segments
        .iter()
        .map(format_segment_line)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Group consecutive non-empty segments so each group stays below
/// `max_chars`. A single longer segment forms its own group.
pub fn chunk_segments(segments: &[NumberedSegment], max_chars: usize) -> Vec<Vec<NumberedSegment>> {
    let mut chunks: Vec<Vec<NumberedSegment>> = Vec::new();
    let mut current: Vec<NumberedSegment> = Vec::new();
    let mut size = 0usize;
    for segment in segments.iter().filter(|s| !s.text.trim().is_empty()) {
        let line = format_segment_line(segment).len() + 1;
        if !current.is_empty() && size + line > max_chars {
            chunks.push(std::mem::take(&mut current));
            size = 0;
        }
        current.push(segment.clone());
        size += line;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Parse the first JSON value in an AI answer (handles code fences and text
/// around the JSON).
pub fn extract_json(text: &str) -> Option<serde_json::Value> {
    let trimmed = text.trim();
    let unfenced = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map(|rest| rest.trim_end().trim_end_matches("```"))
        .unwrap_or(trimmed)
        .trim();
    if let Ok(value) = serde_json::from_str(unfenced) {
        return Some(value);
    }
    let start = unfenced.find(['{', '['])?;
    let closing = if unfenced[start..].starts_with('{') {
        '}'
    } else {
        ']'
    };
    let end = unfenced.rfind(closing)?;
    if end <= start {
        return None;
    }
    serde_json::from_str(&unfenced[start..=end]).ok()
}

fn parse_numbered_text(text: &str) -> Vec<(usize, String)> {
    let mut result: Vec<(usize, String)> = Vec::new();
    let mut current: Option<(usize, String)> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        let numbered = trimmed
            .strip_prefix('[')
            .and_then(|rest| rest.split_once(']'))
            .and_then(|(number, rest)| number.trim().parse::<usize>().ok().map(|n| (n, rest)));
        if let Some((number, rest)) = numbered {
            if let Some(done) = current.take() {
                result.push(done);
            }
            let mut content = rest.trim().to_string();
            // Drop an echoed "(00:12)" timestamp
            if content.starts_with('(')
                && let Some(close) = content.find(')')
                && content[1..close]
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == ':')
            {
                content = content[close + 1..].trim().to_string();
            }
            current = Some((number, content));
        } else if let Some((_, content)) = current.as_mut() {
            content.push('\n');
            content.push_str(line.trim());
        }
    }
    if let Some(done) = current {
        result.push(done);
    }
    result
}

/// Map of segment number to cleaned text, from an AI cleanup answer.
/// The first answer for a number wins.
pub fn parse_cleanup_response(text: &str) -> Vec<(usize, String)> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let mut add = |number: usize, content: String, out: &mut Vec<(usize, String)>| {
        if seen.insert(number) {
            out.push((number, content.trim().to_string()));
        }
    };

    let Some(value) = extract_json(text) else {
        for (number, content) in parse_numbered_text(text) {
            add(number, content, &mut out);
        }
        return out;
    };

    let items = match &value {
        serde_json::Value::Object(map) => map.get("segments").cloned(),
        serde_json::Value::Array(_) => Some(value.clone()),
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => None,
    };
    match items {
        Some(serde_json::Value::Array(items)) => {
            for item in items {
                let Some(object) = item.as_object() else {
                    continue;
                };
                let number = ["n", "number", "id"]
                    .iter()
                    .find_map(|key| match object.get(*key) {
                        Some(serde_json::Value::Number(n)) => n.as_u64().map(|n| n as usize),
                        Some(serde_json::Value::String(s)) => s.trim().parse().ok(),
                        _ => None,
                    });
                if let (Some(number), Some(serde_json::Value::String(content))) =
                    (number, object.get("text"))
                {
                    add(number, content.clone(), &mut out);
                }
            }
        }
        _ => {
            // {"1": "text", "2": "text"}
            if let serde_json::Value::Object(map) = &value {
                for (key, content) in map {
                    if let (Ok(number), Some(content)) =
                        (key.trim().parse::<usize>(), content.as_str())
                    {
                        add(number, content.to_string(), &mut out);
                    }
                }
            }
        }
    }
    out
}

/// Remove a `Speaker:` prefix the AI may have repeated.
pub fn strip_speaker_prefix(text: &str, speaker: &str) -> String {
    let speaker = speaker.trim();
    if !speaker.is_empty()
        && let Some(rest) = text.strip_prefix(speaker)
        && let Some(rest) = rest.strip_prefix(':')
    {
        return rest.trim_start().to_string();
    }
    text.to_string()
}

/// Return a flag if `cleaned` should not be trusted.
pub fn check_cleaned_text(original: &str, cleaned: Option<&str>) -> CleanupFlag {
    let Some(cleaned) = cleaned else {
        return CleanupFlag::Missing;
    };
    let original_len = original.trim().chars().count();
    let cleaned_len = cleaned.trim().chars().count();
    if original_len >= MIN_LENGTH_CHECK_CHARS
        && (cleaned_len as f32) < original_len as f32 * MIN_LENGTH_RATIO
    {
        return CleanupFlag::Suspicious;
    }
    if cleaned_len as f32 > original_len as f32 * MAX_LENGTH_RATIO + MAX_LENGTH_SLACK_CHARS as f32 {
        return CleanupFlag::Suspicious;
    }
    CleanupFlag::None
}

/// Combine the originals with the parsed AI answers of all requests.
/// Never loses content: when in doubt the original is kept and flagged.
pub fn merge_cleanup(
    segments: &[NumberedSegment],
    answers: &[(usize, String)],
) -> Vec<CleanedSegment> {
    segments
        .iter()
        .map(|segment| {
            let original = segment.text.trim().to_string();
            if original.is_empty() {
                return CleanedSegment {
                    number: segment.number,
                    original,
                    text: String::new(),
                    flag: CleanupFlag::None,
                };
            }
            let answer = answers
                .iter()
                .find(|(number, _)| *number == segment.number)
                .map(|(_, text)| strip_speaker_prefix(text, &segment.speaker));
            let flag = check_cleaned_text(&original, answer.as_deref());
            let text = match (flag, answer) {
                (CleanupFlag::None, Some(answer)) => answer,
                _ => original.clone(),
            };
            CleanedSegment {
                number: segment.number,
                original,
                text,
                flag,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(number: usize, speaker: &str, text: &str) -> NumberedSegment {
        NumberedSegment {
            number,
            start_ms: number as u64 * 65_000,
            speaker: speaker.into(),
            text: text.into(),
        }
    }

    #[test]
    fn formats_lines_and_timestamps() {
        assert_eq!(format_timestamp(0), "00:00");
        assert_eq!(format_timestamp(3_725_000), "1:02:05");
        assert_eq!(
            format_segment_line(&seg(1, "Anna", " Hallo ")),
            "[1] (01:05) Anna: Hallo"
        );
        assert_eq!(format_segment_line(&seg(2, "", "Text")), "[2] (02:10) Text");
    }

    #[test]
    fn chunks_keep_order_and_skip_empty_segments() {
        let segments: Vec<_> = (1..=10).map(|n| seg(n, "", &"x".repeat(40))).collect();
        let mut with_empty = segments.clone();
        with_empty.push(seg(11, "", "  "));
        let chunks = chunk_segments(&with_empty, 150);
        assert!(chunks.len() > 1);
        let numbers: Vec<usize> = chunks.iter().flatten().map(|s| s.number).collect();
        assert_eq!(numbers, (1..=10).collect::<Vec<_>>());
    }

    #[test]
    fn parses_json_variants() {
        assert_eq!(
            parse_cleanup_response(
                "```json\n{\"segments\":[{\"n\":1,\"text\":\" Eins \"},{\"n\":\"2\",\"text\":\"Zwei\"}]}\n```"
            ),
            vec![(1, "Eins".into()), (2, "Zwei".into())]
        );
        assert_eq!(
            parse_cleanup_response(
                "Gerne: [{\"n\":1,\"text\":\"A\"},{\"n\":1,\"text\":\"B\"},{\"n\":\"x\"}]"
            ),
            vec![(1, "A".into())]
        );
        let mut mapping = parse_cleanup_response("{\"1\":\"Eins\",\"2\":\"Zwei\"}");
        mapping.sort();
        assert_eq!(mapping, vec![(1, "Eins".into()), (2, "Zwei".into())]);
    }

    #[test]
    fn parses_numbered_fallback() {
        assert_eq!(
            parse_cleanup_response("[1] (00:01) Eins\n[2] Zwei\nmit Umbruch"),
            vec![(1, "Eins".into()), (2, "Zwei\nmit Umbruch".into())]
        );
    }

    #[test]
    fn checks() {
        assert_eq!(check_cleaned_text("Text", None), CleanupFlag::Missing);
        assert_eq!(check_cleaned_text("Äh, ja.", Some("")), CleanupFlag::None);
        assert_eq!(
            check_cleaned_text("Ähm, also, ja, genau.", Some("Genau.")),
            CleanupFlag::None
        );
        assert_eq!(
            check_cleaned_text(
                "Wir müssen das Dach im Obergeschoss bis Freitag abdichten.",
                Some("Dach.")
            ),
            CleanupFlag::Suspicious
        );
        assert_eq!(
            check_cleaned_text("Kurz.", Some(&format!("Kurz. {}", "Erfunden ".repeat(20)))),
            CleanupFlag::Suspicious
        );
        assert_eq!(
            check_cleaned_text("Äh also das ist halt so", Some("Das ist so.")),
            CleanupFlag::None
        );
    }

    #[test]
    fn merge_never_loses_content() {
        let segments = vec![
            seg(1, "Anna", "Äh also das Dach ist halt undicht."),
            seg(2, "Bernd", "Ähm wir sollten den Dachdecker anrufen."),
            seg(3, "Anna", ""),
        ];
        let answers = vec![(1, "Anna: Das Dach ist undicht.".to_string())];
        let merged = merge_cleanup(&segments, &answers);
        assert_eq!(merged[0].text, "Das Dach ist undicht.");
        assert_eq!(merged[0].flag, CleanupFlag::None);
        assert_eq!(merged[1].text, "Ähm wir sollten den Dachdecker anrufen.");
        assert_eq!(merged[1].flag, CleanupFlag::Missing);
        assert_eq!(merged[2].text, "");
        assert_eq!(merged[0].original, "Äh also das Dach ist halt undicht.");
    }

    #[test]
    fn flags_round_trip() {
        for flag in [
            CleanupFlag::None,
            CleanupFlag::Missing,
            CleanupFlag::Suspicious,
        ] {
            assert_eq!(CleanupFlag::from_stored(flag.as_str()), flag);
        }
    }

    #[test]
    fn prompt_contains_the_rules() {
        assert!(CLEANUP_SYSTEM_PROMPT.contains("Never invent information"));
        assert!(CLEANUP_SYSTEM_PROMPT.contains("JSON"));
    }
}

//! Meeting Notes AI: storage of the AI-cleaned transcript of a meeting.
//!
//! The original segments are never modified. Every cleaned segment keeps a
//! snapshot of the original text it was made from, so the comparison stays
//! correct even if the original changes later; `source_segment_count` tells
//! the UI when the meeting got new segments after the cleanup (resumed
//! recording) and the cleanup should be run again. `Database::delete_meeting`
//! removes the row (no FK cascade, see `schema::CREATE_CLEANED_TRANSCRIPTS`).

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::lock_ext::MutexExt;

use super::Database;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CleanedSegmentRecord {
    /// Position of the cleaned paragraph in
    /// `summary::transcript_cleanup::transcript_paragraphs` (paragraphs as the
    /// meeting page shows them; some engines emit one segment per word).
    pub index: usize,
    pub start_time: f64,
    /// `"me"` / `"them"` for diarized meetings, `None` otherwise.
    #[serde(default)]
    pub speaker: Option<String>,
    pub original: String,
    pub text: String,
    /// `""`, `"missing"` or `"suspicious"` (`mna_core::cleanup::CleanupFlag`):
    /// the original was kept because the AI result was not trustworthy.
    #[serde(default)]
    pub flag: String,
    /// Changed by hand after the cleanup.
    #[serde(default)]
    pub edited: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CleanedTranscript {
    pub meeting_id: String,
    pub model: String,
    pub generated_at: DateTime<Utc>,
    pub source_segment_count: usize,
    pub segments: Vec<CleanedSegmentRecord>,
}

impl CleanedTranscript {
    /// True when the meeting's transcript has a different number of segments
    /// than when it was cleaned (e.g. recording resumed afterwards).
    pub fn is_stale(&self, current_segment_count: usize) -> bool {
        self.source_segment_count != current_segment_count
    }

    /// The cleaned text as plain paragraphs, one per non-empty segment.
    pub fn plain_text(&self) -> String {
        self.segments
            .iter()
            .map(|segment| segment.text.trim())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Delete a meeting's cleaned transcript on an already locked connection
/// (used by `Database::delete_meeting`).
pub(super) fn delete_cleaned_with(
    conn: &rusqlite::Connection,
    meeting_id: &str,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM cleaned_transcripts WHERE meeting_id = ?1",
        params![meeting_id],
    )
    .map_err(|e| format!("Delete cleaned transcript: {e}"))?;
    Ok(())
}

impl Database {
    /// Insert or replace the cleaned transcript of a meeting.
    pub fn save_cleaned_transcript(&self, cleaned: &CleanedTranscript) -> Result<(), String> {
        let segments = serde_json::to_string(&cleaned.segments)
            .map_err(|e| format!("Encode cleaned segments: {e}"))?;
        let conn = self.conn.acquire()?;
        conn.execute(
            "INSERT OR REPLACE INTO cleaned_transcripts
                (meeting_id, model, generated_at, source_segment_count, segments)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                cleaned.meeting_id,
                cleaned.model,
                cleaned.generated_at.to_rfc3339(),
                cleaned.source_segment_count as i64,
                segments,
            ],
        )
        .map_err(|e| format!("Save cleaned transcript: {e}"))?;
        Ok(())
    }

    pub fn load_cleaned_transcript(
        &self,
        meeting_id: &str,
    ) -> Result<Option<CleanedTranscript>, String> {
        let conn = self.conn.acquire()?;
        let row = conn
            .query_row(
                "SELECT model, generated_at, source_segment_count, segments
                 FROM cleaned_transcripts WHERE meeting_id = ?1",
                params![meeting_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| format!("Load cleaned transcript: {e}"))?;
        let Some((model, generated_at, count, segments)) = row else {
            return Ok(None);
        };
        let generated_at = DateTime::parse_from_rfc3339(&generated_at)
            .map(|time| time.with_timezone(&Utc))
            .map_err(|e| format!("Parse cleaned transcript date: {e}"))?;
        let segments: Vec<CleanedSegmentRecord> =
            serde_json::from_str(&segments).map_err(|e| format!("Decode cleaned segments: {e}"))?;
        Ok(Some(CleanedTranscript {
            meeting_id: meeting_id.to_string(),
            model,
            generated_at,
            source_segment_count: usize::try_from(count).unwrap_or(0),
            segments,
        }))
    }

    /// Manual edit of one cleaned segment. Returns false when the meeting has
    /// no cleaned transcript or no segment with that index.
    pub fn update_cleaned_segment_text(
        &self,
        meeting_id: &str,
        index: usize,
        text: &str,
    ) -> Result<bool, String> {
        let Some(mut cleaned) = self.load_cleaned_transcript(meeting_id)? else {
            return Ok(false);
        };
        let Some(segment) = cleaned.segments.iter_mut().find(|s| s.index == index) else {
            return Ok(false);
        };
        if segment.text == text {
            return Ok(true);
        }
        segment.text = text.to_string();
        segment.edited = true;
        self.save_cleaned_transcript(&cleaned)?;
        Ok(true)
    }

    pub fn delete_cleaned_transcript(&self, meeting_id: &str) -> Result<(), String> {
        let conn = self.conn.acquire()?;
        delete_cleaned_with(&conn, meeting_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::fixtures::{sample_meeting, test_db};

    fn cleaned(meeting_id: &str) -> CleanedTranscript {
        CleanedTranscript {
            meeting_id: meeting_id.into(),
            model: "deepseek-v4.1-flash".into(),
            generated_at: Utc::now(),
            source_segment_count: 2,
            segments: vec![
                CleanedSegmentRecord {
                    index: 0,
                    start_time: 0.0,
                    speaker: Some("me".into()),
                    original: "äh Hello world".into(),
                    text: "Hello world.".into(),
                    flag: String::new(),
                    edited: false,
                },
                CleanedSegmentRecord {
                    index: 1,
                    start_time: 1.5,
                    speaker: None,
                    original: "second segment".into(),
                    text: "second segment".into(),
                    flag: "missing".into(),
                    edited: false,
                },
            ],
        }
    }

    #[test]
    fn save_load_edit_and_delete() {
        let (db, _dir) = test_db();
        db.save_meeting(&sample_meeting("m1")).unwrap();
        assert!(db.load_cleaned_transcript("m1").unwrap().is_none());

        db.save_cleaned_transcript(&cleaned("m1")).unwrap();
        let loaded = db.load_cleaned_transcript("m1").unwrap().unwrap();
        assert_eq!(loaded.model, "deepseek-v4.1-flash");
        assert_eq!(loaded.segments.len(), 2);
        assert_eq!(loaded.segments[1].flag, "missing");
        assert!(!loaded.is_stale(2));
        assert!(loaded.is_stale(3));
        assert_eq!(loaded.plain_text(), "Hello world.\nsecond segment");

        assert!(
            db.update_cleaned_segment_text("m1", 0, "Hallo Welt.")
                .unwrap()
        );
        let edited = db.load_cleaned_transcript("m1").unwrap().unwrap();
        assert_eq!(edited.segments[0].text, "Hallo Welt.");
        assert!(edited.segments[0].edited);
        assert_eq!(edited.segments[0].original, "äh Hello world");
        assert!(!db.update_cleaned_segment_text("m1", 9, "x").unwrap());

        db.delete_cleaned_transcript("m1").unwrap();
        assert!(db.load_cleaned_transcript("m1").unwrap().is_none());
    }

    #[test]
    fn original_meeting_is_untouched_and_deletion_cascades() {
        let (db, _dir) = test_db();
        let meeting = sample_meeting("m2");
        db.save_meeting(&meeting).unwrap();
        db.save_cleaned_transcript(&cleaned("m2")).unwrap();

        let reloaded = db.load_meeting("m2").unwrap();
        assert_eq!(reloaded.segments[0].text, meeting.segments[0].text);

        // Saving the meeting again (e.g. after a resumed recording) rewrites
        // its row with INSERT OR REPLACE; the cleaned transcript must survive.
        db.save_meeting(&reloaded).unwrap();
        assert!(db.load_cleaned_transcript("m2").unwrap().is_some());

        db.delete_meeting("m2").unwrap();
        assert!(db.load_cleaned_transcript("m2").unwrap().is_none());
    }
}

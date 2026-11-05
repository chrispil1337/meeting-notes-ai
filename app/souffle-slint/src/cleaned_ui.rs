//! Meeting Notes AI: the AI-cleaned transcript on the meeting page
//! (`CleanedTranscriptSection`): filling its rows, running the cleanup and
//! saving manual edits. The original transcript is never touched here.

use std::sync::Arc;

use mna_core::cleanup::{CleanupFlag, format_timestamp};
use mna_core::diff::{DiffOp, word_diff};
use slint::{ComponentHandle, ModelRc, VecModel};
use souffle_lib::db::cleaned::{CleanedSegmentRecord, CleanedTranscript};
use souffle_lib::progress::ProgressChannel;
use souffle_lib::state::AppState;
use souffle_lib::summary::transcript_cleanup::CleanupProgress;

use crate::{CleanedDiffPiece, CleanedSegmentRow, MainWindow, SpeakerRole};

thread_local! {
    /// Meeting whose cleanup is running (one at a time). The page shows the
    /// running state only while that meeting is open.
    static CLEANING_MEETING: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

fn cleaning_meeting() -> Option<String> {
    CLEANING_MEETING.with(|cell| cell.borrow().clone())
}

fn set_cleaning_meeting(id: Option<String>) {
    CLEANING_MEETING.with(|cell| *cell.borrow_mut() = id);
}

/// Diff kinds understood by `CleanedDiffPiece.kind`.
const KIND_EQUAL: i32 = 0;
const KIND_REMOVED: i32 = 1;
const KIND_ADDED: i32 = 2;

fn diff_kind(op: DiffOp) -> i32 {
    match op {
        DiffOp::Equal => KIND_EQUAL,
        DiffOp::Delete => KIND_REMOVED,
        DiffOp::Insert => KIND_ADDED,
    }
}

/// Word pieces for the comparison view: one piece per word, whitespace after
/// a word goes to `trailing` (normalized to one space) so it wraps and is
/// never highlighted.
pub(crate) fn diff_pieces(original: &str, cleaned: &str) -> Vec<(String, String, i32)> {
    let mut pieces: Vec<(String, String, i32)> = Vec::new();
    for piece in word_diff(original, cleaned) {
        let kind = diff_kind(piece.op);
        let mut word = String::new();
        for ch in piece.text.chars() {
            if ch.is_whitespace() {
                if !word.is_empty() {
                    pieces.push((std::mem::take(&mut word), String::new(), kind));
                }
                if let Some(last) = pieces.last_mut() {
                    last.1 = " ".to_string();
                }
            } else {
                word.push(ch);
            }
        }
        if !word.is_empty() {
            pieces.push((word, String::new(), kind));
        }
    }
    pieces
}

fn speaker_role(speaker: Option<&str>) -> Option<SpeakerRole> {
    match souffle_schema::Speaker::parse(speaker?)? {
        souffle_schema::Speaker::Me => Some(SpeakerRole::Me),
        souffle_schema::Speaker::Them => Some(SpeakerRole::Them),
    }
}

fn row(record: &CleanedSegmentRecord) -> CleanedSegmentRow {
    let diff: Vec<CleanedDiffPiece> = diff_pieces(&record.original, &record.text)
        .into_iter()
        .map(|(text, trailing, kind)| CleanedDiffPiece {
            text: text.into(),
            trailing: trailing.into(),
            kind,
        })
        .collect();
    let role = speaker_role(record.speaker.as_deref());
    CleanedSegmentRow {
        index: i32::try_from(record.index).unwrap_or(i32::MAX),
        timestamp: format_timestamp((record.start_time.max(0.0) * 1000.0) as u64).into(),
        has_speaker: role.is_some(),
        speaker: role.unwrap_or(SpeakerRole::Me),
        text: record.text.clone().into(),
        original: record.original.clone().into(),
        flagged: CleanupFlag::from_stored(&record.flag) != CleanupFlag::None,
        edited: record.edited,
        diff: ModelRc::new(VecModel::from(diff)),
    }
}

/// Push a cleaned transcript (or its absence) into the meeting page.
pub(crate) fn show_cleaned(
    window: &MainWindow,
    cleaned: Option<&CleanedTranscript>,
    current_segment_count: usize,
) {
    let rows: Vec<CleanedSegmentRow> = cleaned
        .map(|cleaned| {
            cleaned
                .segments
                .iter()
                .filter(|segment| !segment.text.trim().is_empty() || !segment.original.is_empty())
                .map(row)
                .collect()
        })
        .unwrap_or_default();
    window.set_meeting_detail_cleaned_has(cleaned.is_some());
    window.set_meeting_detail_cleaned_is_stale(
        cleaned.is_some_and(|cleaned| cleaned.is_stale(current_segment_count)),
    );
    let model_label = cleaned
        .map(|cleaned| crate::summary_model_label(window, &cleaned.model))
        .unwrap_or_default();
    window.set_meeting_detail_cleaned_model_label(model_label);
    window.set_meeting_detail_cleaned_rows(ModelRc::new(VecModel::from(rows)));
}

/// Load and show the cleaned transcript of `meeting_id`. A read error is
/// logged and shown as "not cleaned yet" so the page still opens.
pub(crate) fn populate_cleaned(
    window: &MainWindow,
    state: &AppState,
    meeting_id: &str,
    current_segment_count: usize,
) {
    let cleaned = match state.db.load_cleaned_transcript(meeting_id) {
        Ok(cleaned) => cleaned,
        Err(error) => {
            eprintln!("Could not load the cleaned transcript of {meeting_id}: {error}");
            None
        }
    };
    show_cleaned(window, cleaned.as_ref(), current_segment_count);
}

/// Reset the transient cleanup state when a meeting opens: the running
/// state only shows on the meeting being cleaned.
pub(crate) fn reset_cleanup_status(window: &MainWindow, meeting_id: &str) {
    window.set_meeting_detail_cleaned_error("".into());
    let running_here = cleaning_meeting().as_deref() == Some(meeting_id);
    window.set_meeting_detail_cleaned_is_generating(running_here);
    if !running_here {
        window.set_meeting_detail_cleaned_progress_done(0);
        window.set_meeting_detail_cleaned_progress_total(0);
    }
}

pub(crate) fn wire(window: &MainWindow, state: Arc<AppState>) {
    let weak = window.as_weak();
    let state_cleanup = state.clone();
    window.on_meeting_detail_cleanup(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        if cleaning_meeting().is_some() {
            window.set_meeting_detail_cleaned_error(
                "Another transcript is being cleaned. Try again when it is done.".into(),
            );
            return;
        }
        let id = window.get_active_meeting_id().to_string();
        let model = window
            .get_meeting_detail_summary_selected_model_id()
            .to_string();
        if id.is_empty() {
            return;
        }
        if model.is_empty() {
            window.set_meeting_detail_cleaned_error("No AI model selected".into());
            return;
        }
        set_cleaning_meeting(Some(id.clone()));
        window.set_meeting_detail_cleaned_is_generating(true);
        window.set_meeting_detail_cleaned_error("".into());
        window.set_meeting_detail_cleaned_progress_done(0);
        window.set_meeting_detail_cleaned_progress_total(0);

        let weak_progress = weak.clone();
        let progress_id = id.clone();
        let channel = ProgressChannel::new(move |progress: CleanupProgress| {
            let weak = weak_progress.clone();
            let id = progress_id.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = weak.upgrade()
                    && w.get_active_meeting_id().as_str() == id
                {
                    w.set_meeting_detail_cleaned_progress_done(
                        i32::try_from(progress.done).unwrap_or(i32::MAX),
                    );
                    w.set_meeting_detail_cleaned_progress_total(
                        i32::try_from(progress.total).unwrap_or(i32::MAX),
                    );
                }
            });
        });

        let weak_done = weak.clone();
        let state = state_cleanup.clone();
        souffle_lib::async_runtime::spawn(async move {
            let result = souffle_lib::commands::clean_meeting_transcript(
                state.clone(),
                id.clone(),
                model,
                channel,
            )
            .await;
            let _ = slint::invoke_from_event_loop(move || {
                set_cleaning_meeting(None);
                let Some(w) = weak_done.upgrade() else {
                    return;
                };
                if w.get_active_meeting_id().as_str() != id {
                    // Another meeting is open; its page never showed this run.
                    if let Err(error) = result {
                        eprintln!("Transcript cleanup of meeting {id} failed: {error}");
                    }
                    return;
                }
                w.set_meeting_detail_cleaned_is_generating(false);
                match result {
                    Ok(cleaned) => {
                        show_cleaned(&w, Some(&cleaned), cleaned.source_segment_count);
                    }
                    Err(error) => w.set_meeting_detail_cleaned_error(error.into()),
                }
            });
        });
    });

    let weak = window.as_weak();
    window.on_meeting_detail_cleaned_segment_edited(move |index, text| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let id = window.get_active_meeting_id().to_string();
        let Ok(index) = usize::try_from(index) else {
            return;
        };
        match souffle_lib::commands::update_cleaned_transcript_segment(
            &state,
            &id,
            index,
            text.as_str(),
        ) {
            Ok(_) => {
                let count = state
                    .db
                    .load_meeting(&id)
                    .map(|meeting| meeting.segments.len())
                    .unwrap_or(0);
                populate_cleaned(&window, &state, &id, count);
            }
            Err(error) => window.set_meeting_detail_cleaned_error(error.into()),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_pieces_split_words_and_keep_spaces_unhighlighted() {
        let pieces = diff_pieces("äh wir treffen uns", "Wir treffen uns.");
        let words: Vec<&str> = pieces.iter().map(|p| p.0.as_str()).collect();
        assert!(words.contains(&"äh"));
        assert!(words.contains(&"Wir"));
        assert!(pieces.iter().all(|p| !p.0.contains(' ')));
        let removed: Vec<&str> = pieces
            .iter()
            .filter(|p| p.2 == KIND_REMOVED)
            .map(|p| p.0.as_str())
            .collect();
        assert!(removed.contains(&"äh"));
        assert!(pieces.iter().any(|p| p.2 == KIND_ADDED));
        assert!(pieces.iter().any(|p| p.2 == KIND_EQUAL && p.0 == "treffen"));
    }

    #[test]
    fn identical_text_has_no_changes() {
        let pieces = diff_pieces("Hallo Welt", "Hallo Welt");
        assert_eq!(pieces.len(), 2);
        assert!(pieces.iter().all(|p| p.2 == KIND_EQUAL));
        assert_eq!(pieces[0].1, " ");
        assert_eq!(pieces[1].1, "");
    }
}

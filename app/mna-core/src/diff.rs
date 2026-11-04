//! Word level comparison of an original and a cleaned text.
//!
//! Tokens are words and the whitespace between them, so joining the
//! `Equal` and `Delete` pieces gives back the original exactly, and joining
//! `Equal` and `Insert` gives the cleaned text.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffOp {
    Equal,
    Delete,
    Insert,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiffPiece {
    pub op: DiffOp,
    pub text: String,
}

fn tokenize(text: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start = 0;
    let mut in_space: Option<bool> = None;
    for (index, ch) in text.char_indices() {
        let is_space = ch.is_whitespace();
        match in_space {
            Some(previous) if previous != is_space => {
                tokens.push(&text[start..index]);
                start = index;
            }
            _ => {}
        }
        in_space = Some(is_space);
    }
    if start < text.len() {
        tokens.push(&text[start..]);
    }
    tokens
}

/// Longest common subsequence on tokens. Transcripts are diffed segment by
/// segment, so the quadratic table stays small; very long inputs fall back
/// to "everything changed" instead of using a lot of memory.
const MAX_CELLS: usize = 4_000_000;

pub fn word_diff(original: &str, changed: &str) -> Vec<DiffPiece> {
    let a = tokenize(original);
    let b = tokenize(changed);
    let mut pieces: Vec<DiffPiece> = Vec::new();

    let push = |pieces: &mut Vec<DiffPiece>, op: DiffOp, text: &str| {
        if text.is_empty() {
            return;
        }
        if let Some(last) = pieces.last_mut()
            && last.op == op
        {
            last.text.push_str(text);
            return;
        }
        pieces.push(DiffPiece {
            op,
            text: text.to_string(),
        });
    };

    if (a.len() + 1).saturating_mul(b.len() + 1) > MAX_CELLS {
        push(&mut pieces, DiffOp::Delete, original);
        push(&mut pieces, DiffOp::Insert, changed);
        return pieces;
    }

    // lcs[i][j] = LCS length of a[i..] and b[j..]
    let width = b.len() + 1;
    let mut lcs = vec![0u32; (a.len() + 1) * width];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i * width + j] = if a[i] == b[j] {
                lcs[(i + 1) * width + j + 1] + 1
            } else {
                lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
            };
        }
    }

    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            push(&mut pieces, DiffOp::Equal, a[i]);
            i += 1;
            j += 1;
        } else if lcs[(i + 1) * width + j] >= lcs[i * width + j + 1] {
            push(&mut pieces, DiffOp::Delete, a[i]);
            i += 1;
        } else {
            push(&mut pieces, DiffOp::Insert, b[j]);
            j += 1;
        }
    }
    for token in &a[i..] {
        push(&mut pieces, DiffOp::Delete, token);
    }
    for token in &b[j..] {
        push(&mut pieces, DiffOp::Insert, token);
    }
    pieces
}

/// Share of changed words: 0.0 identical, 1.0 completely different.
pub fn change_ratio(original: &str, changed: &str) -> f32 {
    let a: Vec<&str> = original.split_whitespace().collect();
    let b: Vec<&str> = changed.split_whitespace().collect();
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let pieces = word_diff(&a.join(" "), &b.join(" "));
    let equal_words: usize = pieces
        .iter()
        .filter(|p| p.op == DiffOp::Equal)
        .map(|p| p.text.split_whitespace().count())
        .sum();
    1.0 - (2 * equal_words) as f32 / (a.len() + b.len()) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn join(pieces: &[DiffPiece], skip: DiffOp) -> String {
        pieces
            .iter()
            .filter(|p| p.op != skip)
            .map(|p| p.text.as_str())
            .collect()
    }

    #[test]
    fn reconstructs_both_texts() {
        let original = "Äh also das Dach ist halt undicht.";
        let changed = "Das Dach ist undicht.";
        let pieces = word_diff(original, changed);
        assert_eq!(join(&pieces, DiffOp::Insert), original);
        assert_eq!(join(&pieces, DiffOp::Delete), changed);
        assert!(
            pieces
                .iter()
                .any(|p| p.op == DiffOp::Delete && p.text.contains("halt"))
        );
    }

    #[test]
    fn identical_and_empty() {
        assert_eq!(
            word_diff("gleich bleibt", "gleich bleibt"),
            vec![DiffPiece {
                op: DiffOp::Equal,
                text: "gleich bleibt".into()
            }]
        );
        assert!(word_diff("", "").is_empty());
        assert_eq!(change_ratio("a b", "a b"), 0.0);
        assert_eq!(change_ratio("", ""), 0.0);
        assert!(change_ratio("a b c", "x y z") > 0.99);
    }

    #[test]
    fn adjacent_pieces_are_merged() {
        let pieces = word_diff("eins zwei drei", "eins vier fünf drei");
        assert!(pieces.windows(2).all(|w| w[0].op != w[1].op));
    }

    #[test]
    fn huge_inputs_do_not_explode() {
        let long_a = "wort ".repeat(3000);
        let long_b = "anders ".repeat(3000);
        let pieces = word_diff(&long_a, &long_b);
        assert_eq!(pieces.len(), 2);
    }
}

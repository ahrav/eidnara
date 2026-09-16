//! Memory-tool adapter for search over history_segments and notes.
//!
//! Search combines history_segment and note hits with deterministic rank, recency,
//! and identifier ordering, and preserves store errors as [`MemoryToolError`].

use std::{cmp::Reverse, ops::Range};

use memory_store::{
    MemoryStore, MemoryStoreError, StoredHistorySegmentSearchRow, StoredNoteSearchRow,
};

/// Failure returned by memory-tool adapters.
#[derive(thiserror::Error, Debug)]
pub enum MemoryToolError {
    #[error("store: {0}")]
    Store(MemoryStoreError),
}
impl From<MemoryStoreError> for MemoryToolError {
    fn from(e: MemoryStoreError) -> Self {
        MemoryToolError::Store(e)
    }
}

/// Field that supplied a memory-search hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemorySearchSourceKind {
    HistorySegmentTitle,
    HistorySegmentBody,
    Note,
}

/// Search hit with source-specific metadata and a bounded display snippet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemorySearchResult {
    pub source_kind: MemorySearchSourceKind,
    pub id: i64,
    pub snippet: String,
    pub category: Option<String>,
    pub sequence: Option<i64>,
    pub title: Option<String>,
    pub note_status: Option<String>,
    pub surface_condition: Option<String>,
}

#[derive(Debug)]
enum SearchCandidate {
    History {
        row: StoredHistorySegmentSearchRow,
        title_match: Option<Range<usize>>,
    },
    Note(StoredNoteSearchRow),
}

/// Searches one session's history_segment titles, bodies, and notes.
///
/// Blank queries and zero limits return no rows without reading the store.
/// Matching is lowercase-based and non-regex. Title and note hits rank before
/// history_segment-body hits.
/// Equal ranks sort by descending sequence or update time, then ascending source identifier.
/// Stable sorting preserves history-before-note ties.
/// Candidates sort before body and note verification to avoid scanning lower-ranked text.
/// Verification skips non-matches until the limit is filled or candidates run out.
/// Snippets contain at most 200 Unicode scalar values plus truncation ellipses.
///
/// # Errors
///
/// Returns a store error if either history_segment or note search fails.
pub fn search_history_segments_and_notes_for_session(
    store: &MemoryStore,
    project_path: &str,
    session_id: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<MemorySearchResult>, MemoryToolError> {
    let query = query.trim();
    if query.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }

    let mut candidates: Vec<_> = store
        .search_history_segments_like(session_id, query)?
        .into_iter()
        .map(|row| SearchCandidate::History {
            title_match: first_match(&row.title, query),
            row,
        })
        .collect();
    candidates.extend(
        store
            .search_notes_like(project_path, session_id, query)?
            .into_iter()
            .map(SearchCandidate::Note),
    );

    candidates.sort_by_key(SearchCandidate::sort_key);
    Ok(candidates
        .into_iter()
        .filter_map(|candidate| candidate.into_result(query))
        .take(limit)
        .collect())
}

impl SearchCandidate {
    fn sort_key(&self) -> (u8, Reverse<i64>, i64) {
        match self {
            Self::History { row, title_match } => (
                if title_match.is_some() { 1 } else { 2 },
                Reverse(row.sequence),
                row.sequence,
            ),
            Self::Note(note) => (1, Reverse(note.updated_at_ms), note.id),
        }
    }

    fn into_result(self, query: &str) -> Option<MemorySearchResult> {
        match self {
            Self::History { row, title_match } => {
                let (source_kind, snippet) = if let Some(hit) = title_match {
                    (
                        MemorySearchSourceKind::HistorySegmentTitle,
                        snippet_around_match(&row.title, hit),
                    )
                } else {
                    let body = history_segment_body_text(&row);
                    let hit = first_match(&body, query)?;
                    (
                        MemorySearchSourceKind::HistorySegmentBody,
                        snippet_around_match(&body, hit),
                    )
                };
                Some(MemorySearchResult {
                    source_kind,
                    id: row.sequence,
                    snippet,
                    category: None,
                    sequence: Some(row.sequence),
                    title: Some(row.title),
                    note_status: None,
                    surface_condition: None,
                })
            }
            Self::Note(note) => {
                let (matched_text, hit) = if let Some(hit) = first_match(&note.content, query) {
                    (note.content.as_str(), hit)
                } else {
                    let condition = note.surface_condition.as_deref()?;
                    (condition, first_match(condition, query)?)
                };
                Some(MemorySearchResult {
                    source_kind: MemorySearchSourceKind::Note,
                    id: note.id,
                    snippet: snippet_around_match(matched_text, hit),
                    category: None,
                    sequence: None,
                    title: None,
                    note_status: Some(note.status),
                    surface_condition: note.surface_condition,
                })
            }
        }
    }
}

fn history_segment_body_text(history_segment: &StoredHistorySegmentSearchRow) -> String {
    let mut parts = Vec::new();
    push_unique_text(&mut parts, &history_segment.content);
    for tier in [
        &history_segment.p1,
        &history_segment.p2,
        &history_segment.p3,
        &history_segment.p4,
    ] {
        if let Some(text) = tier.as_deref() {
            push_unique_text(&mut parts, text);
        }
    }
    parts.join("\n")
}

fn push_unique_text(parts: &mut Vec<String>, text: &str) {
    if !text.is_empty() && !parts.iter().any(|part| part == text) {
        parts.push(text.to_string());
    }
}

/// Case-insensitive search returning the match's byte range in `text`, not in its folded form.
///
/// Lowercasing can change byte length (`İ` folds to two code points), so the folded string
/// carries a byte-offset map back to the source.
fn first_match(text: &str, query: &str) -> Option<Range<usize>> {
    let needle = query.to_lowercase();
    let mut folded = String::with_capacity(text.len());
    let mut source_offsets = Vec::with_capacity(text.len() + 1);
    for (index, ch) in text.char_indices() {
        for lowered in ch.to_lowercase() {
            let start = folded.len();
            folded.push(lowered);
            source_offsets.extend(std::iter::repeat_n(index, folded.len() - start));
        }
    }
    source_offsets.push(text.len());
    let hit = folded.find(&needle)?;
    let start = source_offsets[hit];
    let end = source_offsets[hit + needle.len()];
    Some(start..end)
}

fn snippet_around_match(text: &str, hit: Range<usize>) -> String {
    const CONTEXT: usize = 100;
    const MAX_CHARS: usize = 200;

    debug_assert!(hit.start <= hit.end && hit.end <= text.len());
    debug_assert!(text.is_char_boundary(hit.start) && text.is_char_boundary(hit.end));

    let mut start = hit.start.saturating_sub(CONTEXT);
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (hit.end + CONTEXT).min(text.len());
    while end < text.len() && !text.is_char_boundary(end) {
        end += 1;
    }

    let snippet: String = text[start..end].chars().take(MAX_CHARS).collect();
    let prefix = if start > 0 { "…" } else { "" };
    let suffix = if end < text.len() { "…" } else { "" };
    format!("{prefix}{}{suffix}", snippet.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_refills_rejected_candidates_and_preserves_rank_and_ties() {
        use memory_store::{CoreState, ModuleMeta, NoteInput, StoredHistorySegment};

        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::open_for_test(dir.path(), "memory-tool-test");
        store
            .commit("session", None, &CoreState::empty(), &ModuleMeta::default())
            .unwrap();
        // SQL folds `ΟΣ` to `ος`, but scalar-by-scalar verification produces `οσ`.
        for (index, (content, now_ms)) in [("ος", 1), ("ΟΣ", 1000)].into_iter().enumerate() {
            let note = store
                .insert_note(NoteInput {
                    project_path: "project",
                    route_project_root: None,
                    session_id: "session",
                    content,
                    surface_condition: None,
                    anchor_block_id: None,
                    now_ms,
                })
                .unwrap();
            assert_eq!(note.id, index as i64 + 1);
        }
        let rows: Vec<_> = [(1, "ος", "title body"), (100, "newer body", "ος")]
            .into_iter()
            .enumerate()
            .map(|(index, (sequence, title, content))| StoredHistorySegment {
                sequence,
                start_message: index as i64 * 2,
                end_message: index as i64 * 2 + 1,
                start_message_id: format!("start-{index}"),
                end_message_id: format!("end-{index}"),
                title: title.into(),
                content: content.into(),
                ..Default::default()
            })
            .collect();
        store.replace_history_segments("session", &rows).unwrap();

        assert_eq!(
            store
                .search_notes_like("project", "session", "ΟΣ")
                .unwrap()
                .len(),
            2
        );
        let expected = [
            (MemorySearchSourceKind::HistorySegmentTitle, 1),
            (MemorySearchSourceKind::Note, 1),
            (MemorySearchSourceKind::HistorySegmentBody, 100),
        ];
        for limit in 1..=expected.len() {
            let results = search_history_segments_and_notes_for_session(
                &store, "project", "session", "ΟΣ", limit,
            )
            .unwrap();
            assert_eq!(
                results
                    .iter()
                    .map(|hit| (hit.source_kind, hit.id))
                    .collect::<Vec<_>>(),
                expected[..limit]
            );
            assert!(results.iter().all(|hit| hit.snippet == "ος"));
        }
    }

    #[test]
    fn case_insensitive_match_offsets_map_back_to_the_source_text() {
        // `İ` (U+0130) lowercases to `i` plus U+0307, so the folded text is longer than the source.
        let text = format!("{}needle tail", "İ".repeat(40));
        let hit = first_match(&text, "NEEDLE").expect("match");
        assert_eq!(&text[hit.clone()], "needle");
        let snippet = snippet_around_match(&text, hit);
        assert!(
            snippet.contains("needle tail"),
            "snippet must cover the real match, got {snippet:?}"
        );
        assert!(first_match("plain", "PLAIN").is_some());
        assert!(first_match("plain", "absent").is_none());
    }
}

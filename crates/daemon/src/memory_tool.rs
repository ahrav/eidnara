//! Memory-tool adapter for search over compartments and notes.
//!
//! Search combines compartment and note hits with deterministic rank, recency,
//! and identifier ordering, and preserves store errors as [`MemoryToolError`].

use memory_store::{
    MemoryStore, MemoryStoreError, StoredCompartmentSearchRow, StoredNoteSearchRow,
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
    CompartmentTitle,
    CompartmentBody,
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
struct RankedSearchResult {
    result: MemorySearchResult,
    rank: u8,
    recency: i64,
}

/// Searches one session's compartment titles, bodies, and notes.
///
/// Blank queries and zero limits return no rows without reading the store.
/// Matching is lowercase-based and non-regex. Title and note hits rank before
/// compartment-body hits. Equal ranks sort by descending sequence or update time,
/// then ascending source identifier. Results are truncated after sorting.
/// Snippets contain at most 200 Unicode scalar values plus truncation ellipses.
///
/// # Errors
///
/// Returns a store error if either compartment or note search fails.
pub fn search_compartments_and_notes_for_session(
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

    let mut ranked = Vec::new();
    for compartment in store.search_compartments_like(session_id, query)? {
        if let Some(hit) = compartment_search_hit(compartment, query) {
            ranked.push(hit);
        }
    }
    for note in store.search_notes_like(project_path, session_id, query)? {
        if first_match(&note.content, query).is_some()
            || note
                .surface_condition
                .as_deref()
                .is_some_and(|condition| first_match(condition, query).is_some())
        {
            ranked.push(note_search_hit(note, query));
        }
    }

    ranked.sort_by(|left, right| {
        left.rank
            .cmp(&right.rank)
            .then_with(|| right.recency.cmp(&left.recency))
            .then_with(|| left.result.id.cmp(&right.result.id))
    });
    ranked.truncate(limit);
    Ok(ranked.into_iter().map(|r| r.result).collect())
}

fn note_search_hit(note: StoredNoteSearchRow, query: &str) -> RankedSearchResult {
    let matched_text = if first_match(&note.content, query).is_some() {
        note.content.as_str()
    } else {
        note.surface_condition
            .as_deref()
            .unwrap_or(note.content.as_str())
    };
    RankedSearchResult {
        rank: 1,
        recency: note.updated_at_ms,
        result: MemorySearchResult {
            source_kind: MemorySearchSourceKind::Note,
            id: note.id,
            snippet: snippet_around_match(matched_text, query),
            category: None,
            sequence: None,
            title: None,
            note_status: Some(note.status),
            surface_condition: note.surface_condition,
        },
    }
}

fn compartment_search_hit(
    compartment: StoredCompartmentSearchRow,
    query: &str,
) -> Option<RankedSearchResult> {
    if first_match(&compartment.title, query).is_some() {
        return Some(RankedSearchResult {
            rank: 1,
            recency: compartment.sequence,
            result: MemorySearchResult {
                source_kind: MemorySearchSourceKind::CompartmentTitle,
                id: compartment.sequence,
                snippet: snippet_around_match(&compartment.title, query),
                category: None,
                sequence: Some(compartment.sequence),
                title: Some(compartment.title),
                note_status: None,
                surface_condition: None,
            },
        });
    }

    let body = compartment_body_text(&compartment);
    first_match(&body, query).map(|_| RankedSearchResult {
        rank: 2,
        recency: compartment.sequence,
        result: MemorySearchResult {
            source_kind: MemorySearchSourceKind::CompartmentBody,
            id: compartment.sequence,
            snippet: snippet_around_match(&body, query),
            category: None,
            sequence: Some(compartment.sequence),
            title: Some(compartment.title),
            note_status: None,
            surface_condition: None,
        },
    })
}

fn compartment_body_text(compartment: &StoredCompartmentSearchRow) -> String {
    let mut parts = Vec::new();
    push_unique_text(&mut parts, &compartment.content);
    for tier in [
        &compartment.p1,
        &compartment.p2,
        &compartment.p3,
        &compartment.p4,
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
fn first_match(text: &str, query: &str) -> Option<std::ops::Range<usize>> {
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

fn snippet_around_match(text: &str, query: &str) -> String {
    const CONTEXT: usize = 100;
    const MAX_CHARS: usize = 200;

    let Some(hit) = first_match(text, query) else {
        return text.chars().take(MAX_CHARS).collect();
    };
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
    fn case_insensitive_match_offsets_map_back_to_the_source_text() {
        // `İ` (U+0130) lowercases to `i` plus U+0307, so the folded text is longer than the source.
        let text = format!("{}needle tail", "İ".repeat(40));
        let hit = first_match(&text, "NEEDLE").expect("match");
        assert_eq!(&text[hit.clone()], "needle");
        let snippet = snippet_around_match(&text, "NEEDLE");
        assert!(
            snippet.contains("needle tail"),
            "snippet must cover the real match, got {snippet:?}"
        );
        assert!(first_match("plain", "PLAIN").is_some());
        assert!(first_match("plain", "absent").is_none());
    }
}

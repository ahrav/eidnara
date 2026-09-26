//! The per-session coverage authority of transform revision 3 (spec C5, D2, D5, D7, D10, D11,
//! D20): what a declared anchor means against one store snapshot, the ordinals the window's
//! messages receive from the effective anchor, where the recipe's input keeps point in the
//! submitted window, and the anchor pages `transform.boundary` answers. Nothing here writes;
//! the transform commit persists what the pass does with a resolution.

use memory_store::{CoverageSnapshot, HistorySegmentEdge, MemoryStore, MemoryStoreError};
use serde_json::{Value, json};

use crate::edit_recipe::{Operation, Source};
use crate::wire::split_block_id;

/// Anchors per `transform.boundary` page.
pub const BOUNDARY_PAGE_LIMIT: usize = 4_096;

/// The largest integer JavaScript represents exactly, `2^53 - 1`.
const MAX_SAFE_INTEGER: i64 = (1 << 53) - 1;

/// One submitted window message, in submitted order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowMessage<'a> {
    pub mid: &'a str,
    pub synthetic: bool,
}

/// The request's non-null `boundary`: the history segment whose end message the window starts at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeclaredAnchor<'a> {
    pub mid: &'a str,
    pub sequence: i64,
}

/// What a declared anchor means (D10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// The declared row is the rendered boundary.
    Normal,
    /// Coverage advanced past the declared row and its end message is still in the window;
    /// the first `cut` submitted messages are sliced off before processing.
    StaleSlice { cut: usize },
    /// The rendered boundary's message is gone from the window: reconcile through the
    /// declared row, or, with no declared row, reset the session's coverage.
    Revert { keep_through_seq: Option<i64> },
    /// The declared sequence names no row.
    Unknown,
    /// No boundary declared and no coverage held.
    FirstPass,
}

/// A resolution with its effective anchor and the ordinals of the processed window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub resolution: Resolution,
    /// The row whose end message heads the processed window, if any.
    pub anchor: Option<HistorySegmentEdge>,
    /// One ordinal per message of the submitted window after the cut; empty for `Unknown`.
    pub ordinals: Vec<u64>,
}

impl Resolved {
    /// The number of submitted messages sliced off before processing.
    pub fn cut(&self) -> usize {
        match self.resolution {
            Resolution::StaleSlice { cut } => cut,
            _ => 0,
        }
    }
}

/// Reads the snapshot [`resolve`] needs for `window`, in one read transaction.
pub fn read_snapshot(
    store: &MemoryStore,
    session_id: &str,
    declared: Option<DeclaredAnchor<'_>>,
    window: &[WindowMessage<'_>],
) -> Result<CoverageSnapshot, MemoryStoreError> {
    let live_mids: Vec<&str> = window
        .iter()
        .filter(|message| !message.synthetic)
        .map(|message| message.mid)
        .collect();
    store.coverage_snapshot(
        session_id,
        declared.map(|anchor| anchor.sequence),
        &live_mids,
    )
}

/// Resolves `declared` against `snapshot` and the original submitted `window`. An error is an
/// `invalid_params` answer: a window that does not start at the declared mid, a declared row
/// whose end block is not the declared mid, or a declared row newer than the rendered boundary,
/// which discovery never returns.
pub fn resolve(
    snapshot: &CoverageSnapshot,
    declared: Option<DeclaredAnchor<'_>>,
    window: &[WindowMessage<'_>],
) -> Result<Resolved, String> {
    let position = |row: &HistorySegmentEdge| {
        let (mid, _) = split_block_id(&row.end_message_id)?;
        window.iter().position(|message| message.mid == mid)
    };
    let (resolution, anchor) = match declared {
        Some(declared) => {
            if window.first().map(|message| message.mid) != Some(declared.mid) {
                return Err(format!(
                    "the window does not start at the declared boundary {:?}",
                    declared.mid
                ));
            }
            let Some(row) = snapshot.declared.as_ref() else {
                return Ok(Resolved {
                    resolution: Resolution::Unknown,
                    anchor: None,
                    ordinals: Vec::new(),
                });
            };
            if split_block_id(&row.end_message_id).map(|(mid, _)| mid) != Some(declared.mid) {
                return Err(format!(
                    "history segment {} ends at {:?}, not at the declared boundary {:?}",
                    row.sequence, row.end_message_id, declared.mid
                ));
            }
            match snapshot.rendered.as_ref() {
                Some(rendered) if rendered.sequence == row.sequence => (Resolution::Normal, row),
                Some(rendered) if row.sequence < rendered.sequence => match position(rendered) {
                    Some(cut) => (Resolution::StaleSlice { cut }, rendered),
                    None => (
                        Resolution::Revert {
                            keep_through_seq: Some(row.sequence),
                        },
                        row,
                    ),
                },
                _ => {
                    return Err(format!(
                        "declared boundary sequence {} is newer than the rendered boundary",
                        row.sequence
                    ));
                }
            }
        }
        None => {
            let unanchored = |resolution, continuation_base| Resolved {
                resolution,
                anchor: None,
                ordinals: assign_ordinals(window, None, continuation_base),
            };
            match (&snapshot.rendered, &snapshot.newest_window_end) {
                (None, _) => {
                    return Ok(unanchored(
                        Resolution::FirstPass,
                        snapshot.continuation_base,
                    ));
                }
                (Some(_), None) => {
                    // The reset clears the continuation base with the rest of the coverage.
                    return Ok(unanchored(
                        Resolution::Revert {
                            keep_through_seq: None,
                        },
                        None,
                    ));
                }
                (Some(_), Some(hit)) => {
                    // The store matched the hit's end block to a window mid.
                    let cut = position(hit).ok_or_else(|| {
                        format!("history segment {} matched no window message", hit.sequence)
                    })?;
                    (Resolution::StaleSlice { cut }, hit)
                }
            }
        }
    };
    let cut = match resolution {
        Resolution::StaleSlice { cut } => cut,
        _ => 0,
    };
    Ok(Resolved {
        resolution,
        ordinals: assign_ordinals(&window[cut..], Some(anchor.end_message as u64), None),
        anchor: Some(anchor.clone()),
    })
}

/// Ordinals for a processed window (D11). A non-synthetic message counts one up from the
/// head: the anchored head receives `anchor_ordinal`, and with no anchor the first
/// non-synthetic message is `continuation_base + 1` (or 1). Synthetic messages follow the
/// rule `resolveOrdinals` in `packages/opencode-plugin/src/hooks/context/module-wire.ts`
/// applies to its unresolved synthetic messages, case for case: one with a non-synthetic
/// message after it borrows the ordinal of the message before it, or, with none before it,
/// 0 (the anchor's ordinal in an anchored window); the trailing run after the last
/// non-synthetic message continues dense numbering from the message before it.
pub fn assign_ordinals(
    window: &[WindowMessage<'_>],
    anchor_ordinal: Option<u64>,
    continuation_base: Option<u64>,
) -> Vec<u64> {
    let first = anchor_ordinal.unwrap_or(continuation_base.unwrap_or(0) + 1);
    let last_live = window.iter().rposition(|message| !message.synthetic);
    let mut next_live = first;
    let mut prior: Option<u64> = None;
    let mut ordinals = Vec::with_capacity(window.len());
    for (index, message) in window.iter().enumerate() {
        let ordinal = if !message.synthetic {
            next_live += 1;
            next_live - 1
        } else if last_live.is_some_and(|last| index < last) {
            prior.unwrap_or(anchor_ordinal.unwrap_or(0))
        } else {
            prior.map_or(first, |prior| prior + 1)
        };
        prior = Some(ordinal);
        ordinals.push(ordinal);
    }
    ordinals
}

/// Moves input keeps built against the processed window into submitted-window coordinates
/// (D5, D20): processed position i after cut c is submitted position c + i. Previous-output
/// keeps and inserts are unchanged.
pub fn translate_input_keeps<V>(operations: &mut [Operation<V>], cut: usize) {
    for operation in operations {
        if let Operation::Keep {
            source: Source::Input,
            start,
            ..
        } = operation
        {
            *start += cut as u64;
        }
    }
}

/// Validates a `transform.boundary` body: `v: 3`, a non-empty `session_id`, an optional
/// `before_sequence` that is a JavaScript safe integer, and nothing else beyond the envelope's
/// `method`, `kind`, and `project_root`. Returns the session and the cursor.
pub fn parse_boundary_request(request: &Value) -> Result<(&str, Option<i64>), String> {
    let Some(fields) = request.as_object() else {
        return Err("transform.boundary requires an object body".to_string());
    };
    if let Some(key) = fields.keys().find(|key| {
        !matches!(
            key.as_str(),
            "method" | "kind" | "v" | "session_id" | "project_root" | "before_sequence"
        )
    }) {
        return Err(format!("transform.boundary does not accept {key:?}"));
    }
    if fields.get("v").and_then(Value::as_u64) != Some(3) {
        return Err("transform.boundary requires v=3".to_string());
    }
    let Some(session_id) = fields
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|session| !session.trim().is_empty())
    else {
        return Err("transform.boundary requires a nonempty session_id".to_string());
    };
    let before_sequence = match fields.get("before_sequence") {
        None => None,
        Some(value) => Some(
            value
                .as_i64()
                .filter(|before| before.abs() <= MAX_SAFE_INTEGER)
                .ok_or("before_sequence must be a JavaScript safe integer")?,
        ),
    };
    Ok((session_id, before_sequence))
}

/// One `transform.boundary` page: anchors at or below the rendered boundary and below
/// `before_sequence`, newest first. A row whose end block names no message is not an anchor
/// and is left out.
pub fn boundary_page(
    store: &MemoryStore,
    session_id: &str,
    before_sequence: Option<i64>,
) -> Result<Value, MemoryStoreError> {
    let anchors: Vec<Value> = store
        .coverage_anchor_page(session_id, before_sequence, BOUNDARY_PAGE_LIMIT)?
        .iter()
        .filter_map(|(sequence, end_id)| {
            let (mid, _) = split_block_id(end_id)?;
            Some(json!({ "mid": mid, "sequence": sequence }))
        })
        .collect();
    Ok(json!({ "anchors": anchors }))
}

#[cfg(test)]
pub(crate) mod tests;

//! `kernel.read`: the visible rows of one surface at one snapshot, filtered
//! to the bound project, each carrying the mutation token `kernel.commit`
//! checks. Decision objects also carry their decision row, so a client can
//! render the decision text without a second route.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use host_runtime::RouteHandle;
use kernel::{
    ChunkRow, DecisionContent, DecisionPayload, DecisionRow, KernelError, KernelStore,
    ServingPosition, Surface, VisibleRow,
};
use serde::Deserialize;
use serde_json::Value;

use super::project::{ProjectBinding, ScopeFilter, stored_terms};
use super::serving;
use super::{InvalidReason, KernelOutcome, blocking, state_only};
use crate::HandlerCore;
use crate::dispatch::{PreparedOutcome, PreparedOutput, PreparedOutputError};

const OPERATION: &str = "kernel.read";

/// Rows served per read. A row's fixed JSON structure (registry fields, token, visibility, scope) encodes to ~500 bytes before payload text, so this cap keeps a payload-light response in the low tens of mebibytes and bounds client-side parse and ranking work; memory surfaces render at most ~100 rows per query, so the newest rows this cap keeps dominate every surface's candidate set.
pub const MAX_READ_ROWS: usize = 8192;

/// Byte budget for the serialized `rows` array: one eighth of the 64 MiB wire cap. Redaction caps each decision text field at 512 KiB, so a worst-case row is ~1 MiB and at least eight always fit; typical memory rows run ~1 KiB, so thousands fit before [`MAX_READ_ROWS`] binds first. The response is the rows plus a fixed envelope of under 200 bytes, so a rows array within this budget cannot reach the cap that would fail the whole response.
pub const MAX_READ_ROW_BYTES: usize = crate::dispatch::MAX_WIRE_BODY_BYTES / 8;

/// Ids per `object_ids` filter. A filtered read preflights one mutation batch, so the bound stays far under [`MAX_READ_ROWS`] and a filtered read never hits the row cap.
pub const MAX_READ_OBJECT_IDS: usize = 64;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReadRequest {
    surface: String,
    /// `None` reads the tip.
    #[serde(default)]
    as_of: Option<i64>,
    /// Filters the read to these objects before the row cap applies, so a targeted lookup addresses a row the bounded unfiltered read drops; `None` reads the whole surface.
    #[serde(default)]
    object_ids: Option<Vec<String>>,
    /// Serves only the rows after this row in serving order; a caller sets it to the last row of the previous page to read the next page, and sets `as_of` to the first page's `known_as_of` so every page reads one snapshot.
    #[serde(default)]
    before: Option<ReadCursor>,
    /// Whether the serving policy judges freshness before rows are returned.
    #[serde(default)]
    gated: bool,
    /// Clock the lag age is measured against, so a test can place a read on
    /// either side of the age threshold; production reads take the wall clock.
    #[cfg(feature = "test-support")]
    #[serde(default)]
    now_ms: Option<i64>,
}

impl ReadRequest {
    #[cfg(feature = "test-support")]
    fn clock_ms(&self) -> i64 {
        self.now_ms.unwrap_or_else(crate::now_ms)
    }

    #[cfg(not(feature = "test-support"))]
    fn clock_ms(&self) -> i64 {
        crate::now_ms()
    }
}

/// A row's position in serving order (`created_commit_seq` descending, `object_id` ascending), as the row reports it in `object`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReadCursor {
    created_commit_seq: i64,
    object_id: String,
}

impl ReadCursor {
    fn precedes(&self, row: &VisibleRow) -> bool {
        row.object.created_commit_seq < self.created_commit_seq
            || (row.object.created_commit_seq == self.created_commit_seq
                && row.object.object_id.as_str() > self.object_id.as_str())
    }
}

pub(crate) struct ReadResponse {
    pub(crate) known_as_of: i64,
    pub(crate) tip: i64,
    pub(crate) rows: Vec<VisibleRow>,
    /// Whether rows beyond [`MAX_READ_ROWS`] were dropped. Byte-budget truncation happens at serialization, so the response's flag can be `true` while `truncated` here is `false`.
    pub(crate) truncated: bool,
    /// Decision rows keyed by `object_id`, looked up at `known_as_of`. Total
    /// over the visible decision-kind rows in `rows`: a missing entry fails
    /// the read, so a `None` lookup during rendering means a non-decision row.
    pub(crate) decisions: HashMap<String, DecisionRow>,
}

/// The rows a read admits before [`MAX_READ_ROWS`] and [`MAX_READ_ROW_BYTES`] apply, so a bounded read spends its budget only on rows the caller can use rather than on the rest of the project's surface.
#[derive(Debug, Clone, Copy)]
pub(crate) enum RowSelection<'a> {
    /// Applied in the kernel query, so a targeted lookup addresses a row the bounded unfiltered read drops.
    Objects(&'a [String]),
    /// Applied in the kernel query as well; `keeps` re-checks each returned row.
    DomainDecisions(&'a str),
}

impl RowSelection<'_> {
    fn keeps(self, row: &VisibleRow) -> bool {
        match self {
            Self::Objects(_) => true,
            Self::DomainDecisions(domain_id) => {
                row.object.object_kind == "decision" && row.object.domain_id == domain_id
            }
        }
    }
}

/// Orders rows for a max-heap whose maximum is the serving-order-last row: an older `created_commit_seq` ranks greater, and among rows of one commit the greater `object_id` ranks greater, so the heap's peek is exactly the row a full newest-first sort then truncate drops first.
struct ServingOrderLast(VisibleRow);

impl Ord for ServingOrderLast {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .0
            .object
            .created_commit_seq
            .cmp(&self.0.object.created_commit_seq)
            .then_with(|| self.0.object.object_id.cmp(&other.0.object.object_id))
    }
}

impl PartialOrd for ServingOrderLast {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for ServingOrderLast {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for ServingOrderLast {}

/// Bounded newest-first selection: keeps at most `cap` rows of the serving order (`created_commit_seq` descending, `object_id` breaking ties) while candidates stream in, so selection costs `O(n log cap)` comparisons and `cap` retained rows instead of a full sort. Each overflow evicts the serving-order-last kept row, so the kept set and its order match a full sort followed by a truncate.
pub struct NewestRows {
    cap: usize,
    heap: BinaryHeap<ServingOrderLast>,
    dropped: bool,
}

impl NewestRows {
    pub fn new(cap: usize) -> Self {
        Self {
            cap,
            heap: BinaryHeap::with_capacity(cap.saturating_add(1)),
            dropped: false,
        }
    }

    pub fn push(&mut self, row: VisibleRow) {
        self.heap.push(ServingOrderLast(row));
        if self.heap.len() > self.cap {
            self.heap.pop();
            self.dropped = true;
        }
    }

    /// Returns the kept rows in serving order and whether any candidate was dropped.
    pub fn finish(self) -> (Vec<VisibleRow>, bool) {
        let rows = self
            .heap
            .into_sorted_vec()
            .into_iter()
            .map(|entry| entry.0)
            .collect();
        (rows, self.dropped)
    }
}

/// One page of an unfiltered read: rows in serving order, each decision object beside the content the read serves
/// for it.
struct NewestPage {
    known_as_of: i64,
    tip: i64,
    rows: Vec<(VisibleRow, Option<DecisionContent>)>,
    /// Whether rows follow the page at the snapshot.
    truncated: bool,
}

/// The first rows of the project's surface after `before` in serving order, within [`MAX_READ_ROWS`] and with
/// decision payloads within [`MAX_READ_ROW_BYTES`]. The kernel serves the surface in chunks of whole commits,
/// newest first, each decision object with its decision, so the read stops once the page and one further row are
/// in hand; a chunk the scope filter thins is followed by the next.
fn newest_page(
    store: &KernelStore,
    project: &ProjectBinding,
    surface: Surface,
    as_of: Option<i64>,
    before: Option<&ReadCursor>,
) -> Result<NewestPage, KernelError> {
    let requested = match as_of {
        Some(as_of) => as_of,
        None => store.tip()?,
    };
    // The kernel keeps rows whose scope names another project out of each chunk, so the page decodes and judges the project's rows alone.
    let scope = project.scope_term();
    // `ScopeFilter` judges scope-term operators the kernel query keeps for the caller, so the row bound cannot be a SQL `LIMIT`; it applies here, after the filter.
    let mut filter = ScopeFilter::new(project);
    let mut terms = stored_terms(store);
    let mut rows: Vec<(VisibleRow, Option<DecisionContent>)> = Vec::new();
    // Positions in `rows` of the decision objects whose chunk carried no decoded content.
    let mut unread: Vec<usize> = Vec::new();
    let mut payload_bytes: u64 = 0;
    let mut position = before.map(|cursor| (cursor.created_commit_seq, cursor.object_id.clone()));
    let read_chunk = |position: &Option<(i64, String)>, rows: usize, payload_bytes: u64| {
        store.visible_chunk_as_of_in_scope(
            surface,
            requested,
            position
                .as_ref()
                .map(|(created_commit_seq, object_id)| ServingPosition {
                    created_commit_seq: *created_commit_seq,
                    object_id,
                }),
            MAX_READ_ROWS + 1 - rows,
            MAX_READ_ROW_BYTES as u64 - payload_bytes,
            Some(scope),
        )
    };
    let mut chunk = read_chunk(&position, 0, 0)?;
    let tip = chunk.tip;
    rows.reserve(chunk.rows.len().min(MAX_READ_ROWS));
    let truncated = 'page: loop {
        let last = chunk.rows.last().map(|entry| {
            (
                entry.row.object.created_commit_seq,
                entry.row.object.object_id.clone(),
            )
        });
        for ChunkRow { row, decision } in chunk.rows {
            if !filter.matches(row.scope_id.as_deref(), &mut terms)? {
                continue;
            }
            if rows.len() == MAX_READ_ROWS {
                break 'page true;
            }
            let mut content = None;
            if row.object.object_kind == "decision" {
                // A visible decision-kind row without its typed decision row at the same snapshot is a canonical-integrity break. Serving it with `decision: null` would be indistinguishable from a non-decision row and silently drop it from memory surfaces, so the read fails closed instead.
                let decision = decision.ok_or(KernelError::CorruptCanonicalRow)?;
                // A serialized row is at least its payload bytes, so cutting where cumulative payload size passes the budget keeps every row the serializer could keep.
                payload_bytes = payload_bytes.saturating_add(decision.payload_bytes);
                if payload_bytes > MAX_READ_ROW_BYTES as u64 {
                    break 'page true;
                }
                content = decision.content;
                if content.is_none() {
                    unread.push(rows.len());
                }
            }
            rows.push((row, content));
        }
        match last {
            Some(last) if !chunk.exhausted => position = Some(last),
            _ => break false,
        }
        chunk = read_chunk(&position, rows.len(), payload_bytes)?;
    };
    if !unread.is_empty() {
        let ids: Vec<String> = unread
            .iter()
            .map(|&index| rows[index].0.object.object_id.clone())
            .collect();
        let mut decisions: HashMap<String, DecisionRow> = store
            .decisions_for_objects_as_of(&ids, requested)?
            .into_iter()
            .map(|decision| (decision.object_id.clone(), decision))
            .collect();
        for (index, id) in unread.into_iter().zip(&ids) {
            let decision = decisions
                .remove(id)
                .ok_or(KernelError::CorruptCanonicalRow)?;
            rows[index].1 = Some(DecisionContent {
                decision_kind: decision.decision_kind,
                payload: decision.payload,
            });
        }
    }
    Ok(NewestPage {
        known_as_of: requested,
        tip,
        rows,
        truncated,
    })
}

pub(crate) fn read_visible(
    store: &KernelStore,
    project: &ProjectBinding,
    surface: Surface,
    as_of: Option<i64>,
    selection: RowSelection<'_>,
) -> Result<ReadResponse, KernelError> {
    read_visible_page(store, project, surface, as_of, selection, None)
}

/// The selected rows of the project's surface. When `before` is set, `read_visible_page` applies the row cap to
/// rows after `before` in serving order.
pub(crate) fn read_visible_page(
    store: &KernelStore,
    project: &ProjectBinding,
    surface: Surface,
    as_of: Option<i64>,
    selection: RowSelection<'_>,
    before: Option<&ReadCursor>,
) -> Result<ReadResponse, KernelError> {
    let requested = match as_of {
        Some(as_of) => as_of,
        None => store.tip()?,
    };
    // The kernel keeps rows whose scope names another project out of the
    // snapshot, so the read costs the project's rows and not the store's. A
    // domain selection is matched in the kernel query too, so rows of other
    // domains are never decoded or judged.
    let scope = project.scope_term();
    let mut filter = ScopeFilter::new(project);
    let mut terms = stored_terms(store);
    let visible = match selection {
        RowSelection::Objects(ids) => {
            store.visible_as_of_in_scope(surface, requested, Some(ids), Some(scope))?
        }
        RowSelection::DomainDecisions(domain_id) => store.visible_domain_decisions_as_of_in_scope(
            surface,
            requested,
            domain_id,
            Some(scope),
        )?,
    };
    // `ScopeFilter` judges scope-term operators the kernel query keeps for the caller, so the row bound cannot be a SQL `LIMIT`; it applies here, after the filter.
    let mut newest = NewestRows::new(MAX_READ_ROWS);
    for row in visible.rows {
        if selection.keeps(&row)
            && before.is_none_or(|cursor| cursor.precedes(&row))
            && filter.matches(row.scope_id.as_deref(), &mut terms)?
        {
            newest.push(row);
        }
    }
    let (mut rows, mut truncated) = newest.finish();
    // Payload sizes are read before any payload: a project can retain [`MAX_READ_ROWS`] decisions of ~1 MiB each, and materializing them all would allocate gigabytes for rows the serialization budget can never carry. A serialized row is at least its payload bytes, so cutting where cumulative payload size passes the budget keeps every row the serializer could keep.
    let decision_ids: Vec<String> = rows
        .iter()
        .filter(|row| row.object.object_kind == "decision")
        .map(|row| row.object.object_id.clone())
        .collect();
    let payload_sizes: HashMap<String, u64> = store
        .decision_payload_sizes_as_of(&decision_ids, visible.known_as_of)?
        .into_iter()
        .collect();
    // A visible decision-kind row without its typed decision row at the same snapshot is a
    // canonical-integrity break. Serving it with `decision: null` would be indistinguishable
    // from a non-decision row and silently drop it from memory surfaces, so the read fails
    // closed instead.
    if decision_ids
        .iter()
        .any(|id| !payload_sizes.contains_key(id))
    {
        return Err(KernelError::CorruptCanonicalRow);
    }
    let mut cumulative_payload_bytes: u64 = 0;
    let mut cutoff = rows.len();
    for (index, row) in rows.iter().enumerate() {
        if row.object.object_kind != "decision" {
            continue;
        }
        cumulative_payload_bytes = cumulative_payload_bytes
            .saturating_add(*payload_sizes.get(&row.object.object_id).unwrap_or(&0));
        if cumulative_payload_bytes > MAX_READ_ROW_BYTES as u64 {
            cutoff = index;
            break;
        }
    }
    if cutoff < rows.len() {
        rows.truncate(cutoff);
        truncated = true;
    }
    let retained_decision_ids: Vec<String> = rows
        .iter()
        .filter(|row| row.object.object_kind == "decision")
        .map(|row| row.object.object_id.clone())
        .collect();
    let decisions: HashMap<String, DecisionRow> = store
        .decisions_for_objects_as_of(&retained_decision_ids, visible.known_as_of)?
        .into_iter()
        .map(|decision| (decision.object_id.clone(), decision))
        .collect();
    if retained_decision_ids
        .iter()
        .any(|id| !decisions.contains_key(id))
    {
        return Err(KernelError::CorruptCanonicalRow);
    }
    Ok(ReadResponse {
        known_as_of: visible.known_as_of,
        tip: visible.tip,
        rows,
        truncated,
        decisions,
    })
}

/// The escape `serde_json` writes after a backslash for `byte`: the byte's short form, or `u` for a control byte
/// with none, which is written as `\u00` and two lowercase hex digits. `None` for a byte written as it is.
fn json_escape(byte: u8) -> Option<u8> {
    match byte {
        b'"' => Some(b'"'),
        b'\\' => Some(b'\\'),
        0x08 => Some(b'b'),
        0x09 => Some(b't'),
        0x0a => Some(b'n'),
        0x0c => Some(b'f'),
        0x0d => Some(b'r'),
        0x00..=0x1f => Some(b'u'),
        _ => None,
    }
}

/// Whether any of the eight bytes of `word` is below 0x20, a quote, or a backslash.
fn word_needs_escape(word: u64) -> bool {
    const LOW: u64 = 0x0101_0101_0101_0101;
    const HIGH: u64 = 0x8080_8080_8080_8080;
    let has_zero = |bytes: u64| bytes.wrapping_sub(LOW) & !bytes & HIGH;
    let control = word.wrapping_sub(LOW * 0x20) & !word & HIGH;
    (control | has_zero(word ^ (LOW * 0x22)) | has_zero(word ^ (LOW * 0x5c))) != 0
}

/// Appends `value` as a JSON string, byte for byte as `serde_json` encodes it. Text is copied in runs between
/// escapes, and a run is scanned eight bytes at a time.
fn write_str(body: &mut Vec<u8>, value: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = value.as_bytes();
    body.push(b'"');
    let mut run = 0;
    let mut at = 0;
    while let Some(&byte) = bytes.get(at) {
        if let Some(word) = bytes[at..].first_chunk::<8>()
            && !word_needs_escape(u64::from_le_bytes(*word))
        {
            at += 8;
            continue;
        }
        if let Some(escape) = json_escape(byte) {
            body.extend_from_slice(&bytes[run..at]);
            body.push(b'\\');
            body.push(escape);
            if escape == b'u' {
                body.extend_from_slice(&[
                    b'0',
                    b'0',
                    HEX[usize::from(byte >> 4)],
                    HEX[usize::from(byte & 0xf)],
                ]);
            }
            run = at + 1;
        }
        at += 1;
    }
    body.extend_from_slice(&bytes[run..]);
    body.push(b'"');
}

fn write_optional_str(body: &mut Vec<u8>, value: Option<&str>) {
    match value {
        Some(value) => write_str(body, value),
        None => body.extend_from_slice(b"null"),
    }
}

fn write_int(body: &mut Vec<u8>, value: i64) {
    let mut digits = [0u8; 20];
    let mut first = digits.len();
    let mut rest = value.unsigned_abs();
    loop {
        first -= 1;
        digits[first] = b'0' + (rest % 10) as u8;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    if value < 0 {
        body.push(b'-');
    }
    body.extend_from_slice(&digits[first..]);
}

/// The part of a decision a row serves.
#[derive(Clone, Copy)]
struct ServedDecision<'a> {
    decision_kind: &'a str,
    payload: &'a DecisionPayload,
}

/// Appends one row in its wire shape. Keys are written in the sorted order a `serde_json::Map` emits and values as
/// `serde_json` encodes them, so the bytes equal the encoding of the row's `json!` tree.
fn write_row(
    body: &mut Vec<u8>,
    row: &VisibleRow,
    decision: Option<ServedDecision<'_>>,
    known_as_of: i64,
) {
    let object = &row.object;
    body.extend_from_slice(b"{\"decision\":");
    match decision {
        Some(decision) => {
            body.extend_from_slice(b"{\"decision_kind\":");
            write_str(body, decision.decision_kind);
            body.extend_from_slice(b",\"payload\":{\"rationale\":");
            write_str(body, &decision.payload.rationale);
            body.extend_from_slice(b",\"summary\":");
            write_str(body, &decision.payload.summary);
            body.extend_from_slice(b"}}");
        }
        None => body.extend_from_slice(b"null"),
    }
    body.extend_from_slice(b",\"labeled\":");
    body.extend_from_slice(if row.labeled { b"true" } else { b"false" });
    body.extend_from_slice(b",\"object\":{\"created_commit_seq\":");
    write_int(body, object.created_commit_seq);
    body.extend_from_slice(b",\"domain_id\":");
    write_str(body, &object.domain_id);
    body.extend_from_slice(b",\"invalidated_commit_seq\":");
    match object.invalidated_commit_seq {
        Some(invalidated) => write_int(body, invalidated),
        None => body.extend_from_slice(b"null"),
    }
    body.extend_from_slice(b",\"object_id\":");
    write_str(body, &object.object_id);
    body.extend_from_slice(b",\"object_kind\":");
    write_str(body, &object.object_kind);
    body.extend_from_slice(b",\"sensitivity\":");
    write_str(body, object.sensitivity.as_str());
    body.extend_from_slice(b",\"source_id\":");
    write_str(body, &object.source_id);
    body.extend_from_slice(b",\"source_kind\":");
    write_str(body, &object.source_kind);
    body.extend_from_slice(b",\"source_revision\":");
    write_int(body, object.source_revision);
    body.extend_from_slice(b",\"superseded_by\":");
    write_optional_str(body, object.superseded_by.as_deref());
    body.extend_from_slice(b"},\"scope_id\":");
    write_optional_str(body, row.scope_id.as_deref());
    body.extend_from_slice(b",\"token\":{\"known_as_of\":");
    write_int(body, known_as_of);
    body.extend_from_slice(b",\"object_id\":");
    write_str(body, &object.object_id);
    body.extend_from_slice(b"},\"visibility\":");
    write_str(body, row.visibility.as_str());
    body.push(b'}');
}

/// The snapshot and flags of a read's response envelope.
#[derive(Clone, Copy)]
struct Envelope {
    gated: bool,
    known_as_of: i64,
    tip: i64,
    /// Whether the read left rows out before the byte budget applied.
    truncated: bool,
}

/// Encodes the read's response body: the envelope and the longest prefix of `rows` whose encoding fits
/// [`MAX_READ_ROW_BYTES`]. Each row is encoded once, straight into the body, and a row that passes the budget is
/// taken back out. Rows are newest first, so the budget retains a contiguous prefix of the most recent rows.
/// `truncated` reports rows the read or the budget left out.
fn encode_body<'a>(
    envelope: Envelope,
    rows: impl ExactSizeIterator<Item = (&'a VisibleRow, Option<ServedDecision<'a>>)>,
) -> Result<Vec<u8>, serde_json::Error> {
    // A typical memory row encodes to about 1 KiB; the envelope is under 200 bytes.
    let mut body =
        Vec::with_capacity(MAX_READ_ROW_BYTES.min(rows.len().saturating_mul(1024)) + 256);
    body.extend_from_slice(b"{\"gated\":");
    serde_json::to_writer(&mut body, &envelope.gated)?;
    body.extend_from_slice(b",\"known_as_of\":");
    serde_json::to_writer(&mut body, &envelope.known_as_of)?;
    body.extend_from_slice(b",\"rows\":[");
    let rows_start = body.len();
    let mut truncated = envelope.truncated;
    for (row, decision) in rows {
        let row_start = body.len();
        if row_start > rows_start {
            body.push(b',');
        }
        write_row(&mut body, row, decision, envelope.known_as_of);
        // The `+ 1` charges each row's array separator or bracket byte against the budget.
        if body.len() - rows_start + 1 > MAX_READ_ROW_BYTES {
            body.truncate(row_start);
            truncated = true;
            break;
        }
    }
    body.extend_from_slice(b"],\"state\":");
    serde_json::to_writer(&mut body, &KernelOutcome::Available)?;
    body.extend_from_slice(b",\"tip\":");
    serde_json::to_writer(&mut body, &envelope.tip)?;
    body.extend_from_slice(b",\"truncated\":");
    serde_json::to_writer(&mut body, &truncated)?;
    body.push(b'}');
    Ok(body)
}

fn encode_response(response: &ReadResponse, gated: bool) -> Result<Vec<u8>, serde_json::Error> {
    let envelope = Envelope {
        gated,
        known_as_of: response.known_as_of,
        tip: response.tip,
        truncated: response.truncated,
    };
    encode_body(
        envelope,
        response.rows.iter().map(|row| {
            let decision = response.decisions.get(&row.object.object_id);
            let decision = decision.map(|decision| ServedDecision {
                decision_kind: &decision.decision_kind,
                payload: &decision.payload,
            });
            (row, decision)
        }),
    )
}

fn encode_page(page: &NewestPage, gated: bool) -> Result<Vec<u8>, serde_json::Error> {
    let envelope = Envelope {
        gated,
        known_as_of: page.known_as_of,
        tip: page.tip,
        truncated: page.truncated,
    };
    encode_body(
        envelope,
        page.rows.iter().map(|(row, content)| {
            let decision = content.as_ref().map(|content| ServedDecision {
                decision_kind: &content.decision_kind,
                payload: &content.payload,
            });
            (row, decision)
        }),
    )
}

impl HandlerCore {
    pub(crate) async fn handle_kernel_read(
        &self,
        channel: RouteHandle,
        request: Value,
    ) -> PreparedOutcome {
        let (scope, parsed) = match self.kernel_request::<ReadRequest>(channel, request, OPERATION)
        {
            Ok(bound) => bound,
            Err(outcome) => return outcome,
        };
        if parsed
            .object_ids
            .as_ref()
            .is_some_and(|ids| ids.len() > MAX_READ_OBJECT_IDS)
        {
            return crate::invalid_params_error(format!(
                "{OPERATION} object_ids must name at most {MAX_READ_OBJECT_IDS} objects"
            ));
        }
        let Ok(surface) = Surface::try_from(parsed.surface.as_str()) else {
            return state_only(KernelOutcome::invalid(InvalidReason::InvalidInput));
        };
        let mut as_of = parsed.as_of;
        if parsed.gated {
            let store = scope.store.clone();
            let now_ms = parsed.clock_ms();
            let gate = blocking(move || {
                let tip = store.tip()?;
                store.outbox_lag(now_ms).map(|lag| (tip, lag))
            })
            .await;
            let outcome = match gate {
                Ok(Ok((tip, lag))) => {
                    as_of.get_or_insert(tip);
                    serving::project(serving::decide(&lag), surface)
                }
                Ok(Err(error)) => KernelOutcome::from(error),
                Err(outcome) => outcome,
            };
            if !outcome.is_available() {
                return state_only(outcome);
            }
        }
        let store = scope.store.clone();
        let project = scope.project.clone();
        let object_ids = parsed.object_ids.clone();
        let before = parsed.before.clone();
        let gated = parsed.gated;
        let result = blocking(move || {
            let before = before.as_ref();
            // An id filter is matched in the kernel query, which reads every named row; an unfiltered read pages the surface in serving order.
            Ok::<_, KernelError>(match object_ids.as_deref() {
                Some(ids) => {
                    let selection = RowSelection::Objects(ids);
                    let response =
                        read_visible_page(&store, &project, surface, as_of, selection, before)?;
                    encode_response(&response, gated)
                }
                None => encode_page(
                    &newest_page(&store, &project, surface, as_of, before)?,
                    gated,
                ),
            })
        })
        .await;
        match result {
            // The body's bytes are fixed here, so the transport writes them into the frame at publication.
            Ok(Ok(Ok(body))) => PreparedOutcome::Response(PreparedOutput::cached_bytes(body)),
            Ok(Ok(Err(error))) => PreparedOutcome::Error {
                code: "encode_failed".to_owned(),
                message: PreparedOutputError::Serialize(error).to_string(),
            },
            Ok(Err(error)) => state_only(KernelOutcome::from(error)),
            Err(outcome) => state_only(outcome),
        }
    }
}

#[cfg(test)]
mod tests {
    use kernel::{DecisionPayload, ObjectRow, Sensitivity, SurfaceVisibility};
    use serde_json::json;

    use super::*;

    fn row(object_kind: &str, domain_id: &str) -> VisibleRow {
        VisibleRow {
            object: ObjectRow {
                object_id: format!("{domain_id}-{object_kind}"),
                object_kind: object_kind.to_string(),
                domain_id: domain_id.to_string(),
                source_kind: "repo".to_string(),
                source_id: "lineage".to_string(),
                source_revision: 1,
                created_commit_seq: 1,
                invalidated_commit_seq: None,
                superseded_by: None,
                sensitivity: Sensitivity::Normal,
            },
            visibility: SurfaceVisibility::Visible,
            labeled: false,
            scope_id: None,
        }
    }

    fn populated_row() -> VisibleRow {
        let mut row = row("decision", "memory");
        row.object.invalidated_commit_seq = Some(9);
        row.object.superseded_by = Some("memory-decision-2".to_string());
        row.object.sensitivity = Sensitivity::Sensitive;
        row.object.source_id = "line \"quoted\" \\ tab\t ünïcödé".to_string();
        row.visibility = SurfaceVisibility::Hidden;
        row.labeled = true;
        row.scope_id = Some("scope-1".to_string());
        row
    }

    fn decision(object_id: &str) -> DecisionRow {
        DecisionRow {
            decision_id: format!("{object_id}-decision"),
            object_id: object_id.to_string(),
            proposition_id: None,
            scope_id: Some("scope-1".to_string()),
            anchor_id: None,
            evidence_id: None,
            decision_kind: "fact".to_string(),
            payload: DecisionPayload {
                summary: "summary with \"quotes\"".to_string(),
                rationale: "rationale\nwith newline".to_string(),
            },
            created_commit_seq: 3,
            sensitivity: Sensitivity::Normal,
        }
    }

    /// `to_value` of a struct emits its keys in sorted order, so this tree fixes field order too.
    fn row_value(row: &VisibleRow, decision: Option<&DecisionRow>, known_as_of: i64) -> Value {
        json!({
            "object": row.object,
            "visibility": row.visibility.as_str(),
            "labeled": row.labeled,
            "scope_id": row.scope_id,
            "token": {"object_id": row.object.object_id, "known_as_of": known_as_of},
            "decision": decision.map(|decision| json!({
                "decision_kind": decision.decision_kind,
                "payload": decision.payload,
            })),
        })
    }

    /// A column added to `ObjectRow` or `DecisionPayload` fails here instead of silently leaving the wire.
    #[test]
    fn object_and_payload_mirrors_match_the_kernel_structs() {
        let row = populated_row();
        let decision = decision(&row.object.object_id);
        let mut encoded = Vec::new();
        let served = ServedDecision {
            decision_kind: &decision.decision_kind,
            payload: &decision.payload,
        };
        write_row(&mut encoded, &row, Some(served), 7);
        let written: Value = serde_json::from_slice(&encoded).unwrap();

        assert_eq!(
            written["object"],
            serde_json::to_value(&row.object).unwrap()
        );
        assert_eq!(
            written["decision"]["payload"],
            serde_json::to_value(&decision.payload).unwrap()
        );
        assert_eq!(
            String::from_utf8(encoded).unwrap(),
            serde_json::to_string(&row_value(&row, Some(&decision), 7)).unwrap(),
            "a row must write its keys in the sorted order a Value map emits"
        );
    }

    fn written(value: &str) -> String {
        let mut encoded = Vec::new();
        write_str(&mut encoded, value);
        String::from_utf8(encoded).unwrap()
    }

    /// Every ASCII byte, at every offset of an eight-byte word and beside multi-byte text, is written as
    /// `serde_json` writes it.
    #[test]
    fn strings_are_written_as_serde_json_writes_them() {
        for byte in 0..=0x7fu8 {
            let special = char::from(byte);
            for padding in 0..=17 {
                for filler in ["a", "\u{e9}", "\u{1f600}"] {
                    let value = format!("{}{special}{}", filler.repeat(padding), filler.repeat(9));
                    assert_eq!(
                        written(&value),
                        serde_json::to_string(&value).unwrap(),
                        "byte {byte:#x} after {padding} of {filler:?}"
                    );
                }
            }
        }
        for value in [
            "",
            "plain",
            "\"\\\u{0}\u{1f}\u{7f}",
            "tab\tend\n",
            "\u{2028}\u{2029}",
        ] {
            assert_eq!(written(value), serde_json::to_string(value).unwrap());
        }
    }

    proptest::proptest! {
        #[test]
        fn any_string_is_written_as_serde_json_writes_it(value in proptest::prelude::any::<String>()) {
            proptest::prop_assert_eq!(written(&value), serde_json::to_string(&value).unwrap());
        }

        #[test]
        fn any_integer_is_written_as_serde_json_writes_it(value in proptest::prelude::any::<i64>()) {
            let mut encoded = Vec::new();
            write_int(&mut encoded, value);
            proptest::prop_assert_eq!(String::from_utf8(encoded).unwrap(), serde_json::to_string(&value).unwrap());
        }
    }

    #[test]
    fn integer_extremes_are_written_as_serde_json_writes_them() {
        for value in [0, 1, -1, 9, 10, -10, i64::MAX, i64::MIN] {
            let mut encoded = Vec::new();
            write_int(&mut encoded, value);
            assert_eq!(
                String::from_utf8(encoded).unwrap(),
                serde_json::to_string(&value).unwrap()
            );
        }
    }

    fn body_value(response: &ReadResponse, served: usize, gated: bool, truncated: bool) -> Value {
        let rows: Vec<Value> = response.rows[..served]
            .iter()
            .map(|row| {
                row_value(
                    row,
                    response.decisions.get(&row.object.object_id),
                    response.known_as_of,
                )
            })
            .collect();
        json!({
            "known_as_of": response.known_as_of,
            "tip": response.tip,
            "gated": gated,
            "truncated": truncated,
            "rows": rows,
            "state": {"kind": "available"},
        })
    }

    #[test]
    fn read_body_bytes_match_the_value_tree_encoding() {
        let populated = populated_row();
        let bare = row("observation", "notes");
        let decision = decision(&populated.object.object_id);
        let mut decisions = HashMap::new();
        decisions.insert(populated.object.object_id.clone(), decision);
        let mut response = ReadResponse {
            known_as_of: 41,
            tip: 42,
            rows: vec![populated, bare],
            truncated: true,
            decisions,
        };
        for (gated, truncated) in [(true, true), (false, false)] {
            response.truncated = truncated;
            let expected = serde_json::to_vec(&body_value(&response, 2, gated, truncated)).unwrap();
            let encoded = encode_response(&response, gated).unwrap();
            assert_eq!(
                String::from_utf8(encoded.clone()).unwrap(),
                String::from_utf8(expected).unwrap()
            );
            let output = PreparedOutput::cached_bytes(encoded.clone());
            assert!(output.serializes_directly());
            assert_eq!(output.measure().unwrap().len(), encoded.len());
        }
        response.rows.clear();
        assert_eq!(
            encode_response(&response, false).unwrap(),
            serde_json::to_vec(&body_value(&response, 0, false, false)).unwrap()
        );
    }

    /// A page encodes as the response that holds the same rows and decisions.
    #[test]
    fn a_page_encodes_as_the_response_with_its_rows() {
        let populated = populated_row();
        let bare = row("observation", "notes");
        let decision = decision(&populated.object.object_id);
        let page = NewestPage {
            known_as_of: 41,
            tip: 42,
            rows: vec![
                (
                    populated.clone(),
                    Some(DecisionContent {
                        decision_kind: decision.decision_kind.clone(),
                        payload: decision.payload.clone(),
                    }),
                ),
                (bare.clone(), None),
            ],
            truncated: true,
        };
        let response = ReadResponse {
            known_as_of: 41,
            tip: 42,
            rows: vec![populated.clone(), bare],
            truncated: true,
            decisions: HashMap::from([(populated.object.object_id, decision)]),
        };
        assert_eq!(
            String::from_utf8(encode_page(&page, true).unwrap()).unwrap(),
            String::from_utf8(encode_response(&response, true).unwrap()).unwrap()
        );
    }

    /// The body keeps the longest prefix of rows whose encodings, each charged one separator byte, fit the
    /// budget, and reports the rows it leaves out as truncation.
    #[test]
    fn the_byte_budget_keeps_the_longest_row_prefix_that_fits() {
        let rationale = "r".repeat(MAX_READ_ROW_BYTES / 8);
        let mut decisions = HashMap::new();
        let rows: Vec<VisibleRow> = (0..9)
            .map(|index| {
                let mut row = row("decision", "memory");
                row.object.object_id = format!("memory-decision-{index}");
                let mut decision = decision(&row.object.object_id);
                decision.payload.rationale = rationale.clone();
                decisions.insert(row.object.object_id.clone(), decision);
                row
            })
            .collect();
        let response = ReadResponse {
            known_as_of: 7,
            tip: 7,
            rows,
            truncated: false,
            decisions,
        };
        let mut charged = 0;
        let fitting = response
            .rows
            .iter()
            .take_while(|row| {
                let decision = response.decisions.get(&row.object.object_id);
                charged += serde_json::to_vec(&row_value(row, decision, 7))
                    .unwrap()
                    .len()
                    + 1;
                charged <= MAX_READ_ROW_BYTES
            })
            .count();
        assert_eq!(
            fitting, 7,
            "eight rows of an eighth of the budget each pass it"
        );
        assert_eq!(
            encode_response(&response, false).unwrap(),
            serde_json::to_vec(&body_value(&response, fitting, false, true)).unwrap()
        );
        let mut within = response;
        within.rows.truncate(fitting);
        assert_eq!(
            encode_response(&within, false).unwrap(),
            serde_json::to_vec(&body_value(&within, fitting, false, false)).unwrap()
        );
    }

    #[test]
    fn domain_selection_keeps_only_that_domains_decisions() {
        let selection = RowSelection::DomainDecisions("memory");
        assert!(selection.keeps(&row("decision", "memory")));
        assert!(
            !selection.keeps(&row("decision", "notes")),
            "a decision from another domain must not spend the memory read's budget"
        );
        assert!(
            !selection.keeps(&row("observation", "memory")),
            "an observation carries no decision and must not spend the budget"
        );
        let ids = ["memory-decision".to_string()];
        let selection = RowSelection::Objects(&ids);
        assert!(selection.keeps(&row("decision", "notes")));
        assert!(selection.keeps(&row("observation", "memory")));
    }
}

//! `kernel.read`: the visible rows of one surface at one snapshot, filtered
//! to the bound project, each carrying the mutation token `kernel.commit`
//! checks. Decision objects also carry their decision row, so a client can
//! render the decision text without a second route.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use host_runtime::RouteHandle;
use kernel::{
    ChunkRow, DecisionContent, DecisionRow, KernelError, KernelStore, ServingPosition, Surface,
    VisibleRow,
};
use serde::Deserialize;
use serde_json::Value;

use super::project::{ProjectBinding, ScopeFilter, stored_terms};
use super::serving;
use super::{InvalidReason, KernelOutcome, blocking, state_only};
use crate::HandlerCore;
use crate::dispatch::{PreparedOutcome, PreparedOutput, PreparedOutputError};
use crate::json_string::{escape, escape_marks, push_json_string};

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
    fn position(&self) -> ServingPosition<'_> {
        ServingPosition {
            created_commit_seq: self.created_commit_seq,
            object_id: &self.object_id,
        }
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

/// The end of the JSON string token that starts at `bytes[start]`, when the token is the encoding `serde_json`
/// writes for its text: [`escape`]'s bytes for a quote, a backslash, and each control byte, and every other byte
/// as it is. Encoding the text such a token decodes to writes the token again. The caller checks that `bytes` is
/// UTF-8.
fn canonical_string_end(bytes: &[u8], start: usize) -> Option<usize> {
    if *bytes.get(start)? != b'"' {
        return None;
    }
    let mut at = start + 1;
    loop {
        // A word with no quote, backslash, or control byte is string content.
        while let Some(word) = bytes.get(at..at + 8) {
            let marks = escape_marks(u64::from_le_bytes(word.try_into().unwrap_or_default()));
            if marks != 0 {
                at += (marks.trailing_zeros() / 8) as usize;
                break;
            }
            at += 8;
        }
        match *bytes.get(at)? {
            b'"' => return Some(at + 1),
            b'\\' => {
                let named = match *bytes.get(at + 1)? {
                    b'"' => b'"',
                    b'\\' => b'\\',
                    b'b' => 0x08,
                    b't' => b'\t',
                    b'n' => b'\n',
                    b'f' => 0x0c,
                    b'r' => b'\r',
                    b'u' => {
                        let digit = |at: usize| char::from(*bytes.get(at)?).to_digit(16);
                        (digit(at + 4)? << 4 | digit(at + 5)?) as u8
                    }
                    _ => return None,
                };
                // The escape is canonical when it is the bytes `serde_json` writes for the byte it names.
                let (written, len) = escape(named);
                if bytes.get(at..at + len)? != &written[..len] {
                    return None;
                }
                at += len;
            }
            0..=0x1f => return None,
            _ => at += 1,
        }
    }
}

/// A decision as a page serves it: its kind, and its payload as the JSON string tokens of the payload's two
/// fields, each the bytes `serde_json` writes for the field's text.
#[derive(Debug, PartialEq, Eq)]
struct PageDecision {
    decision_kind: String,
    /// The bytes that hold both tokens.
    text: Vec<u8>,
    summary: std::ops::Range<usize>,
    rationale: std::ops::Range<usize>,
}

impl PageDecision {
    /// The decision of a stored payload that is the text the kernel writes for a decision payload: the two
    /// fields in stored order, each string in `serde_json`'s encoding. The stored tokens are then the bytes that
    /// decoding the payload and encoding its fields would write, so the page serves them without that round
    /// trip. Any other text is `None`, and the page reads that decision by id.
    fn from_stored(content: DecisionContent) -> Option<Self> {
        const SUMMARY: &[u8] = b"{\"summary\":";
        const RATIONALE: &[u8] = b",\"rationale\":";
        let text = content.stored_payload;
        if !text.starts_with(SUMMARY) || std::str::from_utf8(&text).is_err() {
            return None;
        }
        let summary = SUMMARY.len()..canonical_string_end(&text, SUMMARY.len())?;
        if !text[summary.end..].starts_with(RATIONALE) {
            return None;
        }
        let rationale_start = summary.end + RATIONALE.len();
        let rationale = rationale_start..canonical_string_end(&text, rationale_start)?;
        if text[rationale.end..] != *b"}" {
            return None;
        }
        Some(Self {
            decision_kind: content.decision_kind,
            text,
            summary,
            rationale,
        })
    }

    fn from_row(decision: DecisionRow) -> Self {
        let payload = decision.payload;
        let mut text = Vec::with_capacity(payload.summary.len() + payload.rationale.len() + 4);
        push_json_string(&mut text, &payload.summary);
        let summary = 0..text.len();
        push_json_string(&mut text, &payload.rationale);
        Self {
            decision_kind: decision.decision_kind,
            rationale: summary.end..text.len(),
            summary,
            text,
        }
    }

    fn summary_json(&self) -> &[u8] {
        &self.text[self.summary.clone()]
    }

    fn rationale_json(&self) -> &[u8] {
        &self.text[self.rationale.clone()]
    }
}

/// One page of a read: rows in serving order, each decision object beside the content the read serves for it.
struct Page {
    known_as_of: i64,
    tip: i64,
    rows: Vec<(VisibleRow, Option<PageDecision>)>,
    /// Whether the read left rows out before the byte budget of the encoding applied.
    truncated: bool,
}

impl From<ReadResponse> for Page {
    fn from(mut response: ReadResponse) -> Self {
        let rows = response
            .rows
            .into_iter()
            .map(|row| {
                let decision = response.decisions.remove(&row.object.object_id);
                (row, decision.map(PageDecision::from_row))
            })
            .collect();
        Self {
            known_as_of: response.known_as_of,
            tip: response.tip,
            rows,
            truncated: response.truncated,
        }
    }
}

/// The fewest rows a page asks of one chunk. A page close to its row cap that the scope filter keeps thinning
/// reads the rows the filter drops in transactions of at least this many rows.
const MIN_CHUNK_ROWS: usize = 1024;

/// The smallest payload budget a page passes to one chunk: one worst-case row. A page close to its byte budget
/// reads the payloads the scope filter drops in transactions of at least this many bytes, and the chunk decodes at
/// most this much past the page.
const MIN_CHUNK_PAYLOAD_BYTES: u64 = (MAX_READ_ROW_BYTES / 8) as u64;

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
) -> Result<Page, KernelError> {
    let requested = match as_of {
        Some(as_of) => as_of,
        None => store.tip()?,
    };
    // The kernel keeps rows whose scope names another project out of each chunk, so the page decodes and judges the project's rows alone.
    let scope = project.scope_term();
    // `ScopeFilter` judges scope-term operators the kernel query keeps for the caller, so the row bound cannot be a SQL `LIMIT`; it applies here, after the filter.
    let mut filter = ScopeFilter::new(project);
    let mut terms = stored_terms(store);
    let mut rows: Vec<(VisibleRow, Option<PageDecision>)> = Vec::new();
    // Positions in `rows` of the decision objects whose chunk carried no payload the page can serve as stored.
    let mut unread: Vec<usize> = Vec::new();
    let mut payload_bytes: u64 = 0;
    let mut position = before.cloned();
    let read_chunk = |position: Option<&ReadCursor>, rows: usize, payload_bytes: u64| {
        store.visible_chunk_as_of_in_scope(
            surface,
            requested,
            position.map(ReadCursor::position),
            (MAX_READ_ROWS + 1 - rows).max(MIN_CHUNK_ROWS),
            (MAX_READ_ROW_BYTES as u64 - payload_bytes).max(MIN_CHUNK_PAYLOAD_BYTES),
            Some(scope),
        )
    };
    let mut chunk = read_chunk(position.as_ref(), 0, 0)?;
    let tip = chunk.tip;
    rows.reserve(chunk.rows.len().min(MAX_READ_ROWS));
    let truncated = 'page: loop {
        let last = chunk.rows.last().map(|entry| ReadCursor {
            created_commit_seq: entry.row.object.created_commit_seq,
            object_id: entry.row.object.object_id.clone(),
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
                // A visible decision-kind row without its decision row at the snapshot fails the read closed, as `read_visible_page` explains.
                let decision = decision.ok_or(KernelError::CorruptCanonicalRow)?;
                // A serialized row is at least its payload bytes, so cutting where cumulative payload size passes the budget keeps every row the serializer could keep.
                payload_bytes = payload_bytes.saturating_add(decision.payload_bytes);
                if payload_bytes > MAX_READ_ROW_BYTES as u64 {
                    break 'page true;
                }
                content = decision.content.and_then(PageDecision::from_stored);
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
        chunk = read_chunk(position.as_ref(), rows.len(), payload_bytes)?;
    };
    if !unread.is_empty() {
        // The page's payloads are within the byte budget, so the decisions read here are too.
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
            rows[index].1 = Some(PageDecision::from_row(decision));
        }
    }
    Ok(Page {
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
            && before.is_none_or(|cursor| cursor.position().precedes(&row.object))
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

fn write_optional_str(body: &mut Vec<u8>, value: Option<&str>) {
    match value {
        Some(value) => push_json_string(body, value),
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

/// Appends one row in its wire shape. Keys are written in the sorted order a `serde_json::Map` emits and values as
/// `serde_json` encodes them, so the bytes equal the encoding of the row's `json!` tree.
fn write_row(
    body: &mut Vec<u8>,
    row: &VisibleRow,
    decision: Option<&PageDecision>,
    known_as_of: i64,
) {
    let object = &row.object;
    body.extend_from_slice(b"{\"decision\":");
    match decision {
        Some(decision) => {
            body.extend_from_slice(b"{\"decision_kind\":");
            push_json_string(body, &decision.decision_kind);
            body.extend_from_slice(b",\"payload\":{\"rationale\":");
            body.extend_from_slice(decision.rationale_json());
            body.extend_from_slice(b",\"summary\":");
            body.extend_from_slice(decision.summary_json());
            body.extend_from_slice(b"}}");
        }
        None => body.extend_from_slice(b"null"),
    }
    body.extend_from_slice(b",\"labeled\":");
    body.extend_from_slice(if row.labeled { b"true" } else { b"false" });
    body.extend_from_slice(b",\"object\":{\"created_commit_seq\":");
    write_int(body, object.created_commit_seq);
    body.extend_from_slice(b",\"domain_id\":");
    push_json_string(body, &object.domain_id);
    body.extend_from_slice(b",\"invalidated_commit_seq\":");
    match object.invalidated_commit_seq {
        Some(invalidated) => write_int(body, invalidated),
        None => body.extend_from_slice(b"null"),
    }
    body.extend_from_slice(b",\"object_id\":");
    push_json_string(body, &object.object_id);
    body.extend_from_slice(b",\"object_kind\":");
    push_json_string(body, &object.object_kind);
    body.extend_from_slice(b",\"sensitivity\":");
    push_json_string(body, object.sensitivity.as_str());
    body.extend_from_slice(b",\"source_id\":");
    push_json_string(body, &object.source_id);
    body.extend_from_slice(b",\"source_kind\":");
    push_json_string(body, &object.source_kind);
    body.extend_from_slice(b",\"source_revision\":");
    write_int(body, object.source_revision);
    body.extend_from_slice(b",\"superseded_by\":");
    write_optional_str(body, object.superseded_by.as_deref());
    body.extend_from_slice(b"},\"scope_id\":");
    write_optional_str(body, row.scope_id.as_deref());
    body.extend_from_slice(b",\"token\":{\"known_as_of\":");
    write_int(body, known_as_of);
    body.extend_from_slice(b",\"object_id\":");
    push_json_string(body, &object.object_id);
    body.extend_from_slice(b"},\"visibility\":");
    push_json_string(body, row.visibility.as_str());
    body.push(b'}');
}

/// Encodes the read's response body: the envelope and the longest prefix of the page's rows whose encoding fits
/// [`MAX_READ_ROW_BYTES`]. Each row is encoded once, straight into the body, and a row that passes the budget is
/// taken back out. Rows are newest first, so the budget retains a contiguous prefix of the most recent rows.
/// `truncated` reports rows the read or the budget left out.
fn encode_page(page: &Page, gated: bool) -> Result<Vec<u8>, serde_json::Error> {
    // A typical memory row encodes to about 1 KiB; the envelope is under 200 bytes.
    let mut body =
        Vec::with_capacity(MAX_READ_ROW_BYTES.min(page.rows.len().saturating_mul(1024)) + 256);
    body.extend_from_slice(b"{\"gated\":");
    serde_json::to_writer(&mut body, &gated)?;
    body.extend_from_slice(b",\"known_as_of\":");
    serde_json::to_writer(&mut body, &page.known_as_of)?;
    body.extend_from_slice(b",\"rows\":[");
    let rows_start = body.len();
    let mut truncated = page.truncated;
    for (row, decision) in &page.rows {
        let row_start = body.len();
        if row_start > rows_start {
            body.push(b',');
        }
        write_row(&mut body, row, decision.as_ref(), page.known_as_of);
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
    serde_json::to_writer(&mut body, &page.tip)?;
    body.extend_from_slice(b",\"truncated\":");
    serde_json::to_writer(&mut body, &truncated)?;
    body.push(b'}');
    Ok(body)
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
            let page = match object_ids.as_deref() {
                Some(ids) => {
                    let selection = RowSelection::Objects(ids);
                    read_visible_page(&store, &project, surface, as_of, selection, before)?.into()
                }
                None => newest_page(&store, &project, surface, as_of, before)?,
            };
            Ok::<_, KernelError>(encode_page(&page, gated))
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
    use proptest::prelude::Strategy as _;
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

    fn content(decision: &DecisionRow) -> PageDecision {
        PageDecision::from_row(decision.clone())
    }

    /// The payload a page decision's tokens decode to.
    fn payload(decision: &PageDecision) -> DecisionPayload {
        DecisionPayload {
            summary: serde_json::from_slice(decision.summary_json()).unwrap(),
            rationale: serde_json::from_slice(decision.rationale_json()).unwrap(),
        }
    }

    /// The page decision of the payload text the kernel stores for `decision`.
    fn stored(decision: &DecisionRow) -> Option<PageDecision> {
        PageDecision::from_stored(DecisionContent {
            decision_kind: decision.decision_kind.clone(),
            stored_payload: serde_json::to_vec(&decision.payload).unwrap(),
        })
    }

    /// `to_value` of a struct emits its keys in sorted order, so this tree fixes field order too.
    fn row_value(row: &VisibleRow, decision: Option<&PageDecision>, known_as_of: i64) -> Value {
        json!({
            "object": row.object,
            "visibility": row.visibility.as_str(),
            "labeled": row.labeled,
            "scope_id": row.scope_id,
            "token": {"object_id": row.object.object_id, "known_as_of": known_as_of},
            "decision": decision.map(|decision| json!({
                "decision_kind": decision.decision_kind,
                "payload": payload(decision),
            })),
        })
    }

    /// The body `serde_json` encodes for the first `served` rows of `page`.
    fn body_bytes(page: &Page, served: usize, gated: bool, truncated: bool) -> String {
        let rows: Vec<Value> = page.rows[..served]
            .iter()
            .map(|(row, decision)| row_value(row, decision.as_ref(), page.known_as_of))
            .collect();
        serde_json::to_string(&json!({
            "known_as_of": page.known_as_of,
            "tip": page.tip,
            "gated": gated,
            "truncated": truncated,
            "rows": rows,
            "state": {"kind": "available"},
        }))
        .unwrap()
    }

    fn encoded(page: &Page, gated: bool) -> String {
        String::from_utf8(encode_page(page, gated).unwrap()).unwrap()
    }

    /// A column added to `ObjectRow` or `DecisionPayload` fails here instead of silently leaving the wire.
    #[test]
    fn a_written_row_carries_every_field_of_the_kernel_structs() {
        let row = populated_row();
        let decision = decision(&row.object.object_id);
        let mut written = Vec::new();
        write_row(&mut written, &row, Some(&content(&decision)), 7);
        let value: Value = serde_json::from_slice(&written).unwrap();

        assert_eq!(value["object"], serde_json::to_value(&row.object).unwrap());
        assert_eq!(
            value["decision"]["payload"],
            serde_json::to_value(&decision.payload).unwrap()
        );
        assert_eq!(
            String::from_utf8(written).unwrap(),
            serde_json::to_string(&row_value(&row, Some(&content(&decision)), 7)).unwrap(),
            "a row must write its keys in the sorted order a Value map emits"
        );
    }

    proptest::proptest! {
        #[test]
        fn any_integer_is_written_as_serde_json_writes_it(value in proptest::prelude::any::<i64>()) {
            let mut written = Vec::new();
            write_int(&mut written, value);
            proptest::prop_assert_eq!(String::from_utf8(written).unwrap(), serde_json::to_string(&value).unwrap());
        }

        /// The token of any text ends where `serde_json` ends it, at every offset into a word and with any bytes
        /// after it.
        #[test]
        fn the_encoding_of_any_text_is_a_canonical_token(
            text in proptest::prelude::any::<String>(),
            lead in 0usize..24,
            trail in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..24),
        ) {
            let token = serde_json::to_vec(&text).unwrap();
            let mut bytes = vec![b'a'; lead];
            bytes.extend_from_slice(&token);
            bytes.extend_from_slice(&trail);
            proptest::prop_assert_eq!(canonical_string_end(&bytes, lead), Some(lead + token.len()));
        }

        /// Bytes the scan accepts as a token decode to a text whose encoding is those bytes, so serving them
        /// equals decoding and encoding again.
        #[test]
        fn an_accepted_token_is_the_encoding_of_its_text(
            pieces in proptest::collection::vec(
                proptest::prop_oneof![
                    proptest::sample::select(vec![
                        "\"", "\\", "\\\\", "\\\"", "\\u", "\\u00", "\\u001", "\\u00", "0", "1", "a", "f", "A", "F", "b",
                        "t", "n", "r", "/", "\u{7f}", "\u{1f}", "\n", "é", "\u{1F600}", "  ", "xxxxxxx",
                    ]).prop_map(str::to_string),
                    proptest::prelude::any::<char>().prop_map(String::from),
                ],
                0..24,
            ),
            lead in 0usize..12,
        ) {
            let mut bytes = vec![b'a'; lead];
            bytes.push(b'"');
            bytes.extend_from_slice(pieces.concat().as_bytes());
            if let Some(end) = canonical_string_end(&bytes, lead) {
                let token = &bytes[lead..end];
                let text: String = serde_json::from_slice(token).unwrap();
                proptest::prop_assert_eq!(serde_json::to_vec(&text).unwrap(), token);
            }
        }
    }

    /// Every escape `serde_json` reads is a canonical token exactly when `serde_json` writes that escape back.
    #[test]
    fn an_escape_is_canonical_exactly_when_serde_json_writes_it() {
        let canonical = |token: &[u8]| {
            let accepted = canonical_string_end(token, 0) == Some(token.len());
            let rewritten = serde_json::from_slice::<String>(token)
                .is_ok_and(|text| serde_json::to_vec(&text).unwrap() == token);
            assert_eq!(accepted, rewritten, "{:?}", String::from_utf8_lossy(token));
            accepted
        };
        let mut short = 0;
        for byte in 0u8..=0x7f {
            short += usize::from(canonical(&[b'"', b'\\', byte, b'"']));
            // A raw control byte, quote, or backslash is no string content.
            canonical(&[b'"', byte, b'"']);
        }
        assert_eq!(short, 7, "the two-byte escapes serde_json writes");
        let hex = b"0123456789abcdefABCDEF";
        let mut long = 0;
        for a in hex {
            for b in hex {
                for c in hex {
                    for d in hex {
                        long += usize::from(canonical(&[b'"', b'\\', b'u', *a, *b, *c, *d, b'"']));
                    }
                }
            }
        }
        assert_eq!(long, 0x20 - 5, "the control bytes with no two-byte escape");
        for token in [
            &b""[..],
            b"\"",
            b"\"abc",
            b"\"abc\\",
            b"\"\\u00",
            b"\"\\u001",
            b"abc\"",
        ] {
            assert_eq!(canonical_string_end(token, 0), None, "{token:?}");
        }
    }

    /// A stored payload in the kernel's text becomes the tokens that encoding its fields writes. A payload in any
    /// other text, valid JSON or not, is left to the read by id.
    #[test]
    fn a_stored_payload_is_served_as_stored_only_in_the_kernels_text() {
        let mut decision = decision("memory-decision");
        decision.payload.summary =
            "quote \" slash \\ tab \t bell \u{7} é \u{1F600} \u{7f}".to_string();
        assert_eq!(payload(&stored(&decision).unwrap()), decision.payload);
        let (from_stored, from_row) = (stored(&decision).unwrap(), content(&decision));
        assert_eq!(from_stored.decision_kind, from_row.decision_kind);
        assert_eq!(from_stored.summary_json(), from_row.summary_json());
        assert_eq!(from_stored.rationale_json(), from_row.rationale_json());

        let refused = |stored_payload: &[u8]| {
            PageDecision::from_stored(DecisionContent {
                decision_kind: "fact".to_string(),
                stored_payload: stored_payload.to_vec(),
            })
            .is_none()
        };
        assert!(!refused(br#"{"summary":"s","rationale":"r"}"#));
        for other in [
            &br#"{"rationale":"r","summary":"s"}"#[..],
            br#"{"summary": "s","rationale":"r"}"#,
            br#"{"summary":"s","rationale":"r"} "#,
            br#"{"summary":"s","rationale":"r","extra":1}"#,
            br#"{"summary":"s","rationale":"r"}}"#,
            br#"{"summary":"\u0073","rationale":"r"}"#,
            br#"{"summary":"s\/","rationale":"r"}"#,
            br#"{"summary":"s","rationale":"r""#,
            br#"{"summary":"s","rationale":7}"#,
            br#"{"summary":"s"}"#,
            b"{\"summary\":\"\xff\",\"rationale\":\"r\"}",
            b"{\"summary\":\"s\",\"rationale\":\"\xc3\"}",
            b"{}",
            b"",
        ] {
            assert!(refused(other), "{:?}", String::from_utf8_lossy(other));
        }
    }

    #[test]
    fn integer_extremes_are_written_as_serde_json_writes_them() {
        for value in [0, 1, -1, 9, 10, -10, i64::MAX, i64::MIN] {
            let mut written = Vec::new();
            write_int(&mut written, value);
            assert_eq!(
                String::from_utf8(written).unwrap(),
                serde_json::to_string(&value).unwrap()
            );
        }
    }

    #[test]
    fn read_body_bytes_match_the_value_tree_encoding() {
        let populated = populated_row();
        let decision = decision(&populated.object.object_id);
        let mut page = Page {
            known_as_of: 41,
            tip: 42,
            rows: vec![
                (populated.clone(), Some(content(&decision))),
                (populated, stored(&decision)),
                (row("observation", "notes"), None),
            ],
            truncated: true,
        };
        for (gated, truncated) in [(true, true), (false, false)] {
            page.truncated = truncated;
            let body = encode_page(&page, gated).unwrap();
            assert_eq!(
                String::from_utf8(body.clone()).unwrap(),
                body_bytes(&page, 3, gated, truncated)
            );
            let output = PreparedOutput::cached_bytes(body.clone());
            assert!(output.serializes_directly());
            assert_eq!(output.measure().unwrap().len(), body.len());
        }
        page.rows.clear();
        assert_eq!(encoded(&page, false), body_bytes(&page, 0, false, false));
    }

    /// An id-filtered read's response becomes the page that holds its rows in order, each decision object beside
    /// its decision.
    #[test]
    fn a_response_becomes_the_page_with_its_rows_and_decisions() {
        let populated = populated_row();
        let bare = row("observation", "notes");
        let decision = decision(&populated.object.object_id);
        let response = ReadResponse {
            known_as_of: 41,
            tip: 42,
            rows: vec![populated.clone(), bare.clone()],
            truncated: true,
            decisions: HashMap::from([(populated.object.object_id.clone(), decision.clone())]),
        };
        let page = Page::from(response);
        assert_eq!((page.known_as_of, page.tip, page.truncated), (41, 42, true));
        assert_eq!(
            page.rows,
            vec![(populated, Some(content(&decision))), (bare, None)]
        );
    }

    /// A page of nine decisions whose rationales take `fill` bytes in all but the last, which takes `last`.
    fn page_of_nine(fill: usize, last: usize) -> Page {
        let rows = (0..9)
            .map(|index| {
                let mut row = row("decision", "memory");
                row.object.object_id = format!("memory-decision-{index}");
                let mut decision = decision(&row.object.object_id);
                decision.payload.rationale = "r".repeat(if index == 8 { last } else { fill });
                (row, Some(content(&decision)))
            })
            .collect();
        Page {
            known_as_of: 7,
            tip: 7,
            rows,
            truncated: false,
        }
    }

    /// The bytes the budget charges a page's first `rows` rows: each row's encoding and one separator byte.
    fn charged(page: &Page, rows: usize) -> usize {
        page.rows[..rows]
            .iter()
            .map(|(row, decision)| {
                serde_json::to_vec(&row_value(row, decision.as_ref(), page.known_as_of))
                    .unwrap()
                    .len()
                    + 1
            })
            .sum()
    }

    /// The body keeps the longest prefix of rows whose encodings, each charged one separator byte, fit the
    /// budget, and reports the rows it leaves out as truncation. A last row charged exactly the budget stays; one
    /// byte more takes it out.
    #[test]
    fn the_byte_budget_keeps_the_longest_row_prefix_that_fits() {
        let fill = MAX_READ_ROW_BYTES / 16;
        let room = MAX_READ_ROW_BYTES - charged(&page_of_nine(fill, 0), 9);
        assert!(
            room > 0,
            "nine rows of a sixteenth of the budget leave room"
        );

        let exact = page_of_nine(fill, room);
        assert_eq!(charged(&exact, 9), MAX_READ_ROW_BYTES);
        assert_eq!(encoded(&exact, false), body_bytes(&exact, 9, false, false));

        let over = page_of_nine(fill, room + 1);
        assert_eq!(charged(&over, 9), MAX_READ_ROW_BYTES + 1);
        assert_eq!(encoded(&over, false), body_bytes(&over, 8, false, true));

        let first_row_over = page_of_nine(MAX_READ_ROW_BYTES, 0);
        assert_eq!(
            encoded(&first_row_over, false),
            body_bytes(&first_row_over, 0, false, true)
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

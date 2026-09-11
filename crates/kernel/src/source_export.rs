//! Exports the exact text of the source descriptors a hold protects, in
//! stable `(class, object_id, revision)` pages. A fixed-S page lists the
//! descriptors live at S; a catch-up page lists what changed in `(S, through]`:
//! the descriptors created there, with their text, and the invalidation of
//! descriptors that were live at S. Every page is admitted from stored sizes
//! before any object is opened: a row that would overflow the page is deferred
//! whole to the next page, and a row that can never fit fails the page without
//! being decoded. The read transaction closes before any byte is read, and
//! nothing is loaded and then truncated. Admission charges are logical counts;
//! they are not a measurement of the exporter's live heap.

use std::collections::{HashMap, HashSet};
use std::num::{NonZeroU64, NonZeroUsize};
use std::time::Instant;

use rusqlite::TransactionBehavior;

use super::cas::{ArtifactError, ArtifactErrorKind, is_artifact_digest};
use super::slice::ObservationPayload;
use super::source_descriptor::{
    SOURCE_DESCRIPTOR_DETAIL_VERSION, SourceDescriptorDetail, descriptor_object_id,
};
use super::source_hold::{
    Descriptors, HeldCursor, Keyset, SourceHoldBinding, SourceHoldError, check_coverage,
    check_window, descriptor_rows_sql,
};
use super::source_identity::{
    Occurrence, Span, encode_metadata, payload_id, select, validate_span,
};
use super::{KernelError, KernelStore, Sensitivity, map_sqlite};

#[cfg(feature = "test-support")]
thread_local! {
    static RAW_ROWS_READ: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static AFTER_SNAPSHOT: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

/// Which descriptors a page exports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportWindow {
    /// Descriptors live at S, each with its text.
    Snapshot,
    /// Descriptors created in `(S, through]`, each with its text, and
    /// descriptors live at S that were invalidated in the window, without text.
    CatchUp { through: i64 },
    /// One step of catch-up: descriptors created in `(after, through]`, each with its text, and descriptors live at `after` that were invalidated in the window, without text.
    /// `after` is a complete commit at or after S that an earlier window already delivered, so the hold must cover `(S, through]`.
    Delta { after: i64, through: i64 },
}

/// An exporter-issued continuation for one hold and one fixed window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceCursor {
    hold_id: String,
    window: ExportWindow,
    key: HeldCursor,
}

/// Logical admission bounds for one page, judged before any object is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourcePageBounds {
    pub max_rows: NonZeroUsize,
    /// Stored bytes of every distinct artifact the page reads.
    pub max_encoded_bytes: NonZeroU64,
    /// Bytes of text the page hands back, the logical decoded charge.
    pub max_decoded_bytes: NonZeroU64,
    /// Text one row may carry; a larger row can never fit and fails the page.
    pub max_row_bytes: NonZeroU64,
}

/// What one page charged against its bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SourcePageCharge {
    pub rows: usize,
    pub encoded_bytes: u64,
    pub decoded_bytes: u64,
}

/// One exported descriptor. `Debug` renders the text as its byte length so
/// payload never reaches a log through a formatted row.
#[derive(Clone, PartialEq, Eq)]
pub struct SourceRow {
    pub object_id: String,
    pub revision: i64,
    /// A spanned invalidation-only row carries a format-checked `payload_id`; its absent payload bytes cannot be verified.
    pub detail: SourceDescriptorDetail,
    /// The stable domain identifier the descriptor row belongs to.
    pub domain_id: String,
    pub sensitivity: Sensitivity,
    pub created_commit_seq: i64,
    /// The invalidating commit at or before the export window's end, or `None`
    /// if the descriptor is live at that boundary.
    pub invalidated_commit_seq: Option<i64>,
    /// The newer revision that superseded this descriptor at or before the export window's end, or `None` for a retirement or a descriptor live at that boundary.
    pub superseded_by: Option<String>,
    /// The exact UTF-8 text the descriptor's span selects, present exactly
    /// when the row's creation lies inside the exported window.
    /// Payload bytes are read and verified only for rows that export text.
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcePage {
    pub rows: Vec<SourceRow>,
    /// `None` when this page ends the window's inventory.
    pub next: Option<SourceCursor>,
    pub charge: SourcePageCharge,
}

impl std::fmt::Debug for SourceRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceRow")
            .field("object_id", &self.object_id)
            .field("revision", &self.revision)
            .field("detail", &self.detail)
            .field("domain_id", &self.domain_id)
            .field("sensitivity", &self.sensitivity)
            .field("created_commit_seq", &self.created_commit_seq)
            .field("invalidated_commit_seq", &self.invalidated_commit_seq)
            .field("superseded_by", &self.superseded_by)
            .field("text_bytes", &self.text.as_ref().map(String::len))
            .finish()
    }
}

/// The bound a row could never fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageBound {
    /// `max_row_bytes`.
    Row,
    /// `max_decoded_bytes`.
    Decoded,
    /// `max_encoded_bytes`.
    Encoded,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourceExportError {
    /// `SourceHoldError::Kernel` converts to [`Self::Kernel`], so `Hold`
    /// never contains a [`KernelError`].
    #[error(transparent)]
    Hold(SourceHoldError),
    #[error(
        "source row {object_id} charges {bytes} bytes against {bound:?}, more than an empty page admits"
    )]
    OversizedRow {
        object_id: String,
        bound: PageBound,
        bytes: u64,
    },
    #[error("source row {object_id} has a stored detail or span the exporter cannot honour")]
    MalformedRow { object_id: String },
    #[error("source row {object_id} cites bytes that cannot be read exactly: {kind:?}")]
    BytesUnavailable {
        object_id: String,
        kind: ArtifactErrorKind,
    },
    #[error(transparent)]
    Kernel(#[from] KernelError),
}

impl From<SourceHoldError> for SourceExportError {
    fn from(error: SourceHoldError) -> Self {
        match error {
            SourceHoldError::Kernel(kernel) => Self::Kernel(kernel),
            other => Self::Hold(other),
        }
    }
}

impl From<rusqlite::Error> for SourceExportError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Kernel(map_sqlite(error))
    }
}

/// A row as the preflight sees it: everything needed to charge it, nothing
/// read from disk yet.
struct Preflight {
    row: SourceRow,
    digest: String,
    byte_length: u64,
    span: Option<Span>,
    exports_text: bool,
}

impl Preflight {
    fn text_bytes(&self) -> u64 {
        match (self.exports_text, self.span) {
            (false, _) => 0,
            (true, None) => self.byte_length,
            (true, Some(span)) => span.end.saturating_sub(span.start),
        }
    }
}

impl KernelStore {
    /// Runs a one-shot callback after the export transaction, scoped to `operation` on this thread.
    #[cfg(feature = "test-support")]
    pub fn with_source_export_after_snapshot_hook_for_test<T>(
        hook: impl FnOnce() + 'static,
        operation: impl FnOnce() -> T,
    ) -> T {
        struct ResetHook;
        impl Drop for ResetHook {
            fn drop(&mut self) {
                AFTER_SNAPSHOT.with(|slot| slot.borrow_mut().take());
            }
        }
        AFTER_SNAPSHOT.with(|slot| {
            let mut slot = slot.borrow_mut();
            assert!(slot.is_none(), "source export hook is already installed");
            *slot = Some(Box::new(hook));
        });
        let _reset = ResetHook;
        operation()
    }

    /// Returns and resets the number of source-export SQL rows read on this thread.
    #[cfg(feature = "test-support")]
    pub fn take_source_export_row_reads_for_test() -> usize {
        RAW_ROWS_READ.with(|count| count.replace(0))
    }

    /// One admitted page of `window` after `cursor`, read at the hold's S in
    /// one short transaction and materialized from the object store after
    /// that transaction closes. The hold must be valid at `now`. A catch-up
    /// window's first page also proves the hold is extended through its end;
    /// coverage remains valid while `through` is fixed and the hold remains
    /// valid, so a page continued from a cursor does not repeat that proof.
    /// A cursor from another hold or window is rejected before reading the store.
    /// Pin metadata is rechecked after materialization, with elapsed monotonic time added to `now`.
    /// This check does not synchronize a later publication.
    pub fn export_source_page(
        &self,
        binding: &SourceHoldBinding,
        hold_id: &str,
        now: i64,
        window: ExportWindow,
        cursor: Option<&SourceCursor>,
        bounds: SourcePageBounds,
    ) -> Result<SourcePage, SourceExportError> {
        let started = Instant::now();
        if cursor.is_some_and(|cursor| cursor.hold_id != hold_id || cursor.window != window) {
            return Err(SourceHoldError::InvalidRequest.into());
        }
        let (admitted, next, charge) = {
            let mut reader = self.lock_reader()?;
            let tx = reader.transaction_with_behavior(TransactionBehavior::Deferred)?;
            let pin = self.load_valid_pin(&tx, binding, hold_id, now)?;
            let (body, end, start) = match window {
                ExportWindow::Snapshot => {
                    (Descriptors::LiveAtEnd.cited_evidence_sql(), pin.snapshot, 0)
                }
                ExportWindow::CatchUp { through } => {
                    check_window(&tx, pin.snapshot, through)?;
                    if cursor.is_none() {
                        check_coverage(&tx, pin.snapshot, hold_id, through)?;
                    }
                    (catch_up_body(), through, pin.snapshot)
                }
                ExportWindow::Delta { after, through } => {
                    check_window(&tx, pin.snapshot, through)?;
                    if after < pin.snapshot || after > through {
                        return Err(SourceHoldError::InvalidRequest.into());
                    }
                    if cursor.is_none() {
                        check_coverage(&tx, pin.snapshot, hold_id, through)?;
                    }
                    (catch_up_body(), through, after)
                }
            };
            let keyset = Keyset::after(cursor.map(|cursor| &cursor.key), bounds.max_rows);
            let mut statement = tx.prepare_cached(&Keyset::sql(ROW_SELECT, &body))?;
            let rows = statement.query_map(
                rusqlite::params_from_iter(keyset.params(end, start)),
                |row| {
                    #[cfg(feature = "test-support")]
                    RAW_ROWS_READ.with(|count| count.set(count.get() + 1));
                    Ok(RawRow {
                        class: row.get(0)?,
                        object_id: row.get(1)?,
                        revision: row.get(2)?,
                        evidence_id: row.get(3)?,
                        digest: row.get(4)?,
                        byte_length: row.get(5)?,
                        payload: row.get(6)?,
                        created: row.get(7)?,
                        invalidated: row.get(8)?,
                        observation_created: row.get(9)?,
                        observation_invalidated: row.get(10)?,
                        domain_id: row.get(11)?,
                        sensitivity: row.get(12)?,
                        superseded_by: row.get(13)?,
                    })
                },
            )?;
            admit(rows, pin.snapshot, window, bounds, &keyset)?
        };
        // The read transaction is closed; only admitted rows touch the disk.
        #[cfg(feature = "test-support")]
        if let Some(hook) = AFTER_SNAPSHOT.with(|slot| slot.borrow_mut().take()) {
            hook();
        }
        let rows = self.materialize(admitted)?;
        let mut reader = self.lock_reader()?;
        let tx = reader.transaction_with_behavior(TransactionBehavior::Deferred)?;
        self.recheck_source_hold_after_read(&tx, binding, hold_id, now, started)?;
        let next = next.map(|key| SourceCursor {
            hold_id: hold_id.to_string(),
            window,
            key,
        });
        Ok(SourcePage { rows, next, charge })
    }

    /// Reads each distinct artifact once, verifies its digest and stored
    /// length, and slices every admitted row's span out of it as exact UTF-8.
    fn materialize(&self, admitted: Vec<Preflight>) -> Result<Vec<SourceRow>, SourceExportError> {
        let mut buffers: HashMap<String, String> = HashMap::new();
        let mut rows = Vec::with_capacity(admitted.len());
        for preflight in admitted {
            let mut row = preflight.row;
            if preflight.exports_text {
                if !buffers.contains_key(&preflight.digest) {
                    let bytes = self
                        .read_verified_object(&preflight.digest)
                        .map_err(|error| bytes_unavailable(&row.object_id, error))?;
                    let buffer = String::from_utf8(bytes).map_err(|_| malformed(&row.object_id))?;
                    buffers.insert(preflight.digest.clone(), buffer);
                }
                let buffer = &buffers[&preflight.digest];
                if u64::try_from(buffer.len()).ok() != Some(preflight.byte_length) {
                    return Err(bytes_unavailable(
                        &row.object_id,
                        ArtifactError::new(ArtifactErrorKind::CorruptObject),
                    ));
                }
                validate_span(preflight.span, buffer).map_err(|_| malformed(&row.object_id))?;
                let selected = select(preflight.span, buffer);
                if preflight.span.is_some() && payload_id(selected) != row.detail.payload_id {
                    return Err(malformed(&row.object_id));
                }
                row.text = Some(
                    std::str::from_utf8(selected)
                        .map_err(|_| malformed(&row.object_id))?
                        .to_string(),
                );
            }
            rows.push(row);
        }
        Ok(rows)
    }
}

struct RawRow {
    class: String,
    object_id: String,
    revision: i64,
    evidence_id: String,
    digest: String,
    byte_length: i64,
    payload: Vec<u8>,
    created: i64,
    invalidated: Option<i64>,
    domain_id: String,
    sensitivity: String,
    superseded_by: Option<String>,
    observation_created: i64,
    observation_invalidated: Option<i64>,
}

const ROW_SELECT: &str = "o.source_kind,o.object_id,o.source_revision,e.evidence_id,
     e.artifact_digest,e.byte_length,b.observation_payload,
     o.created_commit_seq,o.invalidated_commit_seq,
     b.created_commit_seq,b.invalidated_commit_seq,o.domain_id,b.sensitivity_class,
     o.superseded_by";

/// The catch-up window as one row source: the descriptors created in
/// `(start, through]` exactly as the hold pins them, or the descriptors live at
/// `start` that were invalidated inside the window, which carry the
/// invalidation fact and export no text. `?1` is `through` and `?2` is the
/// window's start, S for a whole catch-up and `after` for one delta.
fn catch_up_body() -> String {
    format!(
        "{rows} AND (({created}) OR ({live_at_s}
               AND o.invalidated_commit_seq>?2 AND o.invalidated_commit_seq<=?1))",
        rows = descriptor_rows_sql("idx_objects_source_descriptor_page"),
        created = Descriptors::CreatedInWindow.predicate("?1", "?2"),
        live_at_s = Descriptors::LiveAtEnd.predicate("?2", "0"),
    )
}

/// Charges rows in order against `bounds` without opening any object. The
/// first row that would overflow the page ends it and becomes the next page's
/// first row; a row that cannot fit an empty page fails. Returns the admitted
/// rows, the cursor for the next page, and the charge.
fn admit(
    rows: impl Iterator<Item = rusqlite::Result<RawRow>>,
    snapshot: i64,
    window: ExportWindow,
    bounds: SourcePageBounds,
    keyset: &Keyset<'_>,
) -> Result<(Vec<Preflight>, Option<HeldCursor>, SourcePageCharge), SourceExportError> {
    let mut admitted: Vec<Preflight> = Vec::new();
    let mut charge = SourcePageCharge::default();
    let mut charged_digests: HashSet<String> = HashSet::new();
    let mut continues = false;
    for raw in rows {
        let raw = raw?;
        if admitted.len() == bounds.max_rows.get() {
            continues = true;
            break;
        }
        let preflight = preflight(raw, snapshot, window)?;
        let text_bytes = preflight.text_bytes();
        // A row an empty page could not admit can never be exported at these
        // bounds; it fails before anything is read rather than being skipped.
        let oversized = if text_bytes > bounds.max_row_bytes.get() {
            Some((PageBound::Row, text_bytes))
        } else if text_bytes > bounds.max_decoded_bytes.get() {
            Some((PageBound::Decoded, text_bytes))
        } else if preflight.exports_text && preflight.byte_length > bounds.max_encoded_bytes.get() {
            Some((PageBound::Encoded, preflight.byte_length))
        } else {
            None
        };
        if let Some((bound, bytes)) = oversized {
            return Err(SourceExportError::OversizedRow {
                object_id: preflight.row.object_id,
                bound,
                bytes,
            });
        }
        let encoded = if preflight.exports_text && !charged_digests.contains(&preflight.digest) {
            preflight.byte_length
        } else {
            0
        };
        let fits = charge.encoded_bytes.saturating_add(encoded) <= bounds.max_encoded_bytes.get()
            && charge.decoded_bytes.saturating_add(text_bytes) <= bounds.max_decoded_bytes.get();
        if !fits {
            continues = true;
            break;
        }
        charge.rows += 1;
        charge.encoded_bytes += encoded;
        charge.decoded_bytes += text_bytes;
        if preflight.exports_text {
            charged_digests.insert(preflight.digest.clone());
        }
        admitted.push(preflight);
    }
    let next = keyset.finish(&mut admitted, continues, |last| HeldCursor {
        class: last.row.detail.class.clone(),
        object_id: last.row.object_id.clone(),
        revision: last.row.revision,
    });
    // An admissible first row always fits an empty page; a continuing page
    // with no row to resume after would lose the rest of the inventory.
    if continues && next.is_none() {
        return Err(KernelError::CorruptCanonicalRow.into());
    }
    Ok((admitted, next, charge))
}

fn preflight(
    raw: RawRow,
    snapshot: i64,
    window: ExportWindow,
) -> Result<Preflight, SourceExportError> {
    let object_id = raw.object_id;
    if raw.created != raw.observation_created || raw.invalidated != raw.observation_invalidated {
        return Err(malformed(&object_id));
    }
    let payload: ObservationPayload =
        serde_json::from_slice(&raw.payload).map_err(|_| malformed(&object_id))?;
    let detail: SourceDescriptorDetail = payload
        .detail
        .as_deref()
        .and_then(|detail| serde_json::from_str(detail).ok())
        .ok_or_else(|| malformed(&object_id))?;
    if detail.descriptor_version != SOURCE_DESCRIPTOR_DETAIL_VERSION
        || detail.class != raw.class
        || detail.revision != raw.revision.to_string()
        || descriptor_object_id(&detail.lineage_id, &detail.revision) != object_id
        || detail.evidence_id != raw.evidence_id
        || detail.artifact_digest != raw.digest
        || !is_artifact_digest(&raw.digest)
    {
        return Err(malformed(&object_id));
    }
    let byte_length = u64::try_from(raw.byte_length).map_err(|_| malformed(&object_id))?;
    if let Some((start, end)) = detail.span
        && (start > end || end > byte_length)
    {
        return Err(malformed(&object_id));
    }
    let span = detail.span.map(|(start, end)| Span { start, end });
    let identity: Vec<_> = detail
        .identity
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let encoded = encode_metadata(
        &Occurrence {
            class: &detail.class,
            identity: &identity,
            revision: &detail.revision,
            representation: &detail.representation,
            span,
        },
        byte_length,
    )
    .map_err(|_| malformed(&object_id))?;
    if encoded.occurrence_id != detail.occurrence_id
        || encoded.lineage_id != detail.lineage_id
        || encoded.tuple != detail.occurrence_tuple
        || encoded.span != span
        || !is_artifact_digest(&detail.payload_id)
        || (span.is_none() && detail.payload_id != raw.digest)
    {
        return Err(malformed(&object_id));
    }
    detail
        .source_policy
        .validate_for(encoded.class)
        .map_err(|_| malformed(&object_id))?;
    let (exports_text, end) = match window {
        ExportWindow::Snapshot => (true, snapshot),
        ExportWindow::CatchUp { through } => (raw.created > snapshot, through),
        ExportWindow::Delta { after, through } => (raw.created > after, through),
    };
    let invalidated = raw.invalidated.filter(|at| *at <= end);
    Ok(Preflight {
        span,
        row: SourceRow {
            object_id,
            revision: raw.revision,
            detail,
            domain_id: raw.domain_id,
            sensitivity: Sensitivity::from_stored(&raw.sensitivity),
            created_commit_seq: raw.created,
            invalidated_commit_seq: invalidated,
            superseded_by: raw.superseded_by.filter(|_| invalidated.is_some()),
            text: None,
        },
        digest: raw.digest,
        byte_length,
        exports_text,
    })
}

fn malformed(object_id: &str) -> SourceExportError {
    SourceExportError::MalformedRow {
        object_id: object_id.to_string(),
    }
}

fn bytes_unavailable(object_id: &str, error: ArtifactError) -> SourceExportError {
    SourceExportError::BytesUnavailable {
        object_id: object_id.to_string(),
        kind: error.kind(),
    }
}

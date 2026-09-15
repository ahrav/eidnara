//! Live claim occurrences in the projection, classified from canonical facts
//! read at one kernel snapshot.
//!
//! The projection stores candidates. It never stores a claim's state, so a
//! projection that lags the kernel cannot revive a retracted, superseded, or
//! hidden claim: the state is derived from `KernelStore::claim_facts_as_of`
//! every time it is asked for. `classify` takes no causal class, so `Unknown`
//! lineage cannot change a candidate's state, and no field here carries a
//! score, a boost, or a corroboration count.

use std::collections::{BTreeSet, HashMap};
use std::num::NonZeroUsize;

use kernel::source_identity::{
    OccurrenceClass, Span, derived_lineage_id, identity_digest, occurrence_identity_matches,
};
use kernel::{
    ClaimFactBounds, ClaimFacts, ClaimFactsError, Disposition, KernelStore, ServedStanding,
    SurfaceVisibility,
};
use rusqlite::params;
use storage::GuardedConn;

use crate::exact::selector::Family;
use crate::exact::{CANONICAL_OBJECT_NAMESPACE, Coverage, EXTRACTION_VERSION, coverage};
use crate::{ProjectionError, decode_span, read_identity};

/// Variant order is precedence order: restrictive states sort first, so a
/// classified claim is never more visible than the kernel's serving view of the
/// claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CandidateState {
    /// The claim object has no registry row at the snapshot, was invalidated
    /// without a successor, or no longer lists this row's descriptor as live
    /// (the export's rule: descriptor and evidence both live), so catch-up will
    /// tombstone the row.
    Retracted,
    /// The claim object was replaced by a successor, or its own or lineage
    /// admission disposition is `Superseded`.
    Superseded,
    /// The own or lineage admission disposition is `Rejected`, `Contradicted`,
    /// or `Quarantined`, or the serving view lists no row for the object or
    /// lists it hidden on the widest surface.
    Hidden,
    /// The own or lineage admission disposition is `Stale`, or the occurrence
    /// carries a revision other than the object's canonical one. The registry
    /// never changes an object's revision, so the second input can only come
    /// from a corrupt projection row and is kept as a guard.
    Stale,
    /// Served on the widest surface with an `Active` or `Disputed` disposition;
    /// a disputed claim serves labeled, and the label travels with the served
    /// facts rather than as a state.
    Current,
}

/// One live projection row of a claim, keyed to the decision object through
/// the `canonical_object` exact association every claim occurrence carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimCandidateRow {
    pub occurrence_id: String,
    pub class: OccurrenceClass,
    pub representation: String,
    pub object_id: String,
    pub revision: i64,
}

/// One classified candidate. `claim` indexes `ClaimCandidateBatch::claims`;
/// `None` when the object has no registry row at the snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimCandidate {
    pub row: ClaimCandidateRow,
    pub state: CandidateState,
    pub claim: Option<usize>,
}

/// The projection rows are read first and the kernel facts after, so a row the
/// kernel has moved past classifies from the newer facts; reading in the other
/// order could show a row as Current after the kernel withdrew it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimCandidateBatch {
    /// The kernel snapshot the canonical facts were read at.
    pub known_as_of: i64,
    /// The facts of every distinct claim object the rows name, in the order the
    /// objects were first seen.
    pub claims: Vec<ClaimFacts>,
    pub candidates: Vec<ClaimCandidate>,
}

impl ClaimCandidateBatch {
    /// `None` when the object has no registry row at the snapshot, or when
    /// `candidate` came from another batch and its index names another object.
    pub fn claim(&self, candidate: &ClaimCandidate) -> Option<&ClaimFacts> {
        self.claims
            .get(candidate.claim?)
            .filter(|claim| claim.object.object_id == candidate.row.object_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaimCandidateBounds {
    /// Live claim rows the projection may hold before the read is refused
    /// whole. A result bound: the rows are ordered before the limit applies.
    pub max_rows: NonZeroUsize,
    pub facts: ClaimFactBounds,
}

#[derive(Debug, thiserror::Error)]
pub enum ClaimCandidateError {
    #[error("the projection has no identity")]
    NoIdentity,
    #[error("the projection was built for kernel incarnation {kernel_incarnation_id}")]
    ForeignKernel { kernel_incarnation_id: String },
    #[error(transparent)]
    Projection(#[from] ProjectionError),
    #[error(transparent)]
    Facts(#[from] ClaimFactsError),
}

/// The classes the `id` family covers are exactly the claim classes.
fn claim_classes() -> &'static [OccurrenceClass] {
    match coverage(Family::Id) {
        Coverage::Extracted(classes) => classes,
        Coverage::Missing => &[],
    }
}

/// `?1` and `?2` are the two claim class codes, `?3` the row probe, `?4` the
/// association family keyword, `?5` the association namespace.
const LIVE_CLAIM_ROWS_SQL: &str =
    "SELECT o.occurrence_id,o.class,o.representation,a.target_id,a.extraction_version,o.revision,
            a.key,o.tuple,o.lineage_id,o.span_start,o.span_end,a.created_commit_seq,
            o.created_commit_seq
     FROM occurrences o
     LEFT JOIN occurrence_tombstones t ON t.occurrence_id=o.occurrence_id
     LEFT JOIN exact_associations a
       ON a.occurrence_id=o.occurrence_id AND a.family=?4 AND a.namespace=?5
     WHERE t.occurrence_id IS NULL AND o.class IN (?1,?2)
     ORDER BY o.class,o.occurrence_id
     LIMIT ?3";

struct LiveRow {
    occurrence_id: String,
    class: String,
    representation: String,
    target_id: Option<String>,
    version: Option<u32>,
    revision: i64,
    key: Option<Vec<u8>>,
    tuple: Vec<u8>,
    lineage_id: String,
    span_start: Option<i64>,
    span_end: Option<i64>,
    association_commit_seq: Option<i64>,
    created_commit_seq: i64,
}

/// Live claim rows in `(class, occurrence_id)` order. A set larger than `max`
/// is refused whole rather than truncated.
///
/// # Errors
///
/// `TooManyRecords` past `max`; `CorruptRow` for a stored class outside the
/// contract, a claim row with no `canonical_object` association, an
/// association written in another commit than its row or whose key, target,
/// and occurrence tuple do not name one object, or a row whose id, revision,
/// representation, span, or lineage disagree with its tuple;
/// `ExtractionVersionMismatch` for an association from another extractor.
pub fn live_claim_candidates(
    conn: &GuardedConn<'_>,
    max: NonZeroUsize,
) -> Result<Vec<ClaimCandidateRow>, ProjectionError> {
    let classes = claim_classes();
    // The statement binds exactly two class codes; a third claim class needs a
    // wider `IN` list, not a silent drop.
    assert_eq!(
        classes.len(),
        2,
        "LIVE_CLAIM_ROWS_SQL binds two claim class codes"
    );
    // One row past `max` proves the set is too large; the probe saturates rather than overflowing.
    let probe = i64::try_from(max.get().saturating_add(1)).unwrap_or(i64::MAX);
    let rows: Vec<LiveRow> = conn
        .prepare_cached(LIVE_CLAIM_ROWS_SQL)?
        .query_map(
            params![
                classes[0].code(),
                classes[1].code(),
                probe,
                Family::Id.keyword(),
                CANONICAL_OBJECT_NAMESPACE
            ],
            |row| {
                Ok(LiveRow {
                    occurrence_id: row.get(0)?,
                    class: row.get(1)?,
                    representation: row.get(2)?,
                    target_id: row.get(3)?,
                    version: row.get(4)?,
                    revision: row.get(5)?,
                    key: row.get(6)?,
                    tuple: row.get(7)?,
                    lineage_id: row.get(8)?,
                    span_start: row.get(9)?,
                    span_end: row.get(10)?,
                    association_commit_seq: row.get(11)?,
                    created_commit_seq: row.get(12)?,
                })
            },
        )?
        .collect::<rusqlite::Result<_>>()?;
    if rows.len() > max.get() {
        return Err(ProjectionError::TooManyRecords { count: rows.len() });
    }
    rows.into_iter().map(candidate_row).collect()
}

/// The same row checks `exact::lookup::AssociationRow::decode` makes: the
/// association was written in the row's commit and names the object the
/// tuple's identity names, and the id, revision, representation, span, and
/// lineage columns are the tuple's own.
fn candidate_row(row: LiveRow) -> Result<ClaimCandidateRow, ProjectionError> {
    let corrupt = || ProjectionError::CorruptRow;
    let class = OccurrenceClass::from_code(&row.class).ok_or_else(corrupt)?;
    let object_id = row.target_id.ok_or_else(corrupt)?;
    if row.association_commit_seq != Some(row.created_commit_seq) {
        return Err(corrupt());
    }
    // The `id` family derives the target from the key, and the extractor
    // takes both from the occurrence's identity field, so the three must name
    // one object.
    let [field] = class.identity_fields() else {
        return Err(corrupt());
    };
    if row.key.as_deref() != Some(object_id.as_bytes())
        || !occurrence_identity_matches(&row.tuple, class, &[(field, &object_id)])
    {
        return Err(corrupt());
    }
    let span = decode_span(row.span_start, row.span_end)?.map(|(start, end)| Span { start, end });
    if identity_digest(&row.tuple) != row.occurrence_id
        || derived_lineage_id(
            &row.tuple,
            class.code(),
            row.revision,
            &row.representation,
            span,
        )
        .as_deref()
            != Some(row.lineage_id.as_str())
    {
        return Err(corrupt());
    }
    match row.version {
        Some(stored) if stored != EXTRACTION_VERSION => {
            return Err(ProjectionError::ExtractionVersionMismatch {
                stored,
                expected: EXTRACTION_VERSION,
            });
        }
        Some(_) => {}
        None => return Err(corrupt()),
    }
    Ok(ClaimCandidateRow {
        occurrence_id: row.occurrence_id,
        class,
        representation: row.representation,
        object_id,
        revision: row.revision,
    })
}

/// The state of one row given the canonical facts of its object at the
/// snapshot; `None` when the object has no registry row by the snapshot.
pub fn classify(row: &ClaimCandidateRow, facts: Option<&ClaimFacts>) -> CandidateState {
    let Some(facts) = facts else {
        return CandidateState::Retracted;
    };
    if facts.object.superseded_by.is_some() {
        return CandidateState::Superseded;
    }
    let listed = facts.occurrences.iter().any(|occurrence| {
        occurrence.occurrence_id == row.occurrence_id
            && occurrence.class == row.class
            && occurrence.representation == row.representation
    });
    if facts.object.invalidated_commit_seq.is_some() || !listed {
        return CandidateState::Retracted;
    }
    // The serving view folds the lineage row into every surface, so its
    // disposition binds the row like the own row's does.
    let dispositions = [&facts.own_admission, &facts.lineage_admission]
        .map(|admission| admission.as_ref().map(|admission| admission.disposition));
    if dispositions.contains(&Some(Disposition::Superseded)) {
        return CandidateState::Superseded;
    }
    if dispositions.iter().flatten().any(|disposition| {
        matches!(
            disposition,
            Disposition::Rejected | Disposition::Contradicted | Disposition::Quarantined
        )
    }) {
        return CandidateState::Hidden;
    }
    match &facts.served {
        ServedStanding::Served(served) if served.explicit_search != SurfaceVisibility::Hidden => {}
        ServedStanding::Served(_)
        | ServedStanding::NotLiveAtSnapshot
        | ServedStanding::NeverAdmitted => return CandidateState::Hidden,
    }
    if dispositions.contains(&Some(Disposition::Stale))
        || row.revision != facts.object.source_revision
    {
        return CandidateState::Stale;
    }
    CandidateState::Current
}

/// Reads every live claim row, then the kernel's facts for the objects they
/// name at a kernel tip captured with its incarnation before the facts read,
/// and classifies each.
/// `kernel_incarnation_id` names the incarnation of `kernel`; a projection
/// built for another incarnation is refused before any row is read, since a
/// reused object id there would classify from an unrelated history.
///
/// # Errors
///
/// `NoIdentity` and `ForeignKernel` before any row is read; projection
/// refusals from [`live_claim_candidates`]; `TooManyClaims` before kernel
/// access when the rows name more distinct objects than
/// `bounds.facts.max_claims`; `Facts(IncarnationMismatch)` when the kernel was
/// restored between the tip capture and the facts read; other facts refusals
/// from `claim_facts_at`; kernel errors as `Facts(Kernel(_))`.
pub fn classify_live_claims(
    conn: &GuardedConn<'_>,
    kernel: &KernelStore,
    kernel_incarnation_id: &str,
    bounds: ClaimCandidateBounds,
) -> Result<ClaimCandidateBatch, ClaimCandidateError> {
    let identity = read_identity(conn)?.ok_or(ClaimCandidateError::NoIdentity)?;
    if identity.kernel_incarnation_id != kernel_incarnation_id {
        return Err(ClaimCandidateError::ForeignKernel {
            kernel_incarnation_id: identity.kernel_incarnation_id,
        });
    }
    let rows = live_claim_candidates(conn, bounds.max_rows)?;
    let mut seen = BTreeSet::new();
    let object_ids: Vec<String> = rows
        .iter()
        .map(|row| row.object_id.as_str())
        .filter(|id| seen.insert(*id))
        .map(str::to_string)
        .collect();
    if object_ids.len() > bounds.facts.max_claims.get() {
        return Err(ClaimFactsError::TooManyClaims.into());
    }
    // The target carries the tip and the incarnation it was read from; the
    // facts read refuses a store restored in between, so one history's tip
    // cannot be paired with another's rows.
    let target = kernel
        .capture_commit_read_target()
        .map_err(ClaimFactsError::from)?;
    let snapshot = kernel.claim_facts_at(&object_ids, target, bounds.facts)?;
    let known_as_of = target.through_commit;
    let index: HashMap<&str, usize> = snapshot
        .claims
        .iter()
        .enumerate()
        .map(|(position, claim)| (claim.object.object_id.as_str(), position))
        .collect();
    let candidates = rows
        .into_iter()
        .map(|row| {
            let claim = index.get(row.object_id.as_str()).copied();
            ClaimCandidate {
                state: classify(&row, claim.map(|position| &snapshot.claims[position])),
                claim,
                row,
            }
        })
        .collect();
    Ok(ClaimCandidateBatch {
        known_as_of,
        claims: snapshot.claims,
        candidates,
    })
}

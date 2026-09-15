//! Live claim occurrences in the projection, classified from canonical facts
//! read at one kernel snapshot.
//!
//! The projection stores candidates. It never stores a claim's state, so a
//! projection that lags the kernel cannot revive a retracted, superseded, or
//! hidden claim: the state is derived from `KernelStore::claim_facts_as_of`
//! every time it is asked for. `classify` takes no causal class, so `Unknown`
//! lineage cannot change a candidate's state, and no field here carries a
//! score, a boost, or a corroboration count.
//!
//! `validate_for_surface` is the final-use gate: it re-judges `Current`
//! candidates against the kernel's current policy for one surface at a fresh
//! snapshot, so a restriction that landed after classification denies the use
//! whatever the projection still holds.

use std::collections::{BTreeSet, HashMap};
use std::num::NonZeroUsize;

use kernel::applicability::EvalBudget;
use kernel::source_identity::{
    OccurrenceClass, Span, derived_lineage_id, identity_digest, occurrence_identity_matches,
};
use kernel::{
    ArtifactDestination, ClaimFactBounds, ClaimFacts, ClaimFactsError, CommitReadIncarnation,
    Disposition, EgressSnapshot, EligibilityCandidate, EligibilityVerdict, KernelError,
    KernelStore, MAX_ELIGIBILITY_CANDIDATES, MAX_ELIGIBILITY_OBJECT_ID_BYTES, ProjectScope,
    ServedStanding, Surface, SurfaceVisibility,
};
use rusqlite::params;
use storage::GuardedConn;

use crate::batch::read_checkpoint;
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
    /// The own or lineage admission disposition is `Stale`, the occurrence
    /// carries a revision other than the object's canonical one, or the kernel
    /// lists the row's occurrence with an artifact other than the row's. The
    /// registry never changes an object's revision or an occurrence's artifact,
    /// so the last two inputs can only come from a corrupt projection row and
    /// are kept as guards.
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
    /// The digest of the artifact the row's bytes were selected from; the
    /// final-use gate judges its egress facts with the object.
    pub artifact_digest: String,
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
    /// The kernel incarnation `known_as_of` belongs to; `validate_for_surface`
    /// refuses a kernel of another incarnation.
    pub incarnation: CommitReadIncarnation,
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
    #[error("the projection has no checkpoint")]
    NoCheckpoint,
    #[error("the kernel tip {tip} lies behind the projection checkpoint {checkpoint}")]
    KernelBehindProjection { tip: i64, checkpoint: i64 },
    #[error(transparent)]
    Projection(#[from] ProjectionError),
    #[error(transparent)]
    Facts(#[from] ClaimFactsError),
    /// A kernel failure while judging candidates for a surface.
    #[error("surface eligibility: {0}")]
    Eligibility(KernelError),
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
            o.created_commit_seq,o.source_artifact_digest
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
    source_artifact_digest: String,
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
                    source_artifact_digest: row.get(13)?,
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
        artifact_digest: row.source_artifact_digest,
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
    let listed = facts.occurrences.iter().find(|occurrence| {
        occurrence.occurrence_id == row.occurrence_id
            && occurrence.class == row.class
            && occurrence.representation == row.representation
    });
    let Some(listed) = listed.filter(|_| facts.object.invalidated_commit_seq.is_none()) else {
        return CandidateState::Retracted;
    };
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
        || row.artifact_digest != listed.artifact_digest
    {
        return CandidateState::Stale;
    }
    CandidateState::Current
}

/// Reads every live claim row, then the kernel's facts for the objects they
/// name at a kernel tip captured with its incarnation after the row read, and
/// classifies each. The projection identity is compared with the kernel's own
/// database identity, and the kernel incarnation is captured before and after
/// the row read, so a projection built for another kernel or a kernel restored
/// during the read is refused rather than classified from an unrelated
/// history. `budget` bounds every kernel read.
///
/// # Errors
///
/// `NoIdentity`, `ForeignKernel`, and `NoCheckpoint` before any row is read;
/// projection refusals from [`live_claim_candidates`]; `TooManyClaims` before
/// the facts read when the rows name more distinct objects than
/// `bounds.facts.max_claims`; `Facts(IncarnationMismatch)` when the kernel was
/// restored during the read; `KernelBehindProjection` when the kernel tip is
/// behind the projection checkpoint; other facts refusals from
/// `claim_facts_at`; kernel errors, including `Deadline` from `budget`, as
/// `Facts(Kernel(_))`.
pub fn classify_live_claims(
    conn: &GuardedConn<'_>,
    kernel: &KernelStore,
    budget: &EvalBudget,
    bounds: ClaimCandidateBounds,
) -> Result<ClaimCandidateBatch, ClaimCandidateError> {
    let identity = read_identity(conn)?.ok_or(ClaimCandidateError::NoIdentity)?;
    let entry = kernel
        .capture_commit_read_target_within_budget(budget)
        .map_err(ClaimFactsError::from)?;
    let database = kernel
        .database_incarnation_id_within_budget(budget)
        .map_err(ClaimFactsError::from)?;
    if identity.kernel_incarnation_id != database {
        return Err(ClaimCandidateError::ForeignKernel {
            kernel_incarnation_id: identity.kernel_incarnation_id,
        });
    }
    let checkpoint = read_checkpoint(conn, &database)?.ok_or(ClaimCandidateError::NoCheckpoint)?;
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
    // The target carries the tip and the incarnation it was read from. A
    // restore during the row read changes the incarnation since `entry`; one
    // between here and the facts read is refused by `claim_facts_at`.
    let target = kernel
        .capture_commit_read_target_within_budget(budget)
        .map_err(ClaimFactsError::from)?;
    if target.incarnation != entry.incarnation {
        return Err(ClaimFactsError::IncarnationMismatch.into());
    }
    // A kernel restored from an older backup of the same database keeps its
    // identity and generation but not the commits the projection applied;
    // rows past its tip would read as Retracted instead of as another history.
    if target.through_commit < checkpoint.checkpoint_commit_seq {
        return Err(ClaimCandidateError::KernelBehindProjection {
            tip: target.through_commit,
            checkpoint: checkpoint.checkpoint_commit_seq,
        });
    }
    let snapshot =
        kernel.claim_facts_at_within_budget(&object_ids, target, bounds.facts, budget)?;
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
        incarnation: target.incarnation,
        claims: snapshot.claims,
        candidates,
    })
}

/// Why a surface may not present a candidate at the validated snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseDenial {
    /// The candidate's state at the classification snapshot was not `Current`.
    State(CandidateState),
    /// The kernel's batch verdict at the validation snapshot.
    Verdict(EligibilityVerdict),
    /// The batch admitted the object but the serving view hides it on this surface.
    SurfaceHidden,
}

/// What one surface may do with one candidate at the validation snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseVerdict {
    /// The kernel's own visibility on the surface: `Labeled` means the
    /// presentation must carry the claim's label.
    Permitted(SurfaceVisibility),
    Denied(UseDenial),
}

/// `candidates[i]` of the validated slice paired with its verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCandidate {
    pub candidate: ClaimCandidate,
    pub verdict: UseVerdict,
}

/// `attempted_rows` counts rows; the object sets count distinct objects.
/// An object belongs to `permitted_objects` when any row is permitted.
/// An object belongs to `rejected_objects` when any row is denied.
/// An object belongs to both sets when its rows receive different verdicts.
/// `unknown_objects` overlaps either when the lineage is `Unknown`.
/// No set authorizes a presentation; only each row's `UseVerdict` does.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UseAccounting {
    pub attempted_rows: usize,
    pub permitted_objects: BTreeSet<String>,
    pub rejected_objects: BTreeSet<String>,
    pub unknown_objects: BTreeSet<String>,
}

/// The result of validating candidates for one surface at one fresh kernel
/// snapshot. A candidate denied here is denied for that snapshot; a change
/// after `snapshot.tip` is concurrent with the delivery, not a retroactive
/// cancellation of it.
#[derive(Debug)]
pub struct SurfaceValidation {
    /// A `None` classification generation is not a reusable grant and must
    /// not be cached.
    pub snapshot: EgressSnapshot,
    pub incarnation: CommitReadIncarnation,
    pub surface: Surface,
    /// Positionally aligned with the validated slice. Each `candidate` is the
    /// caller's input as given; its `claim` index still refers to the caller's
    /// classification batch, not to `claims` below.
    pub candidates: Vec<ValidatedCandidate>,
    /// The canonical facts of every object the candidates name, read with the
    /// verdicts, so a caller can report the causal class at this snapshot
    /// beside each verdict without a second read. An object with no registry
    /// row at the snapshot has no entry.
    pub claims: Vec<ClaimFacts>,
    pub accounting: UseAccounting,
}

impl SurfaceValidation {
    pub fn is_reusable(&self) -> bool {
        self.snapshot.classification_generation.is_some()
    }
}

/// Validates `candidates` for `surface` against the kernel's current canonical
/// policy in one fresh snapshot bounded by `budget`. Only `Current` candidates
/// are submitted to the kernel, judged by the decision object they name, its
/// revision, and the artifact behind the row; the rest are denied by their
/// state. The same snapshot supplies the canonical facts of every object the
/// candidates name, so a submitted row is reclassified there: a representation
/// retired, or a row whose artifact the kernel does not list for its
/// occurrence, is denied by that fresh state even though the kernel permitted
/// the object, and the accounting's lineage is the snapshot's. A batch `Ok`
/// permits the surface only when the serving view also shows the object there,
/// so an `AutoInject` presentation needs `Visible` on `AutoInject`, never batch
/// `Ok` alone. Call it before preselection admission and again on the exact
/// survivors immediately before handoff; the second call reads a newer snapshot
/// and denies anything restricted since the first. Cancellation is
/// cooperative: local loops poll between rows, but a single row copy and
/// cleanup are not interruptible. Callers remain responsible for bounding
/// total rows and their owned bytes.
///
/// # Errors
///
/// `Eligibility(InvalidInput)` when more than `MAX_ELIGIBILITY_CANDIDATES`
/// candidates are `Current` or a Current row's identity fields are invalid;
/// `Facts(TooManyClaims)` when the candidates name more distinct objects than
/// `bounds.max_claims`; `Facts(IncarnationMismatch)` when `kernel` is not the
/// incarnation `classified_in` (the batch's `incarnation`) names, so a kernel
/// restored since classification cannot judge displaced candidates;
/// `Eligibility(Deadline)` when the budget runs out; other kernel read errors
/// as `Eligibility(_)`.
#[allow(clippy::too_many_arguments)] // The gate's inputs are one judgement; a struct would be built once and read once.
pub fn validate_for_surface(
    kernel: &KernelStore,
    candidates: &[ClaimCandidate],
    project: &ProjectScope,
    destination: ArtifactDestination,
    surface: Surface,
    bounds: ClaimFactBounds,
    classified_in: CommitReadIncarnation,
    budget: &EvalBudget,
) -> Result<SurfaceValidation, ClaimCandidateError> {
    let check_budget = || {
        budget
            .check()
            .map_err(|_| ClaimCandidateError::Eligibility(KernelError::Deadline))
    };
    check_budget()?;
    let mut submitted = Vec::new();
    let mut seen = BTreeSet::new();
    let mut object_ids = Vec::new();
    for candidate in candidates {
        check_budget()?;
        if seen.insert(candidate.row.object_id.as_str()) {
            object_ids.push(candidate.row.object_id.clone());
        }
        if candidate.state != CandidateState::Current {
            continue;
        }
        if submitted.len() == MAX_ELIGIBILITY_CANDIDATES
            || candidate.row.object_id.len() > MAX_ELIGIBILITY_OBJECT_ID_BYTES
            || candidate.row.artifact_digest.len() != 64
        {
            return Err(ClaimCandidateError::Eligibility(KernelError::InvalidInput));
        }
        let candidate = EligibilityCandidate {
            object_id: candidate.row.object_id.clone(),
            source_revision: candidate.row.revision,
            artifact_digest: Some(candidate.row.artifact_digest.clone()),
        };
        candidate
            .validate()
            .map_err(ClaimCandidateError::Eligibility)?;
        submitted.push(candidate);
    }
    let judged = kernel
        .judge_surface_eligibility_with_claims(
            project,
            destination,
            surface,
            &submitted,
            &object_ids,
            bounds,
            classified_in,
            budget,
        )
        .map_err(|error| match error {
            ClaimFactsError::Kernel(error) => ClaimCandidateError::Eligibility(error),
            other => ClaimCandidateError::Facts(other),
        })?;
    #[cfg(test)]
    tests::CANCEL_AFTER_JUDGMENT.with(|slot| {
        if let Some(budget) = slot.take() {
            budget.cancel();
        }
    });
    check_budget()?;
    let mut facts: HashMap<&str, &ClaimFacts> = HashMap::new();
    for claim in &judged.claims {
        check_budget()?;
        facts.insert(claim.object.object_id.as_str(), claim);
    }
    let mut accounting = UseAccounting {
        attempted_rows: candidates.len(),
        ..UseAccounting::default()
    };
    let mut verdicts = judged.batch.verdicts.iter();
    let mut validated = Vec::new();
    for candidate in candidates {
        check_budget()?;
        let object_id = candidate.row.object_id.as_str();
        let fresh = facts.get(object_id).copied();
        let verdict = if candidate.state == CandidateState::Current {
            // `submitted` was filtered by the same predicate over the same
            // slice, so the kernel returned exactly one verdict per `Current`
            // candidate in order.
            match verdicts.next() {
                Some(judged) if judged.permits() => match classify(&candidate.row, fresh) {
                    CandidateState::Current => UseVerdict::Permitted(judged.visibility),
                    state => UseVerdict::Denied(UseDenial::State(state)),
                },
                Some(judged) if judged.verdict == EligibilityVerdict::Ok => {
                    UseVerdict::Denied(UseDenial::SurfaceHidden)
                }
                Some(judged) => UseVerdict::Denied(UseDenial::Verdict(judged.verdict)),
                None => return Err(ClaimCandidateError::Eligibility(KernelError::InvalidInput)),
            }
        } else {
            UseVerdict::Denied(UseDenial::State(candidate.state))
        };
        let objects = match verdict {
            UseVerdict::Permitted(_) => &mut accounting.permitted_objects,
            UseVerdict::Denied(_) => &mut accounting.rejected_objects,
        };
        if !objects.contains(object_id) {
            objects.insert(object_id.to_owned());
        }
        if fresh.is_none_or(|claim| claim.causality.is_unknown())
            && !accounting.unknown_objects.contains(object_id)
        {
            accounting.unknown_objects.insert(object_id.to_owned());
        }
        validated.push(ValidatedCandidate {
            candidate: candidate.clone(),
            verdict,
        });
    }
    check_budget()?;
    Ok(SurfaceValidation {
        snapshot: judged.batch.snapshot,
        incarnation: judged.batch.incarnation,
        surface: judged.batch.surface,
        candidates: validated,
        claims: judged.claims,
        accounting,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::{Arc, atomic::AtomicBool};
    use std::time::Instant;

    thread_local! {
        pub(super) static CANCEL_AFTER_JUDGMENT: Cell<Option<EvalBudget>> = const { Cell::new(None) };
    }

    fn bounds() -> ClaimFactBounds {
        ClaimFactBounds {
            max_claims: NonZeroUsize::new(8).unwrap(),
            max_causal_payload_bytes: std::num::NonZeroU64::new(1 << 16).unwrap(),
        }
    }

    fn fixture() -> (
        tempfile::TempDir,
        KernelStore,
        ClaimCandidateBatch,
        ProjectScope,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let kernel = KernelStore::open(dir.path()).unwrap();
        let batch = ClaimCandidateBatch {
            known_as_of: 0,
            incarnation: kernel.capture_commit_read_target().unwrap().incarnation,
            claims: Vec::new(),
            candidates: vec![ClaimCandidate {
                row: ClaimCandidateRow {
                    occurrence_id: "occ".into(),
                    class: OccurrenceClass::CanonicalClaims,
                    representation: "decision_summary".into(),
                    object_id: "decision-object-1".into(),
                    revision: 1,
                    artifact_digest: "0".repeat(64),
                },
                state: CandidateState::Hidden,
                claim: None,
            }],
        };
        (
            dir,
            kernel,
            batch,
            ProjectScope::new(&"0".repeat(64)).unwrap(),
        )
    }

    #[test]
    fn exhausted_validation_stops_before_candidate_preparation() {
        let (_dir, kernel, mut batch, project) = fixture();
        batch.candidates[0].state = CandidateState::Current;
        batch.candidates[0].row.object_id.clear();
        let cancelled = EvalBudget::unbounded();
        cancelled.cancel();
        let expired = EvalBudget::new(Some(Instant::now()), Arc::new(AtomicBool::new(false)));
        for budget in [cancelled, expired] {
            assert!(matches!(
                validate_for_surface(
                    &kernel,
                    &batch.candidates,
                    &project,
                    ArtifactDestination::Local,
                    Surface::AutoInject,
                    bounds(),
                    batch.incarnation,
                    &budget,
                ),
                Err(ClaimCandidateError::Eligibility(KernelError::Deadline))
            ));
        }
    }

    #[test]
    fn validation_bounds_only_current_submissions_and_preserves_denied_duplicates() {
        let (_dir, kernel, batch, project) = fixture();
        let validate = |candidates: &[ClaimCandidate]| {
            validate_for_surface(
                &kernel,
                candidates,
                &project,
                ArtifactDestination::Local,
                Surface::AutoInject,
                bounds(),
                batch.incarnation,
                &EvalBudget::unbounded(),
            )
        };
        let mut current = batch.candidates[0].clone();
        current.state = CandidateState::Current;
        let at_limit = vec![current.clone(); MAX_ELIGIBILITY_CANDIDATES];
        assert_eq!(
            validate(&at_limit).unwrap().candidates.len(),
            MAX_ELIGIBILITY_CANDIDATES
        );
        let mut over_limit = at_limit;
        over_limit.push(current.clone());
        assert!(matches!(
            validate(&over_limit),
            Err(ClaimCandidateError::Eligibility(KernelError::InvalidInput))
        ));
        for (object_id, digest) in [
            (String::new(), "0".repeat(64)),
            (
                "x".repeat(MAX_ELIGIBILITY_OBJECT_ID_BYTES + 1),
                "0".repeat(64),
            ),
            ("object".into(), "0".repeat(65)),
            ("object".into(), "g".repeat(64)),
        ] {
            current.row.object_id = object_id;
            current.row.artifact_digest = digest;
            assert!(matches!(
                validate(std::slice::from_ref(&current)),
                Err(ClaimCandidateError::Eligibility(KernelError::InvalidInput))
            ));
        }
        let denied = vec![batch.candidates[0].clone(); MAX_ELIGIBILITY_CANDIDATES + 1];
        let result = validate(&denied).unwrap();
        assert_eq!(result.accounting.attempted_rows, denied.len());
        assert_eq!(result.candidates.len(), denied.len());
        assert!(
            result
                .candidates
                .iter()
                .zip(&denied)
                .all(|(result, input)| result.candidate == *input
                    && result.verdict
                        == UseVerdict::Denied(UseDenial::State(CandidateState::Hidden)))
        );
        assert!(result.accounting.permitted_objects.is_empty());
        assert_eq!(
            result.accounting.rejected_objects,
            BTreeSet::from(["decision-object-1".into()])
        );
        assert_eq!(
            result.accounting.unknown_objects,
            result.accounting.rejected_objects
        );
    }

    #[test]
    fn cancellation_after_judgment_refuses_empty_and_non_current_results() {
        let (_dir, kernel, batch, project) = fixture();
        for candidates in [&batch.candidates[..0], &batch.candidates[..]] {
            let budget = EvalBudget::unbounded();
            let control = validate_for_surface(
                &kernel,
                candidates,
                &project,
                ArtifactDestination::Local,
                Surface::AutoInject,
                bounds(),
                batch.incarnation,
                &budget,
            )
            .unwrap();
            assert_eq!(control.accounting.attempted_rows, candidates.len());
            assert_eq!(control.candidates.len(), candidates.len());
            assert!(control.candidates.iter().all(|candidate| candidate.verdict
                == UseVerdict::Denied(UseDenial::State(CandidateState::Hidden))));
            assert!(!budget.is_exhausted());
            CANCEL_AFTER_JUDGMENT.with(|slot| slot.set(Some(budget.clone())));
            let result = validate_for_surface(
                &kernel,
                candidates,
                &project,
                ArtifactDestination::Local,
                Surface::AutoInject,
                bounds(),
                batch.incarnation,
                &budget,
            );
            let unconsumed = CANCEL_AFTER_JUDGMENT.with(Cell::take);
            assert!(
                unconsumed.is_none(),
                "successful kernel read must reach cancellation hook"
            );
            assert!(budget.is_exhausted());
            assert!(matches!(
                result,
                Err(ClaimCandidateError::Eligibility(KernelError::Deadline))
            ));
        }
    }
}

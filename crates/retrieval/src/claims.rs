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
use kernel::source_identity::OccurrenceClass;
use kernel::{
    ArtifactDestination, ClaimFactBounds, ClaimFacts, ClaimFactsError, CommitReadIncarnation,
    Disposition, EgressSnapshot, EligibilityCandidate, EligibilityVerdict, KernelError,
    KernelStore, MAX_ELIGIBILITY_CANDIDATES, MAX_ELIGIBILITY_OBJECT_ID_BYTES, ProjectScope,
    ServedStanding, Surface, SurfaceVisibility,
};
use rusqlite::params;
use storage::GuardedConn;

use crate::ProjectionError;
use crate::exact::selector::Family;
use crate::exact::{CANONICAL_OBJECT_NAMESPACE, Coverage, EXTRACTION_VERSION, coverage};

/// Precedence when more than one applies: `Retracted`, `Superseded`, `Stale`,
/// `Hidden`, then `Current`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CandidateState {
    /// The claim object has no registry row at the snapshot or was invalidated
    /// without a successor.
    Retracted,
    /// The claim object was replaced by a successor, or its own admission
    /// disposition is `Superseded`.
    Superseded,
    /// The own admission disposition is `Stale`, or the occurrence carries a
    /// revision other than the object's canonical one. The registry never
    /// changes an object's revision, so the second input can only come from a
    /// corrupt projection row and is kept as a guard.
    Stale,
    /// The own admission disposition is `Rejected`, `Contradicted`, or
    /// `Quarantined`, or the serving view lists no row for the object or lists
    /// it hidden on the widest surface.
    Hidden,
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
    /// The facts of every distinct claim object the rows name, in the order the
    /// objects were first seen.
    pub claims: Vec<ClaimFacts>,
    pub candidates: Vec<ClaimCandidate>,
}

impl ClaimCandidateBatch {
    pub fn claim(&self, candidate: &ClaimCandidate) -> Option<&ClaimFacts> {
        candidate.claim.map(|index| &self.claims[index])
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
            o.source_artifact_digest
     FROM occurrences o
     LEFT JOIN occurrence_tombstones t ON t.occurrence_id=o.occurrence_id
     LEFT JOIN exact_associations a
       ON a.occurrence_id=o.occurrence_id AND a.family=?4 AND a.namespace=?5
     WHERE t.occurrence_id IS NULL AND o.class IN (?1,?2)
     ORDER BY o.class,o.occurrence_id
     LIMIT ?3";

type LiveRow = (
    String,
    String,
    String,
    Option<String>,
    Option<u32>,
    i64,
    String,
);

/// Live claim rows in `(class, occurrence_id)` order. A set larger than `max`
/// is refused whole rather than truncated.
///
/// # Errors
///
/// `TooManyRecords` past `max`; `CorruptRow` for a stored class outside the
/// contract or a claim row with no `canonical_object` association;
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
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )?
        .collect::<rusqlite::Result<_>>()?;
    if rows.len() > max.get() {
        return Err(ProjectionError::TooManyRecords { count: rows.len() });
    }
    rows.into_iter()
        .map(
            |(occurrence_id, class, representation, object_id, version, revision, digest)| {
                let class =
                    OccurrenceClass::from_code(&class).ok_or(ProjectionError::CorruptRow)?;
                let object_id = object_id.ok_or(ProjectionError::CorruptRow)?;
                match version {
                    Some(stored) if stored != EXTRACTION_VERSION => {
                        return Err(ProjectionError::ExtractionVersionMismatch {
                            stored,
                            expected: EXTRACTION_VERSION,
                        });
                    }
                    Some(_) => {}
                    None => return Err(ProjectionError::CorruptRow),
                }
                Ok(ClaimCandidateRow {
                    occurrence_id,
                    class,
                    representation,
                    object_id,
                    revision,
                    artifact_digest: digest,
                })
            },
        )
        .collect()
}

/// The state of one row given the canonical facts of its object at the
/// snapshot; `None` when the object has no registry row by the snapshot.
pub fn classify(row: &ClaimCandidateRow, facts: Option<&ClaimFacts>) -> CandidateState {
    let Some(facts) = facts else {
        return CandidateState::Retracted;
    };
    if facts.object.invalidated_commit_seq.is_some() {
        return match facts.object.superseded_by {
            Some(_) => CandidateState::Superseded,
            None => CandidateState::Retracted,
        };
    }
    let disposition = facts
        .own_admission
        .as_ref()
        .map(|admission| admission.disposition);
    match disposition {
        Some(Disposition::Superseded) => return CandidateState::Superseded,
        Some(Disposition::Stale) => return CandidateState::Stale,
        Some(Disposition::Rejected | Disposition::Contradicted | Disposition::Quarantined) => {
            return CandidateState::Hidden;
        }
        Some(Disposition::Active | Disposition::Disputed) | None => {}
    }
    if row.revision != facts.object.source_revision {
        return CandidateState::Stale;
    }
    match &facts.served {
        ServedStanding::Served(served) if served.explicit_search != SurfaceVisibility::Hidden => {
            CandidateState::Current
        }
        ServedStanding::Served(_)
        | ServedStanding::NotLiveAtSnapshot
        | ServedStanding::NeverAdmitted => CandidateState::Hidden,
    }
}

/// Reads every live claim row, then the kernel's facts for the objects they
/// name at the kernel tip observed before the facts read, and classifies each.
///
/// # Errors
///
/// Projection refusals from [`live_claim_candidates`]; facts refusals from
/// `claim_facts_as_of`, including `TooManyClaims` when the rows name more
/// distinct objects than `bounds.facts.max_claims`; kernel errors as
/// `Facts(Kernel(_))`.
pub fn classify_live_claims(
    conn: &GuardedConn<'_>,
    kernel: &KernelStore,
    bounds: ClaimCandidateBounds,
) -> Result<ClaimCandidateBatch, ClaimCandidateError> {
    let rows = live_claim_candidates(conn, bounds.max_rows)?;
    let object_ids: Vec<String> = rows
        .iter()
        .map(|row| row.object_id.as_str())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(str::to_string)
        .collect();
    let known_as_of = kernel.tip().map_err(ClaimFactsError::from)?;
    let snapshot = kernel.claim_facts_as_of(&object_ids, known_as_of, bounds.facts)?;
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
    /// Positionally aligned with the validated slice.
    pub candidates: Vec<ValidatedCandidate>,
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
/// state. A batch `Ok` permits the surface only when the serving view also
/// shows the object there, so an `AutoInject` presentation needs `Visible` on
/// `AutoInject`, never batch `Ok` alone. Call it before preselection admission
/// and again on the exact survivors immediately before handoff; the second
/// call reads a newer snapshot and denies anything restricted since the first.
/// `batch` supplies the lineage facts for the accounting; candidates are
/// matched to it by object id. Cancellation is cooperative: local loops poll
/// between rows, but a single row copy and cleanup are not interruptible.
/// Callers remain responsible for bounding total rows and their owned bytes.
///
/// # Errors
///
/// `Eligibility(InvalidInput)` when more than `MAX_ELIGIBILITY_CANDIDATES`
/// candidates are `Current` or a Current row's identity fields are invalid;
/// `Eligibility(Deadline)` when the budget runs out; other kernel read errors
/// as `Eligibility(_)`.
pub fn validate_for_surface(
    kernel: &KernelStore,
    batch: &ClaimCandidateBatch,
    candidates: &[ClaimCandidate],
    project: &ProjectScope,
    destination: ArtifactDestination,
    surface: Surface,
    budget: &EvalBudget,
) -> Result<SurfaceValidation, ClaimCandidateError> {
    let check_budget = || {
        budget
            .check()
            .map_err(|_| ClaimCandidateError::Eligibility(KernelError::Deadline))
    };
    check_budget()?;
    let mut submitted = Vec::new();
    for candidate in candidates {
        check_budget()?;
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
        .judge_surface_eligibility_within_budget(project, destination, surface, &submitted, budget)
        .map_err(ClaimCandidateError::Eligibility)?;
    #[cfg(test)]
    tests::CANCEL_AFTER_JUDGMENT.with(|slot| {
        if let Some(budget) = slot.take() {
            budget.cancel();
        }
    });
    check_budget()?;
    let mut lineage: HashMap<&str, &ClaimFacts> = HashMap::new();
    for claim in &batch.claims {
        check_budget()?;
        lineage.insert(claim.object.object_id.as_str(), claim);
    }
    let mut accounting = UseAccounting {
        attempted_rows: candidates.len(),
        ..UseAccounting::default()
    };
    let mut verdicts = judged.verdicts.iter();
    let mut validated = Vec::new();
    for candidate in candidates {
        check_budget()?;
        let verdict = if candidate.state == CandidateState::Current {
            // `submitted` was filtered by the same predicate over the same
            // slice, so the kernel returned exactly one verdict per `Current`
            // candidate in order.
            match verdicts.next() {
                Some(judged) if judged.permits() => UseVerdict::Permitted(judged.visibility),
                Some(judged) if judged.verdict == EligibilityVerdict::Ok => {
                    UseVerdict::Denied(UseDenial::SurfaceHidden)
                }
                Some(judged) => UseVerdict::Denied(UseDenial::Verdict(judged.verdict)),
                None => return Err(ClaimCandidateError::Eligibility(KernelError::InvalidInput)),
            }
        } else {
            UseVerdict::Denied(UseDenial::State(candidate.state))
        };
        let object_id = candidate.row.object_id.clone();
        match verdict {
            UseVerdict::Permitted(_) => accounting.permitted_objects.insert(object_id.clone()),
            UseVerdict::Denied(_) => accounting.rejected_objects.insert(object_id.clone()),
        };
        if lineage
            .get(object_id.as_str())
            .is_none_or(|claim| claim.causality.is_unknown())
        {
            accounting.unknown_objects.insert(object_id);
        }
        validated.push(ValidatedCandidate {
            candidate: candidate.clone(),
            verdict,
        });
    }
    check_budget()?;
    Ok(SurfaceValidation {
        snapshot: judged.snapshot,
        incarnation: judged.incarnation,
        surface: judged.surface,
        candidates: validated,
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
                    &batch,
                    &batch.candidates,
                    &project,
                    ArtifactDestination::Local,
                    Surface::AutoInject,
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
                &batch,
                candidates,
                &project,
                ArtifactDestination::Local,
                Surface::AutoInject,
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
                &batch,
                candidates,
                &project,
                ArtifactDestination::Local,
                Surface::AutoInject,
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
                &batch,
                candidates,
                &project,
                ArtifactDestination::Local,
                Surface::AutoInject,
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

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

use kernel::source_identity::OccurrenceClass;
use kernel::{
    ClaimFactBounds, ClaimFacts, ClaimFactsError, Disposition, KernelStore, ServedStanding,
    SurfaceVisibility,
};
use rusqlite::params;
use storage::GuardedConn;

use crate::ProjectionError;
use crate::exact::selector::Family;
use crate::exact::{CANONICAL_OBJECT_NAMESPACE, Coverage, EXTRACTION_VERSION, coverage};

/// Variant order is precedence order: restrictive states sort first, so a
/// classified claim is never more visible than the kernel's serving view of the
/// claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CandidateState {
    /// The claim object has no registry row at the snapshot or was invalidated
    /// without a successor.
    Retracted,
    /// The claim object was replaced by a successor, or its own admission
    /// disposition is `Superseded`.
    Superseded,
    /// The own admission disposition is `Rejected`, `Contradicted`, or
    /// `Quarantined`, or the serving view lists no row for the object or lists
    /// it hidden on the widest surface.
    Hidden,
    /// The own admission disposition is `Stale`, or the occurrence carries a
    /// revision other than the object's canonical one. The registry never
    /// changes an object's revision, so the second input can only come from a
    /// corrupt projection row and is kept as a guard.
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
    "SELECT o.occurrence_id,o.class,o.representation,a.target_id,a.extraction_version,o.revision
     FROM occurrences o
     LEFT JOIN occurrence_tombstones t ON t.occurrence_id=o.occurrence_id
     LEFT JOIN exact_associations a
       ON a.occurrence_id=o.occurrence_id AND a.family=?4 AND a.namespace=?5
     WHERE t.occurrence_id IS NULL AND o.class IN (?1,?2)
     ORDER BY o.class,o.occurrence_id
     LIMIT ?3";

type LiveRow = (String, String, String, Option<String>, Option<u32>, i64);

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
                ))
            },
        )?
        .collect::<rusqlite::Result<_>>()?;
    if rows.len() > max.get() {
        return Err(ProjectionError::TooManyRecords { count: rows.len() });
    }
    rows.into_iter()
        .map(
            |(occurrence_id, class, representation, object_id, version, revision)| {
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
    if facts.object.superseded_by.is_some() {
        return CandidateState::Superseded;
    }
    if facts.object.invalidated_commit_seq.is_some() {
        return CandidateState::Retracted;
    }
    let disposition = facts
        .own_admission
        .as_ref()
        .map(|admission| admission.disposition);
    match disposition {
        Some(Disposition::Superseded) => return CandidateState::Superseded,
        Some(Disposition::Rejected | Disposition::Contradicted | Disposition::Quarantined) => {
            return CandidateState::Hidden;
        }
        Some(Disposition::Stale | Disposition::Active | Disposition::Disputed) | None => {}
    }
    match &facts.served {
        ServedStanding::Served(served) if served.explicit_search != SurfaceVisibility::Hidden => {}
        ServedStanding::Served(_)
        | ServedStanding::NotLiveAtSnapshot
        | ServedStanding::NeverAdmitted => return CandidateState::Hidden,
    }
    if disposition == Some(Disposition::Stale) || row.revision != facts.object.source_revision {
        return CandidateState::Stale;
    }
    CandidateState::Current
}

/// Reads every live claim row, then the kernel's facts for the objects they
/// name at the kernel tip observed before the facts read, and classifies each.
///
/// # Errors
///
/// Projection refusals from [`live_claim_candidates`]; `TooManyClaims` before
/// kernel access when the rows name more distinct objects than
/// `bounds.facts.max_claims`; other facts refusals from `claim_facts_as_of`;
/// kernel errors as `Facts(Kernel(_))`.
pub fn classify_live_claims(
    conn: &GuardedConn<'_>,
    kernel: &KernelStore,
    bounds: ClaimCandidateBounds,
) -> Result<ClaimCandidateBatch, ClaimCandidateError> {
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

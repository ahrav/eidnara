//! One judgement for every consumer of `kernel.eligibility.batch` verdicts: the precedence and the scope decision live here so the daemon route and later adapters cannot drift from each other, while consumers keep decoding, authorization, wire literals, and caching. The artifact egress route keeps its own refusal vocabulary and is not routed through this module.

use std::collections::HashMap;

use rusqlite::Transaction;

use crate::admission::{EgressCandidate, EgressSnapshot, Surface, egress_candidates_tx};
use crate::cas::{ArtifactDestination, ArtifactEligibility, is_artifact_digest};
use crate::claim_facts::{
    ClaimFactBounds, ClaimFacts, ClaimFactsError, check_claim_bounds, load_claims_in_tx,
};
use crate::commit_read::CommitReadIncarnation;
use crate::envelope::Sensitivity;
use crate::scope::{
    CanonicalScope, Dimension, MatchOutcome, ScopeMatchContext, ScopeTermSpec, UnknownGraph,
    load_scope_terms, scope_matches,
};
use crate::{KernelError, KernelStore, SurfaceVisibility};

/// One batch holds a reader for one registry read plus one verdict per candidate, so the count bounds how long a single call occupies the pool.
pub const MAX_ELIGIBILITY_CANDIDATES: usize = 1024;

/// Bounds the identity a consumer keys a cache entry by, whether or not the object exists; `applicability::MAX_OBJECT_ID_BYTES` bounds a different surface.
pub const MAX_ELIGIBILITY_OBJECT_ID_BYTES: usize = 512;

/// Verdict order is fixed: an object that is gone or replaced is reported as such before its revision, scope, or sensitivity is considered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EligibilityVerdict {
    Ok,
    Retracted,
    Superseded,
    Stale,
    WrongScope,
    /// Live, but no read surface serves it.
    Hidden,
    ProviderSensitive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EligibilityCandidate {
    pub object_id: String,
    pub source_revision: i64,
    pub artifact_digest: Option<String>,
}

impl EligibilityCandidate {
    /// Validates identity fields without acquiring a kernel reader.
    pub fn validate(&self) -> Result<(), KernelError> {
        if self.object_id.is_empty() || self.object_id.len() > MAX_ELIGIBILITY_OBJECT_ID_BYTES {
            return Err(KernelError::InvalidInput);
        }
        if self
            .artifact_digest
            .as_deref()
            .is_some_and(|digest| !is_artifact_digest(digest))
        {
            return Err(KernelError::InvalidInput);
        }
        Ok(())
    }
}

/// The exact `project` term value a stored scope must carry for its rows to serve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectScope {
    context: ScopeMatchContext,
}

impl ProjectScope {
    /// `project_digest` is the lowercase SHA-256 hex a route stamps as the project term; anything else is refused so a malformed binding cannot silently judge every row `WrongScope`.
    pub fn new(project_digest: &str) -> Result<Self, KernelError> {
        if !is_artifact_digest(project_digest) {
            return Err(KernelError::InvalidInput);
        }
        Ok(Self {
            context: ScopeMatchContext::new().with_value(Dimension::Project, project_digest),
        })
    }

    /// A row with no scope has no project and never serves, and neither does a scope with no `project` term: it constrains nothing, so it would match every project. `Uncertain` (a redacted term, a dimension with no value, a malformed scope) and a scope with no stored row both fail to match.
    pub fn names_project(&self, terms: Option<&[ScopeTermSpec]>) -> bool {
        terms.is_some_and(|terms| {
            CanonicalScope::from_term_specs(terms).is_ok_and(|scope| {
                scope.term(Dimension::Project).is_some()
                    && scope_matches(&scope, &self.context, &UnknownGraph) == MatchOutcome::Matches
            })
        })
    }
}

#[derive(Debug)]
pub struct EligibilityBatch {
    /// The snapshot every verdict was judged at. A `None` classification generation means a classification merge overlapped the read; such a batch is not a reusable grant and must not be cached.
    pub snapshot: EgressSnapshot,
    /// The database incarnation under which `verdicts` were judged.
    pub incarnation: CommitReadIncarnation,
    /// `verdicts[i]` judges `candidates[i]` of the call that produced the batch; the two are positionally aligned and equal in length.
    pub verdicts: Vec<EligibilityVerdict>,
}

/// One candidate's standing on one surface: the batch verdict plus the
/// visibility the serving view gives the object on that surface. A batch `Ok`
/// judged at the widest surface never permits a narrower surface by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceVerdict {
    pub verdict: EligibilityVerdict,
    pub visibility: SurfaceVisibility,
}

impl SurfaceVerdict {
    /// Whether the surface may present the candidate at all.
    pub fn permits(self) -> bool {
        self.verdict == EligibilityVerdict::Ok && self.visibility != SurfaceVisibility::Hidden
    }

    /// Whether a permitted presentation must carry its label.
    pub fn labeled(self) -> bool {
        self.permits() && self.visibility == SurfaceVisibility::Labeled
    }
}

/// [`EligibilityBatch`] for one surface: the same batch verdicts, each paired
/// with the visibility that surface gives the object in the same snapshot.
#[derive(Debug)]
pub struct SurfaceEligibilityBatch {
    /// As for [`EligibilityBatch::snapshot`]: a `None` classification
    /// generation is not a reusable grant and must not be cached.
    pub snapshot: EgressSnapshot,
    pub incarnation: CommitReadIncarnation,
    /// The surface the visibilities were judged for, echoed so a consumer
    /// cannot apply an `ExplicitSearch` batch to `AutoInject`.
    pub surface: Surface,
    /// `verdicts[i]` judges `candidates[i]`; duplicates and order are preserved.
    pub verdicts: Vec<SurfaceVerdict>,
}

/// [`SurfaceEligibilityBatch`] read together with the canonical claim facts
/// of `object_ids`, so a caller can reclassify a projection row against the
/// occurrence inventory and causality the same snapshot holds.
#[derive(Debug)]
pub struct SurfaceEligibilityWithClaims {
    pub batch: SurfaceEligibilityBatch,
    /// One entry per requested object id with a registry row at the snapshot,
    /// in request order; the rest are `missing`.
    pub claims: Vec<ClaimFacts>,
    pub missing: Vec<String>,
}

impl SurfaceEligibilityBatch {
    pub fn is_reusable(&self) -> bool {
        self.snapshot.classification_generation.is_some()
    }
}

struct ScopeVerdicts<'a> {
    project: &'a ProjectScope,
    verdicts: HashMap<String, bool>,
}

impl ScopeVerdicts<'_> {
    fn matches(
        &mut self,
        tx: &Transaction<'_>,
        scope_id: Option<&str>,
    ) -> Result<bool, KernelError> {
        let Some(scope_id) = scope_id else {
            return Ok(false);
        };
        if let Some(verdict) = self.verdicts.get(scope_id) {
            return Ok(*verdict);
        }
        let terms = load_scope_terms(tx, scope_id)?;
        let verdict = self.project.names_project(terms.as_deref());
        self.verdicts.insert(scope_id.to_owned(), verdict);
        Ok(verdict)
    }
}

/// The sensitivity judged is the one the serving view folds onto the object from its admission history, since that is the class a read handed the caller; the registry class stands in when no admission decision serves the object. A secret object is refused for every destination and a non-normal object for a remote one, whether or not the candidate cites an artifact. An object no read serves, hidden by admission or never admitted, is refused as `Hidden`.
fn judge(
    candidate: &EligibilityCandidate,
    facts: &EgressCandidate,
    destination: ArtifactDestination,
    in_scope: bool,
) -> EligibilityVerdict {
    let Some(state) = &facts.state else {
        return EligibilityVerdict::Retracted;
    };
    if state.object.superseded_by.is_some() {
        return EligibilityVerdict::Superseded;
    }
    if state.object.invalidated_commit_seq.is_some() {
        return EligibilityVerdict::Retracted;
    }
    if state.object.source_revision != candidate.source_revision {
        return EligibilityVerdict::Stale;
    }
    if !in_scope {
        return EligibilityVerdict::WrongScope;
    }
    let sensitivity = facts
        .served
        .map_or(state.object.sensitivity, |served| served.sensitivity);
    if sensitivity == Sensitivity::Secret
        || (destination == ArtifactDestination::Remote && sensitivity != Sensitivity::Normal)
    {
        return EligibilityVerdict::ProviderSensitive;
    }
    if facts
        .served
        .is_none_or(|served| served.visibility == SurfaceVisibility::Hidden)
    {
        return EligibilityVerdict::Hidden;
    }
    if let Some(artifact) = &facts.artifact
        && artifact.eligibility != ArtifactEligibility::Allowed
    {
        return EligibilityVerdict::ProviderSensitive;
    }
    EligibilityVerdict::Ok
}

/// Judges every candidate against the registry state, served class, artifact facts, and scope terms visible in `tx` at `tip`.
/// The caller has checked the batch bounds and owns the transaction, so the same verdicts serve a reader snapshot and a writer holder alike.
pub(crate) fn judge_in_tx(
    tx: &Transaction<'_>,
    tip: i64,
    project: &ProjectScope,
    destination: ArtifactDestination,
    candidates: &[EligibilityCandidate],
) -> Result<Vec<EligibilityVerdict>, KernelError> {
    Ok(
        judge_facts_in_tx(tx, tip, project, destination, candidates)?
            .into_iter()
            .map(|(verdict, _)| verdict)
            .collect(),
    )
}

/// [`judge_in_tx`] keeping each candidate's egress facts beside its verdict.
fn judge_facts_in_tx(
    tx: &Transaction<'_>,
    tip: i64,
    project: &ProjectScope,
    destination: ArtifactDestination,
    candidates: &[EligibilityCandidate],
) -> Result<Vec<(EligibilityVerdict, EgressCandidate)>, KernelError> {
    let named: Vec<(&str, Option<&str>)> = candidates
        .iter()
        .map(|candidate| {
            (
                candidate.object_id.as_str(),
                candidate.artifact_digest.as_deref(),
            )
        })
        .collect();
    let facts = egress_candidates_tx(tx, tip, &named, destination)?;
    let mut scopes = ScopeVerdicts {
        project,
        verdicts: HashMap::new(),
    };
    candidates
        .iter()
        .zip(facts)
        .map(|(candidate, facts)| {
            let scope_id = facts
                .state
                .as_ref()
                .and_then(|state| state.scope_id.as_deref());
            let in_scope = scopes.matches(tx, scope_id)?;
            Ok((judge(candidate, &facts, destination, in_scope), facts))
        })
        .collect()
}

/// The batch verdicts at the widest surface, each paired with the visibility
/// `surface` gives the object from the same serving read. An object no row
/// serves is `Hidden` on every surface whatever its batch verdict says. On
/// `ExplicitSearch` the visibility adds only the `Labeled` distinction, since
/// the batch verdict already reports `Hidden` there.
pub(crate) fn judge_surface_in_tx(
    tx: &Transaction<'_>,
    tip: i64,
    project: &ProjectScope,
    destination: ArtifactDestination,
    surface: Surface,
    candidates: &[EligibilityCandidate],
) -> Result<Vec<SurfaceVerdict>, KernelError> {
    Ok(
        judge_facts_in_tx(tx, tip, project, destination, candidates)?
            .into_iter()
            .map(|(verdict, facts)| SurfaceVerdict {
                verdict,
                visibility: facts.served.map_or(SurfaceVisibility::Hidden, |served| {
                    served.visibility_on(surface)
                }),
            })
            .collect(),
    )
}

/// Runs before any reader is acquired; `egress_candidates_tx` repeats the digest check because it also serves callers that skip this gate.
pub(crate) fn check_bounds(candidates: &[EligibilityCandidate]) -> Result<(), KernelError> {
    if candidates.len() > MAX_ELIGIBILITY_CANDIDATES {
        return Err(KernelError::InvalidInput);
    }
    for candidate in candidates {
        candidate.validate()?;
    }
    Ok(())
}

impl KernelStore {
    /// Registry state, served class, artifact facts, and the scope terms each verdict depends on all come from one read snapshot at one tip; bounds are checked before any reader is taken, so an over-bound batch costs no query.
    pub fn judge_eligibility(
        &self,
        project: &ProjectScope,
        destination: ArtifactDestination,
        candidates: &[EligibilityCandidate],
    ) -> Result<EligibilityBatch, KernelError> {
        self.judge_eligibility_with(None, project, destination, candidates)
    }

    /// The wait for a pooled reader and the read itself stop at the budget's deadline or interrupt with `KernelError::Deadline`.
    pub fn judge_eligibility_within_budget(
        &self,
        project: &ProjectScope,
        destination: ArtifactDestination,
        candidates: &[EligibilityCandidate],
        budget: &crate::applicability::EvalBudget,
    ) -> Result<EligibilityBatch, KernelError> {
        self.judge_eligibility_with(
            Some(&budget.acquire_limit()),
            project,
            destination,
            candidates,
        )
    }

    /// [`Self::judge_eligibility`] plus the visibility `surface` gives each
    /// candidate, from the same snapshot, so a caller serving one surface can
    /// tell a candidate it may present from one the batch merely admits.
    pub fn judge_surface_eligibility(
        &self,
        project: &ProjectScope,
        destination: ArtifactDestination,
        surface: Surface,
        candidates: &[EligibilityCandidate],
    ) -> Result<SurfaceEligibilityBatch, KernelError> {
        self.judge_surface_eligibility_with(None, project, destination, surface, candidates)
    }

    /// [`Self::judge_surface_eligibility`] whose reader wait and read stop at
    /// the budget's deadline or interrupt with `KernelError::Deadline`.
    pub fn judge_surface_eligibility_within_budget(
        &self,
        project: &ProjectScope,
        destination: ArtifactDestination,
        surface: Surface,
        candidates: &[EligibilityCandidate],
        budget: &crate::applicability::EvalBudget,
    ) -> Result<SurfaceEligibilityBatch, KernelError> {
        self.judge_surface_eligibility_with(
            Some(&budget.acquire_limit()),
            project,
            destination,
            surface,
            candidates,
        )
    }

    fn judge_surface_eligibility_with(
        &self,
        limit: Option<&crate::open::AcquireLimit>,
        project: &ProjectScope,
        destination: ArtifactDestination,
        surface: Surface,
        candidates: &[EligibilityCandidate],
    ) -> Result<SurfaceEligibilityBatch, KernelError> {
        check_bounds(candidates)?;
        let read = |tx: &Transaction<'_>, tip: i64| {
            Ok((
                self.incarnation(),
                judge_surface_in_tx(tx, tip, project, destination, surface, candidates)?,
            ))
        };
        let (snapshot, (incarnation, verdicts)) = match limit {
            Some(limit) => self.egress_read_within(limit, read)?,
            None => self.egress_read(read)?,
        };
        Ok(SurfaceEligibilityBatch {
            snapshot,
            incarnation,
            surface,
            verdicts,
        })
    }

    /// [`Self::judge_surface_eligibility_within_budget`] plus
    /// [`Self::claim_facts_as_of`] for `object_ids` from the one snapshot the
    /// verdicts come from, so a final-use gate can deny a row whose occurrence
    /// the kernel no longer lists at the tip it judged. `classified_in` is the
    /// incarnation the candidates were classified against; a store of another
    /// incarnation refuses under the reader guard, so a restore between
    /// classification and this judgement cannot match displaced candidates to
    /// reused identifiers.
    ///
    /// # Errors
    ///
    /// The facts request errors before any read; `IncarnationMismatch` before
    /// any row is read; kernel errors as `ClaimFactsError::Kernel`.
    #[allow(clippy::too_many_arguments)] // Splitting the call would split the snapshot.
    pub fn judge_surface_eligibility_with_claims(
        &self,
        project: &ProjectScope,
        destination: ArtifactDestination,
        surface: Surface,
        candidates: &[EligibilityCandidate],
        object_ids: &[String],
        bounds: ClaimFactBounds,
        classified_in: CommitReadIncarnation,
        budget: &crate::applicability::EvalBudget,
    ) -> Result<SurfaceEligibilityWithClaims, ClaimFactsError> {
        check_bounds(candidates)?;
        check_claim_bounds(object_ids, bounds)?;
        let limit = budget.acquire_limit();
        let read = |tx: &Transaction<'_>, tip: i64| {
            if classified_in != self.incarnation() {
                return Ok((
                    self.incarnation(),
                    Vec::new(),
                    Err(ClaimFactsError::IncarnationMismatch),
                ));
            }
            let verdicts = judge_surface_in_tx(tx, tip, project, destination, surface, candidates)?;
            let claims = load_claims_in_tx(tx, tip, object_ids, bounds, &limit);
            Ok((self.incarnation(), verdicts, claims))
        };
        let (snapshot, (incarnation, verdicts, claims)) = self.egress_read_within(&limit, read)?;
        let (claims, missing) = claims?;
        Ok(SurfaceEligibilityWithClaims {
            batch: SurfaceEligibilityBatch {
                snapshot,
                incarnation,
                surface,
                verdicts,
            },
            claims,
            missing,
        })
    }

    fn judge_eligibility_with(
        &self,
        limit: Option<&crate::open::AcquireLimit>,
        project: &ProjectScope,
        destination: ArtifactDestination,
        candidates: &[EligibilityCandidate],
    ) -> Result<EligibilityBatch, KernelError> {
        check_bounds(candidates)?;
        let read = |tx: &Transaction<'_>, tip: i64| {
            let incarnation = self.incarnation();
            Ok((
                incarnation,
                judge_in_tx(tx, tip, project, destination, candidates)?,
            ))
        };
        let (snapshot, (incarnation, verdicts)) = match limit {
            Some(limit) => self.egress_read_within(limit, read)?,
            None => self.egress_read(read)?,
        };
        Ok(EligibilityBatch {
            snapshot,
            incarnation,
            verdicts,
        })
    }
}

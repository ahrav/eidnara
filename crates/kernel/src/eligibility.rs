//! One judgement for every consumer of `kernel.eligibility.batch` verdicts: the precedence and the scope decision live here so the daemon route and later adapters cannot drift from each other, while consumers keep decoding, authorization, wire literals, and caching. The artifact egress route keeps its own refusal vocabulary and is not routed through this module.

use std::collections::HashMap;

use rusqlite::Transaction;

use crate::admission::{EgressCandidate, EgressSnapshot, egress_candidates_tx};
use crate::cas::{ArtifactDestination, ArtifactEligibility, is_artifact_digest};
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
    /// `verdicts[i]` judges `candidates[i]` of the call that produced the batch; the two are positionally aligned and equal in length.
    pub verdicts: Vec<EligibilityVerdict>,
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

/// Runs before any reader is acquired; `egress_candidates_tx` repeats the digest check because it also serves callers that skip this gate.
fn check_bounds(candidates: &[EligibilityCandidate]) -> Result<(), KernelError> {
    if candidates.len() > MAX_ELIGIBILITY_CANDIDATES {
        return Err(KernelError::InvalidInput);
    }
    for candidate in candidates {
        if candidate.object_id.is_empty()
            || candidate.object_id.len() > MAX_ELIGIBILITY_OBJECT_ID_BYTES
        {
            return Err(KernelError::InvalidInput);
        }
        if candidate
            .artifact_digest
            .as_deref()
            .is_some_and(|digest| !is_artifact_digest(digest))
        {
            return Err(KernelError::InvalidInput);
        }
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
        check_bounds(candidates)?;
        let named: Vec<(&str, Option<&str>)> = candidates
            .iter()
            .map(|candidate| {
                (
                    candidate.object_id.as_str(),
                    candidate.artifact_digest.as_deref(),
                )
            })
            .collect();
        let (snapshot, verdicts) = self.egress_read(|tx, tip| {
            let facts = egress_candidates_tx(tx, tip, &named, destination)?;
            let mut scopes = ScopeVerdicts {
                project,
                verdicts: HashMap::new(),
            };
            candidates
                .iter()
                .zip(&facts)
                .map(|(candidate, facts)| {
                    let scope_id = facts
                        .state
                        .as_ref()
                        .and_then(|state| state.scope_id.as_deref());
                    let in_scope = scopes.matches(tx, scope_id)?;
                    Ok(judge(candidate, facts, destination, in_scope))
                })
                .collect::<Result<Vec<_>, KernelError>>()
        })?;
        Ok(EligibilityBatch { snapshot, verdicts })
    }
}

use std::num::{NonZeroU64, NonZeroUsize};
use std::sync::Arc;
use std::time::{Duration, Instant};

use host_runtime::RouteHandle;
use kernel::applicability::EvalBudget;
use kernel::{ClaimFactBounds, ClaimFactsError, EvidenceLiveness, KernelError};
use memory_store::MemoryStoreError;
use memory_store::contradiction_candidates::{MAX_CANDIDATE_PAGE, StoredCandidate};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Count;
use super::eligibility::eligible;
use super::scorer::matcher_digest;
use crate::dispatch::{PreparedOutcome, PreparedOutput};
use crate::kernel_routes::eligibility::MAX_OBJECT_ID_BYTES;
use crate::kernel_routes::project::{ScopeFilter, stored_terms};
use crate::kernel_routes::{blocking, state_only};
use crate::{HandlerCore, MemoriesAuthority, memories_authority_for_route};

pub(crate) const CANDIDATES: &str = "review.candidates";
const READ_BUDGET: Duration = Duration::from_secs(5);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CandidatesRequest {
    decision_object_id: String,
    #[serde(default)]
    after: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

fn response(body: Value) -> PreparedOutcome {
    PreparedOutcome::Response(PreparedOutput::json(body))
}

fn terminal(code: &str) -> PreparedOutcome {
    response(json!({ "kind": "terminal", "terminal": code }))
}

fn item(row: &StoredCandidate) -> Value {
    json!({
        "evidence_id": row.evidence_id,
        "decision_object_id": row.decision_object_id,
        "decision_id": row.decision_id,
        "matched_terms": row.matched_terms,
        "decision_terms": row.decision_terms,
        "score": row.score_bp,
        "publish_tip": row.publish_tip,
        "created_at_ms": row.created_at_ms,
    })
}

enum Refused {
    Terminal(&'static str),
    Kernel(KernelError),
    InvalidCursor,
}

impl HandlerCore {
    pub(crate) async fn handle_review_candidates(
        &self,
        channel: RouteHandle,
        request: Value,
    ) -> PreparedOutcome {
        let consented = match self.management_binding(channel, &request, CANDIDATES) {
            Ok((_, binding)) => binding.config.contradiction_candidates_enabled,
            Err(outcome) => return outcome,
        };
        let (scope, parsed) =
            match self.kernel_request::<CandidatesRequest>(channel, request, CANDIDATES) {
                Ok(bound) => bound,
                Err(outcome) => return outcome,
            };
        if parsed.decision_object_id.is_empty()
            || parsed.decision_object_id.len() > MAX_OBJECT_ID_BYTES
        {
            return crate::invalid_params_error(format!(
                "{CANDIDATES} requires a decision_object_id of 1 to {MAX_OBJECT_ID_BYTES} bytes"
            ));
        }
        if !consented {
            return terminal("disabled");
        }
        let Some(ledger) = self.store() else {
            return terminal("store_unavailable");
        };
        let counts = Arc::clone(&self.contradiction_candidates);
        let limit = parsed.limit.unwrap_or(MAX_CANDIDATE_PAGE);
        let root = scope.project_root.to_string_lossy().to_string();
        let read = blocking(move || -> Result<Value, Refused> {
            match memories_authority_for_route(&ledger, &root) {
                Ok(MemoriesAuthority::Module(_)) => {}
                Ok(MemoriesAuthority::NotModule { .. }) => {
                    return Err(Refused::Terminal("disabled"));
                }
                Err(_) => return Err(Refused::Terminal("store_unavailable")),
            }
            let kernel = &scope.store;
            let budget = EvalBudget::new(Some(Instant::now() + READ_BUDGET), Arc::default());
            let target = kernel
                .capture_identified_commit_read_target_within_budget(&budget)
                .map_err(Refused::Kernel)?;
            let bounds = ClaimFactBounds {
                max_claims: NonZeroUsize::MIN,
                max_causal_payload_bytes: NonZeroU64::new(1024 * 1024).expect("nonzero"),
            };
            let decision = std::slice::from_ref(&parsed.decision_object_id);
            let facts =
                match kernel.claim_facts_at_within_budget(decision, target.target, bounds, &budget)
                {
                    Ok(read) => read.claims.into_iter().next(),
                    Err(ClaimFactsError::NotADecision) => None,
                    Err(ClaimFactsError::Kernel(error)) => return Err(Refused::Kernel(error)),
                    Err(_) => return Err(Refused::Kernel(KernelError::CorruptCanonicalRow)),
                };
            let live = match &facts {
                Some(facts) => {
                    let mut filter = ScopeFilter::new(&scope.project);
                    let names_project = filter
                        .matches(
                            facts.decision.scope_id.as_deref(),
                            &mut stored_terms(kernel),
                        )
                        .map_err(Refused::Kernel)?;
                    eligible(facts, names_project)
                }
                None => false,
            };
            if !live {
                return Err(Refused::Terminal("not_live"));
            }
            let page = ledger
                .candidates_for_decision(
                    scope.project.digest(),
                    &parsed.decision_object_id,
                    parsed.after.as_deref(),
                    limit,
                )
                .map_err(|error| match error {
                    MemoryStoreError::Serde(_) => Refused::InvalidCursor,
                    _ => Refused::Terminal("store_unavailable"),
                })?;
            let evidence: Vec<&str> = page
                .items
                .iter()
                .map(|row| row.evidence_id.as_str())
                .collect();
            let standing = kernel
                .evidence_liveness_at_within_budget(&evidence, target.target, &budget)
                .map_err(|error| match error {
                    kernel::EvidenceLivenessError::Kernel(error) => Refused::Kernel(error),
                    _ => Refused::Kernel(KernelError::Conflict),
                })?;
            let (kept, stale): (Vec<_>, Vec<_>) =
                page.items
                    .iter()
                    .zip(standing)
                    .partition(|(row, liveness)| {
                        *liveness == EvidenceLiveness::Live
                            && row.kernel_incarnation_id == target.database_incarnation_id
                            && row.matcher_digest == matcher_digest()
                            && row.publish_tip <= target.target.through_commit
                    });
            counts.add(Count::ReadFilteredStale, stale.len() as u64);
            Ok(json!({
                "kind": "page",
                "as_of": target.target.through_commit,
                "items": kept.iter().map(|(row, _)| item(row)).collect::<Vec<_>>(),
                "next": page.next,
            }))
        })
        .await;
        match read {
            Ok(Ok(page)) => response(page),
            Ok(Err(Refused::Terminal(code))) => terminal(code),
            Ok(Err(Refused::Kernel(error))) => state_only(error.into()),
            Ok(Err(Refused::InvalidCursor)) => crate::invalid_params_error(format!(
                "{CANDIDATES} requires an after cursor a prior page returned"
            )),
            Err(outcome) => state_only(outcome),
        }
    }
}

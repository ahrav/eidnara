use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};

use host_runtime::RouteHandle;
use kernel::applicability::EvalBudget;
use kernel::{
    ClaimFactsError, EvidenceLiveness, IdentifiedCommitReadTarget, KernelError, KernelStore,
    ScopeTermsError,
};
use memory_store::MemoryStoreError;
use memory_store::contradiction_candidates::{MAX_CANDIDATE_PAGE, StoredCandidate};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Count;
use super::eligibility::eligible;
use super::scorer::matcher_digest;
use crate::dispatch::{PreparedOutcome, PreparedOutput};
use crate::kernel_routes::eligibility::MAX_OBJECT_ID_BYTES;
use crate::kernel_routes::project::{ProjectBinding, ScopeFilter};
use crate::kernel_routes::{blocking, state_only};
use crate::{HandlerCore, MemoriesAuthority, memories_authority};

pub(crate) const CANDIDATES: &str = "review.candidates";
pub(super) const READ_BUDGET: Duration = Duration::from_secs(5);
const ONE_CLAIM: NonZeroUsize = NonZeroUsize::new(1).expect("nonzero");

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CandidatesRequest {
    decision_object_id: String,
    /// Kept as a JSON value so a spelling other than a string or `null` is judged
    /// with the cursor form, after the terminals.
    #[serde(default)]
    after: Option<Value>,
    /// A number past the machine word is the page bound, so a limit of any size clamps.
    #[serde(default, deserialize_with = "saturating_limit")]
    limit: Option<usize>,
}

/// An absent or `null` limit is `None`; a non-negative integer, including one
/// serde reads as a float past `u64`, saturates into `usize`; any other
/// spelling is the deserializer's error.
fn saturating_limit<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<usize>, D::Error> {
    let Some(value) = Option::<Value>::deserialize(deserializer)? else {
        return Ok(None);
    };
    let Value::Number(number) = &value else {
        return Err(serde::de::Error::custom("limit is not an unsigned integer"));
    };
    if let Some(limit) = number.as_u64() {
        return Ok(Some(usize::try_from(limit).unwrap_or(usize::MAX)));
    }
    match number.as_f64() {
        Some(limit) if limit.is_sign_positive() && limit.fract() == 0.0 => Ok(Some(usize::MAX)),
        _ => Err(serde::de::Error::custom("limit is not an unsigned integer")),
    }
}

/// The `after` field's reading: the page start, an issued cursor to decode, or a
/// spelling the store never issued.
enum Cursor {
    Start,
    Issued(String),
    Malformed,
}

impl Cursor {
    fn of(after: Option<Value>) -> Self {
        match after {
            None | Some(Value::Null) => Self::Start,
            Some(Value::String(cursor)) => Self::Issued(cursor),
            Some(_) => Self::Malformed,
        }
    }
}

fn response(body: Value) -> PreparedOutcome {
    PreparedOutcome::Response(PreparedOutput::json(body))
}

fn terminal(code: &str) -> PreparedOutcome {
    response(json!({ "kind": "terminal", "terminal": code }))
}

/// The page body, encoded once from the store rows. Fields are declared in
/// lexicographic key order, the order a `serde_json` object encodes its keys,
/// so the body is the canonical encoding of the page's JSON value.
#[derive(Serialize)]
struct PageOut<'a> {
    as_of: i64,
    items: Vec<ItemOut<'a>>,
    kind: &'static str,
    next: Option<&'a str>,
}

#[derive(Serialize)]
struct ItemOut<'a> {
    created_at_ms: i64,
    decision_id: &'a str,
    decision_object_id: &'a str,
    decision_terms: u32,
    evidence_id: &'a str,
    matched_terms: u32,
    publish_tip: i64,
    score: u32,
}

fn item(row: &StoredCandidate) -> ItemOut<'_> {
    ItemOut {
        created_at_ms: row.created_at_ms,
        decision_id: &row.decision_id,
        decision_object_id: &row.decision_object_id,
        decision_terms: row.decision_terms,
        evidence_id: &row.evidence_id,
        matched_terms: row.matched_terms,
        publish_tip: row.publish_tip,
        score: row.score_bp,
    }
}

enum Refused {
    Terminal(&'static str),
    Kernel(KernelError),
    InvalidCursor,
}

fn decision_is_live(
    kernel: &KernelStore,
    project: &ProjectBinding,
    target: &IdentifiedCommitReadTarget,
    decision_object_id: &str,
    budget: &EvalBudget,
    #[cfg(test)] counts: &super::Candidates,
) -> Result<bool, Refused> {
    let facts = match kernel.claim_standing_at_within_budget(
        &[decision_object_id.to_string()],
        target.target,
        ONE_CLAIM,
        budget,
    ) {
        Ok(mut standings) => standings.pop().flatten(),
        Err(ClaimFactsError::NotADecision) => None,
        Err(ClaimFactsError::Kernel(error)) => return Err(Refused::Kernel(error)),
        Err(ClaimFactsError::IncarnationMismatch) => {
            return Err(Refused::Kernel(KernelError::Conflict));
        }
        Err(_) => return Err(Refused::Kernel(KernelError::CorruptCanonicalRow)),
    };
    let Some(facts) = facts else {
        return Ok(false);
    };
    #[cfg(test)]
    {
        let hook = counts.before_scope_read.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook();
        }
    }
    // The scope read is fenced to the history the claim standing came from, so a restore between the two reads answers the conflict state in place of a judgment over mixed histories.
    let names_project = ScopeFilter::new(project)
        .matches(facts.decision.scope_id.as_deref(), &mut |scope: &str| {
            kernel
                .scope_terms_at_within_budget(scope, target.target, budget)
                .map_err(|error| match error {
                    ScopeTermsError::Kernel(error) => error,
                    ScopeTermsError::IncarnationMismatch => KernelError::Conflict,
                })
        })
        .map_err(Refused::Kernel)?;
    Ok(eligible(&facts, names_project))
}

fn is_current(
    row: &StoredCandidate,
    liveness: EvidenceLiveness,
    target: &IdentifiedCommitReadTarget,
) -> bool {
    liveness == EvidenceLiveness::Live
        && row.kernel_incarnation_id == target.database_incarnation_id
        && row.matcher_digest == matcher_digest()
        && row.publish_tip <= target.target.through_commit
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
        let cursor = Cursor::of(parsed.after);
        let root = scope.project_root.to_string_lossy().to_string();
        // The budget runs from admission: the wait for a blocking thread, both store reads, and every Kernel read share it, so a held store connection or Kernel reader answers at the bound.
        #[cfg(test)]
        let budget = counts.read_budget.lock().unwrap().unwrap_or(READ_BUDGET);
        #[cfg(not(test))]
        let budget = READ_BUDGET;
        let deadline = Instant::now() + budget;
        let read = blocking(move || -> Result<Vec<u8>, Refused> {
            match ledger.module_authority_for_route_within(&root, "memories", deadline) {
                Ok(row) => match memories_authority(row) {
                    MemoriesAuthority::Module(_) => {}
                    MemoriesAuthority::NotModule { .. } => {
                        return Err(Refused::Terminal("disabled"));
                    }
                },
                Err(_) => return Err(Refused::Terminal("store_unavailable")),
            }
            let kernel = &scope.store;
            let budget = EvalBudget::new(Some(deadline), Arc::default());
            let target = kernel
                .capture_identified_commit_read_target_within_budget(&budget)
                .map_err(Refused::Kernel)?;
            let live = decision_is_live(
                kernel,
                &scope.project,
                &target,
                &parsed.decision_object_id,
                &budget,
                #[cfg(test)]
                &counts,
            )?;
            if !live {
                return Err(Refused::Terminal("not_live"));
            }
            // A malformed spelling runs the first-page query so a storage failure still answers its terminal.
            let after = match &cursor {
                Cursor::Issued(cursor) => Some(cursor.as_str()),
                Cursor::Start | Cursor::Malformed => None,
            };
            // The page read judges the route's authority in the connection that reads the page, so a drain or move after the entry check answers `disabled`.
            let page = ledger
                .candidates_for_decision_under_route_within(
                    &root,
                    "memories",
                    scope.project.digest(),
                    &parsed.decision_object_id,
                    after,
                    limit,
                    deadline,
                )
                .map_err(|error| match error {
                    MemoryStoreError::Serde(_) if after.is_some() => Refused::InvalidCursor,
                    _ => Refused::Terminal("store_unavailable"),
                })?;
            let page = match page {
                Some((row, page)) => match memories_authority(Some(row)) {
                    MemoriesAuthority::Module(_) => page,
                    MemoriesAuthority::NotModule { .. } => {
                        return Err(Refused::Terminal("disabled"));
                    }
                },
                None => return Err(Refused::Terminal("disabled")),
            };
            if matches!(cursor, Cursor::Malformed) {
                return Err(Refused::InvalidCursor);
            }
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
            let items: Vec<ItemOut<'_>> = page
                .items
                .iter()
                .zip(standing)
                .filter(|(row, liveness)| is_current(row, *liveness, &target))
                .map(|(row, _)| item(row))
                .collect();
            counts.add(
                Count::ReadFilteredStale,
                (page.items.len() - items.len()) as u64,
            );
            // Encoding strings and integers into a `Vec` succeeds; a failure answers store_unavailable.
            serde_json::to_vec(&PageOut {
                as_of: target.target.through_commit,
                items,
                kind: "page",
                next: page.next.as_deref(),
            })
            .map_err(|_| Refused::Terminal("store_unavailable"))
        })
        .await;
        match read {
            Ok(Ok(body)) => PreparedOutcome::Response(PreparedOutput::cached_bytes(body)),
            Ok(Err(Refused::Terminal(code))) => terminal(code),
            Ok(Err(Refused::Kernel(error))) => state_only(error.into()),
            Ok(Err(Refused::InvalidCursor)) => crate::invalid_params_error(format!(
                "{CANDIDATES} requires an after cursor a prior page returned"
            )),
            Err(outcome) => state_only(outcome),
        }
    }
}

//! The context application protocol's review operations: a bounded list of a project's completed review outcomes and the read of one selected proposal. Both are local reads over the bound route's project. An unbound root or an unready Kernel answers as every kernel route does; an uninstalled Memory Store or a root whose memories authority is not MODULE answers `disabled`, the terminal the lane's other methods use while they are not installed. A receipt lists from its row alone; a read goes through the same receipt-selected path every reader uses, so a proposal is visible only while its receipt is complete, its selection matches, and its review hold is live.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};

use context_core::canonical_json::is_lower_hex;

use crate::dispatch::{PreparedOutcome, PreparedOutput};
use crate::kernel_routes::blocking;
use crate::{HandlerCore, MemoriesAuthority, memories_authority_for_route};
use host_runtime::RouteHandle;
use memory_store::curator_ledger::MAX_RECEIPT_PAGE;
use memory_store::{MemoryStore, MemoryStoreError};

use super::settlement::{ReadRefusal, ReviewOutcome, list_review_outcomes, read_selected_proposal};
use super::worker::job_binding;

pub(crate) const LIST: &str = "review.list";
pub(crate) const READ: &str = "review.read";

/// The list page a caller asks for; `limit` is capped at [`MAX_RECEIPT_PAGE`].
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListRequest {
    #[serde(default)]
    after: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadRequest {
    causal_identity: String,
}

/// The stores and the ledger project one review operation reads under.
struct ReviewScope {
    kernel: Arc<kernel::KernelStore>,
    ledger: Arc<MemoryStore>,
    project: String,
}

fn response(body: Value) -> PreparedOutcome {
    PreparedOutcome::Response(PreparedOutput::json(body))
}

fn terminal(code: &str) -> PreparedOutcome {
    response(json!({ "kind": "terminal", "terminal": code }))
}

/// A causal identity on the wire is a lower-hex sha256; any other spelling, as a read target or as a list cursor, is `invalid_params` rather than a text comparison that skips or repeats outcomes.
fn require_causal_identity(
    operation: &str,
    field: &str,
    value: &str,
) -> Result<(), PreparedOutcome> {
    if is_lower_hex(value, 64) {
        Ok(())
    } else {
        Err(crate::invalid_params_error(format!(
            "{operation} requires a lower-hex sha256 {field}"
        )))
    }
}

/// The wire spelling of why a selected proposal was not returned: a closed set, never the refusal's text.
fn read_terminal(refusal: &ReadRefusal) -> &'static str {
    match refusal {
        ReadRefusal::NotSelected => "not_selected",
        ReadRefusal::IncarnationMismatch => "incarnation_mismatch",
        ReadRefusal::SelectionMismatch => "selection_mismatch",
        ReadRefusal::Kernel(_) => "kernel_refused",
        ReadRefusal::ReviewExpired => "review_expired",
        ReadRefusal::Dependency(_) => "dependency_refused",
        ReadRefusal::Store(_) => "store_unavailable",
    }
}

fn authority_project(
    authority: Result<MemoriesAuthority, MemoryStoreError>,
) -> Result<String, &'static str> {
    match authority {
        Ok(MemoriesAuthority::Module(authority)) => Ok(authority.project),
        Ok(MemoriesAuthority::NotModule { .. }) => Err("disabled"),
        Err(_) => Err("store_unavailable"),
    }
}

/// One completed outcome as the operator surface lists it: identity, generation, the terminal under `outcome`, and an abstention's reason beside it.
fn outcome_item(outcome: &ReviewOutcome) -> Value {
    let mut item = json!({
        "causal_identity": outcome.causal_identity,
        "generation": outcome.generation,
        "outcome": outcome.terminal.as_str(),
        "selected": outcome.selected,
    });
    if let Some(reason) = outcome.abstained_reason {
        item["reason"] = json!(reason.as_str());
    }
    item
}

/// The route and stores one review operation needs before any store is queried; the authority lookup itself runs off the async thread.
struct Bound {
    kernel: Arc<kernel::KernelStore>,
    ledger: Arc<MemoryStore>,
    root: String,
}

impl Bound {
    fn review_scope(self) -> Result<ReviewScope, &'static str> {
        let project = authority_project(memories_authority_for_route(&self.ledger, &self.root))?;
        Ok(ReviewScope {
            kernel: self.kernel,
            ledger: self.ledger,
            project,
        })
    }
}

impl HandlerCore {
    fn bind_review<T: serde::de::DeserializeOwned>(
        &self,
        channel: RouteHandle,
        request: Value,
        operation: &str,
    ) -> Result<(Bound, T), PreparedOutcome> {
        let (scope, parsed) = self.kernel_request::<T>(channel, request, operation)?;
        let Some(ledger) = self.store() else {
            return Err(terminal("disabled"));
        };
        Ok((
            Bound {
                kernel: scope.store,
                ledger,
                root: scope.project_root.to_string_lossy().to_string(),
            },
            parsed,
        ))
    }

    pub(crate) async fn handle_review_list(
        &self,
        channel: RouteHandle,
        request: Value,
    ) -> PreparedOutcome {
        let (bound, parsed) = match self.bind_review::<ListRequest>(channel, request, LIST) {
            Ok(bound) => bound,
            Err(outcome) => return outcome,
        };
        if let Some(after) = parsed.after.as_deref()
            && let Err(outcome) = require_causal_identity(LIST, "after", after)
        {
            return outcome;
        }
        let limit = parsed.limit.unwrap_or(MAX_RECEIPT_PAGE);
        let page = blocking(move || {
            let scope = bound.review_scope()?;
            list_review_outcomes(
                &scope.ledger,
                &scope.project,
                parsed.after.as_deref(),
                limit,
            )
            .map_err(|refusal| read_terminal(&refusal))
        })
        .await;
        match page {
            Ok(Ok(page)) => response(json!({
                "kind": "page",
                "items": page.outcomes.iter().map(outcome_item).collect::<Vec<_>>(),
                "next": page.next,
            })),
            Ok(Err(code)) => terminal(code),
            Err(_) => terminal("store_unavailable"),
        }
    }

    pub(crate) async fn handle_review_read(
        &self,
        channel: RouteHandle,
        request: Value,
    ) -> PreparedOutcome {
        let (bound, parsed) = match self.bind_review::<ReadRequest>(channel, request, READ) {
            Ok(bound) => bound,
            Err(outcome) => return outcome,
        };
        if let Err(outcome) =
            require_causal_identity(READ, "causal_identity", &parsed.causal_identity)
        {
            return outcome;
        }
        let read = blocking(move || {
            let scope = bound.review_scope()?;
            let job = scope
                .ledger
                .lookup_curator_job(&scope.project, &parsed.causal_identity)
                .map_err(|_| "store_unavailable")?
                .ok_or("not_selected")?;
            read_selected_proposal(
                &scope.kernel,
                &scope.ledger,
                &scope.project,
                &parsed.causal_identity,
                |project_digest| job_binding(project_digest, &job),
                crate::now_ms(),
            )
            .map(|selected| (parsed.causal_identity, selected))
            .map_err(|refusal| read_terminal(&refusal))
        })
        .await;
        match read {
            Ok(Ok((causal_identity, selected))) => response(json!({
                "kind": "proposal",
                "causal_identity": causal_identity,
                "reference": selected.reference,
                "proposal": selected.proposal,
                "review_expires_at": selected.review_expires_at,
            })),
            Ok(Err(code)) => terminal(code),
            Err(_) => terminal("store_unavailable"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ModuleMemoriesAuthority;

    #[test]
    fn authority_project_separates_a_store_fault_from_a_root_outside_module_authority() {
        assert_eq!(
            authority_project(Ok(MemoriesAuthority::Module(ModuleMemoriesAuthority {
                context_store_uuid: "ctx".to_string(),
                project: "git:proj".to_string(),
                generation: 1,
            }))),
            Ok("git:proj".to_string())
        );
        assert_eq!(
            authority_project(Ok(MemoriesAuthority::NotModule {
                message: "memories authority is PREPARING".to_string(),
            })),
            Err("disabled")
        );
        assert_eq!(
            authority_project(Err(MemoryStoreError::Serde("read failed".to_string()))),
            Err("store_unavailable"),
            "a store that could not be read is not a disabled capability"
        );
    }
}

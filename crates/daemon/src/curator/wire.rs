//! The context application protocol's review operations: a bounded list of a project's completed review outcomes and the read of one selected proposal. Both are local reads over the bound route's project. An unbound root or an unready Kernel answers as every kernel route does; an uninstalled Memory Store or a root whose memories authority is not MODULE answers `disabled`, the terminal the lane's other methods use while they are not installed. A receipt lists from its row alone; a read goes through the same receipt-selected path every reader uses, so a proposal is visible only while its receipt is complete, its selection matches, and its review hold is live.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::dispatch::{PreparedOutcome, PreparedOutput};
use crate::kernel_routes::state::{InvalidReason, KernelOutcome};
use crate::kernel_routes::{blocking, parse_request_body, state_only};
use crate::{HandlerCore, MemoriesAuthority, memories_authority_for_route};
use host_runtime::RouteHandle;
use memory_store::MemoryStore;
use memory_store::curator_ledger::MAX_RECEIPT_PAGE;

use super::settlement::{ReadRefusal, ReviewOutcome, list_review_outcomes, read_selected_proposal};
use super::worker::{job_binding, module_projects};

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
    /// The Kernel project digest jobs of this authority project are staged and read under: the worker's one-route-per-project rule, so a read on any bound root of the project resolves what the worker staged.
    project_digest: String,
}

fn response(body: Value) -> PreparedOutcome {
    PreparedOutcome::Response(PreparedOutput::json(body))
}

fn terminal(code: &str) -> PreparedOutcome {
    response(json!({ "kind": "terminal", "terminal": code }))
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
    bindings: Arc<std::sync::Mutex<crate::RouteBindings>>,
}

impl Bound {
    /// The authority route binding is keyed by the root as the route bound it, the spelling every other authority lookup uses. A root whose memories authority is not MODULE has no ledger project to answer for.
    fn review_scope(self) -> Result<ReviewScope, &'static str> {
        let project = match memories_authority_for_route(&self.ledger, &self.root) {
            Ok(MemoriesAuthority::Module(authority)) => authority.project,
            Ok(MemoriesAuthority::NotModule { .. }) | Err(_) => return Err("disabled"),
        };
        let project_digest = module_projects(&self.ledger, &self.bindings)
            .into_iter()
            .find(|route| route.project == project)
            .map(|route| route.project_digest)
            .ok_or("disabled")?;
        Ok(ReviewScope {
            kernel: self.kernel,
            ledger: self.ledger,
            project,
            project_digest,
        })
    }
}

impl HandlerCore {
    /// The route must be bound to the requested root and the Kernel ready, answered as the kernel routes answer them; an uninstalled Memory Store is `disabled`.
    fn bind_review(
        &self,
        channel: RouteHandle,
        request: &Value,
        operation: &str,
    ) -> Result<Bound, PreparedOutcome> {
        let (_, binding) = self.management_binding(channel, request, operation)?;
        let Some(requested_root) = request.get("project_root").and_then(Value::as_str) else {
            return Err(crate::invalid_params_error(format!(
                "{operation} requires project_root"
            )));
        };
        if !binding
            .kernel_project
            .accepts(std::path::Path::new(requested_root))
        {
            return Err(state_only(KernelOutcome::invalid(
                InvalidReason::ProjectMismatch,
            )));
        }
        let kernel = self.kernel.kernel_store().map_err(state_only)?;
        let Some(ledger) = self.store() else {
            return Err(terminal("disabled"));
        };
        Ok(Bound {
            kernel,
            ledger,
            root: binding.project_root.to_string_lossy().to_string(),
            bindings: Arc::clone(&self.bindings),
        })
    }

    pub(crate) async fn handle_review_list(
        &self,
        channel: RouteHandle,
        request: Value,
    ) -> PreparedOutcome {
        let bound = match self.bind_review(channel, &request, LIST) {
            Ok(bound) => bound,
            Err(outcome) => return outcome,
        };
        let parsed: ListRequest = match parse_request_body(request, LIST) {
            Ok(parsed) => parsed,
            Err(outcome) => return outcome,
        };
        let limit = parsed
            .limit
            .unwrap_or(MAX_RECEIPT_PAGE)
            .clamp(1, MAX_RECEIPT_PAGE);
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
        let bound = match self.bind_review(channel, &request, READ) {
            Ok(bound) => bound,
            Err(outcome) => return outcome,
        };
        let parsed: ReadRequest = match parse_request_body(request, READ) {
            Ok(parsed) => parsed,
            Err(outcome) => return outcome,
        };
        if parsed.causal_identity.len() != 64
            || !parsed
                .causal_identity
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            return crate::invalid_params_error(format!(
                "{READ} requires a lower-hex sha256 causal_identity"
            ));
        }
        let read = blocking(move || {
            let scope = bound.review_scope()?;
            let job = scope
                .ledger
                .lookup_curator_job(&scope.project, &parsed.causal_identity)
                .map_err(|_| "store_unavailable")?
                .ok_or("not_selected")?;
            let binding = job_binding(&scope.project_digest, &job);
            read_selected_proposal(
                &scope.kernel,
                &scope.ledger,
                &scope.project,
                &parsed.causal_identity,
                &binding,
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

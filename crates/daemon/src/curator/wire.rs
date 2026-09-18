//! The context application protocol's review operations: a bounded list of a project's review receipts and the read of one selected proposal. Both are local reads over the bound route's project. They answer only for a project whose memories authority is MODULE, from the live Kernel and Memory Store; anything else is `disabled`, the same terminal the rest of the protocol answers while it is not installed. A receipt lists from its row alone; a read goes through the same receipt-selected path every reader uses, so a proposal is visible only while its receipt is complete, its selection matches, and its review hold is live.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::dispatch::{PreparedOutcome, PreparedOutput};
use crate::kernel_routes::{RouteScope, blocking, parse_request_body};
use crate::{HandlerCore, MemoriesAuthority, memories_authority_for_route};
use host_runtime::RouteHandle;
use memory_store::MemoryStore;
use memory_store::curator_ledger::{CURATOR_RECEIPT_PAGE_MAX, CuratorReceipt};

use super::settlement::{ReadRefusal, read_selected_proposal};
use super::worker::job_binding;

pub(crate) const LIST: &str = "review.list";
pub(crate) const READ: &str = "review.read";

/// The list page a caller asks for; `limit` is capped at [`CURATOR_RECEIPT_PAGE_MAX`].
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

/// One receipt as the operator surface lists it: identity, generation, the outcome under `outcome`, and an abstention's reason beside it.
fn receipt_item(receipt: &CuratorReceipt) -> Value {
    let mut item = json!({
        "causal_identity": receipt.causal_identity,
        "generation": receipt.generation,
        "outcome": receipt
            .terminal
            .map(|terminal| terminal.as_str())
            .unwrap_or("in_progress"),
        "selected": receipt.selected.is_some(),
    });
    if let Some(reason) = receipt.abstained_reason {
        item["reason"] = json!(reason.as_str());
    }
    item
}

impl HandlerCore {
    /// The route must be bound to the requested root, the Kernel ready, the Memory Store installed, and the root's memories authority MODULE; otherwise the operation is `disabled`.
    fn review_scope(
        &self,
        channel: RouteHandle,
        request: &Value,
        operation: &str,
    ) -> Result<ReviewScope, PreparedOutcome> {
        // The authority route binding is keyed by the root as the route bound it, the spelling every other authority lookup uses, not the canonical root the Kernel digest is built from.
        let (_, binding) = self.management_binding(channel, request, operation)?;
        let RouteScope { store, project, .. } =
            self.kernel_route_scope(channel, request, operation)?;
        let Some(ledger) = self.store() else {
            return Err(terminal("disabled"));
        };
        let root = binding.project_root.to_string_lossy().to_string();
        match memories_authority_for_route(&ledger, &root) {
            Ok(MemoriesAuthority::Module(authority)) => Ok(ReviewScope {
                kernel: store,
                ledger,
                project: authority.project,
                project_digest: project.digest().to_string(),
            }),
            Ok(MemoriesAuthority::NotModule { .. }) | Err(_) => Err(terminal("disabled")),
        }
    }

    pub(crate) async fn handle_review_list(
        &self,
        channel: RouteHandle,
        request: Value,
    ) -> PreparedOutcome {
        let scope = match self.review_scope(channel, &request, LIST) {
            Ok(scope) => scope,
            Err(outcome) => return outcome,
        };
        let parsed: ListRequest = match parse_request_body(request, LIST) {
            Ok(parsed) => parsed,
            Err(outcome) => return outcome,
        };
        let limit = parsed
            .limit
            .unwrap_or(CURATOR_RECEIPT_PAGE_MAX)
            .clamp(1, CURATOR_RECEIPT_PAGE_MAX);
        let page = blocking(move || {
            scope
                .ledger
                .list_curator_receipts(&scope.project, parsed.after.as_deref(), limit)
        })
        .await;
        match page {
            Ok(Ok(receipts)) => {
                let next = (receipts.len() == limit)
                    .then(|| {
                        receipts
                            .last()
                            .map(|receipt| receipt.causal_identity.clone())
                    })
                    .flatten();
                response(json!({
                    "kind": "page",
                    "items": receipts.iter().map(receipt_item).collect::<Vec<_>>(),
                    "next": next,
                }))
            }
            Ok(Err(_)) | Err(_) => terminal("store_unavailable"),
        }
    }

    pub(crate) async fn handle_review_read(
        &self,
        channel: RouteHandle,
        request: Value,
    ) -> PreparedOutcome {
        let scope = match self.review_scope(channel, &request, READ) {
            Ok(scope) => scope,
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
            let job = scope
                .ledger
                .lookup_curator_job(&scope.project, &parsed.causal_identity)
                .map_err(|error| ReadRefusal::Store(error.to_string()))?
                .ok_or(ReadRefusal::NotSelected)?;
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
            Ok(Err(refusal)) => terminal(read_terminal(&refusal)),
            Err(_) => terminal("store_unavailable"),
        }
    }
}

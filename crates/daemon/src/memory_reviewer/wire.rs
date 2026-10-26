//! The context application protocol's review operations: a bounded list of a project's completed review outcomes, the read of one selected proposal, the explicit acceptance of one, and the use-outcome counts of one decision. List and read are local reads over the bound route's project; acceptance applies the selected proposal through the Kernel only while the owner's acceptance record admits it. An unbound root or an unready Kernel answers as every kernel route does; an uninstalled Memory Store or a root whose memories authority is not MODULE answers `disabled`, the terminal the lane's other methods use while they are not installed. A receipt lists from its row alone; a read goes through the same receipt-selected path every reader uses, so a proposal is visible only while its receipt is complete, its selection matches, and its review hold is live.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};

use context_core::canonical_json::is_lower_hex;

use crate::dispatch::{PreparedOutcome, PreparedOutput};
use crate::kernel_routes::blocking;
use crate::{HandlerCore, MemoriesAuthority, memories_authority_for_route};
use host_runtime::RouteHandle;
use memory_store::memory_reviewer_ledger::{MAX_RECEIPT_PAGE, MemoryReviewerReceipt};
use memory_store::{MemoryStore, MemoryStoreError};

use super::settlement::{
    AcceptRefusal, ReadRefusal, ReviewOutcome, accept_selected_proposal, list_review_outcomes,
    read_selected_proposal,
};

pub(crate) const LIST: &str = "review.list";
pub(crate) const READ: &str = "review.read";
pub(crate) const ACCEPT: &str = "review.accept";
pub(crate) const USE_OUTCOME: &str = "review.use_outcome";
/// Longest caller operation key `review.accept` admits.
const MAX_OPERATION_KEY_BYTES: usize = 128;
/// How long the acceptance gate may wait for a Kernel reader to learn the live incarnation.
const GATE_READ_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);

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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AcceptRequest {
    causal_identity: String,
    operation_key: String,
    target: kernel::ProposalTarget,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UseOutcomeRequest {
    object_id: String,
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

/// An operation key is 1 to [`MAX_OPERATION_KEY_BYTES`] visible ASCII bytes (0x21 to 0x7E); any other spelling is `invalid_params`.
fn require_operation_key(key: &str) -> Result<(), PreparedOutcome> {
    if !key.is_empty()
        && key.len() <= MAX_OPERATION_KEY_BYTES
        && key.bytes().all(|byte| byte.is_ascii_graphic())
    {
        Ok(())
    } else {
        Err(crate::invalid_params_error(format!(
            "{ACCEPT} requires an operation_key of 1 to {MAX_OPERATION_KEY_BYTES} visible ASCII bytes"
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

/// The wire spelling of why an acceptance was neither applied nor replayed: a closed set, never the refusal's text.
fn accept_terminal(refusal: &AcceptRefusal) -> &'static str {
    match refusal {
        AcceptRefusal::Receipt(refusal) => read_terminal(refusal),
        AcceptRefusal::Kernel(kernel::AcceptanceRefusal::Unavailable) => "unavailable",
        AcceptRefusal::Kernel(kernel::AcceptanceRefusal::Expired) => "review_expired",
        AcceptRefusal::Kernel(kernel::AcceptanceRefusal::Changed) => "changed",
        AcceptRefusal::Kernel(kernel::AcceptanceRefusal::Conflict) => "conflict",
        AcceptRefusal::Kernel(kernel::AcceptanceRefusal::Invalid) => "invalid",
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

    /// [`Self::review_scope`] with the Memory Store incarnation and `causal_identity`'s receipt copied from the authority's own read, so the store is released before the Kernel is entered.
    fn review_scope_with_receipt(
        self,
        causal_identity: &str,
    ) -> Result<(ReviewScope, String, Option<MemoryReviewerReceipt>), &'static str> {
        let read = self.ledger.module_authority_with_memory_reviewer_receipt(
            &self.root,
            "memories",
            causal_identity,
        );
        let (row, copied) = match read {
            Ok(Some((row, incarnation, receipt))) => (Some(row), Some((incarnation, receipt))),
            Ok(None) => (None, None),
            Err(_) => return Err("store_unavailable"),
        };
        let project = authority_project(Ok(crate::memories_authority(row)))?;
        let (incarnation, receipt) = copied.ok_or("disabled")?;
        Ok((
            ReviewScope {
                kernel: self.kernel,
                ledger: self.ledger,
                project,
            },
            incarnation,
            receipt,
        ))
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
            scope
                .ledger
                .lookup_memory_reviewer_job(&scope.project, &parsed.causal_identity)
                .map_err(|_| "store_unavailable")?
                .ok_or("not_selected")?;
            read_selected_proposal(
                &scope.kernel,
                &scope.ledger,
                &scope.project,
                &parsed.causal_identity,
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

impl HandlerCore {
    /// `review.use_outcome`: the exact served and disputed-after-serve counts of one decision object within the bound project, at one Kernel snapshot. It binds through the kernel-route scope alone, so it needs no Memory Store, and an unready Kernel or a failed read answers its kernel state with no count.
    pub(crate) async fn handle_review_use_outcome(
        &self,
        channel: RouteHandle,
        request: Value,
    ) -> PreparedOutcome {
        let (scope, parsed) =
            match self.kernel_request::<UseOutcomeRequest>(channel, request, USE_OUTCOME) {
                Ok(bound) => bound,
                Err(outcome) => return outcome,
            };
        if parsed.object_id.is_empty()
            || parsed.object_id.len() > crate::kernel_routes::eligibility::MAX_OBJECT_ID_BYTES
        {
            return crate::invalid_params_error(format!(
                "{USE_OUTCOME} requires an object_id of 1 to {} bytes",
                crate::kernel_routes::eligibility::MAX_OBJECT_ID_BYTES
            ));
        }
        let counts = blocking(move || {
            scope
                .store
                .served_counts_for(scope.project.digest(), &parsed.object_id)
                .map(|counts| (parsed.object_id, counts))
        })
        .await;
        match counts {
            Ok(Ok((object_id, counts))) => response(json!({
                "kind": "counts",
                "object_id": object_id,
                "served_sessions": counts.served_sessions,
                "disputed_after_serve": counts.disputed_after_serve,
            })),
            Ok(Err(error)) => crate::kernel_routes::state_only(error.into()),
            Err(outcome) => crate::kernel_routes::state_only(outcome),
        }
    }

    pub(crate) async fn handle_review_accept(
        &self,
        channel: RouteHandle,
        request: Value,
    ) -> PreparedOutcome {
        let (bound, parsed) = match self.bind_review::<AcceptRequest>(channel, request, ACCEPT) {
            Ok(bound) => bound,
            Err(outcome) => return outcome,
        };
        if let Err(outcome) =
            require_causal_identity(ACCEPT, "causal_identity", &parsed.causal_identity)
        {
            return outcome;
        }
        if let Err(outcome) = require_operation_key(&parsed.operation_key) {
            return outcome;
        }
        let home = self.memory_reviewer_home();
        let held = Arc::clone(&self.memory_reviewer_acceptance_record);
        let accepted = blocking(move || {
            // The owner's acceptance record is its own gate: the disclosure record, open or not, admits no acceptance. It reads no store, so it is read first and a sound record's Memory Store inputs come from the authority's own read; the route's state is still answered before the gate.
            let record = home
                .as_deref()
                .map(|home| super::activation::read_acceptance_record(home, &held));
            let (scope, copied) = match &record {
                Some(Ok(_)) => {
                    let (scope, incarnation, receipt) =
                        bound.review_scope_with_receipt(&parsed.causal_identity)?;
                    (scope, Some((incarnation, receipt)))
                }
                _ => (bound.review_scope()?, None),
            };
            let record = record.ok_or("disabled")?.map_err(|_| "disabled")?;
            let (memstore_incarnation, receipt) = copied.ok_or("disabled")?;
            record
                .admits_memory_store(&memstore_incarnation)
                .map_err(|_| "disabled")?;
            // The Kernel checks its live incarnation against the one the record names before it reads anything else, so an acceptance it answers ran under an open gate. A refusal reads the live Kernel incarnation, so a closed gate answers `disabled` before any other terminal.
            accept_selected_proposal(
                &scope.kernel,
                receipt,
                (record.kernel_incarnation(), &memstore_incarnation),
                &parsed.causal_identity,
                &parsed.operation_key,
                &parsed.target,
            )
            .or_else(|refusal| {
                let kernel_incarnation = scope
                    .kernel
                    .database_incarnation_id_within_budget(&kernel::applicability::EvalBudget::new(
                        Some(std::time::Instant::now() + GATE_READ_BUDGET),
                        Arc::default(),
                    ))
                    .map_err(|_| "store_unavailable")?;
                record
                    .admits(&kernel_incarnation, &memstore_incarnation)
                    .map_err(|_| "disabled")?;
                Err(accept_terminal(&refusal))
            })
        })
        .await;
        match accepted {
            Ok(Ok(outcome)) => response(json!({
                "kind": "settled",
                "replayed": outcome.replayed,
                "settlement": outcome.settlement,
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
    use memory_store::memory_reviewer_ledger::AbstainReason;

    /// Every abstention reason a receipt can record is a literal the protocol's `review.list` item vocabulary names; a strict client rejects a page carrying an undocumented one.
    #[test]
    fn every_abstention_reason_is_a_documented_review_list_literal() {
        let wire = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/host-wire-protocol.md"
        ))
        .unwrap();
        let vocabulary = wire
            .lines()
            .find(|line| line.starts_with("`review.list` carries"))
            .expect("the review.list paragraph");
        for reason in AbstainReason::ALL {
            assert!(
                vocabulary.contains(&format!("`{}`", reason.as_str())),
                "{} is not in the review.list reason vocabulary",
                reason.as_str()
            );
        }
    }

    /// The `review.accept` terminals are exactly the literals its paragraph lists, plus the shared `disabled`.
    #[test]
    fn every_review_accept_terminal_is_a_documented_literal() {
        let wire = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/host-wire-protocol.md"
        ))
        .unwrap();
        let paragraph = wire
            .lines()
            .find(|line| line.starts_with("`review.accept` carries"))
            .expect("the review.accept paragraph");
        let listed = paragraph
            .split_once("with `t` one of")
            .expect("the terminal sentence")
            .1;
        let documented: std::collections::BTreeSet<&str> = listed
            .split('`')
            .skip(1)
            .step_by(2)
            .filter(|literal| !literal.contains(' ') && *literal != "review.read")
            .collect();
        let produced: std::collections::BTreeSet<&str> = [
            AcceptRefusal::Receipt(ReadRefusal::NotSelected),
            AcceptRefusal::Receipt(ReadRefusal::IncarnationMismatch),
            AcceptRefusal::Receipt(ReadRefusal::SelectionMismatch),
            AcceptRefusal::Receipt(ReadRefusal::Store(String::new())),
        ]
        .iter()
        .chain(
            [
                kernel::AcceptanceRefusal::Unavailable,
                kernel::AcceptanceRefusal::Expired,
                kernel::AcceptanceRefusal::Changed,
                kernel::AcceptanceRefusal::Conflict,
                kernel::AcceptanceRefusal::Invalid,
            ]
            .map(AcceptRefusal::Kernel)
            .iter(),
        )
        .map(accept_terminal)
        .collect();
        assert_eq!(documented, produced);
        assert!(paragraph.contains(r#"{"kind":"terminal","terminal":"disabled"}"#));
    }

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

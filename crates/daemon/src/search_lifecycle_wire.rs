//! The context application protocol's search lifecycle operations: the operator surface for the search projection's lifecycle record.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::HandlerCore;
use crate::dispatch::{PreparedOutcome, PreparedOutput};
use crate::kernel_routes::blocking;
use crate::projection_lifecycle::{ControlState, LifecycleIntent};
use crate::search_lifecycle_owner::SearchLifecycleOwner;
use host_runtime::RouteHandle;
use kernel::applicability::EvalBudget;

pub(crate) const STATUS: &str = "search.lifecycle.status";
pub(crate) const REBUILD: &str = "search.lifecycle.rebuild";
pub(crate) const DISABLE: &str = "search.lifecycle.disable";
pub(crate) const RECOVER: &str = "search.lifecycle.recover";
pub(crate) const ABANDON: &str = "search.lifecycle.abandon";

const OPERATION_BUDGET: std::time::Duration = std::time::Duration::from_secs(20);
const REASON_CHARS: usize = 512;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoverRequest {
    authorization_ref: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AbandonRequest {
    operator_id: String,
    reason: String,
}

fn response(body: Value) -> PreparedOutcome {
    PreparedOutcome::Response(PreparedOutput::json(body))
}

fn terminal(code: &str) -> PreparedOutcome {
    response(json!({ "kind": "terminal", "terminal": code }))
}

fn refused(reason: impl std::fmt::Display) -> PreparedOutcome {
    let reason = reason.to_string();
    let end = reason
        .char_indices()
        .map(|(index, _)| index)
        .nth(REASON_CHARS)
        .unwrap_or(reason.len());
    response(json!({ "kind": "terminal", "terminal": "refused", "reason": &reason[..end] }))
}

fn budget() -> EvalBudget {
    EvalBudget::new(
        Some(std::time::Instant::now() + OPERATION_BUDGET),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
}

fn operation(intent: &LifecycleIntent, state: &str) -> Value {
    json!({
        "state": state,
        "transition": intent.transition,
        "cause": intent.cause,
        "attempt_id": intent.attempt_id,
        "consumer_id": intent.consumer.consumer_id,
        "generation_id": intent.consumer.generation_id,
        "recovery_target": intent.recovery_target.map(|target| target.commit_seq),
        "episodes": {
            "allowance": intent.episodes.allowance,
            "consumed": intent.episodes.consumed,
            "deadline": intent.episodes.deadline,
        },
    })
}

fn record_json(record: &ControlState) -> Value {
    match record {
        ControlState::Absent => json!({ "state": "absent" }),
        ControlState::Intent(intent) => operation(intent, "intent"),
        ControlState::Current(intent) => operation(intent, "current"),
        ControlState::Disabled(disabled) => json!({
            "state": "disabled",
            "consumer_id": disabled.handoff.as_deref().map(|handoff| &handoff.consumer.consumer_id),
            "through": disabled.through,
            "deregistered": disabled.deregistered,
        }),
        ControlState::Unavailable(reason) => json!({ "state": "unavailable", "reason": reason }),
    }
}

impl HandlerCore {
    fn bind_search_lifecycle<T: serde::de::DeserializeOwned>(
        &self,
        channel: RouteHandle,
        request: Value,
        operation: &str,
    ) -> Result<(Arc<SearchLifecycleOwner>, T), PreparedOutcome> {
        let (_, parsed) = self.kernel_request::<T>(channel, request, operation)?;
        let owner = self
            .lifecycle_owner()
            .ok_or_else(|| terminal("unavailable"))?;
        Ok((owner, parsed))
    }

    pub(crate) fn handle_search_lifecycle_status(
        &self,
        channel: RouteHandle,
        request: Value,
    ) -> PreparedOutcome {
        let (owner, Empty {}) = match self.bind_search_lifecycle(channel, request, STATUS) {
            Ok(bound) => bound,
            Err(outcome) => return outcome,
        };
        let admission = match owner.admission_state() {
            Ok(()) => json!({ "state": "admitted" }),
            Err(reason) => json!({ "state": "refused", "reason": reason }),
        };
        response(json!({
            "kind": "status",
            "record": record_json(&owner.lifecycle_record()),
            "admission": admission,
        }))
    }

    pub(crate) async fn handle_search_lifecycle_rebuild(
        &self,
        channel: RouteHandle,
        request: Value,
    ) -> PreparedOutcome {
        let (owner, Empty {}) = match self.bind_search_lifecycle(channel, request, REBUILD) {
            Ok(bound) => bound,
            Err(outcome) => return outcome,
        };
        match blocking(move || owner.request_operator_rebuild(crate::now_ms(), &budget())).await {
            Ok(Ok(Some(recorded))) => response(json!({
                "kind": "recorded",
                "attempt_id": recorded.intent.attempt_id,
                "replayed": recorded.replayed,
            })),
            Ok(Ok(None)) => terminal("not_current"),
            Ok(Err(error)) => refused(error),
            Err(_) => terminal("unavailable"),
        }
    }

    pub(crate) async fn handle_search_lifecycle_disable(
        &self,
        channel: RouteHandle,
        request: Value,
    ) -> PreparedOutcome {
        let (owner, Empty {}) = match self.bind_search_lifecycle(channel, request, DISABLE) {
            Ok(bound) => bound,
            Err(outcome) => return outcome,
        };
        match owner.disable(&budget(), &mut |_| {}).await {
            Ok(()) => response(json!({
                "kind": "disabled",
                "record": record_json(&owner.lifecycle_record()),
            })),
            Err(error) => refused(error),
        }
    }

    pub(crate) async fn handle_search_lifecycle_recover(
        &self,
        channel: RouteHandle,
        request: Value,
    ) -> PreparedOutcome {
        let (owner, RecoverRequest { authorization_ref }) =
            match self.bind_search_lifecycle(channel, request, RECOVER) {
                Ok(bound) => bound,
                Err(outcome) => return outcome,
            };
        match blocking(move || {
            owner.request_authorized_recovery(&authorization_ref, crate::now_ms(), &budget())
        })
        .await
        {
            Ok(Ok(Some(recorded))) => response(json!({
                "kind": "recorded",
                "attempt_id": recorded.intent.attempt_id,
                "replayed": recorded.replayed,
            })),
            Ok(Ok(None)) => terminal("not_disabled"),
            Ok(Err(error)) => refused(error),
            Err(_) => terminal("unavailable"),
        }
    }

    pub(crate) async fn handle_search_lifecycle_abandon(
        &self,
        channel: RouteHandle,
        request: Value,
    ) -> PreparedOutcome {
        let (
            owner,
            AbandonRequest {
                operator_id,
                reason,
            },
        ) = match self.bind_search_lifecycle(channel, request, ABANDON) {
            Ok(bound) => bound,
            Err(outcome) => return outcome,
        };
        let abandoner = Arc::clone(&owner);
        let budget = budget();
        let abandon_budget = budget.clone();
        let abandoned = blocking(move || {
            abandoner.abandon_disabled_consumer(
                &operator_id,
                &reason,
                crate::now_ms(),
                &abandon_budget,
            )
        })
        .await;
        match abandoned {
            Ok(Ok(Some(consumer))) => match owner.disable(&budget, &mut |_| {}).await {
                Ok(()) => response(json!({
                    "kind": "abandoned",
                    "consumer_id": consumer,
                    "record": record_json(&owner.lifecycle_record()),
                })),
                Err(error) => refused(error),
            },
            Ok(Ok(None)) => terminal("nothing_to_abandon"),
            Ok(Err(error)) => refused(error),
            Err(_) => terminal("unavailable"),
        }
    }
}

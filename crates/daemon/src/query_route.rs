//! Payload bytes are never read here; packing consumes the ranking and materializes payloads under its own bounds.

use std::collections::BTreeSet;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use host_runtime::RouteHandle;
use kernel::source_identity::OCCURRENCE_ENCODING_VERSION;
use kernel::{ArtifactDestination, KernelError, KernelStore, ProjectScope};
use retrieval::eligibility::{Disposition, judge_occurrences_within_budget, live_candidates_by_id};
use retrieval::exact::{
    ExactQuery, Family, Intent, LookupContext, LookupRefusal, Selector, SelectorBounds,
    SelectorValue, classify, page,
};
use retrieval::fusion::{
    DeclaredLanes, Fused, FusedEntry, FusionParameters, Lane, LaneHit, LaneRanking, OccurrenceId,
    RawScore, fuse,
};
use retrieval::lexical::{
    Authority, Completion, IncompleteReason, LexicalBounds, RetrievalBounds, RetrievalRefusal,
    analyze_segments, compile, retrieve,
};
use serde::Deserialize;
use serde_json::{Value, json};
use storage::{GuardedConn, StoreError};

use crate::dispatch::{PreparedOutcome, PreparedOutput, measure_json};
use crate::kernel_routes::RouteScope;
use crate::request_budget::{
    BlockingFailure, BudgetRefusal, Exhaustion, RequestBudget, SharedBudget,
};
use crate::search_projection::{SearchProjection, SearchProjectionError};
use crate::transform_unit::{UnitOutcome, UnitRunner};
use crate::{HandlerCore, invalid_params_error};

pub(crate) const OPERATION: &str = "retrieval.query";

#[derive(Debug, Clone)]
pub struct QueryRouteLimits {
    pub query_bytes: NonZeroUsize,
    pub probes: NonZeroUsize,
    pub lexical_scan_rows: NonZeroUsize,
    pub lexical_accepted: NonZeroUsize,
    pub validation_batch: NonZeroUsize,
    pub exact_page_rows: NonZeroUsize,
    pub exact_pages: NonZeroUsize,
    pub fused_union: NonZeroUsize,
    pub result_rows: NonZeroUsize,
    pub response_bytes: NonZeroUsize,
    pub deadline_ceiling: Duration,
    pub fusion: FusionParameters,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QueryRequest {
    query: String,
    remaining_ms: Option<u64>,
    destination: String,
    harness: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Terminal {
    Unauthorized,
    Deadline,
    Cancelled,
    LaneUnavailable,
    RequiredContextFailure,
    Disabled,
}

impl Terminal {
    pub fn code(self) -> &'static str {
        match self {
            Self::Unauthorized => "unauthorized",
            Self::Deadline => "deadline",
            Self::Cancelled => "cancelled",
            Self::LaneUnavailable => "lane_unavailable",
            Self::RequiredContextFailure => "required_context_failure",
            Self::Disabled => "disabled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryFailure {
    Terminal(Terminal),
    InvalidQuery(String),
}

impl From<Terminal> for QueryFailure {
    fn from(terminal: Terminal) -> Self {
        Self::Terminal(terminal)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaneStatus {
    Complete,
    Incomplete(&'static str),
    Unavailable(String),
    Undeclared,
}

impl LaneStatus {
    fn json(&self) -> Value {
        match self {
            Self::Complete => json!({ "status": "complete" }),
            Self::Incomplete(reason) => json!({ "status": "incomplete", "reason": reason }),
            Self::Unavailable(reason) => json!({ "status": "unavailable", "reason": reason }),
            Self::Undeclared => json!({ "status": "undeclared" }),
        }
    }

    fn degrades(&self) -> bool {
        matches!(self, Self::Incomplete(_) | Self::Unavailable(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Probes,
    Exact,
    Lexical,
    Fusion,
    Revalidation,
    Materialization,
    Response,
}

pub struct QueryOutcome {
    pub statuses: [LaneStatus; Lane::ORDER.len()],
    pub fused: Fused,
    pub truncated: bool,
    pub body: Value,
}

struct LaneOutput {
    ranking: Option<LaneRanking>,
    status: LaneStatus,
}

impl LaneOutput {
    fn unavailable(reason: impl ToString) -> Self {
        Self {
            ranking: None,
            status: LaneStatus::Unavailable(reason.to_string()),
        }
    }

    fn undeclared() -> Self {
        Self {
            ranking: None,
            status: LaneStatus::Undeclared,
        }
    }

    fn ranked(lane: Lane, hits: Vec<LaneHit>, status: LaneStatus) -> Self {
        match LaneRanking::consolidate(lane, OCCURRENCE_ENCODING_VERSION, hits) {
            Ok(ranking) => Self {
                ranking: Some(ranking),
                status,
            },
            Err(refusal) => Self::unavailable(refusal),
        }
    }
}

fn exhaustion(budget: &SharedBudget) -> Terminal {
    match budget.exhaustion() {
        Some(Exhaustion::Cancelled) => Terminal::Cancelled,
        Some(Exhaustion::Deadline) | None => Terminal::Deadline,
    }
}

fn check(budget: &SharedBudget) -> Result<(), Terminal> {
    if budget.is_exhausted() {
        return Err(exhaustion(budget));
    }
    Ok(())
}

fn hit(occurrence_id: &str, raw_score: RawScore) -> Option<LaneHit> {
    OccurrenceId::parse(occurrence_id)
        .ok()
        .map(|occurrence| LaneHit {
            occurrence,
            raw_score,
        })
}

fn exact_lane(
    conn: &GuardedConn<'_>,
    kernel_incarnation_id: &str,
    intent: &Intent,
    limits: &QueryRouteLimits,
    budget: &SharedBudget,
) -> Result<LaneOutput, Terminal> {
    let selectors: Vec<&Selector> = match intent {
        Intent::Direct(selector) => vec![selector],
        Intent::Hybrid(mentions) => mentions.iter().map(|mention| &mention.selector).collect(),
    };
    let object_ids: Vec<&str> = selectors
        .iter()
        .filter(|selector| selector.family == Family::Id)
        .filter_map(|selector| match &selector.value {
            SelectorValue::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    if object_ids.is_empty() {
        return Ok(LaneOutput::undeclared());
    }
    let context = LookupContext {
        kernel_incarnation_id,
        page_rows: limits.exact_page_rows,
        budget: budget.eval(),
    };
    let mut hits = Vec::new();
    let mut status = LaneStatus::Complete;
    for object_id in object_ids {
        let query = ExactQuery::CanonicalObject(object_id.as_bytes());
        let mut cursor = None;
        for _ in 0..limits.exact_pages.get() {
            let page = match page(conn, &context, &query, cursor.as_ref()) {
                Ok(page) => page,
                Err(LookupRefusal::BudgetExhausted) => return Err(exhaustion(budget)),
                Err(refusal) => return Ok(LaneOutput::unavailable(refusal)),
            };
            hits.extend(
                page.rows
                    .iter()
                    .filter(|row| row.tombstone.is_none())
                    .filter_map(|row| hit(&row.occurrence_id, RawScore::Exact)),
            );
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        if cursor.is_some() {
            status = LaneStatus::Incomplete("page_bound");
        }
    }
    Ok(LaneOutput::ranked(Lane::Exact, hits, status))
}

fn lexical_lane(
    conn: &GuardedConn<'_>,
    kernel: &KernelStore,
    authority: Authority<'_>,
    intent: &Intent,
    query: &str,
    limits: &QueryRouteLimits,
    budget: &SharedBudget,
) -> Result<LaneOutput, Terminal> {
    let segments = match intent {
        Intent::Direct(_) => Vec::new(),
        Intent::Hybrid(_) => intent.lexical_segments(query),
    };
    let bounds = LexicalBounds {
        max_input_bytes: limits.query_bytes,
        max_atoms: limits.probes,
    };
    let analysis = match analyze_segments(&segments, bounds) {
        Ok(analysis) => analysis,
        Err(refusal) => return Ok(LaneOutput::unavailable(refusal)),
    };
    if analysis.is_empty() {
        return Ok(LaneOutput::undeclared());
    }
    let probes = compile(&analysis);
    let bounds = RetrievalBounds {
        max_probes: limits.probes,
        scan_rows: limits.lexical_scan_rows,
        max_accepted: limits.lexical_accepted,
        batch_rows: limits.validation_batch,
    };
    let retrieval = match retrieve(conn, kernel, &probes, authority, bounds, budget.eval()) {
        Ok(retrieval) => retrieval,
        Err(RetrievalRefusal::BudgetExhausted) => return Err(exhaustion(budget)),
        Err(refusal) => return Ok(LaneOutput::unavailable(refusal)),
    };
    let status = match retrieval.completion {
        Completion::Complete | Completion::Empty => LaneStatus::Complete,
        Completion::Incomplete(IncompleteReason::BudgetExhausted) => {
            return Err(exhaustion(budget));
        }
        Completion::Incomplete(IncompleteReason::ScanBound) => LaneStatus::Incomplete("scan_bound"),
        Completion::Incomplete(IncompleteReason::AcceptedBound) => {
            LaneStatus::Incomplete("accepted_bound")
        }
        Completion::Incomplete(IncompleteReason::KernelIncarnationChanged) => {
            return Ok(LaneOutput::unavailable("kernel_incarnation_changed"));
        }
        Completion::Incomplete(IncompleteReason::SnapshotChanged) => {
            return Ok(LaneOutput::unavailable("snapshot_changed"));
        }
    };
    let hits = retrieval
        .contributions
        .iter()
        .filter_map(|c| hit(&c.occurrence_id, RawScore::Lexical(c.rank)))
        .collect();
    Ok(LaneOutput::ranked(Lane::Lexical, hits, status))
}

struct Scanned {
    statuses: [LaneStatus; Lane::ORDER.len()],
    fused: Fused,
    candidates: Vec<retrieval::eligibility::OccurrenceCandidate>,
}

#[allow(clippy::too_many_arguments)]
pub fn execute(
    projection: &SearchProjection,
    kernel: &KernelStore,
    project: &ProjectScope,
    destination: ArtifactDestination,
    limits: &QueryRouteLimits,
    budget: &SharedBudget,
    query: &str,
    mut before_phase: impl FnMut(Phase),
) -> Result<QueryOutcome, QueryFailure> {
    before_phase(Phase::Probes);
    check(budget)?;
    let bounds = SelectorBounds {
        max_input_bytes: limits.query_bytes,
        max_value_bytes: limits.query_bytes,
    };
    let intent = classify(query, bounds)
        .map_err(|refusal| QueryFailure::InvalidQuery(refusal.to_string()))?;

    let read = projection.read_under(budget, |conn| {
        let Some(identity) = retrieval::read_identity(conn)? else {
            return Ok(Err(QueryFailure::Terminal(Terminal::LaneUnavailable)));
        };
        let mut phases = || -> Result<Scanned, QueryFailure> {
            before_phase(Phase::Exact);
            check(budget)?;
            let exact = exact_lane(
                conn,
                &identity.kernel_incarnation_id,
                &intent,
                limits,
                budget,
            )?;
            before_phase(Phase::Lexical);
            check(budget)?;
            let authority = Authority {
                project,
                destination,
            };
            let lexical = lexical_lane(conn, kernel, authority, &intent, query, limits, budget)?;
            if exact.ranking.is_none() && lexical.ranking.is_none() {
                let both_undeclared = exact.status == LaneStatus::Undeclared
                    && lexical.status == LaneStatus::Undeclared;
                return Err(if both_undeclared {
                    QueryFailure::InvalidQuery("the query yields no probe".to_string())
                } else {
                    Terminal::LaneUnavailable.into()
                });
            }
            before_phase(Phase::Fusion);
            check(budget)?;
            let rankings = [exact.ranking, lexical.ranking].into_iter().flatten();
            let lanes = DeclaredLanes::admit(rankings).map_err(|_| Terminal::LaneUnavailable)?;
            let fused = fuse(lanes, &limits.fusion, limits.fused_union)
                .map_err(|_| Terminal::LaneUnavailable)?;
            before_phase(Phase::Revalidation);
            check(budget)?;
            let ids: Vec<String> = fused
                .entries()
                .iter()
                .map(|entry| entry.occurrence().to_string())
                .collect();
            let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
            let candidates =
                live_candidates_by_id(conn, &id_refs).map_err(|_| Terminal::LaneUnavailable)?;
            Ok(Scanned {
                statuses: [exact.status, lexical.status, LaneStatus::Undeclared],
                fused,
                candidates,
            })
        };
        Ok(phases())
    });
    let scanned = match read {
        Ok(result) => result?,
        Err(SearchProjectionError::Store(StoreError::Deadline)) => {
            return Err(exhaustion(budget).into());
        }
        Err(_) => return Err(Terminal::LaneUnavailable.into()),
    };

    let eligible: BTreeSet<String> = if scanned.candidates.is_empty() {
        BTreeSet::new()
    } else {
        let report = judge_occurrences_within_budget(
            kernel,
            project,
            destination,
            &scanned.candidates,
            budget.eval(),
        )
        .map_err(|error| match error {
            KernelError::Deadline => exhaustion(budget),
            _ => Terminal::LaneUnavailable,
        })?;
        report
            .occurrences
            .into_iter()
            .filter(|judged| judged.disposition == Disposition::Eligible)
            .map(|judged| judged.occurrence_id)
            .collect()
    };
    let fused = scanned
        .fused
        .filter(|entry| eligible.contains(&entry.occurrence().to_string()));

    before_phase(Phase::Materialization);
    check(budget)?;
    let statuses = scanned.statuses;
    let degraded = statuses.iter().any(LaneStatus::degrades);
    let envelope = |entries: Vec<Value>, truncated: bool| {
        json!({
            "kind": "fused",
            "degraded": degraded,
            "lanes": lanes_json(&statuses),
            "truncated": truncated,
            "entries": entries,
        })
    };
    let mut used =
        measure_json(&envelope(Vec::new(), true)).map_err(|_| Terminal::LaneUnavailable)?;
    let mut entries = Vec::new();
    let mut truncated = false;
    for entry in fused.entries() {
        if entries.len() >= limits.result_rows.get() {
            truncated = true;
            break;
        }
        let value = entry_json(entry);
        let separator = 1;
        let size = measure_json(&value).map_err(|_| Terminal::LaneUnavailable)? + separator;
        if used + size > limits.response_bytes.get() {
            truncated = true;
            break;
        }
        used += size;
        entries.push(value);
    }

    before_phase(Phase::Response);
    check(budget)?;
    let body = envelope(entries, truncated);
    Ok(QueryOutcome {
        statuses,
        fused,
        truncated,
        body,
    })
}

fn lanes_json(statuses: &[LaneStatus; Lane::ORDER.len()]) -> Value {
    let mut lanes = serde_json::Map::new();
    for (lane, status) in Lane::ORDER.into_iter().zip(statuses) {
        lanes.insert(lane.code().to_string(), status.json());
    }
    Value::Object(lanes)
}

fn entry_json(entry: &FusedEntry) -> Value {
    let mut lanes = serde_json::Map::new();
    for lane in Lane::ORDER {
        if let Some(contribution) = entry.lane(lane) {
            let raw = match contribution.raw_score() {
                RawScore::Exact => Value::Null,
                RawScore::Lexical(value) | RawScore::Dense(value) => json!(value),
            };
            lanes.insert(
                lane.code().to_string(),
                json!({ "position": contribution.position().get(), "raw": raw }),
            );
        }
    }
    json!({
        "occurrence_id": entry.occurrence().to_string(),
        "position": entry.position().get(),
        "score": entry.score(),
        "lanes": lanes,
    })
}

pub(crate) fn terminal_response(terminal: Terminal) -> PreparedOutcome {
    PreparedOutcome::Response(PreparedOutput::json(json!({
        "kind": "terminal",
        "terminal": terminal.code(),
    })))
}

fn parse_destination(value: &str) -> Option<ArtifactDestination> {
    match value {
        "local" => Some(ArtifactDestination::Local),
        "remote" => Some(ArtifactDestination::Remote),
        _ => None,
    }
}

impl HandlerCore {
    pub fn set_query_route_limits(&self, limits: Option<QueryRouteLimits>) {
        *self
            .query_route
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = limits.map(Arc::new);
    }

    pub(crate) async fn handle_retrieval_query(
        &self,
        channel: RouteHandle,
        request: Value,
        runner: &dyn UnitRunner,
    ) -> PreparedOutcome {
        let (scope, parsed) = match self.kernel_request::<QueryRequest>(channel, request, OPERATION)
        {
            Ok(bound) => bound,
            Err(outcome) => return outcome,
        };
        let Some(bound_harness) = self.harness_for_route(channel) else {
            return terminal_response(Terminal::Unauthorized);
        };
        if parsed
            .harness
            .as_deref()
            .is_some_and(|claimed| claimed != bound_harness)
        {
            return terminal_response(Terminal::Unauthorized);
        }
        let Some(limits) = self
            .query_route
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        else {
            return terminal_response(Terminal::Disabled);
        };
        if parsed.query.len() > limits.query_bytes.get() {
            return invalid_params_error(format!(
                "{OPERATION} query exceeds {} bytes",
                limits.query_bytes
            ));
        }
        let Some(destination) = parse_destination(&parsed.destination) else {
            return invalid_params_error(format!(
                "{OPERATION} destination must be local or remote"
            ));
        };
        let budget = match RequestBudget::derive(
            runner.cancel_signal(),
            parsed.remaining_ms,
            Some(limits.deadline_ceiling),
        ) {
            Ok(budget) => budget,
            Err(BudgetRefusal::Cancelled) => return terminal_response(Terminal::Cancelled),
            Err(refusal) => return invalid_params_error(format!("{OPERATION}: {refusal}")),
        };
        let Some(lifecycle) = self.lifecycle_owner() else {
            return terminal_response(Terminal::LaneUnavailable);
        };
        let shared = budget.shared().clone();
        let RouteScope { store, project } = scope;
        let query = parsed.query;
        let work = runner.run_unit(Box::new(move || {
            let reader = match lifecycle.pin(shared.eval()) {
                Ok(reader) => reader,
                Err(_) if shared.is_exhausted() => {
                    return UnitOutcome::Terminal(terminal_response(exhaustion(&shared)));
                }
                Err(_) => {
                    return UnitOutcome::Terminal(terminal_response(Terminal::LaneUnavailable));
                }
            };
            let outcome = execute(
                reader.projection(),
                &store,
                project.scope(),
                destination,
                &limits,
                &shared,
                &query,
                |_| {},
            );
            UnitOutcome::Terminal(match outcome {
                Ok(outcome) => PreparedOutcome::Response(PreparedOutput::json(outcome.body)),
                Err(QueryFailure::Terminal(terminal)) => terminal_response(terminal),
                Err(QueryFailure::InvalidQuery(reason)) => {
                    invalid_params_error(format!("{OPERATION}: {reason}"))
                }
            })
        }));
        let outcome = work.await;
        drop(budget);
        match outcome {
            Ok(UnitOutcome::Terminal(outcome)) => outcome,
            Ok(UnitOutcome::Continue(_)) => terminal_response(Terminal::LaneUnavailable),
            Err(failed) => match BlockingFailure::from(failed) {
                BlockingFailure::Panicked => PreparedOutcome::Error {
                    code: "internal_error".to_string(),
                    message: "query work failed".to_string(),
                },
                BlockingFailure::RuntimeStopped | BlockingFailure::RouteClosing => {
                    terminal_response(Terminal::Cancelled)
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_terminal_has_one_wire_code_and_the_response_names_it() {
        let all = [
            Terminal::Unauthorized,
            Terminal::Deadline,
            Terminal::Cancelled,
            Terminal::LaneUnavailable,
            Terminal::RequiredContextFailure,
            Terminal::Disabled,
        ];
        let codes: BTreeSet<&str> = all.iter().map(|terminal| terminal.code()).collect();
        assert_eq!(codes.len(), all.len());
        assert_eq!(
            Terminal::RequiredContextFailure.code(),
            "required_context_failure"
        );
        for terminal in all {
            let PreparedOutcome::Response(output) = terminal_response(terminal) else {
                panic!("a terminal is a response, not a transport error");
            };
            let body = output.json_for_test().unwrap();
            assert_eq!(body["kind"], "terminal");
            assert_eq!(body["terminal"], terminal.code());
        }
    }

    #[test]
    fn lane_statuses_serialize_their_reason_and_only_incomplete_or_unavailable_degrade() {
        assert_eq!(LaneStatus::Complete.json(), json!({"status": "complete"}));
        assert_eq!(
            LaneStatus::Incomplete("scan_bound").json(),
            json!({"status": "incomplete", "reason": "scan_bound"})
        );
        assert_eq!(
            LaneStatus::Unavailable("snapshot_changed".to_string()).json(),
            json!({"status": "unavailable", "reason": "snapshot_changed"})
        );
        assert_eq!(
            LaneStatus::Undeclared.json(),
            json!({"status": "undeclared"})
        );
        assert!(!LaneStatus::Complete.degrades());
        assert!(!LaneStatus::Undeclared.degrades());
        assert!(LaneStatus::Incomplete("page_bound").degrades());
        assert!(LaneStatus::Unavailable(String::new()).degrades());
    }
}

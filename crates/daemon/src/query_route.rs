//! Payload bytes are never read here; packing consumes the ranking and materializes payloads under its own bounds.

use std::collections::BTreeSet;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use host_runtime::RouteHandle;
use host_runtime::local_embeddings::inference::InferenceError;
use host_runtime::local_embeddings::{LocalEmbeddingsComponent, LocalEmbeddingsStatus};
use kernel::applicability::EvalBudget;
use kernel::source_identity::OCCURRENCE_ENCODING_VERSION;
use kernel::{ArtifactDestination, KernelError, KernelStore, MAX_ELIGIBILITY_CANDIDATES};
use retrieval::ProjectionError;
use retrieval::batch::VectorGeneration;
use retrieval::dense::{
    Completion as DenseCompletion, ExhaustiveQuery, IncompleteReason as DenseIncompleteReason,
    Metric, OracleBounds, OracleRefusal, exhaustive,
};
use retrieval::eligibility::{
    Authority, Disposition, judge_occurrences_within_budget, live_candidates_by_id,
};
use retrieval::exact::{
    ExactQuery, Family, Intent, LookupContext, LookupRefusal, Selector, SelectorBounds,
    SelectorValue, classify, page,
};
use retrieval::fusion::IdentityRefusal;
use retrieval::fusion::{
    DeclaredLanes, Fused, FusedEntry, FusionParameters, Lane, LaneHit, LaneRanking, OccurrenceId,
    RawScore, fuse,
};
use retrieval::lexical::{
    Completion, IncompleteReason, LexicalBounds, LexicalRefusal, RetrievalBounds, RetrievalRefusal,
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
use crate::search_lifecycle_owner::SearchLifecycleOwner;
use crate::search_projection::{SearchProjection, SearchProjectionError};
use crate::transform_unit::{UnitOutcome, UnitRunner};
use crate::{HandlerCore, invalid_params_error};

pub(crate) const OPERATION: &str = "retrieval.query";

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LimitsRefusal {
    #[error(
        "validation_batch {value} exceeds the kernel's {MAX_ELIGIBILITY_CANDIDATES} candidate batch"
    )]
    ValidationBatch { value: usize },
    #[error(
        "lexical_accepted {value} exceeds the kernel's {MAX_ELIGIBILITY_CANDIDATES} candidate batch"
    )]
    LexicalAccepted { value: usize },
    #[error(
        "dense {bound} {value} exceeds the kernel's {MAX_ELIGIBILITY_CANDIDATES} candidate batch"
    )]
    Dense { bound: &'static str, value: usize },
}

/// An absent dense limit set leaves the dense lane undeclared; RP2.9 approves the values before it is declared.
#[derive(Debug, Clone, Copy)]
pub struct DenseLimits {
    pub k: NonZeroUsize,
    pub page_rows: NonZeroUsize,
    pub max_rows: NonZeroUsize,
    pub unit_norm_tolerance: f64,
}

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
    pub dense: Option<DenseLimits>,
}

impl QueryRouteLimits {
    /// Refuses a limit the kernel's eligibility batch could never serve, so the refusal lands at installation instead of on every request.
    pub fn validate(&self) -> Result<(), LimitsRefusal> {
        if self.validation_batch.get() > MAX_ELIGIBILITY_CANDIDATES {
            return Err(LimitsRefusal::ValidationBatch {
                value: self.validation_batch.get(),
            });
        }
        if self.lexical_accepted.get() > MAX_ELIGIBILITY_CANDIDATES {
            return Err(LimitsRefusal::LexicalAccepted {
                value: self.lexical_accepted.get(),
            });
        }
        if let Some(dense) = &self.dense {
            for (bound, value) in [("k", dense.k.get()), ("page_rows", dense.page_rows.get())] {
                if value > MAX_ELIGIBILITY_CANDIDATES {
                    return Err(LimitsRefusal::Dense { bound, value });
                }
            }
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QueryRequest {
    query: String,
    remaining_ms: Option<u64>,
    destination: Destination,
    harness: Option<String>,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum Destination {
    Local,
    Remote,
}

impl From<Destination> for ArtifactDestination {
    fn from(destination: Destination) -> Self {
        match destination {
            Destination::Local => Self::Local,
            Destination::Remote => Self::Remote,
        }
    }
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
    /// `lane_unavailable` with the witness that produced it.
    Unavailable(&'static str),
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
    Unavailable(&'static str),
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
    Dense,
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
    fn unavailable(reason: &'static str) -> Self {
        Self {
            ranking: None,
            status: LaneStatus::Unavailable(reason),
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
            Err(refusal) => Self::unavailable(identity_reason(&refusal)),
        }
    }
}

/// Engine text never reaches the wire; each refusal maps to one code.
fn projection_reason(error: &ProjectionError) -> &'static str {
    match error {
        ProjectionError::Interrupted => "interrupted",
        ProjectionError::Sqlite(_) => "engine",
        _ => "projection",
    }
}

fn identity_reason(refusal: &IdentityRefusal) -> &'static str {
    match refusal {
        IdentityRefusal::EncodingVersion { .. } => "encoding_version",
        IdentityRefusal::ScoreLaneMismatch { .. } => "score_lane",
        IdentityRefusal::NonFiniteScore => "non_finite_score",
        _ => "identity",
    }
}

enum LaneRefusal {
    Budget,
    Reason(&'static str),
}

fn lookup_refusal(refusal: &LookupRefusal) -> LaneRefusal {
    match refusal {
        LookupRefusal::BudgetExhausted
        | LookupRefusal::Projection(ProjectionError::Interrupted) => LaneRefusal::Budget,
        LookupRefusal::NoCheckpoint => LaneRefusal::Reason("no_checkpoint"),
        LookupRefusal::StaleCursor { .. } => LaneRefusal::Reason("stale_cursor"),
        LookupRefusal::ForeignCursor => LaneRefusal::Reason("foreign_cursor"),
        LookupRefusal::ExtractionMismatch { .. } => LaneRefusal::Reason("extraction_version"),
        LookupRefusal::Projection(error) => LaneRefusal::Reason(projection_reason(error)),
    }
}

fn retrieval_refusal(refusal: &RetrievalRefusal) -> LaneRefusal {
    match refusal {
        RetrievalRefusal::BudgetExhausted
        | RetrievalRefusal::Projection(ProjectionError::Interrupted)
        | RetrievalRefusal::Kernel(KernelError::Deadline) => LaneRefusal::Budget,
        RetrievalRefusal::ProbesOverBound { .. } => LaneRefusal::Reason("probes_over_bound"),
        RetrievalRefusal::BatchOverBound { .. } => LaneRefusal::Reason("batch_over_bound"),
        RetrievalRefusal::Projection(error) => LaneRefusal::Reason(projection_reason(error)),
        RetrievalRefusal::Kernel(_) => LaneRefusal::Reason("kernel"),
    }
}

fn lexical_refusal(refusal: &LexicalRefusal) -> &'static str {
    match refusal {
        LexicalRefusal::InputTooLong { .. } => "input_too_long",
        LexicalRefusal::TooManyAtoms { .. } => "too_many_atoms",
    }
}

pub struct DenseRequest<'a> {
    pub generation: &'a VectorGeneration,
    pub query: &'a [f32],
    pub authority: Authority<'a>,
    pub budget: &'a EvalBudget,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DenseRanking {
    pub hits: Vec<(String, f64)>,
    pub status: LaneStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenseRefusal {
    Budget,
    /// The query vector is not a member of the generation, so the lane cannot run and the answer degrades.
    QueryShape,
    /// A stored vector failed the generation's layout; the request ends typed rather than serving a ranking over corrupt rows.
    Corruption,
    Unavailable(&'static str),
}

/// One ranking producer behind the dense lane; the exhaustive f32 oracle is the first.
pub trait DenseProducer: Send + Sync {
    fn rank(
        &self,
        conn: &GuardedConn<'_>,
        kernel: &KernelStore,
        request: DenseRequest<'_>,
    ) -> Result<DenseRanking, DenseRefusal>;
}

pub struct ExhaustiveProducer {
    pub limits: DenseLimits,
}

impl DenseProducer for ExhaustiveProducer {
    fn rank(
        &self,
        conn: &GuardedConn<'_>,
        kernel: &KernelStore,
        request: DenseRequest<'_>,
    ) -> Result<DenseRanking, DenseRefusal> {
        let query = ExhaustiveQuery {
            generation: request.generation,
            metric: Metric::InnerProduct,
            unit_norm_tolerance: self.limits.unit_norm_tolerance,
            query: request.query,
            authority: request.authority,
            bounds: OracleBounds {
                k: self.limits.k,
                page_rows: self.limits.page_rows,
                max_rows: self.limits.max_rows,
            },
        };
        let ranking =
            exhaustive(conn, kernel, &query, request.budget).map_err(|refusal| match refusal {
                OracleRefusal::BudgetExhausted
                | OracleRefusal::Projection(ProjectionError::Interrupted)
                | OracleRefusal::Kernel(KernelError::Deadline) => DenseRefusal::Budget,
                OracleRefusal::Query(_) => DenseRefusal::QueryShape,
                OracleRefusal::StoredRow { .. } | OracleRefusal::Unreadable { .. } => {
                    DenseRefusal::Corruption
                }
                OracleRefusal::BatchOverBound { .. } => {
                    DenseRefusal::Unavailable("batch_over_bound")
                }
                OracleRefusal::Projection(error) => {
                    DenseRefusal::Unavailable(projection_reason(&error))
                }
                OracleRefusal::Kernel(_) => DenseRefusal::Unavailable("kernel"),
            })?;
        let status = match ranking.completion {
            DenseCompletion::Complete => LaneStatus::Complete,
            DenseCompletion::Incomplete(DenseIncompleteReason::BudgetExhausted) => {
                return Err(DenseRefusal::Budget);
            }
            DenseCompletion::Incomplete(DenseIncompleteReason::DenseCoverageShortfall) => {
                LaneStatus::Incomplete("coverage_shortfall")
            }
            DenseCompletion::Incomplete(DenseIncompleteReason::RowBound) => {
                LaneStatus::Incomplete("row_bound")
            }
            DenseCompletion::Incomplete(DenseIncompleteReason::KernelIncarnationChanged) => {
                return Err(DenseRefusal::Unavailable("kernel_incarnation_changed"));
            }
            DenseCompletion::Incomplete(DenseIncompleteReason::SnapshotChanged) => {
                return Err(DenseRefusal::Unavailable("snapshot_changed"));
            }
        };
        Ok(DenseRanking {
            hits: ranking
                .ranked
                .into_iter()
                .map(|row| (row.occurrence_id, row.score))
                .collect(),
            status,
        })
    }
}

/// What the route learned about the dense lane before the blocking scan was submitted.
pub enum DenseLane<'a> {
    Undeclared,
    /// The embedding lane produced no query vector; the answer degrades to the other lanes.
    Unavailable(&'static str),
    Ready {
        query: &'a [f32],
        generation_id: &'a str,
        producer: &'a dyn DenseProducer,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedFailure {
    /// The lane is starting, disabled, failing, or busy, or it refused the input; the dense lane degrades.
    Unavailable(&'static str),
    /// Inference ran and failed or broke an invariant; the request ends typed.
    Faulted,
}

pub trait QueryEmbedder: Send + Sync {
    fn embed(&self, text: &str) -> EmbedResult;
}

/// Parent Q5: a busy lane degrades at once; the route does not retry inside the deadline.
impl QueryEmbedder for LocalEmbeddingsComponent {
    fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedFailure> {
        match self.status() {
            LocalEmbeddingsStatus::Ready(_) => {}
            LocalEmbeddingsStatus::Starting => return Err(EmbedFailure::Unavailable("starting")),
            LocalEmbeddingsStatus::Disabled { .. } => {
                return Err(EmbedFailure::Unavailable("disabled"));
            }
            LocalEmbeddingsStatus::Failing { .. } => {
                return Err(EmbedFailure::Unavailable("failing"));
            }
        }
        match self.embed_blocking(&[text]) {
            Ok(mut vectors) if vectors.len() == 1 => Ok(vectors.swap_remove(0)),
            Ok(_) => Err(EmbedFailure::Faulted),
            Err(InferenceError::Artifact(_)) => Err(EmbedFailure::Unavailable("busy")),
            Err(InferenceError::Input(_)) => Err(EmbedFailure::Unavailable("input")),
            Err(InferenceError::Execution(_) | InferenceError::Invariant(_)) => {
                Err(EmbedFailure::Faulted)
            }
        }
    }
}

fn dense_lane(
    conn: &GuardedConn<'_>,
    kernel: &KernelStore,
    identity: &retrieval::ProjectionIdentity,
    authority: Authority<'_>,
    lane: &DenseLane<'_>,
    budget: &SharedBudget,
) -> Result<LaneOutput, QueryFailure> {
    let (query, generation_id, producer) = match lane {
        DenseLane::Undeclared => return Ok(LaneOutput::undeclared()),
        DenseLane::Unavailable(reason) => return Ok(LaneOutput::unavailable(reason)),
        DenseLane::Ready {
            query,
            generation_id,
            producer,
        } => (*query, *generation_id, *producer),
    };
    let generation = VectorGeneration {
        generation_id: generation_id.to_string(),
        embedding_model: identity.embedding_model.clone(),
        tokenizer_fingerprint: identity.tokenizer_fingerprint.clone(),
        vector_dimension: identity.vector_dimension,
        generation_epoch: identity.generation_epoch,
    };
    let request = DenseRequest {
        generation: &generation,
        query,
        authority,
        budget: budget.eval(),
    };
    match producer.rank(conn, kernel, request) {
        Ok(ranking) => {
            let hits = ranking
                .hits
                .iter()
                .filter_map(|(occurrence_id, score)| hit(occurrence_id, RawScore::Dense(*score)))
                .collect();
            Ok(LaneOutput::ranked(Lane::Dense, hits, ranking.status))
        }
        Err(DenseRefusal::Budget) => Err(exhaustion(budget).into()),
        Err(DenseRefusal::QueryShape) => Ok(LaneOutput::unavailable("query_shape")),
        Err(DenseRefusal::Corruption) => Err(QueryFailure::Unavailable("dense_corruption")),
        Err(DenseRefusal::Unavailable(reason)) => Ok(LaneOutput::unavailable(reason)),
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
) -> Result<LaneOutput, QueryFailure> {
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
    if object_ids.len() > limits.probes.get() {
        return Err(QueryFailure::InvalidQuery(format!(
            "{} selectors exceed the {} probe bound",
            object_ids.len(),
            limits.probes
        )));
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
                Err(refusal) => {
                    return match lookup_refusal(&refusal) {
                        LaneRefusal::Budget => Err(exhaustion(budget).into()),
                        LaneRefusal::Reason(reason) => Ok(LaneOutput::unavailable(reason)),
                    };
                }
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
) -> Result<LaneOutput, QueryFailure> {
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
        Err(refusal) => return Ok(LaneOutput::unavailable(lexical_refusal(&refusal))),
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
        Err(refusal) => {
            return match retrieval_refusal(&refusal) {
                LaneRefusal::Budget => Err(exhaustion(budget).into()),
                LaneRefusal::Reason(reason) => Ok(LaneOutput::unavailable(reason)),
            };
        }
    };
    let status = match retrieval.completion {
        Completion::Complete | Completion::Empty => LaneStatus::Complete,
        Completion::Incomplete(IncompleteReason::BudgetExhausted) => {
            return Err(exhaustion(budget).into());
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
    authority: Authority<'_>,
    limits: &QueryRouteLimits,
    budget: &SharedBudget,
    query: &str,
    dense: DenseLane<'_>,
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
            return Ok(Err(QueryFailure::Unavailable("no_identity")));
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
            let lexical = lexical_lane(conn, kernel, authority, &intent, query, limits, budget)?;
            if exact.ranking.is_none() && lexical.ranking.is_none() {
                let both_undeclared = exact.status == LaneStatus::Undeclared
                    && lexical.status == LaneStatus::Undeclared;
                return Err(if both_undeclared {
                    QueryFailure::InvalidQuery("the query yields no probe".to_string())
                } else {
                    QueryFailure::Unavailable("no_lane")
                });
            }
            before_phase(Phase::Dense);
            check(budget)?;
            let dense = dense_lane(conn, kernel, &identity, authority, &dense, budget)?;
            before_phase(Phase::Fusion);
            check(budget)?;
            let rankings = [exact.ranking, lexical.ranking, dense.ranking]
                .into_iter()
                .flatten();
            let lanes = DeclaredLanes::admit(rankings)
                .map_err(|_| QueryFailure::Unavailable("duplicate_lane"))?;
            let fused = fuse(lanes, &limits.fusion, limits.fused_union)
                .map_err(|_| QueryFailure::Unavailable("fused_union"))?;
            let ids: Vec<String> = fused
                .entries()
                .iter()
                .map(|entry| entry.occurrence().to_string())
                .collect();
            let candidates =
                live_candidates_by_id(conn, ids.iter().map(String::as_str)).map_err(|error| {
                    match error {
                        ProjectionError::Interrupted => QueryFailure::Terminal(exhaustion(budget)),
                        _ => QueryFailure::Unavailable("candidate_read"),
                    }
                })?;
            check(budget)?;
            let mut statuses = [const { LaneStatus::Undeclared }; Lane::ORDER.len()];
            for (lane, status) in [
                (Lane::Exact, exact.status),
                (Lane::Lexical, lexical.status),
                (Lane::Dense, dense.status),
            ] {
                statuses[lane_slot(lane)] = status;
            }
            Ok(Scanned {
                statuses,
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
        Err(_) => return Err(QueryFailure::Unavailable("projection_read")),
    };

    before_phase(Phase::Revalidation);
    check(budget)?;
    let mut eligible: BTreeSet<OccurrenceId> = BTreeSet::new();
    for batch in scanned.candidates.chunks(limits.validation_batch.get()) {
        check(budget)?;
        let report = judge_occurrences_within_budget(
            kernel,
            authority.project,
            authority.destination,
            batch,
            budget.eval(),
        )
        .map_err(|error| match error {
            KernelError::Deadline => QueryFailure::Terminal(exhaustion(budget)),
            _ => QueryFailure::Unavailable("eligibility"),
        })?;
        eligible.extend(
            report
                .occurrences
                .into_iter()
                .filter(|judged| judged.disposition == Disposition::Eligible)
                .filter_map(|judged| OccurrenceId::parse(&judged.occurrence_id).ok()),
        );
    }
    let fused = scanned
        .fused
        .filter(|entry| eligible.contains(entry.occurrence()));

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
    let mut used = measure_json(&envelope(Vec::new(), true))
        .map_err(|_| QueryFailure::Unavailable("response_measure"))?;
    let mut entries = Vec::new();
    let mut truncated = false;
    for entry in fused.entries() {
        if entries.len() >= limits.result_rows.get() {
            truncated = true;
            break;
        }
        let value = entry_json(entry);
        let size =
            measure_json(&value).map_err(|_| QueryFailure::Unavailable("response_measure"))? + 1;
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

fn lane_slot(lane: Lane) -> usize {
    Lane::ORDER
        .iter()
        .position(|candidate| *candidate == lane)
        .expect("every lane has a slot in Lane::ORDER")
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

fn unavailable_response(reason: &'static str) -> PreparedOutcome {
    PreparedOutcome::Response(PreparedOutput::json(json!({
        "kind": "terminal",
        "terminal": Terminal::LaneUnavailable.code(),
        "reason": reason,
    })))
}

impl HandlerCore {
    pub fn set_query_route_limits(
        &self,
        limits: Option<QueryRouteLimits>,
    ) -> Result<(), LimitsRefusal> {
        if let Some(limits) = &limits {
            limits.validate()?;
        }
        *self
            .query_route
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = limits.map(Arc::new);
        Ok(())
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
        if parsed
            .harness
            .as_deref()
            .is_some_and(|claimed| claimed != scope.harness)
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
        let destination = ArtifactDestination::from(parsed.destination);
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
            return unavailable_response("no_lifecycle");
        };
        let shared = budget.shared().clone();
        let RouteScope {
            store,
            project,
            harness: _,
        } = scope;
        let query = parsed.query;
        let embedded = if limits.dense.is_some() {
            let embedder = self.query_embedder(&lifecycle);
            let slot: Arc<Mutex<Option<EmbedResult>>> = Arc::default();
            let text = query.clone();
            let written = Arc::clone(&slot);
            let step = runner.run_step(Box::new(move || {
                let result = embedder.embed(&text);
                *written
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(result);
            }));
            if let Err(failed) = step.await {
                drop(budget);
                return blocking_failure(failed);
            }
            let result = slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            match result {
                Some(Ok(vector)) => Embedded::Vector(vector),
                Some(Err(EmbedFailure::Unavailable(reason))) => Embedded::Unavailable(reason),
                Some(Err(EmbedFailure::Faulted)) | None => {
                    drop(budget);
                    return unavailable_response("embedding_failed");
                }
            }
        } else {
            Embedded::Undeclared
        };
        if shared.is_exhausted() {
            let terminal = exhaustion(&shared);
            drop(budget);
            return terminal_response(terminal);
        }
        let work = runner.run_unit(Box::new(move || {
            let reader = match lifecycle.pin(shared.eval()) {
                Ok(reader) => reader,
                Err(_) if shared.is_exhausted() => {
                    return UnitOutcome::Terminal(terminal_response(exhaustion(&shared)));
                }
                Err(_) => {
                    return UnitOutcome::Terminal(unavailable_response("no_family"));
                }
            };
            let producer = limits
                .dense
                .map(|dense| ExhaustiveProducer { limits: dense });
            let dense = match (&embedded, &producer) {
                (Embedded::Vector(vector), Some(producer)) => DenseLane::Ready {
                    query: vector,
                    generation_id: &reader.consumer().generation_id,
                    producer,
                },
                (Embedded::Unavailable(reason), _) => DenseLane::Unavailable(reason),
                (Embedded::Undeclared, _) | (Embedded::Vector(_), None) => DenseLane::Undeclared,
            };
            let outcome = execute(
                reader.projection(),
                &store,
                Authority {
                    project: project.scope(),
                    destination,
                },
                &limits,
                &shared,
                &query,
                dense,
                |_| {},
            );
            UnitOutcome::Terminal(match outcome {
                Ok(outcome) => PreparedOutcome::Response(PreparedOutput::json(outcome.body)),
                Err(QueryFailure::Terminal(terminal)) => terminal_response(terminal),
                Err(QueryFailure::Unavailable(reason)) => unavailable_response(reason),
                Err(QueryFailure::InvalidQuery(reason)) => {
                    invalid_params_error(format!("{OPERATION}: {reason}"))
                }
            })
        }));
        let outcome = work.await;
        drop(budget);
        match outcome {
            Ok(UnitOutcome::Terminal(outcome)) => outcome,
            Ok(UnitOutcome::Continue(_)) => unavailable_response("unit_continued"),
            Err(failed) => blocking_failure(failed),
        }
    }

    /// Parent Q6: the query is embedded in process by the lane the family was built under; nothing is routed, so no remaining duration crosses a process boundary.
    fn query_embedder(&self, lifecycle: &SearchLifecycleOwner) -> Arc<dyn QueryEmbedder> {
        #[cfg(any(test, feature = "test-support"))]
        if let Some(embedder) = self
            .query_embedder_override
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            return embedder;
        }
        Arc::new(lifecycle.embeddings().clone())
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn set_query_embedder_for_test(&self, embedder: Option<Arc<dyn QueryEmbedder>>) {
        *self
            .query_embedder_override
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = embedder;
    }
}

pub type EmbedResult = Result<Vec<f32>, EmbedFailure>;

enum Embedded {
    Undeclared,
    Vector(Vec<f32>),
    Unavailable(&'static str),
}

fn blocking_failure(failed: host_runtime::BlockingWorkFailed) -> PreparedOutcome {
    match BlockingFailure::from(failed) {
        BlockingFailure::Panicked => PreparedOutcome::Error {
            code: "internal_error".to_string(),
            message: "query work failed".to_string(),
        },
        BlockingFailure::RuntimeStopped | BlockingFailure::RouteClosing => {
            terminal_response(Terminal::Cancelled)
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
            LaneStatus::Unavailable("snapshot_changed").json(),
            json!({"status": "unavailable", "reason": "snapshot_changed"})
        );
        assert_eq!(
            LaneStatus::Undeclared.json(),
            json!({"status": "undeclared"})
        );
        assert!(!LaneStatus::Complete.degrades());
        assert!(!LaneStatus::Undeclared.degrades());
        assert!(LaneStatus::Incomplete("page_bound").degrades());
        assert!(LaneStatus::Unavailable("engine").degrades());
    }
}

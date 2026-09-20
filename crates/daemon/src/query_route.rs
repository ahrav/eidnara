//! Payload bytes are never read here; packing consumes the ranking and materializes payloads under its own bounds.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use host_runtime::RouteHandle;
use host_runtime::local_embeddings::{
    DenseUnavailable, InferenceFailureKind, LaneUnavailableState, LocalEmbeddingsComponent,
    LocalEmbeddingsStatus,
};
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
    Authority, AuthorityMoved, Disposition, EligibilityReport, OccurrenceCandidate, judge_tracked,
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
    Scan, admit, analyze_segments, compile, scan,
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
    #[error("response_bytes {value} cannot hold the {floor}-byte empty fused envelope")]
    ResponseBytes { value: usize, floor: usize },
    #[error("response_bytes {value} exceeds the {max}-byte wire body the host can send")]
    ResponseBytesOverWire { value: usize, max: usize },
    #[error(
        "dense {bound} {value} exceeds the kernel's {MAX_ELIGIBILITY_CANDIDATES} candidate batch"
    )]
    Dense { bound: &'static str, value: usize },
    #[error("dense unit_norm_tolerance must be finite and not negative")]
    DenseTolerance,
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
    /// An all-`Undeclared` lane array is larger than an array with one `Complete` lane.
    pub fn response_floor() -> NonZeroUsize {
        let mut statuses = [const { LaneStatus::Undeclared }; Lane::ORDER.len()];
        statuses[0] = LaneStatus::Complete;
        let bytes = measure_json(&fused_envelope(&statuses, false, Vec::new(), false))
            .expect("the empty fused envelope measures");
        NonZeroUsize::new(bytes).expect("the empty fused envelope is not empty")
    }

    /// `validate` rejects limits the kernel eligibility batch cannot serve, response bounds below the empty fused envelope, and response bounds above the host wire maximum at installation.
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
        let floor = Self::response_floor();
        if self.response_bytes < floor {
            return Err(LimitsRefusal::ResponseBytes {
                value: self.response_bytes.get(),
                floor: floor.get(),
            });
        }
        if self.response_bytes.get() > crate::dispatch::MAX_WIRE_BODY_BYTES {
            return Err(LimitsRefusal::ResponseBytesOverWire {
                value: self.response_bytes.get(),
                max: crate::dispatch::MAX_WIRE_BODY_BYTES,
            });
        }
        if let Some(dense) = &self.dense {
            for (bound, value) in [("k", dense.k.get()), ("page_rows", dense.page_rows.get())] {
                if value > MAX_ELIGIBILITY_CANDIDATES {
                    return Err(LimitsRefusal::Dense { bound, value });
                }
            }
            if !dense.unit_norm_tolerance.is_finite() || dense.unit_norm_tolerance < 0.0 {
                return Err(LimitsRefusal::DenseTolerance);
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
    /// The dense oracle judges each page it scores, so its lane is finished under the connection.
    Dense,
    /// The exact and lexical hits are judged by the kernel, after the projection connection is released.
    Admission,
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
    pub exact: ExactReport,
    /// The kernel's verdicts on every fused entry; `None` when nothing was fused.
    pub revalidation: Option<EligibilityReport>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactReport {
    /// Live rows in page order, before admission.
    pub rows: Vec<OccurrenceCandidate>,
    pub admission: ExactAdmission,
}

/// What the kernel said about the exact lane's rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExactAdmission {
    /// The lane ended before admission, or read no live row.
    NotJudged,
    /// Every row judged under one reusable snapshot, in `rows` order.
    Judged(EligibilityReport),
    /// A batch's state differed from the first batch's or described no
    /// reusable window; the report covers that batch alone and its verdicts
    /// join nothing.
    Moved(EligibilityReport),
    KernelError,
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
/// A stored identifier outside the contract spelling is a corrupt row, not a smaller result set.
fn hit(occurrence_id: &str, raw_score: RawScore) -> Result<LaneHit, IdentityRefusal> {
    OccurrenceId::parse(occurrence_id).map(|occurrence| LaneHit {
        occurrence,
        raw_score,
    })
}

pub struct DenseRequest<'a> {
    pub generation: &'a VectorGeneration,
    pub query: &'a [f32],
    pub authority: Authority<'a>,
    pub budget: &'a EvalBudget,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DenseRanking {
    /// Best first; each row carries the terms the producer judged it under, so revalidation judges the same facts.
    pub hits: Vec<(OccurrenceCandidate, f64)>,
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
                .candidates
                .into_iter()
                .zip(ranking.ranked)
                .map(|(candidate, row)| (candidate, row.score))
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
    /// When the lane is starting, disabled, failing, busy, rejects the input, or serves another identity, the dense lane degrades.
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
        let lane = match self.status() {
            LocalEmbeddingsStatus::Ready(lane) => lane,
            LocalEmbeddingsStatus::Starting => return Err(EmbedFailure::Unavailable("starting")),
            LocalEmbeddingsStatus::Disabled { .. } => {
                return Err(EmbedFailure::Unavailable("disabled"));
            }
            LocalEmbeddingsStatus::Failing { .. } => {
                return Err(EmbedFailure::Unavailable("failing"));
            }
        };
        let admitted = self
            .preflight_embedding_for_lane(&lane, text)
            .map_err(embed_refusal)?;
        self.embed_admitted(&admitted).map_err(embed_refusal)
    }
}

fn embed_refusal(refusal: DenseUnavailable) -> EmbedFailure {
    match refusal {
        DenseUnavailable::LaneUnavailable { state } => EmbedFailure::Unavailable(match state {
            LaneUnavailableState::Starting => "starting",
            LaneUnavailableState::Unsupported | LaneUnavailableState::Disabled => "disabled",
            LaneUnavailableState::Failing => "failing",
        }),
        DenseUnavailable::LaneBusy { .. } => EmbedFailure::Unavailable("busy"),
        DenseUnavailable::IdentityChanged => EmbedFailure::Unavailable("lane_changed"),
        DenseUnavailable::EmptyInput
        | DenseUnavailable::ByteOverflow { .. }
        | DenseUnavailable::ZeroTokens { .. }
        | DenseUnavailable::TokenOverflow { .. }
        | DenseUnavailable::CountUnavailable(InferenceFailureKind::Input)
        | DenseUnavailable::Inference(InferenceFailureKind::Input) => {
            EmbedFailure::Unavailable("input")
        }
        DenseUnavailable::CountUnavailable(
            InferenceFailureKind::Execution
            | InferenceFailureKind::Artifact
            | InferenceFailureKind::Invariant,
        )
        | DenseUnavailable::Inference(
            InferenceFailureKind::Execution
            | InferenceFailureKind::Artifact
            | InferenceFailureKind::Invariant,
        ) => EmbedFailure::Faulted,
    }
}

/// Whether the text outside `query`'s selector mentions analyzes to at least one lexical atom; punctuation alone is not prose, so it declares no dense lane.
fn has_prose(intent: &Intent, query: &str, limits: &QueryRouteLimits) -> bool {
    match analyze_segments(&intent.lexical_segments(query), lexical_bounds(limits)) {
        Ok(analysis) => !analysis.is_empty(),
        Err(_) => true,
    }
}

fn lexical_bounds(limits: &QueryRouteLimits) -> LexicalBounds {
    LexicalBounds {
        max_input_bytes: limits.query_bytes,
        max_atoms: limits.probes,
    }
}

fn selector_bounds(limits: &QueryRouteLimits) -> SelectorBounds {
    SelectorBounds {
        max_input_bytes: limits.query_bytes,
        max_value_bytes: limits.query_bytes,
    }
}

/// The dense lane's output and the terms its producer judged each hit under.
struct DenseHits {
    output: LaneOutput,
    candidates: Vec<(OccurrenceId, OccurrenceCandidate)>,
}

impl DenseHits {
    fn ended(output: LaneOutput) -> Self {
        Self {
            output,
            candidates: Vec::new(),
        }
    }
}

/// The producer judges its rows itself, so the lane is finished under the connection and only its terms join admission.
fn dense_lane(
    conn: &GuardedConn<'_>,
    kernel: &KernelStore,
    identity: &retrieval::ProjectionIdentity,
    authority: Authority<'_>,
    lane: &DenseLane<'_>,
    budget: &SharedBudget,
) -> Result<DenseHits, QueryFailure> {
    let (query, generation_id, producer) = match lane {
        DenseLane::Undeclared => return Ok(DenseHits::ended(LaneOutput::undeclared())),
        DenseLane::Unavailable(reason) => {
            return Ok(DenseHits::ended(LaneOutput::unavailable(reason)));
        }
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
    let ranking = match producer.rank(conn, kernel, request) {
        Ok(ranking) => ranking,
        Err(DenseRefusal::Budget) => return Err(exhaustion(budget).into()),
        Err(DenseRefusal::QueryShape) => {
            return Ok(DenseHits::ended(LaneOutput::unavailable("query_shape")));
        }
        Err(DenseRefusal::Corruption) => {
            return Err(QueryFailure::Unavailable("dense_corruption"));
        }
        Err(DenseRefusal::Unavailable(reason)) => {
            return Ok(DenseHits::ended(LaneOutput::unavailable(reason)));
        }
    };
    let mut hits = Vec::with_capacity(ranking.hits.len());
    let mut candidates = Vec::with_capacity(ranking.hits.len());
    for (candidate, score) in ranking.hits {
        let Ok(hit) = hit(&candidate.occurrence_id, RawScore::Dense(score)) else {
            return Ok(DenseHits::ended(LaneOutput::unavailable("identity")));
        };
        candidates.push((hit.occurrence, candidate));
        hits.push(hit);
    }
    Ok(DenseHits {
        output: LaneOutput::ranked(Lane::Dense, hits, ranking.status),
        candidates,
    })
}

struct ScopedHit {
    hit: LaneHit,
    candidate: OccurrenceCandidate,
}

/// A lane's projection read: work the kernel still judges, or the status that ended the lane.
enum LaneRead<T> {
    Pending(T),
    Ended(LaneStatus),
}

struct ExactHits {
    hits: Vec<ScopedHit>,
    status: LaneStatus,
}

/// The `id:` selector values the exact lane probes; the probe bound applies to their count.
fn object_ids(intent: &Intent) -> Vec<&str> {
    let selectors: Vec<&Selector> = match intent {
        Intent::Direct(selector) => vec![selector],
        Intent::Hybrid(mentions) => mentions.iter().map(|mention| &mention.selector).collect(),
    };
    selectors
        .iter()
        .filter(|selector| selector.family == Family::Id)
        .filter_map(|selector| match &selector.value {
            SelectorValue::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn exact_read(
    conn: &GuardedConn<'_>,
    kernel_incarnation_id: &str,
    intent: &Intent,
    limits: &QueryRouteLimits,
    budget: &SharedBudget,
) -> Result<LaneRead<ExactHits>, QueryFailure> {
    let object_ids = object_ids(intent);
    if object_ids.is_empty() {
        return Ok(LaneRead::Ended(LaneStatus::Undeclared));
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
                        LaneRefusal::Reason(reason) => {
                            Ok(LaneRead::Ended(LaneStatus::Unavailable(reason)))
                        }
                    };
                }
            };
            for row in page.rows.iter().filter(|row| row.tombstone.is_none()) {
                let Ok(hit) = hit(&row.occurrence_id, RawScore::Exact) else {
                    return Ok(LaneRead::Ended(LaneStatus::Unavailable("identity")));
                };
                hits.push(ScopedHit {
                    hit,
                    candidate: OccurrenceCandidate::new(
                        row.occurrence_id.clone(),
                        row.class,
                        row.source_object_id.clone(),
                        row.revision,
                        row.source_artifact_digest.clone(),
                    ),
                });
            }
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        if cursor.is_some() {
            status = LaneStatus::Incomplete("page_bound");
        }
    }
    Ok(LaneRead::Pending(ExactHits { hits, status }))
}

fn lexical_read(
    conn: &GuardedConn<'_>,
    intent: &Intent,
    query: &str,
    limits: &QueryRouteLimits,
    budget: &SharedBudget,
) -> Result<LaneRead<Scan>, QueryFailure> {
    let segments = match intent {
        Intent::Direct(_) => Vec::new(),
        Intent::Hybrid(_) => intent.lexical_segments(query),
    };
    let analysis = match analyze_segments(&segments, lexical_bounds(limits)) {
        Ok(analysis) => analysis,
        Err(refusal) => {
            return Ok(LaneRead::Ended(LaneStatus::Unavailable(lexical_refusal(
                &refusal,
            ))));
        }
    };
    if analysis.is_empty() {
        return Ok(LaneRead::Ended(LaneStatus::Undeclared));
    }
    let probes = compile(&analysis);
    let bounds = RetrievalBounds {
        max_probes: limits.probes,
        scan_rows: limits.lexical_scan_rows,
        max_accepted: limits.lexical_accepted,
        batch_rows: limits.validation_batch,
    };
    match scan(conn, &probes, bounds, budget.eval()) {
        Ok(scanned) => Ok(LaneRead::Pending(scanned)),
        Err(refusal) => match retrieval_refusal(&refusal) {
            LaneRefusal::Budget => Err(exhaustion(budget).into()),
            LaneRefusal::Reason(reason) => Ok(LaneRead::Ended(LaneStatus::Unavailable(reason))),
        },
    }
}

enum Judged {
    Eligible {
        eligible: BTreeSet<OccurrenceId>,
        /// Every batch's verdicts under the first batch's snapshot and incarnation, which
        /// every later batch matched or the judgement would have moved; `None` when no
        /// candidate was judged.
        report: Option<EligibilityReport>,
    },
    /// The batch whose state differed from the first batch's, with its reason.
    Moved(&'static str, EligibilityReport),
    Kernel,
}

/// Judges `candidates` in `validation_batch` slices, refusing to join verdicts from different kernel snapshots or incarnations.
///
/// `between_batches` runs before every slice after the first; the caller gates the first slice itself.
fn judge_eligible(
    kernel: &KernelStore,
    authority: Authority<'_>,
    limits: &QueryRouteLimits,
    budget: &SharedBudget,
    candidates: &[(OccurrenceId, &OccurrenceCandidate)],
    mut between_batches: impl FnMut() -> Result<(), QueryFailure>,
) -> Result<Judged, QueryFailure> {
    let mut snapshot = None;
    let mut incarnation = None;
    let mut merged: Option<EligibilityReport> = None;
    for (index, batch) in candidates.chunks(limits.validation_batch.get()).enumerate() {
        if index > 0 {
            between_batches()?;
        }
        let terms: Vec<OccurrenceCandidate> =
            batch.iter().map(|(_, terms)| (*terms).clone()).collect();
        let (report, moved) = match judge_tracked(
            kernel,
            authority,
            &terms,
            budget.eval(),
            &mut snapshot,
            &mut incarnation,
        ) {
            Ok(judged) => judged,
            Err(KernelError::Deadline) => return Err(exhaustion(budget).into()),
            Err(_) => return Ok(Judged::Kernel),
        };
        match moved {
            Some(AuthorityMoved::Incarnation) => {
                return Ok(Judged::Moved("kernel_incarnation_changed", report));
            }
            Some(AuthorityMoved::Snapshot) => {
                return Ok(Judged::Moved("snapshot_changed", report));
            }
            None => {}
        }
        match &mut merged {
            Some(merged) => merged.occurrences.extend(report.occurrences),
            None => merged = Some(report),
        }
    }
    let eligible = candidates
        .iter()
        .zip(merged.iter().flat_map(|report| &report.occurrences))
        .filter(|(_, judged)| judged.disposition == Disposition::Eligible)
        .map(|((occurrence, _), _)| *occurrence)
        .collect();
    Ok(Judged::Eligible {
        eligible,
        report: merged,
    })
}

/// Judges the exact lane's rows under the request's authority before any position is assigned, so a row the caller may not see earns no position and consumes no union slot.
fn admit_exact(
    kernel: &KernelStore,
    authority: Authority<'_>,
    limits: &QueryRouteLimits,
    budget: &SharedBudget,
    read: LaneRead<ExactHits>,
    terms: &mut BTreeMap<OccurrenceId, OccurrenceCandidate>,
) -> Result<(LaneOutput, ExactReport), QueryFailure> {
    let ExactHits { hits, status } = match read {
        LaneRead::Pending(hits) => hits,
        LaneRead::Ended(status) => {
            return Ok((
                LaneOutput {
                    ranking: None,
                    status,
                },
                ExactReport {
                    rows: Vec::new(),
                    admission: ExactAdmission::NotJudged,
                },
            ));
        }
    };
    let rows: Vec<OccurrenceCandidate> =
        hits.iter().map(|scoped| scoped.candidate.clone()).collect();
    let candidates: Vec<(OccurrenceId, &OccurrenceCandidate)> = hits
        .iter()
        .map(|scoped| (scoped.hit.occurrence, &scoped.candidate))
        .collect();
    let (output, admission) =
        match judge_eligible(kernel, authority, limits, budget, &candidates, || {
            check(budget).map_err(Into::into)
        })? {
            Judged::Eligible { eligible, report } => {
                let mut ranked = Vec::new();
                for scoped in hits {
                    if eligible.contains(&scoped.hit.occurrence) {
                        terms.insert(scoped.hit.occurrence, scoped.candidate);
                        ranked.push(scoped.hit);
                    }
                }
                (
                    LaneOutput::ranked(Lane::Exact, ranked, status),
                    report.map_or(ExactAdmission::NotJudged, ExactAdmission::Judged),
                )
            }
            Judged::Moved(reason, report) => (
                LaneOutput::unavailable(reason),
                ExactAdmission::Moved(report),
            ),
            Judged::Kernel => (
                LaneOutput::unavailable("kernel"),
                ExactAdmission::KernelError,
            ),
        };
    Ok((output, ExactReport { rows, admission }))
}

fn admit_lexical(
    kernel: &KernelStore,
    authority: Authority<'_>,
    budget: &SharedBudget,
    read: LaneRead<Scan>,
    terms: &mut BTreeMap<OccurrenceId, OccurrenceCandidate>,
) -> Result<LaneOutput, QueryFailure> {
    let scanned = match read {
        LaneRead::Pending(scanned) => scanned,
        LaneRead::Ended(status) => {
            return Ok(LaneOutput {
                ranking: None,
                status,
            });
        }
    };
    let retrieval = match admit(kernel, authority, scanned, budget.eval()) {
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
    let mut ranked = Vec::with_capacity(retrieval.contributions.len());
    for contribution in &retrieval.contributions {
        let Ok(hit) = hit(
            &contribution.occurrence_id,
            RawScore::Lexical(contribution.rank),
        ) else {
            return Ok(LaneOutput::unavailable("identity"));
        };
        terms.insert(hit.occurrence, contribution.occurrence_candidate());
        ranked.push(hit);
    }
    Ok(LaneOutput::ranked(Lane::Lexical, ranked, status))
}

struct Scanned {
    exact: LaneRead<ExactHits>,
    lexical: LaneRead<Scan>,
    dense: DenseHits,
}

/// The lanes' admitted rankings before fusion assigns positions, with the
/// request context they were judged under so [`select`] cannot be given another.
pub struct Admitted<'a> {
    pub statuses: [LaneStatus; Lane::ORDER.len()],
    pub lanes: DeclaredLanes,
    pub exact: ExactReport,
    terms: BTreeMap<OccurrenceId, OccurrenceCandidate>,
    kernel: &'a KernelStore,
    authority: Authority<'a>,
    limits: &'a QueryRouteLimits,
    budget: &'a SharedBudget,
}

/// [`admit_lanes`] then [`select`]; the handler and the evaluator share this one path.
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
    let admitted = admit_lanes(
        projection,
        kernel,
        authority,
        limits,
        budget,
        query,
        dense,
        &mut before_phase,
    )?;
    select(admitted, before_phase)
}

/// Reads and judges every declared lane; ends at the admission phase.
#[allow(clippy::too_many_arguments)]
pub fn admit_lanes<'a>(
    projection: &SearchProjection,
    kernel: &'a KernelStore,
    authority: Authority<'a>,
    limits: &'a QueryRouteLimits,
    budget: &'a SharedBudget,
    query: &str,
    dense: DenseLane<'_>,
    mut before_phase: impl FnMut(Phase),
) -> Result<Admitted<'a>, QueryFailure> {
    before_phase(Phase::Probes);
    check(budget)?;
    let intent = classify(query, selector_bounds(limits))
        .map_err(|refusal| QueryFailure::InvalidQuery(refusal.to_string()))?;

    // The exact and lexical lanes run only projection statements while the connection is held and take their kernel readers after it is released; the dense oracle judges each page it scores, so it is the one lane that reads the kernel under the connection.
    let read = projection.read_under(budget, |conn| {
        let Some(identity) = retrieval::read_identity(conn)? else {
            return Ok(Err(QueryFailure::Unavailable("no_identity")));
        };
        let mut phases = || -> Result<Scanned, QueryFailure> {
            before_phase(Phase::Exact);
            check(budget)?;
            let exact = exact_read(
                conn,
                &identity.kernel_incarnation_id,
                &intent,
                limits,
                budget,
            )?;
            before_phase(Phase::Lexical);
            check(budget)?;
            let lexical = lexical_read(conn, &intent, query, limits, budget)?;
            before_phase(Phase::Dense);
            check(budget)?;
            let dense = if has_prose(&intent, query, limits) {
                dense_lane(conn, kernel, &identity, authority, &dense, budget)?
            } else {
                DenseHits::ended(LaneOutput::undeclared())
            };
            Ok(Scanned {
                exact,
                lexical,
                dense,
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
    let DenseHits {
        output: dense,
        candidates: dense_candidates,
    } = scanned.dense;
    if matches!(
        (&scanned.exact, &scanned.lexical),
        (
            LaneRead::Ended(LaneStatus::Undeclared),
            LaneRead::Ended(LaneStatus::Undeclared)
        )
    ) && dense.status == LaneStatus::Undeclared
    {
        return Err(QueryFailure::InvalidQuery(
            "the query yields no probe".to_string(),
        ));
    }

    before_phase(Phase::Admission);
    check(budget)?;
    let mut terms: BTreeMap<OccurrenceId, OccurrenceCandidate> = BTreeMap::new();
    let (exact, exact_report) =
        admit_exact(kernel, authority, limits, budget, scanned.exact, &mut terms)?;
    let lexical = admit_lexical(kernel, authority, budget, scanned.lexical, &mut terms)?;
    terms.extend(dense_candidates);
    if exact.ranking.is_none() && lexical.ranking.is_none() && dense.ranking.is_none() {
        return Err(QueryFailure::Unavailable("no_lane"));
    }
    let mut statuses = [const { LaneStatus::Undeclared }; Lane::ORDER.len()];
    for (lane, status) in [
        (Lane::Exact, exact.status),
        (Lane::Lexical, lexical.status),
        (Lane::Dense, dense.status),
    ] {
        statuses[lane_slot(lane)] = status;
    }
    let rankings = [exact.ranking, lexical.ranking, dense.ranking]
        .into_iter()
        .flatten();
    let lanes =
        DeclaredLanes::admit(rankings).map_err(|_| QueryFailure::Unavailable("duplicate_lane"))?;
    Ok(Admitted {
        statuses,
        lanes,
        exact: exact_report,
        terms,
        kernel,
        authority,
        limits,
        budget,
    })
}

/// Fuses, revalidates, and materializes the response from admitted lanes.
pub fn select(
    admitted: Admitted<'_>,
    mut before_phase: impl FnMut(Phase),
) -> Result<QueryOutcome, QueryFailure> {
    let Admitted {
        statuses,
        lanes,
        exact: exact_report,
        terms,
        kernel,
        authority,
        limits,
        budget,
    } = admitted;
    before_phase(Phase::Fusion);
    check(budget)?;
    let fused = fuse(lanes, &limits.fusion, limits.fused_union)
        .map_err(|_| QueryFailure::Unavailable("fused_union"))?;

    before_phase(Phase::Revalidation);
    check(budget)?;
    let candidates: Vec<(OccurrenceId, &OccurrenceCandidate)> = fused
        .entries()
        .iter()
        .map(|entry| {
            let occurrence = *entry.occurrence();
            let terms = terms
                .get(&occurrence)
                .expect("every fused entry came from an admitted lane hit");
            (occurrence, terms)
        })
        .collect();
    let (eligible, revalidation) =
        match judge_eligible(kernel, authority, limits, budget, &candidates, || {
            before_phase(Phase::Revalidation);
            check(budget).map_err(Into::into)
        })? {
            Judged::Eligible { eligible, report } => (eligible, report),
            Judged::Moved(reason, _) => return Err(QueryFailure::Unavailable(reason)),
            Judged::Kernel => return Err(QueryFailure::Unavailable("eligibility")),
        };
    let fused = fused.filter(|entry| eligible.contains(entry.occurrence()));

    before_phase(Phase::Materialization);
    check(budget)?;
    let degraded = statuses.iter().any(LaneStatus::degrades);
    let envelope = |entries: Vec<Value>, truncated: bool| {
        fused_envelope(&statuses, degraded, entries, truncated)
    };
    // `false` is the longer spelling, so the baseline over-counts a truncated body by one byte and never under-counts.
    let mut used = measure_json(&envelope(Vec::new(), false))
        .map_err(|_| QueryFailure::Unavailable("response_measure"))?;
    if used > limits.response_bytes.get() {
        return Err(QueryFailure::Unavailable("response_bytes"));
    }
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
        exact: exact_report,
        revalidation,
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

fn fused_envelope(
    statuses: &[LaneStatus; Lane::ORDER.len()],
    degraded: bool,
    entries: Vec<Value>,
    truncated: bool,
) -> Value {
    json!({
        "kind": "fused",
        "degraded": degraded,
        "lanes": lanes_json(statuses),
        "truncated": truncated,
        "entries": entries,
    })
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
            project_root: _,
            harness: _,
            context_capabilities: _,
        } = scope;
        let query = parsed.query;
        // Requests without prose or with more than `limits.probes` ID selectors skip dense inference.
        let embeds = limits.dense.is_some()
            && classify(&query, selector_bounds(&limits)).is_ok_and(|intent| {
                has_prose(&intent, &query, &limits)
                    && object_ids(&intent).len() <= limits.probes.get()
            });
        let embedded = if embeds {
            let embedder = self.query_embedder(&lifecycle);
            let slot: Arc<Mutex<Option<EmbedResult>>> = Arc::default();
            let text = query.clone();
            let written = Arc::clone(&slot);
            let observed = shared.clone();
            let step = runner.run_step(Box::new(move || {
                if observed.is_exhausted() {
                    return;
                }
                let result = embedder.embed(&text);
                *written
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(result);
            }));
            if let Err(failed) = budget.shared().bridge(step).await {
                drop(budget);
                return blocking_failure(failed);
            }
            if shared.is_exhausted() {
                let terminal = exhaustion(&shared);
                drop(budget);
                return terminal_response(terminal);
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
        let outcome = budget.shared().bridge(work).await;
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
    fn a_lane_that_is_not_ready_is_an_unavailability_never_a_fault() {
        let unsupported = LocalEmbeddingsComponent::unsupported("no lane");
        assert_eq!(
            unsupported.embed("query"),
            Err(EmbedFailure::Unavailable("disabled"))
        );
        let starting = LocalEmbeddingsComponent::new(None);
        assert!(matches!(
            starting.embed("query"),
            Err(EmbedFailure::Unavailable("starting" | "disabled"))
        ));
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

    #[test]
    fn a_response_bound_below_the_empty_envelope_is_refused_at_installation() {
        let floor = QueryRouteLimits::response_floor();
        let mut statuses = [const { LaneStatus::Undeclared }; Lane::ORDER.len()];
        statuses[0] = LaneStatus::Complete;
        let envelope = json!({
            "kind": "fused",
            "degraded": false,
            "lanes": lanes_json(&statuses),
            "truncated": false,
            "entries": [],
        });
        assert_eq!(floor.get(), measure_json(&envelope).unwrap());
        let undeclared = lanes_json(&[const { LaneStatus::Undeclared }; Lane::ORDER.len()]);
        assert!(measure_json(&undeclared).unwrap() > measure_json(&envelope["lanes"]).unwrap());
        let limits = |response_bytes: NonZeroUsize| QueryRouteLimits {
            query_bytes: NonZeroUsize::new(64).unwrap(),
            probes: NonZeroUsize::new(4).unwrap(),
            lexical_scan_rows: NonZeroUsize::new(16).unwrap(),
            lexical_accepted: NonZeroUsize::new(8).unwrap(),
            validation_batch: NonZeroUsize::new(8).unwrap(),
            exact_page_rows: NonZeroUsize::new(4).unwrap(),
            exact_pages: NonZeroUsize::new(2).unwrap(),
            fused_union: NonZeroUsize::new(16).unwrap(),
            result_rows: NonZeroUsize::new(8).unwrap(),
            response_bytes,
            deadline_ceiling: Duration::from_secs(1),
            fusion: FusionParameters::new(
                retrieval::fusion::LaneWeights {
                    exact: 1.0,
                    lexical: 1.0,
                    dense: 1.0,
                },
                60.0,
            )
            .unwrap(),
            dense: None,
        };
        assert_eq!(limits(floor).validate(), Ok(()));
        let below = NonZeroUsize::new(floor.get() - 1).unwrap();
        assert_eq!(
            limits(below).validate(),
            Err(LimitsRefusal::ResponseBytes {
                value: below.get(),
                floor: floor.get(),
            })
        );
        let wire = NonZeroUsize::new(crate::dispatch::MAX_WIRE_BODY_BYTES).unwrap();
        assert_eq!(limits(wire).validate(), Ok(()));
        let over = NonZeroUsize::new(wire.get() + 1).unwrap();
        assert_eq!(
            limits(over).validate(),
            Err(LimitsRefusal::ResponseBytesOverWire {
                value: over.get(),
                max: wire.get(),
            })
        );
    }
}

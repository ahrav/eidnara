//! Payload bytes are never read here; packing consumes the ranking and materializes payloads under its own bounds.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use host_runtime::RouteHandle;
use kernel::source_identity::OCCURRENCE_ENCODING_VERSION;
use kernel::{ArtifactDestination, KernelError, KernelStore, MAX_ELIGIBILITY_CANDIDATES};
use retrieval::ProjectionError;
use retrieval::eligibility::{
    Authority, AuthorityMoved, Disposition, OccurrenceCandidate, judge_tracked,
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
}

impl QueryRouteLimits {
    /// Bytes of a fused answer with no entries and every lane undeclared; no `response_bytes` below it can hold any answer.
    pub fn response_floor() -> NonZeroUsize {
        let statuses = [const { LaneStatus::Undeclared }; Lane::ORDER.len()];
        let bytes = measure_json(&fused_envelope(&statuses, false, Vec::new(), false))
            .expect("the empty fused envelope measures");
        NonZeroUsize::new(bytes).expect("the empty fused envelope is not empty")
    }

    /// Refuses a limit the kernel's eligibility batch could never serve or a response bound no answer fits, so the refusal lands at installation instead of on every request.
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
    /// Both lanes' hits are judged by the kernel, after the projection connection is released.
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

fn exact_read(
    conn: &GuardedConn<'_>,
    kernel_incarnation_id: &str,
    intent: &Intent,
    limits: &QueryRouteLimits,
    budget: &SharedBudget,
) -> Result<LaneRead<ExactHits>, QueryFailure> {
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
    let bounds = LexicalBounds {
        max_input_bytes: limits.query_bytes,
        max_atoms: limits.probes,
    };
    let analysis = match analyze_segments(&segments, bounds) {
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
    Eligible(BTreeSet<OccurrenceId>),
    Moved(&'static str),
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
    let mut eligible = BTreeSet::new();
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
                return Ok(Judged::Moved("kernel_incarnation_changed"));
            }
            Some(AuthorityMoved::Snapshot) => return Ok(Judged::Moved("snapshot_changed")),
            None => {}
        }
        eligible.extend(
            batch
                .iter()
                .zip(report.occurrences)
                .filter(|(_, judged)| judged.disposition == Disposition::Eligible)
                .map(|((occurrence, _), _)| *occurrence),
        );
    }
    Ok(Judged::Eligible(eligible))
}

/// Judges the exact lane's rows under the request's authority before any position is assigned, so a row the caller may not see earns no position and consumes no union slot.
fn admit_exact(
    kernel: &KernelStore,
    authority: Authority<'_>,
    limits: &QueryRouteLimits,
    budget: &SharedBudget,
    read: LaneRead<ExactHits>,
    terms: &mut BTreeMap<OccurrenceId, OccurrenceCandidate>,
) -> Result<LaneOutput, QueryFailure> {
    let ExactHits { hits, status } = match read {
        LaneRead::Pending(hits) => hits,
        LaneRead::Ended(status) => {
            return Ok(LaneOutput {
                ranking: None,
                status,
            });
        }
    };
    let candidates: Vec<(OccurrenceId, &OccurrenceCandidate)> = hits
        .iter()
        .map(|scoped| (scoped.hit.occurrence, &scoped.candidate))
        .collect();
    let eligible = match judge_eligible(kernel, authority, limits, budget, &candidates, || {
        check(budget).map_err(Into::into)
    })? {
        Judged::Eligible(eligible) => eligible,
        Judged::Moved(reason) => return Ok(LaneOutput::unavailable(reason)),
        Judged::Kernel => return Ok(LaneOutput::unavailable("kernel")),
    };
    let mut ranked = Vec::new();
    for scoped in hits {
        if eligible.contains(&scoped.hit.occurrence) {
            terms.insert(scoped.hit.occurrence, scoped.candidate);
            ranked.push(scoped.hit);
        }
    }
    Ok(LaneOutput::ranked(Lane::Exact, ranked, status))
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
}

pub fn execute(
    projection: &SearchProjection,
    kernel: &KernelStore,
    authority: Authority<'_>,
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

    // Only projection statements run while the connection is held; every kernel reader is taken after it is released.
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
            Ok(Scanned { exact, lexical })
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
    if matches!(
        (&scanned.exact, &scanned.lexical),
        (
            LaneRead::Ended(LaneStatus::Undeclared),
            LaneRead::Ended(LaneStatus::Undeclared)
        )
    ) {
        return Err(QueryFailure::InvalidQuery(
            "the query yields no probe".to_string(),
        ));
    }

    before_phase(Phase::Admission);
    check(budget)?;
    let mut terms: BTreeMap<OccurrenceId, OccurrenceCandidate> = BTreeMap::new();
    let exact = admit_exact(kernel, authority, limits, budget, scanned.exact, &mut terms)?;
    let lexical = admit_lexical(kernel, authority, budget, scanned.lexical, &mut terms)?;
    if exact.ranking.is_none() && lexical.ranking.is_none() {
        return Err(QueryFailure::Unavailable("no_lane"));
    }
    let mut statuses = [const { LaneStatus::Undeclared }; Lane::ORDER.len()];
    for (lane, status) in [(Lane::Exact, exact.status), (Lane::Lexical, lexical.status)] {
        statuses[lane_slot(lane)] = status;
    }

    before_phase(Phase::Fusion);
    check(budget)?;
    let rankings = [exact.ranking, lexical.ranking].into_iter().flatten();
    let lanes =
        DeclaredLanes::admit(rankings).map_err(|_| QueryFailure::Unavailable("duplicate_lane"))?;
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
    let eligible = match judge_eligible(kernel, authority, limits, budget, &candidates, || {
        before_phase(Phase::Revalidation);
        check(budget).map_err(Into::into)
    })? {
        Judged::Eligible(eligible) => eligible,
        Judged::Moved(reason) => return Err(QueryFailure::Unavailable(reason)),
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
            harness: _,
        } = scope;
        let query = parsed.query;
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
        let envelope = json!({
            "kind": "fused",
            "degraded": false,
            "lanes": lanes_json(&[const { LaneStatus::Undeclared }; Lane::ORDER.len()]),
            "truncated": false,
            "entries": [],
        });
        assert_eq!(floor.get(), measure_json(&envelope).unwrap());
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
    }
}

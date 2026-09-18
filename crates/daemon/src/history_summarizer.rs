//! The writer persists transitions through idle, firing, awaiting_producer, validating, and publishing.
//! The writer verifies the pinned ordinal-range chunk fingerprint and fails on mismatch.
//! The publish transaction uses CAS gating and exposes its writes through the m1 watermark.
//! The next materializing pass exposes publish writes through the m1 watermark without mutating cached render state.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use memory_store::curator_jobs::{CuratorJobError, CuratorJobOutcome, CuratorJobRefusal};
use memory_store::{
    CuratorActivation, CuratorNonadmissionCode, HistorySegmentSetGeneration,
    HistorySummarizerChunkRange, HistorySummarizerDurableState, HistorySummarizerEventCandidate,
    HistorySummarizerPhase, HistorySummarizerPrimerCandidate, HistorySummarizerPublishError,
    HistorySummarizerPublishPredicate, HistorySummarizerPublishRequest,
    HistorySummarizerPublishResult, HistorySummarizerSelectedMessageIdentity,
    HistorySummarizerUserMemoryCandidate, MemoryStore, MemoryStoreError, PendingPublication,
    StoredHistorySegment,
};

use crate::curator::handoff::{
    self, Handoff, HandoffError, HandoffRequest, HandoffTarget, PreparedActivation,
};
use crate::history_summarizer_citations::{ExtractionOutcome, FrozenAliasTable};
use crate::history_summarizer_producer::{
    ErrorClass, ErrorClassification, HistorySummarizerProducer, HistorySummarizerProducerError,
    ProducerOutput, RunHandle, RunState, attach_cleanup,
};
use crate::history_summarizer_validate::{
    HistorySummarizerChunk, HistorySummarizerValidationError, StoredHistorySegmentRange,
    ValidateOptions, ValidatedChunk, ValidatedHistorySegment, validate_history_summarizer_output,
};

/// `HISTORY_SUMMARIZER_FAILURE_BACKOFF_MS` sets a 60-second cooldown after an abandoned history_summarizer firing.
pub const HISTORY_SUMMARIZER_FAILURE_BACKOFF_MS: i64 = 60_000;

const CHAIN_EXHAUSTED_PERMANENT_PREFIX: &str = "chain-exhausted-permanent:";
const AUTH_REQUIRED_PREFIX: &str = "auth-required:";
const UNKNOWN_ERROR_CLASS_PREFIX: &str = "unknown-error-class:";

fn to_stored_history_segment(
    c: &ValidatedHistorySegment,
    created_at_ms: i64,
    boundary_dates: &BTreeMap<String, String>,
) -> StoredHistorySegment {
    StoredHistorySegment {
        sequence: c.sequence as i64,
        start_message: c.start_message as i64,
        end_message: c.end_message as i64,
        start_message_id: c.start_message_id.clone(),
        end_message_id: c.end_message_id.clone(),
        start_date: boundary_dates.get(&c.start_message_id).cloned(),
        end_date: boundary_dates.get(&c.end_message_id).cloned(),
        title: c.title.clone(),
        content: c.content.clone(),
        p1: c.p1.clone(),
        p2: c.p2.clone(),
        p3: c.p3.clone(),
        p4: c.p4.clone(),
        // The validator captures any `\d+`; the documented range is 1 through
        // 100, so the value is clamped before it narrows to `i32`.
        importance: c
            .importance
            .map(|i| i32::try_from(i.clamp(1, 100)).unwrap_or(100))
            .unwrap_or(50),
        episode_type: c.episode_type.clone(),
        // Strict validation rejects tierless output.
        // P1 determines `legacy` so a validation bypass cannot mark a flat row as v2.
        legacy: if c.p1.as_deref().is_some_and(|p1| !p1.trim().is_empty()) {
            0
        } else {
            1
        },
        created_at: created_at_ms,
    }
}

fn source_history_segment(
    history_segments: &[crate::history_summarizer_validate::ValidatedHistorySegment],
    origin: Option<u64>,
) -> Option<&crate::history_summarizer_validate::ValidatedHistorySegment> {
    origin
        .and_then(|index| index.checked_sub(1))
        .and_then(|index| history_segments.get(index as usize))
}

fn to_store_event(
    event: &crate::history_summarizer_validate::ParsedEvent,
    history_segments: &[crate::history_summarizer_validate::ValidatedHistorySegment],
    created_at: i64,
) -> HistorySummarizerEventCandidate {
    HistorySummarizerEventCandidate {
        kind: event.kind.clone(),
        at_history_segment: event.at_history_segment,
        history_segment_id: source_history_segment(history_segments, event.at_history_segment)
            .map(|history_segment| history_segment.sequence),
        fields_json: serde_json::to_string(&event.fields).unwrap_or_else(|_| "{}".to_string()),
        created_at,
        harness: "module".to_string(),
    }
}

fn to_store_primer(
    candidate: &crate::history_summarizer_validate::PrimerCandidate,
    session_id: &str,
    project_path: &str,
    history_segments: &[crate::history_summarizer_validate::ValidatedHistorySegment],
    created_at: i64,
) -> HistorySummarizerPrimerCandidate {
    let source = source_history_segment(history_segments, candidate.origin_history_segment_index);
    let start = source.or_else(|| history_segments.first());
    let end = source.or_else(|| history_segments.last());
    HistorySummarizerPrimerCandidate {
        project_path: project_path.to_string(),
        session_id: session_id.to_string(),
        question: candidate.question.clone(),
        source_history_segment_start: start.map(|history_segment| history_segment.start_message),
        source_history_segment_end: end.map(|history_segment| history_segment.end_message),
        source_start_message_id: start
            .map(|history_segment| history_segment.start_message_id.clone())
            .unwrap_or_default(),
        source_end_message_id: end
            .map(|history_segment| history_segment.end_message_id.clone())
            .unwrap_or_default(),
        source_message_time: created_at,
        created_at,
    }
}

fn to_store_user_observation(
    observation: &crate::history_summarizer_validate::UserObservationCandidate,
    session_id: &str,
    history_segments: &[crate::history_summarizer_validate::ValidatedHistorySegment],
    created_at: i64,
) -> HistorySummarizerUserMemoryCandidate {
    let source = source_history_segment(history_segments, observation.origin_history_segment_index);
    let start = source.or_else(|| history_segments.first());
    let end = source.or_else(|| history_segments.last());
    HistorySummarizerUserMemoryCandidate {
        content: observation.content.clone(),
        session_id: session_id.to_string(),
        source_history_segment_start: start.map(|history_segment| history_segment.start_message),
        source_history_segment_end: end.map(|history_segment| history_segment.end_message),
        created_at,
    }
}

/// The snapshot fingerprint detects pinned-item insertion, removal, ID, kind, and byte-length changes.
/// The fingerprint records byte lengths rather than content bytes.
/// The fingerprint changes when chunk items are inserted, removed, or assigned different IDs or types.
/// The fingerprint ignores unrelated metadata drift and same-length content edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkSnapshotItem<'a> {
    pub id: &'a str,
    pub kind: &'a str,
    pub byte_len: usize,
}

/// The fingerprint omits content bytes so diagnostics retain item-level fields.
pub fn compute_chunk_fingerprint(items: &[ChunkSnapshotItem<'_>]) -> String {
    items
        .iter()
        .map(|item| format!("{}:{}:{}", item.id, item.kind, item.byte_len))
        .collect::<Vec<_>>()
        .join("|")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FireOutcome {
    Fired(HistorySummarizerDurableState),
    Busy(HistorySummarizerDurableState),
}

#[derive(Debug, thiserror::Error)]
pub enum HistorySummarizerStateError {
    #[error("history_summarizer invalid chunk range: from {from_ordinal} is after to {to_ordinal}")]
    InvalidRange { from_ordinal: u64, to_ordinal: u64 },
    #[error("history_summarizer invalid transition: event {event} cannot run from {from}", from = from.as_str())]
    InvalidTransition {
        from: HistorySummarizerPhase,
        event: &'static str,
    },
    #[error(
        "history_summarizer firing {firing_seq} is missing producer ids needed for reattach/publish"
    )]
    MissingProducerIds { firing_seq: u64 },
    #[error("history_summarizer chunk fingerprint mismatch: expected {expected}, found {found}")]
    FingerprintMismatch { expected: String, found: String },
    #[error("store: {0}")]
    Store(MemoryStoreError),
    #[error("publish: {0}")]
    Publish(HistorySummarizerPublishError),
}

impl From<MemoryStoreError> for HistorySummarizerStateError {
    fn from(e: MemoryStoreError) -> Self {
        HistorySummarizerStateError::Store(e)
    }
}

impl From<HistorySummarizerPublishError> for HistorySummarizerStateError {
    fn from(e: HistorySummarizerPublishError) -> Self {
        HistorySummarizerStateError::Publish(e)
    }
}

/// The fire operation enforces single-flight.
/// A non-idle phase returns `Busy` with the unchanged state.
#[allow(clippy::too_many_arguments)] // The durable firing snapshot carries each fence explicitly.
pub fn fire(
    current: &HistorySummarizerDurableState,
    from_ordinal: u64,
    to_ordinal: u64,
    chunk_fingerprint: String,
    selected_range_identities: Vec<HistorySummarizerSelectedMessageIdentity>,
    expected_revert_epoch: u64,
    history_segment_set_generation: HistorySegmentSetGeneration,
    fired_at_ms: i64,
) -> Result<FireOutcome, HistorySummarizerStateError> {
    if from_ordinal > to_ordinal {
        return Err(HistorySummarizerStateError::InvalidRange {
            from_ordinal,
            to_ordinal,
        });
    }
    if current.state != HistorySummarizerPhase::Idle {
        return Ok(FireOutcome::Busy(current.clone()));
    }

    Ok(FireOutcome::Fired(HistorySummarizerDurableState {
        state: HistorySummarizerPhase::Firing,
        firing_seq: current.firing_seq.saturating_add(1),
        chunk_range: Some(HistorySummarizerChunkRange {
            from_ordinal,
            to_ordinal,
        }),
        chunk_fingerprint,
        selected_range_identities,
        producer_session_id: None,
        producer_run_id: None,
        producer_harness: None,
        fired_at_ms: Some(fired_at_ms),
        expected_revert_epoch,
        history_segment_set_generation,
        failure_backoff_at_ms: current.failure_backoff_at_ms,
        last_failure: current.last_failure.clone(),
        // A fire clears the prior skip reason.
        last_no_fire: None,
        consecutive_publish_failures: current.consecutive_publish_failures,
        curator_nonadmission: current.curator_nonadmission,
        // A reservation left by an earlier firing is not this firing's to publish; its job stays a capped reservation the expiry sweep closes.
        curator_reservation: None,
    }))
}

pub fn producer_started(
    current: &HistorySummarizerDurableState,
    producer_session_id: String,
    producer_run_id: String,
    producer_harness: String,
) -> Result<HistorySummarizerDurableState, HistorySummarizerStateError> {
    require_phase(current, HistorySummarizerPhase::Firing, "producer_started")?;
    let mut next = current.clone();
    next.state = HistorySummarizerPhase::AwaitingProducer;
    next.producer_session_id = Some(producer_session_id);
    next.producer_run_id = Some(producer_run_id);
    // Recovery reattaches using the harness that started the run.
    // Run recovery scopes run identity by `(project_root, harness, session)`.
    next.producer_harness = Some(producer_harness);
    // Establishing a producer run clears failure detail and retry cooldown from the prior firing.
    next.failure_backoff_at_ms = None;
    next.last_failure = None;
    Ok(next)
}

pub fn output_received(
    current: &HistorySummarizerDurableState,
    _output_text: &str,
) -> Result<HistorySummarizerDurableState, HistorySummarizerStateError> {
    require_phase(
        current,
        HistorySummarizerPhase::AwaitingProducer,
        "output_received",
    )?;
    let mut next = current.clone();
    next.state = HistorySummarizerPhase::Validating;
    Ok(next)
}

pub fn validation_ok(
    current: &HistorySummarizerDurableState,
) -> Result<HistorySummarizerDurableState, HistorySummarizerStateError> {
    require_phase(current, HistorySummarizerPhase::Validating, "validation_ok")?;
    let mut next = current.clone();
    next.state = HistorySummarizerPhase::Publishing;
    Ok(next)
}

pub fn tx_committed(
    current: &HistorySummarizerDurableState,
) -> Result<HistorySummarizerDurableState, HistorySummarizerStateError> {
    require_phase(current, HistorySummarizerPhase::Publishing, "tx_committed")?;
    let mut next = current.cleared_of_in_flight_firing();
    next.consecutive_publish_failures = 0;
    Ok(next)
}

pub fn tx_conflict(
    current: &HistorySummarizerDurableState,
    failure_backoff_at_ms: i64,
) -> Result<HistorySummarizerDurableState, HistorySummarizerStateError> {
    require_phase(current, HistorySummarizerPhase::Publishing, "tx_conflict")?;
    Ok(abandon(current, failure_backoff_at_ms))
}

/// The state machine releases the single-flight lease after a terminal, missing, or expired producer, validation rejection, or stale snapshot.
/// The state machine retains the failed firing sequence after releasing the lease.
/// The next fire increments the retained firing sequence.
pub fn abandon(
    current: &HistorySummarizerDurableState,
    failure_backoff_at_ms: i64,
) -> HistorySummarizerDurableState {
    abandon_with_detail(current, failure_backoff_at_ms, None)
}

/// Durable state preserves failure detail independently of spawned-task stderr.
/// Without durable failure detail, a state dump cannot distinguish connect or bind failures from other failures.
pub fn abandon_with_detail(
    current: &HistorySummarizerDurableState,
    failure_backoff_at_ms: i64,
    detail: Option<String>,
) -> HistorySummarizerDurableState {
    HistorySummarizerDurableState {
        state: HistorySummarizerPhase::Idle,
        firing_seq: current.firing_seq,
        failure_backoff_at_ms: Some(failure_backoff_at_ms),
        last_failure: detail.or_else(|| current.last_failure.clone()),
        consecutive_publish_failures: current.consecutive_publish_failures,
        curator_nonadmission: current.curator_nonadmission,
        curator_reservation: current.curator_reservation.clone(),
        ..HistorySummarizerDurableState::default()
    }
}

/// Keeps a Publishing firing that holds a Curator reservation in place with the failure recorded, so recovery reconciles it against the reservation instead of refiring the model.
pub fn retain_with_detail(
    current: &HistorySummarizerDurableState,
    failure_backoff_at_ms: i64,
    detail: Option<String>,
) -> HistorySummarizerDurableState {
    let mut next = retain_backoff(current, failure_backoff_at_ms, detail);
    next.consecutive_publish_failures = current.consecutive_publish_failures.saturating_add(1);
    next
}

/// Arms the backoff without incrementing `consecutive_publish_failures`, for a refusal the store already counted.
fn retain_backoff(
    current: &HistorySummarizerDurableState,
    failure_backoff_at_ms: i64,
    detail: Option<String>,
) -> HistorySummarizerDurableState {
    let mut next = current.clone();
    next.failure_backoff_at_ms = Some(failure_backoff_at_ms);
    next.last_failure = detail.or_else(|| current.last_failure.clone());
    next
}

/// Whether a state's recorded reservation belongs to its own firing; a reservation carried from an earlier firing is not one this firing can publish.
pub(crate) trait HoldsReservation {
    fn holds_reservation(&self) -> bool;
}

impl HoldsReservation for HistorySummarizerDurableState {
    fn holds_reservation(&self) -> bool {
        self.curator_reservation
            .as_ref()
            .is_some_and(|held| held.firing_seq == self.firing_seq)
    }
}

pub fn verify_chunk_fingerprint(
    expected: &str,
    observed: &str,
) -> Result<(), HistorySummarizerStateError> {
    if expected == observed {
        Ok(())
    } else {
        Err(HistorySummarizerStateError::FingerprintMismatch {
            expected: expected.to_string(),
            found: observed.to_string(),
        })
    }
}

pub fn publish_predicate(
    state: &HistorySummarizerDurableState,
) -> Result<HistorySummarizerPublishPredicate, HistorySummarizerStateError> {
    let Some(producer_run_id) = state.producer_run_id.clone() else {
        return Err(HistorySummarizerStateError::MissingProducerIds {
            firing_seq: state.firing_seq,
        });
    };
    Ok(HistorySummarizerPublishPredicate {
        firing_seq: state.firing_seq,
        producer_run_id,
        chunk_fingerprint: state.chunk_fingerprint.clone(),
        selected_range_identities: state.selected_range_identities.clone(),
        history_segment_set_generation: state.history_segment_set_generation,
    })
}

pub fn persist_history_summarizer_state(
    store: &MemoryStore,
    session_id: &str,
    next_state: HistorySummarizerDurableState,
) -> Result<u64, HistorySummarizerStateError> {
    let loaded = store.load(session_id)?;
    let mut meta = loaded.meta.clone();
    meta.history_summarizer = next_state;
    if meta == loaded.meta {
        return Ok(loaded.row_version.unwrap_or(0));
    }
    Ok(store.commit(session_id, loaded.row_version, &loaded.core, &meta)?)
}

pub trait HistorySummarizerPublicationFence: Send + Sync {
    fn publish(
        &self,
        store: &MemoryStore,
        request: HistorySummarizerPublishRequest<'_>,
    ) -> Result<HistorySummarizerPublishResult, HistorySummarizerPublishError>;
}

pub struct ValidatedPublishRequest<'a> {
    pub session_id: &'a str,
    pub project_path: &'a str,
    pub expected_row_version: Option<u64>,
    pub expected_revert_epoch: u64,
    pub predicate: &'a HistorySummarizerPublishPredicate,
    pub observed_chunk_fingerprint: &'a str,
    pub validated: &'a ValidatedChunk,
    /// User observations are stored only when the privacy collection gate is enabled.
    pub collect_user_memory_candidates: bool,
    pub publication_floor_ordinal: u64,
    pub chunk_transcript: &'a str,

    pub created_at_ms: i64,
    /// `boundary_dates` maps native message IDs to YYYY-MM-DD dates; absent IDs have no date.
    pub boundary_dates: &'a BTreeMap<String, String>,
    pub failure_backoff_at_ms: i64,
    pub publication_fence: Option<&'a dyn HistorySummarizerPublicationFence>,
    /// Q31: why this firing's fact candidates were not admitted to Curator review, committed with the history. The caller decides it because the answer depends on what happened before publication (validation verdict, reservation refusal), not on the validated chunk alone.
    pub curator_nonadmission: Option<CuratorNonadmissionCode>,
    /// KTD3: the reserved and staged job this publication activates with its history.
    pub curator_activation: Option<&'a PreparedActivation>,
}

/// The commit re-checks the chunk fingerprint and abandons the matching firing before returning `HistorySummarizerStateError::FingerprintMismatch`.
/// Abandoning the matching firing prevents a stale producer from blocking a future fire.
///
/// Facts promote through additive inserts rather than updates.
/// A publish surfaces only on the next materializing pass.
/// HistorySegment and memory watermarks surface published facts without mutating cached render state.
pub fn publish_validated_chunk(
    store: &MemoryStore,
    request: ValidatedPublishRequest<'_>,
) -> Result<HistorySummarizerPublishResult, HistorySummarizerStateError> {
    if request.predicate.chunk_fingerprint != request.observed_chunk_fingerprint {
        // The chunk itself changed: a reservation for it can never publish.
        settle_unpublishable_reservation(store, &request)?;
        abandon_matching_run_with_detail(
            store,
            request.session_id,
            request.predicate,
            request.failure_backoff_at_ms,
            None,
        )?;
        return Err(HistorySummarizerStateError::FingerprintMismatch {
            expected: request.predicate.chunk_fingerprint.clone(),
            found: request.observed_chunk_fingerprint.to_string(),
        });
    }

    let history_segments: Vec<StoredHistorySegment> = request
        .validated
        .history_segments
        .iter()
        .map(|c| to_stored_history_segment(c, request.created_at_ms, request.boundary_dates))
        .collect();
    let events: Vec<HistorySummarizerEventCandidate> = request
        .validated
        .events
        .iter()
        .map(|event| {
            to_store_event(
                event,
                &request.validated.history_segments,
                request.created_at_ms,
            )
        })
        .collect();
    let primer_candidates: Vec<HistorySummarizerPrimerCandidate> = request
        .validated
        .primer_candidates
        .iter()
        .map(|candidate| {
            to_store_primer(
                candidate,
                request.session_id,
                request.project_path,
                &request.validated.history_segments,
                request.created_at_ms,
            )
        })
        .collect();
    let user_memory_candidates: Vec<HistorySummarizerUserMemoryCandidate> =
        if request.collect_user_memory_candidates {
            request
                .validated
                .user_observations
                .iter()
                .map(|observation| {
                    to_store_user_observation(
                        observation,
                        request.session_id,
                        &request.validated.history_segments,
                        request.created_at_ms,
                    )
                })
                .collect()
        } else {
            Vec::new()
        };

    let publish_request = HistorySummarizerPublishRequest {
        session_id: request.session_id,
        expected_row_version: request.expected_row_version,
        expected_revert_epoch: request.expected_revert_epoch,
        predicate: request.predicate,
        project_path: request.project_path,
        history_segments: &history_segments,
        events: &events,
        primer_candidates: &primer_candidates,
        user_memory_candidates: &user_memory_candidates,
        publication_floor_ordinal: request.publication_floor_ordinal,
        chunk_transcript: Some(request.chunk_transcript),
        curator_nonadmission: request.curator_nonadmission,
        curator_activation: request
            .curator_activation
            .map(|prepared| CuratorActivation {
                causal_identity: &prepared.causal_identity,
                producer: &prepared.producer,
                input: &prepared.input,
                now_ms: request.created_at_ms,
            }),
    };
    let publish_result = match request.publication_fence {
        Some(fence) => fence.publish(store, publish_request),
        None => store.publish_history_summarizer_chunk(publish_request),
    };
    match publish_result {
        Ok(result) => Ok(result),
        Err(HistorySummarizerPublishError::FenceRejected { reason }) => {
            // The store's fence refused the firing's snapshot: the selected input or the segment set changed under it. A reservation it carried can never publish, and the run returns to `Idle` without a failure cooldown.
            settle_unpublishable_reservation(store, &request)?;
            abandon_matching_run_without_cooldown(
                store,
                request.session_id,
                request.predicate,
                Some(format!("publish rejected: {reason}")),
            )?;
            Err(HistorySummarizerStateError::Publish(
                HistorySummarizerPublishError::FenceRejected { reason },
            ))
        }
        Err(HistorySummarizerPublishError::CallerFenceRejected { reason }) => {
            // The caller's own fence (a retired transform snapshot) refused before any write. Without a reservation the run returns to `Idle` for an immediate retry with a fresh snapshot; with one it stays in `Publishing` so recovery republishes the retained output.
            if request.curator_activation.is_some() {
                let _ = store.record_history_summarizer_publish_failure_if_matching(
                    request.session_id,
                    request.predicate,
                );
            } else {
                abandon_matching_run_without_cooldown(
                    store,
                    request.session_id,
                    request.predicate,
                    Some(format!("publish rejected: {reason}")),
                )?;
            }
            Err(HistorySummarizerStateError::Publish(
                HistorySummarizerPublishError::CallerFenceRejected { reason },
            ))
        }
        Err(error @ HistorySummarizerPublishError::HistorySegmentOverlap { .. }) => {
            // Storage-overlap handling treats a storage-detected overlap as a stale local race.
            // A storage-detected overlap makes the matching firing immediately idle.
            // Making the matching firing idle prevents a durable `Publishing` wedge.
            settle_unpublishable_reservation(store, &request)?;
            abandon_matching_run_without_cooldown(
                store,
                request.session_id,
                request.predicate,
                Some(format!("publish rejected: {error}")),
            )?;
            Err(HistorySummarizerStateError::Publish(error))
        }
        Err(HistorySummarizerPublishError::CasConflict {
            expected,
            found,
            reason,
        }) => {
            // A reasoned conflict is a re-cut session, which no retry can publish into; a bare row-version conflict is another writer's ordinary commit, which a reserved firing retries.
            if request.curator_activation.is_some() && reason.is_none() {
                let _ = store.record_history_summarizer_publish_failure_if_matching(
                    request.session_id,
                    request.predicate,
                );
            } else {
                let detail = reason
                    .clone()
                    .map(|reason| format!("publish rejected: {reason}"))
                    .or_else(|| Some("publish rejected: row-version CAS conflict".to_string()));
                settle_unpublishable_reservation(store, &request)?;
                abandon_matching_run_with_detail(
                    store,
                    request.session_id,
                    request.predicate,
                    request.failure_backoff_at_ms,
                    detail,
                )?;
            }
            Err(HistorySummarizerStateError::Publish(
                HistorySummarizerPublishError::CasConflict {
                    expected,
                    found,
                    reason,
                },
            ))
        }
        Err(err) => {
            // A publish error occurs after the producer completes its work.
            // The publish-error handler leaves the producer run available for normal recovery instead of abandoning it.
            // The publisher records publish failures so repeated publication errors remain visible.
            let _ = store.record_history_summarizer_publish_failure_if_matching(
                request.session_id,
                request.predicate,
            );
            Err(err.into())
        }
    }
}

/// A fence refused the history this reservation's candidate was extracted from (the selected input changed, or a competing publication moved the session on), so the job can never activate: the reservation and its retained publication are dropped, and the job is finished as not admitted, or as expired when its deadline has passed. Nothing advances, and nothing is rerun. The drop is fenced on the reservation itself, so a late duplicate attempt finds the state owned by a later firing or another publication and touches neither it nor the job it activated. It runs before the firing is abandoned: a firing that is idle never holds a reservation, and a reservation that is gone leaves at most a job the expiry sweep closes.
fn settle_unpublishable_reservation(
    store: &MemoryStore,
    request: &ValidatedPublishRequest<'_>,
) -> Result<(), HistorySummarizerStateError> {
    let Some(activation) = request.curator_activation else {
        return Ok(());
    };
    let loaded = store.load(request.session_id)?;
    let Some(held) = loaded.meta.history_summarizer.curator_reservation.as_ref() else {
        return Ok(());
    };
    if held.causal_identity != activation.causal_identity
        || store
            .clear_curator_reservation(request.session_id, held)?
            .is_none()
    {
        return Ok(());
    }
    let outcome = if held.queue_deadline_ms <= request.created_at_ms {
        CuratorJobOutcome::Expired
    } else {
        CuratorJobOutcome::Nonadmitted
    };
    match store.finish_curator_job(
        request.project_path,
        &activation.causal_identity,
        outcome,
        request.created_at_ms,
    ) {
        Ok(_) | Err(CuratorJobError::Refused(CuratorJobRefusal::Terminal)) => Ok(()),
        Err(CuratorJobError::Refused(refusal)) => Err(HistorySummarizerStateError::Publish(
            HistorySummarizerPublishError::CuratorActivation(refusal),
        )),
        Err(CuratorJobError::Store(error)) => Err(error.into()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestartAction {
    Done,
    ReattachProducer {
        producer_session_id: String,
        producer_run_id: String,
        /// The durable state records the harness under which the run started.
        /// Reattach uses the durable state's harness binding, not the resuming route's binding.
        /// ModelExecution scopes run identity by project root, durable harness, and session.
        producer_harness: Option<String>,
        firing_seq: u64,
        chunk_fingerprint: String,
    },
    AbandonedAndRefireEligible {
        firing_seq: u64,
    },
    /// The firing reserved a Curator job and retained its publication; recovery republishes it locally under the same fences instead of refiring.
    RepublishReserved {
        firing_seq: u64,
    },
}

/// After restart, a committed publish loads as idle and returns `Done`.
/// A remaining `Publishing` row means the publish transaction did not commit.
/// Restart recovery abandons an uncommitted stale single-flight so a future eligible trigger can refire.
pub fn handle_restart_load(
    store: &MemoryStore,
    session_id: &str,
    failure_backoff_at_ms: i64,
) -> Result<RestartAction, HistorySummarizerStateError> {
    let loaded = store.load(session_id)?;
    let state = loaded.meta.history_summarizer.clone();
    match state.state {
        HistorySummarizerPhase::Idle => Ok(RestartAction::Done),
        HistorySummarizerPhase::AwaitingProducer => {
            let (Some(producer_session_id), Some(producer_run_id)) = (
                state.producer_session_id.clone(),
                state.producer_run_id.clone(),
            ) else {
                let next = abandon(&state, failure_backoff_at_ms);
                persist_history_summarizer_state(store, session_id, next)?;
                return Ok(RestartAction::AbandonedAndRefireEligible {
                    firing_seq: state.firing_seq,
                });
            };
            Ok(RestartAction::ReattachProducer {
                producer_session_id,
                producer_run_id,
                producer_harness: state.producer_harness.clone(),
                firing_seq: state.firing_seq,
                chunk_fingerprint: state.chunk_fingerprint,
            })
        }
        HistorySummarizerPhase::Publishing if state.holds_reservation() => {
            Ok(RestartAction::RepublishReserved {
                firing_seq: state.firing_seq,
            })
        }
        HistorySummarizerPhase::Firing
        | HistorySummarizerPhase::Validating
        | HistorySummarizerPhase::Publishing => {
            let firing_seq = state.firing_seq;
            let next = abandon(&state, failure_backoff_at_ms);
            persist_history_summarizer_state(store, session_id, next)?;
            Ok(RestartAction::AbandonedAndRefireEligible { firing_seq })
        }
    }
}

pub struct RepublishRequest<'a> {
    pub store: &'a MemoryStore,
    pub session_id: &'a str,
    pub project_path: &'a str,
    pub curator_handoff: Option<&'a HandoffTarget>,
    pub now_ms: i64,
    pub failure_backoff_at_ms: i64,
    pub publication_fence: Option<&'a dyn HistorySummarizerPublicationFence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepublishOutcome {
    /// The retained publication committed, activating the job or recording its expiry.
    Published,
    /// The retained publication can never commit; the job is finished with a terminal outcome and the run is idle with nothing advanced.
    Settled,
    /// Nothing could be decided this pass (the Kernel is unavailable, the session row moved, the snapshot retired, or another pass settled or moved the session on first); the reservation, if it is still this firing's, waits for the next pass or its deadline.
    Retained,
}

/// Republishes a reserved firing's retained output without a model run. The reservation is reused as recorded, the subject is restaged or read back (or, past the deadline, nothing is staged and the publication records the expiry), and the publication runs under the fences the firing snapshot fixed. Every refusal is classified: a store fence, an unreadable or diverged retained output, or a proof the job cannot activate settles the reservation with a terminal outcome; a transient condition retains it with the backoff armed, so the next reattach waits instead of repeating the pass.
pub fn republish_reserved(
    request: RepublishRequest<'_>,
) -> Result<RepublishOutcome, HistorySummarizerDriveError> {
    let RepublishRequest {
        store,
        session_id,
        project_path,
        curator_handoff,
        now_ms,
        failure_backoff_at_ms,
        publication_fence,
    } = request;
    let loaded = store.load(session_id)?;
    let publishing = loaded.meta.history_summarizer.clone();
    require_phase(&publishing, HistorySummarizerPhase::Publishing, "republish")?;
    let invalid = || HistorySummarizerStateError::InvalidTransition {
        from: publishing.state.clone(),
        event: "republish",
    };
    let (Some(reservation), Some(row_version)) = (
        publishing
            .curator_reservation
            .clone()
            .filter(|_| publishing.holds_reservation()),
        loaded.row_version,
    ) else {
        return Err(invalid().into());
    };
    let settle = |detail: &str| {
        settle_republish(
            store,
            session_id,
            project_path,
            &publishing,
            now_ms,
            failure_backoff_at_ms,
            detail,
        )
    };
    let expired = reservation.queue_deadline_ms <= now_ms;
    // Before the deadline the subject must be verified through the Kernel, so without a target nothing can be decided and the payload is not worth reading.
    let target = match (expired, curator_handoff) {
        (true, _) => None,
        (false, Some(target)) => Some(target),
        (false, None) => {
            return retain_republish(store, session_id, |current| {
                retain_with_detail(
                    current,
                    failure_backoff_at_ms,
                    Some("curator republish: no Curator handoff target".to_string()),
                )
            });
        }
    };
    // The retained output must be this firing's and must still read as the types this daemon publishes; anything else can never publish.
    let pending = match store.load_pending_publication(session_id) {
        Ok(Some((firing_seq, pending))) if firing_seq == reservation.firing_seq => pending,
        Ok(_) => return settle("no retained publication for the reservation"),
        Err(MemoryStoreError::Serde(_)) => {
            return settle("the retained publication is unreadable");
        }
        Err(error) => return Err(HistorySummarizerStateError::Store(error).into()),
    };
    let (Ok(validated), Ok(aliases)) = (
        serde_json::from_str::<ValidatedChunk>(&pending.validated_json),
        serde_json::from_str::<FrozenAliasTable>(&pending.aliases_json),
    ) else {
        return settle("the retained publication is unreadable");
    };
    let plan = match target {
        None => handoff::expired_activation(
            store,
            project_path,
            session_id,
            &publishing,
            &reservation,
            row_version,
        )
        .map(Box::new),
        Some(target) => {
            // The reservation names its subject by digest and Kernel incarnation. A retained output that reproduces neither would reserve a second job under a name the publication refuses, so it is settled before anything is reserved or staged.
            let reproduces_reservation = target.kernel_incarnation
                == reservation.kernel_incarnation
                && handoff::subject_payload(&validated.facts, &aliases)
                    .is_ok_and(|(_, digest)| digest == reservation.payload_digest);
            if !reproduces_reservation {
                return settle("the retained output no longer names the reserved subject");
            }
            match handoff::reserve_and_stage(
                target,
                &HandoffRequest {
                    store,
                    project: project_path,
                    session_id,
                    firing: &publishing,
                    facts: &validated.facts,
                    aliases: &aliases,
                    now_ms,
                },
                |_| Ok(row_version),
            ) {
                Ok(Handoff::Activate(prepared)) => Ok(prepared),
                // A matching reservation is reused, never reserved again, so a decision without an activation proves nothing about the job and is retried.
                Ok(Handoff::Settled | Handoff::Nonadmission(_)) => {
                    return retain_republish(store, session_id, |current| {
                        retain_with_detail(
                            current,
                            failure_backoff_at_ms,
                            Some(
                                "curator republish: the handoff did not reuse the reservation"
                                    .to_string(),
                            ),
                        )
                    });
                }
                Err(error) => Err(error),
            }
        }
    };
    let prepared = match plan {
        Ok(prepared) => prepared,
        // The reservation's own job is gone: nothing to publish against.
        Err(HandoffError::Reserve(CuratorJobError::Refused(CuratorJobRefusal::Missing))) => {
            return settle("the reservation no longer names a publishable job");
        }
        // Staging or reading back failed for a reason a later pass may not see again.
        Err(error) => {
            return retain_republish(store, session_id, |current| {
                retain_with_detail(
                    current,
                    failure_backoff_at_ms,
                    Some(format!("curator republish: {error}")),
                )
            });
        }
    };
    let predicate = publish_predicate(&publishing)?;
    match publish_validated_chunk(
        store,
        ValidatedPublishRequest {
            session_id,
            project_path,
            expected_row_version: Some(row_version),
            expected_revert_epoch: publishing.expected_revert_epoch,
            predicate: &predicate,
            observed_chunk_fingerprint: &publishing.chunk_fingerprint,
            validated: &validated,
            collect_user_memory_candidates: pending.collect_user_memory_candidates,
            publication_floor_ordinal: pending.publication_floor_ordinal,
            chunk_transcript: &pending.chunk_transcript,
            boundary_dates: &pending.boundary_dates,
            created_at_ms: now_ms,
            failure_backoff_at_ms,
            publication_fence,
            curator_nonadmission: None,
            curator_activation: Some(&prepared),
        },
    ) {
        Ok(_) => Ok(RepublishOutcome::Published),
        // The publication settled the reservation itself: the input changed, the segment set moved on, or the session was re-cut.
        Err(HistorySummarizerStateError::Publish(
            HistorySummarizerPublishError::FenceRejected { .. }
            | HistorySummarizerPublishError::HistorySegmentOverlap { .. }
            | HistorySummarizerPublishError::CasConflict {
                reason: Some(_), ..
            },
        )) => Ok(RepublishOutcome::Settled),
        // The row moved or the snapshot retired under this pass; the store counted the refusal, and the next pass tries again after the backoff.
        Err(HistorySummarizerStateError::Publish(
            error @ (HistorySummarizerPublishError::CallerFenceRejected { .. }
            | HistorySummarizerPublishError::CasConflict { reason: None, .. }),
        )) => retain_republish(store, session_id, |current| {
            retain_backoff(
                current,
                failure_backoff_at_ms,
                Some(format!("curator republish: {error}")),
            )
        }),
        // The job itself refuses to activate: the reservation is settled with a terminal outcome.
        Err(HistorySummarizerStateError::Publish(
            HistorySummarizerPublishError::CuratorActivation(
                CuratorJobRefusal::Terminal
                | CuratorJobRefusal::NotReserved
                | CuratorJobRefusal::ProducerMismatch
                | CuratorJobRefusal::InvalidRequest
                | CuratorJobRefusal::Missing,
            ),
        )) => settle("the job refused activation"),
        Err(error) => Err(error.into()),
    }
}

/// Leaves a firing that still holds its reservation in Publishing as `next` describes it, so the next reattach finds the backoff it armed.
fn retain_republish(
    store: &MemoryStore,
    session_id: &str,
    next: impl FnOnce(&HistorySummarizerDurableState) -> HistorySummarizerDurableState,
) -> Result<RepublishOutcome, HistorySummarizerDriveError> {
    let current = store.load(session_id)?.meta.history_summarizer;
    if current.state == HistorySummarizerPhase::Publishing && current.holds_reservation() {
        persist_history_summarizer_state(store, session_id, next(&current))?;
    }
    Ok(RepublishOutcome::Retained)
}

/// Finishes a reservation whose retained publication can never commit: the reservation and its retained publication are dropped, the job records a terminal outcome (expired past its deadline, not admitted otherwise), and the firing is abandoned with nothing advanced. The drop is fenced on the reservation `publishing` snapshotted; a state that no longer records it was moved on by another pass, whose firing, retained publication, and job are left alone.
fn settle_republish(
    store: &MemoryStore,
    session_id: &str,
    project_path: &str,
    publishing: &HistorySummarizerDurableState,
    now_ms: i64,
    failure_backoff_at_ms: i64,
    detail: &str,
) -> Result<RepublishOutcome, HistorySummarizerDriveError> {
    if let Some(held) = publishing.curator_reservation.as_ref() {
        if store.clear_curator_reservation(session_id, held)?.is_none() {
            return Ok(RepublishOutcome::Retained);
        }
        let outcome = if held.queue_deadline_ms <= now_ms {
            CuratorJobOutcome::Expired
        } else {
            CuratorJobOutcome::Nonadmitted
        };
        match store.finish_curator_job(project_path, &held.causal_identity, outcome, now_ms) {
            Ok(_)
            | Err(CuratorJobError::Refused(
                CuratorJobRefusal::Terminal | CuratorJobRefusal::Missing,
            )) => {}
            Err(CuratorJobError::Refused(refusal)) => {
                return Err(HistorySummarizerStateError::Publish(
                    HistorySummarizerPublishError::CuratorActivation(refusal),
                )
                .into());
            }
            Err(CuratorJobError::Store(error)) => {
                return Err(HistorySummarizerStateError::Store(error).into());
            }
        }
    }
    let current = store.load(session_id)?.meta.history_summarizer;
    if current.state == HistorySummarizerPhase::Publishing
        && current.firing_seq == publishing.firing_seq
    {
        persist_history_summarizer_state(
            store,
            session_id,
            abandon_with_detail(
                &current,
                failure_backoff_at_ms,
                Some(format!("curator reservation settled: {detail}")),
            ),
        )?;
    }
    Ok(RepublishOutcome::Settled)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistorySummarizerRunSuccess {
    pub row_version: u64,
    pub producer_session_id: String,
    pub producer_run_id: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistorySummarizerDriveOutcome {
    Completed(HistorySummarizerRunSuccess),
    Busy(Box<HistorySummarizerDurableState>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistorySummarizerReattachOutcome {
    Done,
    Published(HistorySummarizerRunSuccess),
    RefireEligible {
        firing_seq: u64,
    },
    /// A reserved firing's retained publication was reconciled without a model run.
    Republished(RepublishOutcome),
}

#[derive(Debug)]
pub enum HistorySummarizerDriveError {
    NoModels,
    State(HistorySummarizerStateError),
    Producer(HistorySummarizerProducerError),
    ProducerConnect {
        source: Box<HistorySummarizerProducerError>,
        backoff_error: Option<Box<MemoryStoreError>>,
    },
    Validation(HistorySummarizerValidationError),
    /// The Curator reservation exists but staging or sealing failed; nothing was published and the reservation is retained.
    CuratorHandoff(HandoffError),
    /// The session was deleted while the firing ran; the producer was dropped mid-chain and
    /// nothing was published.
    Cancelled,
}

impl fmt::Display for HistorySummarizerDriveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HistorySummarizerDriveError::NoModels => {
                write!(f, "history_summarizer model chain is empty")
            }
            HistorySummarizerDriveError::State(e) => write!(f, "state: {e}"),
            HistorySummarizerDriveError::CuratorHandoff(e) => write!(f, "curator handoff: {e}"),
            HistorySummarizerDriveError::Producer(e) => write!(f, "producer: {e}"),
            HistorySummarizerDriveError::ProducerConnect {
                source,
                backoff_error: Some(error),
            } => write!(
                f,
                "producer connect: {source}; durable backoff could not be recorded: {error}"
            ),
            HistorySummarizerDriveError::ProducerConnect {
                source,
                backoff_error: None,
            } => write!(f, "producer connect: {source}"),
            HistorySummarizerDriveError::Validation(e) => write!(f, "validation: {e}"),
            HistorySummarizerDriveError::Cancelled => write!(f, "cancelled: session deleted"),
        }
    }
}

impl std::error::Error for HistorySummarizerDriveError {}

impl From<HistorySummarizerStateError> for HistorySummarizerDriveError {
    fn from(e: HistorySummarizerStateError) -> Self {
        HistorySummarizerDriveError::State(e)
    }
}

impl From<HistorySummarizerProducerError> for HistorySummarizerDriveError {
    fn from(e: HistorySummarizerProducerError) -> Self {
        HistorySummarizerDriveError::Producer(e)
    }
}

impl From<HistorySummarizerValidationError> for HistorySummarizerDriveError {
    fn from(e: HistorySummarizerValidationError) -> Self {
        HistorySummarizerDriveError::Validation(e)
    }
}

impl From<MemoryStoreError> for HistorySummarizerDriveError {
    fn from(e: MemoryStoreError) -> Self {
        HistorySummarizerDriveError::State(HistorySummarizerStateError::Store(e))
    }
}

#[async_trait::async_trait]
pub trait HistorySummarizerProducerDriver: Send {
    async fn bind_session(
        &mut self,
        session_id: &str,
    ) -> Result<(), HistorySummarizerProducerError>;
    async fn start(
        &mut self,
        session_id: &str,
        system: &str,
        prompt: &str,
        model: &str,
    ) -> Result<RunHandle, HistorySummarizerProducerError>;
    async fn start_with_generation(
        &mut self,
        session_id: &str,
        system: &str,
        prompt: &str,
        model: &str,
        _max_output_tokens: u32,
        _temperature: f64,
    ) -> Result<RunHandle, HistorySummarizerProducerError> {
        self.start(session_id, system, prompt, model).await
    }
    async fn await_output(
        &mut self,
        run_id: &str,
    ) -> Result<ProducerOutput, HistorySummarizerProducerError>;
    async fn await_output_with_timeout(
        &mut self,
        run_id: &str,
        _timeout: Duration,
    ) -> Result<ProducerOutput, HistorySummarizerProducerError> {
        self.await_output(run_id).await
    }
    async fn redrain_output(
        &mut self,
        run_id: &str,
    ) -> Result<ProducerOutput, HistorySummarizerProducerError> {
        self.await_output(run_id).await
    }
    async fn redrain_output_with_timeout(
        &mut self,
        run_id: &str,
        _timeout: Duration,
    ) -> Result<ProducerOutput, HistorySummarizerProducerError> {
        self.redrain_output(run_id).await
    }
    async fn status(&mut self, run_id: &str) -> Result<RunState, HistorySummarizerProducerError>;
    async fn cancel(&mut self, run_id: &str) -> Result<(), HistorySummarizerProducerError>;
    async fn close_attempt(&mut self) -> Result<(), HistorySummarizerProducerError> {
        self.close().await
    }
    async fn close(&mut self) -> Result<(), HistorySummarizerProducerError>;
    /// Terminal-path cleanup deletes the provider session; the default calls `close()`.
    /// Implementations that retain provider session data must delete it before closing.
    async fn purge_session(
        &mut self,
        _session_id: &str,
    ) -> Result<(), HistorySummarizerProducerError> {
        self.close().await
    }
}

#[async_trait::async_trait]
impl HistorySummarizerProducerDriver for HistorySummarizerProducer {
    async fn bind_session(
        &mut self,
        session_id: &str,
    ) -> Result<(), HistorySummarizerProducerError> {
        HistorySummarizerProducer::bind_session(self, session_id.to_string());
        Ok(())
    }

    async fn start(
        &mut self,
        session_id: &str,
        system: &str,
        prompt: &str,
        model: &str,
    ) -> Result<RunHandle, HistorySummarizerProducerError> {
        HistorySummarizerProducer::start(self, session_id, system, prompt, model).await
    }

    async fn start_with_generation(
        &mut self,
        session_id: &str,
        system: &str,
        prompt: &str,
        model: &str,
        max_output_tokens: u32,
        temperature: f64,
    ) -> Result<RunHandle, HistorySummarizerProducerError> {
        HistorySummarizerProducer::start_with_generation(
            self,
            session_id,
            system,
            prompt,
            model,
            max_output_tokens,
            temperature,
        )
        .await
    }

    async fn await_output(
        &mut self,
        run_id: &str,
    ) -> Result<ProducerOutput, HistorySummarizerProducerError> {
        HistorySummarizerProducer::await_output(self, run_id).await
    }

    async fn redrain_output(
        &mut self,
        run_id: &str,
    ) -> Result<ProducerOutput, HistorySummarizerProducerError> {
        HistorySummarizerProducer::redrain_output(self, run_id).await
    }

    async fn await_output_with_timeout(
        &mut self,
        run_id: &str,
        timeout: Duration,
    ) -> Result<ProducerOutput, HistorySummarizerProducerError> {
        HistorySummarizerProducer::await_output_with_timeout(self, run_id, timeout).await
    }

    async fn redrain_output_with_timeout(
        &mut self,
        run_id: &str,
        timeout: Duration,
    ) -> Result<ProducerOutput, HistorySummarizerProducerError> {
        HistorySummarizerProducer::redrain_output_with_timeout(self, run_id, timeout).await
    }

    async fn status(&mut self, run_id: &str) -> Result<RunState, HistorySummarizerProducerError> {
        HistorySummarizerProducer::status(self, run_id).await
    }

    async fn cancel(&mut self, run_id: &str) -> Result<(), HistorySummarizerProducerError> {
        HistorySummarizerProducer::cancel(self, run_id).await
    }

    async fn close_attempt(&mut self) -> Result<(), HistorySummarizerProducerError> {
        HistorySummarizerProducer::close_attempt(self).await
    }

    async fn close(&mut self) -> Result<(), HistorySummarizerProducerError> {
        HistorySummarizerProducer::close(self).await
    }

    async fn purge_session(
        &mut self,
        session_id: &str,
    ) -> Result<(), HistorySummarizerProducerError> {
        HistorySummarizerProducer::purge_session(self, session_id).await
    }
}

pub struct HistorySummarizerFireRequest<'a> {
    pub store: &'a MemoryStore,
    pub session_id: &'a str,
    pub project_path: &'a str,
    pub project_slug: &'a str,
    /// The awaiting state records `harness` so recovery reattaches to the run's ModelExecution identity.
    pub harness: &'a str,
    /// The producer sends `HISTORY_SUMMARIZER_SYSTEM_PROMPT` through `system`, never concatenates it into `prompt`, and omits `system` when the value is empty.
    pub system: &'a str,
    pub prompt: &'a str,
    pub model_chain: &'a [String],
    pub from_ordinal: u64,
    pub to_ordinal: u64,
    pub chunk_fingerprint: &'a str,
    pub selected_range_identities: Vec<HistorySummarizerSelectedMessageIdentity>,
    pub expected_revert_epoch: u64,
    pub history_segment_set_generation: HistorySegmentSetGeneration,
    pub observed_chunk_fingerprint: &'a str,
    pub validation_chunk: &'a HistorySummarizerChunk,
    pub chunk_transcript: &'a str,

    pub boundary_dates: &'a BTreeMap<String, String>,
    pub prior_history_segments: &'a [StoredHistorySegmentRange],
    pub validate_options: ValidateOptions,
    pub now_ms: i64,
    pub failure_backoff_at_ms: i64,
    pub completion_now_ms: fn() -> i64,
    pub publication_fence: Option<&'a dyn HistorySummarizerPublicationFence>,
    /// The Curator handoff an accepted fact set is reserved and staged through; `None` records the candidates as not admitted for an unavailable Curator.
    pub curator_handoff: Option<&'a HandoffTarget>,
}

pub struct HistorySummarizerReattachRequest<'a> {
    pub store: &'a MemoryStore,
    pub session_id: &'a str,
    pub project_path: &'a str,
    pub observed_chunk_fingerprint: &'a str,
    pub validation_chunk: &'a HistorySummarizerChunk,
    pub chunk_transcript: &'a str,

    pub boundary_dates: &'a BTreeMap<String, String>,
    pub prior_history_segments: &'a [StoredHistorySegmentRange],
    pub validate_options: ValidateOptions,
    pub publication_floor_ordinal: u64,
    pub now_ms: i64,
    pub failure_backoff_at_ms: i64,
    pub completion_now_ms: fn() -> i64,
    pub publication_fence: Option<&'a dyn HistorySummarizerPublicationFence>,
    pub curator_handoff: Option<&'a HandoffTarget>,
}

/// `HISTORY_SUMMARIZER_CHILD_SESSION_PREFIX` marks producer sessions as self-owned.
/// Re-transforming a producer request prepends m0/m1 framing and violates the expected `[system, user]` shape.
pub const HISTORY_SUMMARIZER_CHILD_SESSION_PREFIX: &str = "eidnara-history_summarizer:";

/// `completion_wait_budget` covers one history_summarizer run and one timeout recovery re-drain.
pub fn completion_wait_budget() -> Duration {
    Duration::from_secs(660)
}

/// Consumers must pass `MAX_WRAPUP_REQUEST_BUDGET` unchanged because it includes the margin.
/// Consumers must not add margin to `MAX_WRAPUP_REQUEST_BUDGET` because it includes the margin.
/// The producer has no round-count cap.
/// The producer drains chunks until the keep watermark or the configured drain budget expires.
/// The configured drain budget, not a chunk count, limits the drain.
pub const MAX_WRAPUP_REQUEST_BUDGET: Duration = Duration::from_secs(3_800);

/// A timed-out producer continues under the history_summarizer guard, so durable recovery matches an incremental firing.
pub fn wrapup_round_wait_budget() -> Duration {
    Duration::from_secs(600)
}

/// Consumers must pass the transform-call deadline unchanged because the transform module includes the margin.
/// Consumers must not add local margin because that would double-count the module's margin.
///
/// Emergency95 can wait for an active run and then refire inline, requiring two 660-second completion waits.
/// `MAX_EMERGENCY_REQUEST_BUDGET` includes a 180-second margin beyond two 660-second completion waits.
/// The timeout path forwards the raw request array and discards the transform result.
pub const MAX_EMERGENCY_REQUEST_BUDGET: Duration = Duration::from_secs(1500);

/// Build the llm-runner session id owned by Eidnara for one history_summarizer firing.
/// The firing sequence is part of the id so a fallback model attempt never resumes a
/// failed run under a different model.
///
/// Producer session IDs must be unique per `(lineage, firing)`.
/// Lineages under composite keys fold concurrently, and their firing sequences advance independently.
/// A project-scoped ID can assign one producer session to two lineages at the same sequence.
/// Sharing a producer session crosses lineage terminal-run tracking.
/// Terminal-run tracking would compare one lineage's expected run ID with another's found run ID.
/// Hashing prevents non-slug-safe composite-key delimiters from appearing in the ID.
/// The `eidnara-history_summarizer:` prefix lets self-exemption identify producer sessions.
pub fn history_summarizer_producer_session_id(
    project_slug: &str,
    session_id: &str,
    firing_seq: u64,
) -> String {
    let slug: String = project_slug
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-');
    let slug = if slug.is_empty() { "project" } else { slug };
    let lineage = fnv1a_hex16(session_id);
    format!("eidnara-history_summarizer:{slug}:{lineage}:{firing_seq}")
}

/// Zero-padding preserves the full 64-bit FNV-1a output.
fn fnv1a_hex16(input: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProducerFailureDecision {
    try_next_model: bool,
    failure_backoff_at_ms: i64,
    detail_prefix: Option<&'static str>,
}

fn decide_producer_failure(
    err: &HistorySummarizerProducerError,
    model: &str,
    remaining_models: &[String],
    auth_blocked_providers: &mut Vec<String>,
    all_failures_permanent: &mut bool,
    now_ms: i64,
    default_failure_backoff_at_ms: i64,
) -> ProducerFailureDecision {
    if err.is_cross_incarnation_unknown() {
        *all_failures_permanent = false;
        return ProducerFailureDecision {
            try_next_model: false,
            failure_backoff_at_ms: default_failure_backoff_at_ms,
            detail_prefix: None,
        };
    }
    if let Some(classification) = err.classification() {
        // The producer owns classification.
        // The consumer branches only on the producer's class tag and structured retry-after value.
        // Provider codes and messages are diagnostic only; they do not override the producer's class tag.
        return match classification.class {
            ErrorClass::Permanent => {
                let try_next = has_eligible_model(remaining_models, auth_blocked_providers);
                ProducerFailureDecision {
                    try_next_model: try_next,
                    failure_backoff_at_ms: default_failure_backoff_at_ms,
                    detail_prefix: (!try_next && *all_failures_permanent)
                        .then_some(CHAIN_EXHAUSTED_PERMANENT_PREFIX),
                }
            }
            ErrorClass::Transient => {
                *all_failures_permanent = false;
                let try_next = has_eligible_model(remaining_models, auth_blocked_providers);
                ProducerFailureDecision {
                    try_next_model: try_next,
                    failure_backoff_at_ms: if try_next {
                        default_failure_backoff_at_ms
                    } else {
                        classified_backoff_at_ms(
                            now_ms,
                            default_failure_backoff_at_ms,
                            classification,
                        )
                    },
                    detail_prefix: None,
                }
            }
            ErrorClass::AuthRequired => {
                *all_failures_permanent = false;
                add_auth_blocked_provider(auth_blocked_providers, provider_prefix(model));
                let try_next = has_eligible_model(remaining_models, auth_blocked_providers);
                ProducerFailureDecision {
                    try_next_model: try_next,
                    failure_backoff_at_ms: default_failure_backoff_at_ms,
                    detail_prefix: (!try_next).then_some(AUTH_REQUIRED_PREFIX),
                }
            }
            ErrorClass::ContextOverflow => {
                *all_failures_permanent = false;
                // HistorySummarizer chunks are sized below every configured model window.
                // A source-classified overflow indicates an estimator error; a larger fallback would mask the bad budget and hide the health signal.
                ProducerFailureDecision {
                    try_next_model: false,
                    failure_backoff_at_ms: default_failure_backoff_at_ms,
                    detail_prefix: None,
                }
            }
        };
    }

    if err.has_class_field() {
        *all_failures_permanent = false;
        return ProducerFailureDecision {
            try_next_model: false,
            failure_backoff_at_ms: default_failure_backoff_at_ms,
            detail_prefix: Some(UNKNOWN_ERROR_CLASS_PREFIX),
        };
    }

    *all_failures_permanent = false;
    let heuristic = err.deprecated_heuristic_decision();
    let try_next = heuristic.retryable_model_failure
        && !heuristic.abort_or_overflow
        && has_eligible_model(remaining_models, auth_blocked_providers);
    ProducerFailureDecision {
        try_next_model: try_next,
        failure_backoff_at_ms: default_failure_backoff_at_ms,
        detail_prefix: None,
    }
}

pub(crate) fn completion_failure_backoff_at_ms(
    started_at_ms: i64,
    configured_backoff_at_ms: i64,
    completed_at_ms: i64,
) -> i64 {
    let cooldown_ms = configured_backoff_at_ms
        .saturating_sub(started_at_ms)
        .max(0);
    completed_at_ms.saturating_add(cooldown_ms)
}

fn classified_backoff_at_ms(
    now_ms: i64,
    default_failure_backoff_at_ms: i64,
    classification: ErrorClassification,
) -> i64 {
    let Some(retry_after_secs) = classification.retry_after_secs else {
        return default_failure_backoff_at_ms;
    };
    let retry_after_ms = retry_after_secs.saturating_mul(1000).min(i64::MAX as u64) as i64;
    now_ms.saturating_add(HISTORY_SUMMARIZER_FAILURE_BACKOFF_MS.max(retry_after_ms))
}

fn has_eligible_model(models: &[String], auth_blocked_providers: &[String]) -> bool {
    models
        .iter()
        .any(|model| !provider_is_auth_blocked(auth_blocked_providers, model))
}

fn provider_is_auth_blocked(auth_blocked_providers: &[String], model: &str) -> bool {
    let provider = provider_prefix(model);
    auth_blocked_providers
        .iter()
        .any(|blocked| blocked == provider)
}

fn add_auth_blocked_provider(auth_blocked_providers: &mut Vec<String>, provider: &str) {
    if !auth_blocked_providers
        .iter()
        .any(|blocked| blocked == provider)
    {
        auth_blocked_providers.push(provider.to_string());
    }
}

fn provider_prefix(model: &str) -> &str {
    model
        .split_once('/')
        .map_or(model, |(provider, _)| provider)
}

fn prefixed_detail(prefix: Option<&str>, detail: String) -> String {
    match prefix {
        Some(prefix) => format!("{prefix}{detail}"),
        None => detail,
    }
}

/// The caller persists the durable transition before invoking `log_cleanup_failure`.
fn log_cleanup_failure(
    session_id: &str,
    operation: &str,
    result: &Result<(), HistorySummarizerProducerError>,
) {
    if let Err(err) = result {
        eprintln!("daemon: history_summarizer {operation} cleanup failed for {session_id}: {err}");
    }
}

async fn close_and_log<P>(producer: &mut P, session_id: &str)
where
    P: HistorySummarizerProducerDriver + ?Sized,
{
    log_cleanup_failure(session_id, "close", &producer.close().await);
}

/// Returns true only when cancellation proves the provider run stopped.
///
/// Fallback requires proof that cancellation stopped the provider run because a second run may be billable.
/// The supervisor acquires its command permit before calling `run.cancel.cancel()`.
/// A saturated command semaphore returns terminal `queue_full` while the provider run continues.
/// Treating terminal errors other than `teardown_unconfirmed` as proof would authorize fallback after `queue_full`.
///
/// Only `Ok(())` proves that cancellation stopped the run or that the run is absent from the index.
/// Terminal errors leave the provider run state unproven and do not authorize a second run.
fn cancellation_confirmed_stopped(result: &Result<(), HistorySummarizerProducerError>) -> bool {
    result.is_ok()
}

pub async fn run_history_summarizer_firing<P>(
    producer: &mut P,
    request: HistorySummarizerFireRequest<'_>,
) -> Result<HistorySummarizerDriveOutcome, HistorySummarizerDriveError>
where
    P: HistorySummarizerProducerDriver + ?Sized,
{
    if request.model_chain.is_empty() {
        return Err(HistorySummarizerDriveError::NoModels);
    }

    let mut auth_blocked_providers = Vec::new();
    let mut all_failures_permanent = true;

    for (index, model) in request.model_chain.iter().enumerate() {
        if provider_is_auth_blocked(&auth_blocked_providers, model) {
            continue;
        }
        verify_chunk_fingerprint(
            request.chunk_fingerprint,
            request.observed_chunk_fingerprint,
        )?;
        let loaded = request.store.load(request.session_id)?;
        let fired = match fire(
            &loaded.meta.history_summarizer,
            request.from_ordinal,
            request.to_ordinal,
            request.chunk_fingerprint.to_string(),
            request.selected_range_identities.clone(),
            request.expected_revert_epoch,
            request.history_segment_set_generation,
            request.now_ms,
        )? {
            FireOutcome::Busy(state) => {
                return Ok(HistorySummarizerDriveOutcome::Busy(Box::new(state)));
            }
            FireOutcome::Fired(state) => state,
        };
        persist_history_summarizer_state(request.store, request.session_id, fired.clone())?;

        let producer_session_id = history_summarizer_producer_session_id(
            request.project_slug,
            request.session_id,
            fired.firing_seq,
        );
        let handle = match producer
            .start(&producer_session_id, request.system, request.prompt, model)
            .await
        {
            Ok(handle) => handle,
            Err(err) => {
                let completed_at_ms = (request.completion_now_ms)();
                let failure_backoff_at_ms = completion_failure_backoff_at_ms(
                    request.now_ms,
                    request.failure_backoff_at_ms,
                    completed_at_ms,
                );
                let decision = decide_producer_failure(
                    &err,
                    model,
                    &request.model_chain[index + 1..],
                    &mut auth_blocked_providers,
                    &mut all_failures_permanent,
                    completed_at_ms,
                    failure_backoff_at_ms,
                );
                persist_history_summarizer_state(
                    request.store,
                    request.session_id,
                    abandon_with_detail(
                        &fired,
                        decision.failure_backoff_at_ms,
                        Some(prefixed_detail(
                            decision.detail_prefix,
                            format!("producer start ({model}): {err:?}"),
                        )),
                    ),
                )?;
                if decision.try_next_model {
                    let cleanup = producer.close_attempt().await;
                    log_cleanup_failure(request.session_id, "attempt close", &cleanup);
                    continue;
                }
                let close_result = producer.close().await;
                return Err(HistorySummarizerDriveError::Producer(attach_cleanup(
                    err,
                    close_result,
                    "close",
                )));
            }
        };

        let awaiting = producer_started(
            &fired,
            producer_session_id.clone(),
            handle.run_id.clone(),
            request.harness.to_owned(),
        )?;
        persist_history_summarizer_state(request.store, request.session_id, awaiting.clone())?;

        let output = match producer.await_output(&handle.run_id).await {
            Ok(output) => output,
            Err(HistorySummarizerProducerError::TimedOut) => {
                match producer.redrain_output(&handle.run_id).await {
                    Ok(output) => output,
                    Err(recovery_err) => {
                        let cancel_result = producer.cancel(&handle.run_id).await;
                        let detail = format!(
                            "producer output ({model}): timed out; recovery re-drain also failed: {recovery_err}"
                        );
                        let failure_backoff_at_ms = completion_failure_backoff_at_ms(
                            request.now_ms,
                            request.failure_backoff_at_ms,
                            (request.completion_now_ms)(),
                        );
                        persist_history_summarizer_state(
                            request.store,
                            request.session_id,
                            abandon_with_detail(&awaiting, failure_backoff_at_ms, Some(detail)),
                        )?;
                        let close_result = producer.close().await;
                        return Err(HistorySummarizerDriveError::Producer(attach_cleanup(
                            attach_cleanup(recovery_err, cancel_result, "cancel"),
                            close_result,
                            "close",
                        )));
                    }
                }
            }
            Err(err) => {
                let cancel_result = producer.cancel(&handle.run_id).await;
                let completed_at_ms = (request.completion_now_ms)();
                let failure_backoff_at_ms = completion_failure_backoff_at_ms(
                    request.now_ms,
                    request.failure_backoff_at_ms,
                    completed_at_ms,
                );
                let decision = decide_producer_failure(
                    &err,
                    model,
                    &request.model_chain[index + 1..],
                    &mut auth_blocked_providers,
                    &mut all_failures_permanent,
                    completed_at_ms,
                    failure_backoff_at_ms,
                );
                persist_history_summarizer_state(
                    request.store,
                    request.session_id,
                    abandon_with_detail(
                        &awaiting,
                        decision.failure_backoff_at_ms,
                        Some(prefixed_detail(
                            decision.detail_prefix,
                            format!("producer output ({model}): {err:?}"),
                        )),
                    ),
                )?;
                if decision.try_next_model && cancellation_confirmed_stopped(&cancel_result) {
                    let cleanup = producer.close_attempt().await;
                    log_cleanup_failure(request.session_id, "cancel", &cancel_result);
                    log_cleanup_failure(request.session_id, "attempt close", &cleanup);
                    continue;
                }
                let close_result = producer.close().await;
                return Err(HistorySummarizerDriveError::Producer(attach_cleanup(
                    attach_cleanup(err, cancel_result, "cancel"),
                    close_result,
                    "close",
                )));
            }
        };

        let publish_result = publish_output_from_awaiting(PublishOutputRequest {
            store: request.store,
            session_id: request.session_id,
            project_path: request.project_path,
            awaiting,
            output,
            observed_chunk_fingerprint: request.observed_chunk_fingerprint,
            validation_chunk: request.validation_chunk,
            chunk_transcript: request.chunk_transcript,

            boundary_dates: request.boundary_dates,
            prior_history_segments: request.prior_history_segments,
            validate_options: request.validate_options,
            created_at_ms: request.now_ms,
            failure_started_at_ms: request.now_ms,
            failure_backoff_at_ms: request.failure_backoff_at_ms,
            completion_now_ms: request.completion_now_ms,
            publication_fence: request.publication_fence,
            curator_handoff: request.curator_handoff,
        });
        let row_version = match publish_result {
            Ok(row_version) => row_version,
            Err(HistorySummarizerDriveError::Validation(err)) => {
                if has_eligible_model(&request.model_chain[index + 1..], &auth_blocked_providers) {
                    let cleanup = producer.close_attempt().await;
                    log_cleanup_failure(request.session_id, "attempt close", &cleanup);
                    continue;
                }
                close_and_log(producer, request.session_id).await;
                return Err(HistorySummarizerDriveError::Validation(err));
            }
            Err(err) => {
                close_and_log(producer, request.session_id).await;
                return Err(err);
            }
        };
        close_and_log(producer, request.session_id).await;
        return Ok(HistorySummarizerDriveOutcome::Completed(
            HistorySummarizerRunSuccess {
                row_version,
                producer_session_id,
                producer_run_id: handle.run_id,
                model: model.clone(),
            },
        ));
    }

    Err(HistorySummarizerDriveError::NoModels)
}

pub async fn reattach_history_summarizer_producer<P>(
    producer: &mut P,
    request: HistorySummarizerReattachRequest<'_>,
) -> Result<HistorySummarizerReattachOutcome, HistorySummarizerDriveError>
where
    P: HistorySummarizerProducerDriver + ?Sized,
{
    let action = handle_restart_load(
        request.store,
        request.session_id,
        request.failure_backoff_at_ms,
    )?;
    let RestartAction::ReattachProducer {
        producer_session_id,
        producer_run_id,
        firing_seq,
        ..
    } = action
    else {
        return match action {
            RestartAction::Done => Ok(HistorySummarizerReattachOutcome::Done),
            RestartAction::AbandonedAndRefireEligible { firing_seq } => {
                Ok(HistorySummarizerReattachOutcome::RefireEligible { firing_seq })
            }
            RestartAction::RepublishReserved { .. } => republish_reserved(RepublishRequest {
                store: request.store,
                session_id: request.session_id,
                project_path: request.project_path,
                curator_handoff: request.curator_handoff,
                now_ms: request.now_ms,
                failure_backoff_at_ms: request.failure_backoff_at_ms,
                publication_fence: request.publication_fence,
            })
            .map(HistorySummarizerReattachOutcome::Republished),
            RestartAction::ReattachProducer { .. } => unreachable!(),
        };
    };

    producer.bind_session(&producer_session_id).await?;
    let state = match producer.status(&producer_run_id).await {
        Ok(state) => state,
        Err(err) => {
            let close_result = producer.close().await;
            return Err(HistorySummarizerDriveError::Producer(attach_cleanup(
                err,
                close_result,
                "close",
            )));
        }
    };

    match state {
        RunState::Terminal | RunState::Active => {}
        RunState::Missing { .. } => {
            let failure_backoff_at_ms = completion_failure_backoff_at_ms(
                request.now_ms,
                request.failure_backoff_at_ms,
                (request.completion_now_ms)(),
            );
            abandon_current_state(request.store, request.session_id, failure_backoff_at_ms)?;
            close_and_log(producer, request.session_id).await;
            return Ok(HistorySummarizerReattachOutcome::RefireEligible { firing_seq });
        }
    }

    let loaded = request.store.load(request.session_id)?;
    let awaiting = loaded.meta.history_summarizer.clone();
    let awaited = match producer.await_output(&producer_run_id).await {
        Err(HistorySummarizerProducerError::TimedOut) => {
            producer.redrain_output(&producer_run_id).await
        }
        other => other,
    };
    let output = match awaited {
        Ok(output) => output,
        Err(err) => {
            let cancel_result = producer.cancel(&producer_run_id).await;
            let detail_prefix = match err
                .classification()
                .map(|classification| classification.class)
            {
                Some(ErrorClass::AuthRequired) => Some(AUTH_REQUIRED_PREFIX),
                _ if err.has_class_field() && err.classification().is_none() => {
                    Some(UNKNOWN_ERROR_CLASS_PREFIX)
                }
                _ => None,
            };
            let completed_at_ms = (request.completion_now_ms)();
            let failure_backoff_at_ms = completion_failure_backoff_at_ms(
                request.now_ms,
                request.failure_backoff_at_ms,
                completed_at_ms,
            );
            let backoff_at_ms =
                err.classification()
                    .map_or(failure_backoff_at_ms, |classification| {
                        if classification.class == ErrorClass::Transient {
                            classified_backoff_at_ms(
                                completed_at_ms,
                                failure_backoff_at_ms,
                                classification,
                            )
                        } else {
                            failure_backoff_at_ms
                        }
                    });
            abandon_current_state_with_detail(
                request.store,
                request.session_id,
                backoff_at_ms,
                Some(prefixed_detail(
                    detail_prefix,
                    format!("producer reattach output ({producer_run_id}): {err:?}"),
                )),
            )?;
            let close_result = producer.close().await;
            return Err(HistorySummarizerDriveError::Producer(attach_cleanup(
                attach_cleanup(err, cancel_result, "cancel"),
                close_result,
                "close",
            )));
        }
    };

    let publish_result = publish_output_from_awaiting(PublishOutputRequest {
        store: request.store,
        session_id: request.session_id,
        project_path: request.project_path,
        awaiting,
        output,
        observed_chunk_fingerprint: request.observed_chunk_fingerprint,
        validation_chunk: request.validation_chunk,
        chunk_transcript: request.chunk_transcript,

        boundary_dates: request.boundary_dates,
        prior_history_segments: request.prior_history_segments,
        validate_options: request.validate_options,
        created_at_ms: request.now_ms,
        failure_started_at_ms: request.now_ms,
        failure_backoff_at_ms: request.failure_backoff_at_ms,
        completion_now_ms: request.completion_now_ms,
        publication_fence: request.publication_fence,
        curator_handoff: request.curator_handoff,
    });
    close_and_log(producer, request.session_id).await;
    let row_version = publish_result?;
    Ok(HistorySummarizerReattachOutcome::Published(
        HistorySummarizerRunSuccess {
            row_version,
            producer_session_id,
            producer_run_id,
            model: String::new(),
        },
    ))
}

struct PublishOutputRequest<'a> {
    store: &'a MemoryStore,
    session_id: &'a str,
    project_path: &'a str,
    awaiting: HistorySummarizerDurableState,
    output: ProducerOutput,
    observed_chunk_fingerprint: &'a str,
    validation_chunk: &'a HistorySummarizerChunk,
    chunk_transcript: &'a str,

    boundary_dates: &'a BTreeMap<String, String>,
    prior_history_segments: &'a [StoredHistorySegmentRange],
    validate_options: ValidateOptions,
    created_at_ms: i64,
    failure_started_at_ms: i64,
    failure_backoff_at_ms: i64,
    completion_now_ms: fn() -> i64,
    publication_fence: Option<&'a dyn HistorySummarizerPublicationFence>,
    curator_handoff: Option<&'a HandoffTarget>,
}

fn publish_output_from_awaiting(
    request: PublishOutputRequest<'_>,
) -> Result<u64, HistorySummarizerDriveError> {
    let PublishOutputRequest {
        store,
        session_id,
        project_path,
        awaiting,
        output,
        observed_chunk_fingerprint,
        validation_chunk,
        chunk_transcript,

        boundary_dates,
        prior_history_segments,
        validate_options,
        created_at_ms,
        failure_started_at_ms,
        failure_backoff_at_ms,
        completion_now_ms,
        publication_fence,
        curator_handoff,
    } = request;
    let validating = output_received(&awaiting, &output.text)?;
    persist_history_summarizer_state(store, session_id, validating.clone())?;

    let validation_result = if output.length_capped {
        Err(HistorySummarizerValidationError {
            message:
                "HistorySummarizer output hit the length cap; refusing a potentially partial document."
                    .to_string(),
        })
    } else {
        validate_history_summarizer_output(
            &output.text,
            validation_chunk,
            prior_history_segments,
            validate_options,
        )
    };
    let validated = match validation_result {
        Ok(validated) => validated,
        Err(err) => {
            let failure_backoff_at_ms = completion_failure_backoff_at_ms(
                failure_started_at_ms,
                failure_backoff_at_ms,
                completion_now_ms(),
            );
            let cap_hint = if output.length_capped {
                " [output hit the length cap: raise the output budget or shrink the chunk]"
            } else {
                ""
            };
            persist_history_summarizer_state(
                store,
                session_id,
                abandon_with_detail(
                    &validating,
                    failure_backoff_at_ms,
                    Some(format!("validate rejected: {err}{cap_hint}")),
                ),
            )?;
            return Err(HistorySummarizerDriveError::Validation(err));
        }
    };

    let publishing = validation_ok(&validating)?;
    let publishing_row_version =
        persist_history_summarizer_state(store, session_id, publishing.clone())?;
    let predicate = publish_predicate(&publishing)?;
    let decision = curator_decision_before_publish(CuratorDecisionRequest {
        store,
        session_id,
        project_path,
        publishing: &publishing,
        publishing_row_version,
        validated: &validated,
        aliases: &validation_chunk.aliases,
        pending: PendingPublication {
            validated_json: serde_json::to_string(&validated).map_err(|error| {
                HistorySummarizerStateError::Store(MemoryStoreError::Serde(error.to_string()))
            })?,
            aliases_json: serde_json::to_string(&validation_chunk.aliases).map_err(|error| {
                HistorySummarizerStateError::Store(MemoryStoreError::Serde(error.to_string()))
            })?,
            chunk_transcript: chunk_transcript.to_string(),
            boundary_dates: boundary_dates.clone(),
            publication_floor_ordinal: validated.unprocessed_from,
            collect_user_memory_candidates: validate_options.user_memory_collection_enabled,
        },
        curator_handoff,
        created_at_ms,
        failure_started_at_ms,
        failure_backoff_at_ms,
        completion_now_ms,
    })?;
    let CuratorDecision {
        curator_nonadmission,
        curator_activation,
        publishing_row_version,
    } = decision;
    let published = publish_validated_chunk(
        store,
        ValidatedPublishRequest {
            session_id,
            project_path,
            expected_row_version: Some(publishing_row_version),
            expected_revert_epoch: publishing.expected_revert_epoch,
            predicate: &predicate,
            observed_chunk_fingerprint,
            validated: &validated,
            collect_user_memory_candidates: validate_options.user_memory_collection_enabled,
            publication_floor_ordinal: validated.unprocessed_from,
            chunk_transcript,

            boundary_dates,
            created_at_ms,
            failure_backoff_at_ms,
            publication_fence,
            curator_nonadmission,
            curator_activation: curator_activation.as_deref(),
        },
    )?;
    Ok(published.row_version)
}

/// Writes the reservation into the Publishing state this firing persisted, fenced on that write's row version so a competing writer that moved the session on cannot be overwritten with a resurrected Publishing state.
fn persist_reservation(
    store: &MemoryStore,
    session_id: &str,
    publishing_row_version: u64,
    reservation: &memory_store::CuratorReservation,
    pending: &PendingPublication,
) -> Result<u64, HistorySummarizerStateError> {
    store
        .record_curator_reservation(session_id, publishing_row_version, reservation, pending)
        .map_err(HistorySummarizerStateError::Publish)
}

struct CuratorDecisionRequest<'a> {
    store: &'a MemoryStore,
    session_id: &'a str,
    project_path: &'a str,
    publishing: &'a HistorySummarizerDurableState,
    publishing_row_version: u64,
    validated: &'a ValidatedChunk,
    aliases: &'a FrozenAliasTable,
    /// Retained with the reservation so a local retry republishes the same output.
    pending: PendingPublication,
    curator_handoff: Option<&'a HandoffTarget>,
    created_at_ms: i64,
    failure_started_at_ms: i64,
    failure_backoff_at_ms: i64,
    completion_now_ms: fn() -> i64,
}

/// What the publication carries for the Curator, with the row version it must CAS against.
struct CuratorDecision {
    curator_nonadmission: Option<CuratorNonadmissionCode>,
    curator_activation: Option<Box<PreparedActivation>>,
    publishing_row_version: u64,
}

/// KTD3/Q31: an accepted set is reserved and staged before publication, and the reservation is written into the Publishing state the moment it exists; every other outcome is decided without a reservation. A failure after the reservation abandons the run with the reservation retained and does not publish.
fn curator_decision_before_publish(
    request: CuratorDecisionRequest<'_>,
) -> Result<CuratorDecision, HistorySummarizerDriveError> {
    let CuratorDecisionRequest {
        store,
        session_id,
        project_path,
        publishing,
        publishing_row_version,
        validated,
        aliases,
        pending,
        curator_handoff,
        created_at_ms,
        failure_started_at_ms,
        failure_backoff_at_ms,
        completion_now_ms,
    } = request;
    let (ExtractionOutcome::Accepted { .. }, Some(target)) =
        (&validated.extraction, curator_handoff)
    else {
        return Ok(CuratorDecision {
            curator_nonadmission: curator_nonadmission_before_reservation(&validated.extraction),
            curator_activation: None,
            publishing_row_version,
        });
    };
    // Q31: a publication the store refuses to retain (past its envelope, or content the durable scan rejects) could never be recovered after the reservation, so nothing is reserved for it and the refusal is recorded like the Kernel's.
    if !store.pending_publication_retainable(session_id, &pending)? {
        return Ok(CuratorDecision {
            curator_nonadmission: Some(CuratorNonadmissionCode::SubjectRefused),
            curator_activation: None,
            publishing_row_version,
        });
    }
    let handoff = handoff::reserve_and_stage(
        target,
        &HandoffRequest {
            store,
            project: project_path,
            session_id,
            firing: publishing,
            facts: &validated.facts,
            aliases,
            now_ms: created_at_ms,
        },
        |reservation| {
            persist_reservation(
                store,
                session_id,
                publishing_row_version,
                reservation,
                &pending,
            )
        },
    );
    match handoff {
        Ok(Handoff::Activate(prepared)) => Ok(CuratorDecision {
            curator_nonadmission: None,
            publishing_row_version: prepared.row_version,
            curator_activation: Some(prepared),
        }),
        Ok(Handoff::Nonadmission(code)) => Ok(CuratorDecision {
            curator_nonadmission: Some(code),
            curator_activation: None,
            publishing_row_version,
        }),
        Ok(Handoff::Settled) => Ok(CuratorDecision {
            curator_nonadmission: None,
            curator_activation: None,
            publishing_row_version,
        }),
        Err(error) => {
            // After the reservation exists the firing stays in Publishing with the failure recorded, so recovery reconciles it against the reservation instead of abandoning and refiring.
            let failure_backoff_at_ms = completion_failure_backoff_at_ms(
                failure_started_at_ms,
                failure_backoff_at_ms,
                completion_now_ms(),
            );
            let current = store.load(session_id)?.meta.history_summarizer;
            let detail = Some(format!("curator handoff failed: {error}"));
            let next = if current.holds_reservation() {
                retain_with_detail(&current, failure_backoff_at_ms, detail)
            } else {
                abandon_with_detail(&current, failure_backoff_at_ms, detail)
            };
            persist_history_summarizer_state(store, session_id, next)?;
            Err(HistorySummarizerDriveError::CuratorHandoff(error))
        }
    }
}

/// Q31, before any reservation: a rejected optional fact set is a nonadmission with the validator's code; an accepted set that reaches here had no Curator handoff to take it, so it is a nonadmission for an unavailable Curator; an intentional no-fact or extraction-free run is not a nonadmission.
fn curator_nonadmission_before_reservation(
    extraction: &ExtractionOutcome,
) -> Option<CuratorNonadmissionCode> {
    match extraction {
        ExtractionOutcome::NotRequested | ExtractionOutcome::NoFacts => None,
        ExtractionOutcome::Accepted { .. } => Some(CuratorNonadmissionCode::CuratorUnavailable),
        ExtractionOutcome::Rejected { failure } => {
            Some(CuratorNonadmissionCode::FactSetRejected { failure: *failure })
        }
    }
}

fn abandon_current_state(
    store: &MemoryStore,
    session_id: &str,
    failure_backoff_at_ms: i64,
) -> Result<(), HistorySummarizerStateError> {
    abandon_current_state_with_detail(store, session_id, failure_backoff_at_ms, None)
}

fn abandon_current_state_with_detail(
    store: &MemoryStore,
    session_id: &str,
    failure_backoff_at_ms: i64,
    detail: Option<String>,
) -> Result<(), HistorySummarizerStateError> {
    let loaded = store.load(session_id)?;
    persist_history_summarizer_state(
        store,
        session_id,
        abandon_with_detail(
            &loaded.meta.history_summarizer,
            failure_backoff_at_ms,
            detail,
        ),
    )?;
    Ok(())
}

fn require_phase(
    current: &HistorySummarizerDurableState,
    expected: HistorySummarizerPhase,
    event: &'static str,
) -> Result<(), HistorySummarizerStateError> {
    if current.state == expected {
        Ok(())
    } else {
        Err(HistorySummarizerStateError::InvalidTransition {
            from: current.state.clone(),
            event,
        })
    }
}

/// Fence rejection clears the matching run without delaying a fresh snapshot retry.
fn abandon_matching_run_without_cooldown(
    store: &MemoryStore,
    session_id: &str,
    predicate: &HistorySummarizerPublishPredicate,
    detail: Option<String>,
) -> Result<Option<u64>, HistorySummarizerStateError> {
    Ok(
        store.abandon_history_summarizer_run_if_matching_with_publish_failure(
            session_id,
            predicate,
            None,
            detail.as_deref(),
            true,
        )?,
    )
}

fn abandon_matching_run_with_detail(
    store: &MemoryStore,
    session_id: &str,
    predicate: &HistorySummarizerPublishPredicate,
    failure_backoff_at_ms: i64,
    detail: Option<String>,
) -> Result<Option<u64>, HistorySummarizerStateError> {
    Ok(
        store.abandon_history_summarizer_run_if_matching_with_publish_failure(
            session_id,
            predicate,
            Some(failure_backoff_at_ms),
            detail.as_deref(),
            true,
        )?,
    )
}

#[cfg(test)]
#[path = "history_summarizer_handoff_tests.rs"]
mod handoff_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// Shared reattach-test prologue: fire the trigger, mark the producer
    /// started ("producer-session"/"run-1" on pi), and commit the awaiting
    /// history_summarizer state for session "ses".
    fn seed_awaiting_history_summarizer(store: &MemoryStore) {
        let fired = match fire(
            &HistorySummarizerDurableState::default(),
            2,
            4,
            "fp".into(),
            test_selected_range_identities(),
            0,
            HistorySegmentSetGeneration {
                max_sequence: 1,
                count: 1,
            },
            1,
        )
        .unwrap()
        {
            FireOutcome::Fired(state) => state,
            FireOutcome::Busy(_) => unreachable!(),
        };
        let awaiting = producer_started(
            &fired,
            "producer-session".into(),
            "run-1".into(),
            "pi".into(),
        )
        .unwrap();
        store
            .commit(
                "ses",
                None,
                &CoreState::empty(),
                &test_meta_with_history_summarizer(awaiting),
            )
            .unwrap();
    }

    use std::collections::VecDeque;

    use crate::history_summarizer_producer::HistorySummarizerSendOutcome;
    use cache_stability::CoreState;
    use memory_store::{ModuleMeta, StoredHistorySegment};
    use storage::{Isolation, StorageBackend, StorageDescriptor};

    fn store(dir: &std::path::Path) -> MemoryStore {
        MemoryStore::open(&StorageDescriptor {
            module_id: "eidnara-test".to_string(),
            storage_namespace: "memory".to_string(),
            isolation: Isolation::Module,
            backend: StorageBackend::Sqlite {
                path: dir.join("store.db").to_string_lossy().to_string(),
            },
        })
        .unwrap()
    }

    fn empty_boundary_dates() -> &'static BTreeMap<String, String> {
        static EMPTY: std::sync::OnceLock<BTreeMap<String, String>> = std::sync::OnceLock::new();
        EMPTY.get_or_init(BTreeMap::new)
    }

    fn comp(seq: i64, start: i64, end: i64, end_id: &str, p1: &str) -> StoredHistorySegment {
        StoredHistorySegment {
            sequence: seq,
            start_message: start,
            end_message: end,
            end_message_id: format!("{end_id}#0"),
            title: format!("C{seq}"),
            content: p1.to_string(),
            p1: Some(p1.to_string()),
            importance: 50,
            ..Default::default()
        }
    }

    #[test]
    fn stored_history_segment_legacy_flag_tracks_p1_presence() {
        let tiered = ValidatedHistorySegment {
            sequence: 1,
            start_message: 1,
            end_message: 2,
            start_message_id: "m1#0".into(),
            end_message_id: "m2#0".into(),
            title: "tiered".into(),
            content: "full".into(),
            p1: Some("full".into()),
            p2: Some("short".into()),
            p3: Some("brief".into()),
            p4: Some("".into()),
            importance: None,
            episode_type: None,
        };
        let flat = ValidatedHistorySegment {
            p1: None,
            p2: None,
            p3: None,
            p4: None,
            ..tiered.clone()
        };

        assert_eq!(
            to_stored_history_segment(&tiered, 1, empty_boundary_dates()).legacy,
            0
        );
        assert_eq!(
            to_stored_history_segment(&flat, 1, empty_boundary_dates()).legacy,
            1
        );
    }

    /// The validator accepts any `\d+` importance; an out-of-range value is
    /// clamped to the documented 1..=100 before it narrows to `i32`, so a
    /// value past `i32::MAX` cannot wrap negative and read as importance 1.
    #[test]
    fn stored_history_segment_importance_is_clamped_before_narrowing() {
        let base = ValidatedHistorySegment {
            sequence: 1,
            start_message: 1,
            end_message: 2,
            start_message_id: "m1#0".into(),
            end_message_id: "m2#0".into(),
            title: "t".into(),
            content: "c".into(),
            p1: Some("c".into()),
            p2: None,
            p3: None,
            p4: None,
            importance: None,
            episode_type: None,
        };
        for (raw, stored) in [
            (Some(4_294_967_295), 100),
            (Some(u64::MAX), 100),
            (Some(0), 1),
            (Some(60), 60),
            (None, 50),
        ] {
            let history_segment = ValidatedHistorySegment {
                importance: raw,
                ..base.clone()
            };
            assert_eq!(
                to_stored_history_segment(&history_segment, 1, empty_boundary_dates()).importance,
                stored,
                "importance {raw:?}"
            );
        }
    }

    fn flat_history_summarizer_xml(content: &str) -> String {
        format!(
            r#"<output>
<history_segments>
<history_segment start="2" end="3" title="flat">{content}</history_segment>
</history_segments>
<meta><messages_processed>2-3</messages_processed><unprocessed_from>4</unprocessed_from></meta>
</output>"#
        )
    }

    fn history_summarizer_xml(p1: &str) -> String {
        format!(
            r#"<output>
<history_segments>
<history_segment start="2" end="3" title="second arc" episode_type="feature" importance="60">
<p1>{p1}</p1>
<p2>second arc short</p2>
<p3>second arc</p3>
<p4 />
</history_segment>
</history_segments>
<meta><messages_processed>2-3</messages_processed><unprocessed_from>4</unprocessed_from></meta>
</output>"#
        )
    }

    fn history_summarizer_chunk() -> HistorySummarizerChunk {
        use crate::history_summarizer_validate::ChunkLine;
        HistorySummarizerChunk {
            aliases: Default::default(),
            start_index: 2,
            end_index: 4,
            lines: vec![
                ChunkLine {
                    ordinal: 2,
                    message_id: "m2#0".into(),
                    anchorable: true,
                },
                ChunkLine {
                    ordinal: 3,
                    message_id: "m3#0".into(),
                    anchorable: true,
                },
                ChunkLine {
                    ordinal: 4,
                    message_id: "m4#0".into(),
                    anchorable: true,
                },
            ],
            present_ordinals: vec![1, 2, 3, 4],
            tool_only_ranges: vec![],
            completed_tool_arcs: vec![],
        }
    }

    fn prior_ranges() -> Vec<StoredHistorySegmentRange> {
        vec![StoredHistorySegmentRange {
            start_message: 1,
            end_message: 1,
        }]
    }

    fn validate_options() -> ValidateOptions {
        ValidateOptions {
            sequence_offset: 1,
            in_emergency: true,
            memory_enabled: true,
            auto_promote: true,
            user_memory_collection_enabled: false,
            force_keep_last_history_segment: false,
        }
    }

    fn seed_prior_history_segment(store: &MemoryStore) {
        store
            .replace_history_segments("ses", &[comp(1, 1, 1, "m1", "C1 summary")])
            .unwrap();
    }

    fn test_selected_range_identities() -> Vec<HistorySummarizerSelectedMessageIdentity> {
        ["m2", "m3"]
            .into_iter()
            .map(|mid| HistorySummarizerSelectedMessageIdentity {
                mid: mid.to_string(),
                block_identities: vec![memory_store::BlockIdentity {
                    kind_tag: "text".to_string(),
                    byte_fingerprint: format!("{mid}-content-a"),
                }],
            })
            .collect()
    }

    fn test_meta_with_history_summarizer(
        history_summarizer: HistorySummarizerDurableState,
    ) -> ModuleMeta {
        let mut meta = ModuleMeta {
            history_summarizer,
            ..Default::default()
        };
        for selected in test_selected_range_identities() {
            meta.block_identity_by_mid
                .insert(selected.mid, selected.block_identities);
        }
        meta
    }

    fn seed_test_selected_range_identities(store: &MemoryStore) {
        let loaded = store.load("ses").unwrap();
        let mut meta = loaded.meta.clone();
        for selected in test_selected_range_identities() {
            meta.block_identity_by_mid
                .insert(selected.mid, selected.block_identities);
        }
        if meta != loaded.meta {
            store
                .commit("ses", loaded.row_version, &loaded.core, &meta)
                .unwrap();
        }
    }

    #[derive(Default)]
    struct ScriptedProducer {
        starts: VecDeque<Result<RunHandle, HistorySummarizerProducerError>>,
        outputs: VecDeque<Result<ProducerOutput, HistorySummarizerProducerError>>,
        statuses: VecDeque<Result<RunState, HistorySummarizerProducerError>>,
        cancel_results: VecDeque<Result<(), HistorySummarizerProducerError>>,
        close_results: VecDeque<Result<(), HistorySummarizerProducerError>>,
        observed_starts: Vec<(String, String)>,
        observed_sessions: Vec<String>,
        observed_systems: Vec<String>,
        await_run_ids: Vec<String>,
        cancels: Vec<String>,
        attempt_closes: usize,
        closes: usize,
        connection_closed: bool,
        on_await_output: Option<Box<dyn FnOnce() + Send>>,
    }

    impl ScriptedProducer {
        fn with_start(mut self, result: Result<RunHandle, HistorySummarizerProducerError>) -> Self {
            self.starts.push_back(result);
            self
        }

        fn with_output(
            mut self,
            result: Result<ProducerOutput, HistorySummarizerProducerError>,
        ) -> Self {
            self.outputs.push_back(result);
            self
        }

        fn with_status(mut self, result: Result<RunState, HistorySummarizerProducerError>) -> Self {
            self.statuses.push_back(result);
            self
        }

        fn with_cancel_result(
            mut self,
            result: Result<(), HistorySummarizerProducerError>,
        ) -> Self {
            self.cancel_results.push_back(result);
            self
        }

        fn with_close_result(mut self, result: Result<(), HistorySummarizerProducerError>) -> Self {
            self.close_results.push_back(result);
            self
        }

        fn with_await_output_hook(mut self, hook: impl FnOnce() + Send + 'static) -> Self {
            self.on_await_output = Some(Box::new(hook));
            self
        }
    }

    #[async_trait::async_trait]
    impl HistorySummarizerProducerDriver for ScriptedProducer {
        async fn bind_session(
            &mut self,
            session_id: &str,
        ) -> Result<(), HistorySummarizerProducerError> {
            self.observed_sessions.push(session_id.to_string());
            Ok(())
        }

        async fn start(
            &mut self,
            session_id: &str,
            system: &str,
            _prompt: &str,
            model: &str,
        ) -> Result<RunHandle, HistorySummarizerProducerError> {
            if self.connection_closed {
                return Err(HistorySummarizerProducerError::tagged_call(
                    "connection_closed",
                    "managed producer connection is closed",
                    ErrorClass::Permanent,
                    None,
                ));
            }
            self.observed_sessions.push(session_id.to_string());
            self.observed_systems.push(system.to_string());
            self.observed_starts
                .push((session_id.to_string(), model.to_string()));
            self.starts
                .pop_front()
                .expect("scripted start result available")
        }

        async fn await_output(
            &mut self,
            run_id: &str,
        ) -> Result<ProducerOutput, HistorySummarizerProducerError> {
            self.await_run_ids.push(run_id.to_string());
            if let Some(hook) = self.on_await_output.take() {
                hook();
            }
            self.outputs
                .pop_front()
                .expect("scripted output result available")
        }

        async fn status(
            &mut self,
            _run_id: &str,
        ) -> Result<RunState, HistorySummarizerProducerError> {
            self.statuses
                .pop_front()
                .expect("scripted status result available")
        }

        async fn cancel(&mut self, run_id: &str) -> Result<(), HistorySummarizerProducerError> {
            self.cancels.push(run_id.to_string());
            self.cancel_results.pop_front().unwrap_or(Ok(()))
        }

        async fn close_attempt(&mut self) -> Result<(), HistorySummarizerProducerError> {
            self.attempt_closes += 1;
            self.close_results.pop_front().unwrap_or(Ok(()))
        }

        async fn close(&mut self) -> Result<(), HistorySummarizerProducerError> {
            self.closes += 1;
            self.connection_closed = true;
            self.close_results.pop_front().unwrap_or(Ok(()))
        }
    }

    fn run_handle(id: &str) -> RunHandle {
        RunHandle {
            run_id: id.to_string(),
        }
    }

    fn producer_output(text: String) -> ProducerOutput {
        ProducerOutput {
            text,
            length_capped: false,
        }
    }

    #[test]
    fn full_lineage_hash_separates_keys_that_collided_at_32_bits() {
        let first = history_summarizer_producer_session_id("proj", "lineage-ZiDxmBSjhQbv", 2);
        let second = history_summarizer_producer_session_id("proj", "lineage-OPeZDtvlh9LD", 2);

        assert_ne!(first, second);
        let first_hash = first.split(':').nth(2).expect("hash segment");
        let second_hash = second.split(':').nth(2).expect("hash segment");
        assert_eq!(first_hash.len(), 16);
        assert_eq!(second_hash.len(), 16);
        assert_eq!(
            &first_hash[..8],
            &second_hash[..8],
            "the regression keys collided under the former 32-bit prefix"
        );
    }

    #[test]
    fn producer_session_ids_are_lineage_scoped_under_one_project() {
        let parent = history_summarizer_producer_session_id("proj", "84b85b9f", 2);
        let subagent =
            history_summarizer_producer_session_id("proj", "84b85b9f\u{241F}a063e\u{241F}0", 2);
        assert_ne!(parent, subagent);
        assert!(parent.starts_with("eidnara-history_summarizer:"));
        assert!(subagent.starts_with("eidnara-history_summarizer:"));
        assert!(subagent.is_ascii());
        assert_ne!(
            parent,
            history_summarizer_producer_session_id("proj", "84b85b9f", 3)
        );
    }

    fn fire_request<'a>(
        store: &'a MemoryStore,
        prompt: &'a str,
        models: &'a [String],
        chunk: &'a HistorySummarizerChunk,
        prior: &'a [StoredHistorySegmentRange],
    ) -> HistorySummarizerFireRequest<'a> {
        seed_test_selected_range_identities(store);
        let history_segment_set_generation = store
            .load_history_summarizer_assembly_snapshot("ses")
            .unwrap()
            .history_segment_set_generation;
        HistorySummarizerFireRequest {
            store,
            harness: "pi",
            session_id: "ses",
            project_path: "git:proj",
            project_slug: "proj",
            system: "role guidance",
            prompt,
            model_chain: models,
            from_ordinal: 2,
            to_ordinal: 4,
            chunk_fingerprint: "fp",
            selected_range_identities: test_selected_range_identities(),
            expected_revert_epoch: 0,
            history_segment_set_generation,
            observed_chunk_fingerprint: "fp",
            validation_chunk: chunk,
            chunk_transcript: "U: transcript",

            boundary_dates: empty_boundary_dates(),
            prior_history_segments: prior,
            validate_options: validate_options(),
            now_ms: 123,
            failure_backoff_at_ms: 999,
            completion_now_ms: || 123,
            publication_fence: None,
            curator_handoff: None,
        }
    }

    fn reattach_request<'a>(
        store: &'a MemoryStore,
        chunk: &'a HistorySummarizerChunk,
        prior: &'a [StoredHistorySegmentRange],
    ) -> HistorySummarizerReattachRequest<'a> {
        HistorySummarizerReattachRequest {
            store,
            session_id: "ses",
            project_path: "git:proj",
            observed_chunk_fingerprint: "fp",
            validation_chunk: chunk,
            chunk_transcript: "U: transcript",

            boundary_dates: empty_boundary_dates(),
            prior_history_segments: prior,
            validate_options: validate_options(),
            publication_floor_ordinal: 4,
            now_ms: 123,
            failure_backoff_at_ms: 999,
            completion_now_ms: || 123,
            publication_fence: None,
            curator_handoff: None,
        }
    }

    fn publishing_state() -> HistorySummarizerDurableState {
        HistorySummarizerDurableState {
            state: HistorySummarizerPhase::Publishing,
            firing_seq: 3,
            chunk_range: Some(HistorySummarizerChunkRange {
                from_ordinal: 2,
                to_ordinal: 4,
            }),
            chunk_fingerprint: "fp".into(),
            selected_range_identities: test_selected_range_identities(),
            producer_session_id: Some("producer-session".into()),
            producer_run_id: Some("run-3".into()),
            producer_harness: None,
            fired_at_ms: Some(10),
            expected_revert_epoch: 0,
            history_segment_set_generation: HistorySegmentSetGeneration::default(),
            failure_backoff_at_ms: None,
            last_failure: None,
            last_no_fire: None,
            consecutive_publish_failures: 0,
            curator_nonadmission: Default::default(),
            curator_reservation: None,
        }
    }

    #[tokio::test]
    async fn wired_history_summarizer_happy_path_sends_validates_and_publishes() {
        let dir = tempfile::tempdir().unwrap();
        let main_store = store(dir.path());
        seed_prior_history_segment(&main_store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        let models = vec!["prov/model-a".to_string()];
        let text = history_summarizer_xml("second arc full and exact");
        let mut producer = ScriptedProducer::default()
            .with_start(Ok(run_handle("run-1")))
            .with_output(Ok(producer_output(text)));

        let outcome = run_history_summarizer_firing(
            &mut producer,
            fire_request(&main_store, "placeholder prompt", &models, &chunk, &prior),
        )
        .await
        .unwrap();

        let HistorySummarizerDriveOutcome::Completed(success) = outcome else {
            panic!("expected completed outcome");
        };
        assert_eq!(success.model, "prov/model-a");
        let expected_session = history_summarizer_producer_session_id("proj", "ses", 1);
        assert_eq!(success.producer_session_id, expected_session);
        assert_eq!(producer.observed_starts.len(), 1);
        assert_eq!(producer.observed_starts[0].0, expected_session);
        assert_eq!(
            producer.observed_systems,
            vec!["role guidance".to_string()],
            "exactly ONE send carries the system prompt; a reattach path never re-sends \
             (system rides the run's durable input, re-drained not re-sent)"
        );
        assert_eq!(producer.await_run_ids, vec!["run-1"]);

        let loaded = main_store.load("ses").unwrap();
        assert_eq!(
            loaded.meta.history_summarizer.state,
            HistorySummarizerPhase::Idle
        );
        assert_eq!(loaded.meta.publication_floor_ordinal, Some(4));
        let comps = main_store.load_history_segments("ses").unwrap();
        assert_eq!(
            comps.len(),
            2,
            "prior C1 preserved and history_summarizer C2 appended"
        );
        let c2 = comps.last().unwrap();
        assert_eq!(c2.end_message_id, "m3#0");
        assert_eq!(c2.p1.as_deref(), Some("second arc full and exact"));
        assert_eq!(c2.created_at, 123);
    }

    #[tokio::test]
    async fn selected_range_identity_drift_during_await_rejects_without_cooldown() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(store(dir.path()));
        seed_prior_history_segment(&store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        let models = vec!["prov/model-a".to_string()];
        let hook_store = std::sync::Arc::clone(&store);
        let mut producer = ScriptedProducer::default()
            .with_start(Ok(run_handle("run-1")))
            .with_output(Ok(producer_output(history_summarizer_xml("stale summary"))))
            .with_await_output_hook(move || {
                let loaded = hook_store.load("ses").unwrap();
                let mut meta = loaded.meta;
                meta.block_identity_by_mid.get_mut("m2").unwrap()[0].byte_fingerprint =
                    "m2-content-b".to_string();
                hook_store
                    .commit("ses", loaded.row_version, &loaded.core, &meta)
                    .unwrap();
            });

        let error = run_history_summarizer_firing(
            &mut producer,
            fire_request(&store, "placeholder prompt", &models, &chunk, &prior),
        )
        .await
        .unwrap_err();

        assert!(matches!(
            error,
            HistorySummarizerDriveError::State(HistorySummarizerStateError::Publish(
                HistorySummarizerPublishError::FenceRejected { .. }
            ))
        ));
        let loaded = store.load("ses").unwrap();
        assert_eq!(
            loaded.meta.history_summarizer.state,
            HistorySummarizerPhase::Idle
        );
        assert_eq!(loaded.meta.history_summarizer.failure_backoff_at_ms, None);
        assert_eq!(loaded.meta.publication_floor_ordinal, None);
        assert_eq!(store.load_history_segments("ses").unwrap().len(), 1);
        assert!(
            store
                .load_chunk_transcripts_for_range("ses", 2, 4)
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn tail_identity_extension_during_await_still_publishes() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(store(dir.path()));
        seed_prior_history_segment(&store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        let models = vec!["prov/model-a".to_string()];
        let hook_store = std::sync::Arc::clone(&store);
        let mut producer = ScriptedProducer::default()
            .with_start(Ok(run_handle("run-1")))
            .with_output(Ok(producer_output(history_summarizer_xml(
                "current summary",
            ))))
            .with_await_output_hook(move || {
                let loaded = hook_store.load("ses").unwrap();
                let mut meta = loaded.meta;
                meta.block_identity_by_mid.insert(
                    "m5".to_string(),
                    vec![memory_store::BlockIdentity {
                        kind_tag: "text".to_string(),
                        byte_fingerprint: "later-content".to_string(),
                    }],
                );
                hook_store
                    .commit("ses", loaded.row_version, &loaded.core, &meta)
                    .unwrap();
            });

        let outcome = run_history_summarizer_firing(
            &mut producer,
            fire_request(&store, "placeholder prompt", &models, &chunk, &prior),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            HistorySummarizerDriveOutcome::Completed(_)
        ));
        let loaded = store.load("ses").unwrap();
        assert_eq!(loaded.meta.publication_floor_ordinal, Some(4));
        assert!(loaded.meta.block_identity_by_mid.contains_key("m5"));
        assert_eq!(store.load_history_segments("ses").unwrap().len(), 2);
    }

    #[tokio::test]
    async fn fallback_retry_uses_new_session_and_overflow_short_circuits() {
        let dir = tempfile::tempdir().unwrap();
        let fallback_store = store(dir.path());
        seed_prior_history_segment(&fallback_store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        let models = vec!["prov/model-a".to_string(), "prov/model-b".to_string()];
        let mut producer = ScriptedProducer::default()
            .with_start(Err(
                HistorySummarizerProducerError::retryable_model_failure("provider overloaded"),
            ))
            .with_start(Ok(run_handle("run-2")))
            .with_output(Ok(producer_output(history_summarizer_xml(
                "fallback model summary",
            ))));

        let outcome = run_history_summarizer_firing(
            &mut producer,
            fire_request(
                &fallback_store,
                "placeholder prompt",
                &models,
                &chunk,
                &prior,
            ),
        )
        .await
        .unwrap();
        let HistorySummarizerDriveOutcome::Completed(success) = outcome else {
            panic!("expected completed fallback outcome");
        };
        assert_eq!(success.model, "prov/model-b");
        assert_eq!(
            producer.observed_starts,
            vec![
                (
                    history_summarizer_producer_session_id("proj", "ses", 1),
                    "prov/model-a".to_string()
                ),
                (
                    history_summarizer_producer_session_id("proj", "ses", 2),
                    "prov/model-b".to_string()
                ),
            ],
            "fallback retries author a new session/run instead of resuming under another model"
        );
        assert_eq!(
            fallback_store
                .load("ses")
                .unwrap()
                .meta
                .history_summarizer
                .firing_seq,
            2
        );

        let dir = tempfile::tempdir().unwrap();
        let overflow_store = store(dir.path());
        seed_prior_history_segment(&overflow_store);
        let mut overflow = ScriptedProducer::default().with_start(Err(
            HistorySummarizerProducerError::context_overflow("context window exceeded"),
        ));
        let err = run_history_summarizer_firing(
            &mut overflow,
            fire_request(
                &overflow_store,
                "placeholder prompt",
                &models,
                &chunk,
                &prior,
            ),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, HistorySummarizerDriveError::Producer(_)));
        assert_eq!(
            overflow.observed_starts.len(),
            1,
            "overflow does not try the next model"
        );
        let state = overflow_store.load("ses").unwrap().meta.history_summarizer;
        assert_eq!(state.state, HistorySummarizerPhase::Idle);
        assert_eq!(state.firing_seq, 1);
    }

    #[tokio::test]
    async fn cross_incarnation_unknown_records_completion_backoff_without_fallback() {
        fn completed_at() -> i64 {
            10_000
        }

        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        seed_prior_history_segment(&store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        let models = vec!["prov/model-a".to_owned(), "prov/model-b".to_owned()];
        let mut producer = ScriptedProducer::default().with_start(Err(
            HistorySummarizerProducerError::CrossIncarnationUnknown {
                daemon_changed: true,
                identity_changed: false,
            },
        ));
        let mut request = fire_request(&store, "placeholder prompt", &models, &chunk, &prior);
        request.completion_now_ms = completed_at;

        let error = run_history_summarizer_firing(&mut producer, request)
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            HistorySummarizerDriveError::Producer(
                HistorySummarizerProducerError::CrossIncarnationUnknown { .. }
            )
        ));
        assert_eq!(producer.observed_starts.len(), 1);
        assert_eq!(producer.closes, 1);
        let history_summarizer = store.load("ses").unwrap().meta.history_summarizer;
        assert_eq!(history_summarizer.state, HistorySummarizerPhase::Idle);
        assert_eq!(history_summarizer.failure_backoff_at_ms, Some(10_876));
    }

    #[tokio::test]
    async fn permanent_class_advances_chain_immediately() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        seed_prior_history_segment(&store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        let models = vec!["prov/model-a".to_string(), "prov/model-b".to_string()];
        let mut producer = ScriptedProducer::default()
            .with_start(Err(HistorySummarizerProducerError::tagged_call(
                "provider_error",
                "model id does not exist",
                ErrorClass::Permanent,
                None,
            )))
            .with_start(Ok(run_handle("run-2")))
            .with_output(Ok(producer_output(history_summarizer_xml(
                "fallback after permanent",
            ))));

        let outcome = run_history_summarizer_firing(
            &mut producer,
            fire_request(&store, "placeholder prompt", &models, &chunk, &prior),
        )
        .await
        .unwrap();

        let HistorySummarizerDriveOutcome::Completed(success) = outcome else {
            panic!("expected permanent failure to advance to fallback model");
        };
        assert_eq!(success.model, "prov/model-b");
        assert_eq!(producer.observed_starts.len(), 2);
        assert_eq!(producer.attempt_closes, 1);
        assert_eq!(producer.closes, 1);
        assert!(producer.connection_closed);
    }

    #[tokio::test]
    async fn output_failure_closes_attempt_routes_but_keeps_connection_for_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        seed_prior_history_segment(&store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        let models = vec!["prov/model-a".to_string(), "other/model-b".to_string()];
        let mut producer = ScriptedProducer::default()
            .with_start(Ok(run_handle("run-1")))
            .with_output(Err(HistorySummarizerProducerError::tagged_call(
                "provider_error",
                "provider unavailable",
                ErrorClass::Transient,
                None,
            )))
            .with_start(Ok(run_handle("run-2")))
            .with_output(Ok(producer_output(history_summarizer_xml(
                "fallback output",
            ))));

        let outcome = run_history_summarizer_firing(
            &mut producer,
            fire_request(&store, "placeholder prompt", &models, &chunk, &prior),
        )
        .await
        .expect("fallback uses still-open managed connection");

        let HistorySummarizerDriveOutcome::Completed(success) = outcome else {
            panic!("expected fallback completion");
        };
        assert_eq!(success.model, "other/model-b");
        assert_eq!(producer.observed_starts.len(), 2);
        assert_eq!(producer.attempt_closes, 1);
        assert_eq!(producer.closes, 1);
        assert!(producer.connection_closed);
    }

    #[tokio::test]
    async fn chain_exhausted_all_permanent_records_marker() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        seed_prior_history_segment(&store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        let models = vec!["prov/model-a".to_string(), "other/model-b".to_string()];
        let mut producer = ScriptedProducer::default()
            .with_start(Err(HistorySummarizerProducerError::tagged_call(
                "provider_error",
                "model a is permanently unavailable",
                ErrorClass::Permanent,
                None,
            )))
            .with_start(Err(HistorySummarizerProducerError::tagged_call(
                "provider_error",
                "model b is permanently unavailable",
                ErrorClass::Permanent,
                None,
            )));

        let err = run_history_summarizer_firing(
            &mut producer,
            fire_request(&store, "placeholder prompt", &models, &chunk, &prior),
        )
        .await
        .unwrap_err();

        assert!(matches!(err, HistorySummarizerDriveError::Producer(_)));
        assert_eq!(producer.observed_starts.len(), 2);
        let state = store.load("ses").unwrap().meta.history_summarizer;
        assert_eq!(state.failure_backoff_at_ms, Some(999));
        assert!(
            state
                .last_failure
                .as_deref()
                .is_some_and(|detail| detail.starts_with(CHAIN_EXHAUSTED_PERMANENT_PREFIX)),
            "all-permanent chain exhaustion must be visible in durable state: {:?}",
            state.last_failure
        );
    }

    /// `retry_after_secs` is a floor input: a short value cannot shorten the
    /// history_summarizer schedule, and a longer value pushes the backoff out.
    #[tokio::test]
    async fn transient_retry_after_only_extends_the_backoff_schedule() {
        for (retry_after_secs, expected_backoff_at_ms) in [
            (5, 123 + HISTORY_SUMMARIZER_FAILURE_BACKOFF_MS),
            (120, 123 + 120_000),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let store = store(dir.path());
            seed_prior_history_segment(&store);
            let chunk = history_summarizer_chunk();
            let prior = prior_ranges();
            let models = vec!["prov/model-a".to_string()];
            let mut producer = ScriptedProducer::default().with_start(Err(
                HistorySummarizerProducerError::tagged_call(
                    "provider_error",
                    "rate limited",
                    ErrorClass::Transient,
                    Some(retry_after_secs),
                ),
            ));

            let err = run_history_summarizer_firing(
                &mut producer,
                fire_request(&store, "placeholder prompt", &models, &chunk, &prior),
            )
            .await
            .unwrap_err();

            assert!(matches!(err, HistorySummarizerDriveError::Producer(_)));
            let state = store.load("ses").unwrap().meta.history_summarizer;
            assert_eq!(
                state.failure_backoff_at_ms,
                Some(expected_backoff_at_ms),
                "retry_after_secs={retry_after_secs}"
            );
        }
    }

    #[tokio::test]
    async fn auth_required_skips_same_provider_and_tries_different_provider() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        seed_prior_history_segment(&store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        let models = vec![
            "openai/model-a".to_string(),
            "openai/model-b".to_string(),
            "anthropic/model-c".to_string(),
        ];
        let mut producer = ScriptedProducer::default()
            .with_start(Err(HistorySummarizerProducerError::tagged_call(
                "provider_error",
                "credential needs re-authentication",
                ErrorClass::AuthRequired,
                None,
            )))
            .with_start(Ok(run_handle("run-3")))
            .with_output(Ok(producer_output(history_summarizer_xml(
                "different provider summary",
            ))));

        let outcome = run_history_summarizer_firing(
            &mut producer,
            fire_request(&store, "placeholder prompt", &models, &chunk, &prior),
        )
        .await
        .unwrap();

        let HistorySummarizerDriveOutcome::Completed(success) = outcome else {
            panic!("expected auth failure to try the different provider fallback");
        };
        assert_eq!(success.model, "anthropic/model-c");
        assert_eq!(
            producer.observed_starts,
            vec![
                (
                    history_summarizer_producer_session_id("proj", "ses", 1),
                    "openai/model-a".to_string()
                ),
                (
                    history_summarizer_producer_session_id("proj", "ses", 2),
                    "anthropic/model-c".to_string()
                ),
            ],
            "same-provider auth alternatives are skipped without opening a producer session"
        );
    }

    #[tokio::test]
    async fn auth_required_all_same_provider_records_marker() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        seed_prior_history_segment(&store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        let models = vec!["openai/model-a".to_string(), "openai/model-b".to_string()];
        let mut producer = ScriptedProducer::default().with_start(Err(
            HistorySummarizerProducerError::tagged_call(
                "provider_error",
                "credential needs re-authentication",
                ErrorClass::AuthRequired,
                None,
            ),
        ));

        let err = run_history_summarizer_firing(
            &mut producer,
            fire_request(&store, "placeholder prompt", &models, &chunk, &prior),
        )
        .await
        .unwrap_err();

        assert!(matches!(err, HistorySummarizerDriveError::Producer(_)));
        assert_eq!(producer.observed_starts.len(), 1);
        let state = store.load("ses").unwrap().meta.history_summarizer;
        assert!(
            state
                .last_failure
                .as_deref()
                .is_some_and(|detail| detail.starts_with(AUTH_REQUIRED_PREFIX)),
            "same-provider auth exhaustion must be visible in durable state: {:?}",
            state.last_failure
        );
    }

    #[tokio::test]
    async fn tagged_context_overflow_short_circuits_chain() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        seed_prior_history_segment(&store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        let models = vec!["prov/model-a".to_string(), "other/model-b".to_string()];
        let mut producer = ScriptedProducer::default().with_start(Err(
            HistorySummarizerProducerError::tagged_call(
                "provider_error",
                "context window exceeded",
                ErrorClass::ContextOverflow,
                None,
            ),
        ));

        let err = run_history_summarizer_firing(
            &mut producer,
            fire_request(&store, "placeholder prompt", &models, &chunk, &prior),
        )
        .await
        .unwrap_err();

        assert!(matches!(err, HistorySummarizerDriveError::Producer(_)));
        assert_eq!(producer.observed_starts.len(), 1);
    }

    #[tokio::test]
    async fn untagged_error_uses_deprecated_heuristic_and_counts_it() {
        crate::history_summarizer_producer::reset_deprecated_heuristic_uses_for_test();
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        seed_prior_history_segment(&store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        let models = vec!["prov/model-a".to_string(), "prov/model-b".to_string()];
        let mut producer = ScriptedProducer::default()
            .with_start(Err(
                HistorySummarizerProducerError::retryable_model_failure("provider overloaded"),
            ))
            .with_start(Ok(run_handle("run-2")))
            .with_output(Ok(producer_output(history_summarizer_xml(
                "heuristic fallback",
            ))));

        let outcome = run_history_summarizer_firing(
            &mut producer,
            fire_request(&store, "placeholder prompt", &models, &chunk, &prior),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            HistorySummarizerDriveOutcome::Completed(_)
        ));
        assert!(
            crate::history_summarizer_producer::deprecated_heuristic_uses() >= 1,
            "an untagged producer error must increment the migration counter"
        );
    }

    #[tokio::test]
    async fn tagged_error_ignores_contradicting_heuristic_text() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        seed_prior_history_segment(&store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        let models = vec!["prov/model-a".to_string(), "prov/model-b".to_string()];
        let mut producer = ScriptedProducer::default()
            .with_start(Err(HistorySummarizerProducerError::tagged_call(
                "context_overflow",
                "overflow text would block retry under the deprecated heuristic",
                ErrorClass::Permanent,
                None,
            )))
            .with_start(Ok(run_handle("run-2")))
            .with_output(Ok(producer_output(history_summarizer_xml(
                "tag wins summary",
            ))));

        let outcome = run_history_summarizer_firing(
            &mut producer,
            fire_request(&store, "placeholder prompt", &models, &chunk, &prior),
        )
        .await
        .unwrap();

        let HistorySummarizerDriveOutcome::Completed(success) = outcome else {
            panic!("expected permanent tag to advance despite overflow text");
        };
        assert_eq!(success.model, "prov/model-b");
    }

    #[tokio::test]
    async fn reattach_terminal_redrains_from_start_without_second_send() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        seed_prior_history_segment(&store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        seed_awaiting_history_summarizer(&store);
        let mut producer = ScriptedProducer::default()
            .with_status(Ok(RunState::Terminal))
            .with_output(Ok(producer_output(history_summarizer_xml(
                "terminal replay summary",
            ))));

        let outcome = reattach_history_summarizer_producer(
            &mut producer,
            reattach_request(&store, &chunk, &prior),
        )
        .await
        .unwrap();
        assert!(matches!(
            outcome,
            HistorySummarizerReattachOutcome::Published(_)
        ));
        assert!(
            producer.observed_starts.is_empty(),
            "reattach publishes replayed output without a second session.send"
        );
        assert_eq!(producer.observed_sessions, vec!["producer-session"]);
        assert_eq!(producer.await_run_ids, vec!["run-1"]);
        let c2 = store.load_history_segments("ses").unwrap().pop().unwrap();
        assert_eq!(c2.p1.as_deref(), Some("terminal replay summary"));
    }

    #[tokio::test]
    async fn reattach_equal_length_identity_drift_rejects_before_publish() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        seed_prior_history_segment(&store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        seed_awaiting_history_summarizer(&store);
        let loaded = store.load("ses").unwrap();
        let mut meta = loaded.meta;
        meta.block_identity_by_mid.get_mut("m2").unwrap()[0].byte_fingerprint =
            "m2-content-b".to_string();
        store
            .commit("ses", loaded.row_version, &loaded.core, &meta)
            .unwrap();
        let mut producer = ScriptedProducer::default()
            .with_status(Ok(RunState::Terminal))
            .with_output(Ok(producer_output(history_summarizer_xml(
                "stale replay summary",
            ))));

        let error = reattach_history_summarizer_producer(
            &mut producer,
            reattach_request(&store, &chunk, &prior),
        )
        .await
        .unwrap_err();

        assert!(matches!(
            error,
            HistorySummarizerDriveError::State(HistorySummarizerStateError::Publish(
                HistorySummarizerPublishError::FenceRejected { .. }
            ))
        ));
        let loaded = store.load("ses").unwrap();
        assert_eq!(
            loaded.meta.history_summarizer.state,
            HistorySummarizerPhase::Idle
        );
        assert_eq!(loaded.meta.history_summarizer.failure_backoff_at_ms, None);
        assert_eq!(loaded.meta.publication_floor_ordinal, None);
        assert_eq!(store.load_history_segments("ses").unwrap().len(), 1);
    }

    #[tokio::test]
    async fn concurrent_lineages_reattach_and_publish_in_isolated_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        let left_lineage = "lineage-ZiDxmBSjhQbv";
        let right_lineage = "lineage-OPeZDtvlh9LD";
        let left_producer_session = history_summarizer_producer_session_id("proj", left_lineage, 1);
        let right_producer_session =
            history_summarizer_producer_session_id("proj", right_lineage, 1);
        assert_ne!(left_producer_session, right_producer_session);

        for (lineage, producer_session, run_id) in [
            (left_lineage, left_producer_session.as_str(), "run-left"),
            (right_lineage, right_producer_session.as_str(), "run-right"),
        ] {
            store
                .replace_history_segments(lineage, &[comp(1, 1, 1, "m1", "C1 summary")])
                .unwrap();
            let fired = match fire(
                &HistorySummarizerDurableState::default(),
                2,
                4,
                "fp".into(),
                test_selected_range_identities(),
                0,
                HistorySegmentSetGeneration {
                    max_sequence: 1,
                    count: 1,
                },
                1,
            )
            .unwrap()
            {
                FireOutcome::Fired(state) => state,
                FireOutcome::Busy(_) => unreachable!(),
            };
            let awaiting = producer_started(
                &fired,
                producer_session.to_string(),
                run_id.to_string(),
                "pi".into(),
            )
            .unwrap();
            store
                .commit(
                    lineage,
                    None,
                    &CoreState::empty(),
                    &test_meta_with_history_summarizer(awaiting),
                )
                .unwrap();
        }

        let mut left_producer = ScriptedProducer::default()
            .with_status(Ok(RunState::Terminal))
            .with_output(Ok(producer_output(history_summarizer_xml("left summary"))));
        let mut right_producer = ScriptedProducer::default()
            .with_status(Ok(RunState::Terminal))
            .with_output(Ok(producer_output(history_summarizer_xml("right summary"))));
        let left_request = HistorySummarizerReattachRequest {
            store: &store,
            session_id: left_lineage,
            project_path: "git:proj",
            observed_chunk_fingerprint: "fp",
            validation_chunk: &chunk,
            chunk_transcript: "U: left transcript",

            boundary_dates: empty_boundary_dates(),
            prior_history_segments: &prior,
            validate_options: validate_options(),
            publication_floor_ordinal: 4,
            now_ms: 123,
            failure_backoff_at_ms: 999,
            completion_now_ms: || 123,
            publication_fence: None,
            curator_handoff: None,
        };
        let right_request = HistorySummarizerReattachRequest {
            store: &store,
            session_id: right_lineage,
            project_path: "git:proj",
            observed_chunk_fingerprint: "fp",
            validation_chunk: &chunk,
            chunk_transcript: "U: right transcript",

            boundary_dates: empty_boundary_dates(),
            prior_history_segments: &prior,
            validate_options: validate_options(),
            publication_floor_ordinal: 4,
            now_ms: 123,
            failure_backoff_at_ms: 999,
            completion_now_ms: || 123,
            publication_fence: None,
            curator_handoff: None,
        };

        let (left, right) = tokio::join!(
            reattach_history_summarizer_producer(&mut left_producer, left_request),
            reattach_history_summarizer_producer(&mut right_producer, right_request),
        );

        assert!(matches!(
            left.unwrap(),
            HistorySummarizerReattachOutcome::Published(_)
        ));
        assert!(matches!(
            right.unwrap(),
            HistorySummarizerReattachOutcome::Published(_)
        ));
        assert_eq!(left_producer.observed_sessions, vec![left_producer_session]);
        assert_eq!(
            right_producer.observed_sessions,
            vec![right_producer_session]
        );
        let left_tail = store
            .load_history_segments(left_lineage)
            .unwrap()
            .pop()
            .unwrap();
        let right_tail = store
            .load_history_segments(right_lineage)
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(left_tail.p1.as_deref(), Some("left summary"));
        assert_eq!(right_tail.p1.as_deref(), Some("right summary"));
    }

    #[tokio::test]
    async fn reattach_missing_abandons_and_releases_single_flight() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        let fired = match fire(
            &HistorySummarizerDurableState::default(),
            2,
            4,
            "fp".into(),
            test_selected_range_identities(),
            0,
            HistorySegmentSetGeneration::default(),
            1,
        )
        .unwrap()
        {
            FireOutcome::Fired(state) => state,
            FireOutcome::Busy(_) => unreachable!(),
        };
        let awaiting = producer_started(
            &fired,
            "producer-session".into(),
            "run-1".into(),
            "pi".into(),
        )
        .unwrap();
        store
            .commit(
                "ses",
                None,
                &CoreState::empty(),
                &test_meta_with_history_summarizer(awaiting),
            )
            .unwrap();
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        let mut producer = ScriptedProducer::default().with_status(Ok(RunState::Missing {
            detail: Some("gone".into()),
        }));

        let outcome = reattach_history_summarizer_producer(
            &mut producer,
            reattach_request(&store, &chunk, &prior),
        )
        .await
        .unwrap();
        assert_eq!(
            outcome,
            HistorySummarizerReattachOutcome::RefireEligible { firing_seq: 1 }
        );
        let state = store.load("ses").unwrap().meta.history_summarizer;
        assert_eq!(state.state, HistorySummarizerPhase::Idle);
        assert!(matches!(
            fire(
                &state,
                2,
                4,
                "fp2".into(),
                Vec::new(),
                0,
                HistorySegmentSetGeneration::default(),
                2,
            )
            .unwrap(),
            FireOutcome::Fired(_)
        ));
    }

    #[tokio::test]
    async fn producer_timeout_redrain_recovers_completed_run_without_abandon() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        seed_prior_history_segment(&store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        let models = vec!["prov/model-a".to_string()];
        let mut producer = ScriptedProducer::default()
            .with_start(Ok(run_handle("run-1")))
            .with_output(Err(HistorySummarizerProducerError::TimedOut))
            .with_output(Ok(producer_output(history_summarizer_xml(
                "recovered summary",
            ))));

        let outcome = run_history_summarizer_firing(
            &mut producer,
            fire_request(&store, "placeholder prompt", &models, &chunk, &prior),
        )
        .await
        .unwrap();
        assert!(matches!(
            outcome,
            HistorySummarizerDriveOutcome::Completed(_)
        ));
        assert_eq!(producer.await_run_ids, vec!["run-1", "run-1"]);
        assert!(
            producer.cancels.is_empty(),
            "successful recovery must not abandon the run"
        );
        let state = store.load("ses").unwrap().meta.history_summarizer;
        assert_eq!(state.state, HistorySummarizerPhase::Idle);
        assert_eq!(state.failure_backoff_at_ms, None);
        assert_eq!(state.last_failure, None);
        let c2 = store.load_history_segments("ses").unwrap().pop().unwrap();
        assert_eq!(c2.p1.as_deref(), Some("recovered summary"));
    }

    #[tokio::test]
    async fn producer_timeout_recovery_timeout_abandons_and_best_effort_cancels() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        seed_prior_history_segment(&store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        let models = vec!["prov/model-a".to_string()];
        let mut producer = ScriptedProducer::default()
            .with_start(Ok(run_handle("run-1")))
            .with_output(Err(HistorySummarizerProducerError::TimedOut))
            .with_output(Err(HistorySummarizerProducerError::TimedOut));

        let err = run_history_summarizer_firing(
            &mut producer,
            fire_request(&store, "placeholder prompt", &models, &chunk, &prior),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, HistorySummarizerDriveError::Producer(_)));
        assert_eq!(producer.await_run_ids, vec!["run-1", "run-1"]);
        assert_eq!(producer.cancels, vec!["run-1"]);
        let state = store.load("ses").unwrap().meta.history_summarizer;
        assert_eq!(state.state, HistorySummarizerPhase::Idle);
        assert_eq!(state.failure_backoff_at_ms, Some(999));
        let detail = state.last_failure.expect("failure detail recorded");
        assert!(
            detail.contains("timed out; recovery re-drain also failed")
                && detail.contains("prov/model-a"),
            "durable failure detail keeps the timeout + recovery cause: {detail}"
        );
    }

    #[tokio::test]
    async fn cancel_and_close_failures_surface_without_corrupting_abandon() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        seed_prior_history_segment(&store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        let models = vec!["prov/model-a".to_string()];
        let mut producer = ScriptedProducer::default()
            .with_start(Ok(run_handle("run-1")))
            .with_output(Err(HistorySummarizerProducerError::tagged_call(
                "provider_error",
                "model gone",
                ErrorClass::Permanent,
                None,
            )))
            .with_cancel_result(Err(HistorySummarizerProducerError::TimedOut))
            .with_close_result(Err(HistorySummarizerProducerError::TimedOut));

        let err = run_history_summarizer_firing(
            &mut producer,
            fire_request(&store, "placeholder prompt", &models, &chunk, &prior),
        )
        .await
        .unwrap_err();

        let HistorySummarizerDriveError::Producer(producer_err) = err else {
            panic!("expected producer error, got {err:?}");
        };
        assert_eq!(
            producer_err.classification().map(|c| c.class),
            Some(ErrorClass::Permanent),
            "cleanup failures must not change the primary retry policy"
        );
        let rendered = producer_err.to_string();
        assert!(
            rendered.contains("model gone") && rendered.contains("cleanup also failed"),
            "both diagnostics must survive: {rendered}"
        );
        assert_eq!(producer.cancels, vec!["run-1"]);
        assert_eq!(producer.closes, 1);
        let state = store.load("ses").unwrap().meta.history_summarizer;
        assert_eq!(
            state.state,
            HistorySummarizerPhase::Idle,
            "the durable abandon transition survives cleanup failures"
        );
        assert!(state.failure_backoff_at_ms.is_some());
        assert!(
            state
                .last_failure
                .expect("failure detail recorded")
                .contains("producer output")
        );
    }

    /// An unconfirmed cancellation cannot authorize another billable run.
    #[tokio::test]
    async fn unconfirmed_cancellation_stops_the_fallback_chain() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        seed_prior_history_segment(&store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        let models = vec!["prov/model-a".to_string(), "prov/model-b".to_string()];
        let mut producer = ScriptedProducer::default()
            .with_start(Ok(run_handle("run-1")))
            .with_output(Err(HistorySummarizerProducerError::tagged_call(
                "provider_error",
                "provider overloaded",
                ErrorClass::Transient,
                None,
            )))
            .with_cancel_result(Err(HistorySummarizerProducerError::tagged_call(
                "teardown_unconfirmed",
                "the harness process group could not be confirmed stopped",
                ErrorClass::Transient,
                None,
            )));

        let err = run_history_summarizer_firing(
            &mut producer,
            fire_request(&store, "placeholder prompt", &models, &chunk, &prior),
        )
        .await
        .unwrap_err();

        let HistorySummarizerDriveError::Producer(producer_err) = err else {
            panic!("expected producer error, got {err:?}");
        };
        let rendered = producer_err.to_string();
        assert!(
            rendered.contains("teardown_unconfirmed"),
            "the unconfirmed cancellation must surface: {rendered}"
        );
        assert_eq!(producer.cancels, vec!["run-1"]);
        assert_eq!(
            producer.observed_starts.len(),
            1,
            "the fallback chain must not start another run"
        );
        let state = store.load("ses").unwrap().meta.history_summarizer;
        assert_eq!(
            state.state,
            HistorySummarizerPhase::Idle,
            "the durable abandon transition still lands"
        );
        assert!(state.failure_backoff_at_ms.is_some());
    }

    /// Only `Ok(())` from cancel proves the provider run stopped. Every untagged
    /// cancel failure, whatever its send outcome or terminal code, leaves the run
    /// unproven and must not authorize a second billable run.
    #[tokio::test]
    async fn unproven_cancel_failures_never_authorize_fallback() {
        for (outcome, code) in [
            (
                HistorySummarizerSendOutcome::NotSent,
                "cancel_transport_failure",
            ),
            (
                HistorySummarizerSendOutcome::OutcomeUnknown,
                "cancel_transport_failure",
            ),
            (HistorySummarizerSendOutcome::Terminal, "queue_full"),
            (HistorySummarizerSendOutcome::Terminal, "closed"),
            (
                HistorySummarizerSendOutcome::Terminal,
                "teardown_unconfirmed",
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let store = store(dir.path());
            seed_prior_history_segment(&store);
            let chunk = history_summarizer_chunk();
            let prior = prior_ranges();
            let models = vec!["prov/model-a".to_string(), "prov/model-b".to_string()];
            let mut producer = ScriptedProducer::default()
                .with_start(Ok(run_handle("run-1")))
                .with_output(Err(HistorySummarizerProducerError::tagged_call(
                    "provider_error",
                    "provider overloaded",
                    ErrorClass::Transient,
                    None,
                )))
                .with_cancel_result(Err(HistorySummarizerProducerError::Call(
                    crate::history_summarizer_producer::HistorySummarizerCallFailure::untagged(
                        outcome,
                        code,
                        "cancel did not prove the run stopped",
                    ),
                )));

            let err = run_history_summarizer_firing(
                &mut producer,
                fire_request(&store, "placeholder prompt", &models, &chunk, &prior),
            )
            .await
            .unwrap_err();

            assert!(matches!(err, HistorySummarizerDriveError::Producer(_)));
            assert_eq!(producer.cancels, vec!["run-1"]);
            assert_eq!(
                producer.observed_starts.len(),
                1,
                "{outcome:?} `{code}` cancellation must not start a second billable run"
            );
            let state = store.load("ses").unwrap().meta.history_summarizer;
            assert_eq!(state.state, HistorySummarizerPhase::Idle);
            assert_eq!(state.failure_backoff_at_ms, Some(999));
        }
    }

    #[tokio::test]
    async fn close_failure_after_publish_keeps_the_completed_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        seed_prior_history_segment(&store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        let models = vec!["prov/model-a".to_string()];
        let mut producer = ScriptedProducer::default()
            .with_start(Ok(run_handle("run-1")))
            .with_output(Ok(producer_output(history_summarizer_xml(
                "published summary",
            ))))
            .with_close_result(Err(HistorySummarizerProducerError::TimedOut));

        let outcome = run_history_summarizer_firing(
            &mut producer,
            fire_request(&store, "placeholder prompt", &models, &chunk, &prior),
        )
        .await
        .unwrap();

        assert!(
            matches!(outcome, HistorySummarizerDriveOutcome::Completed(_)),
            "a route-release failure after publish must not refire published work"
        );
        assert_eq!(producer.closes, 1);
        let state = store.load("ses").unwrap().meta.history_summarizer;
        assert_eq!(state.state, HistorySummarizerPhase::Idle);
        assert_eq!(state.last_failure, None);
        let published = store.load_history_segments("ses").unwrap().pop().unwrap();
        assert_eq!(published.p1.as_deref(), Some("published summary"));
    }

    #[tokio::test]
    async fn producer_timeout_recovery_run_mismatch_abandons() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        seed_prior_history_segment(&store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        let models = vec!["prov/model-a".to_string()];
        let mut producer = ScriptedProducer::default()
            .with_start(Ok(run_handle("run-1")))
            .with_output(Err(HistorySummarizerProducerError::TimedOut))
            .with_output(Err(HistorySummarizerProducerError::TerminalRunMismatch {
                expected: "run-1".into(),
                found: Some("run-other".into()),
            }));

        let err = run_history_summarizer_firing(
            &mut producer,
            fire_request(&store, "placeholder prompt", &models, &chunk, &prior),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, HistorySummarizerDriveError::Producer(_)));
        assert_eq!(producer.await_run_ids, vec!["run-1", "run-1"]);
        assert_eq!(producer.cancels, vec!["run-1"]);
        let state = store.load("ses").unwrap().meta.history_summarizer;
        assert_eq!(state.state, HistorySummarizerPhase::Idle);
        assert_eq!(state.failure_backoff_at_ms, Some(999));
        let detail = state.last_failure.expect("failure detail recorded");
        assert!(
            detail.contains("timed out; recovery re-drain also failed")
                && detail.contains("run-1 received terminal control unit"),
            "a replay terminal for another run id must not publish this firing: {detail}"
        );
    }

    #[tokio::test]
    async fn length_capped_closed_output_is_rejected_before_publish() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        seed_prior_history_segment(&store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        let models = vec!["prov/model-a".to_string()];
        let mut capped = producer_output(history_summarizer_xml("closed but incomplete summary"));
        capped.length_capped = true;
        let mut producer = ScriptedProducer::default()
            .with_start(Ok(run_handle("run-cap")))
            .with_output(Ok(capped));

        let err = run_history_summarizer_firing(
            &mut producer,
            fire_request(&store, "placeholder prompt", &models, &chunk, &prior),
        )
        .await
        .unwrap_err();

        assert!(matches!(err, HistorySummarizerDriveError::Validation(_)));
        assert_eq!(store.load_history_segments("ses").unwrap().len(), 1);
        let state = store.load("ses").unwrap().meta.history_summarizer;
        assert_eq!(state.state, HistorySummarizerPhase::Idle);
        assert!(
            state
                .last_failure
                .as_deref()
                .is_some_and(|detail| detail.contains("length cap"))
        );
    }

    #[tokio::test]
    async fn truncated_output_validation_reject_records_cap_hint() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        seed_prior_history_segment(&store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        let models = vec!["prov/model-a".to_string()];
        let mut truncated = producer_output(history_summarizer_xml("truncated summary"));
        truncated.text.truncate(truncated.text.len() / 2);
        truncated.length_capped = true;
        let mut producer = ScriptedProducer::default()
            .with_start(Ok(run_handle("run-cap")))
            .with_output(Ok(truncated));

        let err = run_history_summarizer_firing(
            &mut producer,
            fire_request(&store, "placeholder prompt", &models, &chunk, &prior),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, HistorySummarizerDriveError::Validation(_)));
        let state = store.load("ses").unwrap().meta.history_summarizer;
        assert_eq!(state.state, HistorySummarizerPhase::Idle);
        let detail = state.last_failure.expect("validate reject records detail");
        assert!(
            detail.contains("validate rejected") && detail.contains("length cap"),
            "truncation self-diagnoses from the state dump: {detail}"
        );
    }

    #[tokio::test]
    async fn producer_start_failure_records_durable_detail() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        seed_prior_history_segment(&store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        let models = vec!["prov/model-a".to_string()];
        let mut producer =
            ScriptedProducer::default().with_start(Err(HistorySummarizerProducerError::Call(
                crate::history_summarizer_producer::HistorySummarizerCallFailure::untagged(
                    crate::history_summarizer_producer::HistorySummarizerSendOutcome::Terminal,
                    "route_rejected",
                    "no such module",
                ),
            )));

        let err = run_history_summarizer_firing(
            &mut producer,
            fire_request(&store, "placeholder prompt", &models, &chunk, &prior),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, HistorySummarizerDriveError::Producer(_)));
        let state = store.load("ses").unwrap().meta.history_summarizer;
        assert_eq!(state.state, HistorySummarizerPhase::Idle);
        let detail = state.last_failure.expect("failure detail recorded");
        assert!(
            detail.contains("producer start") && detail.contains("route_rejected"),
            "connect/bind-class failures are diagnosable from the state dump alone: {detail}"
        );

        let mut ok_producer = ScriptedProducer::default()
            .with_start(Ok(run_handle("run-2")))
            .with_output(Ok(producer_output(history_summarizer_xml(
                "recovery summary",
            ))));
        run_history_summarizer_firing(
            &mut ok_producer,
            fire_request(&store, "placeholder prompt", &models, &chunk, &prior),
        )
        .await
        .unwrap();
        let state = store.load("ses").unwrap().meta.history_summarizer;
        assert_eq!(
            state.last_failure, None,
            "success clears the failure detail"
        );
    }

    #[tokio::test]
    async fn run_paused_abandons_and_refires() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        seed_prior_history_segment(&store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        let models = vec!["prov/model-a".to_string(), "prov/model-b".to_string()];
        let mut producer = ScriptedProducer::default()
            .with_start(Ok(run_handle("run-1")))
            .with_output(Err(HistorySummarizerProducerError::RunPaused {
                run_id: "run-1".into(),
                reason: Some("auth_required".into()),
                classification: None,
                class_field_present: false,
            }));

        let err = run_history_summarizer_firing(
            &mut producer,
            fire_request(&store, "placeholder prompt", &models, &chunk, &prior),
        )
        .await
        .unwrap_err();

        assert!(matches!(err, HistorySummarizerDriveError::Producer(_)));
        assert_eq!(
            producer.observed_starts,
            vec![(
                history_summarizer_producer_session_id("proj", "ses", 1),
                "prov/model-a".to_string()
            )],
            "paused runs abandon the slot instead of retrying the next model"
        );
        assert_eq!(producer.cancels, vec!["run-1"]);
        let state = store.load("ses").unwrap().meta.history_summarizer;
        assert_eq!(state.state, HistorySummarizerPhase::Idle);
        assert_eq!(state.failure_backoff_at_ms, Some(999));
        assert!(matches!(
            fire(
                &state,
                2,
                4,
                "fp2".into(),
                Vec::new(),
                0,
                HistorySegmentSetGeneration::default(),
                124,
            )
            .unwrap(),
            FireOutcome::Fired(_)
        ));
    }

    #[tokio::test]
    async fn reattach_fingerprint_mismatch_recovers_to_idle_and_releases_routes() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        seed_prior_history_segment(&store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        seed_awaiting_history_summarizer(&store);
        let mut producer = ScriptedProducer::default()
            .with_status(Ok(RunState::Terminal))
            .with_output(Ok(producer_output(history_summarizer_xml(
                "summary for a changed tail",
            ))));

        let request = HistorySummarizerReattachRequest {
            store: &store,
            session_id: "ses",
            project_path: "git:proj",
            observed_chunk_fingerprint: "fp-changed",
            validation_chunk: &chunk,
            chunk_transcript: "U: transcript",

            boundary_dates: empty_boundary_dates(),
            prior_history_segments: &prior,
            validate_options: validate_options(),
            publication_floor_ordinal: 4,
            now_ms: 123,
            failure_backoff_at_ms: 999,
            completion_now_ms: || 123,
            publication_fence: None,
            curator_handoff: None,
        };
        let err = reattach_history_summarizer_producer(&mut producer, request)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            HistorySummarizerDriveError::State(
                HistorySummarizerStateError::FingerprintMismatch { .. }
            )
        ));
        let state = store.load("ses").unwrap().meta.history_summarizer;
        assert_eq!(
            state.state,
            HistorySummarizerPhase::Idle,
            "fingerprint mismatch must recover to Idle, never wedge in Publishing"
        );
        assert_eq!(state.failure_backoff_at_ms, Some(999));
        assert!(
            producer.closes >= 1,
            "routes must be released on the error path"
        );
    }

    #[tokio::test]
    async fn fresh_path_validation_rejection_releases_routes() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        seed_prior_history_segment(&store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        let models = vec!["prov/model-a".to_string()];
        let mut producer = ScriptedProducer::default()
            .with_start(Ok(run_handle("run-1")))
            .with_output(Ok(producer_output(
                "not a valid history_summarizer document".to_string(),
            )));

        let err = run_history_summarizer_firing(
            &mut producer,
            fire_request(&store, "placeholder prompt", &models, &chunk, &prior),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, HistorySummarizerDriveError::Validation(_)));
        assert_eq!(
            producer.closes, 1,
            "the route must be closed even when validate/publish errors out"
        );
        let state = store.load("ses").unwrap().meta.history_summarizer;
        assert_eq!(state.state, HistorySummarizerPhase::Idle);
    }

    #[tokio::test]
    async fn validation_rejection_advances_to_valid_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        seed_prior_history_segment(&store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        let models = vec!["prov/model-a".to_string(), "other/model-b".to_string()];
        let mut producer = ScriptedProducer::default()
            .with_start(Ok(run_handle("run-invalid")))
            .with_output(Ok(producer_output(flat_history_summarizer_xml(
                "flat primary",
            ))))
            .with_start(Ok(run_handle("run-valid")))
            .with_output(Ok(producer_output(history_summarizer_xml(
                "fallback summary",
            ))));

        let outcome = run_history_summarizer_firing(
            &mut producer,
            fire_request(&store, "placeholder prompt", &models, &chunk, &prior),
        )
        .await
        .expect("a valid fallback must publish after primary validation rejection");

        let HistorySummarizerDriveOutcome::Completed(success) = outcome else {
            panic!("expected completed fallback run");
        };
        assert_eq!(success.model, "other/model-b");
        assert_eq!(producer.observed_starts.len(), 2);
        assert_eq!(producer.attempt_closes, 1);
        assert_eq!(producer.closes, 1);
        assert!(producer.connection_closed);
        assert_eq!(
            store
                .load_history_summarizer_assembly_snapshot("ses")
                .unwrap()
                .history_segments
                .len(),
            2
        );
    }

    fn completion_after_long_model_run() -> i64 {
        120_000
    }

    #[tokio::test]
    async fn all_validation_rejections_preserve_final_error_and_start_cooldown_at_completion() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        seed_prior_history_segment(&store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        let models = vec!["prov/model-a".to_string(), "other/model-b".to_string()];
        let mut producer = ScriptedProducer::default()
            .with_start(Ok(run_handle("run-invalid-a")))
            .with_output(Ok(producer_output(flat_history_summarizer_xml(
                "flat primary",
            ))))
            .with_start(Ok(run_handle("run-invalid-b")))
            .with_output(Ok(producer_output(flat_history_summarizer_xml(
                "flat fallback",
            ))));
        let mut request = fire_request(&store, "placeholder prompt", &models, &chunk, &prior);
        request.now_ms = 0;
        request.failure_backoff_at_ms = HISTORY_SUMMARIZER_FAILURE_BACKOFF_MS;
        request.completion_now_ms = completion_after_long_model_run;

        let err = run_history_summarizer_firing(&mut producer, request)
            .await
            .expect_err("the final validation rejection must be returned");
        let HistorySummarizerDriveError::Validation(final_error) = err else {
            panic!("expected final validation error");
        };
        let state = store.load("ses").unwrap().meta.history_summarizer;

        assert_eq!(producer.observed_starts.len(), 2);
        assert_eq!(
            store
                .load_history_summarizer_assembly_snapshot("ses")
                .unwrap()
                .history_segments
                .len(),
            1,
            "flat retries must not publish any new history_segment rows"
        );
        assert_eq!(
            state.last_failure.as_deref(),
            Some(format!("validate rejected: {final_error}").as_str()),
        );
        assert_eq!(state.failure_backoff_at_ms, Some(180_000));
        assert!(
            completion_after_long_model_run() < state.failure_backoff_at_ms.unwrap(),
            "a model run longer than the cooldown must not leave immediate refire eligible"
        );
    }

    #[tokio::test]
    async fn narrative_gap_rejection_preserves_boundary_for_next_run() {
        use crate::history_summarizer_validate::ChunkLine;

        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        seed_prior_history_segment(&store);
        let chunk = HistorySummarizerChunk {
            aliases: Default::default(),
            start_index: 2,
            end_index: 9,
            lines: (2..=9)
                .map(|ordinal| ChunkLine {
                    ordinal,
                    message_id: format!("m{ordinal}#0"),
                    anchorable: true,
                })
                .collect(),
            present_ordinals: (1..=9).collect(),
            tool_only_ranges: vec![],
            completed_tool_arcs: vec![],
        };
        let prior = prior_ranges();
        let models = vec!["prov/model-a".to_string()];
        let rejected_output = r#"<output><history_segments>
<history_segment start="2" end="2" title="first" episode_type="feature" importance="50"><p1>first</p1><p2>first</p2><p3>first</p3><p4 /></history_segment>
<history_segment start="8" end="9" title="second" episode_type="feature" importance="50"><p1>second</p1><p2>second</p2><p3>second</p3><p4 /></history_segment>
</history_segments><meta><unprocessed_from>10</unprocessed_from></meta></output>"#;
        let mut rejected_request = fire_request(&store, "messages 2-9", &models, &chunk, &prior);
        rejected_request.to_ordinal = 9;
        let mut rejecting_producer = ScriptedProducer::default()
            .with_start(Ok(run_handle("run-gap")))
            .with_output(Ok(producer_output(rejected_output.to_string())));

        let error = run_history_summarizer_firing(&mut rejecting_producer, rejected_request)
            .await
            .expect_err("a five-message narrative gap must reject");
        assert!(matches!(error, HistorySummarizerDriveError::Validation(_)));
        let after_rejection = store
            .load_history_summarizer_assembly_snapshot("ses")
            .unwrap();
        assert_eq!(after_rejection.history_segments.len(), 1);
        assert_eq!(after_rejection.history_segments[0].end_message, 1);
        assert_eq!(
            store.load("ses").unwrap().meta.publication_floor_ordinal,
            None
        );

        let accepted_output = r#"<output><history_segments>
<history_segment start="2" end="9" title="re-read" episode_type="feature" importance="50"><p1>all messages re-read</p1><p2>re-read</p2><p3>re-read</p3><p4 /></history_segment>
</history_segments><meta><unprocessed_from>10</unprocessed_from></meta></output>"#;
        let mut retry_request = fire_request(&store, "messages 2-9", &models, &chunk, &prior);
        retry_request.to_ordinal = 9;
        retry_request.now_ms = 1_000;
        let mut accepting_producer = ScriptedProducer::default()
            .with_start(Ok(run_handle("run-reread")))
            .with_output(Ok(producer_output(accepted_output.to_string())));
        let outcome = run_history_summarizer_firing(&mut accepting_producer, retry_request)
            .await
            .expect("the same ordinals remain publishable on the next run");

        assert!(matches!(
            outcome,
            HistorySummarizerDriveOutcome::Completed(_)
        ));
        let after_retry = store
            .load_history_summarizer_assembly_snapshot("ses")
            .unwrap();
        assert_eq!(after_retry.history_segments.len(), 2);
        assert_eq!(after_retry.history_segments[1].start_message, 2);
        assert_eq!(after_retry.history_segments[1].end_message, 9);
        assert_eq!(
            store.load("ses").unwrap().meta.publication_floor_ordinal,
            Some(10)
        );
    }

    #[test]
    fn validated_output_drives_publish_end_to_end() {
        use crate::history_summarizer_validate::{
            ChunkLine, HistorySummarizerChunk, ValidateOptions, validate_history_summarizer_output,
        };

        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        store
            .replace_history_segments("ses", &[comp(1, 1, 1, "m1", "C1 summary")])
            .unwrap();

        let text = r#"<output>
<history_segments>
<history_segment start="2" end="3" title="second arc" episode_type="feature" importance="60">
<p1>second arc full and exact</p1>
<p2>second arc short</p2>
<p3>second arc</p3>
<p4 />
</history_segment>
</history_segments>
<facts><ARCHITECTURE>* [s1:0-11] Publish facts in the same flow.</ARCHITECTURE></facts>
<events><causal_incident at_history_segment="1"><summary>event survives</summary></causal_incident></events>
<primer_candidates><primer at_history_segment="1">What did this publish preserve?</primer></primer_candidates>
<user_observations>* [at_history_segment=1] The user prefers durable history.</user_observations>
<meta><messages_processed>2-3</messages_processed><unprocessed_from>4</unprocessed_from></meta>
</output>"#;
        let mut aliases = crate::history_summarizer_citations::FrozenAliasTable::default();
        aliases.issue(crate::history_summarizer_citations::FrozenAlias {
            message_id: "m2".into(),
            ordinal: 2,
            block_ids: vec!["m2#0".into()],
            block_hashes: vec!["0".repeat(64)],
            presented: "second arc: publish facts through the same flow".into(),
            ..Default::default()
        });
        let chunk = HistorySummarizerChunk {
            aliases,
            start_index: 2,
            end_index: 4,
            lines: vec![
                ChunkLine {
                    ordinal: 2,
                    message_id: "m2#0".into(),
                    anchorable: true,
                },
                ChunkLine {
                    ordinal: 3,
                    message_id: "m3#0".into(),
                    anchorable: true,
                },
                ChunkLine {
                    ordinal: 4,
                    message_id: "m4#0".into(),
                    anchorable: true,
                },
            ],
            present_ordinals: vec![1, 2, 3, 4],
            tool_only_ranges: vec![],
            completed_tool_arcs: vec![],
        };
        let prior = [
            crate::history_summarizer_validate::StoredHistorySegmentRange {
                start_message: 1,
                end_message: 1,
            },
        ];
        let validated = validate_history_summarizer_output(
            text,
            &chunk,
            &prior,
            ValidateOptions {
                sequence_offset: 1,
                in_emergency: true, // skip discard-last so the single history_segment persists
                memory_enabled: true,
                auto_promote: true,
                user_memory_collection_enabled: true,
                force_keep_last_history_segment: false,
            },
        )
        .expect("validation succeeds");
        assert_eq!(validated.history_segments.len(), 1);
        assert_eq!(validated.history_segments[0].end_message_id, "m3#0");
        assert_eq!(
            validated.extraction,
            crate::history_summarizer_citations::ExtractionOutcome::Accepted { count: 1 }
        );
        assert_eq!(
            validated.facts[0].content,
            "Publish facts in the same flow."
        );
        // Q30: a bad citation rejects the fact set but leaves the same publishable history; publication below consumes only the history, so both variants publish identically.
        let rejected = validate_history_summarizer_output(
            &text.replace("[s1:0-11]", "[s1:0-999]"),
            &chunk,
            &prior,
            ValidateOptions {
                sequence_offset: 1,
                in_emergency: true,
                memory_enabled: true,
                auto_promote: true,
                user_memory_collection_enabled: true,
                force_keep_last_history_segment: false,
            },
        )
        .expect("history still validates");
        assert_eq!(rejected.history_segments, validated.history_segments);
        assert_eq!(
            rejected.extraction,
            crate::history_summarizer_citations::ExtractionOutcome::Rejected {
                failure: crate::history_summarizer_citations::ExtractionFailure::InvalidSpan
            }
        );
        assert!(rejected.facts.is_empty());

        let mut meta = store.load("ses").unwrap().meta;
        for selected in test_selected_range_identities() {
            meta.block_identity_by_mid
                .insert(selected.mid, selected.block_identities);
        }
        meta.history_summarizer = HistorySummarizerDurableState {
            state: HistorySummarizerPhase::Publishing,
            firing_seq: 1,
            chunk_range: Some(HistorySummarizerChunkRange {
                from_ordinal: 2,
                to_ordinal: 4,
            }),
            chunk_fingerprint: "fp".into(),
            selected_range_identities: test_selected_range_identities(),
            producer_session_id: Some("ps".into()),
            producer_run_id: Some("run-1".into()),
            producer_harness: None,
            fired_at_ms: Some(1),
            expected_revert_epoch: 0,
            history_segment_set_generation: HistorySegmentSetGeneration {
                max_sequence: 1,
                count: 1,
            },
            failure_backoff_at_ms: None,
            last_failure: None,
            last_no_fire: None,
            consecutive_publish_failures: 0,
            curator_nonadmission: Default::default(),
            curator_reservation: None,
        };
        let rv = store
            .commit(
                "ses",
                store.load("ses").unwrap().row_version,
                &store.load("ses").unwrap().core,
                &meta,
            )
            .unwrap();
        let predicate = publish_predicate(&meta.history_summarizer).unwrap();

        publish_validated_chunk(
            &store,
            ValidatedPublishRequest {
                session_id: "ses",
                project_path: "git:proj",
                expected_row_version: Some(rv),
                expected_revert_epoch: 0,
                predicate: &predicate,
                observed_chunk_fingerprint: "fp",
                validated: &validated,
                collect_user_memory_candidates: true,
                publication_floor_ordinal: 4,
                chunk_transcript: "U: transcript",

                boundary_dates: empty_boundary_dates(),
                created_at_ms: 123,
                failure_backoff_at_ms: 0,
                publication_fence: None,
                curator_nonadmission: None,
                curator_activation: None,
            },
        )
        .expect("publish succeeds");

        let after = store.load("ses").unwrap();
        assert_eq!(
            after.meta.history_summarizer.state,
            HistorySummarizerPhase::Idle
        );
        assert_eq!(after.meta.publication_floor_ordinal, Some(4));
        let comps = store.load_history_segments("ses").unwrap();
        assert_eq!(comps.len(), 2, "C1 preserved, C2 appended");
        assert_eq!(store.load_history_segment_events("ses").unwrap().len(), 1);
        assert_eq!(store.load_primer_candidates("ses").unwrap().len(), 1);
        assert_eq!(store.load_user_memory_candidates("ses").unwrap().len(), 1);
        let c2 = comps.last().unwrap();
        assert_eq!(c2.end_message_id, "m3#0");
        assert_eq!(c2.p1.as_deref(), Some("second arc full and exact"));
        assert_eq!(c2.legacy, 0);
        assert_eq!(c2.created_at, 123);
    }

    /// Q31 before reservation: a rejected fact set records one nonadmission with the firing's identity in the same publication that advances history; a later no-fact success, an extraction-free run, a restart with the producer in flight, a validation failure, and a store reopen all leave the count and latest reason in place; an accepted set with no Curator handoff is recorded as an unavailable Curator.
    #[test]
    fn nonadmission_facts_survive_later_firings_failures_and_reopen() {
        use crate::history_summarizer_citations::{FrozenAlias, FrozenAliasTable};
        use crate::history_summarizer_validate::{
            ChunkLine, HistorySummarizerChunk, StoredHistorySegmentRange, ValidateOptions,
        };
        use memory_store::{CuratorNonadmission, ExtractionFailure, RecordedNonadmission};

        fn chunk(start: u64, end: u64) -> HistorySummarizerChunk {
            let mut aliases = FrozenAliasTable::default();
            for ordinal in start..=end {
                aliases.issue(FrozenAlias {
                    message_id: format!("m{ordinal}"),
                    ordinal,
                    block_ids: vec![format!("m{ordinal}#0")],
                    block_hashes: vec!["0".repeat(64)],
                    presented: "presented text".into(),
                    ..Default::default()
                });
            }
            HistorySummarizerChunk {
                aliases,
                start_index: start,
                end_index: end,
                lines: (start..=end)
                    .map(|ordinal| ChunkLine {
                        ordinal,
                        message_id: format!("m{ordinal}#0"),
                        anchorable: true,
                    })
                    .collect(),
                present_ordinals: (start..=end).collect(),
                tool_only_ranges: vec![],
                completed_tool_arcs: vec![],
            }
        }
        /// One segment `start..=end` plus `facts`, leaving `end + 1` unprocessed.
        fn output(start: u64, end: u64, facts: &str) -> ProducerOutput {
            let unprocessed_from = end + 1;
            ProducerOutput {
                text: format!(
                    r#"<output><history_segments><history_segment start="{start}" end="{end}" title="arc" episode_type="feature" importance="60"><p1>arc</p1><p2>arc</p2><p3>arc</p3><p4 /></history_segment></history_segments>{facts}<meta><unprocessed_from>{unprocessed_from}</unprocessed_from></meta></output>"#
                ),
                length_capped: false,
            }
        }
        /// The published ranges so far, as the validator's prior-coverage input.
        fn prior(store: &MemoryStore) -> Vec<StoredHistorySegmentRange> {
            store
                .load_history_segments("ses")
                .unwrap()
                .iter()
                .map(|segment| StoredHistorySegmentRange {
                    start_message: segment.start_message as u64,
                    end_message: segment.end_message as u64,
                })
                .collect()
        }
        let options = ValidateOptions {
            in_emergency: true,
            ..ValidateOptions::default()
        };
        const CITED: &str =
            "<facts><PROJECT_RULES>\n* [s1:0-9] presented\n</PROJECT_RULES></facts>";

        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        store
            .replace_history_segments("ses", &[comp(1, 1, 1, "m1", "C1 summary")])
            .unwrap();
        seed_awaiting_history_summarizer(&store);
        let nonadmission = || {
            store
                .load("ses")
                .unwrap()
                .meta
                .history_summarizer
                .curator_nonadmission
        };
        let floor = || store.load("ses").unwrap().meta.publication_floor_ordinal;
        // Fires `from..=to` from the idle state and marks the producer started, as the live path does.
        let fire_next = |from: u64, to: u64| {
            let loaded = store.load("ses").unwrap();
            let segments = store.load_history_segments("ses").unwrap();
            let generation = HistorySegmentSetGeneration {
                max_sequence: segments.iter().map(|c| c.sequence).max().unwrap_or(0),
                count: segments.len() as i64,
            };
            let fired = match fire(
                &loaded.meta.history_summarizer,
                from,
                to,
                "fp".into(),
                test_selected_range_identities(),
                0,
                generation,
                20,
            )
            .unwrap()
            {
                FireOutcome::Fired(state) => state,
                FireOutcome::Busy(_) => unreachable!("idle after publication"),
            };
            let awaiting = producer_started(
                &fired,
                "producer-session".into(),
                format!("run-{}", fired.firing_seq),
                "pi".into(),
            )
            .unwrap();
            let mut meta = loaded.meta.clone();
            meta.history_summarizer = awaiting.clone();
            store
                .commit("ses", loaded.row_version, &loaded.core, &meta)
                .unwrap();
            awaiting
        };
        let publish_with = |awaiting: HistorySummarizerDurableState,
                            output: ProducerOutput,
                            validation_chunk: &HistorySummarizerChunk,
                            validate_options: ValidateOptions| {
            publish_output_from_awaiting(PublishOutputRequest {
                store: &store,
                session_id: "ses",
                project_path: "git:proj",
                awaiting,
                output,
                observed_chunk_fingerprint: "fp",
                validation_chunk,
                chunk_transcript: "U: transcript",
                boundary_dates: empty_boundary_dates(),
                prior_history_segments: &prior(&store),
                validate_options,
                created_at_ms: 10,
                failure_started_at_ms: 10,
                failure_backoff_at_ms: 0,
                completion_now_ms: || 11,
                publication_fence: None,
                curator_handoff: None,
            })
        };
        let publish = |awaiting: HistorySummarizerDurableState,
                       output: ProducerOutput,
                       validation_chunk: &HistorySummarizerChunk| {
            publish_with(awaiting, output, validation_chunk, options)
        };

        // Firing 1 (2..=4): the fact cites an alias the chunk never issued; history publishes with the rejection recorded.
        let awaiting = store.load("ses").unwrap().meta.history_summarizer;
        publish(
            awaiting,
            output(
                2,
                3,
                "<facts><PROJECT_RULES>\n* [s9:0-4] unknown alias\n</PROJECT_RULES></facts>",
            ),
            &chunk(2, 4),
        )
        .expect("history publishes beside the rejected set");
        assert_eq!(floor(), Some(4));
        assert_eq!(store.load_history_segments("ses").unwrap().len(), 2);
        let recorded = CuratorNonadmission {
            count: 1,
            latest: Some(RecordedNonadmission {
                firing_seq: 1,
                code: CuratorNonadmissionCode::FactSetRejected {
                    failure: ExtractionFailure::UnknownAlias,
                },
            }),
        };
        assert_eq!(nonadmission(), recorded);

        // Firing 2 (4..=6): an intentional no-fact output is not a nonadmission.
        let awaiting = fire_next(4, 6);
        assert_eq!(awaiting.curator_nonadmission, recorded);
        publish(
            awaiting,
            output(4, 5, "<facts><PROJECT_RULES>\n</PROJECT_RULES></facts>"),
            &chunk(4, 6),
        )
        .expect("no-fact output publishes");
        assert_eq!(floor(), Some(6));
        assert_eq!(nonadmission(), recorded);

        // Firing 3 (6..=8): a restart finds the producer in flight, abandons the firing, and keeps the facts.
        fire_next(6, 8);
        let mut loaded = store.load("ses").unwrap();
        loaded.meta.history_summarizer.state = HistorySummarizerPhase::Publishing;
        store
            .commit("ses", loaded.row_version, &loaded.core, &loaded.meta)
            .unwrap();
        assert!(matches!(
            handle_restart_load(&store, "ses", 30).unwrap(),
            RestartAction::AbandonedAndRefireEligible { firing_seq: 3 }
        ));
        let after_restart = store.load("ses").unwrap().meta.history_summarizer;
        assert_eq!(after_restart.state, HistorySummarizerPhase::Idle);
        assert_eq!(after_restart.curator_nonadmission, recorded);

        // Firing 4 (6..=8): an extraction-free run (memory disabled) is not a nonadmission, whatever the model emitted.
        let awaiting = fire_next(6, 8);
        publish_with(
            awaiting,
            output(6, 7, CITED),
            &chunk(6, 8),
            ValidateOptions {
                memory_enabled: false,
                ..options
            },
        )
        .expect("extraction-free output publishes");
        assert_eq!(floor(), Some(8));
        assert_eq!(nonadmission(), recorded);

        // Firing 5 (8..=10): the envelope is invalid, so publication is rejected and the run abandoned; the facts survive the abandonment.
        let awaiting = fire_next(8, 10);
        let failure = publish(
            awaiting,
            ProducerOutput {
                text: "<output>not a history</output>".into(),
                length_capped: false,
            },
            &chunk(8, 10),
        );
        assert!(matches!(
            failure,
            Err(HistorySummarizerDriveError::Validation(_))
        ));
        let after_failure = store.load("ses").unwrap().meta.history_summarizer;
        assert_eq!(after_failure.state, HistorySummarizerPhase::Idle);
        assert!(after_failure.last_failure.is_some());
        assert_eq!(floor(), Some(8));
        assert_eq!(nonadmission(), recorded);

        // Firing 6 (8..=10): an accepted set has no Curator handoff yet, so it is recorded against this firing as an unavailable Curator.
        let awaiting = fire_next(8, 10);
        let firing_seq = awaiting.firing_seq;
        publish(awaiting, output(8, 9, CITED), &chunk(8, 10))
            .expect("accepted set publishes its history");
        drop(store);

        // Reopening the store returns the same producer-owned facts.
        let reopened = self::store(dir.path());
        let loaded = reopened.load("ses").unwrap();
        assert_eq!(
            loaded.meta.history_summarizer.curator_nonadmission,
            CuratorNonadmission {
                count: 2,
                latest: Some(RecordedNonadmission {
                    firing_seq,
                    code: CuratorNonadmissionCode::CuratorUnavailable,
                }),
            }
        );
        assert_eq!(loaded.meta.publication_floor_ordinal, Some(10));
    }

    #[test]
    fn chunk_fingerprint_uses_id_kind_and_byte_length() {
        let fingerprint = compute_chunk_fingerprint(&[
            ChunkSnapshotItem {
                id: "m1",
                kind: "user",
                byte_len: 3,
            },
            ChunkSnapshotItem {
                id: "m2",
                kind: "assistant",
                byte_len: 2,
            },
        ]);
        assert_eq!(fingerprint, "m1:user:3|m2:assistant:2");
        for (bytes, byte_len) in [("", 0), ("🙂", 4), ("e\u{301}", 3), ("中文", 6)] {
            assert_eq!(bytes.len(), byte_len);
            assert_eq!(
                compute_chunk_fingerprint(&[ChunkSnapshotItem {
                    id: "m:|",
                    kind: "text",
                    byte_len: bytes.len(),
                }]),
                format!("m:|:text:{byte_len}")
            );
        }
        assert_eq!(compute_chunk_fingerprint(&[]), "");
    }

    #[test]
    fn pure_state_machine_happy_path_and_single_flight() {
        let idle = HistorySummarizerDurableState::default();
        let fired = match fire(
            &idle,
            2,
            5,
            "fp".into(),
            Vec::new(),
            0,
            HistorySegmentSetGeneration::default(),
            100,
        )
        .unwrap()
        {
            FireOutcome::Fired(state) => state,
            FireOutcome::Busy(_) => panic!("idle state must fire"),
        };
        assert_eq!(fired.state, HistorySummarizerPhase::Firing);
        assert_eq!(fired.firing_seq, 1);
        assert!(matches!(
            fire(
                &fired,
                6,
                7,
                "other".into(),
                Vec::new(),
                0,
                HistorySegmentSetGeneration::default(),
                101,
            )
            .unwrap(),
            FireOutcome::Busy(_)
        ));

        let awaiting = producer_started(&fired, "ps".into(), "run".into(), "pi".into()).unwrap();
        let validating = output_received(&awaiting, "text").unwrap();
        let publishing = validation_ok(&validating).unwrap();
        let idle_again = tx_committed(&publishing).unwrap();
        assert_eq!(idle_again.state, HistorySummarizerPhase::Idle);
        assert_eq!(idle_again.firing_seq, 1);
    }

    #[test]
    fn producer_establish_clears_failure_detail_and_backoff() {
        let idle = HistorySummarizerDurableState {
            failure_backoff_at_ms: Some(999),
            last_failure: Some("stale failure".into()),
            ..HistorySummarizerDurableState::default()
        };
        let fired = match fire(
            &idle,
            2,
            5,
            "fp".into(),
            Vec::new(),
            0,
            HistorySegmentSetGeneration::default(),
            100,
        )
        .unwrap()
        {
            FireOutcome::Fired(state) => state,
            FireOutcome::Busy(_) => panic!("idle state must fire"),
        };
        assert_eq!(fired.failure_backoff_at_ms, Some(999));
        let awaiting = producer_started(&fired, "ps".into(), "run".into(), "pi".into()).unwrap();
        assert_eq!(awaiting.failure_backoff_at_ms, None);
        assert_eq!(awaiting.last_failure, None);
    }

    #[test]
    fn fingerprint_mismatch_at_publish_abandons_and_releases_single_flight() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        let meta = test_meta_with_history_summarizer(publishing_state());
        store
            .commit("ses", None, &CoreState::empty(), &meta)
            .unwrap();
        let loaded = store.load("ses").unwrap();
        let predicate = publish_predicate(&loaded.meta.history_summarizer).unwrap();
        let abandon_hook_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let abandon_hook_calls_for_hook = std::sync::Arc::clone(&abandon_hook_calls);
        store.set_abandon_history_summarizer_hook(Box::new(move || {
            abandon_hook_calls_for_hook.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }));
        let err = publish_validated_chunk(
            &store,
            ValidatedPublishRequest {
                session_id: "ses",
                project_path: "git:proj",
                expected_row_version: loaded.row_version,
                expected_revert_epoch: 0,
                predicate: &predicate,
                observed_chunk_fingerprint: "different-fingerprint",
                validated: &ValidatedChunk::default(),
                collect_user_memory_candidates: false,
                publication_floor_ordinal: 5,
                chunk_transcript: "U: transcript",

                boundary_dates: empty_boundary_dates(),
                created_at_ms: 0,
                failure_backoff_at_ms: 999,
                publication_fence: None,
                curator_nonadmission: None,
                curator_activation: None,
            },
        )
        .unwrap_err();
        assert!(matches!(
            err,
            HistorySummarizerStateError::FingerprintMismatch { .. }
        ));

        let after = store.load("ses").unwrap().meta.history_summarizer;
        assert_eq!(after.state, HistorySummarizerPhase::Idle);
        assert_eq!(after.failure_backoff_at_ms, Some(999));
        assert_eq!(
            abandon_hook_calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "fingerprint cleanup must use the store's fenced abandon primitive"
        );
        assert!(matches!(
            fire(
                &after,
                6,
                7,
                "new".into(),
                Vec::new(),
                0,
                HistorySegmentSetGeneration::default(),
                1000,
            )
            .unwrap(),
            FireOutcome::Fired(_)
        ));
    }

    #[test]
    fn fire_persists_expected_revert_epoch_for_reattach() {
        let fired = match fire(
            &HistorySummarizerDurableState::default(),
            2,
            5,
            "fp".into(),
            Vec::new(),
            42,
            HistorySegmentSetGeneration::default(),
            100,
        )
        .unwrap()
        {
            FireOutcome::Fired(state) => state,
            FireOutcome::Busy(_) => panic!("idle state must fire"),
        };
        assert_eq!(fired.expected_revert_epoch, 42);
        let awaiting = producer_started(&fired, "ps".into(), "run".into(), "pi".into()).unwrap();
        assert_eq!(awaiting.expected_revert_epoch, 42);
    }

    #[test]
    fn epoch_mismatch_publish_abandons_to_idle_with_detail() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        let meta = ModuleMeta {
            revert_epoch: 1,
            ..test_meta_with_history_summarizer(publishing_state())
        };
        store
            .commit("ses", None, &CoreState::empty(), &meta)
            .unwrap();
        let loaded = store.load("ses").unwrap();
        let predicate = publish_predicate(&loaded.meta.history_summarizer).unwrap();

        let err = publish_validated_chunk(
            &store,
            ValidatedPublishRequest {
                session_id: "ses",
                project_path: "git:proj",
                expected_row_version: loaded.row_version,
                expected_revert_epoch: 0,
                predicate: &predicate,
                observed_chunk_fingerprint: "fp",
                validated: &ValidatedChunk::default(),
                collect_user_memory_candidates: false,
                publication_floor_ordinal: 5,
                chunk_transcript: "U: transcript",

                boundary_dates: empty_boundary_dates(),
                created_at_ms: 0,
                failure_backoff_at_ms: 999,
                publication_fence: None,
                curator_nonadmission: None,
                curator_activation: None,
            },
        )
        .unwrap_err();
        assert!(matches!(
            err,
            HistorySummarizerStateError::Publish(HistorySummarizerPublishError::CasConflict { .. })
        ));

        let after = store.load("ses").unwrap().meta.history_summarizer;
        assert_eq!(after.state, HistorySummarizerPhase::Idle);
        assert_eq!(after.failure_backoff_at_ms, Some(999));
        assert_eq!(
            after.last_failure.as_deref(),
            Some("publish rejected: revert epoch mismatch (session was re-cut mid-firing)")
        );
        assert!(store.load_history_segments("ses").unwrap().is_empty());
    }

    #[test]
    fn history_segment_generation_fence_releases_overlapped_publish_to_idle() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        store
            .commit(
                "ses",
                None,
                &CoreState::empty(),
                &test_meta_with_history_summarizer(publishing_state()),
            )
            .unwrap();
        let predicate =
            publish_predicate(&store.load("ses").unwrap().meta.history_summarizer).unwrap();

        store
            .append_history_segments("ses", &[comp(1, 2, 4, "m4#0", "seeded summary")])
            .unwrap();
        let synced = store.load("ses").unwrap();
        store
            .commit("ses", synced.row_version, &synced.core, &synced.meta)
            .unwrap();
        let fresh = store.load("ses").unwrap();

        let validated = ValidatedChunk {
            history_segments: vec![
                crate::history_summarizer_validate::ValidatedHistorySegment {
                    sequence: 1,
                    start_message: 2,
                    end_message: 4,
                    start_message_id: "m2#0".to_string(),
                    end_message_id: "m4#0".to_string(),
                    title: "stale summary".to_string(),
                    content: "stale summary".to_string(),
                    p1: Some("stale summary".to_string()),
                    p2: None,
                    p3: None,
                    p4: None,
                    importance: Some(50),
                    episode_type: None,
                },
            ],
            unprocessed_from: 5,
            ..Default::default()
        };
        let error = publish_validated_chunk(
            &store,
            ValidatedPublishRequest {
                session_id: "ses",
                project_path: "git:proj",
                expected_row_version: fresh.row_version,
                expected_revert_epoch: 0,
                predicate: &predicate,
                observed_chunk_fingerprint: "fp",
                validated: &validated,
                collect_user_memory_candidates: false,
                publication_floor_ordinal: 5,
                chunk_transcript: "U: transcript",

                boundary_dates: empty_boundary_dates(),
                created_at_ms: 0,
                failure_backoff_at_ms: 999,
                publication_fence: None,
                curator_nonadmission: None,
                curator_activation: None,
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            HistorySummarizerStateError::Publish(
                HistorySummarizerPublishError::FenceRejected { .. }
            )
        ));
        let after = store.load("ses").unwrap();
        assert_eq!(
            after.meta.history_summarizer.state,
            HistorySummarizerPhase::Idle
        );
        assert_eq!(after.meta.history_summarizer.failure_backoff_at_ms, None);
        let history_segments = store.load_history_segments("ses").unwrap();
        assert_eq!(
            history_segments.len(),
            1,
            "the stale publish must not append a second overlapping range"
        );
        assert!(crate::history_segment_coverage::resolve_coverage(&history_segments).is_ok());
    }

    #[tokio::test]
    async fn reattach_carries_durable_revert_epoch_to_publish() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        seed_prior_history_segment(&store);
        let chunk = history_summarizer_chunk();
        let prior = prior_ranges();
        let fired = match fire(
            &HistorySummarizerDurableState::default(),
            2,
            4,
            "fp".into(),
            test_selected_range_identities(),
            7,
            HistorySegmentSetGeneration::default(),
            1,
        )
        .unwrap()
        {
            FireOutcome::Fired(state) => state,
            FireOutcome::Busy(_) => unreachable!(),
        };
        let awaiting = producer_started(
            &fired,
            "producer-session".into(),
            "run-1".into(),
            "pi".into(),
        )
        .unwrap();
        store
            .commit(
                "ses",
                None,
                &CoreState::empty(),
                &ModuleMeta {
                    revert_epoch: 8,
                    ..test_meta_with_history_summarizer(awaiting)
                },
            )
            .unwrap();
        let mut producer = ScriptedProducer::default()
            .with_status(Ok(RunState::Terminal))
            .with_output(Ok(producer_output(history_summarizer_xml(
                "stale epoch summary",
            ))));

        let err = reattach_history_summarizer_producer(
            &mut producer,
            reattach_request(&store, &chunk, &prior),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            err,
            HistorySummarizerDriveError::State(HistorySummarizerStateError::Publish(
                HistorySummarizerPublishError::CasConflict { .. }
            ))
        ));
        let after = store.load("ses").unwrap().meta.history_summarizer;
        assert_eq!(after.state, HistorySummarizerPhase::Idle);
        assert_eq!(
            after.last_failure.as_deref(),
            Some("publish rejected: revert epoch mismatch (session was re-cut mid-firing)")
        );
        assert_eq!(store.load_history_segments("ses").unwrap().len(), 1);
    }

    #[test]
    fn restart_mid_awaiting_exposes_reattach_ids() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        let awaiting = producer_started(
            &match fire(
                &HistorySummarizerDurableState::default(),
                1,
                3,
                "fp".into(),
                test_selected_range_identities(),
                0,
                HistorySegmentSetGeneration::default(),
                10,
            )
            .unwrap()
            {
                FireOutcome::Fired(state) => state,
                FireOutcome::Busy(_) => unreachable!(),
            },
            "producer-session".into(),
            "run-1".into(),
            "pi".into(),
        )
        .unwrap();
        let meta = test_meta_with_history_summarizer(awaiting);
        store
            .commit("ses", None, &CoreState::empty(), &meta)
            .unwrap();

        let action = handle_restart_load(&store, "ses", 500).unwrap();
        assert_eq!(
            action,
            RestartAction::ReattachProducer {
                producer_session_id: "producer-session".into(),
                producer_run_id: "run-1".into(),
                producer_harness: Some("pi".into()),
                firing_seq: 1,
                chunk_fingerprint: "fp".into(),
            }
        );
        assert_eq!(
            store.load("ses").unwrap().meta.history_summarizer.state,
            HistorySummarizerPhase::AwaitingProducer,
            "reattach does not clear the durable single-flight"
        );
    }

    #[test]
    fn restart_mid_publishing_with_committed_tx_detects_idle() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        let meta = test_meta_with_history_summarizer(publishing_state());
        store
            .commit("ses", None, &CoreState::empty(), &meta)
            .unwrap();
        let loaded = store.load("ses").unwrap();
        let predicate = publish_predicate(&loaded.meta.history_summarizer).unwrap();
        store
            .publish_history_summarizer_chunk(HistorySummarizerPublishRequest {
                session_id: "ses",
                expected_row_version: loaded.row_version,
                expected_revert_epoch: 0,
                predicate: &predicate,
                project_path: "git:proj",
                history_segments: &[comp(1, 2, 4, "m4", "summary")],
                events: &[],
                primer_candidates: &[],
                user_memory_candidates: &[],
                publication_floor_ordinal: 5,
                chunk_transcript: Some("U: transcript"),
                curator_nonadmission: None,
                curator_activation: None,
            })
            .unwrap();

        assert_eq!(
            handle_restart_load(&store, "ses", 500).unwrap(),
            RestartAction::Done
        );
        assert_eq!(
            store.load("ses").unwrap().meta.history_summarizer.state,
            HistorySummarizerPhase::Idle
        );
    }
}

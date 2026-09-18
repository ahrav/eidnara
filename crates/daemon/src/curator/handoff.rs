//! The History Summarizer's reserve, stage, activate handoff of accepted facts to Curator review (KTD3).
//!
//! The order is fixed by the two-store contract. First the producer reserves the review's causal identity in the Memory Store: the target is the staged subject the facts will become, named by the Kernel incarnation, a candidate id derived from the subject bytes, and the digest of those bytes, all computable before anything is staged, so the reservation carries the input fingerprint, the queue deadline, and the worst-case allowance from the start. The reservation is then written into the firing's durable state, so a crash before publication leaves a capped reservation the firing reconciles against and never a second one. Only then is the subject staged and sealed in the Kernel under the reservation's identity and deadline. Activation to `Ready` is left to the fenced publication, which commits it together with the history or not at all.
//!
//! The facts cite frozen aliases (#594); the subject carries each fact's spans and one origin per cited alias: the native message, the blocks whose text produced the presented part, their content hashes, and the distinct cited ranges. Two facts that cite one block share one origin, and no block's bytes are stored again; the originals resolve from the chunk's frozen identity.

use std::collections::BTreeMap;
use std::sync::Arc;

use kernel::{
    ExtractedFact, KernelStore, ReviewBinding, ReviewOwner, ReviewPayload, ReviewStageError,
    ReviewStagedReference, ReviewStagingSpec, ReviewSubject, SourceDependency, SourceSpan,
    StagingTerminalState, SubjectOrigin,
};
use memory_store::curator_jobs::{
    CausalInputs, CuratorJob, CuratorJobError, CuratorJobInput, CuratorJobRefusal, CuratorJobState,
    ProducerBinding, ReserveOutcome, ReviewTarget,
};
use memory_store::{
    CuratorNonadmissionCode, CuratorReservation, ExtractionFailure, HistorySummarizerDurableState,
    MemoryStore,
};

use crate::history_summarizer::HistorySummarizerStateError;
use crate::history_summarizer_citations::FrozenAliasTable;
use crate::history_summarizer_validate::FactCandidate;

use super::broker::QuestionTemplate;
use super::steps::STEP_VERSION;

/// The producer name every History Summarizer reservation carries.
pub const PRODUCER: &str = "history_summarizer";
/// The source lineage a staged subject cites: the session's chunk at the firing that presented it.
pub const SUBJECT_SOURCE_KIND: &str = "history_summarizer_chunk";

/// The Kernel and scope a firing hands accepted facts to; the Memory Store is the one the publication itself commits under.
#[derive(Clone)]
pub struct HandoffTarget {
    pub kernel: Arc<KernelStore>,
    /// Lower-hex SHA-256 of the project identity the Kernel scopes the subject under.
    pub project_digest: String,
    pub domain_id: String,
    pub kernel_incarnation: String,
}

/// What the firing carries into publication after the handoff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handoff {
    /// The reservation is recorded, staged, and sealed; publication activates it.
    Activate(Box<PreparedActivation>),
    /// No reservation was made; publication records this reason with its progress (Q31).
    Nonadmission(CuratorNonadmissionCode),
    /// Identical causal inputs already have a job this firing cannot activate; nothing is reopened and nothing is recorded.
    Settled,
}

/// The reserved job and the reference-only input that activates it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedActivation {
    pub causal_identity: String,
    pub producer: ProducerBinding,
    pub input: CuratorJobInput,
    /// The session row version after the reservation was written; the publication's CAS expects it.
    pub row_version: u64,
}

/// A failure after the reservation exists. The reservation stays in the durable state; publication does not proceed.
#[derive(Debug, thiserror::Error)]
pub enum HandoffError {
    #[error("curator reservation: {0}")]
    Reserve(#[from] CuratorJobError),
    #[error("recording the reservation: {0}")]
    Persist(#[from] HistorySummarizerStateError),
    #[error("kernel staging: {0}")]
    Stage(#[from] ReviewStageError),
    #[error("kernel seal: {0}")]
    Seal(kernel::KernelError),
    #[error("the staged subject does not read back: {0}")]
    Read(kernel::ReviewReadError),
    #[error("an accepted fact cites an alias the chunk did not issue")]
    UnknownAlias,
    #[error("the firing has no chunk range")]
    NoChunkRange,
}

/// The review policies a job depends on: a change to either permits one new job at an unchanged target (Q25/Q29).
pub fn review_policy_versions() -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "question_template".to_string(),
            QuestionTemplate::ExtractedFacts.revision(),
        ),
        ("step_schema".to_string(), STEP_VERSION.to_string()),
    ])
}

/// The binding a staged History Summarizer subject is read under. `chunk_ordinal` is the first message of the chunk that presented the facts, the job's `producer.ordinal`, so the coordinator reconstructs the binding from the job row alone and a firing that adopts the reservation reads under the same binding.
pub fn review_binding(
    project_digest: &str,
    domain_id: &str,
    session_id: &str,
    chunk_ordinal: u64,
    job_id: &str,
) -> ReviewBinding {
    ReviewBinding {
        project_digest: project_digest.to_string(),
        domain_id: domain_id.to_string(),
        owner: ReviewOwner::Job {
            job_id: job_id.to_string(),
        },
        subject_source: SourceDependency {
            source_kind: SUBJECT_SOURCE_KIND.to_string(),
            source_id: session_id.to_string(),
            source_revision: i64::try_from(chunk_ordinal).unwrap_or(i64::MAX),
        },
        reference_sources: Vec::new(),
    }
}

/// The subject the accepted facts become: every fact with its cited spans, sorted and deduplicated so equal fact sets encode to equal bytes, and one origin per cited alias.
pub fn review_subject(
    facts: &[FactCandidate],
    aliases: &FrozenAliasTable,
) -> Result<ReviewSubject, ExtractionFailure> {
    let mut origins: BTreeMap<&str, SubjectOrigin> = BTreeMap::new();
    let mut extracted = Vec::with_capacity(facts.len());
    for fact in facts {
        let mut spans = Vec::with_capacity(fact.citations.len());
        for citation in &fact.citations {
            let frozen = aliases
                .resolve(&citation.alias)
                .ok_or(ExtractionFailure::UnknownAlias)?;
            let span = SourceSpan {
                alias: frozen.alias.clone(),
                start: citation.start as u64,
                end: citation.end as u64,
            };
            origins
                .entry(frozen.alias.as_str())
                .or_insert_with(|| SubjectOrigin {
                    alias: frozen.alias.clone(),
                    message_id: frozen.message_id.clone(),
                    ordinal: frozen.ordinal,
                    block_ids: frozen.block_ids.clone(),
                    block_hashes: frozen.block_hashes.clone(),
                    ranges: Vec::new(),
                })
                .ranges
                .push(span.range());
            spans.push(span);
        }
        spans.sort();
        spans.dedup();
        extracted.push(ExtractedFact {
            text: fact.content.clone(),
            spans,
        });
    }
    let origins = origins
        .into_values()
        .map(|mut origin| {
            origin.ranges.sort();
            origin.ranges.dedup();
            origin
        })
        .collect();
    Ok(ReviewSubject {
        facts: extracted,
        origins,
    })
}

/// One firing's accepted facts and the store they publish under.
pub struct HandoffRequest<'a> {
    pub store: &'a MemoryStore,
    /// The Memory Store project the job row belongs to: the one the publication commits under.
    pub project: &'a str,
    pub session_id: &'a str,
    pub firing: &'a HistorySummarizerDurableState,
    pub facts: &'a [FactCandidate],
    pub aliases: &'a FrozenAliasTable,
    pub now_ms: i64,
}

/// The row the firing works against: its own recorded reservation, a fresh reservation, or a decision that ends the handoff without one.
enum ReservedRow {
    Job(Box<CuratorJob>),
    Done(Handoff),
}

fn reserved_row(
    request: &HandoffRequest<'_>,
    producer: &ProducerBinding,
    inputs: &CausalInputs,
    payload_digest: &str,
    kernel_incarnation: &str,
) -> Result<ReservedRow, HandoffError> {
    // A recorded reservation names the job only when its payload digest and kernel incarnation match this firing's; `adopt` decides whether the firing can activate it.
    let held = request
        .firing
        .curator_reservation
        .as_ref()
        .filter(|held| {
            held.payload_digest == payload_digest && held.kernel_incarnation == kernel_incarnation
        })
        .map(|held| {
            request
                .store
                .lookup_curator_job(request.project, &held.causal_identity)
                .map_err(CuratorJobError::Store)
        })
        .transpose()?
        .flatten();
    let existing = match held {
        Some(job) => job,
        None => match request.store.reserve_curator_job(
            request.project,
            producer,
            inputs,
            request.now_ms,
        ) {
            Ok(ReserveOutcome::Reserved(job)) => return Ok(ReservedRow::Job(Box::new(job))),
            Ok(ReserveOutcome::Existing(job)) => job,
            Err(CuratorJobError::Refused(
                CuratorJobRefusal::ProjectCapacity
                | CuratorJobRefusal::HostCapacity
                | CuratorJobRefusal::MetadataQuota,
            )) => {
                return Ok(ReservedRow::Done(Handoff::Nonadmission(
                    CuratorNonadmissionCode::CapacityFull,
                )));
            }
            Err(CuratorJobError::Store(_)) => {
                return Ok(ReservedRow::Done(Handoff::Nonadmission(
                    CuratorNonadmissionCode::CuratorUnavailable,
                )));
            }
            Err(error) => return Err(error.into()),
        },
    };
    adopt(request, producer, existing)
}

fn adopt(
    request: &HandoffRequest<'_>,
    producer: &ProducerBinding,
    job: CuratorJob,
) -> Result<ReservedRow, HandoffError> {
    match job.state {
        CuratorJobState::Reserved if job.producer == *producer => {
            Ok(ReservedRow::Job(Box::new(job)))
        }
        // Firing ids are unique per firing, so a `Reserved` row another firing of this producer left can be activated by no one until it is rebound.
        CuratorJobState::Reserved if job.producer.producer == producer.producer => {
            match request.store.rebind_reserved_curator_job(
                request.project,
                &job.causal_identity,
                producer,
                request.now_ms,
            ) {
                Ok(job) => Ok(ReservedRow::Job(Box::new(job))),
                Err(CuratorJobError::Refused(
                    CuratorJobRefusal::Expired
                    | CuratorJobRefusal::NotReserved
                    | CuratorJobRefusal::Terminal,
                )) => Ok(ReservedRow::Done(Handoff::Settled)),
                Err(error) => Err(error.into()),
            }
        }
        _ => Ok(ReservedRow::Done(Handoff::Settled)),
    }
}

/// Reserves the review, records the reservation through `persist`, then stages and seals the subject and reads it back. Capacity and quota refusals before the reservation exists are returned as the nonadmission code the publication records; a firing that already holds a matching reservation reuses it instead of reserving again. `persist` returns the session row version after the reservation is written.
pub fn reserve_and_stage(
    target: &HandoffTarget,
    request: &HandoffRequest<'_>,
    persist: impl FnOnce(&CuratorReservation) -> Result<u64, HistorySummarizerStateError>,
) -> Result<Handoff, HandoffError> {
    let HandoffRequest {
        session_id,
        firing,
        facts,
        aliases,
        now_ms,
        ..
    } = *request;
    let subject = review_subject(facts, aliases).map_err(|_| HandoffError::UnknownAlias)?;
    let payload = ReviewPayload::Subject(subject);
    // Refused before any reservation exists: the publication records it and advances.
    let Ok(payload_digest) = payload.digest() else {
        return Ok(Handoff::Nonadmission(
            CuratorNonadmissionCode::SubjectRefused,
        ));
    };
    // The candidate and its run are named by the subject bytes, not the firing, so a later firing that adopts the reservation restages the same row under the same identity.
    let candidate_id = format!("hs-{session_id}-{}", &payload_digest[..32]);
    let extraction_run_id = format!("hs-run-{session_id}-{}", &payload_digest[..32]);
    let chunk_ordinal = firing
        .chunk_range
        .as_ref()
        .ok_or(HandoffError::NoChunkRange)?
        .from_ordinal;
    let producer = ProducerBinding {
        producer: PRODUCER.to_string(),
        firing_id: format!("{session_id}#{}", firing.firing_seq),
        ordinal: chunk_ordinal,
    };
    let inputs = CausalInputs {
        target: ReviewTarget::StagedSubject {
            kernel_incarnation: target.kernel_incarnation.clone(),
            candidate_id: candidate_id.clone(),
            payload_digest: payload_digest.clone(),
        },
        question_template: QuestionTemplate::ExtractedFacts.id().to_string(),
        signals: Vec::new(),
        required_evidence: Vec::new(),
        policy_versions: review_policy_versions(),
    };
    let job = match reserved_row(
        request,
        &producer,
        &inputs,
        &payload_digest,
        &target.kernel_incarnation,
    )? {
        ReservedRow::Job(job) => *job,
        ReservedRow::Done(handoff) => return Ok(handoff),
    };
    let row_version = persist(&CuratorReservation {
        firing_seq: firing.firing_seq,
        causal_identity: job.causal_identity.clone(),
        candidate_id: candidate_id.clone(),
        payload_digest: payload_digest.clone(),
        kernel_incarnation: target.kernel_incarnation.clone(),
        queue_deadline_ms: job.queue_deadline_ms,
    })?;

    // Staging under the reservation's identity and deadline. A byte-identical restage of a live row returns the same reference, a sealed row refuses the restage, and either way the row must read back as exactly the reserved subject.
    let binding = review_binding(
        &target.project_digest,
        &target.domain_id,
        session_id,
        chunk_ordinal,
        &job.causal_identity,
    );
    let reference = ReviewStagedReference {
        database_incarnation_id: target.kernel_incarnation.clone(),
        candidate_id: candidate_id.clone(),
        payload_digest,
    };
    match target.kernel.stage_review_input(ReviewStagingSpec {
        extraction_run_id: extraction_run_id.clone(),
        candidate_id,
        producer: PRODUCER.to_string(),
        binding: binding.clone(),
        payload,
        recorded_at: now_ms,
        queue_deadline_at: job.queue_deadline_ms,
    }) {
        Ok(_) => match target.kernel.finish_staging_run(
            &extraction_run_id,
            StagingTerminalState::Completed,
            now_ms,
        ) {
            Ok(()) | Err(kernel::KernelError::Conflict) => {}
            Err(error) => return Err(HandoffError::Seal(error)),
        },
        Err(ReviewStageError::Refused(kernel::ReviewStageRefusal::Terminal)) => {}
        Err(error) => return Err(error.into()),
    }
    target
        .kernel
        .read_review_input(&reference, &binding, now_ms)
        .map_err(HandoffError::Read)?;

    Ok(Handoff::Activate(Box::new(PreparedActivation {
        causal_identity: job.causal_identity,
        producer,
        input: CuratorJobInput {
            subject: job.target,
            starting_references: Vec::new(),
            question_template: QuestionTemplate::ExtractedFacts.id().to_string(),
        },
        row_version,
    })))
}

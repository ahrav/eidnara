//! Exact-target acceptance of a MemoryReviewer proposal. One Kernel envelope validates the selected proposal row bound to the run, its live review hold, every input the run disclosed, the full policy union with each owner live and served to a local reader of the project, and the proposal's exact target, then applies the proposal's action and records one proposal-level settlement beside it.
//!
//! The settlement is a receipt under the reserved producer [`SETTLEMENT_PRODUCER`]. Its key is a versioned, length-delimited derivation over the project digest, the proposal id, its payload digest, the receipt generation, and both store incarnations; the caller's operation key is not part of it, and its request digest is derived here from that key. A second acceptance of one proposal under any caller key therefore replays the first settlement, and that replay is checked before the review hold, the queue deadline, or the payload is read. The caller's own intent keeps its usual meaning: a key that committed an acceptance replays only that acceptance, and the same key for other input conflicts.
//!
//! Retire runs the existing retirement and its authority cascade. Retain and no-change write nothing canonical. Create and revise are refused as unsupported.

use context_core::memory_reviewer_policy_union::{PolicyUnion, PolicyUnionMember};
use rusqlite::{OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::eligibility::judge_in_tx;
use super::envelope::{CommitIntent, Envelope, commit_with_writer, recorded_receipt};
use super::memory_reviewer_hold::validate_review_evidence_in;
use super::review_staging::bound_review_row_in;
use super::{
    ArtifactDestination, CachedSql, CanonicalTarget, EligibilityCandidate, EligibilityVerdict,
    KernelError, KernelStore, MemoryReviewerHoldBinding, MemoryReviewerHoldError,
    MemoryReviewerHoldRefusal, ObjectState, ObservationPayload, ProjectScope, ProposalAction,
    ProposalTarget, REVIEW_DEPENDENCIES_VERSION, ReviewOwner, ReviewPayload, ReviewProposal,
    ReviewQuestionTemplate, ReviewReadError, ReviewReadRefusal, ReviewStagedReference, Sensitivity,
    SourceDescriptorDetail, current_time_ms, map_sqlite, provisional_result_identity,
};
use crate::source_identity::{OccurrenceClass, identity_digest};

/// The reserved producer every settlement receipt is recorded under.
pub const SETTLEMENT_PRODUCER: &str = "eidnara-kernel/memory-reviewer-settlement";
const SETTLEMENT_KEY_VERSION: &str = "memory-reviewer-settlement-v1";
/// Version of the [`Settlement`] shape a receipt stores.
pub const SETTLEMENT_VERSION: u32 = 1;
/// One explicit acceptance of the proposal a completed receipt selects. The daemon copies every field but `intent` from the trusted receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalAcceptance {
    /// The caller's idempotency intent; its key never enters the settlement identity.
    pub intent: CommitIntent,
    /// The job's run binding: project digest, both incarnations, the job's causal identity as `subject`, and the receipt generation.
    pub run: MemoryReviewerHoldBinding,
    /// The selected row: the Kernel incarnation, the proposal id, and its payload digest.
    pub proposal: ReviewStagedReference,
    /// The receipt's completion time, which the Memory Store fenced against the row's queue deadline; the deadline is judged against it. The Kernel cannot check where it came from, so the caller copies it from the trusted receipt and from nowhere else.
    pub selected_at: i64,
    /// The target the caller read and accepts; it must equal the proposal's.
    pub target: ProposalTarget,
}

/// What one acceptance did, keyed by its proposal. A refused acceptance records nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settlement {
    /// [`SETTLEMENT_VERSION`] when written.
    pub version: u32,
    /// The selected proposal row's candidate id.
    pub proposal_id: String,
    pub payload_digest: String,
    pub receipt_generation: u64,
    pub kernel_incarnation: String,
    pub memstore_incarnation: String,
    pub project_digest: String,
    pub action: ProposalAction,
    /// The target expectation the envelope validated.
    pub target: ProposalTarget,
    /// The commit that applied the settlement.
    pub commit_seq: i64,
    pub effect: SettlementEffect,
}

/// The action-specific tail of a settlement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SettlementEffect {
    Retired {
        object_id: String,
        object_kind: String,
        invalidated_commit_seq: i64,
        granted_authority_before: bool,
        demoted: usize,
        deferred: usize,
    },
    /// Retain and no-change: no canonical write, TTL refresh, maturity change, or corroboration.
    Unchanged,
}

/// A settlement and whether it was recorded by an earlier acceptance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptanceOutcome {
    pub settlement: Settlement,
    pub replayed: bool,
}

/// Why an acceptance was refused. Each refusal leaves the store unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AcceptanceRefusal {
    /// No selected proposal of this run is readable at the reference: missing, bound to another project, owner, or incarnation, or not a proposal. One answer covers them all, so a refusal discloses nothing about rows the caller did not name.
    #[error("unavailable")]
    Unavailable,
    /// The review hold is released or expired, or the row's queue deadline is not after its selection.
    #[error("review_expired")]
    Expired,
    /// The target, an owner, or a disclosed input no longer stands as the proposal read it, or the caller's target is not the proposal's.
    #[error("changed")]
    Changed,
    /// The caller reused its operation key for other input.
    #[error("conflict")]
    Conflict,
    /// The action is one this build does not apply.
    #[error("unsupported")]
    Unsupported,
    /// A malformed binding, reference, or intent, a reserved caller producer, or a selection time before the row was sealed.
    #[error("invalid")]
    Invalid,
}

/// A refused acceptance, or a store failure that decided nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AcceptanceError {
    #[error(transparent)]
    Refused(#[from] AcceptanceRefusal),
    #[error("kernel {0}")]
    Store(KernelError),
}

impl From<KernelError> for AcceptanceError {
    fn from(error: KernelError) -> Self {
        Self::Store(error)
    }
}

impl KernelStore {
    /// The settlement already recorded for the proposal `run` and `proposal` name, or `None`. A read, so a caller can answer a settled proposal before it touches any expirable state.
    pub fn review_settlement(
        &self,
        run: &MemoryReviewerHoldBinding,
        proposal: &ReviewStagedReference,
    ) -> Result<Option<Settlement>, AcceptanceError> {
        let intent = settlement_intent(run, proposal)?;
        let mut reader = self.lock_reader()?;
        // One read snapshot; dropping it rolls nothing back because it wrote nothing.
        let tx = reader
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(map_sqlite)?;
        recorded_receipt(&tx, intent)?
            .map(|receipt| decode(&receipt.result))
            .transpose()
    }

    /// Applies the proposal `acceptance` names in one envelope, or replays the settlement an earlier acceptance of it recorded. A caller key that committed an acceptance is bound to it; a key that only received a replay is not recorded.
    ///
    /// # Errors
    ///
    /// [`AcceptanceError::Refused`] when the proposal, its hold, its target, or a disclosed input does not stand, or the caller key was reused for other input; [`AcceptanceError::Store`] for storage, lock, or fence failures.
    pub fn accept_review_proposal(
        &self,
        acceptance: &ProposalAcceptance,
    ) -> Result<AcceptanceOutcome, AcceptanceError> {
        acceptance
            .intent
            .refuse_reserved_producer()
            .map_err(|_| AcceptanceRefusal::Invalid)?;
        let settlement = settlement_intent(&acceptance.run, &acceptance.proposal)?;
        let mut writer = self.lock_writer()?;
        // The settlement replay comes before the hold, the deadline, or the payload: a settled proposal answers its settlement whatever has expired since. A caller key with a receipt falls through, so the commit path replays it under its fence and alignment repair.
        {
            let tx = writer
                .transaction_with_behavior(TransactionBehavior::Deferred)
                .map_err(map_sqlite)?;
            let caller = match recorded_receipt(&tx, acceptance.intent.clone()) {
                Err(KernelError::Conflict) => return Err(AcceptanceRefusal::Conflict.into()),
                other => other?,
            };
            if caller.is_none()
                && let Some(receipt) = recorded_receipt(&tx, settlement.clone())?
            {
                let recorded = decode(&receipt.result)?;
                if recorded.target != acceptance.target {
                    return Err(AcceptanceRefusal::Changed.into());
                }
                return Ok(AcceptanceOutcome {
                    settlement: recorded,
                    replayed: true,
                });
            }
        }
        let mut refusal = None;
        let receipt = commit_with_writer(
            &mut writer,
            self.lease_epoch(),
            acceptance.intent.clone(),
            |envelope| {
                apply(envelope, acceptance, &settlement).map_err(|error| {
                    // Every refusal is classified inside `apply`; a store conflict there is a moved row, never the caller's key.
                    refusal = Some(match error {
                        AcceptanceError::Refused(code) => code,
                        AcceptanceError::Store(KernelError::Conflict) => AcceptanceRefusal::Changed,
                        AcceptanceError::Store(error) => return error,
                    });
                    KernelError::Conflict
                })
            },
            || Ok(()),
        );
        match receipt {
            Ok(receipt) => {
                let recorded = decode(&receipt.result)?;
                // A caller key replays only the acceptance it committed: the same proposal at the same target.
                if receipt.replayed
                    && (recorded.proposal_id != acceptance.proposal.candidate_id
                        || recorded.payload_digest != acceptance.proposal.payload_digest
                        || recorded.receipt_generation != acceptance.run.generation
                        || recorded.project_digest != acceptance.run.project_digest
                        || recorded.kernel_incarnation != acceptance.run.kernel_incarnation
                        || recorded.memstore_incarnation != acceptance.run.memstore_incarnation
                        || recorded.target != acceptance.target)
                {
                    return Err(AcceptanceRefusal::Conflict.into());
                }
                Ok(AcceptanceOutcome {
                    settlement: recorded,
                    replayed: receipt.replayed,
                })
            }
            Err(error) => Err(refusal.map_or(AcceptanceError::Store(error), Into::into)),
        }
    }
}

/// The settlement receipt identity for one proposal of one run: every field is length-prefixed, so two field sets that concatenate to one text derive distinct keys.
fn settlement_intent(
    run: &MemoryReviewerHoldBinding,
    proposal: &ReviewStagedReference,
) -> Result<CommitIntent, AcceptanceError> {
    run.validate().map_err(|_| AcceptanceRefusal::Invalid)?;
    if proposal.candidate_id
        != provisional_result_identity(&run.subject, run.generation).candidate_id
        || !super::cas::is_artifact_digest(&proposal.payload_digest)
    {
        return Err(AcceptanceRefusal::Invalid.into());
    }
    let generation = run.generation.to_string();
    let mut key = String::from(SETTLEMENT_KEY_VERSION);
    for field in [
        run.project_digest.as_str(),
        proposal.candidate_id.as_str(),
        proposal.payload_digest.as_str(),
        generation.as_str(),
        run.kernel_incarnation.as_str(),
        run.memstore_incarnation.as_str(),
    ] {
        key.push_str(&format!(":{}#{field}", field.len()));
    }
    Ok(CommitIntent {
        producer: SETTLEMENT_PRODUCER.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        operation_key: key,
        actor: "memory-reviewer".to_string(),
        cause: "proposal accepted".to_string(),
    })
}

fn decode(result: &str) -> Result<Settlement, AcceptanceError> {
    serde_json::from_str::<Settlement>(result)
        .ok()
        .filter(|settlement| settlement.version == SETTLEMENT_VERSION)
        .ok_or(AcceptanceError::Store(KernelError::Io))
}

fn changed<T>() -> Result<T, AcceptanceError> {
    Err(AcceptanceRefusal::Changed.into())
}

fn read_refusal(error: ReviewReadError) -> AcceptanceError {
    match error {
        ReviewReadError::Refused(
            ReviewReadRefusal::Changed | ReviewReadRefusal::Unsealed | ReviewReadRefusal::Abandoned,
        ) => AcceptanceRefusal::Changed.into(),
        ReviewReadError::Refused(ReviewReadRefusal::Expired) => AcceptanceRefusal::Expired.into(),
        ReviewReadError::Refused(
            ReviewReadRefusal::Missing
            | ReviewReadRefusal::IncarnationMismatch
            | ReviewReadRefusal::ScopeMismatch
            | ReviewReadRefusal::DecodeRefused,
        ) => AcceptanceRefusal::Unavailable.into(),
        ReviewReadError::Invalid => AcceptanceRefusal::Invalid.into(),
        ReviewReadError::Store(error) => error.into(),
    }
}

fn hold_refusal(error: MemoryReviewerHoldError) -> AcceptanceError {
    match error {
        MemoryReviewerHoldError::Refused(
            MemoryReviewerHoldRefusal::Missing
            | MemoryReviewerHoldRefusal::Released
            | MemoryReviewerHoldRefusal::Expired,
        ) => AcceptanceRefusal::Expired.into(),
        MemoryReviewerHoldError::Refused(
            MemoryReviewerHoldRefusal::PurgeDegraded
            | MemoryReviewerHoldRefusal::NotCovered
            | MemoryReviewerHoldRefusal::UnavailableEvidence,
        ) => AcceptanceRefusal::Changed.into(),
        MemoryReviewerHoldError::Refused(MemoryReviewerHoldRefusal::IncarnationMismatch) => {
            AcceptanceRefusal::Unavailable.into()
        }
        MemoryReviewerHoldError::Refused(
            MemoryReviewerHoldRefusal::InvalidRequest
            | MemoryReviewerHoldRefusal::TooManyReferences
            | MemoryReviewerHoldRefusal::ProjectHoldLimit
            | MemoryReviewerHoldRefusal::HostHoldLimit
            | MemoryReviewerHoldRefusal::ProjectBackingExhausted
            | MemoryReviewerHoldRefusal::HostBackingExhausted,
        ) => AcceptanceRefusal::Invalid.into(),
        MemoryReviewerHoldError::Store(error) => error.into(),
    }
}

/// A canonical write the validated action makes can still be refused by the Kernel's own rules, such as a barred lineage or a parent retired since staging; the proposal no longer applies, which is a changed input, not a store failure.
fn action_refusal(error: KernelError) -> AcceptanceError {
    match error {
        KernelError::AdmissionPolicy | KernelError::NotFound | KernelError::Conflict => {
            AcceptanceRefusal::Changed.into()
        }
        error => error.into(),
    }
}

/// What validation proved: the proposal and the project it is judged in.
struct Validated {
    proposal: ReviewProposal,
    project: ProjectScope,
}

/// Validates and applies one acceptance inside its envelope and records the settlement receipt; returns the settlement's JSON for the caller's receipt.
fn apply(
    envelope: &mut Envelope<'_>,
    acceptance: &ProposalAcceptance,
    settlement_key: &CommitIntent,
) -> Result<String, AcceptanceError> {
    let validated = validate(envelope, acceptance)?;
    let proposal = &validated.proposal;
    let effect = match (proposal.action, &proposal.target) {
        (ProposalAction::Retain | ProposalAction::NoChange, ProposalTarget::Memory(target)) => {
            target_stands(envelope, &validated.project, target)?;
            SettlementEffect::Unchanged
        }
        (ProposalAction::NoChange, ProposalTarget::StagedCandidate { candidate_id }) => {
            subject_stands(envelope, acceptance, candidate_id)?;
            SettlementEffect::Unchanged
        }
        (ProposalAction::Retire, ProposalTarget::Memory(target)) => {
            target_stands(envelope, &validated.project, target)?;
            let retirement = envelope
                .retire_decision_with_cascade(&target.object_id)
                .map_err(action_refusal)?;
            SettlementEffect::Retired {
                object_id: retirement.outcome.object_id,
                object_kind: retirement.outcome.object_kind,
                invalidated_commit_seq: retirement.invalidated_commit_seq,
                granted_authority_before: retirement.granted_before,
                demoted: retirement.demoted,
                deferred: retirement.deferred,
            }
        }
        (ProposalAction::Create | ProposalAction::Revise, _) => {
            return Err(AcceptanceRefusal::Unsupported.into());
        }
        // Staging admits only legal action and target pairs, so a stored row with another pair is corruption.
        _ => return Err(KernelError::Io.into()),
    };
    let run = &acceptance.run;
    let settlement = Settlement {
        version: SETTLEMENT_VERSION,
        proposal_id: acceptance.proposal.candidate_id.clone(),
        payload_digest: acceptance.proposal.payload_digest.clone(),
        receipt_generation: run.generation,
        kernel_incarnation: run.kernel_incarnation.clone(),
        memstore_incarnation: run.memstore_incarnation.clone(),
        project_digest: run.project_digest.clone(),
        action: proposal.action,
        target: proposal.target.clone(),
        commit_seq: envelope.commit_seq,
        effect,
    };
    let encoded = serde_json::to_string(&settlement).map_err(|_| KernelError::Io)?;
    envelope.record_reserved_receipt(settlement_key.clone(), &encoded)?;
    Ok(encoded)
}

/// Everything an action depends on except its target: the row bound to this run, its dependency record, every disclosed input under the live review hold, and every policy-union member.
fn validate(
    envelope: &Envelope<'_>,
    acceptance: &ProposalAcceptance,
) -> Result<Validated, AcceptanceError> {
    let run = &acceptance.run;
    let incarnation = super::open::database_incarnation_id_via(envelope.tx)?;
    if incarnation != run.kernel_incarnation {
        return Err(AcceptanceRefusal::Unavailable.into());
    }
    let row = bound_review_row_in(
        envelope.tx,
        &acceptance.proposal,
        &run.project_digest,
        &ReviewOwner::Proposal {
            job_id: run.subject.clone(),
            generation: run.generation,
        },
        acceptance.selected_at,
    )
    .map_err(read_refusal)?;
    let ReviewPayload::Proposal(proposal) = row.payload else {
        return Err(AcceptanceRefusal::Unavailable.into());
    };
    if row.sensitivity == Sensitivity::Secret || proposal.target != acceptance.target {
        return changed();
    }
    let Some(dependencies) = row.dependencies.filter(|record| {
        record.version == REVIEW_DEPENDENCIES_VERSION && record.generation == run.generation
    }) else {
        return changed();
    };
    // Every disclosed input, cited or not, is still covered by the live review hold and names live, unpurged, non-secret evidence; every citation is among them.
    let mut disclosed: Vec<String> = proposal
        .policy_dependencies
        .disclosed_inputs
        .iter()
        .map(|reference| reference.evidence_id.clone())
        .collect();
    disclosed.sort_unstable();
    disclosed.dedup();
    let review = MemoryReviewerHoldBinding {
        subject: acceptance.proposal.candidate_id.clone(),
        ..run.clone()
    };
    let now = current_time_ms();
    let held =
        validate_review_evidence_in(envelope.tx, &review, &disclosed, now).map_err(hold_refusal)?;
    if held
        .iter()
        .any(|evidence| evidence.sensitivity == Sensitivity::Secret)
        || proposal
            .support
            .iter()
            .chain(&proposal.contradictions)
            .any(|cited| disclosed.binary_search(&cited.evidence_id).is_err())
    {
        return changed();
    }
    let project = ProjectScope::new(&run.project_digest).map_err(|_| AcceptanceRefusal::Invalid)?;
    let union = PolicyUnion::decode(&dependencies.union_canonical, &dependencies.union_digest)
        .or_else(|_| changed())?;
    for member in union.members() {
        member_stands(envelope, acceptance, &project, &proposal, member)?;
    }
    Ok(Validated {
        proposal: *proposal,
        project,
    })
}

/// Whether the batch verdicts let a local reader of `project` see each object: every one `Ok`, except a descriptor, which no surface serves and whose bytes the evidence check already judged.
fn served(
    envelope: &Envelope<'_>,
    project: &ProjectScope,
    candidates: &[(EligibilityCandidate, bool)],
) -> Result<bool, AcceptanceError> {
    let judged: Vec<EligibilityCandidate> = candidates
        .iter()
        .map(|(candidate, _)| candidate.clone())
        .collect();
    let verdicts = judge_in_tx(
        envelope.tx,
        envelope.commit_seq - 1,
        project,
        ArtifactDestination::Local,
        &judged,
    )?;
    Ok(verdicts
        .iter()
        .zip(candidates)
        .all(|(verdict, (_, descriptor))| {
            *verdict == EligibilityVerdict::Ok
                || (*descriptor && *verdict == EligibilityVerdict::Hidden)
        }))
}

/// The target decision is live and served at exactly the proposal's tokens: the same object, kind, and source revision, and no change after the commit the proposer observed. Returns its state.
fn target_stands(
    envelope: &Envelope<'_>,
    project: &ProjectScope,
    target: &CanonicalTarget,
) -> Result<ObjectState, AcceptanceError> {
    if target.known_as_of >= envelope.commit_seq || target.commit_token > target.known_as_of {
        return changed();
    }
    let Some(state) = envelope.object_state(&target.object_id)? else {
        return changed();
    };
    if state.object.object_kind != "decision"
        || state.object.invalidated_commit_seq.is_some()
        || state.object.source_revision != target.source_revision
        || state.latest_change_commit_seq != Some(target.commit_token)
        || !served(
            envelope,
            project,
            &[(
                EligibilityCandidate {
                    object_id: target.object_id.clone(),
                    source_revision: target.source_revision,
                    artifact_digest: None,
                },
                false,
            )],
        )?
    {
        return changed();
    }
    Ok(state)
}

/// The staged subject a create or no-change names, or a union member names, is this job's sealed subject row at `digest`, judged at the selection time; with no `digest`, at whatever digest it is stored.
fn subject_row_stands(
    envelope: &Envelope<'_>,
    acceptance: &ProposalAcceptance,
    candidate_id: &str,
    digest: Option<&str>,
) -> Result<(), AcceptanceError> {
    let stored = match digest {
        Some(digest) => Some(digest.to_string()),
        None => envelope
            .tx
            .query_row_cached(
                "SELECT payload FROM candidates WHERE candidate_id=?1",
                [candidate_id],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()
            .map_err(map_sqlite)?
            .map(|payload| identity_digest(&payload)),
    };
    let Some(payload_digest) = stored else {
        return changed();
    };
    let row = bound_review_row_in(
        envelope.tx,
        &ReviewStagedReference {
            database_incarnation_id: acceptance.run.kernel_incarnation.clone(),
            candidate_id: candidate_id.to_string(),
            payload_digest,
        },
        &acceptance.run.project_digest,
        &ReviewOwner::Job {
            job_id: acceptance.run.subject.clone(),
        },
        acceptance.selected_at,
    );
    match row {
        Ok(row) if matches!(row.payload, ReviewPayload::Subject(_)) => Ok(()),
        Ok(_) => changed(),
        Err(ReviewReadError::Store(error)) => Err(error.into()),
        Err(_) => changed(),
    }
}

fn subject_stands(
    envelope: &Envelope<'_>,
    acceptance: &ProposalAcceptance,
    candidate_id: &str,
) -> Result<(), AcceptanceError> {
    subject_row_stands(envelope, acceptance, candidate_id, None)
}

/// One policy-union member still stands as the run read it, judged the way a reader's revalidation judges it: the question template is the proposal's; a descriptor is live at its revision and still names its decision owner, which is live, served, and at the owner revision; a staged subject is this job's sealed row at its digest; and a capture is live, unpurged evidence at its digest.
fn member_stands(
    envelope: &Envelope<'_>,
    acceptance: &ProposalAcceptance,
    project: &ProjectScope,
    proposal: &ReviewProposal,
    member: &PolicyUnionMember,
) -> Result<(), AcceptanceError> {
    let stands = match (
        member.kind.as_str(),
        member.owner_id.as_deref(),
        member.owner_revision.as_deref(),
    ) {
        ("question_template", None, None) => {
            let ReviewQuestionTemplate::ExtractedFacts =
                proposal.policy_dependencies.question_template;
            member.id == "extracted_facts"
        }
        ("native_source", None, None) => descriptor_stands(envelope, project, member, None)?,
        ("canonical_source", Some(owner), Some(owner_revision)) => {
            descriptor_stands(envelope, project, member, Some((owner, owner_revision)))?
        }
        ("staged_subject", None, None) => {
            subject_row_stands(envelope, acceptance, &member.id, Some(&member.revision))?;
            true
        }
        ("temporary_capture", None, None) => envelope
            .tx
            .query_row_cached(
                "SELECT EXISTS(SELECT 1 FROM evidence_meta e
                 WHERE e.evidence_id=?1 AND e.artifact_digest=?2 AND e.invalidated_commit_seq IS NULL
                   AND NOT EXISTS(SELECT 1 FROM artifact_purge_tombstones t
                                  WHERE t.artifact_digest=e.artifact_digest))",
                rusqlite::params![member.id, member.revision],
                |row| row.get::<_, bool>(0),
            )
            .map_err(map_sqlite)?,
        _ => false,
    };
    if stands { Ok(()) } else { changed() }
}

/// A native or canonical descriptor member is a live descriptor at its revision whose class agrees with the member kind; a canonical one's detail names `owner` in its class's leading identity field, and the owner is a live decision at `owner_revision`. Both pass the batch eligibility judgement for a local reader.
fn descriptor_stands(
    envelope: &Envelope<'_>,
    project: &ProjectScope,
    member: &PolicyUnionMember,
    owner: Option<(&str, &str)>,
) -> Result<bool, AcceptanceError> {
    let Ok(revision) = member.revision.parse::<i64>() else {
        return Ok(false);
    };
    let Some(state) = envelope.object_state(&member.id)? else {
        return Ok(false);
    };
    let payload: Option<Vec<u8>> = envelope
        .tx
        .query_row_cached(
            "SELECT observation_payload FROM observations WHERE object_id=?1",
            [member.id.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_sqlite)?;
    let Some(detail) = payload
        .and_then(|payload| serde_json::from_slice::<ObservationPayload>(&payload).ok())
        .and_then(|payload| payload.detail)
        .and_then(|detail| serde_json::from_str::<SourceDescriptorDetail>(&detail).ok())
    else {
        return Ok(false);
    };
    let Some(class) = OccurrenceClass::from_code(&detail.class) else {
        return Ok(false);
    };
    if state.object.invalidated_commit_seq.is_some() || state.object.source_revision != revision {
        return Ok(false);
    }
    let mut candidates = vec![(
        EligibilityCandidate {
            object_id: member.id.clone(),
            source_revision: revision,
            artifact_digest: Some(detail.artifact_digest.clone()),
        },
        true,
    )];
    match owner {
        None => {
            if decision_derived(class) {
                return Ok(false);
            }
        }
        Some((owner, owner_revision)) => {
            let leading = class.identity_fields().first().copied();
            let names_owner = detail
                .identity
                .iter()
                .any(|(field, value)| Some(field.as_str()) == leading && value == owner);
            let Ok(owner_revision) = owner_revision.parse::<i64>() else {
                return Ok(false);
            };
            let Some(owner_state) = envelope.object_state(owner)? else {
                return Ok(false);
            };
            if !decision_derived(class)
                || !names_owner
                || owner_state.object.object_kind != "decision"
                || owner_state.object.invalidated_commit_seq.is_some()
                || owner_state.object.source_revision != owner_revision
            {
                return Ok(false);
            }
            candidates.push((
                EligibilityCandidate {
                    object_id: owner.to_string(),
                    source_revision: owner_revision,
                    artifact_digest: None,
                },
                false,
            ));
        }
    }
    served(envelope, project, &candidates)
}

/// The descriptor classes whose leading identity field names an originating decision.
const fn decision_derived(class: OccurrenceClass) -> bool {
    matches!(
        class,
        OccurrenceClass::CanonicalClaims | OccurrenceClass::PromotedMemory
    )
}

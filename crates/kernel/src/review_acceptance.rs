//! Exact-target acceptance of a MemoryReviewer proposal. One Kernel envelope validates the selected proposal row bound to the run, its live review hold, every input the run disclosed, the full policy union with each owner live and served to a local reader of the project, and the proposal's exact target, then applies the proposal's action and records one proposal-level settlement beside it.
//!
//! The settlement is a receipt under the reserved producer [`SETTLEMENT_PRODUCER`]. Its key is a versioned, length-delimited derivation over the project digest, the proposal id, its payload digest, the receipt generation, and both store incarnations; the caller's operation key is not part of it, and its request digest is derived here from that key. A second acceptance of one proposal under any caller key therefore replays the first settlement, and that replay is checked before the review hold, the queue deadline, or the payload is read. The caller's own intent keeps its usual meaning: a key that committed an acceptance replays only that acceptance, and the same key for other input conflicts.
//!
//! Create inserts one decision whose identity derives from the settlement key and admits it as `ModelInference` with `MemoryClassifierInference` taint under `EventKind::Other`. Revise writes a fresh successor that keeps the predecessor's domain and source identity at the next source revision and is admitted on its own. Retire runs the existing retirement and its authority cascade. Retain and no-change write nothing canonical. Create and revise cite each supporting evidence row through a decision event, so the support stays durable after the review hold ends.

use context_core::memory_reviewer_policy_union::{PolicyUnion, PolicyUnionMember};
use rusqlite::{OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::admission::SUPPORT_EVENT_KIND;
use super::eligibility::judge_with_states_in_tx;
use super::envelope::{
    CommitIntent, Envelope, PreparedIntent, commit_intent_with_writer, recorded_receipt,
};
use super::memory_reviewer_hold::validate_review_evidence_in;
use super::review_staging::bound_review_row_in;
use super::{
    AdmissionDecision, AdmissionEvent, AdmissionRequest, ArtifactDestination, CachedSql,
    CanonicalTarget, DecisionPayload, DecisionSpec, EligibilityCandidate, EligibilityVerdict,
    EventKind, HeldEvidence, KernelError, KernelStore, MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS,
    MemoryReviewerHoldBinding, MemoryReviewerHoldError, MemoryReviewerHoldRefusal, ObjectState,
    ObservationPayload, ProjectScope, ProposalAction, ProposalTarget, REVIEW_DEPENDENCIES_VERSION,
    ReviewOwner, ReviewPayload, ReviewProposal, ReviewQuestionTemplate, ReviewReadError,
    ReviewReadRefusal, ReviewStagedReference, Sensitivity, SourceClass, SourceDescriptorDetail,
    TaintClass, current_time_ms, map_sqlite, provisional_result_identity,
};
use crate::scope::load_scope_terms;
use crate::source_identity::{OccurrenceClass, identity_digest};

/// The reserved producer every settlement receipt is recorded under.
pub const SETTLEMENT_PRODUCER: &str = "eidnara-kernel/memory-reviewer-settlement";
const SETTLEMENT_KEY_VERSION: &str = "memory-reviewer-settlement-v1";
/// Version of the [`Settlement`] shape a receipt stores.
pub const SETTLEMENT_VERSION: u32 = 1;
/// `source_kind` of a decision an accepted create inserts; its `source_id` is the proposal id.
pub const ACCEPTED_SOURCE_KIND: &str = "memory_reviewer_proposal";
/// `decision_kind` of a decision an accepted create inserts.
const ACCEPTED_DECISION_KIND: &str = "memory_reviewer_create";
/// `event_payload.summary` of each support event.
const SUPPORT_EVENT_SUMMARY: &str = "supporting evidence of the accepted proposal";
/// The decision kind that makes a decision an approval; model-written text never carries it.
const APPROVAL_DECISION_KIND: &str = "adr_accepted";
/// The classes every accepted create or revise is admitted under.
const ACCEPTED_SOURCE_CLASS: SourceClass = SourceClass::ModelInference;
const ACCEPTED_TAINT_CLASS: TaintClass = TaintClass::MemoryClassifierInference;
const ADMISSION_REASON: &str = "memory reviewer proposal accepted";

/// One explicit acceptance of the proposal a completed receipt selects. The daemon copies every field but `intent` and `scope_id` from the trusted receipt.
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
    /// The revision of the question template the caller runs; the union's template member must carry it.
    pub question_template_revision: String,
    /// The live scope a created decision is scoped to; it must name the run's project. Required for create and ignored otherwise.
    pub scope_id: Option<String>,
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
    Created {
        object_id: String,
        admission: AdmittedState,
    },
    Revised {
        object_id: String,
        predecessor_object_id: String,
        predecessor_source_revision: i64,
        source_revision: i64,
        /// Always `superseded`: the predecessor is invalidated and names the successor.
        predecessor_disposition: String,
        admission: AdmittedState,
    },
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

/// The admission the decision a create or revise wrote received, which may be a denial on a correct store. Disposition, maturity, visibility, and sensitivity are the stored decision's; source and taint class are the classes the admission was recorded under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmittedState {
    pub disposition: String,
    pub effective_maturity: String,
    pub visibility: String,
    pub source_class: String,
    pub taint_class: String,
    pub sensitivity_class: String,
}

impl AdmittedState {
    fn of(decision: &AdmissionDecision) -> Self {
        Self {
            disposition: decision.disposition.as_str().to_string(),
            effective_maturity: decision.effective_maturity.as_str().to_string(),
            visibility: decision.visibility.as_str().to_string(),
            source_class: ACCEPTED_SOURCE_CLASS.as_str().to_string(),
            taint_class: ACCEPTED_TAINT_CLASS.as_str().to_string(),
            sensitivity_class: decision.sensitivity.as_str().to_string(),
        }
    }
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
    /// A malformed binding, reference, or intent, a reserved caller producer, a selection time before the row was sealed, or a create without a scope.
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
        recorded_receipt(&tx, &PreparedIntent::new(intent)?)?
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
        let settlement_key = settlement_intent(&acceptance.run, &acceptance.proposal)?;
        let settlement = PreparedIntent::new(settlement_key.clone())?;
        let caller = PreparedIntent::new(acceptance.intent.clone()).map_err(intent_refusal)?;
        let mut writer = self.lock_writer()?;
        let committed = {
            let tx = writer
                .transaction_with_behavior(TransactionBehavior::Deferred)
                .map_err(map_sqlite)?;
            let caller_receipt = match recorded_receipt(&tx, &caller) {
                Err(KernelError::Conflict) => return Err(AcceptanceRefusal::Conflict.into()),
                other => other?,
            };
            match (caller_receipt, recorded_receipt(&tx, &settlement)?) {
                (None, None) => None,
                (None, Some(settled)) => {
                    let recorded = decode(&settled.result)?;
                    if recorded.target != acceptance.target {
                        return Err(AcceptanceRefusal::Changed.into());
                    }
                    return Ok(AcceptanceOutcome {
                        settlement: recorded,
                        replayed: true,
                    });
                }
                (Some(caller), Some(settled)) if caller.commit_seq == settled.commit_seq => {
                    let recorded = decode(&settled.result)?;
                    if recorded.target != acceptance.target {
                        return Err(AcceptanceRefusal::Conflict.into());
                    }
                    Some(recorded)
                }
                (Some(_), _) => return Err(AcceptanceRefusal::Conflict.into()),
            }
        };
        let mut refusal = None;
        let mut applied = None;
        let receipt = commit_intent_with_writer(
            &mut writer,
            self.lease_epoch(),
            caller,
            |envelope| {
                let (recorded, encoded) = apply(envelope, acceptance, &settlement_key, &settlement)
                    .map_err(|error| {
                        // Every refusal is classified inside `apply`; a store conflict there is a moved row, never the caller's key.
                        refusal = Some(match error {
                            AcceptanceError::Refused(code) => code,
                            AcceptanceError::Store(KernelError::Conflict) => {
                                AcceptanceRefusal::Changed
                            }
                            AcceptanceError::Store(error) => return error,
                        });
                        KernelError::Conflict
                    })?;
                applied = Some((recorded, encoded.clone()));
                Ok(encoded)
            },
            || Ok(()),
        );
        match receipt {
            Ok(receipt) => {
                let settlement = match (committed, receipt.replayed, applied) {
                    (Some(settlement), true, _) => settlement,
                    // A fresh commit stores the settlement it applied unless redaction changed its text.
                    (None, false, Some((recorded, encoded))) if receipt.result == encoded => {
                        recorded
                    }
                    (None, false, _) => decode(&receipt.result)?,
                    _ => return Err(AcceptanceRefusal::Conflict.into()),
                };
                Ok(AcceptanceOutcome {
                    settlement,
                    replayed: receipt.replayed,
                })
            }
            Err(error) => Err(refusal.map_or(AcceptanceError::Store(error), Into::into)),
        }
    }
}

fn intent_refusal(error: KernelError) -> AcceptanceError {
    match error {
        KernelError::InvalidInput => AcceptanceRefusal::Invalid.into(),
        error => error.into(),
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
        || proposal.database_incarnation_id != run.kernel_incarnation
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

/// What validation proved: the proposal, the project it is judged in, its deduplicated support, and the objects one batch eligibility judgement must still serve, each with what its verdict and state must show.
struct Validated {
    proposal: ReviewProposal,
    domain_id: String,
    project: ProjectScope,
    support: Vec<String>,
    /// The store clock the hold was judged at; the support events are stamped with it.
    now: i64,
    judged: Vec<(EligibilityCandidate, Requirement)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Requirement {
    Descriptor,
    Decision,
    Target { commit_token: i64 },
}

impl Requirement {
    fn met(self, verdict: EligibilityVerdict, state: Option<&ObjectState>) -> bool {
        let decision = |state: &ObjectState| state.object.object_kind == "decision";
        match self {
            Self::Descriptor => {
                matches!(verdict, EligibilityVerdict::Ok | EligibilityVerdict::Hidden)
            }
            Self::Decision => verdict == EligibilityVerdict::Ok && state.is_some_and(decision),
            Self::Target { commit_token } => {
                verdict == EligibilityVerdict::Ok
                    && state.is_some_and(|state| {
                        decision(state) && state.latest_change_commit_seq == Some(commit_token)
                    })
            }
        }
    }
}

/// Validates and applies one acceptance inside its envelope and records the settlement receipt under `settlement`, the prepared form of `settlement_key`; returns the settlement and its JSON for the caller's receipt.
fn apply(
    envelope: &mut Envelope<'_>,
    acceptance: &ProposalAcceptance,
    settlement_key: &CommitIntent,
    settlement: &PreparedIntent,
) -> Result<(Settlement, String), AcceptanceError> {
    let mut validated = validate(envelope, acceptance)?;
    let mut judged = std::mem::take(&mut validated.judged);
    let proposal = &validated.proposal;
    // Every check reads before any write: the target or subject stands, then one eligibility judgement covers the target and every union member.
    match (proposal.action, &proposal.target) {
        (
            ProposalAction::Retain
            | ProposalAction::NoChange
            | ProposalAction::Retire
            | ProposalAction::Revise,
            ProposalTarget::Memory(target),
        ) => target_stands(envelope, target, &mut judged)?,
        (
            ProposalAction::NoChange | ProposalAction::Create,
            ProposalTarget::StagedCandidate { candidate_id },
        ) => subject_stands(envelope, acceptance, candidate_id)?,
        // Staging admits only legal action and target pairs, so a stored row with another pair is corruption.
        _ => return Err(KernelError::Io.into()),
    }
    if !served(envelope, &validated.project, judged)? {
        return changed();
    }
    let effect = match (proposal.action, &proposal.target) {
        (ProposalAction::Retain | ProposalAction::NoChange, _) => SettlementEffect::Unchanged,
        (ProposalAction::Retire, ProposalTarget::Memory(target)) => {
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
        (ProposalAction::Revise, ProposalTarget::Memory(target)) => {
            // The batch judgement proved the target a served decision at the proposal's tokens; its row supplies the identity the successor keeps, so a missing row is corruption.
            let predecessor = envelope
                .object_state(&target.object_id)?
                .ok_or(KernelError::Io)?;
            revise(
                envelope,
                &validated,
                &AcceptedIds::new(settlement_key),
                predecessor,
            )?
        }
        (ProposalAction::Create, _) => create(
            envelope,
            acceptance,
            &validated,
            &AcceptedIds::new(settlement_key),
        )?,
        _ => return Err(KernelError::Io.into()),
    };
    let run = &acceptance.run;
    let recorded = Settlement {
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
    let encoded = serde_json::to_string(&recorded).map_err(|_| KernelError::Io)?;
    envelope.record_reserved_receipt(settlement, &encoded)?;
    Ok((recorded, encoded))
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
    let disclosed = Disclosed {
        ids: disclosed,
        held,
        now,
    };
    if disclosed
        .held
        .iter()
        .any(|evidence| evidence.sensitivity == Sensitivity::Secret)
        || proposal
            .support
            .iter()
            .chain(&proposal.contradictions)
            .any(|cited| disclosed.get(&cited.evidence_id).is_none())
    {
        return changed();
    }
    let project = ProjectScope::new(&run.project_digest).map_err(|_| AcceptanceRefusal::Invalid)?;
    let union = PolicyUnion::decode(&dependencies.union_canonical, &dependencies.union_digest)
        .or_else(|_| changed())?;
    let mut judged = Vec::new();
    for member in union.members() {
        member_stands(
            envelope,
            acceptance,
            &proposal,
            &disclosed,
            member,
            &mut judged,
        )?;
    }
    let mut support: Vec<String> = proposal
        .support
        .iter()
        .map(|cited| cited.evidence_id.clone())
        .collect();
    support.sort_unstable();
    support.dedup();
    Ok(Validated {
        domain_id: row.binding.domain_id,
        proposal: *proposal,
        project,
        support,
        now: disclosed.now,
        judged,
    })
}

/// The object and decision ids an accepted create or revise writes, derived from the settlement key so one proposal names one decision.
struct AcceptedIds {
    object_id: String,
    decision_id: String,
}

impl AcceptedIds {
    fn new(settlement_key: &CommitIntent) -> Self {
        let object_id = format!(
            "memreview-decision-{}",
            identity_digest(settlement_key.operation_key.as_bytes())
        );
        Self {
            decision_id: format!("{object_id}-decision"),
            object_id,
        }
    }
}

/// `ids` is sorted, and each `held` entry is the live review hold's facts for the id at the same index, read at `now`.
struct Disclosed {
    ids: Vec<String>,
    held: Vec<HeldEvidence>,
    now: i64,
}

impl Disclosed {
    fn get(&self, evidence_id: &str) -> Option<&HeldEvidence> {
        self.ids
            .binary_search_by(|id| id.as_str().cmp(evidence_id))
            .ok()
            .map(|index| &self.held[index])
    }
}

fn served(
    envelope: &Envelope<'_>,
    project: &ProjectScope,
    mut judged: Vec<(EligibilityCandidate, Requirement)>,
) -> Result<bool, AcceptanceError> {
    if judged.is_empty() {
        return Ok(true);
    }
    judged.sort_unstable();
    judged.dedup();
    let mut candidates: Vec<EligibilityCandidate> = Vec::with_capacity(judged.len());
    let mut requirements = Vec::with_capacity(judged.len());
    for (candidate, requirement) in judged {
        if candidates.last() != Some(&candidate) {
            candidates.push(candidate);
        }
        requirements.push((candidates.len() - 1, requirement));
    }
    let verdicts = judge_with_states_in_tx(
        envelope.tx,
        envelope.commit_seq - 1,
        project,
        ArtifactDestination::Local,
        &candidates,
    )?;
    Ok(requirements.into_iter().all(|(index, requirement)| {
        let (verdict, state) = &verdicts[index];
        requirement.met(*verdict, state.as_ref())
    }))
}

/// The target's `commit_token` identifies the commit the proposer observed.
fn target_stands(
    envelope: &Envelope<'_>,
    target: &CanonicalTarget,
    judged: &mut Vec<(EligibilityCandidate, Requirement)>,
) -> Result<(), AcceptanceError> {
    if target.known_as_of >= envelope.commit_seq || target.commit_token > target.known_as_of {
        return changed();
    }
    judged.push((
        EligibilityCandidate {
            object_id: target.object_id.clone(),
            source_revision: target.source_revision,
            artifact_digest: None,
        },
        Requirement::Target {
            commit_token: target.commit_token,
        },
    ));
    Ok(())
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

fn member_stands(
    envelope: &Envelope<'_>,
    acceptance: &ProposalAcceptance,
    proposal: &ReviewProposal,
    disclosed: &Disclosed,
    member: &PolicyUnionMember,
    judged: &mut Vec<(EligibilityCandidate, Requirement)>,
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
                && member.revision == acceptance.question_template_revision
        }
        ("native_source", None, None) => descriptor_stands(envelope, member, None, judged)?,
        ("canonical_source", Some(owner), Some(owner_revision)) => {
            descriptor_stands(envelope, member, Some((owner, owner_revision)), judged)?
        }
        ("staged_subject", None, None) => {
            subject_row_stands(envelope, acceptance, &member.id, Some(&member.revision))?;
            true
        }
        ("temporary_capture", None, None) => disclosed.get(&member.id).is_some_and(|held| {
            held.artifact_digest == member.revision
                && held.retention_class == MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS
                && held.retain_until.is_some_and(|until| until > disclosed.now)
        }),
        _ => false,
    };
    if stands { Ok(()) } else { changed() }
}

/// A native or canonical descriptor member is a descriptor whose class agrees with the member kind; a canonical one's detail names `owner` in its class's leading identity field. Both are added to `judged`, and the batch eligibility judgement for a local reader proves the descriptor live at its revision and the owner a served decision at `owner_revision`.
fn descriptor_stands(
    envelope: &Envelope<'_>,
    member: &PolicyUnionMember,
    owner: Option<(&str, &str)>,
    judged: &mut Vec<(EligibilityCandidate, Requirement)>,
) -> Result<bool, AcceptanceError> {
    let Ok(revision) = member.revision.parse::<i64>() else {
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
    let descriptor = (
        EligibilityCandidate {
            object_id: member.id.clone(),
            source_revision: revision,
            artifact_digest: Some(detail.artifact_digest.clone()),
        },
        Requirement::Descriptor,
    );
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
            if !decision_derived(class) || !names_owner {
                return Ok(false);
            }
            judged.push((
                EligibilityCandidate {
                    object_id: owner.to_string(),
                    source_revision: owner_revision,
                    artifact_digest: None,
                },
                Requirement::Decision,
            ));
        }
    }
    judged.push(descriptor);
    Ok(true)
}

/// The descriptor classes whose leading identity field names an originating decision.
const fn decision_derived(class: OccurrenceClass) -> bool {
    matches!(
        class,
        OccurrenceClass::CanonicalClaims | OccurrenceClass::PromotedMemory
    )
}

/// The proposal's text: staging admits create and revise only with text, so a sealed row without it is corruption.
fn proposal_text(proposal: &ReviewProposal) -> Result<String, AcceptanceError> {
    proposal
        .new_text
        .clone()
        .ok_or(AcceptanceError::Store(KernelError::Io))
}

/// A derived object id is unused: a settlement key is unique per proposal, so a row under it is corruption, never a survivor to fold into.
fn fresh(envelope: &Envelope<'_>, object_id: &str) -> Result<(), AcceptanceError> {
    match envelope.object_state(object_id)? {
        None => Ok(()),
        Some(_) => Err(KernelError::Io.into()),
    }
}

/// Inserts the decision an accepted create names, scoped to the caller's scope, which must name the run's project, then admits it.
fn create(
    envelope: &mut Envelope<'_>,
    acceptance: &ProposalAcceptance,
    validated: &Validated,
    ids: &AcceptedIds,
) -> Result<SettlementEffect, AcceptanceError> {
    let scope_id = acceptance
        .scope_id
        .as_deref()
        .ok_or(AcceptanceRefusal::Invalid)?;
    if !validated
        .project
        .names_project(load_scope_terms(envelope.tx, scope_id)?.as_deref())
    {
        return Err(AcceptanceRefusal::Unavailable.into());
    }
    fresh(envelope, &ids.object_id)?;
    envelope
        .insert_decision(DecisionSpec {
            decision_id: ids.decision_id.clone(),
            object_id: ids.object_id.clone(),
            domain_id: validated.domain_id.clone(),
            proposition_id: None,
            scope_id: Some(scope_id.to_string()),
            anchor_id: None,
            evidence_id: None,
            decision_kind: ACCEPTED_DECISION_KIND.to_string(),
            payload: DecisionPayload {
                summary: proposal_text(&validated.proposal)?,
                rationale: String::new(),
            },
            source_kind: ACCEPTED_SOURCE_KIND.to_string(),
            source_id: acceptance.proposal.candidate_id.clone(),
            source_revision: 1,
            sensitivity: Sensitivity::Sensitive,
        })
        .map_err(action_refusal)?;
    let admission = admit(envelope, ids, &validated.support, validated.now)?;
    Ok(SettlementEffect::Created {
        object_id: ids.object_id.clone(),
        admission,
    })
}

/// Writes the successor of `predecessor` with the proposal's text as its summary, then admits it on its own. The successor keeps the predecessor's domain, source identity, kind, proposition, scope, and anchor; its support is the proposal's, cited through decision events, so it carries no rationale or direct evidence of its own. An approval is not revised: model-written text never carries the approval kind.
fn revise(
    envelope: &mut Envelope<'_>,
    validated: &Validated,
    ids: &AcceptedIds,
    predecessor: ObjectState,
) -> Result<SettlementEffect, AcceptanceError> {
    let predecessor_id = predecessor.object.object_id.clone();
    let (proposition_id, scope_id, anchor_id, decision_kind): (
        Option<String>,
        Option<String>,
        Option<String>,
        String,
    ) = envelope
        .tx
        .query_row_cached(
            "SELECT proposition_id,scope_id,anchor_id,decision_kind FROM decisions
             WHERE object_id=?1",
            [predecessor_id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(map_sqlite)?
        .ok_or(KernelError::Io)?;
    if decision_kind == APPROVAL_DECISION_KIND {
        return changed();
    }
    fresh(envelope, &ids.object_id)?;
    let source_revision = predecessor.object.source_revision + 1;
    envelope
        .supersede_decision(
            &predecessor_id,
            DecisionSpec {
                decision_id: ids.decision_id.clone(),
                object_id: ids.object_id.clone(),
                domain_id: predecessor.object.domain_id.clone(),
                proposition_id,
                scope_id,
                anchor_id,
                evidence_id: None,
                decision_kind,
                payload: DecisionPayload {
                    summary: proposal_text(&validated.proposal)?,
                    rationale: String::new(),
                },
                source_kind: predecessor.object.source_kind.clone(),
                source_id: predecessor.object.source_id.clone(),
                source_revision,
                sensitivity: Sensitivity::Sensitive,
            },
        )
        .map_err(action_refusal)?;
    let admission = admit(envelope, ids, &validated.support, validated.now)?;
    Ok(SettlementEffect::Revised {
        object_id: ids.object_id.clone(),
        predecessor_object_id: predecessor_id,
        predecessor_source_revision: predecessor.object.source_revision,
        source_revision,
        predecessor_disposition: "superseded".to_string(),
        admission,
    })
}

/// Cites each supporting evidence row through a decision event, then admits the decision as model inference with no prior authority.
fn admit(
    envelope: &mut Envelope<'_>,
    ids: &AcceptedIds,
    support: &[String],
    now: i64,
) -> Result<AdmittedState, AcceptanceError> {
    envelope
        .append_decision_citations(
            &ids.decision_id,
            SUPPORT_EVENT_KIND,
            SUPPORT_EVENT_SUMMARY,
            support,
            now,
        )
        .map_err(action_refusal)?;
    let decision = envelope
        .record_admission(AdmissionRequest {
            candidate_id: None,
            subject_object_id: Some(ids.object_id.clone()),
            source_class: Some(ACCEPTED_SOURCE_CLASS),
            taint_class: Some(ACCEPTED_TAINT_CLASS),
            event: AdmissionEvent {
                kind: EventKind::Other,
                trigger_object_id: None,
                approval_object_id: None,
                evidence_id: None,
                reason: ADMISSION_REASON.to_string(),
            },
        })
        .map_err(action_refusal)?;
    Ok(AdmittedState::of(&decision))
}

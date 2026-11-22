//! What the worker does for a project whose user tier set `memory_reviewer.enabled`: it gives the bound root MODULE memories authority, keeps the project's claim descriptors published, selects the memories most likely to be wrong and enqueues one review job per decision, and treats each published proposal as `memory_reviewer.apply` says.
//!
//! Selection works on decisions. A decision's `decision_summary` descriptor is the job's subject, so one decision yields one job whatever other forms it has. The job carries the decision's cited message blocks and the source text published after the decision that overlaps it, the contradiction-candidate rows, as starting references, so a run reads the statement, where it came from, and what was said later. A new candidate row changes the job's causal inputs, so a memory is reviewed again when later text overlaps it and passed while nothing new bears on it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};

use kernel::applicability::EvalBudget;
use kernel::source_identity::OccurrenceClass;
use kernel::{
    AdmissionEvent, AdmissionRequest, ArtifactDestination, CitingDescriptor, CommitIntent,
    DecisionRow, DisputeReporter, EligibilityCandidate, EventKind, KernelError, KernelStore,
    LiveDescriptor, MAX_ELIGIBILITY_CANDIDATES, ProjectScope, ProposalAction, ProposalTarget,
    ProviderEgress, SourceClass, Surface, TaintClass,
};
use memory_store::memory_reviewer_jobs::{
    MAX_IDENTITY_BYTES, MAX_MEMORY_REVIEWER_STARTING_REFERENCES,
    MAX_PENDING_MEMORY_REVIEWER_JOBS_PER_PROJECT, MemoryReviewerJobError, MemoryReviewerJobInput,
    MemoryReviewerJobRefusal, ProducerBinding, ReserveOutcome,
};
use memory_store::{MemoryStore, MemoryStoreError};
use sha2::{Digest, Sha256};

use super::broker::{QuestionTemplate, originating_decision};
use super::settlement::{
    AcceptRefusal, ReadRefusal, accept_selected_proposal, read_selected_proposal,
    release_settled_review_hold,
};
use crate::claim_sources::{ClaimMaterializer, ClaimProgress, MaterializationEnd};
use crate::config::ReviewApply;

/// The context store the switch's authority rows are recorded under.
pub const SWITCH_AUTHORITY_STORE: &str = "eidnara-memory-reviewer";
/// The producer every job the switch selects is reserved under.
pub const SELECTION_PRODUCER: &str = "memory-reviewer-switch";
/// The operation key the worker accepts its own proposals under in `apply` mode.
pub const APPLY_OPERATION_KEY: &str = "memory-reviewer-switch";
/// Claim descriptors one selection examines, so a selection's work is bounded whatever the inventory's size.
pub const MAX_WALKED_DESCRIPTORS: usize = 4096;
/// Later overlapping sources a job carries as starting references and causal signals, newest first.
pub const MAX_LATER_SOURCES: usize = 4;
/// Cited message blocks a job carries as starting references.
pub const MAX_CITED_SOURCES: usize = 2;
const _: () =
    assert!(MAX_LATER_SOURCES + MAX_CITED_SOURCES <= MAX_MEMORY_REVIEWER_STARTING_REFERENCES);
/// Candidate rows read per decision.
const CANDIDATE_ROWS_PER_DECISION: usize = 16;
/// The representation of a decision whose descriptor a job reviews.
const SUBJECT_REPRESENTATION: &str = "decision_summary";
/// The longest one selection or one claim catch-up runs.
const CYCLE_BUDGET: Duration = Duration::from_secs(30);
/// Claim slices one catch-up runs before it leaves the rest to the next pass.
const MAX_CLAIM_SLICES: usize = 256;

/// Gives `route_root` MODULE memories authority when it has none, with the root itself as the authority project, so every key derived from the authority project keeps the value an unmanaged root has. A root that already holds the authority is left as it is.
pub fn grant_authority(store: &MemoryStore, route_root: &str) -> Result<(), MemoryStoreError> {
    const DOMAIN: &str = "memories";
    if store
        .module_authority_for_route(route_root, DOMAIN)?
        .is_some()
    {
        return Ok(());
    }
    let begun = store.authority_begin_prepare(SWITCH_AUTHORITY_STORE, route_root, DOMAIN)?;
    // The seed is the store's own rows for this identity, so the expected checksum is the one it computes.
    let checksum = store.authority_seed_checksum(SWITCH_AUTHORITY_STORE, route_root, DOMAIN)?;
    store.authority_verify_prepare(
        SWITCH_AUTHORITY_STORE,
        route_root,
        DOMAIN,
        begun.generation,
        &checksum,
        &checksum,
    )?;
    store.authority_ack_prepare(SWITCH_AUTHORITY_STORE, route_root, DOMAIN, begun.generation)?;
    store.bind_authority_route(SWITCH_AUTHORITY_STORE, route_root, route_root)
}

/// Runs claim-source slices from `progress` until the consumer reaches the Kernel's tip, the budget ends, or a slice is blocked, so every live memory decision has its descriptors before a selection reads them. The host's search lifecycle owner runs the same consumer where it exists; this catch-up serves a deployment without one.
pub fn publish_claims(kernel: &KernelStore, progress: &mut ClaimProgress, now: i64) {
    let budget = EvalBudget::new(Some(Instant::now() + CYCLE_BUDGET), Arc::default());
    for _ in 0..MAX_CLAIM_SLICES {
        let report = ClaimMaterializer::new(kernel, ProviderEgress::LocalOnly).run_slice(
            progress,
            crate::search_lifecycle_owner::CLAIM_SLICE_BOUNDS,
            &budget,
            now,
        );
        match report.map(|report| report.end) {
            Ok(MaterializationEnd::Continues) => {}
            Ok(MaterializationEnd::ReachedTarget | MaterializationEnd::Exhausted) => return,
            Ok(MaterializationEnd::Blocked(blocked)) => {
                eprintln!("daemon: memory_reviewer claim catch-up is blocked: {blocked:?}");
                return;
            }
            Err(error) => {
                eprintln!("daemon: memory_reviewer claim catch-up failed: {error}");
                return;
            }
        }
    }
}

/// How likely a memory is to be wrong, most likely first: selection orders its jobs by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Suspicion {
    /// Source text published after the memory overlaps it.
    LaterOverlap,
    /// An observation already contests the statement.
    Contested,
    /// Work in progress the user stated, which later sessions may have finished.
    CurrentWork,
    /// A statement automatic delivery serves.
    Delivered,
    /// A row only explicit search serves.
    SearchOnly,
}

/// One project's selection scope: the Kernel scope its decisions are judged in and the Memory Store project its jobs live under.
pub struct SelectionScope<'a> {
    pub project: &'a ProjectScope,
    pub project_digest: &'a str,
    pub ledger_project: &'a str,
}

/// What one selection did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Selected {
    /// Decisions examined.
    pub examined: usize,
    /// Jobs enqueued, by suspicion class in [`Suspicion`] order.
    pub enqueued: [usize; 5],
}

impl Selected {
    pub fn total(&self) -> usize {
        self.enqueued.iter().sum()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CycleError {
    #[error("kernel {0}")]
    Kernel(KernelError),
    #[error("ledger {0}")]
    Ledger(String),
}

struct Candidate<'a> {
    descriptor: &'a LiveDescriptor,
    decision: &'a DecisionRow,
    suspicion: Suspicion,
    /// The newest later source's publish tip, or the decision's own commit, so the newest evidence is reviewed first inside a class.
    recency: i64,
    cited: Vec<String>,
    later: Vec<String>,
}

/// Selects the project's memories without a job at their causal inputs, most suspicious first, and enqueues at most `room` jobs, each Ready with its starting references.
pub fn select_and_enqueue(
    kernel: &KernelStore,
    ledger: &MemoryStore,
    scope: &SelectionScope<'_>,
    room: usize,
    now: i64,
) -> Result<Selected, CycleError> {
    let mut selected = Selected::default();
    if room == 0 {
        return Ok(selected);
    }
    let budget = EvalBudget::new(Some(Instant::now() + CYCLE_BUDGET), Arc::default());
    let tip = kernel
        .tip_within_budget(&budget)
        .map_err(CycleError::Kernel)?;
    let descriptors = subject_descriptors(kernel, scope.project, tip, &budget)?;
    let ids: Vec<String> = descriptors
        .iter()
        .filter_map(|row| decision_of(row).map(str::to_string))
        .collect();
    let decisions: HashMap<String, DecisionRow> = kernel
        .decisions_for_objects_as_of_within_budget(&ids, tip, &budget)
        .map_err(CycleError::Kernel)?
        .into_iter()
        .map(|row| (row.object_id.clone(), row))
        .collect();
    let delivered = delivered(kernel, scope.project, &decisions, &budget)?;
    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let linked = kernel
        .linked_decisions_evidence_within_budget(
            &id_refs,
            tip,
            super::selection::MAX_LINKED_EVIDENCE,
            &budget,
        )
        .map_err(CycleError::Kernel)?;
    let linked: HashMap<&str, &Vec<kernel::LinkedEvidence>> =
        id_refs.iter().copied().zip(&linked).collect();
    // A memory is reviewed under the memory template, whose revision is part of every job's causal inputs, so an edited template reviews each memory once more.
    let mut policy_versions = super::handoff::review_policy_versions();
    policy_versions.insert(
        "question_template".to_string(),
        QuestionTemplate::MemoryStatement.revision(),
    );
    let mut candidates = Vec::new();
    for descriptor in &descriptors {
        let Some(decision) = decision_of(descriptor).and_then(|id| decisions.get(id)) else {
            continue;
        };
        selected.examined += 1;
        let cited: Vec<String> = linked
            .get(decision.object_id.as_str())
            .into_iter()
            .flat_map(|rows| rows.iter())
            .filter(|row| row.live)
            .map(|row| row.evidence_id.clone())
            .collect();
        let rows = ledger
            .candidates_for_decision(
                scope.project_digest,
                &decision.object_id,
                None,
                CANDIDATE_ROWS_PER_DECISION,
            )
            .map_err(|error| CycleError::Ledger(error.to_string()))?
            .items;
        // A later source is one published after the decision's own commit that the decision does not cite: the statement's own message is its origin, and text recorded afterwards is what may have changed it.
        let mut later: Vec<(i64, String)> = rows
            .into_iter()
            .filter(|row| {
                row.publish_tip > decision.created_commit_seq
                    && row.evidence_id.len() <= MAX_IDENTITY_BYTES
                    && !cited.contains(&row.evidence_id)
            })
            .map(|row| (row.publish_tip, row.evidence_id))
            .collect();
        later.sort_by(|a, b| b.cmp(a));
        later.dedup_by(|a, b| a.1 == b.1);
        later.truncate(MAX_LATER_SOURCES);
        let recency = later
            .first()
            .map_or(decision.created_commit_seq, |(tip, _)| *tip);
        let suspicion = if !later.is_empty() {
            Suspicion::LaterOverlap
        } else if crate::memory_capture::is_contested(decision) {
            Suspicion::Contested
        } else if crate::memory_capture::states_current_work(decision) {
            Suspicion::CurrentWork
        } else if delivered.contains(decision.object_id.as_str()) {
            Suspicion::Delivered
        } else {
            Suspicion::SearchOnly
        };
        candidates.push(Candidate {
            descriptor,
            decision,
            suspicion,
            recency,
            cited,
            later: later.into_iter().map(|(_, evidence)| evidence).collect(),
        });
    }
    candidates.sort_by(|a, b| {
        (
            a.suspicion,
            std::cmp::Reverse(a.recency),
            &a.decision.object_id,
        )
            .cmp(&(
                b.suspicion,
                std::cmp::Reverse(b.recency),
                &b.decision.object_id,
            ))
    });
    for candidate in candidates {
        if selected.total() >= room {
            break;
        }
        let linked_rows = linked
            .get(candidate.decision.object_id.as_str())
            .map_or(&[][..], |rows| rows.as_slice());
        let Some((mut inputs, _)) =
            super::selection::causal_inputs(candidate.descriptor, linked_rows, &policy_versions)
        else {
            continue;
        };
        let mut signals = candidate.later.clone();
        signals.sort();
        inputs.signals = signals;
        inputs.question_template = QuestionTemplate::MemoryStatement.id().to_string();
        let Ok(causal_identity) = inputs.causal_identity() else {
            continue;
        };
        let exists = ledger
            .memory_reviewer_jobs_exist(scope.ledger_project, &[causal_identity.as_str()])
            .map_err(|error| CycleError::Ledger(error.to_string()))?;
        if exists.first().copied().unwrap_or(true) {
            continue;
        }
        let mut starting = Vec::new();
        for evidence in candidate
            .cited
            .iter()
            .take(MAX_CITED_SOURCES)
            .chain(&candidate.later)
        {
            if let Some(object_id) = source_descriptor(kernel, evidence, tip, &budget)?
                && !starting.contains(&object_id)
            {
                starting.push(object_id);
            }
        }
        let producer = ProducerBinding {
            producer: SELECTION_PRODUCER.to_string(),
            firing_id: format!("selection@{now}"),
            ordinal: u64::try_from(selected.total()).unwrap_or(u64::MAX),
        };
        let reserved =
            match ledger.reserve_memory_reviewer_job(scope.ledger_project, &producer, &inputs, now)
            {
                Ok(ReserveOutcome::Reserved(job)) => job,
                Ok(ReserveOutcome::Existing(_)) => continue,
                // Capacity and quota refusals hold for every later candidate of this selection.
                Err(MemoryReviewerJobError::Refused(
                    MemoryReviewerJobRefusal::ProjectCapacity
                    | MemoryReviewerJobRefusal::HostCapacity
                    | MemoryReviewerJobRefusal::MetadataQuota,
                )) => break,
                Err(MemoryReviewerJobError::Refused(_)) => continue,
                Err(error) => return Err(CycleError::Ledger(error.to_string())),
            };
        let input = MemoryReviewerJobInput {
            subject: inputs.target.clone(),
            starting_references: starting,
            question_template: QuestionTemplate::MemoryStatement.id().to_string(),
        };
        match ledger.activate_memory_reviewer_job(
            scope.ledger_project,
            &reserved.causal_identity,
            &producer,
            &input,
            now,
        ) {
            Ok(_) => selected.enqueued[candidate.suspicion as usize] += 1,
            Err(MemoryReviewerJobError::Refused(refusal)) => eprintln!(
                "daemon: memory_reviewer selection left a reserved job unactivated: {refusal}"
            ),
            Err(error) => return Err(CycleError::Ledger(error.to_string())),
        }
    }
    Ok(selected)
}

/// The decision a claim descriptor names.
fn decision_of(descriptor: &LiveDescriptor) -> Option<&str> {
    originating_decision(
        OccurrenceClass::CanonicalClaims,
        &descriptor.detail.identity,
    )
}

/// The live `decision_summary` descriptors of the project's decisions that the configured model may read, at most [`MAX_WALKED_DESCRIPTORS`] examined.
fn subject_descriptors(
    kernel: &KernelStore,
    project: &ProjectScope,
    tip: i64,
    budget: &EvalBudget,
) -> Result<Vec<LiveDescriptor>, CycleError> {
    let page_rows = NonZeroUsize::new(256).expect("nonzero");
    let mut rows = Vec::new();
    let mut after: Option<String> = None;
    let mut examined = 0;
    while examined < MAX_WALKED_DESCRIPTORS {
        let page = kernel
            .live_source_descriptors(
                OccurrenceClass::CanonicalClaims,
                tip,
                after.as_deref(),
                page_rows,
                budget,
            )
            .map_err(CycleError::Kernel)?;
        examined += page.rows.len();
        rows.extend(
            page.rows
                .into_iter()
                .filter(|row| row.detail.representation == SUBJECT_REPRESENTATION),
        );
        match page.next {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    let mut permitted = Vec::with_capacity(rows.len());
    for chunk in rows.chunks(MAX_ELIGIBILITY_CANDIDATES) {
        let candidates: Vec<EligibilityCandidate> = chunk
            .iter()
            .map(|row| EligibilityCandidate {
                object_id: row.object_id.clone(),
                source_revision: row.detail.revision.parse().unwrap_or(-1),
                artifact_digest: Some(row.detail.artifact_digest.clone()),
            })
            .collect();
        let verdicts = kernel
            .judge_surface_eligibility_within_budget(
                project,
                ArtifactDestination::ConfiguredModel,
                Surface::ExplicitSearch,
                &candidates,
                budget,
            )
            .map_err(CycleError::Kernel)?
            .verdicts;
        permitted.extend(verdicts.iter().map(|verdict| verdict.permits()));
    }
    let mut permitted = permitted.into_iter();
    rows.retain(|_| permitted.next().unwrap_or(false));
    Ok(rows)
}

/// The decisions automatic delivery serves.
fn delivered<'a>(
    kernel: &KernelStore,
    project: &ProjectScope,
    decisions: &'a HashMap<String, DecisionRow>,
    budget: &EvalBudget,
) -> Result<BTreeSet<&'a str>, CycleError> {
    let mut served = BTreeSet::new();
    let rows: Vec<&'a DecisionRow> = decisions.values().collect();
    let (_, states) = kernel
        .object_states(
            &rows
                .iter()
                .map(|row| row.object_id.clone())
                .collect::<Vec<_>>(),
        )
        .map_err(CycleError::Kernel)?;
    let judged: Vec<(&'a DecisionRow, EligibilityCandidate)> = rows
        .into_iter()
        .zip(states)
        .filter_map(|(row, state)| {
            Some((
                row,
                EligibilityCandidate {
                    object_id: row.object_id.clone(),
                    source_revision: state?.object.source_revision,
                    artifact_digest: None,
                },
            ))
        })
        .collect();
    for chunk in judged.chunks(MAX_ELIGIBILITY_CANDIDATES) {
        let candidates: Vec<EligibilityCandidate> = chunk
            .iter()
            .map(|(_, candidate)| candidate.clone())
            .collect();
        let verdicts = kernel
            .judge_surface_eligibility_within_budget(
                project,
                ArtifactDestination::Local,
                Surface::AutoInject,
                &candidates,
                budget,
            )
            .map_err(CycleError::Kernel)?
            .verdicts;
        served.extend(
            chunk
                .iter()
                .zip(verdicts)
                .filter(|(_, verdict)| verdict.permits())
                .map(|((row, _), _)| row.object_id.as_str()),
        );
    }
    Ok(served)
}

/// The object id of a live source descriptor citing `evidence_id`: a message block first, then a repository document, then a tool span.
fn source_descriptor(
    kernel: &KernelStore,
    evidence_id: &str,
    tip: i64,
    budget: &EvalBudget,
) -> Result<Option<String>, CycleError> {
    for class in [
        OccurrenceClass::Messages,
        OccurrenceClass::RepositoryDocuments,
        OccurrenceClass::RawToolSpans,
    ] {
        match kernel.source_descriptor_citing_evidence(class, evidence_id, tip, budget) {
            Ok(CitingDescriptor::Live(descriptor)) => return Ok(Some(descriptor.object_id)),
            Ok(CitingDescriptor::Retired | CitingDescriptor::Uncited) => {}
            // A row of another class under the evidence fails this class's row check; the next class reads it.
            Err(KernelError::CorruptCanonicalRow) => {}
            Err(error) => return Err(CycleError::Kernel(error)),
        }
    }
    Ok(None)
}

/// What the worker did with one published proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// The mode writes nothing for this proposal.
    Left,
    /// `contest`: the target took a `mark_disputed` disposition and now reads `disposition`.
    Disputed { disposition: String },
    /// `apply`: the Kernel settled the proposal.
    Accepted(Box<kernel::Settlement>),
    /// The proposal could not be read back or the Kernel refused the write; the proposal stays readable for `review.accept`.
    Refused(String),
}

impl Applied {
    /// The outcome's name for the worker's log: a closed word, never proposal text.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Disputed { .. } => "disputed",
            Self::Accepted(settlement) => match settlement.effect {
                kernel::SettlementEffect::Created { .. } => "created",
                kernel::SettlementEffect::Revised { .. } => "revised",
                kernel::SettlementEffect::Retired { .. } => "retired",
                kernel::SettlementEffect::Unchanged => "unchanged",
            },
            Self::Refused(_) => "refused",
        }
    }
}

/// The live store pair and project a proposal is applied under.
pub struct ApplyScope<'a> {
    pub ledger_project: &'a str,
    pub project_digest: &'a str,
    pub kernel_incarnation: &'a str,
    pub memstore_incarnation: &'a str,
}

/// Treats the proposal the job's completed receipt selects as `mode` says. A `retain` or `no_change` proposal asks for no write, so every mode accepts it at once: the acceptance leaves the memory as it is and ends the proposal's review hold, which keeps the project's hold allowance for proposals that wait on a decision. For a proposal that would change a memory, `propose` writes nothing and leaves it for `review.accept`. `contest` disputes the target of a `retire` or `revise` that cites a contradiction, citing the first contradicting evidence row; the statement leaves automatic delivery and stays on explicit search as a labeled row. `apply` accepts the proposal through the Kernel acceptance `review.accept` runs.
pub fn apply_proposal(
    kernel: &KernelStore,
    ledger: &MemoryStore,
    scope: &ApplyScope<'_>,
    causal_identity: &str,
    mode: ReviewApply,
    now: i64,
) -> Applied {
    let selected =
        match read_selected_proposal(kernel, ledger, scope.ledger_project, causal_identity, now) {
            Ok(selected) => selected,
            Err(refusal) => return Applied::Refused(refusal.to_string()),
        };
    let proposal = &selected.proposal;
    let keeps = matches!(
        proposal.action,
        ProposalAction::Retain | ProposalAction::NoChange
    );
    match mode {
        ReviewApply::Propose if !keeps => Applied::Left,
        ReviewApply::Contest if !keeps => {
            let (ProposalTarget::Memory(target), Some(contradiction)) =
                (&proposal.target, proposal.contradictions.first())
            else {
                return Applied::Left;
            };
            if !matches!(
                proposal.action,
                ProposalAction::Retire | ProposalAction::Revise
            ) {
                return Applied::Left;
            }
            dispute(
                kernel,
                scope,
                causal_identity,
                &target.object_id,
                &contradiction.evidence_id,
            )
        }
        ReviewApply::Propose | ReviewApply::Contest | ReviewApply::Apply => {
            let receipt = match ledger
                .lookup_memory_reviewer_receipt(scope.ledger_project, causal_identity)
            {
                Ok(receipt) => receipt,
                Err(error) => return Applied::Refused(error.to_string()),
            };
            match accept_selected_proposal(
                kernel,
                receipt.clone(),
                (scope.kernel_incarnation, scope.memstore_incarnation),
                causal_identity,
                APPLY_OPERATION_KEY,
                &proposal.target,
            ) {
                Ok(outcome) => {
                    // The settlement is the decision the review hold waited for, so the hold ends with it.
                    if let Some(receipt) = receipt
                        && let Err(error) = release_settled_review_hold(
                            kernel,
                            receipt,
                            scope.memstore_incarnation,
                            causal_identity,
                            now,
                        )
                    {
                        eprintln!(
                            "daemon: memory_reviewer kept the review hold of a settled proposal: {error}"
                        );
                    }
                    Applied::Accepted(Box::new(outcome.settlement))
                }
                Err(AcceptRefusal::Receipt(refusal)) => Applied::Refused(refusal.to_string()),
                Err(AcceptRefusal::Kernel(refusal)) => Applied::Refused(format!("{refusal:?}")),
            }
        }
    }
}

/// Records `mark_disputed` on `object_id`, citing `evidence_id`, under an operation key the job names, so a retried pass replays the first commit.
fn dispute(
    kernel: &KernelStore,
    scope: &ApplyScope<'_>,
    causal_identity: &str,
    object_id: &str,
    evidence_id: &str,
) -> Applied {
    let request = format!("{object_id}\u{1f}{evidence_id}");
    let intent = CommitIntent {
        producer: "eidnara-daemon/memory-reviewer-contest".to_string(),
        operation_key: format!("{}#{causal_identity}", scope.project_digest),
        request_digest: format!("{:x}", Sha256::digest(request.as_bytes())),
        actor: "memory-reviewer".to_string(),
        cause: "a review proposal cites a contradiction".to_string(),
    };
    let reporter = DisputeReporter {
        source_class: SourceClass::ModelInference,
        taint_class: TaintClass::MemoryClassifierInference,
        event_payload: None,
        project_digest: scope.project_digest.to_string(),
        harness: "memory_reviewer".to_string(),
        session_id: causal_identity.to_string(),
    };
    let committed = kernel.commit(intent, |envelope| {
        let (prior, _) = envelope
            .subject_admission(object_id)?
            .ok_or(KernelError::AdmissionPolicy)?;
        let decision = envelope.record_dispute_admission(
            AdmissionRequest {
                candidate_id: None,
                subject_object_id: Some(object_id.to_string()),
                source_class: Some(prior.source_class),
                taint_class: Some(prior.taint_class),
                event: AdmissionEvent {
                    kind: EventKind::MarkDisputed,
                    trigger_object_id: None,
                    approval_object_id: None,
                    evidence_id: Some(evidence_id.to_string()),
                    reason: "a memory review cited contradicting evidence".to_string(),
                },
            },
            &reporter,
        )?;
        Ok(decision.disposition.as_str().to_string())
    });
    match committed {
        Ok(receipt) => Applied::Disputed {
            disposition: receipt.result,
        },
        Err(error) => Applied::Refused(error.to_string()),
    }
}

/// Reads a refusal as text for the worker's log.
impl From<ReadRefusal> for Applied {
    fn from(refusal: ReadRefusal) -> Self {
        Self::Refused(refusal.to_string())
    }
}

/// The per-class counts of one selection, for the worker's log line.
pub fn describe(selected: &Selected) -> BTreeMap<&'static str, usize> {
    [
        "later_overlap",
        "contested",
        "current_work",
        "delivered",
        "search_only",
    ]
    .into_iter()
    .zip(selected.enqueued)
    .collect()
}

/// Pending capacity the project has left for new jobs.
pub fn room(pending: usize, per_pass: usize) -> usize {
    MAX_PENDING_MEMORY_REVIEWER_JOBS_PER_PROJECT
        .saturating_sub(pending)
        .min(per_pass.saturating_sub(pending))
}

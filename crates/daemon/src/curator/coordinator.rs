//! One bounded investigation: the daemon's loop that renders the subject, sends one supervised request per round, executes the model's read batch through the broker, and settles the run through its completed receipt.
//!
//! The coordinator owns orchestration only. Evidence reads, egress, and aliases are the broker's; the connection, marker, and handoff are the disclosure's; the receipt and holds are the settlement's; live execution and physical completion are the Model Execution supervisor's. Capacity is one active investigation per project and four per host, acquired before anything is spawned and refused rather than queued. A job runs at most four rounds and four physical requests, admits no read or model work at its execution cutoff, and keeps its original run deadline whatever happens inside it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use host_runtime::model_execution::backend::{
    BackendError, BackendEvent, BackendFuture, BackendTerminal, ErrorClass, EventSink, FinishReason,
};
use host_runtime::model_execution::supervisor::{InternalOutcome, InternalRunKey, Supervisor};
use kernel::applicability::EvalBudget;
use kernel::source_identity::OccurrenceClass;
use kernel::{
    CuratorHoldBinding, CuratorHoldError, CuratorHoldRefusal, EvidenceReference, KernelStore,
    ManifestReference, PolicyDependencies, ProposalTarget, ReviewBinding, ReviewProposal,
    ReviewQuestionTemplate, ReviewStagedReference, SourceDescriptorDetail, SourceSpan,
};
use memory_store::MemoryStore;
use memory_store::curator_jobs::{CuratorJob, CuratorJobInput, ReviewTarget};
use memory_store::curator_ledger::{CURATOR_ATTEMPT_MAX_MS, CuratorLedgerRefusal, CuratorReceipt};
use sha2::{Digest, Sha256};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::broker::{
    Alias, EvidenceBroker, QuestionTemplate, ReferenceExpectation, Refusal, RefusalCode,
    RenderedBuffer, RunBinding, refuse,
};
use super::disclosure::{
    AttemptBinding, Disclosed, Disclosure, DisclosureApproval, DisclosureRefusal, ModelProfile,
    prepare_body,
};
use super::project_text::{ProjectText, SearchQuery};
use super::related_memories::RelatedMemoryDiscovery;
use super::settlement::{RunResult, Settled, Settlement, SettlementError, TaskClaim};
use super::steps::{Operation, ProposedOutcome, SearchBy, Step};

pub const MAX_ROUNDS: usize = 4;
/// Active investigations per host; per project the bound is one.
pub const MAX_ACTIVE_PER_HOST: usize = 4;
/// The supervisor key's task kind for Curator attempts.
pub const CURATOR_TASK_KIND: &str = "curator_review";

/// Host text sent with every request: the step schema the model must answer in. Content-free and fixed; it never carries source text.
const STEP_INSTRUCTIONS: &str = "Answer with exactly one JSON object {\"v\":1,\"step\":{...}} and nothing else. step.kind is one of: \"read_batch\" with \"operations\" (at most 8) where each operation is {\"op\":\"read_reference\",\"alias\":\"ref-N\",\"range\":{\"start\":0,\"end\":N}?}, {\"op\":\"find_related\",\"cursor\":\"...\"?}, {\"op\":\"search_project\",\"by\":\"path\"|\"name\"|\"content\",\"literal\":\"...\"}, or {\"op\":\"read_project\",\"path\":\"relative/path\",\"range\":{...}?}; \"propose\" with \"action\" (create|revise|retain|retire|no_change), \"new_text\"?, \"support\" and \"contradictions\" as lists of {\"alias\":\"ref-N\",\"range\":{...}?}, \"limitations\" as a list of strings, and \"uncertainty\" (low|medium|high); or \"abstain\" with \"reason\". Cite only aliases you were given. Zero search results never prove absence.";

/// Active-investigation capacity: one per project and [`MAX_ACTIVE_PER_HOST`] per host. Acquisition never waits; a full slot refuses.
#[derive(Debug, Default)]
pub struct InvestigationPermits {
    active: Mutex<BTreeSet<String>>,
}

/// One held slot; dropping it releases the slot.
pub struct InvestigationPermit {
    permits: Arc<InvestigationPermits>,
    project: String,
}

impl Drop for InvestigationPermit {
    fn drop(&mut self) {
        if let Ok(mut active) = self.permits.active.lock() {
            active.remove(&self.project);
        }
    }
}

impl InvestigationPermits {
    pub fn try_acquire(self: &Arc<Self>, project: &str) -> Option<InvestigationPermit> {
        let mut active = self.active.lock().ok()?;
        if active.len() >= MAX_ACTIVE_PER_HOST || active.contains(project) {
            return None;
        }
        active.insert(project.to_string());
        Some(InvestigationPermit {
            permits: Arc::clone(self),
            project: project.to_string(),
        })
    }

    pub fn active(&self) -> usize {
        self.active.lock().map(|active| active.len()).unwrap_or(0)
    }
}

/// Everything one investigation borrows from its composition: the stores, the supervisor, the sender and its approval, the model profile, the capacity table, and the clock.
pub struct Coordinator {
    pub store: Arc<KernelStore>,
    pub ledger: Arc<MemoryStore>,
    pub supervisor: Arc<Supervisor>,
    pub sender: Arc<super::model_request::Sender>,
    pub approval: Option<DisclosureApproval>,
    pub profile: ModelProfile,
    pub credential_id: String,
    pub permits: Arc<InvestigationPermits>,
    pub now_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
    /// Issued inspections per job; production passes [`super::broker::MAX_ISSUED_INSPECTIONS`], tests lower it (Q23).
    pub inspection_limit: usize,
}

/// One claimed job with its begun receipt and the trusted bindings the daemon resolved for it.
pub struct JobContext<'a> {
    pub job: &'a CuratorJob,
    pub input: &'a CuratorJobInput,
    pub receipt: &'a CuratorReceipt,
    pub claim: &'a TaskClaim,
    pub binding: &'a ReviewBinding,
    pub question: QuestionTemplate,
    /// The confined project directory this run may inspect; `None` refuses project-text operations as unavailable.
    pub project_root: Option<&'a mut ProjectText>,
}

/// Why an investigation ended without a settlement. The receipt stays in progress for its lifecycle owner.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvestigationError {
    /// No slot for this project or host; nothing was spawned.
    #[error("capacity")]
    Capacity,
    #[error("cancelled")]
    Cancelled,
    /// No startup approval covers the sender; nothing was sent.
    #[error("unavailable")]
    Unavailable,
    /// The receipt is not this run's to settle.
    #[error("fenced")]
    Fenced,
    /// The subject cannot be resolved for a remote run; the caller settles the run on this refusal.
    #[error("refused {0}")]
    Refused(RefusalCode),
    #[error("supervisor {0}")]
    Supervisor(String),
    #[error("store {0}")]
    Store(String),
    #[error("kernel {0}")]
    Kernel(RefusalCode),
    #[error(transparent)]
    Settlement(#[from] SettlementError),
}

/// What one supervised attempt yielded.
enum Attempt {
    Text(String),
    /// The request failed or was refused after being charged; the round is spent.
    Spent,
    /// The ledger, the cutoff, or the byte budget admits no further attempt.
    Exhausted,
    /// A disclosed input no longer passes the Kernel; the run settles on that refusal.
    Refused(RefusalCode),
}

impl Coordinator {
    /// Runs one investigation to its settlement. `cancel` stops admission of new rounds, signals the live attempt, and joins it before the permit is released.
    pub async fn investigate(
        &self,
        context: JobContext<'_>,
        cancel: &CancellationToken,
    ) -> Result<Settled, InvestigationError> {
        let project = &context.job.project;
        let _permit = self
            .permits
            .try_acquire(project)
            .ok_or(InvestigationError::Capacity)?;
        let memstore_incarnation = self
            .ledger
            .curator_store_incarnation()
            .map_err(|_| InvestigationError::Kernel(RefusalCode::Store))?;
        // The hold is scoped by the Kernel project digest the binding carries; the ledger project is the authority key the job row lives under and need not be a digest.
        let hold_binding = CuratorHoldBinding {
            project_digest: context.binding.project_digest.clone(),
            kernel_incarnation: context.receipt.kernel_incarnation_id.clone(),
            memstore_incarnation,
            subject: context.job.causal_identity.clone(),
            generation: context.receipt.generation,
        };
        // A subject or linked reference the run cannot resolve for a remote model settles the run on that refusal; nothing is sent.
        let mut protected = Vec::new();
        let mut starting = Vec::with_capacity(context.input.starting_references.len());
        let subject = match resolve_subject(&self.store, &context.input.subject, context.binding)
            .and_then(|(subject, evidence)| {
                protected.extend(evidence);
                // Starting references are descriptor object ids the producer linked; each becomes an alias the model may read, never rendered unasked.
                for object_id in &context.input.starting_references {
                    let (expectation, evidence) = resolve_descriptor(&self.store, object_id, None)?;
                    protected.extend(evidence);
                    starting.push(expectation);
                }
                Ok(subject)
            }) {
            Ok(subject) => Ok(subject),
            Err(InvestigationError::Refused(code)) => Err(code),
            Err(error) => return Err(error),
        };
        protected.sort();
        protected.dedup();
        let hold = self
            .store
            .acquire_execution_hold(
                &hold_binding,
                &protected,
                context.receipt.execution_cutoff_ms,
            )
            .map_err(|error| InvestigationError::Kernel(super::broker::hold_refusal(error)))?;
        let broker = EvidenceBroker::new(
            RunBinding {
                project: kernel::ProjectScope::new(&context.binding.project_digest)
                    .map_err(|_| InvestigationError::Kernel(RefusalCode::Scope))?,
                hold: hold_binding.clone(),
                hold_id: hold.hold_id.clone(),
                destination: kernel::ArtifactDestination::Remote,
            },
            context.question,
        )
        .with_inspection_limit(self.inspection_limit);
        // The cutoff as a monotonic instant: the ledger's absolute millisecond mapped through the same clock the run reads.
        let remaining = context
            .receipt
            .execution_cutoff_ms
            .saturating_sub((self.now_ms)());
        let cutoff = Instant::now() + Duration::from_millis(u64::try_from(remaining).unwrap_or(0));
        let hold_id = hold.hold_id;
        let mut run = Run {
            coordinator: self,
            context,
            hold_binding,
            broker: Arc::new(tokio::sync::Mutex::new(broker)),
            transcript: Vec::new(),
            sent: 0,
            disclosed_spans: BTreeMap::new(),
            discovery: RelatedMemoryDiscovery::new(""),
            cutoff,
        };
        let outcome = run.investigate(subject, starting, cancel).await;
        // A run that ends without settling (cancelled, fenced, a store or supervisor failure) owns no result; the execution hold it acquired is released here rather than left to its cutoff, so a job retried each pass never accumulates holds toward the project's limit. Settlement moved or released the hold on every other path.
        if outcome.is_err() {
            match self
                .store
                .release_execution_hold(&hold_id, &run.hold_binding)
            {
                Ok(())
                | Err(CuratorHoldError::Refused(
                    CuratorHoldRefusal::Released | CuratorHoldRefusal::Expired,
                )) => {}
                Err(error) => eprintln!(
                    "daemon: curator run could not release its execution hold for {}/{} generation {}: {error}",
                    run.context.job.project, run.hold_binding.subject, run.hold_binding.generation
                ),
            }
        }
        outcome
    }
}

/// One investigation's state: the broker every disclosure goes through, the transcript resent each round, the related-memory cursors, and the cutoff every wait is bounded by.
struct Run<'a> {
    coordinator: &'a Coordinator,
    context: JobContext<'a>,
    hold_binding: CuratorHoldBinding,
    /// The broker is shared with the launch future, which must be `'static`; the coordinator is idle while an attempt runs, so the lock is never contended.
    broker: Arc<tokio::sync::Mutex<EvidenceBroker>>,
    transcript: Vec<RenderedBuffer>,
    /// Transcript buffers below this index have been sent at least once; Q22 charges them again on every later send.
    sent: usize,
    /// Byte ranges of each alias the model has seen, so a citation can only name bytes that were disclosed.
    disclosed_spans: BTreeMap<String, Vec<std::ops::Range<u64>>>,
    discovery: RelatedMemoryDiscovery,
    cutoff: Instant,
}

impl Run<'_> {
    /// The task lease is shorter than a run: one attempt plus the settlement reserve. A live worker renews before each attempt; a lease that will not renew is another worker's fence, and the run ends without settling.
    fn renew_claim(&self) -> Result<(), InvestigationError> {
        let claim = self.context.claim;
        match self
            .coordinator
            .ledger
            .renew_curator_task(
                &self.context.job.project,
                &claim.claim_id,
                &claim.worker_instance,
                claim.slot,
                i64::try_from(self.context.receipt.authority_generation).unwrap_or(i64::MAX),
                (self.coordinator.now_ms)(),
            )
            .map_err(|error| InvestigationError::Store(error.to_string()))?
        {
            memory_store::NoteEvalRenewOutcome::Renewed { .. } => Ok(()),
            _ => Err(InvestigationError::Fenced),
        }
    }

    async fn investigate(
        &mut self,
        subject: Result<ReferenceExpectation, RefusalCode>,
        starting: Vec<ReferenceExpectation>,
        cancel: &CancellationToken,
    ) -> Result<Settled, InvestigationError> {
        let now = (self.coordinator.now_ms)();
        // The subject is the first disclosure; a subject the policy refuses is the abstention AE1 names, settled without any request. At the cutoff not even that read is admitted.
        {
            let broker = Arc::clone(&self.broker);
            let mut broker = broker.lock().await;
            if now >= self.context.receipt.execution_cutoff_ms {
                return self.settle(&broker, RunResult::Exhausted);
            }
            let subject = match subject {
                Ok(subject) => subject,
                Err(code) => return self.settle(&broker, RunResult::Refused(code)),
            };
            let alias = broker.aliases.issue(subject);
            match broker.read(&self.coordinator.store, alias.as_str(), None, now) {
                Ok(read) => {
                    self.discovery = RelatedMemoryDiscovery::new(
                        std::str::from_utf8(read.buffer.bytes()).unwrap_or(""),
                    );
                    self.record_span(&alias, 0..read.buffer.bytes().len() as u64);
                    self.push_labeled(&broker, read.buffer);
                }
                Err(refusal) => return self.settle(&broker, RunResult::Refused(refusal.code)),
            }
            // The opening disclosure is the host's, not a batch the model issued.
            broker.accounting.end_batch();
            let issued: Vec<String> = starting
                .into_iter()
                .map(|expectation| broker.aliases.issue(expectation).as_str().to_string())
                .collect();
            let notice = if issued.is_empty() {
                "no linked references".to_string()
            } else {
                format!("linked references: {}", issued.join(", "))
            };
            let notice = broker
                .render_host_text(&notice)
                .map_err(|refusal| InvestigationError::Kernel(refusal.code))?;
            self.transcript.push(notice);
        }
        for round in 0..MAX_ROUNDS {
            if cancel.is_cancelled() {
                return Err(InvestigationError::Cancelled);
            }
            if (self.coordinator.now_ms)() >= self.context.receipt.execution_cutoff_ms {
                return self.settle_now(RunResult::Exhausted).await;
            }
            self.renew_claim()?;
            let attempt_index = u32::try_from(round).unwrap_or(u32::MAX);
            let text = match self.attempt(attempt_index, cancel).await? {
                Attempt::Text(text) => text,
                Attempt::Spent => continue,
                Attempt::Exhausted => return self.settle_now(RunResult::Exhausted).await,
                Attempt::Refused(code) => return self.settle_now(RunResult::Refused(code)).await,
            };
            let broker = Arc::clone(&self.broker);
            let mut broker = broker.lock().await;
            let targets_memory = matches!(self.context.input.subject, ReviewTarget::Memory { .. });
            let step = match Step::parse(&text, &broker, targets_memory) {
                Ok(step) => step,
                Err(code) => {
                    push_notice(&broker, &mut self.transcript, None, code);
                    continue;
                }
            };
            match step {
                Step::ReadBatch { operations } => {
                    for operation in operations {
                        if cancel.is_cancelled() {
                            return Err(InvestigationError::Cancelled);
                        }
                        let now = (self.coordinator.now_ms)();
                        if now >= self.context.receipt.execution_cutoff_ms {
                            return self.settle(&broker, RunResult::Exhausted);
                        }
                        self.execute(&mut broker, operation, now);
                    }
                    broker.accounting.end_batch();
                }
                Step::Propose(outcome) => {
                    let proposal = match bind_proposal(
                        &self.coordinator.store,
                        &mut broker,
                        &self.context,
                        &self.disclosed_spans,
                        *outcome,
                    ) {
                        Ok(proposal) => proposal,
                        Err(refusal) => {
                            return self.settle(&broker, RunResult::Refused(refusal.code));
                        }
                    };
                    return self.settle(&broker, RunResult::Proposal(Box::new(proposal)));
                }
                Step::Abstain { .. } => return self.settle(&broker, RunResult::Declined),
            }
        }
        self.settle_now(RunResult::Exhausted).await
    }

    async fn settle_now(&self, result: RunResult) -> Result<Settled, InvestigationError> {
        let broker = Arc::clone(&self.broker);
        let broker = broker.lock().await;
        self.settle(&broker, result)
    }

    fn settle(
        &self,
        broker: &EvidenceBroker,
        result: RunResult,
    ) -> Result<Settled, InvestigationError> {
        let now_ms = Arc::clone(&self.coordinator.now_ms);
        Settlement {
            store: &self.coordinator.store,
            ledger: &self.coordinator.ledger,
            project: &self.context.job.project,
            binding: self.context.binding,
            claim: self.context.claim,
            now_ms: &move || now_ms(),
            #[cfg(any(test, feature = "test-support"))]
            before_completion_for_test: None,
        }
        .settle(broker, result)
        .map_err(InvestigationError::from)
    }

    /// One supervised physical request: the prepared body is disclosed inside the supervisor's internal launch, so the backend permit, cutoff, cancellation, and physical completion are the supervisor's, and the text comes back only after the disclosure accepted it.
    async fn attempt(
        &mut self,
        attempt_index: u32,
        cancel: &CancellationToken,
    ) -> Result<Attempt, InvestigationError> {
        let coordinator = self.coordinator;
        let prepared = {
            let mut guard = self.broker.lock().await;
            // Q22: a buffer was charged when it was read; every later send charges it again.
            for buffer in &self.transcript[..self.sent] {
                let tag = buffer.tag();
                if guard
                    .accounting
                    .charge_render(tag.alias.as_ref(), tag.charged_bytes)
                    .is_err()
                {
                    return Ok(Attempt::Exhausted);
                }
            }
            let system = guard
                .render_host_text(&format!(
                    "{}\n\n{STEP_INSTRUCTIONS}",
                    self.context.question.text()
                ))
                .map_err(|refusal| InvestigationError::Kernel(refusal.code))?;
            prepare_body(
                &guard,
                &coordinator.profile,
                system,
                self.transcript.clone(),
            )
            .map_err(|refusal| match refusal {
                DisclosureRefusal::Send { .. } => InvestigationError::Kernel(RefusalCode::TooLarge),
                _ => InvestigationError::Kernel(RefusalCode::Store),
            })?
        };
        let key = InternalRunKey {
            task_kind: CURATOR_TASK_KIND.to_string(),
            project_digest: self.hold_binding.project_digest.clone(),
            job_id: self.hold_binding.subject.clone(),
            receipt_generation: self.hold_binding.generation,
            attempt: attempt_index,
            kernel_incarnation: self.hold_binding.kernel_incarnation.clone(),
            memstore_incarnation: self.hold_binding.memstore_incarnation.clone(),
        };
        let cutoff = self.cutoff;
        let deadline = Instant::now()
            .checked_add(Duration::from_millis(CURATOR_ATTEMPT_MAX_MS as u64))
            .map_or(cutoff, |attempt| attempt.min(cutoff));
        let request_bytes = prepared.body().len();
        let outcome: Arc<Mutex<Option<Result<Disclosed, DisclosureRefusal>>>> =
            Arc::new(Mutex::new(None));
        let launch = {
            let store = Arc::clone(&coordinator.store);
            let ledger = Arc::clone(&coordinator.ledger);
            let broker = Arc::clone(&self.broker);
            let sender = Arc::clone(&coordinator.sender);
            let approval = coordinator.approval.clone();
            let now_ms = Arc::clone(&coordinator.now_ms);
            let binding = AttemptBinding {
                project: self.context.job.project.clone(),
                causal_identity: self.hold_binding.subject.clone(),
                generation: self.hold_binding.generation,
                claim_id: self.context.claim.claim_id.clone(),
                credential_id: coordinator.credential_id.clone(),
            };
            let outcome = Arc::clone(&outcome);
            move |sink: EventSink, token: CancellationToken| {
                Box::pin(async move {
                    let broker = broker.lock().await;
                    let disclosure = Disclosure {
                        store: &store,
                        ledger: &ledger,
                        broker: &broker,
                        sender: &sender,
                        approval: approval.as_ref(),
                        binding: &binding,
                        now_ms: &move || now_ms(),
                    };
                    let disclosed = disclosure.disclose(&prepared, &token, deadline).await;
                    let terminal = match &disclosed {
                        Ok(disclosed) => {
                            sink.emit(BackendEvent::AssistantText {
                                text: disclosed.text.text.clone(),
                                finish_reason: Some(FinishReason::Completed),
                            });
                            BackendTerminal::Completed {
                                finish_reason: FinishReason::Completed,
                            }
                        }
                        Err(refusal) => BackendTerminal::Failed(BackendError {
                            class: ErrorClass::Permanent,
                            message: format!("curator disclosure: {refusal}"),
                            retry_after_secs: None,
                            provider_code: None,
                        }),
                    };
                    *outcome.lock().unwrap_or_else(|poison| poison.into_inner()) = Some(disclosed);
                    terminal
                }) as BackendFuture
            }
        };
        let run = coordinator
            .supervisor
            .launch_internal(key, request_bytes, cutoff, Box::new(launch))
            .map_err(|error| InvestigationError::Supervisor(error.code.to_string()))?;
        let settled = tokio::select! {
            biased;
            () = cancel.cancelled() => {
                let _ = run.cancel().await;
                return Err(InvestigationError::Cancelled);
            }
            settled = run.settled() => settled,
        };
        let disclosed = outcome
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take();
        // Buffers count as sent once a request byte left the host; a refusal before the handoff leaves them uncharged for the next send.
        let left_host = matches!(
            &disclosed,
            Some(Ok(_))
                | Some(Err(DisclosureRefusal::Send { sent: true, .. }
                    | DisclosureRefusal::ModelMismatch { .. }
                    | DisclosureRefusal::TerminalNotRecorded { .. }))
        );
        if left_host {
            self.sent = self.transcript.len();
        }
        // The supervisor's verdict comes first: text that arrives at the cutoff is not admitted, whatever the disclosure recorded for the attempt.
        match (settled, disclosed) {
            (InternalOutcome::Cutoff, _) => Ok(Attempt::Exhausted),
            (InternalOutcome::Shutdown | InternalOutcome::Cancelled, _) => {
                Err(InvestigationError::Cancelled)
            }
            (InternalOutcome::Lost, _) => Err(InvestigationError::Supervisor("lost".to_string())),
            (_, Some(Ok(disclosed))) => Ok(Attempt::Text(disclosed.text.text)),
            (_, Some(Err(refusal))) => Ok(match refusal {
                DisclosureRefusal::Unavailable => return Err(InvestigationError::Unavailable),
                DisclosureRefusal::Cancelled => return Err(InvestigationError::Cancelled),
                // A disclosed input no longer stands, or the hold is gone: another request would refuse the same way.
                DisclosureRefusal::Revalidation(refusal) => Attempt::Refused(refusal.code),
                DisclosureRefusal::Hold(code) => Attempt::Refused(code),
                DisclosureRefusal::Ledger(reason)
                | DisclosureRefusal::ChargedNotDispatched { reason, .. } => ledger_verdict(reason)?,
                DisclosureRefusal::DestinationNotRemote
                | DisclosureRefusal::PromptNotUtf8
                | DisclosureRefusal::PolicyUnion => {
                    return Err(InvestigationError::Kernel(RefusalCode::Unsupported));
                }
                DisclosureRefusal::Store(error) => return Err(InvestigationError::Store(error)),
                // The request was charged and failed or was withheld; the disclosure recorded the attempt's terminal, and the remaining budget decides whether another is made.
                DisclosureRefusal::Send { .. }
                | DisclosureRefusal::ModelMismatch { .. }
                | DisclosureRefusal::TerminalNotRecorded { .. } => Attempt::Spent,
            }),
            (InternalOutcome::Completed | InternalOutcome::Failed, None) => {
                Err(InvestigationError::Supervisor("no outcome".to_string()))
            }
        }
    }

    fn record_span(&mut self, alias: &Alias, span: std::ops::Range<u64>) {
        self.disclosed_spans
            .entry(alias.as_str().to_string())
            .or_default()
            .push(span);
    }

    /// Appends a buffer to the transcript behind a host label naming its alias and prompt boundary, so the model can tell which bytes belong to which reference and cite by alias.
    fn push_labeled(&mut self, broker: &EvidenceBroker, buffer: RenderedBuffer) {
        if let Some(alias) = &buffer.tag().alias
            && let Ok(label) = broker.render_host_text(&format!("\n[{}]\n", alias.as_str()))
        {
            self.transcript.push(label);
        }
        self.transcript.push(buffer);
    }
}

/// What a ledger refusal before or after the marker commit means for the run: its budget or time is spent, its authority is gone, or it was cancelled.
fn ledger_verdict(reason: CuratorLedgerRefusal) -> Result<Attempt, InvestigationError> {
    match reason {
        CuratorLedgerRefusal::AttemptsExhausted
        | CuratorLedgerRefusal::Cutoff
        | CuratorLedgerRefusal::JobDeadline
        | CuratorLedgerRefusal::JobUnavailable => Ok(Attempt::Exhausted),
        CuratorLedgerRefusal::Cancelled => Err(InvestigationError::Cancelled),
        // The post-commit recheck could not read the ledger: the attempt is charged and nothing was sent; another round decides.
        CuratorLedgerRefusal::RecheckUnavailable => Ok(Attempt::Spent),
        CuratorLedgerRefusal::InvalidRequest
        | CuratorLedgerRefusal::Missing
        | CuratorLedgerRefusal::Fenced
        | CuratorLedgerRefusal::BindingMismatch
        | CuratorLedgerRefusal::ClaimInvalid
        | CuratorLedgerRefusal::AuthorityChanged
        | CuratorLedgerRefusal::AttemptTerminal => Err(InvestigationError::Fenced),
    }
}

impl Run<'_> {
    /// Executes one admitted operation and appends its rendered results, or a refusal notice, to the transcript. Refused-after-admission operations count against the job (Q23); a refusal is a host-authored code, never provider or source text.
    fn execute(&mut self, broker: &mut EvidenceBroker, operation: Operation, now: i64) {
        let store = &self.coordinator.store;
        let unavailable = || refuse(None, RefusalCode::Unavailable);
        let mut spans: Vec<(Alias, std::ops::Range<u64>)> = Vec::new();
        // Every read below is admitted and charged by the broker (Q23); the coordinator adds nothing to that accounting.
        let outcome: Result<Vec<RenderedBuffer>, Refusal> = match operation {
            Operation::ReadReference { alias, range } => broker
                .read(store, &alias, range.map(|range| range.to_range()), now)
                .map(|read| {
                    let end = read.buffer.bytes().len() as u64;
                    let span = range.map_or(0..end, |range| range.start..range.start + end);
                    spans.push((read.alias.clone(), span));
                    vec![read.buffer]
                }),
            Operation::FindRelated { cursor } => {
                let budget = EvalBudget::new(Some(self.cutoff.into_std()), Arc::default());
                self.discovery
                    .page(store, broker, cursor.as_deref(), &budget, now)
                    .and_then(|page| {
                        spans.extend(
                            page.hits
                                .iter()
                                .map(|hit| (hit.alias.clone(), hit.span.clone())),
                        );
                        let summary = format!(
                            "related search: {} hits, completeness {:?}{}{}",
                            page.hits.len(),
                            page.completeness,
                            withheld(page.withheld),
                            page.next_cursor
                                .as_deref()
                                .map(|cursor| format!(", cursor {cursor}"))
                                .unwrap_or_default()
                        );
                        let hits = page
                            .hits
                            .into_iter()
                            .map(|hit| (hit.alias, hit.buffer, hit.shared_origin));
                        render_hits(broker, hits, &summary)
                    })
            }
            Operation::SearchProject { by, literal } => {
                match self.context.project_root.as_deref_mut() {
                    None => Err(unavailable()),
                    Some(root) => {
                        let query = match by {
                            SearchBy::Path => SearchQuery::Path(&literal),
                            SearchBy::Name => SearchQuery::Name(&literal),
                            SearchBy::Content => SearchQuery::Content(&literal),
                        };
                        root.search(store, broker, query, now).and_then(|outcome| {
                            spans.extend(
                                outcome
                                    .hits
                                    .iter()
                                    .map(|hit| (hit.alias.clone(), hit.span.clone())),
                            );
                            let summary = format!(
                                "project search: {} hits, completeness {:?}{}",
                                outcome.hits.len(),
                                outcome.completeness,
                                withheld(outcome.withheld)
                            );
                            let hits = outcome
                                .hits
                                .into_iter()
                                .map(|hit| (hit.alias, hit.buffer, None));
                            render_hits(broker, hits, &summary)
                        })
                    }
                }
            }
            Operation::ReadProject { path, range } => {
                match self.context.project_root.as_deref_mut() {
                    None => Err(unavailable()),
                    Some(root) => root
                        .read(
                            store,
                            broker,
                            &path,
                            range.map(|range| range.to_range()),
                            now,
                        )
                        .map(|read| vec![read.buffer]),
                }
            }
        };
        match outcome {
            Ok(buffers) => {
                for (alias, span) in spans {
                    self.record_span(&alias, span);
                }
                for buffer in buffers {
                    self.push_labeled(broker, buffer);
                }
            }
            Err(refusal) => {
                // The broker marks the disclosure partial when it refuses admission itself; a byte or buffer bound hit inside a read is the same truncation.
                if matches!(
                    refusal.code,
                    RefusalCode::ByteLimit | RefusalCode::BufferLimit
                ) {
                    broker.ledger.record_partial_disclosure();
                }
                push_notice(
                    broker,
                    &mut self.transcript,
                    refusal.alias.as_ref(),
                    refusal.code,
                );
            }
        }
    }
}

/// Whether `range` lies inside the union of `spans`: adjacent or overlapping disclosures cover a citation across their seam.
fn covered(spans: &[std::ops::Range<u64>], range: std::ops::Range<u64>) -> bool {
    let mut sorted: Vec<_> = spans.to_vec();
    sorted.sort_by_key(|span| span.start);
    let mut reach: Option<std::ops::Range<u64>> = None;
    for span in sorted {
        reach = match reach {
            Some(current) if span.start <= current.end => {
                Some(current.start..current.end.max(span.end))
            }
            _ => Some(span),
        };
        if reach
            .as_ref()
            .is_some_and(|current| current.start <= range.start && range.end <= current.end)
        {
            return true;
        }
    }
    false
}

/// A host-authored notice appended to the transcript: the alias, when there is one, and a bounded code. A notice that itself fails the render check is dropped rather than shown.
fn push_notice(
    broker: &EvidenceBroker,
    transcript: &mut Vec<RenderedBuffer>,
    alias: Option<&Alias>,
    code: RefusalCode,
) {
    let text = match alias {
        Some(alias) => format!("refused {}: {code}", alias.as_str()),
        None => format!("refused: {code}"),
    };
    if let Ok(buffer) = broker.render_host_text(&text) {
        transcript.push(buffer);
    }
}

fn withheld(withheld: bool) -> &'static str {
    if withheld { ", some withheld" } else { "" }
}

/// Search hits laid into the transcript as the searcher rendered them, a shared origin named beside its hit, and the host's completeness summary closing the batch.
fn render_hits(
    broker: &EvidenceBroker,
    hits: impl Iterator<Item = (Alias, RenderedBuffer, Option<Alias>)>,
    summary: &str,
) -> Result<Vec<RenderedBuffer>, Refusal> {
    let mut buffers = Vec::new();
    for (alias, buffer, shared_origin) in hits {
        buffers.push(buffer);
        if let Some(origin) = shared_origin {
            buffers.push(broker.render_host_text(&format!(
                "{} shares its origin with {}",
                alias.as_str(),
                origin.as_str()
            ))?);
        }
    }
    buffers.push(broker.render_host_text(summary)?);
    Ok(buffers)
}

/// The Kernel expectation for the job's subject and the evidence ids the execution hold must protect for it.
fn resolve_subject(
    store: &KernelStore,
    target: &ReviewTarget,
    binding: &ReviewBinding,
) -> Result<(ReferenceExpectation, Vec<String>), InvestigationError> {
    match target {
        ReviewTarget::StagedSubject {
            kernel_incarnation,
            candidate_id,
            payload_digest,
        } => Ok((
            ReferenceExpectation::StagedSubject {
                reference: ReviewStagedReference {
                    database_incarnation_id: kernel_incarnation.clone(),
                    candidate_id: candidate_id.clone(),
                    payload_digest: payload_digest.clone(),
                },
                binding: binding.clone(),
            },
            Vec::new(),
        )),
        ReviewTarget::Memory {
            object_id,
            source_revision,
        } => resolve_descriptor(store, object_id, Some(*source_revision)),
    }
}

/// The expectation for one native source descriptor at its live revision, or at `expected_revision` when the job bound one.
fn resolve_descriptor(
    store: &KernelStore,
    object_id: &str,
    expected_revision: Option<i64>,
) -> Result<(ReferenceExpectation, Vec<String>), InvestigationError> {
    let kernel = |code| InvestigationError::Kernel(code);
    let tip = store.tip().map_err(|_| kernel(RefusalCode::Store))?;
    let row = store
        .observation_for_object_as_of(object_id, tip)
        .map_err(|_| kernel(RefusalCode::Store))?
        .ok_or(kernel(RefusalCode::NotFound))?;
    let detail: SourceDescriptorDetail = row
        .payload
        .detail
        .as_deref()
        .and_then(|detail| serde_json::from_str(detail).ok())
        .ok_or(kernel(RefusalCode::Unsupported))?;
    let class =
        OccurrenceClass::from_code(&detail.class).ok_or(kernel(RefusalCode::Unsupported))?;
    let source_revision = match expected_revision {
        Some(revision) => revision,
        None => detail
            .revision
            .parse()
            .map_err(|_| kernel(RefusalCode::Unsupported))?,
    };
    if !resolves_class(class) {
        return Err(InvestigationError::Refused(RefusalCode::Unsupported));
    }
    let expectation = ReferenceExpectation::NativeSource {
        object_id: object_id.to_string(),
        class,
        source_revision,
        artifact_digest: detail.artifact_digest.clone(),
        evidence_id: detail.evidence_id.clone(),
        occurrence_tuple: detail.occurrence_tuple.clone(),
    };
    Ok((expectation, vec![detail.evidence_id]))
}

/// Whether a descriptor of `class` resolves to a subject the run can read. Canonical and promoted descriptors resolve through their originating decision, a path this run does not implement, so they are refused; the selector walks only classes this admits.
pub fn resolves_class(class: OccurrenceClass) -> bool {
    match class {
        OccurrenceClass::CanonicalClaims | OccurrenceClass::PromotedMemory => false,
        OccurrenceClass::Messages | OccurrenceClass::GitCommits | OccurrenceClass::RawToolSpans => {
            true
        }
    }
}

/// Binds the model's outcome into a proposal: every citation resolves to disclosed evidence through the broker, names only bytes the model was shown, and is recorded as a citation; the target is the job's subject; the manifest reference digests the disclosed spans; policy dependencies are left for settlement to fill from the broker.
fn bind_proposal(
    store: &KernelStore,
    broker: &mut EvidenceBroker,
    context: &JobContext<'_>,
    disclosed_spans: &BTreeMap<String, Vec<std::ops::Range<u64>>>,
    outcome: ProposedOutcome,
) -> Result<ReviewProposal, Refusal> {
    let tip = store.tip().map_err(|_| refuse(None, RefusalCode::Store))?;
    let mut cite =
        |citations: Vec<super::steps::Citation>| -> Result<Vec<EvidenceReference>, Refusal> {
            let mut references = Vec::with_capacity(citations.len());
            for citation in citations {
                let (alias, expectation) = broker.aliases.resolve(&citation.alias)?;
                let alias = alias.clone();
                let evidence_id = expectation
                    .evidence_id()
                    .ok_or_else(|| refuse(Some(&alias), RefusalCode::Unsupported))?
                    .to_string();
                let shown = disclosed_spans
                    .get(alias.as_str())
                    .ok_or_else(|| refuse(Some(&alias), RefusalCode::UnknownAlias))?;
                if let Some(range) = citation.range
                    && !covered(shown, range.start..range.end)
                {
                    return Err(refuse(Some(&alias), RefusalCode::InvalidRange));
                }
                broker.ledger.record_citation(&alias)?;
                references.push(EvidenceReference {
                    evidence_id,
                    span: citation.range.map(|range| SourceSpan {
                        alias: alias.as_str().to_string(),
                        start: range.start,
                        end: range.end,
                    }),
                });
            }
            Ok(references)
        };
    let support = cite(outcome.support)?;
    let contradictions = cite(outcome.contradictions)?;
    let target = match &context.input.subject {
        ReviewTarget::StagedSubject { candidate_id, .. } => ProposalTarget::StagedCandidate {
            candidate_id: candidate_id.clone(),
        },
        ReviewTarget::Memory {
            object_id,
            source_revision,
        } => ProposalTarget::Memory(kernel::CanonicalTarget {
            object_id: object_id.clone(),
            source_revision: *source_revision,
            known_as_of: context.receipt.created_at_ms,
            commit_token: tip,
        }),
    };
    // The manifest is the inspection record this run can attest to: every alias and the byte ranges disclosed under it, in order.
    let mut manifest = Sha256::new();
    for (alias, spans) in disclosed_spans {
        manifest.update(alias.as_bytes());
        for span in spans {
            manifest.update(span.start.to_be_bytes());
            manifest.update(span.end.to_be_bytes());
        }
        manifest.update([0x1f]);
    }
    Ok(ReviewProposal {
        action: outcome.action,
        target,
        new_text: outcome.new_text,
        support,
        contradictions,
        limitations: outcome.limitations,
        uncertainty: outcome.uncertainty,
        manifest: ManifestReference {
            manifest_id: format!(
                "manifest:{}:{}",
                context.job.causal_identity, context.receipt.generation
            ),
            digest: format!("{:x}", manifest.finalize()),
        },
        policy_dependencies: PolicyDependencies {
            question_template: ReviewQuestionTemplate::ExtractedFacts,
            disclosed_inputs: vec![],
            uncited_disclosed_inputs: vec![],
            ancestry: vec![],
        },
    })
}

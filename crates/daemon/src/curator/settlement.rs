//! Settlement of one Curator run across the Kernel and Memory Store, and the read that follows it.
//!
//! Settlement copies the run's attempt terminals out of the Memory Store, releases that read, and derives the unknown-disclosure scalar before anything else; an unknown attempt or a truncated disclosure permits only a content-free terminal. It then revalidates the full disclosed union through the Kernel at the current clock, stages the proposal at the provisional identity the run reserved, moves retention from the execution hold to a review hold in one Kernel envelope, and only then completes the task lease and receipt in the Memory Store's fenced transaction. That completion is the store's to decide: a receipt cancelled before it, or completed at or after its run deadline, records `cancelled` or `expired` in place of whatever the run produced, and settlement reads the receipt back to report what was recorded. A receipt that another generation already owns writes nothing, so a late worker's staged result stays private: fenced before its hold transfer, the row expires under its original queue deadline; fenced after it, the row's deadline has already moved to the review expiry and only moves later, so the loser releases the review hold and the unselected row lapses there. Completed staging is not selection.
//!
//! Reads follow the same copy-then-enter order in reverse: the completed receipt is copied and the Memory Store released before the Kernel is entered; the Kernel row must match every selected field and both incarnations; the review hold must be live; and every disclosed input is revalidated before content is returned. Completion never freezes eligibility.

use kernel::{
    ArtifactDestination, ArtifactEligibility, ArtifactHandle, CuratorHold, CuratorHoldBinding,
    CuratorHoldError, CuratorHoldKind, CuratorHoldRefusal, EvidenceReference, KernelStore,
    PolicyDependencies, REVIEW_EXPIRY_MAX_MS, ReviewBinding, ReviewOwner, ReviewPayload,
    ReviewProposal, ReviewReadError, ReviewReadRefusal, ReviewStageError, ReviewStageRefusal,
    ReviewStagedReference, ReviewStagingSpec, StagingTerminalState, provisional_result_identity,
};
use memory_store::curator_ledger::{
    AbstainReason, CuratorAttemptTerminal, CuratorReceiptTerminal, MAX_RECEIPT_PAGE,
    ReceiptCompletion, ResultSelection,
};
use memory_store::{LeaseCompleteOutcome, MemoryStore};

use super::broker::{EvidenceBroker, HeldUnder, RefusalCode, check_render};

/// Producer recorded on settled proposals.
pub const SETTLEMENT_PRODUCER: &str = "curator-settlement";

/// The task lease the settling worker holds. The completion id is derived from the job and generation, so a retried settlement replays instead of completing twice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskClaim {
    pub claim_id: String,
    pub worker_instance: String,
    pub slot: i64,
}

/// What the run produced: a proposal to publish, the model's own decision not to conclude, a run that spent its rounds or requests without concluding, or a run the broker refused before the model saw its subject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunResult {
    Proposal(Box<ReviewProposal>),
    Declined,
    Exhausted,
    Refused(RefusalCode),
}

/// How one settlement ended in the Memory Store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settled {
    /// The completed receipt selects this staged proposal.
    Published(ReviewStagedReference),
    /// A content-free abstention completed the receipt.
    Abstained(AbstainReason),
    /// An attempt's outcome is unknown; the receipt records that and nothing publishes.
    Unknown,
    /// The receipt was cancelled before this completion; the Memory Store recorded that in place of what the run produced.
    Cancelled,
    /// The completion came at or after the run deadline; the Memory Store recorded that in place of what the run produced.
    Expired,
}

/// Why a settlement wrote no completion. Nothing here carries proposal content.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SettlementError {
    /// The receipt is not in progress at this generation under this claim; another generation owns the job.
    #[error("fenced")]
    Fenced,
    /// A different proposal already occupies the provisional identity.
    #[error("conflicting_content")]
    ConflictingContent,
    /// The staging binding names another project or job than the run's hold; nothing was staged.
    #[error("binding_mismatch")]
    BindingMismatch,
    /// The Kernel refused the staging or hold transfer for a reason other than conflict.
    #[error("kernel {0}")]
    Kernel(String),
    #[error("store {0}")]
    Store(String),
}

/// Everything settlement needs beyond the broker: both stores, the job's review binding, the worker's claim, and the ledger clock.
pub struct Settlement<'a> {
    pub store: &'a KernelStore,
    pub ledger: &'a MemoryStore,
    /// The Memory Store project the job, receipt, and attempt rows live under: the authority key, which is not the Kernel project digest the hold and binding are scoped by.
    pub project: &'a str,
    /// The job's staging binding, owned by the run's job in the run's project; its owner is replaced by the proposal owner for this generation.
    pub binding: &'a ReviewBinding,
    pub claim: &'a TaskClaim,
    pub now_ms: &'a (dyn Fn() -> i64 + Sync),
    /// Runs after the Kernel work and before the Memory Store completion, so a test can interleave a takeover in the crash window between the two stores.
    #[cfg(any(test, feature = "test-support"))]
    pub before_completion_for_test: Option<&'a dyn Fn()>,
}

impl Settlement<'_> {
    /// Settles one run. The broker is the run's own; its binding names the job, generation, execution hold, and incarnations.
    pub fn settle(
        &self,
        broker: &EvidenceBroker,
        result: RunResult,
    ) -> Result<Settled, SettlementError> {
        let run = &broker.binding().hold;
        self.copy_receipt(run)?;
        // The binding's project and job must be the run's own: the row is staged under this binding's scope and lineage, and only that job's receipt may select it.
        if self.binding.project_digest != run.project_digest
            || !matches!(&self.binding.owner, ReviewOwner::Job { job_id } if *job_id == run.subject)
        {
            return Err(SettlementError::BindingMismatch);
        }
        // Q8: the staged proposal keeps the job's immutable queue deadline until selection moves it to the review expiry.
        let queue_deadline_at = self
            .ledger
            .lookup_curator_job(self.project, &run.subject)
            .map_err(store)?
            .ok_or(SettlementError::Fenced)?
            .queue_deadline_ms;
        let disclosure_unknown = self.disclosure_unknown(run, broker)?;
        let now = (self.now_ms)();
        // A settlement retried after its Kernel envelope committed finds retention already on the review hold; every check below runs under that hold instead of the released execution hold.
        let identity = provisional_result_identity(&run.subject, run.generation);
        let review = CuratorHoldBinding {
            subject: identity.candidate_id,
            ..run.clone()
        };
        let recovered = self
            .store
            .lookup_review_hold(&review, now)
            .map_err(kernel)?;
        let content_free = |completion: ContentFree| {
            self.complete_without_content(
                broker,
                completion,
                recovered.as_ref().map(|hold| (&review, hold)),
            )
        };
        if disclosure_unknown {
            return content_free(ContentFree::Unknown);
        }
        if broker.ledger.is_partial() {
            return content_free(ContentFree::Abstained(AbstainReason::PartialDisclosure));
        }
        let proposal = match result {
            RunResult::Declined => {
                return content_free(ContentFree::Abstained(AbstainReason::ModelDeclined));
            }
            RunResult::Exhausted => {
                return content_free(ContentFree::Abstained(AbstainReason::BudgetExhausted));
            }
            RunResult::Refused(code) => {
                return match Verdict::from_refusal(code) {
                    Verdict::Abstain(reason) => content_free(ContentFree::Abstained(reason)),
                    Verdict::Store(error) => Err(SettlementError::Store(error)),
                };
            }
            RunResult::Proposal(proposal) => proposal,
        };
        let held_under = match &recovered {
            Some(hold) => HeldUnder::Review {
                hold,
                binding: &review,
            },
            None => HeldUnder::Execution(broker.binding()),
        };
        let disclosed = match self.revalidate_union(broker, held_under, now) {
            Ok(disclosed) => disclosed,
            Err(Verdict::Abstain(reason)) => {
                return content_free(ContentFree::Abstained(reason));
            }
            Err(Verdict::Store(error)) => return Err(SettlementError::Store(error)),
        };
        let payload = match bind_dependencies(broker, *proposal, &disclosed) {
            Ok(payload) => payload,
            Err(reason) => return content_free(ContentFree::Abstained(reason)),
        };
        let (reference, created_at) = self.stage(run, queue_deadline_at, payload, now)?;
        let review_hold = match recovered {
            Some(hold) => hold,
            None => self.transfer_retention(broker, &review, created_at)?,
        };
        self.publish(run, &review, &review_hold, reference)
    }

    /// Q20: the marker-derived scalar. An attempt without a terminal at settlement is as unknown as one recorded unknown, and so is a broker that recorded an uncertain disclosure.
    fn disclosure_unknown(
        &self,
        run: &CuratorHoldBinding,
        broker: &EvidenceBroker,
    ) -> Result<bool, SettlementError> {
        let attempts = self
            .ledger
            .list_curator_attempts(self.project, &run.subject)
            .map_err(store)?;
        Ok(attempts.iter().any(|attempt| {
            matches!(
                attempt.terminal,
                None | Some((CuratorAttemptTerminal::Unknown, _))
            )
        }) || broker.ledger.is_uncertain())
    }

    /// The Memory Store's fenced completion selecting `reference`. A fenced completion means another generation owns the receipt: the staged row stays private, and the retention this generation moved to review is released so a losing result holds nothing past its own settlement. A completion the store recorded as cancelled or expired selected nothing either, and releases the same hold.
    fn publish(
        &self,
        run: &CuratorHoldBinding,
        review: &CuratorHoldBinding,
        review_hold: &CuratorHold,
        reference: ReviewStagedReference,
    ) -> Result<Settled, SettlementError> {
        let selection = ResultSelection {
            candidate_id: reference.candidate_id.clone(),
            payload_digest: reference.payload_digest.clone(),
            project_digest: run.project_digest.clone(),
        };
        match self.complete(run, ReceiptCompletion::Complete(selection))? {
            // The completion id proves this claim completed the receipt, not what it recorded: a replay may have selected other content, and the store may have recorded a cancellation or the deadline instead of the selection.
            LeaseCompleteOutcome::Applied { .. } | LeaseCompleteOutcome::Replayed { .. } => {
                // A terminal that is not this reference means the hold protects a row nothing selects. A read-back that failed decided nothing: the receipt may select this row, readers need the hold to reach it, and a retry is fenced by the terminal, so the hold stays.
                let settled = self.recorded(run, Some(reference));
                if !matches!(
                    settled,
                    Ok(Settled::Published(_)) | Err(SettlementError::Store(_))
                ) {
                    self.release_review_hold(run, review, review_hold);
                }
                settled
            }
            LeaseCompleteOutcome::Conflict { kind } => {
                if !conflict_ends_claim(kind) {
                    return Err(retry_later(kind));
                }
                self.release_review_hold(run, review, review_hold);
                Err(SettlementError::Fenced)
            }
        }
    }

    /// What the receipt records once this run's completion id has completed it, by this write or an earlier one. A selection must be this generation's `published` reference; any other content at a publication, or content where a content-free terminal was reported, conflicts. `cancelled` and `expired` are the store's own terminals and stand whatever the run produced.
    fn recorded(
        &self,
        run: &CuratorHoldBinding,
        published: Option<ReviewStagedReference>,
    ) -> Result<Settled, SettlementError> {
        let receipt = self
            .ledger
            .lookup_curator_receipt(self.project, &run.subject)
            .map_err(store)?
            .ok_or(SettlementError::ConflictingContent)?;
        let selects = |reference: &ReviewStagedReference| {
            receipt
                .selected
                .as_ref()
                .is_some_and(|(generation, selection)| {
                    *generation == run.generation
                        && selection.candidate_id == reference.candidate_id
                        && selection.payload_digest == reference.payload_digest
                })
        };
        match (receipt.terminal, receipt.abstained_reason, published) {
            (Some(CuratorReceiptTerminal::Complete), _, Some(reference)) if selects(&reference) => {
                Ok(Settled::Published(reference))
            }
            (Some(CuratorReceiptTerminal::Abstained), Some(reason), None) => {
                Ok(Settled::Abstained(reason))
            }
            (Some(CuratorReceiptTerminal::Unknown), _, None) => Ok(Settled::Unknown),
            (Some(CuratorReceiptTerminal::Cancelled), _, _) => Ok(Settled::Cancelled),
            (Some(CuratorReceiptTerminal::Expired), _, _) => Ok(Settled::Expired),
            _ => Err(SettlementError::ConflictingContent),
        }
    }

    /// The receipt must be in progress at the run's generation under the worker's claim and bound to the run's incarnations.
    fn copy_receipt(&self, run: &CuratorHoldBinding) -> Result<(), SettlementError> {
        let receipt = self
            .ledger
            .lookup_curator_receipt(self.project, &run.subject)
            .map_err(store)?
            .ok_or(SettlementError::Fenced)?;
        if receipt.terminal.is_some()
            || receipt.generation != run.generation
            || receipt.claim_id != self.claim.claim_id
            || receipt.kernel_incarnation_id != run.kernel_incarnation
            || receipt.database_incarnation_id != run.memstore_incarnation
        {
            return Err(SettlementError::Fenced);
        }
        Ok(())
    }

    /// A content-free completion: the receipt records the terminal, then the hold that still protects the run's evidence is released on that trusted terminal. Nothing is staged.
    fn complete_without_content(
        &self,
        broker: &EvidenceBroker,
        completion: ContentFree,
        recovered: Option<(&CuratorHoldBinding, &CuratorHold)>,
    ) -> Result<Settled, SettlementError> {
        let run = &broker.binding().hold;
        let completion = match completion {
            ContentFree::Unknown => ReceiptCompletion::Unknown,
            ContentFree::Abstained(reason) => ReceiptCompletion::Abstained(reason),
        };
        let settled = match self.complete(run, completion)? {
            // The completion id is the run's, whatever it recorded: a replay reports the reason the earlier write recorded, and the store's own cancelled or expired terminal stands over what the run derived. Any of those is a trusted terminal to release retention on; recorded content is not.
            LeaseCompleteOutcome::Applied { .. } | LeaseCompleteOutcome::Replayed { .. } => {
                self.recorded(run, None)?
            }
            // Another generation owns the receipt: this one can never complete, so the review hold it had moved retention to is released as the publication path does. Its execution hold ends only on a trusted terminal or at the run cutoff.
            LeaseCompleteOutcome::Conflict { kind } => {
                if !conflict_ends_claim(kind) {
                    return Err(retry_later(kind));
                }
                if let Some((review, hold)) = recovered {
                    self.release_review_hold(run, review, hold);
                }
                return Err(SettlementError::Fenced);
            }
        };
        match recovered {
            Some((review, hold)) => self.release_review_hold(run, review, hold),
            None if broker.binding().hold_id.is_empty() => {}
            None => match self
                .store
                .release_execution_hold(&broker.binding().hold_id, run)
            {
                Ok(())
                | Err(CuratorHoldError::Refused(
                    CuratorHoldRefusal::Released | CuratorHoldRefusal::Expired,
                )) => {}
                Err(error) => {
                    eprintln!(
                        "daemon: curator settlement could not release execution hold for {}/{} generation {}: {error}",
                        self.project, run.subject, run.generation
                    );
                }
            },
        }
        Ok(settled)
    }

    fn release_review_hold(
        &self,
        run: &CuratorHoldBinding,
        review: &CuratorHoldBinding,
        hold: &CuratorHold,
    ) {
        if let Err(error) = self.store.release_review_hold(&hold.hold_id, review) {
            eprintln!(
                "daemon: curator settlement could not release review hold for {}/{} generation {}: {error}",
                self.project, run.subject, run.generation
            );
        }
    }

    /// The clock is sampled at the completion itself, so Kernel work between the copy and this write never lets a lapsed lease complete on stale time.
    fn complete(
        &self,
        run: &CuratorHoldBinding,
        completion: ReceiptCompletion,
    ) -> Result<LeaseCompleteOutcome, SettlementError> {
        #[cfg(any(test, feature = "test-support"))]
        if let Some(hook) = self.before_completion_for_test {
            hook();
        }
        self.ledger
            .complete_curator_receipt(
                self.project,
                &run.subject,
                &self.claim.claim_id,
                &format!("curator-settlement:{}:{}", run.subject, run.generation),
                &self.claim.worker_instance,
                self.claim.slot,
                run.generation,
                &run.kernel_incarnation,
                &completion,
                (self.now_ms)(),
            )
            .map_err(store)
    }

    /// Every disclosed alias, cited or not, is re-judged at `now` under `hold`; the evidence ids they name are then validated as one held batch. Returns those ids, sorted.
    fn revalidate_union(
        &self,
        broker: &EvidenceBroker,
        hold: HeldUnder<'_>,
        now: i64,
    ) -> Result<Vec<String>, Verdict> {
        let mut evidence = Vec::new();
        for alias in broker.ledger.disclosed() {
            match broker.revalidate_under(self.store, alias.as_str(), now, hold) {
                Ok(Some(id)) => evidence.push(id),
                Ok(None) => {}
                Err(refusal) => return Err(Verdict::from_refusal(refusal.code)),
            }
        }
        evidence.sort();
        evidence.dedup();
        self.store
            .validate_held_evidence(hold.hold_id(), hold.kind(), hold.binding(), &evidence, now)
            .map_err(Verdict::from_hold)?;
        Ok(evidence)
    }

    /// Stages the proposal at the provisional identity and seals its run; returns the reference and the row's creation time. A byte-identical row already sealed by an earlier attempt of this settlement is adopted; different bytes at the identity conflict.
    fn stage(
        &self,
        run: &CuratorHoldBinding,
        queue_deadline_at: i64,
        payload: ReviewPayload,
        now: i64,
    ) -> Result<(ReviewStagedReference, i64), SettlementError> {
        let identity = provisional_result_identity(&run.subject, run.generation);
        let binding = proposal_binding(self.binding, run);
        let staged = self.store.stage_review_input(ReviewStagingSpec {
            extraction_run_id: identity.extraction_run_id.clone(),
            candidate_id: identity.candidate_id.clone(),
            producer: SETTLEMENT_PRODUCER.to_string(),
            binding: binding.clone(),
            payload: payload.clone(),
            recorded_at: now,
            queue_deadline_at,
        });
        let reference = match staged {
            Ok(reference) => reference,
            Err(ReviewStageError::Refused(ReviewStageRefusal::Changed)) => {
                return Err(SettlementError::ConflictingContent);
            }
            Err(ReviewStageError::Refused(ReviewStageRefusal::Terminal)) => {
                // The run is already sealed: only the same bytes at the identity count as this settlement's result.
                let reference = ReviewStagedReference {
                    database_incarnation_id: run.kernel_incarnation.clone(),
                    candidate_id: identity.candidate_id.clone(),
                    payload_digest: payload.digest().map_err(kernel)?,
                };
                return match self.store.read_review_input(&reference, &binding, now) {
                    Ok(row) => Ok((reference, row.lifecycle.created_at)),
                    Err(ReviewReadError::Refused(ReviewReadRefusal::Changed)) => {
                        Err(SettlementError::ConflictingContent)
                    }
                    Err(error) => Err(kernel(error)),
                };
            }
            Err(other) => return Err(kernel(other)),
        };
        self.store
            .finish_staging_run(
                &identity.extraction_run_id,
                StagingTerminalState::Completed,
                now,
            )
            .map_err(kernel)?;
        let created_at = self
            .store
            .read_review_input(&reference, &binding, now)
            .map_err(kernel)?
            .lifecycle
            .created_at;
        Ok((reference, created_at))
    }

    /// One Kernel envelope moves retention from the execution hold to the review hold owned by `review`, expiring seven days after the result's creation.
    fn transfer_retention(
        &self,
        broker: &EvidenceBroker,
        review: &CuratorHoldBinding,
        created_at: i64,
    ) -> Result<CuratorHold, SettlementError> {
        let binding = broker.binding();
        self.store
            .transfer_execution_to_review(
                &binding.hold_id,
                &binding.hold,
                review,
                created_at.saturating_add(REVIEW_EXPIRY_MAX_MS),
            )
            .map_err(kernel)
    }
}

/// The ledger dates every write at or after its newest event and leaves a completion dated before that for the worker to repeat once its clock has caught up; that claim is still live. Every other conflict means this claim can never complete the receipt.
fn conflict_ends_claim(kind: &str) -> bool {
    kind != "clock_behind"
}

fn retry_later(kind: &str) -> SettlementError {
    SettlementError::Store(format!(
        "completion refused: {kind}; retry once the clock has caught up"
    ))
}

/// The two completions settlement records without staging anything.
enum ContentFree {
    Unknown,
    Abstained(AbstainReason),
}

/// The job's binding under the proposal owner for `run`'s generation.
fn proposal_binding(job: &ReviewBinding, run: &CuratorHoldBinding) -> ReviewBinding {
    ReviewBinding {
        owner: ReviewOwner::Proposal {
            job_id: run.subject.clone(),
            generation: run.generation,
        },
        ..job.clone()
    }
}

/// The model text is render-checked, every citation must name disclosed evidence under a disclosed alias, and the policy dependencies are the broker's, never the model's. The bound payload then passes the Kernel's own payload rules, so a secret in any model-controlled field, or a shape the Kernel would refuse, abstains instead of leaving the receipt in progress behind a staging refusal no retry can pass.
fn bind_dependencies(
    broker: &EvidenceBroker,
    mut proposal: ReviewProposal,
    disclosed: &[String],
) -> Result<ReviewPayload, AbstainReason> {
    for text in proposal.new_text.iter().chain(&proposal.limitations) {
        if check_render(text.as_bytes(), None).is_err() {
            return Err(AbstainReason::Secret);
        }
    }
    let cited: Vec<&str> = proposal
        .support
        .iter()
        .chain(&proposal.contradictions)
        .map(|reference| reference.evidence_id.as_str())
        .collect();
    if cited.iter().any(|evidence_id| {
        disclosed
            .binary_search_by(|d| d.as_str().cmp(evidence_id))
            .is_err()
    }) {
        return Err(AbstainReason::UndisclosedCitation);
    }
    // A span names the alias the model saw the bytes under: it must be an alias this run disclosed, that alias must resolve to the evidence the citation names, and the offsets must lie within one range rendered under it.
    let anchored = |reference: &EvidenceReference| {
        reference.span.as_ref().is_none_or(|span| {
            broker
                .aliases
                .resolve(&span.alias)
                .ok()
                .filter(|(alias, _)| broker.ledger.covers(alias, span.start, span.end))
                .is_some_and(|(_, expectation)| {
                    expectation.evidence_id() == Some(reference.evidence_id.as_str())
                })
        })
    };
    if !proposal
        .support
        .iter()
        .chain(&proposal.contradictions)
        .all(anchored)
    {
        return Err(AbstainReason::UndisclosedCitation);
    }
    let disclosed_inputs: Vec<EvidenceReference> = disclosed
        .iter()
        .map(|evidence_id| EvidenceReference {
            evidence_id: evidence_id.clone(),
            span: None,
        })
        .collect();
    let uncited_disclosed_inputs = disclosed_inputs
        .iter()
        .filter(|input| !cited.contains(&input.evidence_id.as_str()))
        .cloned()
        .collect();
    let mut ancestry: Vec<String> = broker
        .ledger
        .union()
        .members()
        .filter_map(|member| member.owner_id.clone())
        .collect();
    ancestry.sort();
    ancestry.dedup();
    proposal.policy_dependencies = PolicyDependencies {
        question_template: proposal.policy_dependencies.question_template,
        disclosed_inputs,
        uncited_disclosed_inputs,
        ancestry,
    };
    // The Kernel's own payload rules, run here so a proposal that could never be staged completes the receipt instead of failing every retry the same way. The dependencies above are settlement's; anything else these rules reject is the model's.
    let payload = ReviewPayload::Proposal(Box::new(proposal));
    match payload.encode() {
        Err(ReviewStageRefusal::SecretDetected) => Err(AbstainReason::Secret),
        Err(ReviewStageRefusal::Invalid) => Err(AbstainReason::InvalidProposal),
        _ => Ok(payload),
    }
}

/// A revalidation outcome: a durable abstention reason, or a store failure that must not become one.
enum Verdict {
    Abstain(AbstainReason),
    Store(String),
}

impl Verdict {
    /// A resource or store refusal is transient and must not become a durable abstention; every other code says the disclosed input no longer stands as rendered.
    fn from_refusal(code: RefusalCode) -> Self {
        match code {
            RefusalCode::Store
            | RefusalCode::HoldLimit
            | RefusalCode::BatchLimit
            | RefusalCode::InspectionLimit
            | RefusalCode::ByteLimit
            | RefusalCode::BufferLimit
            | RefusalCode::Unavailable => Self::Store(format!("revalidation refused: {code}")),
            RefusalCode::PolicyBlocked => Self::Abstain(AbstainReason::OwnerSensitive),
            RefusalCode::Scope => Self::Abstain(AbstainReason::WrongScope),
            RefusalCode::RenderCheck => Self::Abstain(AbstainReason::Secret),
            // A citation the coordinator could not bind names an alias never disclosed or bytes never shown: the model cited what it did not see, and nothing about the evidence changed.
            RefusalCode::UnknownAlias | RefusalCode::InvalidRange => {
                Self::Abstain(AbstainReason::UndisclosedCitation)
            }
            RefusalCode::ExpectationChanged
            | RefusalCode::OriginRevoked
            | RefusalCode::HoldInvalid
            | RefusalCode::Undecodable
            | RefusalCode::InvalidCursor
            | RefusalCode::InvalidPath
            | RefusalCode::Protected
            | RefusalCode::NotRegularFile
            | RefusalCode::Confinement
            | RefusalCode::Unsupported
            | RefusalCode::NotFound
            | RefusalCode::TooLarge
            | RefusalCode::UnsupportedQuestion => Self::Abstain(AbstainReason::ExpectationChanged),
        }
    }

    fn from_hold(error: CuratorHoldError) -> Self {
        match error {
            CuratorHoldError::Store(error) => Self::Store(error.to_string()),
            CuratorHoldError::Refused(_) => Self::Abstain(AbstainReason::ExpectationChanged),
        }
    }
}

fn store(error: impl std::fmt::Display) -> SettlementError {
    SettlementError::Store(error.to_string())
}

fn kernel(error: impl std::fmt::Display) -> SettlementError {
    SettlementError::Kernel(error.to_string())
}

/// One completed job's content-free outcome, as the receipt alone reports it (Q28: an abstention lists with no Kernel row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewOutcome {
    pub causal_identity: String,
    pub generation: u64,
    pub terminal: CuratorReceiptTerminal,
    pub abstained_reason: Option<AbstainReason>,
    /// Set when the receipt selects a result; content is read separately.
    pub selected: bool,
}

/// One bounded page of completed outcomes and the cursor for the next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewOutcomePage {
    pub outcomes: Vec<ReviewOutcome>,
    pub next: Option<String>,
}

/// A selected proposal whose every selected field and dependency passed the Kernel at the read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedProposal {
    pub reference: ReviewStagedReference,
    pub proposal: ReviewProposal,
    /// The review hold's expiry: the sole source of the review deadline.
    pub review_expires_at: i64,
}

/// Why a selected proposal was not returned. `NotSelected` covers receipts that are missing, in progress, or completed without a selection.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReadRefusal {
    #[error("not_selected")]
    NotSelected,
    /// The receipt belongs to another Memory Store or Kernel incarnation than the live pair.
    #[error("incarnation_mismatch")]
    IncarnationMismatch,
    /// The receipt's selection and generation disagree.
    #[error("selection_mismatch")]
    SelectionMismatch,
    /// The Kernel row does not match the selected fields or is no longer readable.
    #[error("kernel {0}")]
    Kernel(ReviewReadRefusal),
    /// The review hold is gone: the review deadline passed or the hold was released or degraded.
    #[error("review_expired")]
    ReviewExpired,
    /// A disclosed input no longer passes the Kernel's checks.
    #[error("dependency {0}")]
    Dependency(RefusalCode),
    #[error("store {0}")]
    Store(String),
}

/// Completed outcomes for `project`, at most `limit` (capped at [`MAX_RECEIPT_PAGE`]), after `after`. One Memory Store read, released before return.
pub fn list_review_outcomes(
    ledger: &MemoryStore,
    project: &str,
    after: Option<&str>,
    limit: usize,
) -> Result<ReviewOutcomePage, ReadRefusal> {
    let limit = limit.clamp(1, MAX_RECEIPT_PAGE);
    let receipts = ledger
        .list_completed_curator_receipts(project, after, limit)
        .map_err(|error| ReadRefusal::Store(error.to_string()))?;
    let next = (receipts.len() == limit)
        .then(|| {
            receipts
                .last()
                .map(|receipt| receipt.causal_identity.clone())
        })
        .flatten();
    let outcomes = receipts
        .into_iter()
        .filter_map(|receipt| {
            Some(ReviewOutcome {
                causal_identity: receipt.causal_identity,
                generation: receipt.generation,
                terminal: receipt.terminal?,
                abstained_reason: receipt.abstained_reason,
                selected: receipt.selected.is_some(),
            })
        })
        .collect();
    Ok(ReviewOutcomePage { outcomes, next })
}

/// Reads the proposal the job's completed receipt selects. `binding` builds the job's staging binding from the Kernel project digest the selection records; its owner is replaced by the selected generation's proposal owner. A job with no defined binding is refused as the Kernel would refuse any binding invented for it: a scope mismatch.
pub fn read_selected_proposal(
    store: &KernelStore,
    ledger: &MemoryStore,
    project: &str,
    causal_identity: &str,
    binding: impl FnOnce(&str) -> Option<ReviewBinding>,
    now: i64,
) -> Result<SelectedProposal, ReadRefusal> {
    read_selected_proposal_inner(store, ledger, project, causal_identity, binding, now, None)
}

/// [`read_selected_proposal`] with `after_hold_lookup` run between the review-hold lookup and the evidence validation under it, so a test can end the hold inside that window.
#[cfg(any(test, feature = "test-support"))]
pub fn read_selected_proposal_with_hook_for_test(
    store: &KernelStore,
    ledger: &MemoryStore,
    project: &str,
    causal_identity: &str,
    binding: impl FnOnce(&str) -> Option<ReviewBinding>,
    now: i64,
    after_hold_lookup: &dyn Fn(),
) -> Result<SelectedProposal, ReadRefusal> {
    read_selected_proposal_inner(
        store,
        ledger,
        project,
        causal_identity,
        binding,
        now,
        Some(after_hold_lookup),
    )
}

fn read_selected_proposal_inner(
    store: &KernelStore,
    ledger: &MemoryStore,
    project: &str,
    causal_identity: &str,
    binding: impl FnOnce(&str) -> Option<ReviewBinding>,
    now: i64,
    after_hold_lookup: Option<&dyn Fn()>,
) -> Result<SelectedProposal, ReadRefusal> {
    let receipt = ledger
        .lookup_curator_receipt(project, causal_identity)
        .map_err(|error| ReadRefusal::Store(error.to_string()))?
        .ok_or(ReadRefusal::NotSelected)?;
    let live_memstore = ledger
        .curator_store_incarnation()
        .map_err(|error| ReadRefusal::Store(error.to_string()))?;
    // The Memory Store is released; everything below is the Kernel's.
    let (selected_generation, selection) = match (receipt.terminal, receipt.selected) {
        (Some(CuratorReceiptTerminal::Complete), Some(selected)) => selected,
        _ => return Err(ReadRefusal::NotSelected),
    };
    if receipt.database_incarnation_id != live_memstore {
        return Err(ReadRefusal::IncarnationMismatch);
    }
    if selected_generation != receipt.generation
        || selection.candidate_id
            != provisional_result_identity(causal_identity, receipt.generation).candidate_id
    {
        return Err(ReadRefusal::SelectionMismatch);
    }
    let reference = ReviewStagedReference {
        database_incarnation_id: receipt.kernel_incarnation_id.clone(),
        candidate_id: selection.candidate_id.clone(),
        payload_digest: selection.payload_digest.clone(),
    };
    let run = CuratorHoldBinding {
        project_digest: selection.project_digest.clone(),
        kernel_incarnation: receipt.kernel_incarnation_id.clone(),
        memstore_incarnation: receipt.database_incarnation_id.clone(),
        subject: causal_identity.to_string(),
        generation: receipt.generation,
    };
    let binding = binding(&selection.project_digest)
        .ok_or(ReadRefusal::Kernel(ReviewReadRefusal::ScopeMismatch))?;
    let row = store
        .read_review_input(&reference, &proposal_binding(&binding, &run), now)
        .map_err(|error| match error {
            ReviewReadError::Refused(ReviewReadRefusal::IncarnationMismatch) => {
                ReadRefusal::IncarnationMismatch
            }
            // Selection moved the row's deadline to the review expiry, so a lapsed deadline on a selected row is the review window ending.
            ReviewReadError::Refused(ReviewReadRefusal::Expired) => ReadRefusal::ReviewExpired,
            ReviewReadError::Refused(refusal) => ReadRefusal::Kernel(refusal),
            ReviewReadError::Invalid => ReadRefusal::SelectionMismatch,
            ReviewReadError::Store(error) => ReadRefusal::Store(error.to_string()),
        })?;
    // Review rows are classified `Sensitive` by construction; a row the Kernel classifies `Secret` (including a stored class this build does not recognize) is refused as a local read rather than returned on the strength of that construction.
    if row.sensitivity == kernel::Sensitivity::Secret {
        return Err(ReadRefusal::Dependency(RefusalCode::PolicyBlocked));
    }
    let ReviewPayload::Proposal(proposal) = row.payload else {
        return Err(ReadRefusal::Kernel(ReviewReadRefusal::DecodeRefused));
    };
    let review = CuratorHoldBinding {
        subject: selection.candidate_id.clone(),
        ..run
    };
    let hold = store
        .lookup_review_hold(&review, now)
        .map_err(|error| match error {
            CuratorHoldError::Store(error) => ReadRefusal::Store(error.to_string()),
            CuratorHoldError::Refused(_) => ReadRefusal::ReviewExpired,
        })?
        .ok_or(ReadRefusal::ReviewExpired)?;
    if let Some(hook) = after_hold_lookup {
        hook();
    }
    // Every disclosed input, cited or not, must still be live under the review hold and eligible for a local reader.
    let evidence: Vec<String> = proposal
        .policy_dependencies
        .disclosed_inputs
        .iter()
        .map(|input| input.evidence_id.clone())
        .collect();
    let held = store
        .validate_held_evidence(
            &hold.hold_id,
            CuratorHoldKind::Review,
            &review,
            &evidence,
            now,
        )
        .map_err(|error| match error {
            CuratorHoldError::Store(error) => ReadRefusal::Store(error.to_string()),
            // The hold ended between the lookup above and this validation: the same review expiry the lookup would have reported a moment later, since the lookup excludes released, purge-degraded, and expired holds alike.
            CuratorHoldError::Refused(
                CuratorHoldRefusal::Missing
                | CuratorHoldRefusal::Released
                | CuratorHoldRefusal::PurgeDegraded
                | CuratorHoldRefusal::Expired,
            ) => ReadRefusal::ReviewExpired,
            CuratorHoldError::Refused(_) => ReadRefusal::Dependency(RefusalCode::HoldInvalid),
        })?;
    for fact in held {
        let facts = store
            .artifact_egress_facts(
                &ArtifactHandle {
                    digest: fact.artifact_digest,
                    evidence_id: fact.evidence_id,
                },
                ArtifactDestination::Local,
            )
            .map_err(|error| ReadRefusal::Store(error.to_string()))?;
        if facts.eligibility != ArtifactEligibility::Allowed {
            return Err(ReadRefusal::Dependency(RefusalCode::PolicyBlocked));
        }
    }
    Ok(SelectedProposal {
        reference,
        proposal: *proposal,
        review_expires_at: hold.expires_at,
    })
}

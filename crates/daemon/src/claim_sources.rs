//! `ClaimMaterializer` emits nonempty `decision_summary` and `rationale` representations of a decision as `canonical_claims` occurrences, and the `summary` of a positive-category decision in `MEMORY_DOMAIN_ID` as a `promoted_memory` occurrence. The two classes keep distinct occurrence identities when their bytes are equal.
//!
//! Identity is the decision's object id and canonical source revision. The memory domain is selected by its stable id, never its name.
//!
//! The materializer consumes the kernel commit log as a registered outbox consumer: the next episode resumes from the durable checkpoint, publication replays by receipt, and retiring an already-retired descriptor is a no-op. Correction, retirement, and approval revocation retire the predecessor's descriptors before any successor's are published, so no snapshot serves both revisions; the retirements reach the projection as tombstones through the shared export. The checkpoint advances only after every decision of a page has been published or retired.
//!
//! A descriptor's admission records the decision's admission classes as they stood in the publishing transaction, with the decision as `trigger_object_id`. The materializer consumes decision registry rows only; an admission-only disposition of the decision (quarantine, rejection, contradiction, staleness) is recorded on the decision and not on its descriptors, so a reader that must honor it resolves eligibility through the decision the descriptor names.

use kernel::source_identity::{
    Occurrence, OccurrenceClass, OccurrenceRefusal, encode_preserving_span, identity_digest,
};
use std::num::{NonZeroU64, NonZeroUsize};

use kernel::applicability::EvalBudget;
use kernel::{
    APPROVAL_REVOKE_KIND, CommitIntent, CommitPageBounds, CommitReadIncarnation,
    DECISION_CHANGE_KINDS, DECISION_CORRECT_KIND, DECISION_INSERT_KIND, DECISION_RETIRE_KIND,
    DecisionRow, KernelError, KernelStore, ObjectRow, ProviderEgress, Sensitivity,
    descriptor_object_id,
};
use serde::Deserialize;

use crate::canonical_memory::MEMORY_DOMAIN_ID;
use crate::commit_stream::{CommitStreamBlocked, CommitWalk, drive_commit_pages, outcome_unknown};
use crate::harness_sources::{
    CANONICAL_ROLE, PublishError, Representation, SourcePublisher, SourceUnit,
};
use crate::memory_render::is_positive_memory_category;

/// The outbox consumer the materializer reads and acknowledges under.
pub const CLAIM_CONSUMER: &str = "claim-sources";

const PRODUCER: &str = "eidnara-daemon/claim-sources";

const _: () = assert!(
    DECISION_CHANGE_KINDS.len() == 5,
    "retire_change and publish_change name a disposition for each decision change kind"
);

/// The registry facts of one decision that its occurrences are keyed by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimSubject {
    pub object_id: String,
    pub domain_id: String,
    /// The canonical source revision, the occurrence revision.
    pub revision: i64,
    pub sensitivity: Sensitivity,
}

impl ClaimSubject {
    /// # Errors
    ///
    /// Returns [`KernelError::InvalidInput`] when `object` is not a decision.
    pub fn from_object(object: &ObjectRow) -> Result<Self, KernelError> {
        if object.object_kind != "decision" {
            return Err(KernelError::InvalidInput);
        }
        Ok(Self {
            object_id: object.object_id.clone(),
            domain_id: object.domain_id.clone(),
            revision: object.source_revision,
            sensitivity: object.sensitivity,
        })
    }
}

/// Why a decision yields no occurrence of one class or representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimExclusion {
    /// The representation's text is empty, so the class has one fewer representation rather than an empty row.
    EmptyRepresentation(Representation),
    OutsideMemoryDomain,
    NegativeCategory,
    /// A decision without a project scope serves no project; publishing it would retain bytes and queue work no read can reach.
    Unscoped,
    /// The object id is not a well-formed source-identity value, so the kernel refuses every descriptor keyed by it; none exists to publish or retire.
    MalformedIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClaimUnits {
    pub units: Vec<SourceUnit>,
    pub exclusions: Vec<ClaimExclusion>,
}

/// The two classes and their representations, in publication order.
const REPRESENTATIONS: [(OccurrenceClass, Representation); 3] = [
    (
        OccurrenceClass::CanonicalClaims,
        Representation::DecisionSummary,
    ),
    (OccurrenceClass::CanonicalClaims, Representation::Rationale),
    (OccurrenceClass::PromotedMemory, Representation::Summary),
];

fn unit(
    subject: &ClaimSubject,
    class: OccurrenceClass,
    representation: Representation,
    text: &str,
) -> SourceUnit {
    SourceUnit {
        class,
        identity: vec![(class.identity_fields()[0], subject.object_id.clone())],
        revision: subject.revision.to_string(),
        representation,
        text: text.to_owned(),
        role: CANONICAL_ROLE.to_owned(),
    }
}

/// # Errors
///
/// Returns [`KernelError::InvalidInput`] when `subject` and `decision` name different objects.
pub fn claim_units(
    subject: &ClaimSubject,
    decision: &DecisionRow,
) -> Result<ClaimUnits, KernelError> {
    if subject.object_id != decision.object_id {
        return Err(KernelError::InvalidInput);
    }
    let mut out = ClaimUnits::default();
    if descriptor_ids(subject).is_err() {
        out.exclusions.push(ClaimExclusion::MalformedIdentity);
        return Ok(out);
    }
    if decision.scope_id.is_none() {
        out.exclusions.push(ClaimExclusion::Unscoped);
        return Ok(out);
    }
    let promoted = if subject.domain_id != MEMORY_DOMAIN_ID {
        out.exclusions.push(ClaimExclusion::OutsideMemoryDomain);
        false
    } else if !is_positive_memory_category(&decision.decision_kind) {
        out.exclusions.push(ClaimExclusion::NegativeCategory);
        false
    } else {
        true
    };
    for (class, representation) in REPRESENTATIONS {
        let text = match representation {
            Representation::Rationale => &decision.payload.rationale,
            _ => &decision.payload.summary,
        };
        if class == OccurrenceClass::PromotedMemory && !promoted {
            continue;
        }
        if text.is_empty() {
            out.exclusions
                .push(ClaimExclusion::EmptyRepresentation(representation));
            continue;
        }
        out.units.push(unit(subject, class, representation, text));
    }
    Ok(out)
}

/// Every descriptor object id the subject could have published, whether or not each representation existed, from the kernel's own identity encoding.
///
/// # Errors
///
/// Returns the refusal when `subject.object_id` is not a well-formed identity value.
fn descriptor_ids(subject: &ClaimSubject) -> Result<Vec<String>, OccurrenceRefusal> {
    let revision = subject.revision.to_string();
    REPRESENTATIONS
        .iter()
        .map(|(class, representation)| {
            let encoded = encode_preserving_span(&Occurrence {
                class: class.code(),
                identity: &[(class.identity_fields()[0], &subject.object_id)],
                revision: &revision,
                representation: representation.as_str(),
                span: None,
            })?;
            Ok(descriptor_object_id(&encoded.lineage_id, &revision))
        })
        .collect()
}

/// Why an episode stopped short of its target. Nothing durable moved for the refused commit; the next episode starts from the same checkpoint.
#[derive(Debug)]
pub enum ClaimBlocked {
    /// The claim consumer is not registered.
    UnknownConsumer,
    Stream(CommitStreamBlocked),
    /// The decision cannot be read at that commit, or its change payload is not the kernel's shape.
    DecisionUnreadable {
        object_id: String,
        commit_seq: i64,
    },
    /// A decision change kind this materializer has no disposition for; skipping it could leave the projection stale.
    UnknownChangeKind {
        object_id: String,
        commit_seq: i64,
        change_kind: String,
    },
    Publish {
        object_id: String,
        error: PublishError,
    },
    Retire {
        object_id: String,
        error: KernelError,
    },
    /// The acknowledgement reply was lost and the durable checkpoint still sits below the published page.
    AcknowledgementUnresolved {
        through: i64,
        checkpoint: Option<i64>,
    },
}

impl From<CommitStreamBlocked> for ClaimBlocked {
    fn from(blocked: CommitStreamBlocked) -> Self {
        Self::Stream(blocked)
    }
}

#[derive(Debug)]
pub enum MaterializationEnd {
    ReachedTarget,
    /// The slice finished its one page; more bootstrap decisions or retained commits remain.
    Continues,
    /// The slice's budget ran out. Nothing past the last completed decision or acknowledged page moved, and the next slice repeats the rest.
    Exhausted,
    Blocked(ClaimBlocked),
}

/// Bounds of one maintenance slice: a bootstrap page examines at most `decisions` decision rows carrying at most `decision_bytes` of payload, and a change page is one commit page under `commits`.
#[derive(Debug, Clone, Copy)]
pub struct ClaimSliceBounds {
    pub decisions: NonZeroUsize,
    pub decision_bytes: NonZeroU64,
    pub commits: CommitPageBounds,
}

/// The runner's position. Bootstrap progress is owned by the process: after a restart the scan begins again from the first decision, and the change history retained from the consumer's registration covers every change the scan races.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ClaimProgress {
    #[default]
    Unregistered,
    /// Scanning current decisions after `after` in object-id order.
    Bootstrap { after: Option<String> },
    /// The scan is complete; retained commits are applied from the durable checkpoint.
    Changes,
}

#[derive(Debug)]
pub struct MaterializationReport {
    pub target: i64,
    /// Current decisions a bootstrap page published or replayed.
    pub bootstrapped: usize,
    pub acknowledged_through: i64,
    pub commits_consumed: usize,
    /// Descriptors this episode published for the first time.
    pub published: usize,
    /// Descriptors whose receipt already existed, so the kernel wrote nothing.
    pub replayed: usize,
    /// Descriptor objects invalidated by this episode's retirements, including ones a lost reply had already retired.
    pub retired: usize,
    /// Exclusions per decision object, in commit order.
    pub exclusions: Vec<(String, ClaimExclusion)>,
    pub end: MaterializationEnd,
}

#[derive(Deserialize)]
struct DecisionChange {
    change_kind: String,
    object: ChangedObject,
    #[serde(default)]
    replaced_object_id: Option<String>,
}

#[derive(Deserialize)]
struct ChangedObject {
    domain_id: String,
}

/// Which reply an episode loses, so a test can watch the reconciliation discriminate. Only the test-support entry point can set one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpisodeFault {
    /// The acknowledgement commits, then its reply arrives as a kernel I/O failure.
    LoseAcknowledgementReply,
    /// The page is published but never acknowledged, as a crash between the two would leave it.
    SkipAcknowledgement,
    /// The acknowledgement fails with an outcome-unknown error before it commits.
    FailAcknowledgement,
}

pub struct ClaimMaterializer<'a> {
    kernel: &'a KernelStore,
    egress: ProviderEgress,
    fault: Option<EpisodeFault>,
}

enum Stop {
    Blocked(ClaimBlocked),
    Failed(KernelError),
    /// A slice applied its one page.
    PageDone,
}

impl From<ClaimBlocked> for Stop {
    fn from(blocked: ClaimBlocked) -> Self {
        Stop::Blocked(blocked)
    }
}

impl From<CommitStreamBlocked> for Stop {
    fn from(blocked: CommitStreamBlocked) -> Self {
        Stop::Blocked(blocked.into())
    }
}

impl From<KernelError> for Stop {
    fn from(error: KernelError) -> Self {
        Stop::Failed(error)
    }
}

impl<'a> ClaimMaterializer<'a> {
    pub fn new(kernel: &'a KernelStore, egress: ProviderEgress) -> Self {
        Self {
            kernel,
            egress,
            fault: None,
        }
    }

    /// Registers the consumer and acknowledges through the registration commit, so the first episode starts after every commit that preceded registration; decisions committed before it are not materialized. A repeated call replays the receipt and acknowledges nothing the checkpoint has already passed.
    ///
    /// # Errors
    ///
    /// Returns the kernel's error when the registration commit or its acknowledgement fails.
    pub fn register(kernel: &KernelStore, now: i64) -> Result<(), KernelError> {
        Self::register_within(kernel, None, now)
    }

    fn register_within(
        kernel: &KernelStore,
        budget: Option<&EvalBudget>,
        now: i64,
    ) -> Result<(), KernelError> {
        let intent = CommitIntent {
            producer: PRODUCER.to_owned(),
            operation_key: format!("register:{CLAIM_CONSUMER}"),
            request_digest: identity_digest(CLAIM_CONSUMER.as_bytes()),
            actor: CANONICAL_ROLE.to_owned(),
            cause: "claim consumer registration".to_owned(),
        };
        let operation = |envelope: &mut kernel::Envelope<'_>| {
            envelope.register_outbox_consumer(CLAIM_CONSUMER, now)?;
            Ok(String::new())
        };
        let receipt = match budget {
            Some(budget) => kernel.commit_within_budget(budget, intent, operation)?,
            None => kernel.commit(intent, operation)?,
        };
        // The kernel registers a consumer at the oldest retained outbox commit. A replayed receipt carries the first registration's commit, so the guard keeps a later call from moving an advanced checkpoint.
        let checkpoint = match budget {
            Some(budget) => {
                kernel.outbox_consumer_checkpoint_within_budget(budget, CLAIM_CONSUMER)?
            }
            None => kernel.outbox_consumer_checkpoint(CLAIM_CONSUMER)?,
        }
        .ok_or(KernelError::NotFound)?;
        if checkpoint < receipt.commit_seq {
            kernel.acknowledge_outbox(CLAIM_CONSUMER, receipt.commit_seq, now)?;
        }
        Ok(())
    }

    /// Captures a target and processes complete commits from the consumer checkpoint toward it one bounded page at a time, acknowledging each page after its decisions are published or retired.
    ///
    /// `now` is Unix-epoch milliseconds, recorded as the descriptors' observation time and the acknowledgement's `updated_at`.
    /// Page reads carry no budget: kernel waits block as they do for every other unbudgeted caller.
    ///
    /// # Errors
    ///
    /// Returns the kernel's error when a read or commit fails without a durable fact to reconcile against. Refusals end the episode in the report.
    pub fn run_episode(
        &mut self,
        bounds: CommitPageBounds,
        now: i64,
    ) -> Result<MaterializationReport, KernelError> {
        self.fault = None;
        self.run_episode_inner(bounds, now)
    }

    /// [`Self::run_episode`] under one injected fault.
    #[cfg(feature = "test-support")]
    pub fn run_episode_with_fault_for_test(
        &mut self,
        bounds: CommitPageBounds,
        now: i64,
        fault: EpisodeFault,
    ) -> Result<MaterializationReport, KernelError> {
        self.fault = Some(fault);
        self.run_episode_inner(bounds, now)
    }

    /// Runs one bounded maintenance slice from `progress` and advances it: the first slice registers the consumer, bootstrap slices publish one page of current decisions, and change slices apply one retained commit page. The report ends `ReachedTarget` once the scan is complete and the checkpoint sits at the captured tip.
    ///
    /// The consumer is registered before the scan reads any decision, so every change the scan races is retained for replay. The scan publishes only decisions created at or before the durable checkpoint; a later decision is published by the replay of its own commit, which retires the predecessor it replaced first. Every kernel read and acknowledgement waits within `budget`, and the slice stops between decisions or commits once `budget` is exhausted.
    ///
    /// # Errors
    ///
    /// Returns the kernel's error when a read or commit fails without a durable fact to reconcile against. Refusals and an exhausted budget end the slice in the report.
    pub fn run_slice(
        &mut self,
        progress: &mut ClaimProgress,
        bounds: ClaimSliceBounds,
        budget: &EvalBudget,
        now: i64,
    ) -> Result<MaterializationReport, KernelError> {
        self.fault = None;
        self.run_slice_inner(progress, bounds, budget, now)
    }

    /// [`Self::run_slice`] under one injected fault; a change slice honors [`EpisodeFault::SkipAcknowledgement`].
    #[cfg(feature = "test-support")]
    pub fn run_slice_with_fault_for_test(
        &mut self,
        progress: &mut ClaimProgress,
        bounds: ClaimSliceBounds,
        budget: &EvalBudget,
        now: i64,
        fault: EpisodeFault,
    ) -> Result<MaterializationReport, KernelError> {
        self.fault = Some(fault);
        self.run_slice_inner(progress, bounds, budget, now)
    }

    fn run_slice_inner(
        &mut self,
        progress: &mut ClaimProgress,
        bounds: ClaimSliceBounds,
        budget: &EvalBudget,
        now: i64,
    ) -> Result<MaterializationReport, KernelError> {
        let mut report = MaterializationReport {
            target: 0,
            bootstrapped: 0,
            acknowledged_through: 0,
            commits_consumed: 0,
            published: 0,
            replayed: 0,
            retired: 0,
            exclusions: Vec::new(),
            end: MaterializationEnd::Continues,
        };
        let outcome = match progress {
            ClaimProgress::Unregistered => Self::register_within(self.kernel, Some(budget), now)
                .map(|()| {
                    *progress = ClaimProgress::Bootstrap { after: None };
                }),
            ClaimProgress::Bootstrap { after } => self
                .bootstrap_page(after.clone(), bounds, budget, now, &mut report)
                .map(|next| *progress = next),
            ClaimProgress::Changes => {
                match self.change_page(bounds.commits, budget, now, &mut report) {
                    Ok(()) | Err(Stop::PageDone) => Ok(()),
                    Err(Stop::Blocked(blocked)) => {
                        report.end = MaterializationEnd::Blocked(blocked);
                        Ok(())
                    }
                    Err(Stop::Failed(error)) => Err(error),
                }
            }
        };
        match outcome {
            Ok(()) => Ok(report),
            Err(KernelError::Deadline) if budget.is_exhausted() => {
                report.end = MaterializationEnd::Exhausted;
                Ok(report)
            }
            Err(error) => Err(error),
        }
    }

    /// Publishes one page of current decisions and returns where the scan stands after it.
    fn bootstrap_page(
        &self,
        after: Option<String>,
        bounds: ClaimSliceBounds,
        budget: &EvalBudget,
        now: i64,
        report: &mut MaterializationReport,
    ) -> Result<ClaimProgress, KernelError> {
        let Some(checkpoint) = self
            .kernel
            .outbox_consumer_checkpoint_within_budget(budget, CLAIM_CONSUMER)?
        else {
            report.end = MaterializationEnd::Blocked(ClaimBlocked::UnknownConsumer);
            return Ok(ClaimProgress::Unregistered);
        };
        report.acknowledged_through = checkpoint;
        let tip = self.kernel.tip_within_budget(budget)?;
        report.target = tip;
        let page = self.kernel.decision_page_within_budget(
            tip,
            after.as_deref(),
            bounds.decisions,
            bounds.decision_bytes,
            budget,
        )?;
        let mut done = after;
        for (object, decision) in &page.decisions {
            if budget.is_exhausted() {
                report.end = MaterializationEnd::Exhausted;
                return Ok(ClaimProgress::Bootstrap { after: done });
            }
            if object.created_commit_seq <= checkpoint {
                let subject = ClaimSubject::from_object(object)?;
                match self.publish_units(&subject, decision, now, report) {
                    Ok(()) => report.bootstrapped += 1,
                    Err(Stop::Blocked(blocked)) => {
                        report.end = MaterializationEnd::Blocked(blocked);
                        return Ok(ClaimProgress::Bootstrap { after: done });
                    }
                    Err(Stop::Failed(error)) => return Err(error),
                    Err(Stop::PageDone) => unreachable!("publication never ends a page"),
                }
            }
            done = Some(object.object_id.clone());
        }
        Ok(match page.next {
            Some(next) => ClaimProgress::Bootstrap { after: Some(next) },
            None => ClaimProgress::Changes,
        })
    }

    /// Applies and acknowledges one retained commit page from the durable checkpoint toward a captured target.
    fn change_page(
        &self,
        commits: CommitPageBounds,
        budget: &EvalBudget,
        now: i64,
        report: &mut MaterializationReport,
    ) -> Result<(), Stop> {
        let target = self
            .kernel
            .capture_commit_read_target_within_budget(budget)?;
        report.target = target.through_commit;
        let Some(checkpoint) = self
            .kernel
            .outbox_consumer_checkpoint_within_budget(budget, CLAIM_CONSUMER)?
        else {
            return Err(ClaimBlocked::UnknownConsumer.into());
        };
        report.acknowledged_through = checkpoint;
        if checkpoint >= target.through_commit {
            report.end = MaterializationEnd::ReachedTarget;
            return Ok(());
        }
        let outcome = drive_commit_pages(
            self.kernel,
            CommitWalk {
                budget: Some(budget.clone()),
                consumer_id: CLAIM_CONSUMER,
                incarnation: target.incarnation,
                now,
                after: checkpoint,
                target: target.through_commit,
                bounds: commits,
            },
            |page| {
                self.apply_page(page, now, report, Some(budget))?;
                if budget.is_exhausted() {
                    return Err(KernelError::Deadline.into());
                }
                if self.fault == Some(EpisodeFault::SkipAcknowledgement) {
                    return Err(Stop::PageDone);
                }
                self.acknowledge_within(budget, page.through, now, target.incarnation)?;
                report.acknowledged_through = page.through;
                Err(Stop::PageDone)
            },
        );
        match outcome {
            Ok(()) | Err(Stop::PageDone) => {
                if report.acknowledged_through >= target.through_commit {
                    report.end = MaterializationEnd::ReachedTarget;
                }
                Ok(())
            }
            Err(stop) => Err(stop),
        }
    }

    fn run_episode_inner(
        &mut self,
        bounds: CommitPageBounds,
        now: i64,
    ) -> Result<MaterializationReport, KernelError> {
        let target = self.kernel.capture_commit_read_target()?;
        let mut report = MaterializationReport {
            target: target.through_commit,
            bootstrapped: 0,
            acknowledged_through: 0,
            commits_consumed: 0,
            published: 0,
            replayed: 0,
            retired: 0,
            exclusions: Vec::new(),
            end: MaterializationEnd::ReachedTarget,
        };
        let checkpoint = match self.kernel.outbox_consumer_checkpoint(CLAIM_CONSUMER)? {
            Some(checkpoint) => checkpoint,
            None => {
                report.end = MaterializationEnd::Blocked(ClaimBlocked::UnknownConsumer);
                return Ok(report);
            }
        };
        report.acknowledged_through = checkpoint;
        let outcome = drive_commit_pages(
            self.kernel,
            CommitWalk {
                budget: None,
                consumer_id: CLAIM_CONSUMER,
                incarnation: target.incarnation,
                now,
                after: checkpoint,
                target: target.through_commit,
                bounds,
            },
            |page| {
                self.apply_page(page, now, &mut report, None)?;
                if self.fault == Some(EpisodeFault::SkipAcknowledgement) {
                    return Ok(());
                }
                self.acknowledge(page.through, now)?;
                report.acknowledged_through = page.through;
                Ok::<(), Stop>(())
            },
        );
        match outcome {
            Ok(()) => Ok(report),
            Err(Stop::Blocked(blocked)) => {
                report.end = MaterializationEnd::Blocked(blocked);
                Ok(report)
            }
            Err(Stop::Failed(error)) => Err(error),
            Err(Stop::PageDone) => unreachable!("an episode handler never ends its walk early"),
        }
    }

    /// Retires and publishes every decision change of `page` in commit order. A `budget` that runs out stops the page between commits; the page is then neither acknowledged nor complete, and its replay is idempotent.
    fn apply_page(
        &self,
        page: &kernel::CommitPage,
        now: i64,
        report: &mut MaterializationReport,
        budget: Option<&EvalBudget>,
    ) -> Result<(), Stop> {
        for commit in &page.commits {
            if budget.is_some_and(EvalBudget::is_exhausted) {
                return Err(KernelError::Deadline.into());
            }
            let changes = commit
                .rows
                .iter()
                .filter(|row| row.object_kind == "decision")
                .map(|row| Ok((row, self.parse_change(row, commit.commit_seq)?)))
                .collect::<Result<Vec<_>, Stop>>()?;
            // Every predecessor a commit invalidates is retired before any successor in it is published, whatever order the rows were written in, so no commit holds a predecessor beside its successor.
            for (row, change) in &changes {
                self.retire_change(row, change, commit.commit_seq, report)?;
            }
            for (row, change) in &changes {
                self.publish_change(row, change, commit.commit_seq, now, report)?;
            }
            report.commits_consumed += 1;
        }
        Ok(())
    }

    /// A change kind this materializer does not name blocks before any row of its commit is applied.
    fn parse_change(
        &self,
        row: &kernel::OutboxEntry,
        commit_seq: i64,
    ) -> Result<DecisionChange, Stop> {
        let change: DecisionChange =
            serde_json::from_slice(&row.payload).map_err(|_| ClaimBlocked::DecisionUnreadable {
                object_id: row.object_id.clone(),
                commit_seq,
            })?;
        if !DECISION_CHANGE_KINDS.contains(&change.change_kind.as_str()) {
            return Err(ClaimBlocked::UnknownChangeKind {
                object_id: row.object_id.clone(),
                commit_seq,
                change_kind: change.change_kind,
            }
            .into());
        }
        Ok(change)
    }

    /// A correction retires the object it replaced; a retirement or approval revocation retires the object itself.
    fn retire_change(
        &self,
        row: &kernel::OutboxEntry,
        change: &DecisionChange,
        commit_seq: i64,
        report: &mut MaterializationReport,
    ) -> Result<(), Stop> {
        match change.change_kind.as_str() {
            DECISION_INSERT_KIND | DECISION_CORRECT_KIND => match &change.replaced_object_id {
                Some(replaced) => self.retire_decision(replaced, commit_seq, report),
                None => Ok(()),
            },
            DECISION_RETIRE_KIND | APPROVAL_REVOKE_KIND => {
                self.retire_decision(&row.object_id, commit_seq, report)
            }
            _ => Ok(()),
        }
    }

    /// An insert or correction publishes the object's units; an event append changes no text and produces nothing.
    fn publish_change(
        &self,
        row: &kernel::OutboxEntry,
        change: &DecisionChange,
        commit_seq: i64,
        now: i64,
        report: &mut MaterializationReport,
    ) -> Result<(), Stop> {
        match change.change_kind.as_str() {
            DECISION_INSERT_KIND | DECISION_CORRECT_KIND => {
                let subject = ClaimSubject {
                    object_id: row.object_id.clone(),
                    domain_id: change.object.domain_id.clone(),
                    revision: row.source_revision,
                    sensitivity: row.sensitivity,
                };
                self.publish_decision(&subject, commit_seq, now, report)
            }
            _ => Ok(()),
        }
    }

    fn publish_decision(
        &self,
        subject: &ClaimSubject,
        commit_seq: i64,
        now: i64,
        report: &mut MaterializationReport,
    ) -> Result<(), Stop> {
        let Some(decision) = self.decision_at(&subject.object_id, commit_seq)? else {
            // The commit that changed the decision also invalidated it; its descriptors, if any, are retired by that commit's own rows.
            return Ok(());
        };
        self.publish_units(subject, &decision, now, report)
    }

    fn publish_units(
        &self,
        subject: &ClaimSubject,
        decision: &DecisionRow,
        now: i64,
        report: &mut MaterializationReport,
    ) -> Result<(), Stop> {
        let units = claim_units(subject, decision)?;
        report.exclusions.extend(
            units
                .exclusions
                .iter()
                .map(|exclusion| (subject.object_id.clone(), *exclusion)),
        );
        let publisher = SourcePublisher {
            kernel: self.kernel,
            domain_id: &subject.domain_id,
            scope_id: decision.scope_id.as_deref(),
            egress: self.egress,
            sensitivity: subject.sensitivity,
        };
        for unit in &units.units {
            let published = publisher.publish(unit, now).map_err(|error| {
                Stop::Blocked(ClaimBlocked::Publish {
                    object_id: subject.object_id.clone(),
                    error,
                })
            })?;
            if published.replayed {
                report.replayed += 1;
            } else {
                report.published += 1;
            }
        }
        Ok(())
    }

    /// Retires every live descriptor the decision could have published. Identity depends on the registry row alone, so the decision's text is not read; a representation that was never published has no live descriptor and is skipped.
    fn retire_decision(
        &self,
        object_id: &str,
        commit_seq: i64,
        report: &mut MaterializationReport,
    ) -> Result<(), Stop> {
        let (_, states) = self
            .kernel
            .object_states(std::slice::from_ref(&object_id.to_owned()))?;
        let Some(state) = states.into_iter().next().flatten() else {
            return Err(ClaimBlocked::DecisionUnreadable {
                object_id: object_id.to_owned(),
                commit_seq,
            }
            .into());
        };
        // An identity the encoding refuses was refused at publication too, so no descriptor of it exists.
        let Ok(ids) = descriptor_ids(&ClaimSubject::from_object(&state.object)?) else {
            return Ok(());
        };
        let receipt = self.kernel.commit(
            CommitIntent {
                producer: PRODUCER.to_owned(),
                operation_key: format!("claim-retire:{object_id}:{commit_seq}"),
                request_digest: identity_digest(ids.join("\u{1f}").as_bytes()),
                actor: CANONICAL_ROLE.to_owned(),
                cause: "claim descriptor retirement".to_owned(),
            },
            |envelope| {
                for id in &ids {
                    if envelope
                        .object_state(id)?
                        .is_some_and(|state| state.object.invalidated_commit_seq.is_none())
                    {
                        envelope.retire_observation(id)?;
                    }
                }
                Ok(String::new())
            },
        );
        if let Err(error) = receipt {
            return Err(ClaimBlocked::Retire {
                object_id: object_id.to_owned(),
                error,
            }
            .into());
        }
        // Counted from durable state, so a replayed receipt reports what the first commit retired.
        let (_, states) = self.kernel.object_states(&ids)?;
        report.retired += states
            .iter()
            .filter(|state| {
                state
                    .as_ref()
                    .is_some_and(|state| state.object.invalidated_commit_seq.is_some())
            })
            .count();
        Ok(())
    }

    /// `None` when no decision row is visible at `commit_seq` because the commit itself invalidated the object: it was created and invalidated together, or an existing survivor absorbed a fold and was then retired or corrected. The same commit carries the row that retires its descriptors.
    fn decision_at(&self, object_id: &str, commit_seq: i64) -> Result<Option<DecisionRow>, Stop> {
        let mut rows = self
            .kernel
            .decisions_for_objects_as_of(std::slice::from_ref(&object_id.to_owned()), commit_seq)?;
        if let Some(row) = rows.pop() {
            return Ok(Some(row));
        }
        let (_, states) = self
            .kernel
            .object_states(std::slice::from_ref(&object_id.to_owned()))?;
        match states.into_iter().next().flatten() {
            Some(state) if state.object.invalidated_commit_seq == Some(commit_seq) => Ok(None),
            _ => Err(ClaimBlocked::DecisionUnreadable {
                object_id: object_id.to_owned(),
                commit_seq,
            }
            .into()),
        }
    }

    /// [`Self::acknowledge`] waiting within `budget` under the captured incarnation; a `Deadline` refusal is definite and leaves the page for the next slice.
    fn acknowledge_within(
        &self,
        budget: &EvalBudget,
        through: i64,
        now: i64,
        incarnation: CommitReadIncarnation,
    ) -> Result<(), Stop> {
        match self.kernel.acknowledge_outbox_within_budget(
            budget,
            CLAIM_CONSUMER,
            through,
            now,
            incarnation,
        ) {
            Ok(()) => Ok(()),
            Err(error) if outcome_unknown(error) => {
                let checkpoint = self
                    .kernel
                    .outbox_consumer_checkpoint_within_budget(budget, CLAIM_CONSUMER)?;
                if checkpoint.is_none_or(|checkpoint| checkpoint < through) {
                    return Err(ClaimBlocked::AcknowledgementUnresolved {
                        through,
                        checkpoint,
                    }
                    .into());
                }
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }

    /// A reply lost after the kernel may have committed is reconciled from the durable checkpoint.
    fn acknowledge(&self, through: i64, now: i64) -> Result<(), Stop> {
        let mut acknowledged = match self.fault {
            Some(EpisodeFault::FailAcknowledgement) => Err(KernelError::Io),
            _ => self.kernel.acknowledge_outbox(CLAIM_CONSUMER, through, now),
        };
        if self.fault == Some(EpisodeFault::LoseAcknowledgementReply) && acknowledged.is_ok() {
            acknowledged = Err(KernelError::Io);
        }
        match acknowledged {
            Ok(()) => Ok(()),
            Err(error) if outcome_unknown(error) => {
                let checkpoint = self.kernel.outbox_consumer_checkpoint(CLAIM_CONSUMER)?;
                if checkpoint.is_none_or(|checkpoint| checkpoint < through) {
                    return Err(ClaimBlocked::AcknowledgementUnresolved {
                        through,
                        checkpoint,
                    }
                    .into());
                }
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }
}

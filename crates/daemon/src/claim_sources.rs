//! `ClaimMaterializer` emits nonempty `decision_summary` and `rationale` representations of a decision as `canonical_claims` occurrences, and the `summary` of a positive-category decision in `MEMORY_DOMAIN_ID` as a `promoted_memory` occurrence. The two classes keep distinct occurrence identities when their bytes are equal.
//!
//! Identity is the decision's object id and canonical source revision. The memory domain is selected by its stable id, never its name.
//!
//! The materializer consumes the kernel commit log as a registered outbox consumer: the next episode resumes from the durable checkpoint, publication replays by receipt, and retiring an already-retired descriptor is a no-op. Correction, retirement, and approval revocation retire the predecessor's descriptors in the commit that publishes any successor's, or an earlier one, so no snapshot serves both revisions; the retirements reach the projection as tombstones through the shared export. The checkpoint advances only after every decision of a page has been published or retired.
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
    CANONICAL_ROLE, KeyedWrite, PublishError, Representation, SourcePublisher, SourceUnit,
    publish_batch,
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
    /// The publisher refused the representation for a reason no retry of the same decision revision can clear, such as an oversized field or a payload the secret scan rejects, so the representation has no descriptor.
    PublicationRefused(Representation),
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

/// A correction retires the object it replaced; a retirement or approval revocation retires the object itself; an insert without a replaced object and an event append retire nothing.
fn retired_by<'r>(row: &'r kernel::OutboxEntry, change: &'r DecisionChange) -> Option<&'r str> {
    match change.change_kind.as_str() {
        DECISION_INSERT_KIND | DECISION_CORRECT_KIND => change.replaced_object_id.as_deref(),
        DECISION_RETIRE_KIND | APPROVAL_REVOKE_KIND => Some(&row.object_id),
        _ => None,
    }
}

fn retirement_intent(retirement: &Retirement) -> CommitIntent {
    CommitIntent {
        producer: PRODUCER.to_owned(),
        operation_key: format!(
            "claim-retire:{}:{}",
            retirement.object_id, retirement.commit_seq
        ),
        request_digest: identity_digest(retirement.ids.join("\u{1f}").as_bytes()),
        actor: CANONICAL_ROLE.to_owned(),
        cause: "claim descriptor retirement".to_owned(),
    }
}

/// Retires each of `ids` that names a live descriptor.
fn retire_descriptors(
    envelope: &mut kernel::Envelope<'_>,
    ids: &[String],
) -> Result<String, KernelError> {
    for id in ids {
        if envelope
            .object_state(id)?
            .is_some_and(|state| state.object.invalidated_commit_seq.is_none())
        {
            envelope.retire_observation(id)?;
        }
    }
    Ok(String::new())
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

/// Bounds of one maintenance slice: a bootstrap page examines at most `decisions` decision rows carrying at most `decision_bytes` of payload, and a change page is one commit page under `commits`. A page's decisions or retained commits are applied at most `batch` at a time, each batch in one kernel commit.
#[derive(Debug, Clone, Copy)]
pub struct ClaimSliceBounds {
    pub decisions: NonZeroUsize,
    pub decision_bytes: NonZeroU64,
    pub commits: CommitPageBounds,
    pub batch: NonZeroUsize,
}

/// A batch closes before its units would exceed either bound, well inside the kernel's descriptors-per-commit limit; a single item past them commits on its own path.
const BATCH_UNITS: usize = 512;
const BATCH_TEXT_BYTES: usize = 8 * 1024 * 1024;

/// The runner's position, owned by the process; a restart begins again at `Unregistered`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ClaimProgress {
    #[default]
    Unregistered,
    /// Retiring the descriptors of `class`'s live inventory after `after` whose decisions are no longer live, canonical claims first and promoted memories second. Slices alternate with change pages as the scan's do.
    Reconcile {
        class: OccurrenceClass,
        after: Option<String>,
        changes_next: bool,
    },
    /// Scanning current decisions after `after` in object-id order. Slices alternate between a scan page and a change page, so the durable checkpoint follows retained commits while the scan runs.
    Bootstrap {
        after: Option<String>,
        changes_next: bool,
    },
    /// The scan is complete; retained commits are applied from the durable checkpoint.
    Changes,
}

#[derive(Debug)]
pub struct MaterializationReport {
    pub target: i64,
    /// Current decisions a bootstrap page published or replayed.
    pub bootstrapped: usize,
    /// Decisions a bootstrap page moved its cursor past, published or deferred to replay.
    pub scanned: usize,
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
    /// The slice's budget is cancelled once this many decisions or commits are complete.
    ExhaustBudgetAfter(usize),
}

pub struct ClaimMaterializer<'a> {
    kernel: &'a KernelStore,
    egress: ProviderEgress,
    fault: Option<EpisodeFault>,
    /// The running slice's budget; kernel writes and reads that have a budgeted entry wait within it.
    budget: Option<EvalBudget>,
    /// Decisions or retained commits one kernel commit applies.
    batch: NonZeroUsize,
}

/// Descriptor retirements of one invalidated decision, keyed by the commit that invalidated it.
struct Retirement {
    object_id: String,
    commit_seq: i64,
    ids: Vec<String>,
}

/// The effects of one bootstrap decision or retained commit: its retirements, then its publications.
struct BatchItem {
    /// The retained commit the item applies; `None` for a bootstrap decision.
    commit_seq: Option<i64>,
    /// The scanned decision the item completes; `None` for a retained commit.
    scanned: Option<String>,
    retirements: Vec<Retirement>,
    decisions: Vec<(ClaimSubject, DecisionRow, ClaimUnits)>,
}

impl BatchItem {
    fn objects(&self) -> impl Iterator<Item = &str> {
        self.retirements
            .iter()
            .map(|retirement| retirement.object_id.as_str())
            .chain(
                self.decisions
                    .iter()
                    .map(|(subject, _, _)| subject.object_id.as_str()),
            )
    }

    fn units(&self) -> usize {
        self.decisions
            .iter()
            .map(|(_, _, units)| units.units.len())
            .sum()
    }

    fn text_bytes(&self) -> usize {
        self.decisions
            .iter()
            .flat_map(|(_, _, units)| &units.units)
            .map(|unit| unit.text.len())
            .sum()
    }
}

/// Items whose effects commit together. No two items touch one decision object, so the batch may apply every retirement before every publication.
#[derive(Default)]
struct Batch {
    items: Vec<BatchItem>,
    objects: std::collections::HashSet<String>,
    units: usize,
    text_bytes: usize,
}

impl Batch {
    /// Whether `item` may join without passing the batch bounds or touching a decision object an earlier item touches.
    fn admits(&self, item: &BatchItem, limit: NonZeroUsize) -> bool {
        self.items.is_empty()
            || (self.items.len() < limit.get()
                && self.units + item.units() <= BATCH_UNITS
                && self.text_bytes + item.text_bytes() <= BATCH_TEXT_BYTES
                && item.objects().all(|object| !self.objects.contains(object)))
    }

    fn push(&mut self, item: BatchItem) {
        self.objects.extend(item.objects().map(str::to_owned));
        self.units += item.units();
        self.text_bytes += item.text_bytes();
        self.items.push(item);
    }
}

enum Stop {
    Blocked(ClaimBlocked),
    Failed(KernelError),
    /// A slice applied its one page.
    PageDone,
}

impl MaterializationReport {
    fn start(target: i64, end: MaterializationEnd) -> Self {
        Self {
            target,
            bootstrapped: 0,
            scanned: 0,
            acknowledged_through: 0,
            commits_consumed: 0,
            published: 0,
            replayed: 0,
            retired: 0,
            exclusions: Vec::new(),
            end,
        }
    }

    /// Whether the slice moved durable progress, so the scheduler runs the next one without idling.
    pub fn advanced(&self) -> bool {
        match self.end {
            MaterializationEnd::Continues => true,
            MaterializationEnd::Exhausted => self.scanned > 0 || self.commits_consumed > 0,
            MaterializationEnd::ReachedTarget | MaterializationEnd::Blocked(_) => false,
        }
    }
}

/// How long an acknowledgement of fully applied commits may wait for the writer after the slice's own budget is spent.
const ACKNOWLEDGEMENT_GRACE: std::time::Duration = std::time::Duration::from_secs(1);

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
            budget: None,
            batch: NonZeroUsize::MIN,
        }
    }

    /// Registers the consumer when it is absent and acknowledges through the registration commit, so the first episode materializes decisions from later commits. A call for a registered consumer returns at once and preserves its checkpoint.
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
        let read_checkpoint = || match budget {
            Some(budget) => kernel.outbox_consumer_checkpoint_within_budget(budget, CLAIM_CONSUMER),
            None => kernel.outbox_consumer_checkpoint(CLAIM_CONSUMER),
        };
        if read_checkpoint()?.is_some() {
            return Ok(());
        }
        let tip = match budget {
            Some(budget) => kernel.tip_within_budget(budget)?,
            None => kernel.tip()?,
        };
        // Every registration and removal is a commit, so the tip read while the consumer is absent keys this registration alone.
        let intent = CommitIntent {
            producer: PRODUCER.to_owned(),
            operation_key: format!("register:{CLAIM_CONSUMER}@{tip}"),
            request_digest: identity_digest(CLAIM_CONSUMER.as_bytes()),
            actor: CANONICAL_ROLE.to_owned(),
            cause: "claim consumer registration".to_owned(),
        };
        let operation = |envelope: &mut kernel::Envelope<'_>| {
            envelope.register_outbox_consumer(CLAIM_CONSUMER, now)?;
            Ok(String::new())
        };
        let receipt = match budget {
            Some(budget) => kernel.commit_within_budget(budget, intent, operation),
            None => kernel.commit(intent, operation),
        }?;
        // The kernel registers a consumer at the oldest retained outbox commit. The skipped history is covered by the reconciliation and scan that follow registration.
        let checkpoint = read_checkpoint()?.ok_or(KernelError::NotFound)?;
        if checkpoint < receipt.commit_seq {
            match budget {
                Some(budget) => {
                    let incarnation = kernel
                        .capture_commit_read_target_within_budget(budget)?
                        .incarnation;
                    kernel.acknowledge_outbox_within_budget(
                        budget,
                        CLAIM_CONSUMER,
                        receipt.commit_seq,
                        now,
                        incarnation,
                    )?;
                }
                None => kernel.acknowledge_outbox(CLAIM_CONSUMER, receipt.commit_seq, now)?,
            }
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

    /// Runs one bounded maintenance slice from `progress` and advances it: the first slice registers the consumer, reconciliation slices retire live descriptors whose decisions are no longer live, bootstrap slices publish one page of current decisions, and each of those alternates with one retained commit page; change slices apply one retained commit page. The report ends `ReachedTarget` only once reconciliation and the scan are complete and the checkpoint sits at the captured tip.
    ///
    /// The runner registers the consumer before reading decisions, so concurrent changes stay retained for replay. The scan publishes a decision only when its creation and latest fold into it are at or before the durable checkpoint read in the same slice; replay retires predecessors in the commit that publishes their successors, or an earlier one. The daemon's lifecycle owner serializes consumer slices and owns production acknowledgements.
    ///
    /// A slice applies its page's decisions or commits in batches of at most `bounds.batch`, each batch's artifact references, retirements, and descriptors in one kernel commit. Kernel reads, the batch commit's wait for the writer, and the acknowledgement wait within `budget`; staging a batch's artifact bytes runs before that wait and has no budget. A slice starts its first batch whatever the budget, stops between later batches once `budget` is exhausted, and acknowledges every fully applied commit within a short grace. A batch refused for any reason other than an exhausted budget applies its items one at a time under the same rule, which reports the refusal.
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

    /// [`Self::run_slice`] under one injected fault.
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
        self.budget = Some(budget.clone());
        self.batch = bounds.batch;
        let mut report = MaterializationReport::start(0, MaterializationEnd::Continues);
        let outcome = match progress.clone() {
            ClaimProgress::Unregistered => Self::register_within(self.kernel, Some(budget), now)
                .map(|()| {
                    *progress = ClaimProgress::Reconcile {
                        class: OccurrenceClass::CanonicalClaims,
                        after: None,
                        changes_next: false,
                    };
                })
                .map_err(Stop::Failed),
            ClaimProgress::Reconcile {
                class,
                after,
                changes_next: true,
            } => {
                *progress = ClaimProgress::Reconcile {
                    class,
                    after,
                    changes_next: false,
                };
                self.change_page(bounds.commits, budget, now, &mut report)
            }
            ClaimProgress::Reconcile {
                class,
                after,
                changes_next: false,
            } => {
                // A `reconcile_page` error preserves the cursor with `changes_next: true`.
                *progress = ClaimProgress::Reconcile {
                    class,
                    after: after.clone(),
                    changes_next: true,
                };
                self.reconcile_page(class, after, bounds, budget, &mut report)
                    .map(|next| *progress = next)
            }
            ClaimProgress::Bootstrap {
                after,
                changes_next: true,
            } => {
                *progress = ClaimProgress::Bootstrap {
                    after,
                    changes_next: false,
                };
                self.change_page(bounds.commits, budget, now, &mut report)
            }
            ClaimProgress::Bootstrap {
                after,
                changes_next: false,
            } => {
                // A `bootstrap_page` error preserves the cursor with `changes_next: true`.
                *progress = ClaimProgress::Bootstrap {
                    after: after.clone(),
                    changes_next: true,
                };
                self.bootstrap_page(after, bounds, budget, now, &mut report)
                    .map(|next| *progress = next)
            }
            ClaimProgress::Changes => self.change_page(bounds.commits, budget, now, &mut report),
        };
        self.budget = None;
        match outcome {
            Ok(()) | Err(Stop::PageDone) => {}
            Err(Stop::Blocked(ClaimBlocked::UnknownConsumer)) => {
                *progress = ClaimProgress::Unregistered;
                report.end = MaterializationEnd::Blocked(ClaimBlocked::UnknownConsumer);
            }
            Err(Stop::Blocked(blocked)) => report.end = MaterializationEnd::Blocked(blocked),
            Err(Stop::Failed(KernelError::Deadline)) if budget.is_exhausted() => {
                report.end = MaterializationEnd::Exhausted;
            }
            Err(Stop::Failed(error)) => return Err(error),
        }
        if matches!(
            progress,
            ClaimProgress::Reconcile { .. } | ClaimProgress::Bootstrap { .. }
        ) && matches!(report.end, MaterializationEnd::ReachedTarget)
        {
            report.end = MaterializationEnd::Continues;
        }
        Ok(report)
    }

    /// Cancels the slice's budget when an injected fault says `completed` items are enough.
    fn exhaust_on_fault(&self, completed: usize) {
        if let (Some(EpisodeFault::ExhaustBudgetAfter(limit)), Some(budget)) =
            (self.fault, &self.budget)
            && completed >= limit
        {
            budget.cancel();
        }
    }

    fn object_states(
        &self,
        object_ids: &[String],
    ) -> Result<Vec<Option<kernel::ObjectState>>, KernelError> {
        let (_, states) = match &self.budget {
            Some(budget) => self
                .kernel
                .object_states_within_budget(budget, object_ids)?,
            None => self.kernel.object_states(object_ids)?,
        };
        Ok(states)
    }

    fn decisions_as_of(
        &self,
        object_ids: &[String],
        requested: i64,
    ) -> Result<Vec<DecisionRow>, KernelError> {
        match &self.budget {
            Some(budget) => self
                .kernel
                .decisions_for_objects_as_of_within_budget(object_ids, requested, budget),
            None => self
                .kernel
                .decisions_for_objects_as_of(object_ids, requested),
        }
    }

    /// Retires the descriptors of every decision in one page of `class`'s live inventory that is no longer live, and returns where the reconciliation stands after it. Registration acknowledges past history the consumer did not apply, so a decision invalidated while the consumer was absent is retired here; the retirement key names the commit that invalidated the decision, which is the key the replay of that commit uses.
    fn reconcile_page(
        &self,
        class: OccurrenceClass,
        after: Option<String>,
        bounds: ClaimSliceBounds,
        budget: &EvalBudget,
        report: &mut MaterializationReport,
    ) -> Result<ClaimProgress, Stop> {
        let tip = self.kernel.tip_within_budget(budget)?;
        report.target = tip;
        let page = self.kernel.live_source_descriptors(
            class,
            tip,
            after.as_deref(),
            bounds.decisions,
            budget,
        )?;
        let decisions: Vec<String> = page
            .rows
            .iter()
            .filter_map(|row| row.detail.identity.first().map(|(_, id)| id.clone()))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let states = self.object_states(&decisions)?;
        for (object_id, state) in decisions.iter().zip(states) {
            if let Some(invalidated) = state.and_then(|state| state.object.invalidated_commit_seq) {
                self.retire_decision(object_id, invalidated, report)?;
            }
        }
        Ok(match (page.next, class) {
            (Some(next), class) => ClaimProgress::Reconcile {
                class,
                after: Some(next),
                changes_next: true,
            },
            (None, OccurrenceClass::CanonicalClaims) => ClaimProgress::Reconcile {
                class: OccurrenceClass::PromotedMemory,
                after: None,
                changes_next: true,
            },
            (None, _) => ClaimProgress::Bootstrap {
                after: None,
                changes_next: false,
            },
        })
    }

    /// Publishes one page of current decisions and returns where the scan stands after it.
    fn bootstrap_page(
        &self,
        after: Option<String>,
        bounds: ClaimSliceBounds,
        budget: &EvalBudget,
        now: i64,
        report: &mut MaterializationReport,
    ) -> Result<ClaimProgress, Stop> {
        let checkpoint = self
            .kernel
            .outbox_consumer_checkpoint_within_budget(budget, CLAIM_CONSUMER)?
            .ok_or(ClaimBlocked::UnknownConsumer)?;
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
        let mut completed = 0;
        let mut batch = Batch::default();
        for kernel::PagedDecision {
            object,
            decision,
            lineage_commit_seq,
        } in &page.decisions
        {
            let mut item = BatchItem {
                commit_seq: None,
                scanned: Some(object.object_id.clone()),
                retirements: Vec::new(),
                decisions: Vec::new(),
            };
            if *lineage_commit_seq <= checkpoint {
                let subject = ClaimSubject::from_object(object)?;
                let units = claim_units(&subject, decision)?;
                item.decisions.push((subject, decision.clone(), units));
            }
            if !batch.admits(&item, self.batch)
                && let Some(stopped) =
                    self.flush_scan(&mut batch, &mut done, &mut completed, budget, now, report)?
            {
                return Ok(stopped);
            }
            if completed > 0 && batch.items.is_empty() && budget.is_exhausted() {
                report.end = MaterializationEnd::Exhausted;
                return Ok(ClaimProgress::Bootstrap {
                    after: done,
                    changes_next: true,
                });
            }
            batch.push(item);
        }
        if let Some(stopped) =
            self.flush_scan(&mut batch, &mut done, &mut completed, budget, now, report)?
        {
            return Ok(stopped);
        }
        Ok(match page.next {
            Some(next) => ClaimProgress::Bootstrap {
                after: Some(next),
                changes_next: true,
            },
            None => ClaimProgress::Changes,
        })
    }

    /// Publishes the batch's scanned decisions together and moves `done` past them. A refused batch publishes them one at a time and stops between them once `budget` is exhausted. `Some` is where the scan stops early, with `report.end` saying why.
    fn flush_scan(
        &self,
        batch: &mut Batch,
        done: &mut Option<String>,
        completed: &mut usize,
        budget: &EvalBudget,
        now: i64,
        report: &mut MaterializationReport,
    ) -> Result<Option<ClaimProgress>, Stop> {
        let items = std::mem::take(batch).items;
        if items.is_empty() {
            return Ok(None);
        }
        let stopped = |end, report: &mut MaterializationReport, done: &Option<String>| {
            report.end = end;
            Ok(Some(ClaimProgress::Bootstrap {
                after: done.clone(),
                changes_next: true,
            }))
        };
        match self.commit_batch(&items, now, report) {
            Ok(true) => {
                *completed += items.len();
                for item in items {
                    report.bootstrapped += item.decisions.len();
                    *done = item.scanned;
                }
                report.scanned = *completed;
                self.exhaust_on_fault(*completed);
                return Ok(None);
            }
            Ok(false) => {}
            // The decisions before this batch are complete, so the cursor keeps them and the next slice runs a change page.
            Err(Stop::Failed(KernelError::Deadline)) if *completed > 0 => {
                return stopped(MaterializationEnd::Exhausted, report, done);
            }
            Err(stop) => return Err(stop),
        }
        for item in items {
            if *completed > 0 && budget.is_exhausted() {
                return stopped(MaterializationEnd::Exhausted, report, done);
            }
            for (subject, decision, _) in &item.decisions {
                match self.publish_units(subject, decision, now, report) {
                    Ok(()) => report.bootstrapped += 1,
                    Err(Stop::Blocked(blocked)) => {
                        return stopped(MaterializationEnd::Blocked(blocked), report, done);
                    }
                    Err(Stop::Failed(KernelError::Deadline)) if *completed > 0 => {
                        return stopped(MaterializationEnd::Exhausted, report, done);
                    }
                    Err(stop) => return Err(stop),
                }
            }
            *done = item.scanned;
            *completed += 1;
            report.scanned = *completed;
            self.exhaust_on_fault(*completed);
        }
        Ok(None)
    }

    /// Catches the consumer up to the tip within `budget`, then deregisters it. Successful deregistration or [`KernelError::NotFound`] resets `progress` to `Unregistered` and returns `true`. The method returns `false` and preserves `progress` when `progress` is already `Unregistered`, catch-up stays incomplete, or deregistration reports [`KernelError::ConsumerPending`].
    pub fn release(
        &mut self,
        progress: &mut ClaimProgress,
        bounds: ClaimSliceBounds,
        budget: &EvalBudget,
        now: i64,
    ) -> Result<bool, KernelError> {
        if *progress == ClaimProgress::Unregistered {
            return Ok(false);
        }
        let mut changes = ClaimProgress::Changes;
        loop {
            let report = self.run_slice(&mut changes, bounds, budget, now)?;
            match &report.end {
                MaterializationEnd::ReachedTarget if report.commits_consumed == 0 => break,
                MaterializationEnd::ReachedTarget | MaterializationEnd::Continues
                    if !budget.is_exhausted() => {}
                _ => return Ok(false),
            }
        }
        let tip = self.kernel.tip_within_budget(budget)?;
        let intent = CommitIntent {
            producer: PRODUCER.to_owned(),
            operation_key: format!("deregister:{CLAIM_CONSUMER}@{tip}"),
            request_digest: identity_digest(CLAIM_CONSUMER.as_bytes()),
            actor: CANONICAL_ROLE.to_owned(),
            cause: "claim consumer release".to_owned(),
        };
        let removed = self
            .kernel
            .commit_within_budget(budget, intent, |envelope| {
                envelope.deregister_outbox_consumer(CLAIM_CONSUMER, now)?;
                Ok(String::new())
            });
        match removed {
            Ok(_) | Err(KernelError::NotFound) => {
                *progress = ClaimProgress::Unregistered;
                Ok(true)
            }
            Err(KernelError::ConsumerPending) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Applies one retained commit page from the durable checkpoint toward a captured target and acknowledges every commit it fully applied.
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
        let checkpoint = self
            .kernel
            .outbox_consumer_checkpoint_within_budget(budget, CLAIM_CONSUMER)?
            .ok_or(ClaimBlocked::UnknownConsumer)?;
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
                let mut applied = checkpoint;
                let outcome = self.apply_page(page, now, report, Some(budget), &mut applied);
                if applied > report.acknowledged_through
                    && self.fault != Some(EpisodeFault::SkipAcknowledgement)
                {
                    // Commits applied before the budget ran out are acknowledged within a short grace, so a page larger than one slice still advances the checkpoint.
                    let grace = EvalBudget::new(
                        Some(std::time::Instant::now() + ACKNOWLEDGEMENT_GRACE),
                        std::sync::Arc::default(),
                    );
                    let ack_budget = if budget.is_exhausted() {
                        &grace
                    } else {
                        budget
                    };
                    self.acknowledge_within(ack_budget, applied, now, target.incarnation)?;
                    report.acknowledged_through = applied;
                }
                outcome?;
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
        let mut report =
            MaterializationReport::start(target.through_commit, MaterializationEnd::ReachedTarget);
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
                let mut applied = 0;
                self.apply_page(page, now, &mut report, None, &mut applied)?;
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

    /// Retires and publishes every decision change of `page` in commit order and records in `applied` the last commit whose effects are complete. Commits are applied in batches of at most `self.batch`, each in one kernel commit; a commit that touches a decision an earlier commit of the batch touched starts the next batch. A `budget` that runs out stops the page between batches; the first batch starts whatever the budget, and its own budgeted writes may still refuse. Replaying the rest is idempotent.
    fn apply_page(
        &self,
        page: &kernel::CommitPage,
        now: i64,
        report: &mut MaterializationReport,
        budget: Option<&EvalBudget>,
        applied: &mut i64,
    ) -> Result<(), Stop> {
        let exhausted = || budget.is_some_and(EvalBudget::is_exhausted);
        let mut batch = Batch::default();
        let mut consumed = 0;
        for commit in &page.commits {
            let item = match self.change_item(commit) {
                Ok(item) => item,
                Err(stop) => {
                    self.flush_changes(&mut batch, &mut consumed, now, report, budget, applied)?;
                    return Err(stop);
                }
            };
            if !batch.admits(&item, self.batch) {
                self.flush_changes(&mut batch, &mut consumed, now, report, budget, applied)?;
                if exhausted() {
                    return Err(KernelError::Deadline.into());
                }
            }
            batch.push(item);
        }
        self.flush_changes(&mut batch, &mut consumed, now, report, budget, applied)
    }

    /// Reads what one retained commit changes. Every predecessor a commit invalidates is retired before any successor in it is published, whatever order the rows were written in, so no commit holds a predecessor beside its successor.
    fn change_item(&self, commit: &kernel::CompleteCommit) -> Result<BatchItem, Stop> {
        let changes = commit
            .rows
            .iter()
            .filter(|row| row.object_kind == "decision")
            .map(|row| Ok((row, self.parse_change(row, commit.commit_seq)?)))
            .collect::<Result<Vec<_>, Stop>>()?;
        let mut item = BatchItem {
            commit_seq: Some(commit.commit_seq),
            scanned: None,
            retirements: Vec::new(),
            decisions: Vec::new(),
        };
        for (row, change) in &changes {
            if let Some(target) = retired_by(row, change)
                && !item
                    .retirements
                    .iter()
                    .any(|retirement| retirement.object_id == target)
                && let Some(retirement) = self.retirement(target, commit.commit_seq)?
            {
                item.retirements.push(retirement);
            }
        }
        for (row, change) in &changes {
            if !matches!(
                change.change_kind.as_str(),
                DECISION_INSERT_KIND | DECISION_CORRECT_KIND
            ) {
                continue;
            }
            let subject = ClaimSubject {
                object_id: row.object_id.clone(),
                domain_id: change.object.domain_id.clone(),
                revision: row.source_revision,
                sensitivity: row.sensitivity,
            };
            // The commit that changed the decision also invalidated it; its descriptors, if any, are retired by that commit's own rows.
            if let Some(decision) = self.decision_at(&subject.object_id, commit.commit_seq)? {
                let units = claim_units(&subject, &decision)?;
                item.decisions.push((subject, decision, units));
            }
        }
        Ok(item)
    }

    /// Commits the batch's commits together and acknowledges nothing; a refused batch applies its commits one at a time and stops between them once `budget` is exhausted.
    fn flush_changes(
        &self,
        batch: &mut Batch,
        consumed: &mut usize,
        now: i64,
        report: &mut MaterializationReport,
        budget: Option<&EvalBudget>,
        applied: &mut i64,
    ) -> Result<(), Stop> {
        let items = std::mem::take(batch).items;
        if items.is_empty() {
            return Ok(());
        }
        if self.commit_batch(&items, now, report)? {
            for item in &items {
                report.commits_consumed += 1;
                *applied = item.commit_seq.expect("a change item names its commit");
            }
            *consumed += items.len();
            self.exhaust_on_fault(*consumed);
            return Ok(());
        }
        for item in &items {
            if *consumed > 0 && budget.is_some_and(EvalBudget::is_exhausted) {
                return Err(KernelError::Deadline.into());
            }
            for retirement in &item.retirements {
                self.retire(retirement, report)?;
            }
            for (subject, decision, _) in &item.decisions {
                self.publish_units(subject, decision, now, report)?;
            }
            report.commits_consumed += 1;
            *applied = item.commit_seq.expect("a change item names its commit");
            *consumed += 1;
            self.exhaust_on_fault(*consumed);
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
            let published = match publisher.publish_within(unit, now, self.budget.as_ref()) {
                Ok(published) => published,
                Err(PublishError::Kernel {
                    error: KernelError::Deadline,
                    ..
                }) => return Err(Stop::Failed(KernelError::Deadline)),
                Err(error) if error.is_permanent() => {
                    report.exclusions.push((
                        subject.object_id.clone(),
                        ClaimExclusion::PublicationRefused(unit.representation),
                    ));
                    continue;
                }
                Err(error) => {
                    return Err(Stop::Blocked(ClaimBlocked::Publish {
                        object_id: subject.object_id.clone(),
                        error,
                    }));
                }
            };
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
        match self.retirement(object_id, commit_seq)? {
            Some(retirement) => self.retire(&retirement, report),
            None => Ok(()),
        }
    }

    /// The descriptors `object_id` could have published, retired under the key of `commit_seq`; `None` for an identity the encoding refuses, which was refused at publication too, so no descriptor of it exists.
    fn retirement(&self, object_id: &str, commit_seq: i64) -> Result<Option<Retirement>, Stop> {
        let states = self.object_states(std::slice::from_ref(&object_id.to_owned()))?;
        let Some(state) = states.into_iter().next().flatten() else {
            return Err(ClaimBlocked::DecisionUnreadable {
                object_id: object_id.to_owned(),
                commit_seq,
            }
            .into());
        };
        Ok(descriptor_ids(&ClaimSubject::from_object(&state.object)?)
            .ok()
            .map(|ids| Retirement {
                object_id: object_id.to_owned(),
                commit_seq,
                ids,
            }))
    }

    fn retire(
        &self,
        retirement: &Retirement,
        report: &mut MaterializationReport,
    ) -> Result<(), Stop> {
        let intent = retirement_intent(retirement);
        let operation =
            |envelope: &mut kernel::Envelope<'_>| retire_descriptors(envelope, &retirement.ids);
        let receipt = match &self.budget {
            Some(budget) => self.kernel.commit_within_budget(budget, intent, operation),
            None => self.kernel.commit(intent, operation),
        };
        match receipt {
            Ok(_) => {}
            Err(KernelError::Deadline) if self.budget.is_some() => {
                return Err(Stop::Failed(KernelError::Deadline));
            }
            Err(error) => {
                return Err(ClaimBlocked::Retire {
                    object_id: retirement.object_id.clone(),
                    error,
                }
                .into());
            }
        }
        self.count_retired(&[retirement], report)
    }

    /// Counted from durable state, so a replayed receipt reports what the first commit retired.
    fn count_retired(
        &self,
        retirements: &[&Retirement],
        report: &mut MaterializationReport,
    ) -> Result<(), Stop> {
        let ids: Vec<String> = retirements
            .iter()
            .flat_map(|retirement| retirement.ids.iter().cloned())
            .collect();
        if ids.is_empty() {
            return Ok(());
        }
        let states = self.object_states(&ids)?;
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

    /// Commits the effects of `items` in one kernel commit: every retirement, then every publication, each under its own receipt. `Ok(false)` reports a refused batch that committed nothing, which the caller applies item by item to learn the refusal.
    fn commit_batch(
        &self,
        items: &[BatchItem],
        now: i64,
        report: &mut MaterializationReport,
    ) -> Result<bool, Stop> {
        let decisions: Vec<&(ClaimSubject, DecisionRow, ClaimUnits)> =
            items.iter().flat_map(|item| &item.decisions).collect();
        let publishers: Vec<SourcePublisher<'_>> = decisions
            .iter()
            .map(|(subject, decision, _)| SourcePublisher {
                kernel: self.kernel,
                domain_id: &subject.domain_id,
                scope_id: decision.scope_id.as_deref(),
                egress: self.egress,
                sensitivity: subject.sensitivity,
            })
            .collect();
        let units: Vec<(&SourcePublisher<'_>, &SourceUnit)> = publishers
            .iter()
            .zip(&decisions)
            .flat_map(|(publisher, (_, _, units))| {
                units.units.iter().map(move |unit| (publisher, unit))
            })
            .collect();
        let retirements: Vec<&Retirement> =
            items.iter().flat_map(|item| &item.retirements).collect();
        let writes = retirements
            .iter()
            .map(|retirement| KeyedWrite {
                intent: retirement_intent(retirement),
                write: Box::new(|envelope: &mut kernel::Envelope<'_>| {
                    retire_descriptors(envelope, &retirement.ids)
                }),
            })
            .collect();
        match publish_batch(self.kernel, writes, &units, now, self.budget.as_ref()) {
            Ok(published) => {
                for (subject, _, units) in &decisions {
                    report.exclusions.extend(
                        units
                            .exclusions
                            .iter()
                            .map(|exclusion| (subject.object_id.clone(), *exclusion)),
                    );
                }
                for published in published {
                    if published.replayed {
                        report.replayed += 1;
                    } else {
                        report.published += 1;
                    }
                }
                self.count_retired(&retirements, report)?;
                Ok(true)
            }
            Err(KernelError::Deadline) if self.budget.is_some() => {
                Err(Stop::Failed(KernelError::Deadline))
            }
            Err(_) => Ok(false),
        }
    }

    /// `None` when no decision row is visible at `commit_seq` because the commit itself invalidated the object: it was created and invalidated together, or an existing survivor absorbed a fold and was then retired or corrected. The same commit carries the row that retires its descriptors.
    fn decision_at(&self, object_id: &str, commit_seq: i64) -> Result<Option<DecisionRow>, Stop> {
        let mut rows =
            self.decisions_as_of(std::slice::from_ref(&object_id.to_owned()), commit_seq)?;
        if let Some(row) = rows.pop() {
            return Ok(Some(row));
        }
        let states = self.object_states(std::slice::from_ref(&object_id.to_owned()))?;
        match states.into_iter().next().flatten() {
            Some(state) if state.object.invalidated_commit_seq == Some(commit_seq) => Ok(None),
            _ => Err(ClaimBlocked::DecisionUnreadable {
                object_id: object_id.to_owned(),
                commit_seq,
            }
            .into()),
        }
    }

    /// [`Self::acknowledge`] waiting within `budget` under the captured incarnation; a `Deadline` refusal is definite and leaves the commits for the next slice.
    fn acknowledge_within(
        &self,
        budget: &EvalBudget,
        through: i64,
        now: i64,
        incarnation: CommitReadIncarnation,
    ) -> Result<(), Stop> {
        let acknowledged = self.faulted_acknowledgement(|| {
            self.kernel.acknowledge_outbox_within_budget(
                budget,
                CLAIM_CONSUMER,
                through,
                now,
                incarnation,
            )
        });
        self.reconcile_acknowledgement(acknowledged, through, || {
            self.kernel
                .outbox_consumer_checkpoint_within_budget(budget, CLAIM_CONSUMER)
        })
    }

    /// A reply lost after the kernel may have committed is reconciled from the durable checkpoint.
    fn acknowledge(&self, through: i64, now: i64) -> Result<(), Stop> {
        let acknowledged = self.faulted_acknowledgement(|| {
            self.kernel.acknowledge_outbox(CLAIM_CONSUMER, through, now)
        });
        self.reconcile_acknowledgement(acknowledged, through, || {
            self.kernel.outbox_consumer_checkpoint(CLAIM_CONSUMER)
        })
    }

    /// Runs `acknowledge` under the injected acknowledgement fault, if any.
    fn faulted_acknowledgement(
        &self,
        acknowledge: impl FnOnce() -> Result<(), KernelError>,
    ) -> Result<(), KernelError> {
        let mut acknowledged = match self.fault {
            Some(EpisodeFault::FailAcknowledgement) => Err(KernelError::Io),
            _ => acknowledge(),
        };
        if self.fault == Some(EpisodeFault::LoseAcknowledgementReply) && acknowledged.is_ok() {
            acknowledged = Err(KernelError::Io);
        }
        acknowledged
    }

    /// An outcome-unknown acknowledgement counts as done only when the durable checkpoint reached `through`.
    fn reconcile_acknowledgement(
        &self,
        acknowledged: Result<(), KernelError>,
        through: i64,
        checkpoint: impl FnOnce() -> Result<Option<i64>, KernelError>,
    ) -> Result<(), Stop> {
        match acknowledged {
            Ok(()) => Ok(()),
            Err(error) if outcome_unknown(error) => {
                let checkpoint = checkpoint()?;
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

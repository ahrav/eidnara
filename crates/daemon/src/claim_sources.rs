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
use kernel::{
    APPROVAL_REVOKE_KIND, CommitIntent, CommitPageBounds, DECISION_CHANGE_KINDS,
    DECISION_CORRECT_KIND, DECISION_EVENT_APPEND_KIND, DECISION_INSERT_KIND, DECISION_RETIRE_KIND,
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
    "apply_change names a disposition for each decision change kind"
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
    Blocked(ClaimBlocked),
}

#[derive(Debug)]
pub struct MaterializationReport {
    pub target: i64,
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
}

pub struct ClaimMaterializer<'a> {
    kernel: &'a KernelStore,
    egress: ProviderEgress,
    fault: Option<EpisodeFault>,
}

enum Stop {
    Blocked(ClaimBlocked),
    Failed(KernelError),
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
        let receipt = kernel.commit(
            CommitIntent {
                producer: PRODUCER.to_owned(),
                operation_key: format!("register:{CLAIM_CONSUMER}"),
                request_digest: identity_digest(CLAIM_CONSUMER.as_bytes()),
                actor: CANONICAL_ROLE.to_owned(),
                cause: "claim consumer registration".to_owned(),
            },
            |envelope| {
                envelope.register_outbox_consumer(CLAIM_CONSUMER, now)?;
                Ok(String::new())
            },
        )?;
        // The kernel registers a consumer at the oldest retained outbox commit. A replayed receipt carries the first registration's commit, so the guard keeps a later call from moving an advanced checkpoint.
        let checkpoint = kernel
            .outbox_consumer_checkpoint(CLAIM_CONSUMER)?
            .ok_or(KernelError::NotFound)?;
        if checkpoint < receipt.commit_seq {
            kernel.acknowledge_outbox(CLAIM_CONSUMER, receipt.commit_seq, now)?;
        }
        Ok(())
    }

    /// Captures a target and processes complete commits from the consumer checkpoint toward it one bounded page at a time, acknowledging each page after its decisions are published or retired.
    ///
    /// `now` is Unix-epoch milliseconds, recorded as the descriptors' observation time and the acknowledgement's `updated_at`.
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

    fn run_episode_inner(
        &mut self,
        bounds: CommitPageBounds,
        now: i64,
    ) -> Result<MaterializationReport, KernelError> {
        let target = self.kernel.capture_commit_read_target()?;
        let mut report = MaterializationReport {
            target: target.through_commit,
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
                consumer_id: CLAIM_CONSUMER,
                incarnation: target.incarnation,
                now,
                after: checkpoint,
                target: target.through_commit,
                bounds,
            },
            |page| {
                for commit in &page.commits {
                    for row in commit
                        .rows
                        .iter()
                        .filter(|row| row.object_kind == "decision")
                    {
                        self.apply_change(row, commit.commit_seq, now, &mut report)?;
                    }
                    report.commits_consumed += 1;
                }
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
        }
    }

    /// An event append changes no text and produces nothing; an approval revocation invalidates the decision and retires like a retirement; every other kind must be one this materializer names.
    fn apply_change(
        &self,
        row: &kernel::OutboxEntry,
        commit_seq: i64,
        now: i64,
        report: &mut MaterializationReport,
    ) -> Result<(), Stop> {
        let change: DecisionChange =
            serde_json::from_slice(&row.payload).map_err(|_| ClaimBlocked::DecisionUnreadable {
                object_id: row.object_id.clone(),
                commit_seq,
            })?;
        let kind = change.change_kind.as_str();
        match kind {
            DECISION_INSERT_KIND | DECISION_CORRECT_KIND => {
                if let Some(replaced) = change.replaced_object_id {
                    self.retire_decision(&replaced, commit_seq, report)?;
                }
                let subject = ClaimSubject {
                    object_id: row.object_id.clone(),
                    domain_id: change.object.domain_id,
                    revision: row.source_revision,
                    sensitivity: row.sensitivity,
                };
                self.publish_decision(&subject, commit_seq, now, report)
            }
            DECISION_RETIRE_KIND | APPROVAL_REVOKE_KIND => {
                self.retire_decision(&row.object_id, commit_seq, report)
            }
            DECISION_EVENT_APPEND_KIND => Ok(()),
            _ => Err(ClaimBlocked::UnknownChangeKind {
                object_id: row.object_id.clone(),
                commit_seq,
                change_kind: change.change_kind,
            }
            .into()),
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
            // A decision created and invalidated in the same commit was never readable; there is nothing to publish and nothing to retire.
            return Ok(());
        };
        let units = claim_units(subject, &decision)?;
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

    /// `None` when no decision row is visible at `commit_seq`, which after a registry row exists at that commit means the row was created and invalidated together.
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
            Some(state)
                if state.object.invalidated_commit_seq == Some(state.object.created_commit_seq) =>
            {
                Ok(None)
            }
            _ => Err(ClaimBlocked::DecisionUnreadable {
                object_id: object_id.to_owned(),
                commit_seq,
            }
            .into()),
        }
    }

    /// A reply lost after the kernel may have committed is reconciled from the durable checkpoint.
    fn acknowledge(&self, through: i64, now: i64) -> Result<(), Stop> {
        let mut acknowledged = self.kernel.acknowledge_outbox(CLAIM_CONSUMER, through, now);
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

//! The attestation write set: one Kernel commit per checked claim.
//!
//! The caller classifies a decision with [`classify`](super::classify::classify), computes its
//! verdict with [`verify`](super::verify) outside the Kernel writer, and hands both to [`attest`],
//! which performs at most one commit and returns one [`Outcome`]. The commit closure reads only
//! the store.
//!
//! On `Match` one envelope rechecks the model decision against the classification, then writes, in
//! order: the attestation decision under `source_kind = "verifier"` with the model decision's
//! anchor, scope, and served sensitivity; its witness observation, `code_present` or `config_present` by
//! document kind, carrying the attestation's source triple and a versioned detail naming the
//! compared evidence, digest, commit, blob, path, and descriptor; the witness admission under
//! `(TrustedLocalCode, CurrentCode|CurrentConfig)`; a `DerivedReinjection` causality record naming
//! the model decision at its revision; and one decision event citing the compared evidence. The
//! envelope commits only when the witness admission returns effective maturity `Verified` with
//! disposition `Active`. The attestation and its witness cite the evidence row through that event
//! and the detail alone, so the attestation serves at the class the Kernel serves the model
//! decision at.
//!
//! On `Mismatch` one envelope records a `MarkStale` on the model decision under the classes of its
//! latest admission. `Unavailable` writes nothing.
//!
//! Every [`Identity`] field derives from `(model object id, oid)` through [`Identity::of`], and the
//! operation key names the same pair, so a retry converges on the same rows or replays the receipt.

use kernel::applicability::EvalBudget;
use kernel::source_identity::{OccurrenceClass, lower_hex as hex};
use kernel::{
    AdmissionEvent, AdmissionRequest, AnchorCondition, CausalEvidence, ClaimCausalityError,
    ClaimCausalityRequest, CommitIntent, CommitReadTarget, CommitReceipt, DecisionEventPayload,
    DecisionEventSpec, DecisionPayload, DecisionRow, DecisionSpec, Disposition, Envelope,
    EventKind, GitCondition, KernelError, KernelStore, Maturity, ObservationPayload,
    ObservationSpec, ParentReference, PriorDecision, SourceClass,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::classify::{Class, Classification, VERIFIER_SOURCE_KIND, VerifierInput};
use super::{Evaluation, Verdict};

/// The commit producer of every verifier write.
pub const PRODUCER: &str = "verifier";

/// The decision-event kind of the attestation's citation of the compared evidence.
pub const CITATION_EVENT_KIND: &str = "verifier_citation";

/// The version of the witness observation's detail. The applicability step reads a detail at
/// another version as unavailable.
pub const WITNESS_DETAIL_VERSION: u32 = 1;

/// The ids one attestation of a model decision at one commit oid is written under: `source_id` is
/// the SHA-256 hex of the pair, and the object, decision, witness observation, and witness object
/// ids are that hash under their own prefixes, outside the Kernel's reserved descriptor,
/// causality, and local-file namespaces. The Kernel derives the causality record's ids from the
/// attestation's object id and the commit sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub source_id: String,
    pub object_id: String,
    pub decision_id: String,
    pub witness_observation_id: String,
    pub witness_object_id: String,
}

impl Identity {
    pub fn of(model_object_id: &str, oid: &str) -> Self {
        let hash = format!("{:x}", Sha256::digest(format!("{model_object_id}\n{oid}")));
        Self::of_hash(hash)
    }

    /// The ids derived from `hash`, the SHA-256 hex of a `(model object id, oid)` pair.
    fn of_hash(hash: String) -> Self {
        Self {
            source_id: hash.clone(),
            object_id: format!("{}{hash}", ID_PREFIXES[0]),
            decision_id: format!("{}{hash}", ID_PREFIXES[1]),
            witness_observation_id: format!("{}{hash}", ID_PREFIXES[2]),
            witness_object_id: format!("{}{hash}", ID_PREFIXES[3]),
        }
    }

    /// The identity whose attestation object id is `object_id`, or `None` for an id outside the
    /// attestation prefix.
    pub fn of_attestation(object_id: &str) -> Option<Self> {
        object_id
            .strip_prefix(ID_PREFIXES[0])
            .map(|hash| Self::of_hash(hash.to_owned()))
    }
}

/// What one call of [`attest`] did.
#[must_use = "a `Refused` outcome can follow a durable commit and needs resolving"]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The attestation and every companion row committed.
    Committed(CommitReceipt),
    /// The `MarkStale` committed: on the model decision for [`attest`], or on the attestation for
    /// [`super::applicability::mark_republished`].
    StaleCommitted(CommitReceipt),
    /// The key's receipt was already recorded with the same request digest; nothing was written.
    Replayed(CommitReceipt),
    Skipped(Skip),
    /// The witness admission returned below `Verified` or not `Active`; the envelope rolled back.
    WitnessDegraded,
    /// The key is recorded under another request digest, or the opposite verdict's key for the
    /// same decision and oid is recorded.
    Conflict,
    /// The model decision, its served sensitivity, its anchor, its cited descriptor or evidence
    /// row, or the store's incarnation differs from the classification, or the Kernel reports the
    /// causality parent missing or at another revision; for
    /// [`super::applicability::mark_republished`], the attestation is retired, superseded, or
    /// without an admission, the store's incarnation changed, or the lineage's live descriptor is
    /// another one or none. Nothing committed, whether detected before or inside the envelope.
    Changed,
    /// A `Match` or `Mismatch` names no blob, the witness detail failed to serialize, the write
    /// set failed [`WriteSet::check`] before the commit, a set handed to [`WriteSet::commit`]
    /// differs from the set the model decision's row yields, the Kernel refused the causality
    /// request as malformed, or the `MarkStale` admission returned a disposition other than
    /// `Stale`; the store stays at its tip.
    InvalidWriteSet,
    /// The Kernel refused the commit or a read before it. A [`KernelError::Io`] from the commit
    /// can follow a durable commit, so a
    /// caller treats it as an unknown outcome. The next pass resolves an attestation by replaying
    /// the key's receipt; a committed `MarkStale` leaves its subject `Stale`, outside the next
    /// pass's input, so a caller resolves it by reading the disposition of the marked subject: the
    /// model decision for [`attest`], the attestation for
    /// [`super::applicability::mark_republished`].
    Refused(KernelError),
}

/// Why [`attest`] wrote nothing for a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// The classification is not [`Class::Verifiable`], `self_source` included.
    NotVerifiable,
    /// The verdict is `Unavailable`.
    Unavailable,
    /// The marked subject is already `Stale`: the model decision for [`attest`], the attestation
    /// for [`super::applicability::mark_republished`].
    AlreadyStale,
}

/// Performs the one write `evaluation`'s verdict calls for on the decision `classification`
/// names.
pub fn attest(
    kernel: &KernelStore,
    classification: &Classification,
    evaluation: &Evaluation,
    budget: &EvalBudget,
) -> Outcome {
    let Class::Verifiable(input) = &classification.class else {
        return Outcome::Skipped(Skip::NotVerifiable);
    };
    let target = classification.target;
    match (evaluation.verdict, evaluation.blob_oid.as_deref()) {
        (Verdict::Unavailable(_), _) => Outcome::Skipped(Skip::Unavailable),
        (_, None) => Outcome::InvalidWriteSet,
        (Verdict::Match, Some(blob)) => attest_match(kernel, target, input, blob, budget),
        (Verdict::Mismatch, Some(blob)) => mark_stale(kernel, target, input, blob, budget),
    }
}

fn attest_match(
    kernel: &KernelStore,
    target: CommitReadTarget,
    input: &VerifierInput,
    blob_oid: &str,
    budget: &EvalBudget,
) -> Outcome {
    let keyed = intent(input, Write::Attest, blob_oid);
    let opposite = intent(input, Write::Stale, blob_oid);
    run(
        kernel,
        budget,
        keyed,
        Some(opposite),
        Outcome::Committed,
        |envelope| {
            let write_set = WriteSet::at_tip(envelope, target, input, blob_oid)?;
            if !write_set.check() {
                return Err(Stop::Outcome(Outcome::InvalidWriteSet));
            }
            write_set.write(envelope, kernel)
        },
    )
}

/// The rows a `Match` writes, built from the classification and the model decision's row.
/// [`WriteSet::commit`] checks them with [`WriteSet::check`] before it opens the envelope and
/// commits them only while they equal the set the model decision's row at the committed tip
/// yields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteSet {
    pub target: CommitReadTarget,
    pub input: VerifierInput,
    pub blob_oid: String,
    pub attestation: DecisionSpec,
    pub witness: ObservationSpec,
    pub citation: DecisionEventSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WitnessDetail {
    pub(super) witness_version: u32,
    evidence_id: String,
    pub(super) artifact_digest: String,
    commit_oid: String,
    blob_oid: String,
    pub(super) relative_path: String,
    descriptor_object_id: String,
    pub(super) lineage_id: String,
    descriptor_revision: String,
}

impl WriteSet {
    /// The write set for `input` matched at blob `blob_oid`, reading the model decision's row at
    /// `target.through_commit` for its kind, scope, and summary.
    ///
    /// # Errors
    ///
    /// Returns [`Outcome::Changed`] when the store's incarnation differs from `target`'s or the
    /// row is absent at that sequence, and [`Outcome::Refused`] for a refused read.
    pub fn new(
        kernel: &KernelStore,
        target: CommitReadTarget,
        input: &VerifierInput,
        blob_oid: &str,
        budget: &EvalBudget,
    ) -> Result<Self, Outcome> {
        let model = &input.decision;
        let read = kernel.decisions_for_objects_as_of_within_budget(
            std::slice::from_ref(&model.object_id),
            target.through_commit,
            budget,
        );
        kernel
            .require_incarnation(target.incarnation)
            .map_err(|_| Outcome::Changed)?;
        let row = read
            .map_err(Outcome::Refused)?
            .into_iter()
            .find(|row| row.object_id == model.object_id)
            .ok_or(Outcome::Changed)?;
        Self::from_row(target, input, blob_oid, row)
    }

    /// The attestation takes its kind, scope, and summary from `row`, the model decision's row.
    fn from_row(
        target: CommitReadTarget,
        input: &VerifierInput,
        blob_oid: &str,
        row: DecisionRow,
    ) -> Result<Self, Outcome> {
        let model = &input.decision;
        let claim = &input.claim;
        let ids = Identity::of(&model.object_id, claim.oid());
        let digest = hex(claim.digest());
        let rationale = serde_json::json!({
            "source": PRODUCER,
            "attests": model.object_id,
            "commit_oid": claim.oid(),
            "relative_path": claim.relative_path(),
            "artifact_digest": digest,
        });
        let detail = WitnessDetail {
            witness_version: WITNESS_DETAIL_VERSION,
            evidence_id: input.evidence_id.clone(),
            artifact_digest: digest,
            commit_oid: claim.oid().to_owned(),
            blob_oid: blob_oid.to_owned(),
            relative_path: claim.relative_path().to_owned(),
            descriptor_object_id: input.descriptor_object_id.clone(),
            lineage_id: input.lineage_id.clone(),
            descriptor_revision: input.descriptor_revision.clone(),
        };
        let detail = serde_json::to_string(&detail).map_err(|_| Outcome::InvalidWriteSet)?;
        let observed_at = crate::now_ms();
        let kind = input.document_kind().witness_kind();
        Ok(Self {
            target,
            input: input.clone(),
            blob_oid: blob_oid.to_owned(),
            attestation: DecisionSpec {
                decision_id: ids.decision_id,
                object_id: ids.object_id.clone(),
                domain_id: model.domain_id.clone(),
                proposition_id: None,
                scope_id: row.scope_id.clone(),
                anchor_id: model.anchor_id.clone(),
                evidence_id: None,
                decision_kind: row.decision_kind,
                payload: DecisionPayload {
                    summary: row.payload.summary,
                    rationale: rationale.to_string(),
                },
                source_kind: VERIFIER_SOURCE_KIND.into(),
                source_id: ids.source_id.clone(),
                source_revision: 1,
                sensitivity: model.served_sensitivity,
            },
            witness: ObservationSpec {
                observation_id: ids.witness_observation_id,
                object_id: ids.witness_object_id,
                domain_id: model.domain_id.clone(),
                proposition_id: None,
                scope_id: row.scope_id,
                anchor_id: None,
                evidence_id: None,
                observation_kind: kind.into(),
                payload: ObservationPayload {
                    summary: "the cited document's bytes are present at the commit".into(),
                    classification: kind.into(),
                    detail: Some(detail),
                },
                observed_at,
                dependencies: Vec::new(),
                source_kind: VERIFIER_SOURCE_KIND.into(),
                source_id: ids.source_id,
                source_revision: 1,
                sensitivity: model.served_sensitivity,
            },
            citation: DecisionEventSpec {
                event_kind: CITATION_EVENT_KIND.into(),
                payload: DecisionEventPayload {
                    summary: "cites the compared repository document".into(),
                },
                evidence_id: Some(input.evidence_id.clone()),
                recorded_at: observed_at,
            },
        })
    }

    /// Whether the rows cite only what was compared and stand for the model decision: the
    /// compared evidence row, the witness detail's evidence id, and the citation event's evidence
    /// id are one id; the witness detail is at [`WITNESS_DETAIL_VERSION`] and names this write
    /// set's blob and the claim's commit, digest, path, descriptor, lineage, and revision; the
    /// citation is a [`CITATION_EVENT_KIND`] event; the attestation and the witness carry the
    /// model decision's domain and served sensitivity and one scope, the attestation its anchor;
    /// every id is the [`Identity`] of the model decision and the claim's oid; the witness carries
    /// the attestation's source triple under `verifier` at revision 1 and the document kind's
    /// witness kind; and neither the attestation nor the witness cites evidence by its row.
    pub fn check(&self) -> bool {
        let detail = self
            .witness
            .payload
            .detail
            .as_deref()
            .and_then(|detail| serde_json::from_str::<WitnessDetail>(detail).ok());
        let compared = self.input.evidence_id.as_str();
        let model = &self.input.decision;
        let (attestation, witness) = (&self.attestation, &self.witness);
        let named = Identity {
            source_id: attestation.source_id.clone(),
            object_id: attestation.object_id.clone(),
            decision_id: attestation.decision_id.clone(),
            witness_observation_id: witness.observation_id.clone(),
            witness_object_id: witness.object_id.clone(),
        };
        let claim = &self.input.claim;
        detail.is_some_and(|detail| {
            detail.witness_version == WITNESS_DETAIL_VERSION
                && detail.evidence_id == compared
                && detail.blob_oid == self.blob_oid
                && detail.commit_oid == claim.oid()
                && detail.artifact_digest == hex(claim.digest())
                && detail.relative_path == claim.relative_path()
                && detail.descriptor_object_id == self.input.descriptor_object_id
                && detail.lineage_id == self.input.lineage_id
                && detail.descriptor_revision == self.input.descriptor_revision
        }) && self.citation.evidence_id.as_deref() == Some(compared)
            && self.citation.event_kind == CITATION_EVENT_KIND
            && named == Identity::of(&model.object_id, claim.oid())
            && (attestation.sensitivity, witness.sensitivity)
                == (model.served_sensitivity, model.served_sensitivity)
            && witness.scope_id == attestation.scope_id
            && (&attestation.domain_id, &witness.domain_id) == (&model.domain_id, &model.domain_id)
            && attestation.anchor_id == model.anchor_id
            && attestation.source_kind == VERIFIER_SOURCE_KIND
            && attestation.source_revision == 1
            && (
                &witness.source_kind,
                &witness.source_id,
                witness.source_revision,
            ) == (
                &attestation.source_kind,
                &attestation.source_id,
                attestation.source_revision,
            )
            && witness.observation_kind == self.input.document_kind().witness_kind()
            && attestation.evidence_id.is_none()
            && witness.evidence_id.is_none()
    }

    /// The write set must match the set rebuilt from the model decision's row at the committed
    /// tip, except for `observed_at` and `recorded_at`; other differences produce
    /// [`Outcome::InvalidWriteSet`].
    pub fn commit(&self, kernel: &KernelStore, budget: &EvalBudget) -> Outcome {
        if !self.check() {
            return Outcome::InvalidWriteSet;
        }
        let blob = &self.blob_oid;
        let keyed = intent(&self.input, Write::Attest, blob);
        let opposite = intent(&self.input, Write::Stale, blob);
        run(
            kernel,
            budget,
            keyed,
            Some(opposite),
            Outcome::Committed,
            |envelope| {
                let mut built = Self::at_tip(envelope, self.target, &self.input, blob)?;
                built.witness.observed_at = self.witness.observed_at;
                built.citation.recorded_at = self.citation.recorded_at;
                if built != *self {
                    return Err(Stop::Outcome(Outcome::InvalidWriteSet));
                }
                self.write(envelope, kernel)
            },
        )
    }

    fn at_tip(
        envelope: &Envelope<'_>,
        target: CommitReadTarget,
        input: &VerifierInput,
        blob_oid: &str,
    ) -> Result<Self, Stop> {
        let object_id = &input.decision.object_id;
        let row = envelope
            .decisions_for_objects(std::slice::from_ref(object_id))?
            .into_iter()
            .find(|row| &row.object_id == object_id)
            .ok_or(Stop::Outcome(Outcome::Changed))?;
        Self::from_row(target, input, blob_oid, row).map_err(Stop::Outcome)
    }

    fn write(&self, envelope: &mut Envelope<'_>, kernel: &KernelStore) -> Result<(), Stop> {
        let (scope_id, prior) = recheck(envelope, kernel, self.target, &self.input)?;
        if prior.disposition != Disposition::Active || scope_id != self.attestation.scope_id {
            return Err(Stop::Outcome(Outcome::Changed));
        }
        let model = &self.input.decision;
        let served = envelope
            .served_rows_for(&[model.object_id.as_str()], None)?
            .remove(&model.object_id)
            .map(|row| row.object.sensitivity);
        if served != Some(self.attestation.sensitivity) {
            return Err(Stop::Outcome(Outcome::Changed));
        }
        let subject = &self.attestation.object_id;
        envelope.insert_decision(self.attestation.clone())?;
        envelope.insert_observation(self.witness.clone())?;
        let kind = self.input.document_kind();
        let admission = envelope.record_admission(AdmissionRequest {
            candidate_id: None,
            subject_object_id: Some(subject.clone()),
            source_class: Some(SourceClass::TrustedLocalCode),
            taint_class: Some(kind.taint_class()),
            event: AdmissionEvent {
                kind: kind.event_kind(),
                trigger_object_id: Some(self.witness.object_id.clone()),
                approval_object_id: None,
                evidence_id: None,
                reason: "the verifier's witness of the cited bytes at the commit".into(),
            },
        })?;
        if admission.effective_maturity != Maturity::Verified
            || admission.disposition != Disposition::Active
        {
            return Err(Stop::Outcome(Outcome::WitnessDegraded));
        }
        envelope
            .record_claim_causality(&ClaimCausalityRequest {
                subject_object_id: subject,
                subject_revision: self.attestation.source_revision,
                evidence: CausalEvidence::DerivedReinjection {
                    parents: vec![ParentReference {
                        object_id: model.object_id.clone(),
                        revision: model.source_revision,
                    }],
                },
                observed_at: self.witness.observed_at,
            })
            .map_err(|error| match error {
                ClaimCausalityError::Kernel(error) => Stop::Kernel(error),
                ClaimCausalityError::ParentMissing
                | ClaimCausalityError::ParentRevisionMismatch => Stop::Outcome(Outcome::Changed),
                ClaimCausalityError::SubjectNotDecision
                | ClaimCausalityError::SubjectRevisionMismatch
                | ClaimCausalityError::EvidenceMissing
                | ClaimCausalityError::ArtifactMismatch
                | ClaimCausalityError::EvidenceNotExact
                | ClaimCausalityError::NoParents
                | ClaimCausalityError::TooManyParents
                | ClaimCausalityError::DuplicateParent
                | ClaimCausalityError::ParentIsSubject
                | ClaimCausalityError::DuplicateSubject => Stop::Outcome(Outcome::InvalidWriteSet),
            })?;
        envelope.append_decision_event(&self.attestation.decision_id, self.citation.clone())?;
        Ok(())
    }
}

/// Records `MarkStale` on the model decision of `input`, whose cited bytes differ from blob
/// `blob_oid` at the claim's commit.
fn mark_stale(
    kernel: &KernelStore,
    target: CommitReadTarget,
    input: &VerifierInput,
    blob_oid: &str,
    budget: &EvalBudget,
) -> Outcome {
    let claim = &input.claim;
    let reason = format!(
        "verifier: the blob {blob_oid} at {} in commit {} differs from the cited sha256 {}",
        claim.relative_path(),
        claim.oid(),
        hex(claim.digest()),
    );
    let keyed = intent(input, Write::Stale, blob_oid);
    let opposite = intent(input, Write::Attest, blob_oid);
    run(
        kernel,
        budget,
        keyed,
        Some(opposite),
        Outcome::StaleCommitted,
        |envelope| {
            let (_, prior) = recheck(envelope, kernel, target, input)?;
            record_stale(envelope, &input.decision.object_id, &prior, reason)
        },
    )
}

/// Records one `MarkStale` on `subject` under the classes of `prior`, its latest admission, with
/// no evidence. An `Active` subject is marked, a `Stale` one stops the envelope as
/// [`Skip::AlreadyStale`], and any other disposition stops it as `Changed`; a mark the Kernel
/// leaves short of `Stale` stops it as an invalid write set.
pub(super) fn record_stale(
    envelope: &mut Envelope<'_>,
    subject: &str,
    prior: &PriorDecision,
    reason: String,
) -> Result<(), Stop> {
    match prior.disposition {
        Disposition::Active => {}
        Disposition::Stale => return Err(Stop::Outcome(Outcome::Skipped(Skip::AlreadyStale))),
        _ => return Err(Stop::Outcome(Outcome::Changed)),
    }
    let marked = envelope.record_admission(AdmissionRequest {
        candidate_id: None,
        subject_object_id: Some(subject.to_owned()),
        source_class: Some(prior.source_class),
        taint_class: Some(prior.taint_class),
        event: AdmissionEvent {
            kind: EventKind::MarkStale,
            trigger_object_id: None,
            approval_object_id: None,
            evidence_id: None,
            reason,
        },
    })?;
    if marked.disposition != Disposition::Stale {
        return Err(Stop::Outcome(Outcome::InvalidWriteSet));
    }
    Ok(())
}

/// Why an envelope stopped before it committed.
pub(super) enum Stop {
    Outcome(Outcome),
    Kernel(KernelError),
}

impl From<KernelError> for Stop {
    fn from(error: KernelError) -> Self {
        Self::Kernel(error)
    }
}

/// Commits `write` under `intent`, after refusing a recorded `opposite` key when one is given,
/// mapping the result to one outcome. A stopped envelope returns its error from the closure, so
/// the commit rolls back.
pub(super) fn run(
    kernel: &KernelStore,
    budget: &EvalBudget,
    intent: CommitIntent,
    opposite: Option<CommitIntent>,
    committed: fn(CommitReceipt) -> Outcome,
    write: impl FnOnce(&mut Envelope<'_>) -> Result<(), Stop>,
) -> Outcome {
    let mut stopped = None;
    let mut entered = false;
    let result = kernel.commit_within_budget(budget, intent, |envelope| {
        entered = true;
        let stop = match opposite.map_or(Ok(None), |opposite| envelope.stored_receipt(opposite)) {
            Ok(Some(_)) | Err(KernelError::Conflict) => Stop::Outcome(Outcome::Conflict),
            Ok(None) => match write(envelope) {
                Ok(()) => return Ok(String::new()),
                Err(stop) => stop,
            },
            Err(error) => Stop::Kernel(error),
        };
        Err(match stop {
            Stop::Outcome(outcome) => {
                stopped = Some(outcome);
                // The error only rolls the envelope back; once `stopped` is set it is not read.
                KernelError::Conflict
            }
            Stop::Kernel(error) => error,
        })
    });
    match result {
        Ok(receipt) if receipt.replayed => Outcome::Replayed(receipt),
        Ok(receipt) => committed(receipt),
        Err(error) => match stopped {
            Some(outcome) => outcome,
            // The store compares a recorded key's digest before it runs the closure.
            None if error == KernelError::Conflict && !entered => Outcome::Conflict,
            None => Outcome::Refused(error),
        },
    }
}

/// Rereads, in the envelope, what the classification found at `target`: the store's
/// incarnation; the model decision live with no successor, still a decision with the same
/// creating commit, domain, source triple, and classes, under a source kind other than
/// `verifier`; its anchor live as `reachable_from` the claim's oid at the committed tip the
/// envelope extends; and the cited descriptor the live revision of
/// its lineage, still citing the compared evidence. The live-descriptor read returns a descriptor
/// only while the evidence row it cites is live, so a deleted evidence row reads as `Changed`.
/// Returns the decision's scope and latest admission.
fn recheck(
    envelope: &Envelope<'_>,
    kernel: &KernelStore,
    target: CommitReadTarget,
    input: &VerifierInput,
) -> Result<(Option<String>, PriorDecision), Stop> {
    let changed = || Stop::Outcome(Outcome::Changed);
    // The envelope holds the writer, so no restore runs between this check and the commit.
    kernel
        .require_incarnation(target.incarnation)
        .map_err(|_| changed())?;
    let model = &input.decision;
    let state = envelope
        .object_state(&model.object_id)?
        .ok_or_else(changed)?;
    let object = &state.object;
    let same = object.object_kind == "decision"
        && object.invalidated_commit_seq.is_none()
        && object.superseded_by.is_none()
        && object.created_commit_seq == model.created_commit_seq
        && model.created_commit_seq <= target.through_commit
        && object.domain_id == model.domain_id
        && (
            &object.source_kind,
            &object.source_id,
            object.source_revision,
        ) == (&model.source_kind, &model.source_id, model.source_revision)
        && object.source_kind != VERIFIER_SOURCE_KIND;
    if !same {
        return Err(changed());
    }
    let (prior, _) = envelope
        .subject_admission(&model.object_id)?
        .ok_or_else(changed)?;
    if (prior.source_class, prior.taint_class) != (model.source_class, model.taint_class) {
        return Err(changed());
    }
    let row = envelope
        .decisions_for_objects(std::slice::from_ref(&model.object_id))?
        .into_iter()
        .find(|row| row.object_id == model.object_id)
        .ok_or_else(changed)?;
    if row.anchor_id != model.anchor_id {
        return Err(changed());
    }
    let anchor = match &model.anchor_id {
        Some(anchor_id) => envelope.anchor_condition(anchor_id)?,
        None => None,
    };
    let anchored = matches!(
        anchor,
        Some(AnchorCondition::Git(GitCondition::ReachableFrom { oid, .. }))
            if oid == input.claim.oid()
    );
    let claim = &input.claim;
    let cited = envelope
        .live_source_descriptor_of_lineage(OccurrenceClass::RepositoryDocuments, &input.lineage_id)?
        .is_some_and(|descriptor| {
            let field = |name: &str| crate::git_reconcile::identity_field(&descriptor, name);
            descriptor.object_id == input.descriptor_object_id
                && descriptor.detail.evidence_id == input.evidence_id
                && descriptor.detail.revision == input.descriptor_revision
                && field("relative_path") == Some(claim.relative_path())
                && field("repository_id") == Some(input.repository_id.as_str())
                && crate::memory_capture::cite::digest_bytes(&descriptor.detail.artifact_digest)
                    == Some(*claim.digest())
                && descriptor.byte_length == claim.byte_length()
        });
    if !anchored || !cited {
        return Err(changed());
    }
    // The classifier reads the row's own citation first and the newest events after it, so the
    // own citation is checked outside the recency-bounded event window.
    let own = row.evidence_id.as_deref();
    let max = super::classify::MAX_EVENT_CITATIONS;
    let cites = own == Some(input.evidence_id.as_str())
        || envelope
            .linked_decision_evidence(&model.object_id, max.saturating_add(1))?
            .iter()
            .filter(|evidence| Some(evidence.evidence_id.as_str()) != own)
            .take(max.get())
            .any(|evidence| evidence.evidence_id == input.evidence_id);
    if !cites {
        return Err(changed());
    }
    Ok((state.scope_id, prior))
}

/// The write a verdict calls for: the attestation on `Match`, the `MarkStale` on `Mismatch`.
#[derive(Clone, Copy)]
enum Write {
    Attest,
    Stale,
}

/// The intent for `write` on `input`: the key names the write, the model decision, and the oid,
/// and the request digest covers the frozen claim, the evidence it resolved through, the blob,
/// and the verdict.
fn intent(input: &VerifierInput, write: Write, blob_oid: &str) -> CommitIntent {
    let (write_name, verdict) = match write {
        Write::Attest => ("attest", "match"),
        Write::Stale => ("stale", "mismatch"),
    };
    let claim = &input.claim;
    let model = &input.decision.object_id;
    verifier_intent(
        format!("{write_name}:{model}:{}", claim.oid()),
        &[
            model,
            &input.decision.source_revision.to_string(),
            claim.oid(),
            claim.relative_path(),
            &hex(claim.digest()),
            &claim.byte_length().to_string(),
            &input.evidence_id,
            blob_oid,
            verdict,
        ],
    )
}

/// The verifier's commit intent for `operation_key`, whose request digest is the SHA-256 of a
/// preimage led by [`REQUEST_DIGEST_FORMAT`] and followed by each of `parts` as its big-endian
/// `u64` length and bytes.
pub(super) fn verifier_intent(operation_key: String, parts: &[&str]) -> CommitIntent {
    let mut digest = Sha256::new();
    for part in std::iter::once(&REQUEST_DIGEST_FORMAT).chain(parts) {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    CommitIntent {
        producer: PRODUCER.into(),
        operation_key,
        request_digest: hex(&digest.finalize()),
        actor: "agent:verifier".into(),
        cause: "deterministic attestation verifier".into(),
    }
}

/// The id prefixes [`Identity::of`] writes under, in field order: object, decision, witness
/// observation, witness object. The plugin commit route refuses rows under them, so no caller can
/// occupy an attestation's ids before the verifier writes them.
pub const ID_PREFIXES: [&str; 4] = [
    "verattest:",
    "verattestdec:",
    "verwitness:",
    "verwitnessobj:",
];

/// The tag that leads the request digest's preimage: each input follows as its big-endian `u64`
/// length and bytes, so a change to the layout moves the tag and old receipts read as `Conflict`
/// rather than replaying under a digest computed another way.
const REQUEST_DIGEST_FORMAT: &str = "verifier-request.v1";

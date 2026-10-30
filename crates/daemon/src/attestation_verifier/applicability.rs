//! Attestation applicability: an attestation stops standing for its document once source capture
//! republishes the document with other bytes.

use kernel::applicability::EvalBudget;
use kernel::source_identity::OccurrenceClass;
use kernel::{
    AdmissionEvent, AdmissionRequest, CommitIntent, CommitReadTarget, Disposition, EventKind,
    KernelError, KernelStore,
};
use sha2::{Digest as _, Sha256};

use super::attest::{
    Identity, Outcome, PRODUCER, Skip, Stop, WITNESS_DETAIL_VERSION, WitnessDetail, run,
};
use super::classify::VERIFIER_SOURCE_KIND;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applicability {
    /// The live descriptor's digest is the witnessed digest.
    Current,
    /// The lineage has no live descriptor, or the witness detail is absent or does not decode.
    Unavailable,
    /// The live descriptor's digest differs from the witnessed digest.
    Republished(Republished),
}

/// An attestation whose document's live descriptor carries other bytes than it witnessed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Republished {
    pub attestation_object_id: String,
    pub lineage_id: String,
    pub relative_path: String,
    pub witnessed_digest: String,
    pub descriptor_object_id: String,
    pub descriptor_revision: String,
    pub descriptor_digest: String,
}

pub fn evaluate(
    kernel: &KernelStore,
    attestation_object_id: &str,
    target: CommitReadTarget,
    budget: &EvalBudget,
) -> Result<Applicability, KernelError> {
    let Some(identity) = Identity::of_attestation(attestation_object_id) else {
        return Ok(Applicability::Unavailable);
    };
    let requested = target.through_commit;
    let witness = kernel.observation_for_object_as_of_within_budget(
        &identity.witness_object_id,
        requested,
        budget,
    )?;
    let Some(detail) = witness
        .and_then(|row| row.payload.detail)
        .and_then(|detail| serde_json::from_str::<WitnessDetail>(&detail).ok())
        .filter(|detail| detail.witness_version == WITNESS_DETAIL_VERSION)
    else {
        return Ok(Applicability::Unavailable);
    };
    let live = kernel.live_source_descriptor_of_lineage_within_budget(
        OccurrenceClass::RepositoryDocuments,
        &detail.lineage_id,
        requested,
        budget,
    )?;
    Ok(match live {
        None => Applicability::Unavailable,
        Some(live) if live.detail.artifact_digest == detail.artifact_digest => {
            Applicability::Current
        }
        Some(live) => Applicability::Republished(Republished {
            attestation_object_id: attestation_object_id.to_owned(),
            lineage_id: detail.lineage_id,
            relative_path: detail.relative_path,
            witnessed_digest: detail.artifact_digest,
            descriptor_object_id: live.object_id,
            descriptor_revision: live.detail.revision,
            descriptor_digest: live.detail.artifact_digest,
        }),
    })
}

pub fn mark_republished(
    kernel: &KernelStore,
    target: CommitReadTarget,
    republished: &Republished,
    budget: &EvalBudget,
) -> Outcome {
    let attestation = &republished.attestation_object_id;
    let frozen = serde_json::json!({
        "attestation_object_id": attestation,
        "lineage_id": republished.lineage_id,
        "witnessed_digest": republished.witnessed_digest,
        "descriptor_object_id": republished.descriptor_object_id,
        "descriptor_digest": republished.descriptor_digest,
    });
    let intent = CommitIntent {
        producer: PRODUCER.into(),
        operation_key: format!(
            "applicability:{attestation}:{}",
            republished.descriptor_revision
        ),
        request_digest: format!("{:x}", Sha256::digest(frozen.to_string())),
        actor: "agent:verifier".into(),
        cause: "deterministic attestation verifier".into(),
    };
    let reason = format!(
        "verifier: the document at {} was republished at descriptor revision {} with sha256 {}, \
         which differs from the attested sha256 {}",
        republished.relative_path,
        republished.descriptor_revision,
        republished.descriptor_digest,
        republished.witnessed_digest,
    );
    run(
        kernel,
        budget,
        intent,
        None,
        Outcome::StaleCommitted,
        |envelope| {
            let changed = || Stop::Outcome(Outcome::Changed);
            kernel
                .require_incarnation(target.incarnation)
                .map_err(|_| changed())?;
            let state = envelope.object_state(attestation)?.ok_or_else(changed)?;
            let object = &state.object;
            if object.object_kind != "decision"
                || object.invalidated_commit_seq.is_some()
                || object.superseded_by.is_some()
                || object.source_kind != VERIFIER_SOURCE_KIND
            {
                return Err(changed());
            }
            let (prior, _) = envelope
                .subject_admission(attestation)?
                .ok_or_else(changed)?;
            match prior.disposition {
                Disposition::Active => {}
                Disposition::Stale => {
                    return Err(Stop::Outcome(Outcome::Skipped(Skip::AlreadyStale)));
                }
                _ => return Err(changed()),
            }
            let live = envelope.live_source_descriptor_of_lineage(
                OccurrenceClass::RepositoryDocuments,
                &republished.lineage_id,
            )?;
            if live.map(|live| live.object_id) != Some(republished.descriptor_object_id.clone()) {
                return Err(changed());
            }
            let marked = envelope.record_admission(AdmissionRequest {
                candidate_id: None,
                subject_object_id: Some(attestation.clone()),
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
        },
    )
}

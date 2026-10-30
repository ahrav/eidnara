//! Attestation applicability: an attestation stops standing for its document once source capture
//! republishes the document with other bytes.

use kernel::applicability::EvalBudget;
use kernel::source_identity::OccurrenceClass;
use kernel::{CommitReadTarget, KernelError, KernelStore, LiveDescriptor};

use super::attest::{
    Identity, Outcome, Stop, WITNESS_DETAIL_VERSION, WitnessDetail, record_stale, run,
    verifier_intent,
};
use super::classify::VERIFIER_SOURCE_KIND;

/// How an attestation's witnessed document stands against its lineage's live descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applicability {
    /// The live descriptor's digest is the witnessed digest.
    Current,
    /// The id is outside the `verattest:` prefix, the lineage has no live descriptor, or the
    /// witness or its detail is absent, does not decode, or has a version other than
    /// [`WITNESS_DETAIL_VERSION`].
    Unavailable,
    /// The live descriptor's digest differs from the witnessed digest.
    Republished(Box<Republished>),
}

/// An attestation whose document's live descriptor carries other bytes than it witnessed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Republished {
    attestation_object_id: String,
    witnessed: WitnessDetail,
    live: LiveDescriptor,
}

/// The applicability of attestation `attestation_object_id` at `target`: its witness detail's
/// digest compared with the digest of its lineage's live document descriptor, both read at
/// `target`'s commit under `budget`.
///
/// # Errors
///
/// Returns the Kernel's refusal of either read, [`KernelError::Deadline`] among them when
/// `budget` runs out.
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
        Some(live) => Applicability::Republished(Box::new(Republished {
            attestation_object_id: attestation_object_id.to_owned(),
            witnessed: detail,
            live,
        })),
    })
}

/// Records one `MarkStale` on the attestation of `republished` under the classes of its latest
/// admission, keyed by the attestation and the live descriptor revision. The envelope rereads the
/// attestation and its lineage's live descriptor: a changed store returns `Changed`, an
/// attestation already `Stale` returns `Skipped(AlreadyStale)`, a key recorded with the same
/// request digest returns `Replayed` and with another `Conflict`, a mark the Kernel leaves short
/// of `Stale` returns `InvalidWriteSet`, a refused read or commit returns `Refused`, and a
/// committed mark returns `StaleCommitted`.
pub fn mark_republished(
    kernel: &KernelStore,
    target: CommitReadTarget,
    republished: &Republished,
    budget: &EvalBudget,
) -> Outcome {
    let Republished {
        attestation_object_id: attestation,
        witnessed,
        live,
    } = republished;
    let (revision, digest) = (&live.detail.revision, &live.detail.artifact_digest);
    let frozen = serde_json::json!({
        "attestation_object_id": attestation,
        "witnessed_digest": witnessed.artifact_digest,
        "descriptor_object_id": live.object_id,
        "artifact_digest": digest,
    });
    let intent = verifier_intent(format!("applicability:{attestation}:{revision}"), &frozen);
    let reason = format!(
        "verifier: the document at {} was republished at descriptor revision {revision} with \
         sha256 {digest}, which differs from the attested sha256 {}",
        witnessed.relative_path, witnessed.artifact_digest,
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
            let current = envelope.live_source_descriptor_of_lineage(
                OccurrenceClass::RepositoryDocuments,
                &witnessed.lineage_id,
            )?;
            if current.map(|current| current.object_id).as_ref() != Some(&live.object_id) {
                return Err(changed());
            }
            record_stale(envelope, attestation, &prior, reason)
        },
    )
}

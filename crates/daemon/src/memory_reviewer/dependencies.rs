//! A `ReviewDependencies` record lets a resumed worker, a takeover, or a reader revalidate a proposal without the run that staged it.
//!
//! A run's broker holds every disclosed reference as an alias with a Kernel expectation, and settlement revalidates those aliases before it stages. The broker is process-local, so nothing after the run has it. The record persists what the broker knew that the proposal payload does not: the complete policy union, with each member's kind, id, revision, owner, and owner revision, and the attempt marker whose response the proposal is. Revalidation rebuilds each member's Kernel expectation from the live store, issues it to a fresh broker, and runs the same per-alias judgement settlement runs, so a resumed claim adopts a result only under the checks a live run would pass, and a selected read refuses a result whose cited or uncited lineage has moved.

use context_core::memory_reviewer_policy_union::{PolicyUnion, PolicyUnionMember};
use kernel::{
    KernelStore, MemoryReviewerHoldBinding, REVIEW_DEPENDENCIES_VERSION, ReviewBinding,
    ReviewDependencies, ReviewOwner, ReviewQuestionTemplate, ReviewStagedReference,
    ReviewStagedRow,
};
use memory_store::memory_reviewer_ledger::{
    AbstainReason, MemoryReviewerAttempt, MemoryReviewerAttemptTerminal,
};

use super::broker::{
    EvidenceBroker, HeldUnder, QuestionTemplate, ReferenceExpectation, RefusalCode, RunBinding,
    hold_refusal,
};
use super::coordinator::{InvestigationError, resolve_descriptor};

/// A revalidation outcome: a durable abstention reason, or a store failure that must not become one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Abstain(AbstainReason),
    Store(String),
}

impl Verdict {
    fn changed() -> Self {
        Self::Abstain(AbstainReason::ExpectationChanged)
    }

    /// A resource or store refusal is transient and must not become a durable abstention; every other code says the disclosed input no longer stands as rendered.
    pub(crate) fn from_refusal(code: RefusalCode) -> Self {
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
            | RefusalCode::UnsupportedQuestion => Self::changed(),
        }
    }

    /// A hold refusal judged the way the broker judges one, so a backing limit stays transient here as it does on a live read.
    pub(crate) fn from_hold(error: kernel::MemoryReviewerHoldError) -> Self {
        Self::from_refusal(hold_refusal(error))
    }
}

/// The record for a proposal produced at `generation`: the broker's complete union and the last completed marker at that generation, whose response the proposal is. `None` when no completed marker exists at the generation, which no proposal can be the response of.
pub fn record(
    broker: &EvidenceBroker,
    attempts: &[MemoryReviewerAttempt],
    generation: u64,
) -> Result<Option<ReviewDependencies>, String> {
    let union = broker
        .ledger
        .union()
        .encode()
        .map_err(|error| error.to_string())?;
    let Some(marker) = attempts
        .iter()
        .filter(|attempt| {
            attempt.generation == generation
                && matches!(
                    attempt.terminal,
                    Some((MemoryReviewerAttemptTerminal::Complete, _))
                )
        })
        .max_by_key(|attempt| attempt.attempt_index)
    else {
        return Ok(None);
    };
    Ok(Some(ReviewDependencies {
        version: REVIEW_DEPENDENCIES_VERSION,
        union_canonical: union.canonical,
        union_digest: union.digest,
        generation,
        attempt_index: marker.attempt_index,
        body_digest: marker.marker.body_digest.clone(),
        marker_union_digest: marker.marker.policy_union_digest.clone(),
    }))
}

/// Everything a revalidation needs beside the row: the run whose result it is, the job binding staged rows are read under, the hold that protects the disclosed inputs, and where the reader would send bytes.
pub struct Revalidation<'a> {
    pub store: &'a KernelStore,
    pub run: &'a MemoryReviewerHoldBinding,
    pub job_binding: &'a ReviewBinding,
    pub hold: HeldUnder<'a>,
    pub destination: kernel::ArtifactDestination,
    pub now: i64,
}

/// Revalidates a sealed proposal row from its dependency record alone. Refuses a row without a record, a record whose canonical bytes do not re-encode to their digest, a record whose marker is not a completed attempt at the row's generation with the recorded digests, any member whose live kind, revision, owner, owner revision, scope, or egress no longer matches, and a record whose members do not produce exactly the payload's disclosed inputs and ancestry. Returns the disclosed evidence ids the hold validated.
pub fn revalidate(
    revalidation: &Revalidation<'_>,
    reference: &ReviewStagedReference,
    row: &ReviewStagedRow,
    attempts: &[MemoryReviewerAttempt],
) -> Result<Vec<String>, Verdict> {
    let kernel::ReviewPayload::Proposal(proposal) = &row.payload else {
        return Err(Verdict::changed());
    };
    let dependencies = row.dependencies.as_ref().ok_or_else(Verdict::changed)?;
    if dependencies.generation != revalidation.run.generation {
        return Err(Verdict::changed());
    }
    let union = PolicyUnion::decode(&dependencies.union_canonical, &dependencies.union_digest)
        .map_err(|_| Verdict::changed())?;
    let marker_matches = attempts.iter().any(|attempt| {
        attempt.generation == dependencies.generation
            && attempt.attempt_index == dependencies.attempt_index
            && matches!(
                attempt.terminal,
                Some((MemoryReviewerAttemptTerminal::Complete, _))
            )
            && attempt.marker.body_digest == dependencies.body_digest
            && attempt.marker.policy_union_digest == dependencies.marker_union_digest
    });
    if !marker_matches {
        return Err(Verdict::changed());
    }
    let question = match proposal.policy_dependencies.question_template {
        ReviewQuestionTemplate::ExtractedFacts => QuestionTemplate::ExtractedFacts,
    };
    let mut broker = EvidenceBroker::new(
        RunBinding {
            hold: revalidation.run.clone(),
            hold_id: revalidation.hold.hold_id().to_string(),
            destination: revalidation.destination,
        },
        question,
    )
    .map_err(|error| Verdict::Store(error.to_string()))?;
    let mut evidence = Vec::new();
    let mut ancestry = Vec::new();
    for member in union.members() {
        if member.kind == "question_template" {
            if member.id != question.id() || member.revision != question.revision() {
                return Err(Verdict::changed());
            }
            continue;
        }
        let expectation = expectation(revalidation, reference, member)?;
        if let Some(owner) = &member.owner_id {
            ancestry.push(owner.clone());
        }
        let alias = broker.aliases.issue(expectation);
        if let Some(id) = broker
            .revalidate_under(
                revalidation.store,
                alias.as_str(),
                revalidation.now,
                revalidation.hold,
            )
            .map_err(|refusal| Verdict::from_refusal(refusal.code))?
        {
            evidence.push(id);
        }
    }
    evidence.sort();
    evidence.dedup();
    ancestry.sort();
    ancestry.dedup();
    let mut disclosed: Vec<&str> = proposal
        .policy_dependencies
        .disclosed_inputs
        .iter()
        .map(|input| input.evidence_id.as_str())
        .collect();
    disclosed.sort_unstable();
    disclosed.dedup();
    if disclosed != evidence.iter().map(String::as_str).collect::<Vec<_>>()
        || ancestry != proposal.policy_dependencies.ancestry
    {
        return Err(Verdict::changed());
    }
    revalidation
        .store
        .validate_held_evidence(
            revalidation.hold.hold_id(),
            revalidation.hold.kind(),
            revalidation.hold.binding(),
            &evidence,
            revalidation.now,
        )
        .map_err(Verdict::from_hold)?;
    Ok(evidence)
}

/// The live Kernel expectation one persisted member names. A staged subject is the job's own row; a native or canonical descriptor is resolved at the recorded revision and its recorded owner and owner revision must be the ones the live decision carries; a capture is the held evidence at the recorded digest.
fn expectation(
    revalidation: &Revalidation<'_>,
    reference: &ReviewStagedReference,
    member: &PolicyUnionMember,
) -> Result<ReferenceExpectation, Verdict> {
    let changed = Verdict::changed;
    match member.kind.as_str() {
        "staged_subject" => {
            if member.owner_id.is_some()
                || !matches!(revalidation.job_binding.owner, ReviewOwner::Job { .. })
            {
                return Err(changed());
            }
            Ok(ReferenceExpectation::StagedSubject {
                reference: ReviewStagedReference {
                    database_incarnation_id: reference.database_incarnation_id.clone(),
                    candidate_id: member.id.clone(),
                    payload_digest: member.revision.clone(),
                },
                binding: revalidation.job_binding.clone(),
            })
        }
        "native_source" | "canonical_source" => {
            let revision: i64 = member.revision.parse().map_err(|_| changed())?;
            let (expectation, _) =
                resolve_descriptor(revalidation.store, &member.id, Some(revision)).map_err(
                    |error| match error {
                        InvestigationError::Refused(code) | InvestigationError::Kernel(code) => {
                            Verdict::from_refusal(code)
                        }
                        other => Verdict::Store(other.to_string()),
                    },
                )?;
            let consistent = match (&expectation, member.kind.as_str()) {
                (ReferenceExpectation::NativeSource { .. }, "native_source") => {
                    member.owner_id.is_none() && member.owner_revision.is_none()
                }
                (
                    ReferenceExpectation::CanonicalSource {
                        originating_decision_id,
                        decision_source_revision,
                        ..
                    },
                    "canonical_source",
                ) => {
                    member.owner_id.as_deref() == Some(originating_decision_id.as_str())
                        && member.owner_revision.as_deref()
                            == Some(decision_source_revision.to_string().as_str())
                }
                _ => false,
            };
            if !consistent {
                return Err(changed());
            }
            Ok(expectation)
        }
        "temporary_capture" => {
            if member.owner_id.is_some() {
                return Err(changed());
            }
            let held = revalidation
                .store
                .validate_held_evidence(
                    revalidation.hold.hold_id(),
                    revalidation.hold.kind(),
                    revalidation.hold.binding(),
                    std::slice::from_ref(&member.id),
                    revalidation.now,
                )
                .map_err(Verdict::from_hold)?
                .pop()
                .ok_or_else(changed)?;
            if held.artifact_digest != member.revision {
                return Err(changed());
            }
            Ok(ReferenceExpectation::TemporaryCapture {
                evidence_id: member.id.clone(),
                artifact_digest: held.artifact_digest,
                byte_length: held.byte_length,
                retain_until: held.retain_until.ok_or_else(changed)?,
            })
        }
        _ => Err(changed()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusal_codes_map_to_verdicts() {
        assert_eq!(
            Verdict::from_refusal(RefusalCode::PolicyBlocked),
            Verdict::Abstain(AbstainReason::OwnerSensitive)
        );
        assert_eq!(
            Verdict::from_refusal(RefusalCode::Scope),
            Verdict::Abstain(AbstainReason::WrongScope)
        );
        assert_eq!(
            Verdict::from_refusal(RefusalCode::OriginRevoked),
            Verdict::Abstain(AbstainReason::ExpectationChanged)
        );
        assert!(matches!(
            Verdict::from_refusal(RefusalCode::Store),
            Verdict::Store(_)
        ));
    }
}

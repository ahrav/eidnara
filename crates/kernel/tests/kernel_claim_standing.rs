//! At every snapshot, the standing read and the claim-facts read return the
//! same registry row, decision row, own admission row, and served standing
//! for each claim.

#![cfg(feature = "test-support")]

mod claim_fixture;

use std::num::NonZeroUsize;

use claim_fixture::{Fixture, admission, bounds, decision, direct, intent, request};
use kernel::applicability::EvalBudget;
use kernel::{
    AdmissionEvent, AdmissionRequest, ClaimFactsError, ClaimStanding, CommitReadTarget, EventKind,
    KernelError, Sensitivity, SourceClass, TaintClass,
};

fn event(object_id: &str, kind: EventKind) -> AdmissionRequest {
    AdmissionRequest {
        event: AdmissionEvent {
            kind,
            ..admission(object_id).event
        },
        ..admission(object_id)
    }
}

fn standing_at(
    fixture: &Fixture,
    ids: &[String],
    target: CommitReadTarget,
    max_claims: usize,
) -> Result<Vec<Option<ClaimStanding>>, ClaimFactsError> {
    fixture.store.claim_standing_at_within_budget(
        ids,
        target,
        NonZeroUsize::new(max_claims).unwrap(),
        &EvalBudget::unbounded(),
    )
}

/// The fixture seeds admission states, a published descriptor, and a
/// causality record to exercise claim-facts occurrences and causal classes.
fn seeded() -> (Fixture, Vec<String>) {
    let fixture = Fixture::open();
    for index in 1..=9 {
        fixture.admit_decision(index, 1);
    }
    let events = [
        (2, EventKind::MarkStale),
        (3, EventKind::MarkDisputed),
        (4, EventKind::Contradict),
        (5, EventKind::ExplicitReject),
        (6, EventKind::Quarantine),
    ];
    for (index, kind) in events {
        fixture
            .store
            .commit(intent(&format!("event-{index}")), |envelope| {
                envelope.record_admission(event(&format!("decision-object-{index}"), kind))?;
                Ok(String::new())
            })
            .unwrap();
    }
    fixture
        .store
        .commit(intent("never-admitted"), |envelope| {
            envelope.insert_decision(decision(10, 1))?;
            envelope.insert_decision(decision(11, 1))?;
            Ok(String::new())
        })
        .unwrap();
    fixture
        .store
        .commit(intent("secret"), |envelope| {
            envelope.insert_decision(kernel::DecisionSpec {
                sensitivity: Sensitivity::Secret,
                ..decision(12, 1)
            })?;
            envelope.record_admission(AdmissionRequest {
                source_class: Some(SourceClass::TrustedLocalCode),
                taint_class: Some(TaintClass::CurrentCode),
                ..admission("decision-object-12")
            })?;
            Ok(String::new())
        })
        .unwrap();
    fixture.publish_summary(1, 1);
    let (evidence_id, digest) = fixture.retain("acquisition", "observed text");
    fixture
        .record(
            "direct",
            request("decision-object-7", 1, direct(&evidence_id, &digest)),
        )
        .unwrap();
    fixture
        .store
        .commit(intent("retire"), |envelope| {
            envelope.retire_decision("decision-object-8")?;
            Ok(String::new())
        })
        .unwrap();
    let mut ids: Vec<String> = (1..=12)
        .map(|index| format!("decision-object-{index}"))
        .collect();
    ids.push("never-registered".to_string());
    (fixture, ids)
}

#[test]
fn every_standing_equals_the_claim_facts_of_its_claim_at_every_snapshot() {
    let (fixture, ids) = seeded();
    let tip = fixture.store.capture_commit_read_target().unwrap();
    for through_commit in 1..=tip.through_commit {
        let target = CommitReadTarget {
            through_commit,
            ..tip
        };
        let facts = fixture
            .store
            .claim_facts_at(&ids, target, bounds())
            .unwrap();
        let standing = standing_at(&fixture, &ids, target, 64).unwrap();
        let mut claims = facts.claims.iter();
        let projected: Vec<Option<ClaimStanding>> = ids
            .iter()
            .map(|id| {
                if facts.missing.contains(id) {
                    return None;
                }
                let claim = claims.next().unwrap();
                assert_eq!(&claim.object.object_id, id);
                Some(ClaimStanding {
                    object: claim.object.clone(),
                    decision: claim.decision.clone(),
                    own_admission: claim.own_admission.clone(),
                    served: claim.served.clone(),
                })
            })
            .collect();
        assert_eq!(standing, projected, "at {through_commit}");
    }
    let facts = fixture.store.claim_facts_at(&ids, tip, bounds()).unwrap();
    assert!(
        facts
            .claims
            .iter()
            .any(|claim| !claim.occurrences.is_empty()),
        "the claim-facts read carries an occurrence the standing read omits"
    );
    assert!(
        facts
            .claims
            .iter()
            .any(|claim| claim.causal_record.is_some()),
        "the claim-facts read carries a causal record the standing read omits"
    );
    assert!(
        facts
            .claims
            .iter()
            .any(|claim| claim.object.invalidated_commit_seq.is_some()),
        "a retired claim is read"
    );
}

#[test]
fn standing_refusals_match_the_claim_facts_refusals() {
    let (fixture, ids) = seeded();
    let target = fixture.store.capture_commit_read_target().unwrap();
    let pair = [ids[0].clone(), ids[1].clone()];
    assert_eq!(
        standing_at(&fixture, &pair, target, 1),
        Err(ClaimFactsError::TooManyClaims)
    );
    assert_eq!(
        standing_at(&fixture, &[ids[0].clone(), ids[0].clone()], target, 64),
        Err(ClaimFactsError::DuplicateClaim)
    );
    assert_eq!(
        standing_at(&fixture, &["domain-object".to_string()], target, 64),
        Err(ClaimFactsError::NotADecision)
    );
    assert_eq!(
        standing_at(&fixture, &[String::new()], target, 64),
        Err(ClaimFactsError::Kernel(KernelError::InvalidInput))
    );
    let future = CommitReadTarget {
        through_commit: target.through_commit + 1,
        ..target
    };
    assert_eq!(
        standing_at(&fixture, &ids, future, 64),
        Err(ClaimFactsError::Kernel(KernelError::FutureSnapshot))
    );
    let other = Fixture::open();
    let foreign = CommitReadTarget {
        through_commit: 1,
        ..other.store.capture_commit_read_target().unwrap()
    };
    assert_eq!(
        standing_at(&fixture, &ids, foreign, 64),
        Err(ClaimFactsError::IncarnationMismatch)
    );
    let expired = EvalBudget::new(
        Some(std::time::Instant::now() - std::time::Duration::from_millis(1)),
        Default::default(),
    );
    assert_eq!(
        fixture.store.claim_standing_at_within_budget(
            &ids,
            target,
            NonZeroUsize::new(64).unwrap(),
            &expired
        ),
        Err(ClaimFactsError::Kernel(KernelError::Deadline))
    );
}

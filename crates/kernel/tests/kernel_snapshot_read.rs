#![cfg(feature = "test-support")]

mod claim_fixture;

use std::num::NonZeroUsize;
use std::sync::Arc;

use claim_fixture::{Fixture, admission, decision, intent};
use kernel::applicability::EvalBudget;
use kernel::source_identity::OccurrenceClass;
use kernel::{
    AdmissionEvent, CitingDescriptor, ClaimFactsError, CommitReadTarget, DecisionStanding,
    EventKind, EvidenceLiveness, KernelError, SourceClass, TaintClass,
};

fn standing(fixture: &Fixture, id: &str, target: CommitReadTarget) -> Option<DecisionStanding> {
    fixture
        .store
        .claim_standing_at_within_budget(
            &[id.to_string()],
            target,
            NonZeroUsize::MIN,
            &EvalBudget::unbounded(),
        )
        .unwrap()
        .pop()
        .flatten()
        .map(|standing| {
            let evidence_liveness = standing.decision.evidence_id.as_deref().map(|evidence_id| {
                fixture
                    .store
                    .evidence_liveness_at_within_budget(
                        &[evidence_id],
                        target,
                        &EvalBudget::unbounded(),
                    )
                    .unwrap()[0]
            });
            DecisionStanding {
                object: standing.object,
                decision: standing.decision,
                own_admission: standing.own_admission,
                evidence_liveness,
            }
        })
}

fn read<T>(
    fixture: &Fixture,
    target: CommitReadTarget,
    read: impl FnOnce(&kernel::SnapshotRead<'_>) -> Result<T, ClaimFactsError>,
) -> Result<T, ClaimFactsError> {
    fixture
        .store
        .read_at(target, &EvalBudget::unbounded(), read)
}

fn delete(fixture: &Fixture, key: &str, digest: String) {
    fixture
        .store
        .delete_artifact(kernel::ArtifactDeletionRequest {
            intent: intent(key),
            identity: kernel::ArtifactDeletionIdentity::Digest(digest),
            kind: kernel::ArtifactDeletionKind::Delete,
            operator_id: None,
            target_locator: None,
            reason: None,
            deleted_at: 1,
        })
        .unwrap();
}

fn seeded() -> (Fixture, Vec<String>) {
    let fixture = Fixture::open();
    fixture.seed_approval();
    for index in 1..=3 {
        fixture.admit_decision(index, 1);
    }
    let mut approved = admission("decision-object-4");
    approved.source_class = Some(SourceClass::ModelInference);
    approved.taint_class = Some(TaintClass::AssistantInference);
    approved.event = AdmissionEvent {
        kind: EventKind::Verify,
        trigger_object_id: None,
        approval_object_id: Some("approval".to_string()),
        evidence_id: None,
        reason: "verified under approval".to_string(),
    };
    let (cited, cited_digest) = fixture.retain("cited", "cited text");
    let (late, _) = fixture.retain("late", "late text");
    fixture
        .store
        .commit(intent("approved"), |envelope| {
            for (index, evidence_id) in [(6, &cited), (7, &late)] {
                envelope.insert_decision(kernel::DecisionSpec {
                    evidence_id: Some(evidence_id.clone()),
                    ..decision(index, 1)
                })?;
                envelope.record_admission(admission(&format!("decision-object-{index}")))?;
            }
            envelope.insert_decision(decision(4, 1))?;
            envelope.record_admission(approved)?;
            envelope.insert_decision(decision(5, 1))?;
            let mut stale = admission("decision-object-2");
            stale.event.kind = EventKind::MarkStale;
            envelope.record_admission(stale)?;
            envelope.retire_decision("decision-object-3")?;
            Ok(String::new())
        })
        .unwrap();
    fixture
        .store
        .commit(intent("revoke"), |envelope| {
            envelope.revoke_approval("approval", "authority revoked")?;
            Ok(String::new())
        })
        .unwrap();
    delete(&fixture, "delete-cited", cited_digest);
    let latest: String = rusqlite::Connection::open(fixture.root.path().join("kernel.sqlite"))
        .unwrap()
        .query_row(
            "SELECT admission_decision_id FROM admission_decisions
             WHERE subject_object_id='decision-object-4'
             ORDER BY commit_seq DESC,admission_decision_id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    fixture.sql(&format!(
        "UPDATE admission_decisions SET approval_object_id='approval'
         WHERE admission_decision_id='{latest}';
         UPDATE evidence_meta SET created_commit_seq={tip} WHERE evidence_id='{late}';",
        tip = fixture.tip(),
    ));
    let mut ids: Vec<String> = (1..=7)
        .map(|index| format!("decision-object-{index}"))
        .collect();
    ids.push("never-registered".to_string());
    (fixture, ids)
}

#[test]
fn the_decision_standing_equals_the_standing_read_without_its_serving_facts_at_every_snapshot() {
    let (fixture, ids) = seeded();
    let tip = fixture.store.capture_commit_read_target().unwrap();
    let mut approvals = Vec::new();
    let mut cited = Vec::new();
    for through_commit in 1..=tip.through_commit {
        let target = CommitReadTarget {
            through_commit,
            ..tip
        };
        for id in &ids {
            let read = read(&fixture, target, |snapshot| snapshot.decision_standing(id)).unwrap();
            assert_eq!(
                read,
                standing(&fixture, id, target),
                "{id} at {through_commit}"
            );
            cited.extend(
                read.as_ref()
                    .and_then(|standing| standing.evidence_liveness),
            );
            if let Some(approval) = read
                .and_then(|standing| standing.own_admission)
                .and_then(|admission| admission.supporting_approval)
            {
                approvals.push(approval.valid_at_snapshot);
            }
        }
    }
    assert!(
        approvals.contains(&true) && approvals.contains(&false),
        "a supporting approval is read both valid and revoked: {approvals:?}"
    );
    for liveness in [
        EvidenceLiveness::Live,
        EvidenceLiveness::Retired,
        EvidenceLiveness::Unknown,
    ] {
        assert!(
            cited.contains(&liveness),
            "a row-level citation is read {liveness:?}: {cited:?}"
        );
    }
    assert_eq!(
        read(&fixture, tip, |snapshot| snapshot
            .decision_standing("domain-object")),
        Err(ClaimFactsError::NotADecision)
    );
    assert_eq!(
        read(&fixture, tip, |snapshot| snapshot.decision_standing("")),
        Err(ClaimFactsError::Kernel(KernelError::InvalidInput))
    );
}

#[test]
fn one_descriptor_statement_answers_each_cited_row_as_its_single_call_read() {
    let fixture = Fixture::open();
    let first = fixture.publish_summary(1, 1);
    let second = fixture.publish_summary(2, 1);
    let retired = fixture.publish_summary(3, 1);
    let corrupt = fixture.publish_summary(4, 1);
    let (uncited, _) = fixture.retain("uncited", "uncited text");
    fixture
        .store
        .commit(intent("retire"), |envelope| {
            envelope.retire_observation(&retired.descriptor_object_id)?;
            Ok(String::new())
        })
        .unwrap();
    fixture.sql(&format!(
        "UPDATE observations SET observation_payload=x'00' WHERE object_id='{}';",
        corrupt.descriptor_object_id
    ));
    let ids = [
        second.evidence_id.as_str(),
        corrupt.evidence_id.as_str(),
        retired.evidence_id.as_str(),
        uncited.as_str(),
        "missing",
        first.evidence_id.as_str(),
        second.evidence_id.as_str(),
    ];
    let class = OccurrenceClass::CanonicalClaims;
    let tip = fixture.store.capture_commit_read_target().unwrap();
    let mut entries = Vec::new();
    for through_commit in 1..=tip.through_commit {
        let target = CommitReadTarget {
            through_commit,
            ..tip
        };
        let single: Vec<_> = ids
            .iter()
            .map(|evidence_id| {
                fixture.store.source_descriptor_citing_evidence(
                    class,
                    evidence_id,
                    through_commit,
                    &EvalBudget::unbounded(),
                )
            })
            .collect();
        entries = read(&fixture, target, |snapshot| {
            Ok(snapshot.source_descriptors_citing_evidence(class, &ids)?)
        })
        .unwrap();
        assert_eq!(entries, single, "at {through_commit}");
    }
    let shape: Vec<_> = entries
        .iter()
        .map(|entry| match entry {
            Ok(CitingDescriptor::Live(descriptor)) => Ok(Some(descriptor.object_id.as_str())),
            Ok(CitingDescriptor::Retired) => Ok(None),
            Ok(CitingDescriptor::Uncited) => Err("uncited"),
            Err(_) => Err("refused"),
        })
        .collect();
    assert_eq!(
        shape,
        [
            Ok(Some(second.descriptor_object_id.as_str())),
            Err("refused"),
            Ok(None),
            Err("uncited"),
            Err("uncited"),
            Ok(Some(first.descriptor_object_id.as_str())),
            Ok(Some(second.descriptor_object_id.as_str())),
        ],
        "each entry answers its own id, and the corrupt row refuses its entry alone"
    );
    assert_eq!(entries[1], Err(KernelError::CorruptCanonicalRow));
}

#[test]
fn read_at_refuses_before_and_after_its_closure_runs() {
    let fixture = Fixture::open();
    fixture.admit_decision(1, 1);
    let target = fixture.store.capture_commit_read_target().unwrap();
    let refused = |target: CommitReadTarget, budget: &EvalBudget| {
        fixture
            .store
            .read_at::<(), ClaimFactsError>(target, budget, |_| {
                panic!("the closure runs only after every refusal check passes")
            })
            .unwrap_err()
    };
    let unbounded = EvalBudget::unbounded();
    let future = CommitReadTarget {
        through_commit: target.through_commit + 1,
        ..target
    };
    assert_eq!(
        refused(future, &unbounded),
        ClaimFactsError::Kernel(KernelError::FutureSnapshot)
    );
    let negative = CommitReadTarget {
        through_commit: -1,
        ..target
    };
    assert_eq!(
        refused(negative, &unbounded),
        ClaimFactsError::Kernel(KernelError::InvalidInput)
    );
    let foreign = CommitReadTarget {
        through_commit: 1,
        ..Fixture::open().store.capture_commit_read_target().unwrap()
    };
    assert_eq!(
        refused(foreign, &unbounded),
        ClaimFactsError::IncarnationMismatch
    );
    let cancelled = EvalBudget::new(None, Arc::new(true.into()));
    assert_eq!(
        refused(target, &cancelled),
        ClaimFactsError::Kernel(KernelError::Deadline)
    );
    let budget = EvalBudget::new(None, Arc::new(false.into()));
    assert_eq!(
        fixture
            .store
            .read_at::<(), ClaimFactsError>(target, &budget, |_| {
                budget.cancel();
                Ok(())
            }),
        Err(ClaimFactsError::Kernel(KernelError::Deadline)),
        "a budget cancelled inside the closure refuses the closure's result"
    );
    let budget = EvalBudget::new(None, Arc::new(false.into()));
    assert_eq!(
        fixture
            .store
            .read_at::<(), ClaimFactsError>(target, &budget, |_| {
                budget.cancel();
                Err(ClaimFactsError::NotADecision)
            }),
        Err(ClaimFactsError::Kernel(KernelError::Deadline)),
        "a budget cancelled inside the closure refuses the closure's refusal"
    );
}

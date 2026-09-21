//! Independently authored expected verdicts for `KernelStore::judge_eligibility`.

#![cfg(feature = "test-support")]

#[path = "support/eligibility_fixture.rs"]
mod eligibility_fixture;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use eligibility_fixture::{
    Fixture, PROJECT_A, PROJECT_B, SCOPE_A, SCOPE_B, admission, artifact, branch_term, candidate,
    decision, fixture, intent, project_term, with_artifact,
};
use kernel::{
    ArtifactDestination, EligibilityCandidate, EligibilityVerdict, EventKind, KernelError,
    MAX_ELIGIBILITY_CANDIDATES, MAX_ELIGIBILITY_OBJECT_ID_BYTES, ProjectScope, ScopeTermSpec,
    Sensitivity,
};

fn expected(
    fixture: &Fixture,
    destination: ArtifactDestination,
) -> Vec<(EligibilityCandidate, EligibilityVerdict)> {
    use EligibilityVerdict::*;
    let remote = destination == ArtifactDestination::Remote;
    let artifact = fixture.sensitive_artifact.as_str();
    vec![
        (candidate("ok", 1), Ok),
        (candidate("automatic", 1), Ok),
        (candidate("retired", 1), Retracted),
        (candidate("replaced", 1), Superseded),
        (candidate("replacement", 2), Ok),
        (candidate("replacement", 1), Stale),
        (candidate("stale", 7), Stale),
        (candidate("other-project", 1), WrongScope),
        (candidate("branch-only-scope", 1), WrongScope),
        (candidate("unscoped", 1), WrongScope),
        (candidate("secret", 1), ProviderSensitive),
        (
            candidate("sensitive", 1),
            if remote { ProviderSensitive } else { Ok },
        ),
        (candidate("unadmitted", 1), Hidden),
        (candidate("contradicted", 1), Hidden),
        (
            with_artifact(candidate("with-artifact", 1), artifact),
            if remote { ProviderSensitive } else { Ok },
        ),
        (candidate("never-written", 1), Retracted),
        // The same object twice at different revisions: verdicts are
        // positional, so the pair must come back in candidate order.
        (candidate("stale", 1), Ok),
        (candidate("stale", 7), Stale),
        // Precedence pairs: each candidate carries two faults and the verdict
        // names the earlier one.
        (candidate("replaced", 9), Superseded),
        (candidate("retired-other-project", 1), Retracted),
        (candidate("stale-other-project", 9), Stale),
        (candidate("secret-other-project", 1), WrongScope),
        (candidate("secret-unadmitted", 1), ProviderSensitive),
        (
            with_artifact(candidate("contradicted-with-artifact", 1), artifact),
            Hidden,
        ),
    ]
}

#[test]
fn every_verdict_class_and_precedence_pair_matches_the_expected_table() {
    let fixture = fixture();
    let project = ProjectScope::new(PROJECT_A).unwrap();
    for destination in [ArtifactDestination::Local, ArtifactDestination::Remote] {
        let table = expected(&fixture, destination);
        let candidates: Vec<EligibilityCandidate> = table
            .iter()
            .map(|(candidate, _)| candidate.clone())
            .collect();
        let batch = fixture
            .store
            .judge_eligibility(&project, destination, &candidates)
            .unwrap();
        assert_eq!(
            batch.snapshot.tip,
            fixture.store.tip().unwrap(),
            "{destination:?}"
        );
        assert!(
            batch.snapshot.classification_generation.is_some(),
            "{destination:?}"
        );
        let judged: Vec<(String, EligibilityVerdict)> = candidates
            .iter()
            .zip(&batch.verdicts)
            .map(|(candidate, verdict)| (candidate.object_id.clone(), *verdict))
            .collect();
        let wanted: Vec<(String, EligibilityVerdict)> = table
            .iter()
            .map(|(candidate, verdict)| (candidate.object_id.clone(), *verdict))
            .collect();
        assert_eq!(judged, wanted, "{destination:?}");
        for (index, candidate) in candidates.iter().enumerate() {
            let single = fixture
                .store
                .judge_eligibility(&project, destination, std::slice::from_ref(candidate))
                .unwrap();
            assert_eq!(
                single.verdicts,
                [batch.verdicts[index]],
                "{destination:?} #{index}"
            );
            assert_eq!(single.snapshot, batch.snapshot);
        }
    }
}

#[test]
fn the_other_project_sees_its_own_rows_and_nothing_of_the_first() {
    let fixture = fixture();
    let project_b = ProjectScope::new(PROJECT_B).unwrap();
    let batch = fixture
        .store
        .judge_eligibility(
            &project_b,
            ArtifactDestination::Local,
            &[
                candidate("other-project", 1),
                candidate("ok", 1),
                candidate("secret-other-project", 1),
                candidate("branch-only-scope", 1),
            ],
        )
        .unwrap();
    assert_eq!(
        batch.verdicts,
        [
            EligibilityVerdict::Ok,
            EligibilityVerdict::WrongScope,
            EligibilityVerdict::ProviderSensitive,
            EligibilityVerdict::WrongScope,
        ]
    );
}

#[test]
fn a_scope_names_a_project_only_through_an_exact_project_term() {
    for malformed in ["", "abc", &"A".repeat(64), &"a".repeat(63), &"a".repeat(65)] {
        assert!(
            matches!(ProjectScope::new(malformed), Err(KernelError::InvalidInput)),
            "{malformed:?}"
        );
    }
    let project = ProjectScope::new(PROJECT_A).unwrap();
    assert!(project.names_project(Some(&[project_term(PROJECT_A)])));
    // A term on a dimension the project context carries no value for is
    // `Uncertain`, and an uncertain scope never serves.
    assert!(!project.names_project(Some(&[branch_term(), project_term(PROJECT_A)])));
    assert!(!project.names_project(Some(&[project_term(PROJECT_B)])));
    assert!(!project.names_project(None));
    assert!(!project.names_project(Some(&[])));
    assert!(!project.names_project(Some(&[branch_term()])));
    let redacted = ScopeTermSpec {
        exact_value: Some(kernel::OPERATOR_REDACTION_PLACEHOLDER.to_string()),
        ..project_term(PROJECT_A)
    };
    assert!(!project.names_project(Some(&[redacted])));
}

#[test]
fn over_bound_and_malformed_batches_are_refused_before_any_read() {
    let fixture = fixture();
    let project = ProjectScope::new(PROJECT_A).unwrap();
    let tip = fixture.store.tip().unwrap();
    // With every reader connection held, a refusal that touched the store
    // would block until the holder released the pool.
    {
        let held = std::sync::Barrier::new(2);
        let hold = std::time::Duration::from_millis(1_500);
        let started = std::time::Instant::now();
        let outcome = thread::scope(|scope| {
            scope.spawn(|| fixture.store.hold_readers_for_test(&held, hold));
            held.wait();
            let too_many: Vec<EligibilityCandidate> = (0..=MAX_ELIGIBILITY_CANDIDATES)
                .map(|index| candidate(&format!("object-{index}"), 1))
                .collect();
            let over =
                fixture
                    .store
                    .judge_eligibility(&project, ArtifactDestination::Local, &too_many);
            let malformed = fixture.store.judge_eligibility(
                &project,
                ArtifactDestination::Local,
                &[candidate("ok", 1), with_artifact(candidate("ok", 1), "abc")],
            );
            (over, malformed, started.elapsed())
        });
        assert!(matches!(outcome.0, Err(KernelError::InvalidInput)));
        assert!(matches!(outcome.1, Err(KernelError::InvalidInput)));
        assert!(
            outcome.2 < hold,
            "the refusals waited for a reader, took {:?}",
            outcome.2
        );
    }
    let too_many: Vec<EligibilityCandidate> = (0..=MAX_ELIGIBILITY_CANDIDATES)
        .map(|index| candidate(&format!("object-{index}"), 1))
        .collect();
    let exact: Vec<EligibilityCandidate> = too_many[..MAX_ELIGIBILITY_CANDIDATES].to_vec();
    for destination in [ArtifactDestination::Local, ArtifactDestination::Remote] {
        assert!(matches!(
            fixture
                .store
                .judge_eligibility(&project, destination, &too_many),
            Err(KernelError::InvalidInput)
        ));
        let batch = fixture
            .store
            .judge_eligibility(&project, destination, &exact)
            .unwrap();
        assert_eq!(batch.verdicts.len(), MAX_ELIGIBILITY_CANDIDATES);
        for bad in [
            candidate("", 1),
            candidate(&"x".repeat(MAX_ELIGIBILITY_OBJECT_ID_BYTES + 1), 1),
            with_artifact(candidate("ok", 1), "abc"),
            with_artifact(candidate("ok", 1), &"A".repeat(64)),
        ] {
            assert!(
                matches!(
                    fixture.store.judge_eligibility(
                        &project,
                        destination,
                        &[candidate("ok", 1), bad]
                    ),
                    Err(KernelError::InvalidInput)
                ),
                "{destination:?}"
            );
        }
        let longest = fixture
            .store
            .judge_eligibility(
                &project,
                destination,
                &[candidate(&"x".repeat(MAX_ELIGIBILITY_OBJECT_ID_BYTES), 1)],
            )
            .unwrap();
        assert_eq!(longest.verdicts, [EligibilityVerdict::Retracted]);
    }
    let empty = fixture
        .store
        .judge_eligibility(&project, ArtifactDestination::Local, &[])
        .unwrap();
    assert!(empty.verdicts.is_empty());
    assert_eq!(fixture.store.tip().unwrap(), tip, "judging moves no tip");
}

#[test]
fn a_tightened_classification_changes_the_verdict_and_the_snapshot_identity() {
    let fixture = fixture();
    let project = ProjectScope::new(PROJECT_A).unwrap();
    let normal = fixture
        .store
        .ingest_artifact(artifact("normal", b"public bytes", Sensitivity::Normal))
        .unwrap();
    let candidates = [with_artifact(candidate("ok", 1), &normal.digest)];
    let before = fixture
        .store
        .judge_eligibility(&project, ArtifactDestination::Remote, &candidates)
        .unwrap();
    assert_eq!(before.verdicts, [EligibilityVerdict::Ok]);

    // An exact replay of the same intent with a stricter class tightens the
    // stored classification under the classification seqlock.
    fixture
        .store
        .ingest_artifact(artifact("normal", b"public bytes", Sensitivity::Sensitive))
        .unwrap();
    let after = fixture
        .store
        .judge_eligibility(&project, ArtifactDestination::Remote, &candidates)
        .unwrap();
    assert_eq!(after.verdicts, [EligibilityVerdict::ProviderSensitive]);
    // The generation moved, so a cache keyed on it cannot serve the old
    // verdict even if the tip alone would not have told the two states apart.
    assert_ne!(
        after.snapshot.classification_generation, before.snapshot.classification_generation,
        "the old identity cannot authorize the new classification"
    );
    assert!(after.snapshot.classification_generation.is_some());
}

#[test]
fn a_batch_never_mixes_snapshots_while_a_writer_flips_candidates_and_scopes() {
    let fixture = fixture();
    let store = Arc::new(fixture.store);
    let stop = Arc::new(AtomicBool::new(false));
    // Each round inserts a live decision, alternating the project scope it
    // names, then retires it. A reader that mixed snapshots could see one
    // duplicate of the same candidate live and another retracted.
    let writer = {
        let store = Arc::clone(&store);
        let stop = Arc::clone(&stop);
        thread::spawn(move || {
            let mut round = 0u32;
            while !stop.load(Ordering::Relaxed) {
                let object = format!("flip-{round}");
                let scope_id = if round.is_multiple_of(2) {
                    SCOPE_A
                } else {
                    SCOPE_B
                };
                store
                    .commit(intent(&format!("flip-insert-{round}")), |envelope| {
                        envelope.insert_decision(decision(
                            &object,
                            Some(scope_id),
                            Sensitivity::Normal,
                        ))?;
                        envelope.record_admission(admission(&object, EventKind::Other))?;
                        Ok(String::new())
                    })
                    .unwrap();
                store
                    .commit(intent(&format!("flip-retire-{round}")), |envelope| {
                        envelope.retire_decision(&object)?;
                        Ok(String::new())
                    })
                    .unwrap();
                round += 1;
            }
            round
        })
    };
    let project = ProjectScope::new(PROJECT_A).unwrap();
    let mut seen = std::collections::BTreeSet::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        let tip = store.tip().unwrap();
        let round = (tip - 3).max(0) / 2;
        let object = format!("flip-{round}");
        let batch = store
            .judge_eligibility(
                &project,
                ArtifactDestination::Local,
                &[
                    candidate(&object, 1),
                    candidate("ok", 1),
                    candidate(&object, 1),
                    candidate("retired", 1),
                    candidate(&object, 1),
                ],
            )
            .unwrap();
        assert_eq!(batch.verdicts[0], batch.verdicts[2]);
        assert_eq!(batch.verdicts[2], batch.verdicts[4]);
        assert_eq!(batch.verdicts[1], EligibilityVerdict::Ok);
        assert_eq!(batch.verdicts[3], EligibilityVerdict::Retracted);
        assert!(batch.snapshot.tip >= tip);
        assert!(matches!(
            batch.verdicts[0],
            EligibilityVerdict::Ok | EligibilityVerdict::WrongScope | EligibilityVerdict::Retracted
        ));
        seen.insert(format!("{:?}", batch.verdicts[0]));
        if seen.len() == 3 {
            break;
        }
    }
    stop.store(true, Ordering::Relaxed);
    let rounds = writer.join().unwrap();
    assert!(rounds > 0, "the writer made progress while readers judged");
    assert_eq!(
        seen.len(),
        3,
        "readers observed every legal state of a flipping candidate: {seen:?}"
    );
}

#[test]
fn racing_test_holders_never_hold_the_window_together() {
    let fixture = fixture();
    let store = &fixture.store;
    let project = ProjectScope::new(PROJECT_A).unwrap();
    let candidates = [candidate("ok", 1)];
    // Two holders that both passed the parity check would leave the generation
    // even while both guards live, so a snapshot taken then would look reusable.
    // A racing second holder is either refused or waits for the first to drop.
    for _ in 0..200 {
        let barrier = std::sync::Barrier::new(2);
        let hold = || {
            barrier.wait();
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _window = store.hold_classification_change_for_test();
                store
                    .judge_eligibility(&project, ArtifactDestination::Remote, &candidates)
                    .unwrap()
                    .snapshot
                    .classification_generation
            }))
            .ok()
        };
        let outcomes = thread::scope(|scope| {
            let first = scope.spawn(hold);
            let second = scope.spawn(hold);
            [first.join().unwrap(), second.join().unwrap()]
        });
        let held: Vec<_> = outcomes.iter().flatten().collect();
        assert!(!held.is_empty(), "one racing holder opens the window");
        assert!(
            held.iter().all(|generation| generation.is_none()),
            "a snapshot under a held window is never reusable: {outcomes:?}"
        );
    }
    let closed = store
        .judge_eligibility(&project, ArtifactDestination::Remote, &candidates)
        .unwrap();
    assert!(closed.snapshot.classification_generation.is_some());
}

#[test]
fn a_production_opener_waits_for_the_held_test_window() {
    let fixture = fixture();
    let store = &fixture.store;
    let project = ProjectScope::new(PROJECT_A).unwrap();
    let normal = store
        .ingest_artifact(artifact("normal", b"public bytes", Sensitivity::Normal))
        .unwrap();
    let candidates = [with_artifact(candidate("ok", 1), &normal.digest)];
    let window = store.hold_classification_change_for_test();
    let (done, finished) = std::sync::mpsc::channel();
    let (ready, at_writer_lock) = std::sync::mpsc::sync_channel(0);
    thread::scope(|scope| {
        scope.spawn(|| {
            // Tightening the stored classification opens a production window;
            // the hook runs right before the tightening takes the writer lock.
            store
                .ingest_artifact_with_temp_hook_for_test(
                    artifact("normal", b"public bytes", Sensitivity::Sensitive),
                    |_| ready.send(()).unwrap(),
                )
                .unwrap();
            let _ = done.send(());
        });
        at_writer_lock.recv().unwrap();
        assert!(
            finished
                .recv_timeout(std::time::Duration::from_secs(2))
                .is_err(),
            "the tightening waited behind the held window"
        );
        let batch = store
            .judge_eligibility(&project, ArtifactDestination::Remote, &candidates)
            .unwrap();
        assert_eq!(
            batch.snapshot.classification_generation, None,
            "a snapshot under a held window is never reusable"
        );
        drop(window);
        finished.recv().unwrap();
    });
    let after = store
        .judge_eligibility(&project, ArtifactDestination::Remote, &candidates)
        .unwrap();
    assert_eq!(after.verdicts, [EligibilityVerdict::ProviderSensitive]);
    assert!(after.snapshot.classification_generation.is_some());
}

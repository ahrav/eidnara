//! Independently authored expected verdicts for `KernelStore::judge_eligibility`.

#![cfg(feature = "test-support")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use kernel::{
    AdmissionEvent, AdmissionRequest, ArtifactDestination, ArtifactIngestRequest, CommitIntent,
    DecisionPayload, DecisionSpec, Dimension, DomainSpec, EligibilityCandidate, EligibilityVerdict,
    EventKind, KernelError, KernelStore, MAX_ELIGIBILITY_CANDIDATES,
    MAX_ELIGIBILITY_OBJECT_ID_BYTES, ProjectScope, ProviderEgress, RepositoryProvenance, ScopeSpec,
    ScopeTermSpec, Sensitivity, SourceClass, TaintClass,
};
use sha2::{Digest, Sha256};

const DOMAIN: &str = "domain";
const PROJECT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PROJECT_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const SCOPE_A: &str = "project:a";
const SCOPE_B: &str = "project:b";
const SCOPE_NO_PROJECT: &str = "scope:branch-only";

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "kernel-eligibility-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

fn project_term(digest: &str) -> ScopeTermSpec {
    ScopeTermSpec {
        dimension: Dimension::Project.as_str().to_string(),
        operator: "exact".to_string(),
        exact_value: Some(digest.to_string()),
        ..ScopeTermSpec::default()
    }
}

fn branch_term() -> ScopeTermSpec {
    ScopeTermSpec {
        dimension: Dimension::Branch.as_str().to_string(),
        operator: "exact".to_string(),
        exact_value: Some("main".to_string()),
        ..ScopeTermSpec::default()
    }
}

fn scope(scope_id: &str, terms: Vec<ScopeTermSpec>) -> ScopeSpec {
    ScopeSpec {
        scope_id: scope_id.to_string(),
        object_id: scope_id.to_string(),
        source_id: scope_id.to_string(),
        domain_id: DOMAIN.to_string(),
        source_kind: "kernel_route".to_string(),
        source_revision: 1,
        sensitivity: Sensitivity::Normal,
        terms,
    }
}

fn decision(object: &str, scope_id: Option<&str>, sensitivity: Sensitivity) -> DecisionSpec {
    DecisionSpec {
        decision_id: format!("decision-{object}"),
        object_id: object.to_string(),
        domain_id: DOMAIN.to_string(),
        proposition_id: None,
        scope_id: scope_id.map(str::to_string),
        anchor_id: None,
        evidence_id: None,
        decision_kind: "architecture".to_string(),
        payload: DecisionPayload {
            summary: format!("summary {object}"),
            rationale: format!("rationale {object}"),
        },
        source_kind: "repo".to_string(),
        source_id: format!("src/{object}"),
        source_revision: 1,
        sensitivity,
    }
}

fn admission(object: &str, kind: EventKind) -> AdmissionRequest {
    AdmissionRequest {
        candidate_id: None,
        subject_object_id: Some(object.to_string()),
        source_class: Some(SourceClass::ExplicitUser),
        taint_class: Some(TaintClass::UserExplicit),
        event: AdmissionEvent {
            kind,
            trigger_object_id: None,
            approval_object_id: None,
            evidence_id: None,
            reason: "test".to_string(),
        },
    }
}

fn artifact(key: &str, payload: &[u8], sensitivity: Sensitivity) -> ArtifactIngestRequest {
    ArtifactIngestRequest {
        intent: intent(key),
        payload: payload.to_vec(),
        evidence_id: format!("evidence-{key}"),
        object_id: format!("evidence-object-{key}"),
        object_kind: "evidence".to_string(),
        domain_id: DOMAIN.to_string(),
        source_kind: "repository".to_string(),
        source_id: format!("src/{key}"),
        source_revision: 1,
        media_type: "text/plain".to_string(),
        retention_class: "canonical".to_string(),
        retain_until: None,
        asserted_sensitivity: sensitivity,
        provider_egress: ProviderEgress::RemoteAllowed,
        // A `Normal` assertion without provenance is stored as `Sensitive`.
        provenance: Some(RepositoryProvenance {
            repository_id: "repo".to_string(),
            revision: "abc123".to_string(),
        }),
    }
}

fn candidate(object: &str, revision: i64) -> EligibilityCandidate {
    EligibilityCandidate {
        object_id: object.to_string(),
        source_revision: revision,
        artifact_digest: None,
    }
}

fn with_artifact(mut candidate: EligibilityCandidate, digest: &str) -> EligibilityCandidate {
    candidate.artifact_digest = Some(digest.to_string());
    candidate
}

struct Fixture {
    _root: tempfile::TempDir,
    store: KernelStore,
    sensitive_artifact: String,
}

fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    store
        .commit(intent("seed"), |envelope| {
            envelope.insert_domain(DomainSpec {
                domain_id: DOMAIN.to_string(),
                object_id: "domain-object".to_string(),
                name: "fixture".to_string(),
                source_kind: "fixture".to_string(),
                source_id: DOMAIN.to_string(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
            })?;
            envelope.insert_scope(scope(SCOPE_A, vec![project_term(PROJECT_A)]))?;
            envelope.insert_scope(scope(SCOPE_B, vec![project_term(PROJECT_B)]))?;
            envelope.insert_scope(scope(SCOPE_NO_PROJECT, vec![branch_term()]))?;
            for (object, scope_id, sensitivity) in [
                ("ok", Some(SCOPE_A), Sensitivity::Normal),
                ("retired", Some(SCOPE_A), Sensitivity::Normal),
                ("replaced", Some(SCOPE_A), Sensitivity::Normal),
                ("stale", Some(SCOPE_A), Sensitivity::Normal),
                ("other-project", Some(SCOPE_B), Sensitivity::Normal),
                (
                    "branch-only-scope",
                    Some(SCOPE_NO_PROJECT),
                    Sensitivity::Normal,
                ),
                ("unscoped", None, Sensitivity::Normal),
                ("secret", Some(SCOPE_A), Sensitivity::Secret),
                ("sensitive", Some(SCOPE_A), Sensitivity::Sensitive),
                ("unadmitted", Some(SCOPE_A), Sensitivity::Normal),
                ("contradicted", Some(SCOPE_A), Sensitivity::Normal),
                ("with-artifact", Some(SCOPE_A), Sensitivity::Normal),
                ("retired-other-project", Some(SCOPE_B), Sensitivity::Normal),
                ("stale-other-project", Some(SCOPE_B), Sensitivity::Normal),
                ("secret-other-project", Some(SCOPE_B), Sensitivity::Secret),
                ("secret-unadmitted", Some(SCOPE_A), Sensitivity::Secret),
                (
                    "contradicted-with-artifact",
                    Some(SCOPE_A),
                    Sensitivity::Normal,
                ),
            ] {
                envelope.insert_decision(decision(object, scope_id, sensitivity))?;
                if !matches!(object, "unadmitted" | "secret-unadmitted") {
                    let kind = if object.starts_with("contradicted") {
                        EventKind::Contradict
                    } else {
                        EventKind::Other
                    };
                    envelope.record_admission(admission(object, kind))?;
                }
            }
            Ok(String::new())
        })
        .unwrap();
    store
        .commit(intent("mutate"), |envelope| {
            envelope.retire_decision("retired")?;
            envelope.retire_decision("retired-other-project")?;
            let mut replacement = decision("replacement", Some(SCOPE_A), Sensitivity::Normal);
            replacement.source_id = "src/replaced".to_string();
            replacement.source_revision = 2;
            envelope.supersede_decision("replaced", replacement)?;
            envelope.record_admission(admission("replacement", EventKind::Other))?;
            Ok(String::new())
        })
        .unwrap();
    let sensitive_artifact = store
        .ingest_artifact(artifact(
            "sensitive",
            b"sensitive bytes",
            Sensitivity::Sensitive,
        ))
        .unwrap()
        .digest;
    Fixture {
        _root: root,
        store,
        sensitive_artifact,
    }
}

fn expected(
    fixture: &Fixture,
    destination: ArtifactDestination,
) -> Vec<(EligibilityCandidate, EligibilityVerdict)> {
    use EligibilityVerdict::*;
    let remote = destination == ArtifactDestination::Remote;
    let artifact = fixture.sensitive_artifact.as_str();
    vec![
        (candidate("ok", 1), Ok),
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

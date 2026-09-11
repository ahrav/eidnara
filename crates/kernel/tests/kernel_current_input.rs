//! The current-input guard: bounded acquisition of the kernel writer, revalidation of one descriptor under it, and exclusion of every canonical mutation while it lives.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use kernel::source_identity::Occurrence;
use kernel::{
    AdmissionEvent, AdmissionRequest, ArtifactDeletionIdentity, ArtifactDeletionKind,
    ArtifactDeletionRequest, ArtifactDestination, ArtifactIngestRequest, CommitIntent,
    CurrentInputExpectation, Dimension, DomainSpec, EligibilityBinding, EligibilityVerdict,
    EventKind, KernelError, KernelStore, ProjectScope, ProviderEgress, RepositoryProvenance,
    ScopeSpec, ScopeTermSpec, Sensitivity, SourceClass, SourceDescriptorRequest, StaleInput,
    TaintClass,
};
use sha2::{Digest, Sha256};

const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SCOPE: &str = "project:a";

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "kernel-current-input-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    store: Arc<KernelStore>,
}

impl Fixture {
    fn open() -> Self {
        let root = tempfile::tempdir().unwrap();
        let store = KernelStore::open(root.path()).unwrap();
        store
            .commit(intent("seed"), |envelope| {
                envelope.insert_domain(DomainSpec {
                    domain_id: "domain".to_string(),
                    object_id: "domain-object".to_string(),
                    name: "Name".to_string(),
                    source_kind: "fixture".to_string(),
                    source_id: "domain".to_string(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                })?;
                envelope.insert_scope(ScopeSpec {
                    scope_id: SCOPE.to_string(),
                    object_id: SCOPE.to_string(),
                    source_id: SCOPE.to_string(),
                    domain_id: "domain".to_string(),
                    source_kind: "kernel_route".to_string(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                    terms: vec![ScopeTermSpec {
                        dimension: Dimension::Project.as_str().to_string(),
                        operator: "exact".to_string(),
                        exact_value: Some(PROJECT.to_string()),
                        ..ScopeTermSpec::default()
                    }],
                })?;
                Ok(String::new())
            })
            .unwrap();
        Self {
            _root: root,
            store: Arc::new(store),
        }
    }

    /// Publishes one message descriptor and returns the expectation a consumer would hold for it.
    fn publish(
        &self,
        key: &str,
        message_id: &str,
        revision: &str,
        text: &str,
        scoped: bool,
        admitted: bool,
    ) -> CurrentInputExpectation {
        let handle = self
            .store
            .ingest_exact_artifact(ArtifactIngestRequest {
                intent: intent(&format!("artifact-{key}")),
                payload: text.as_bytes().to_vec(),
                evidence_id: format!("evidence-{key}"),
                object_id: format!("evidence-object-{key}"),
                object_kind: "evidence".to_string(),
                domain_id: "domain".to_string(),
                source_kind: "tool_output".to_string(),
                source_id: format!("native/{key}"),
                source_revision: 1,
                media_type: "text/plain".to_string(),
                retention_class: "canonical".to_string(),
                retain_until: None,
                asserted_sensitivity: Sensitivity::Normal,
                provider_egress: ProviderEgress::RemoteAllowed,
                provenance: Some(RepositoryProvenance {
                    repository_id: "repo".to_string(),
                    revision: "abc123".to_string(),
                }),
            })
            .unwrap();
        let mut expectation = None;
        self.store
            .commit(intent(&format!("publish-{key}")), |envelope| {
                let identity = [
                    ("project_id", "proj-a"),
                    ("harness", "opencode"),
                    ("session_id", "sess-01"),
                    ("message_id", message_id),
                    ("block_index", "0"),
                ];
                let outcome = envelope
                    .publish_source_descriptor(&SourceDescriptorRequest {
                        source_policy: kernel::SourceDescriptorPolicy::Native,
                        occurrence: Occurrence {
                            class: "messages",
                            identity: &identity,
                            revision,
                            representation: "text",
                            span: None,
                        },
                        domain_id: "domain",
                        scope_id: scoped.then_some(SCOPE),
                        evidence_id: &handle.evidence_id,
                        artifact_digest: &handle.digest,
                        buffer: text,
                        sensitivity: Sensitivity::Normal,
                        observed_at: 1,
                    })
                    .unwrap();
                if admitted {
                    envelope.record_admission(AdmissionRequest {
                        candidate_id: None,
                        subject_object_id: Some(outcome.object_id.clone()),
                        source_class: Some(SourceClass::ExplicitUser),
                        taint_class: Some(TaintClass::UserExplicit),
                        event: AdmissionEvent {
                            kind: EventKind::Other,
                            trigger_object_id: None,
                            approval_object_id: None,
                            evidence_id: None,
                            reason: "test".to_string(),
                        },
                    })?;
                }
                expectation = Some(CurrentInputExpectation {
                    object_id: outcome.object_id,
                    source_revision: revision.parse().unwrap(),
                    occurrence_id: outcome.occurrence_id,
                    payload_id: outcome.payload_id,
                    artifact_digest: handle.digest.clone(),
                });
                Ok(String::new())
            })
            .unwrap();
        expectation.unwrap()
    }
}

fn binding(project: &ProjectScope) -> EligibilityBinding<'_> {
    EligibilityBinding {
        project,
        destination: ArtifactDestination::Remote,
    }
}

fn soon() -> Instant {
    Instant::now() + Duration::from_secs(5)
}

#[test]
fn the_guard_excludes_canonical_mutations_until_it_drops_and_times_out_behind_a_held_writer() {
    let fixture = Fixture::open();
    let project = ProjectScope::new(PROJECT).unwrap();
    let expectation = fixture.publish("a", "msg-a", "1", "first message", true, true);
    let tip = fixture.store.tip().unwrap();

    let other = fixture.publish("b", "msg-b", "1", "second message", true, true);
    let guard = fixture
        .store
        .guard_current_input(&expectation, binding(&project), soon())
        .unwrap()
        .expect("the descriptor is current and eligible");
    assert_eq!(guard.tip(), fixture.store.tip().unwrap());
    assert!(guard.tip() > tip);
    // The guard holds the writer mutex and nothing else: no kernel transaction is open, and the database file itself is unlocked.
    assert!(!guard.holds_kernel_transaction_for_test());
    let probe = rusqlite::Connection::open_with_flags(
        fixture._root.path().join("kernel.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
    )
    .unwrap();
    probe.busy_timeout(Duration::ZERO).unwrap();
    probe.execute_batch("BEGIN IMMEDIATE; ROLLBACK;").unwrap();
    drop(probe);

    // A retirement started while the guard lives lands only after the guard drops.
    let done = Arc::new(AtomicBool::new(false));
    let started = Arc::new(AtomicBool::new(false));
    let writer = {
        let store = Arc::clone(&fixture.store);
        let done = Arc::clone(&done);
        let started = Arc::clone(&started);
        let object = other.object_id.clone();
        thread::spawn(move || {
            started.store(true, Ordering::SeqCst);
            let receipt = store.commit(intent("retire-b"), |envelope| {
                envelope.retire_observation(&object)?;
                Ok(String::new())
            });
            done.store(true, Ordering::SeqCst);
            receipt
        })
    };
    while !started.load(Ordering::SeqCst) {
        thread::yield_now();
    }
    thread::sleep(Duration::from_millis(200));
    assert!(
        !done.load(Ordering::SeqCst),
        "the mutation waits behind the guard"
    );
    // A second guard cannot be taken while the first holds the writer, and it reports the deadline rather than waiting forever.
    let refused = fixture.store.guard_current_input(
        &expectation,
        binding(&project),
        Instant::now() + Duration::from_millis(100),
    );
    assert!(matches!(refused, Err(KernelError::Deadline)), "{refused:?}");
    drop(guard);
    writer
        .join()
        .unwrap()
        .expect("the retirement commits once the guard drops");
    assert!(done.load(Ordering::SeqCst));
    assert_eq!(
        fixture
            .store
            .guard_current_input(&other, binding(&project), soon())
            .unwrap()
            .err()
            .map(|stale| stale.reason().clone()),
        Some(StaleInput::Retracted)
    );
    // Reading under a fresh guard succeeds again; the earlier deadline granted nothing.
    fixture
        .store
        .guard_current_input(&expectation, binding(&project), soon())
        .unwrap()
        .expect("current again");
}

#[test]
fn every_stale_dimension_is_named_and_retains_the_writer_until_dropped() {
    let fixture = Fixture::open();
    let project = ProjectScope::new(PROJECT).unwrap();
    let superseded = fixture.publish("a", "msg-a", "1", "first message", true, true);
    fixture.publish("a2", "msg-a", "2", "first message, revised", true, true);
    let retired = fixture.publish("b", "msg-b", "1", "second message", true, true);
    fixture
        .store
        .commit(intent("retire-b"), |envelope| {
            envelope.retire_observation(&retired.object_id)?;
            Ok(String::new())
        })
        .unwrap();
    let unscoped = fixture.publish("c", "msg-c", "1", "third message", false, true);
    let unadmitted = fixture.publish("d", "msg-d", "1", "fourth message", true, false);
    let current = fixture.publish("e", "msg-e", "1", "fifth message", true, true);
    let current_incarnation = {
        let guard = fixture
            .store
            .guard_current_input(&current, binding(&project), soon())
            .unwrap()
            .expect("the descriptor is current");
        guard.database_incarnation_id().to_string()
    };
    let wrong_revision = CurrentInputExpectation {
        source_revision: 7,
        ..current.clone()
    };
    let wrong_payload = CurrentInputExpectation {
        payload_id: format!("{:x}", Sha256::digest(b"other bytes")),
        ..current.clone()
    };
    let wrong_occurrence = CurrentInputExpectation {
        occurrence_id: format!("{:x}", Sha256::digest(b"other occurrence")),
        ..current.clone()
    };
    let wrong_digest = CurrentInputExpectation {
        artifact_digest: format!("{:x}", Sha256::digest(b"other artifact")),
        ..current.clone()
    };
    let missing = CurrentInputExpectation {
        object_id: "srcdesc:never:1".to_string(),
        ..current.clone()
    };
    for (label, expectation, stale) in [
        ("superseded", &superseded, StaleInput::Superseded),
        ("retired", &retired, StaleInput::Retracted),
        ("missing", &missing, StaleInput::Retracted),
        (
            "unscoped",
            &unscoped,
            StaleInput::Ineligible(EligibilityVerdict::WrongScope),
        ),
        (
            "unadmitted",
            &unadmitted,
            StaleInput::Ineligible(EligibilityVerdict::Hidden),
        ),
        (
            "revision",
            &wrong_revision,
            StaleInput::RevisionChanged { current: 1 },
        ),
        ("payload", &wrong_payload, StaleInput::InputChanged),
        ("occurrence", &wrong_occurrence, StaleInput::InputChanged),
        // An artifact the kernel has never admitted is judged ineligible before the detail is compared.
        (
            "digest",
            &wrong_digest,
            StaleInput::Ineligible(EligibilityVerdict::ProviderSensitive),
        ),
    ] {
        let judged = fixture
            .store
            .guard_current_input(expectation, binding(&project), soon())
            .unwrap();
        let stale_input = judged.expect_err(label);
        assert_eq!(stale_input.reason(), &stale, "{label}");
        assert_eq!(
            stale_input.database_incarnation_id(),
            current_incarnation,
            "{label}"
        );
        if label == "superseded" {
            let blocked = fixture.store.commit_before(
                Instant::now() + Duration::from_millis(50),
                intent("while-stale-held"),
                |_| Ok(String::new()),
            );
            assert!(matches!(blocked, Err(KernelError::Deadline)), "{blocked:?}");
        }
        drop(stale_input);
        // Once the consumer has acted on the stale verdict, dropping it releases the writer.
        fixture
            .store
            .commit_before(soon(), intent(&format!("after-{label}")), |_| {
                Ok(String::new())
            })
            .unwrap();
    }
    fixture
        .store
        .guard_current_input(&current, binding(&project), soon())
        .unwrap()
        .expect("the current descriptor is admitted");
}

#[test]
fn a_descriptor_whose_cited_evidence_was_logically_deleted_is_retracted() {
    let fixture = Fixture::open();
    let project = ProjectScope::new(PROJECT).unwrap();
    let expectation = fixture.publish("a", "msg-a", "1", "first message", true, true);
    fixture
        .store
        .delete_artifact(ArtifactDeletionRequest {
            intent: intent("delete-evidence"),
            identity: ArtifactDeletionIdentity::Digest(expectation.artifact_digest.clone()),
            kind: ArtifactDeletionKind::Delete,
            operator_id: None,
            target_locator: None,
            reason: None,
            deleted_at: 1,
        })
        .unwrap();

    let local = EligibilityBinding {
        project: &project,
        destination: ArtifactDestination::Local,
    };
    assert_eq!(
        fixture
            .store
            .guard_current_input(&expectation, local, soon())
            .unwrap()
            .err()
            .map(|stale| stale.reason().clone()),
        Some(StaleInput::Retracted),
        "the guard granted a descriptor whose evidence was canonically deleted",
    );
}

#[test]
fn a_descriptor_whose_evidence_digest_disagrees_is_corrupt() {
    let fixture = Fixture::open();
    let project = ProjectScope::new(PROJECT).unwrap();
    let expectation = fixture.publish("a", "msg-a", "1", "first message", true, true);
    let other_digest = format!("{:x}", Sha256::digest(b"other artifact"));
    rusqlite::Connection::open(fixture._root.path().join("kernel.sqlite"))
        .unwrap()
        .execute(
            "UPDATE evidence_meta SET artifact_digest=?1 WHERE evidence_id='evidence-a'",
            [other_digest],
        )
        .unwrap();
    let local = EligibilityBinding {
        project: &project,
        destination: ArtifactDestination::Local,
    };

    assert!(matches!(
        fixture
            .store
            .guard_current_input(&expectation, local, soon()),
        Err(KernelError::CorruptCanonicalRow),
    ));
}

#[test]
fn a_corrupt_stored_descriptor_is_refused_rather_than_granted() {
    let fixture = Fixture::open();
    let project = ProjectScope::new(PROJECT).unwrap();
    let current = fixture.publish("a", "msg-a", "1", "first message", true, true);
    let db_path = fixture._root.path().join("kernel.sqlite");
    let pristine: Vec<u8> =
        rusqlite::Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap()
            .query_row(
                "SELECT observation_payload FROM observations WHERE object_id=?1",
                [&current.object_id],
                |row| row.get(0),
            )
            .unwrap();

    // Each tamper corrupts one detail field the guard does not compare against the expectation.
    for (label, field, value) in [
        ("lineage", "lineage_id", serde_json::json!("tampered")),
        ("revision", "revision", serde_json::json!("7")),
        ("tuple", "occurrence_tuple", serde_json::json!([1, 2, 3])),
        ("span", "span", serde_json::json!([0, 3])),
        ("evidence", "evidence_id", serde_json::json!("evidence-x")),
        (
            "representation",
            "representation",
            serde_json::json!("bytes"),
        ),
        (
            "identity",
            "identity",
            serde_json::json!([
                ["project_id", "proj-x"],
                ["harness", "opencode"],
                ["session_id", "sess-01"],
                ["message_id", "msg-a"],
                ["block_index", "0"]
            ]),
        ),
        (
            "policy",
            "source_policy",
            serde_json::json!({"kind": "git", "version": "1"}),
        ),
    ] {
        let mut stored: serde_json::Value = serde_json::from_slice(&pristine).unwrap();
        let mut detail: serde_json::Value =
            serde_json::from_str(stored["detail"].as_str().unwrap()).unwrap();
        detail[field] = value;
        stored["detail"] = serde_json::Value::String(serde_json::to_string(&detail).unwrap());
        let db = rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .unwrap();
        db.execute(
            "UPDATE observations SET observation_payload=?1 WHERE object_id=?2",
            rusqlite::params![serde_json::to_vec(&stored).unwrap(), &current.object_id],
        )
        .unwrap();
        drop(db);
        let refused = fixture
            .store
            .guard_current_input(&current, binding(&project), soon());
        assert!(
            matches!(refused, Err(KernelError::CorruptCanonicalRow)),
            "{label}: {refused:?}"
        );
        // The refusal holds nothing: a bounded commit goes straight through.
        fixture
            .store
            .commit_before(soon(), intent(&format!("after-{label}")), |_| {
                Ok(String::new())
            })
            .unwrap();
    }

    let db = rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
    )
    .unwrap();
    db.execute(
        "UPDATE observations SET observation_payload=?1 WHERE object_id=?2",
        rusqlite::params![&pristine, &current.object_id],
    )
    .unwrap();
    drop(db);
    fixture
        .store
        .guard_current_input(&current, binding(&project), soon())
        .unwrap()
        .expect("the restored descriptor is current");
}

#[test]
fn registry_and_observation_metadata_must_agree_with_the_descriptor() {
    let fixture = Fixture::open();
    let project = ProjectScope::new(PROJECT).unwrap();
    let current = fixture.publish("a", "msg-a", "1", "first message", true, true);
    fixture
        .store
        .commit(intent("later-metadata-test-tip"), |_| Ok(String::new()))
        .unwrap();
    let db_path = fixture._root.path().join("kernel.sqlite");
    let original_source_id: String = rusqlite::Connection::open(&db_path)
        .unwrap()
        .query_row(
            "SELECT source_id FROM object_registry WHERE object_id=?1",
            [&current.object_id],
            |row| row.get(0),
        )
        .unwrap();
    rusqlite::Connection::open(&db_path)
        .unwrap()
        .execute_batch("DROP TRIGGER object_registry_append_only_update;")
        .unwrap();

    for (label, corrupt, restore) in [
        (
            "source id",
            "UPDATE object_registry SET source_id='tampered' WHERE object_id=?1",
            "",
        ),
        (
            "source kind",
            "UPDATE object_registry SET source_kind='canonical_claims' WHERE object_id=?1",
            "UPDATE object_registry SET source_kind='messages' WHERE object_id=?1",
        ),
        (
            "object kind",
            "UPDATE object_registry SET object_kind='decision' WHERE object_id=?1",
            "UPDATE object_registry SET object_kind='observation' WHERE object_id=?1",
        ),
        (
            "observation kind",
            "UPDATE observations SET observation_kind='other' WHERE object_id=?1",
            "UPDATE observations SET observation_kind='source_descriptor' WHERE object_id=?1",
        ),
        (
            "observation liveness",
            "UPDATE observations SET invalidated_commit_seq=(SELECT MAX(commit_seq) FROM commit_log) WHERE object_id=?1",
            "UPDATE observations SET invalidated_commit_seq=NULL WHERE object_id=?1",
        ),
        (
            "sensitivity",
            "UPDATE observations SET sensitivity_class='secret' WHERE object_id=?1",
            "UPDATE observations SET sensitivity_class='normal' WHERE object_id=?1",
        ),
    ] {
        let db = rusqlite::Connection::open(&db_path).unwrap();
        db.execute(corrupt, [&current.object_id]).unwrap();
        drop(db);
        let refused = fixture
            .store
            .guard_current_input(&current, binding(&project), soon());
        assert!(
            matches!(refused, Err(KernelError::CorruptCanonicalRow)),
            "{label}: {refused:?}"
        );
        let db = rusqlite::Connection::open(&db_path).unwrap();
        if label == "source id" {
            db.execute(
                "UPDATE object_registry SET source_id=?2 WHERE object_id=?1",
                rusqlite::params![&current.object_id, &original_source_id],
            )
            .unwrap();
        } else {
            db.execute(restore, [&current.object_id]).unwrap();
        }
    }

    fixture
        .store
        .guard_current_input(&current, binding(&project), soon())
        .unwrap()
        .expect("the restored metadata is current");
}

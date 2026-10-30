#![cfg(feature = "test-support")]

use std::{cell::Cell, fs};

use kernel::{
    AdmissionEvent, AdmissionRequest, ArtifactDeletionIdentity, ArtifactDeletionKind,
    ArtifactDeletionRequest, ArtifactIngestRequest, CommitIntent, DecisionEventPayload,
    DecisionEventSpec, DecisionPayload, DecisionSpec, DependentObservationQuery, DomainSpec,
    EventKind, KernelError, KernelStore, ObservationDependencySpec, ObservationPayload,
    ObservationSpec, ProviderEgress, RepositoryProvenance, Sensitivity, SourceClass, TaintClass,
};
use rusqlite::{Connection, OpenFlags};

const SECRET: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678";

fn intent(key: &str, digest: char) -> CommitIntent {
    CommitIntent {
        producer: "kernel-slice-test".to_string(),
        operation_key: key.to_string(),
        request_digest: digest.to_string().repeat(64),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

fn domain() -> DomainSpec {
    DomainSpec {
        domain_id: "domain".to_string(),
        object_id: "domain-object".to_string(),
        name: "fixture".to_string(),
        source_kind: "fixture".to_string(),
        source_id: "domain".to_string(),
        source_revision: 1,
        sensitivity: Sensitivity::Normal,
    }
}

fn decision(index: i64) -> DecisionSpec {
    DecisionSpec {
        decision_id: format!("decision-{index}"),
        object_id: format!("decision-object-{index}"),
        domain_id: "domain".to_string(),
        proposition_id: None,
        scope_id: None,
        anchor_id: None,
        evidence_id: None,
        decision_kind: "architecture".to_string(),
        payload: DecisionPayload {
            summary: format!("decision {index}"),
            rationale: format!("because {index}"),
        },
        source_kind: "fixture".to_string(),
        source_id: "decision".to_string(),
        source_revision: index,
        sensitivity: Sensitivity::Normal,
    }
}

fn observation(index: i64, dependency_object_id: &str) -> ObservationSpec {
    ObservationSpec {
        observation_id: format!("observation-{index}"),
        object_id: format!("observation-object-{index}"),
        domain_id: "domain".to_string(),
        proposition_id: None,
        scope_id: None,
        anchor_id: None,
        evidence_id: None,
        observation_kind: "implementation".to_string(),
        payload: ObservationPayload {
            summary: format!("observed {index}"),
            classification: "implemented".to_string(),
            detail: None,
        },
        observed_at: index,
        dependencies: vec![ObservationDependencySpec {
            dependency_object_id: dependency_object_id.to_string(),
            dependency_kind: "implements".to_string(),
            dependency_payload: None,
        }],
        source_kind: "fixture".to_string(),
        source_id: "observation".to_string(),
        source_revision: index,
        sensitivity: Sensitivity::Normal,
    }
}

/// The query for fixture observations, which are written under source kind
/// `fixture` in domain `domain` with no scope.
fn fixture_dependents<'a>(
    dependency_object_id: &'a str,
    dependency_kind: &'a str,
    observation_kind: &'a str,
) -> DependentObservationQuery<'a> {
    DependentObservationQuery {
        dependency_object_id,
        dependency_kind,
        observation_kind,
        source_kind: "fixture",
        domain_id: "domain",
        scope_id: None,
    }
}

fn inspect_i64(root: &std::path::Path, sql: &str) -> i64 {
    let connection =
        Connection::open_with_flags(root.join("kernel.sqlite"), OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    connection.query_row(sql, [], |row| row.get(0)).unwrap()
}

fn family_bytes(root: &std::path::Path) -> Vec<u8> {
    let base = root.join("kernel.sqlite");
    [
        base.clone(),
        std::path::PathBuf::from(format!("{}-wal", base.display())),
    ]
    .into_iter()
    .filter_map(|path| fs::read(path).ok())
    .flatten()
    .collect()
}

fn seed_domain(store: &KernelStore) {
    store
        .commit(intent("domain", '0'), |envelope| {
            envelope.insert_domain(domain())?;
            Ok(String::new())
        })
        .unwrap();
}

#[test]
fn inserts_slice_rows_atomically_and_replay_is_effect_free() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    seed_domain(&store);

    let receipt = store
        .commit(intent("insert", '1'), |envelope| {
            let decision = envelope.insert_decision(decision(1))?;
            let observation = envelope.insert_observation(observation(1, "decision-object-1"))?;
            Ok(format!(
                "[{},{}]",
                decision.result_json(),
                observation.result_json()
            ))
        })
        .unwrap();
    assert_eq!(receipt.commit_seq, 2);
    assert_eq!(
        inspect_i64(directory.path(), "SELECT COUNT(*) FROM decisions"),
        1
    );
    assert_eq!(
        inspect_i64(directory.path(), "SELECT COUNT(*) FROM observations"),
        1
    );
    assert_eq!(
        inspect_i64(
            directory.path(),
            "SELECT COUNT(*) FROM observation_dependencies"
        ),
        1
    );
    assert_eq!(
        inspect_i64(
            directory.path(),
            "SELECT COUNT(*) FROM change_event WHERE commit_seq=2"
        ),
        2
    );
    assert_eq!(
        inspect_i64(
            directory.path(),
            "SELECT COUNT(*) FROM outbox WHERE commit_seq=2"
        ),
        2
    );

    let called = Cell::new(false);
    let replay = store
        .commit(intent("insert", '1'), |_| {
            called.set(true);
            Ok(String::new())
        })
        .unwrap();
    assert!(replay.replayed);
    assert!(!called.get());
    assert_eq!(replay.result, receipt.result);
    assert_eq!(
        inspect_i64(directory.path(), "SELECT COUNT(*) FROM decisions"),
        1
    );
}

#[test]
fn slice_payloads_redact_before_storage_and_missing_parents_are_typed() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    seed_domain(&store);
    let mut secret = decision(1);
    secret.payload.summary = format!("summary {SECRET}");
    store
        .commit(intent("secret", '2'), |envelope| {
            Ok(envelope.insert_decision(secret)?.result_json())
        })
        .unwrap();
    let mut secret_observation = observation(1, "decision-object-1");
    secret_observation.dependencies[0].dependency_payload = Some(format!("context {SECRET}"));
    store
        .commit(intent("secret-dependency", 'a'), |envelope| {
            Ok(envelope
                .insert_observation(secret_observation)?
                .result_json())
        })
        .unwrap();

    let connection = Connection::open(directory.path().join("kernel.sqlite")).unwrap();
    let payload: Vec<u8> = connection
        .query_row("SELECT decision_payload FROM decisions", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(
        !payload
            .windows(SECRET.len())
            .any(|window| window == SECRET.as_bytes())
    );
    assert!(String::from_utf8(payload).unwrap().contains("REDACTED"));
    assert!(
        !family_bytes(directory.path())
            .windows(SECRET.len())
            .any(|window| window == SECRET.as_bytes())
    );
    let field: String = connection
        .query_row(
            "SELECT field_name FROM durable_text_redactions
             WHERE owner_kind='decisions' AND owner_id='decision-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(field, "decision_payload.summary");
    let dependency_field: String = connection
        .query_row(
            "SELECT field_name FROM durable_text_redactions
             WHERE owner_kind='observations' AND owner_id='observation-1'
               AND field_name='dependencies.0.dependency_payload'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(dependency_field, "dependencies.0.dependency_payload");

    let missing_domain = store
        .commit(intent("missing-domain", '3'), |envelope| {
            let mut spec = decision(2);
            spec.domain_id = "missing".to_string();
            envelope.insert_decision(spec)?;
            Ok(String::new())
        })
        .unwrap_err();
    assert_eq!(missing_domain, KernelError::NotFound);

    let missing_scope = store
        .commit(intent("missing-scope", '4'), |envelope| {
            let mut spec = decision(2);
            spec.scope_id = Some("missing".to_string());
            envelope.insert_decision(spec)?;
            Ok(String::new())
        })
        .unwrap_err();
    assert_eq!(missing_scope, KernelError::NotFound);

    let missing_dependency = store
        .commit(intent("missing-dependency", '5'), |envelope| {
            envelope.insert_observation(observation(1, "missing"))?;
            Ok(String::new())
        })
        .unwrap_err();
    assert_eq!(missing_dependency, KernelError::NotFound);
    let wrong_dependency_kind = store
        .commit(intent("non-decision-dependency", 'b'), |envelope| {
            envelope.insert_observation(observation(1, "domain-object"))?;
            Ok(String::new())
        })
        .unwrap_err();
    assert_eq!(wrong_dependency_kind, KernelError::NotFound);

    let duplicate = store
        .commit(intent("duplicate-source", '6'), |envelope| {
            envelope.insert_decision(decision(1))?;
            Ok(String::new())
        })
        .unwrap_err();
    assert_eq!(duplicate, KernelError::Conflict);
}

#[test]
fn events_allocate_per_decision_ordinals_replay_and_reject_dead_decisions() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    seed_domain(&store);
    store
        .commit(intent("decisions", '5'), |envelope| {
            envelope.insert_decision(decision(1))?;
            envelope.insert_decision(decision(2))?;
            Ok(String::new())
        })
        .unwrap();

    let append = |key: &str, digest: char, decision_id: &str, summary: &str| {
        store
            .commit(intent(key, digest), |envelope| {
                Ok(envelope
                    .append_decision_event(
                        decision_id,
                        DecisionEventSpec {
                            event_kind: "status".to_string(),
                            payload: DecisionEventPayload {
                                summary: summary.to_string(),
                            },
                            evidence_id: None,
                            recorded_at: 10,
                        },
                    )?
                    .result_json())
            })
            .unwrap()
    };
    assert!(
        append("event-1", '6', "decision-1", "one")
            .result
            .contains("\"event_ordinal\":1")
    );
    assert!(
        append("event-2", '7', "decision-1", "two")
            .result
            .contains("\"event_ordinal\":2")
    );
    assert!(
        append("event-other", '8', "decision-2", "one")
            .result
            .contains("\"event_ordinal\":1")
    );
    let replay = append("event-1", '6', "decision-1", "ignored");
    assert!(replay.replayed);
    assert!(replay.result.contains("\"event_ordinal\":1"));
    assert_eq!(
        inspect_i64(directory.path(), "SELECT COUNT(*) FROM decision_events"),
        3
    );

    store
        .commit(intent("retire", '9'), |envelope| {
            Ok(envelope.retire_decision("decision-object-1")?.result_json())
        })
        .unwrap();
    let error = store
        .commit(intent("event-dead", 'a'), |envelope| {
            envelope.append_decision_event(
                "decision-1",
                DecisionEventSpec {
                    event_kind: "status".to_string(),
                    payload: DecisionEventPayload {
                        summary: "late".to_string(),
                    },
                    evidence_id: None,
                    recorded_at: 11,
                },
            )?;
            Ok(String::new())
        })
        .unwrap_err();
    assert_eq!(error, KernelError::NotFound);

    drop(store);
    let store = KernelStore::open(directory.path()).unwrap();
    let after_restart = store
        .commit(intent("event-after-restart", 'b'), |envelope| {
            Ok(envelope
                .append_decision_event(
                    "decision-2",
                    DecisionEventSpec {
                        event_kind: "status".to_string(),
                        payload: DecisionEventPayload {
                            summary: "after restart".to_string(),
                        },
                        evidence_id: None,
                        recorded_at: 12,
                    },
                )?
                .result_json())
        })
        .unwrap();
    assert!(after_restart.result.contains("\"event_ordinal\":2"));
}

#[test]
fn decision_event_preserves_valid_evidence_identifier() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    seed_domain(&store);
    let evidence_id = "evidence-id".to_string();
    let handle = store
        .ingest_artifact(ArtifactIngestRequest {
            intent: intent("evidence", '1'),
            payload: b"fixture evidence".to_vec(),
            evidence_id: evidence_id.clone(),
            object_id: "evidence-object".to_string(),
            object_kind: "evidence".to_string(),
            domain_id: "domain".to_string(),
            source_kind: "fixture".to_string(),
            source_id: "evidence".to_string(),
            source_revision: 1,
            media_type: "text/plain".to_string(),
            retention_class: "canonical".to_string(),
            retain_until: None,
            asserted_sensitivity: Sensitivity::Normal,
            provider_egress: ProviderEgress::RemoteAllowed,
            provenance: Some(RepositoryProvenance {
                repository_id: "fixture".to_string(),
                revision: "abc123".to_string(),
            }),
        })
        .unwrap();
    store
        .commit(intent("decision-with-evidence-event", '2'), |envelope| {
            envelope.insert_decision(decision(1))?;
            envelope.append_decision_event(
                "decision-1",
                DecisionEventSpec {
                    event_kind: "status".to_string(),
                    payload: DecisionEventPayload {
                        summary: "accepted".to_string(),
                    },
                    evidence_id: Some(evidence_id),
                    recorded_at: 10,
                },
            )?;
            Ok(String::new())
        })
        .unwrap();

    let connection = Connection::open(directory.path().join("kernel.sqlite")).unwrap();
    let stored_evidence_id: String = connection
        .query_row(
            "SELECT evidence_id FROM decision_events
             WHERE decision_id='decision-1' AND event_ordinal=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored_evidence_id, handle.evidence_id);
    let tip = store.tip().unwrap();
    let retirement = store.commit(intent("retire-event-evidence", '3'), |envelope| {
        envelope.retire_evidence("evidence-object")?;
        Ok(String::new())
    });
    assert_eq!(retirement.unwrap_err(), KernelError::Conflict);
    assert_eq!(store.tip().unwrap(), tip);
    assert_eq!(store.read_artifact(&handle).unwrap(), b"fixture evidence");
    store
        .commit(intent("retire-event-owner", '4'), |envelope| {
            envelope.retire_decision("decision-object-1")?;
            envelope.retire_evidence("evidence-object")?;
            Ok(String::new())
        })
        .unwrap();
    assert_eq!(
        store.read_artifact(&handle).unwrap_err().kind(),
        kernel::ArtifactErrorKind::ReferenceUnavailable
    );
    assert_eq!(
        inspect_i64(
            directory.path(),
            "SELECT COUNT(*) FROM durable_text_redactions
             WHERE owner_kind='decision_events' AND owner_id='decision-1:1'
               AND field_name='evidence_id'"
        ),
        0
    );
}

#[test]
fn decisions_for_objects_as_of_returns_only_requested_live_rows() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    seed_domain(&store);
    let inserted = store
        .commit(intent("decisions", '1'), |envelope| {
            envelope.insert_decision(decision(1))?;
            envelope.insert_decision(decision(2))?;
            envelope.insert_decision(decision(3))?;
            Ok(String::new())
        })
        .unwrap();
    let retired = store
        .commit(intent("retire", '2'), |envelope| {
            Ok(envelope.retire_decision("decision-object-2")?.result_json())
        })
        .unwrap();
    let tip = store.tip().unwrap();
    assert_eq!(tip, retired.commit_seq);

    let ids = |rows: Vec<kernel::DecisionRow>| -> Vec<String> {
        let mut ids: Vec<String> = rows.into_iter().map(|row| row.object_id).collect();
        ids.sort();
        ids
    };
    let requested = [
        "decision-object-1".to_string(),
        "decision-object-2".to_string(),
        "decision-object-9".to_string(),
    ];

    assert_eq!(
        ids(store.decisions_for_objects_as_of(&requested, tip).unwrap()),
        ["decision-object-1"]
    );
    assert_eq!(
        ids(store
            .decisions_for_objects_as_of(&requested, inserted.commit_seq)
            .unwrap()),
        ["decision-object-1", "decision-object-2"]
    );
    let full = store.slice_as_of(tip).unwrap();
    assert_eq!(
        store
            .decisions_for_objects_as_of(&["decision-object-3".to_string()], tip)
            .unwrap(),
        full.decisions
            .into_iter()
            .filter(|row| row.object_id == "decision-object-3")
            .collect::<Vec<_>>()
    );

    // Payload sizes follow the same selection and report stored payload byte
    // lengths.
    let sizes = store.decision_payload_sizes_as_of(&requested, tip).unwrap();
    assert_eq!(sizes.len(), 1);
    let (object_id, size) = &sizes[0];
    assert_eq!(object_id, "decision-object-1");
    let live = store
        .decisions_for_objects_as_of(&requested, tip)
        .unwrap()
        .remove(0);
    let stored = serde_json::to_vec(&live.payload).unwrap();
    assert_eq!(*size, stored.len() as u64);

    assert!(
        store
            .decisions_for_objects_as_of(&[], tip)
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .decision_payload_sizes_as_of(&[], tip)
            .unwrap()
            .is_empty()
    );
    // An empty batch still validates its snapshot token.
    assert_eq!(
        store.decisions_for_objects_as_of(&[], tip + 1).unwrap_err(),
        KernelError::FutureSnapshot
    );
    assert_eq!(
        store.decisions_for_objects_as_of(&[], -1).unwrap_err(),
        KernelError::InvalidInput
    );
    assert_eq!(
        store
            .decision_payload_sizes_as_of(&[], tip + 1)
            .unwrap_err(),
        KernelError::FutureSnapshot
    );
    assert_eq!(
        store
            .decisions_for_objects_as_of(&requested, tip + 1)
            .unwrap_err(),
        KernelError::FutureSnapshot
    );
    assert_eq!(
        store
            .decision_payload_sizes_as_of(&requested, tip + 1)
            .unwrap_err(),
        KernelError::FutureSnapshot
    );
    assert_eq!(
        store
            .decisions_for_objects_as_of(&requested, -1)
            .unwrap_err(),
        KernelError::InvalidInput
    );

    let many: Vec<String> = (0..1_200)
        .map(|index| format!("decision-object-{index}"))
        .collect();
    assert_eq!(
        ids(store.decisions_for_objects_as_of(&many, tip).unwrap()),
        ["decision-object-1", "decision-object-3"]
    );
}

#[test]
fn corrections_preserve_old_rows_and_reauthor_observation_dependencies() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    seed_domain(&store);
    store
        .commit(intent("seed-slice", 'b'), |envelope| {
            envelope.insert_decision(decision(1))?;
            let mut independent = decision(2);
            independent.source_id = "independent-decision".to_string();
            envelope.insert_decision(independent)?;
            envelope.insert_observation(observation(1, "decision-object-1"))?;
            Ok(String::new())
        })
        .unwrap();

    let before = store.known_as_of(2).unwrap();
    assert!(
        before
            .objects
            .iter()
            .any(|row| row.object_id == "decision-object-1")
    );
    assert!(
        !before
            .objects
            .iter()
            .any(|row| row.object_id == "decision-object-3")
    );
    store
        .commit(intent("correct", 'c'), |envelope| {
            let mut replacement = decision(3);
            replacement.source_revision = 2;
            envelope.correct_decision("decision-object-1", replacement)?;
            let mut replacement = observation(2, "decision-object-2");
            replacement.source_revision = 2;
            envelope.correct_observation("observation-object-1", replacement)?;
            Ok(String::new())
        })
        .unwrap();

    let after = store.known_as_of(3).unwrap();
    assert!(
        !after
            .objects
            .iter()
            .any(|row| row.object_id == "decision-object-1")
    );
    assert!(
        after
            .objects
            .iter()
            .any(|row| row.object_id == "decision-object-3")
    );

    assert_eq!(
        inspect_i64(directory.path(), "SELECT COUNT(*) FROM decisions"),
        3
    );
    assert_eq!(
        inspect_i64(directory.path(), "SELECT COUNT(*) FROM observations"),
        2
    );
    assert_eq!(
        inspect_i64(
            directory.path(),
            "SELECT COUNT(*) FROM observation_dependencies"
        ),
        2
    );
    let connection = Connection::open(directory.path().join("kernel.sqlite")).unwrap();
    let corrected_dependency: String = connection
        .query_row(
            "SELECT dependency_object_id FROM observation_dependencies
             WHERE observation_id='observation-2'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(corrected_dependency, "decision-object-2");
    assert_eq!(
        inspect_i64(
            directory.path(),
            "SELECT COUNT(*) FROM decisions
             WHERE decision_id='decision-1' AND invalidated_commit_seq=3
             AND superseded_by='decision-object-3'"
        ),
        1
    );

    let error = store
        .commit(intent("correct-dead", 'd'), |envelope| {
            envelope.correct_decision("decision-object-1", decision(4))?;
            Ok(String::new())
        })
        .unwrap_err();
    assert_eq!(error, KernelError::NotFound);

    let unbumped = store
        .commit(intent("unbumped", 'e'), |envelope| {
            let mut replacement = decision(4);
            replacement.source_revision = 2;
            envelope.correct_decision("decision-object-3", replacement)?;
            Ok(String::new())
        })
        .unwrap_err();
    assert_eq!(unbumped, KernelError::Conflict);

    store
        .commit(intent("retire-observation", 'f'), |envelope| {
            Ok(envelope
                .retire_observation("observation-object-2")?
                .result_json())
        })
        .unwrap();
    assert_eq!(
        inspect_i64(
            directory.path(),
            "SELECT COUNT(*) FROM observations
             WHERE observation_id='observation-2' AND invalidated_commit_seq=4
             AND superseded_by IS NULL"
        ),
        1
    );
}

#[test]
fn live_dependent_observations_follow_the_dependency_edge_and_drop_retired_rows() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    seed_domain(&store);
    store
        .commit(intent("seed", 'a'), |envelope| {
            envelope.insert_decision(decision(1))?;
            envelope.insert_decision(decision(2))?;
            envelope.insert_observation(observation(1, "decision-object-1"))?;
            envelope.insert_observation(observation(2, "decision-object-1"))?;
            envelope.insert_observation(observation(3, "decision-object-2"))?;
            let mut other_kind = observation(4, "decision-object-1");
            other_kind.observation_kind = "classification".to_string();
            envelope.insert_observation(other_kind)?;
            let mut other_edge = observation(5, "decision-object-1");
            other_edge.dependencies[0].dependency_kind = "classifies".to_string();
            envelope.insert_observation(other_edge)?;
            // Same edge and kind, another writer: excluded by source kind.
            let mut other_writer = observation(6, "decision-object-1");
            other_writer.source_kind = "other-producer".to_string();
            envelope.insert_observation(other_writer)?;
            Ok(String::new())
        })
        .unwrap();
    store
        .commit(intent("read-and-retire", 'b'), |envelope| {
            assert_eq!(
                envelope.live_dependent_observations(&fixture_dependents(
                    "decision-object-1",
                    "implements",
                    "implementation"
                ))?,
                ["observation-object-1", "observation-object-2"]
            );
            assert_eq!(
                envelope.live_dependent_observations(&DependentObservationQuery {
                    source_kind: "other-producer",
                    ..fixture_dependents("decision-object-1", "implements", "implementation")
                })?,
                ["observation-object-6"]
            );
            assert!(
                envelope
                    .live_dependent_observations(&DependentObservationQuery {
                        scope_id: Some("scope:elsewhere"),
                        ..fixture_dependents("decision-object-1", "implements", "implementation")
                    })?
                    .is_empty(),
                "an unscoped row does not match a scoped query"
            );
            assert_eq!(
                envelope.live_dependent_observations(&fixture_dependents(
                    "decision-object-1",
                    "classifies",
                    "implementation"
                ))?,
                ["observation-object-5"]
            );
            assert_eq!(
                envelope.live_dependent_observations(&fixture_dependents(
                    "decision-object-1",
                    "implements",
                    "classification"
                ))?,
                ["observation-object-4"]
            );
            assert!(
                envelope
                    .live_dependent_observations(&fixture_dependents(
                        "decision-object-9",
                        "implements",
                        "implementation"
                    ))?
                    .is_empty()
            );
            // A retirement in this envelope is visible to the next call.
            envelope.retire_observation("observation-object-1")?;
            assert_eq!(
                envelope.live_dependent_observations(&fixture_dependents(
                    "decision-object-1",
                    "implements",
                    "implementation"
                ))?,
                ["observation-object-2"]
            );
            Ok(String::new())
        })
        .unwrap();
}

#[test]
fn live_dependent_observations_are_ordered_by_commit_then_object_id() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    seed_domain(&store);
    // The later commit inserts its rows in reverse-lexical order; the earlier
    // commit's row sorts first whatever its id.
    store
        .commit(intent("first", 'a'), |envelope| {
            envelope.insert_decision(decision(1))?;
            envelope.insert_observation(observation(9, "decision-object-1"))?;
            Ok(String::new())
        })
        .unwrap();
    store
        .commit(intent("second", 'b'), |envelope| {
            envelope.insert_observation(observation(3, "decision-object-1"))?;
            envelope.insert_observation(observation(2, "decision-object-1"))?;
            envelope.insert_observation(observation(1, "decision-object-1"))?;
            Ok(String::new())
        })
        .unwrap();
    store
        .commit(intent("read", 'c'), |envelope| {
            assert_eq!(
                envelope.live_dependent_observations(&fixture_dependents(
                    "decision-object-1",
                    "implements",
                    "implementation"
                ))?,
                [
                    "observation-object-9",
                    "observation-object-1",
                    "observation-object-2",
                    "observation-object-3",
                ]
            );
            Ok(String::new())
        })
        .unwrap();
}

#[test]
fn a_secret_bearing_slice_identifier_is_refused_rather_than_redacted() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    seed_domain(&store);

    // Every identifier is a lookup key somewhere, so none of them may carry a
    // detected secret: redaction would alias distinct values onto one row.
    let mut secret_object = decision(1);
    secret_object.object_id = format!("decision-{SECRET}");
    let mut secret_decision = decision(2);
    secret_decision.decision_id = format!("decision-{SECRET}");
    let mut secret_source = decision(3);
    secret_source.source_id = format!("src/{SECRET}");
    let mut secret_observation = observation(1, "decision-object-9");
    secret_observation.observation_id = format!("observation-{SECRET}");
    let mut secret_dependency = observation(2, &format!("decision-{SECRET}"));
    secret_dependency.observation_id = "observation-2".to_string();
    for (label, result) in [
        (
            "object_id",
            store.commit(intent("secret-object", '1'), |envelope| {
                envelope.insert_decision(secret_object)?;
                Ok(String::new())
            }),
        ),
        (
            "decision_id",
            store.commit(intent("secret-decision", '2'), |envelope| {
                envelope.insert_decision(secret_decision)?;
                Ok(String::new())
            }),
        ),
        (
            "source_id",
            store.commit(intent("secret-source", '3'), |envelope| {
                envelope.insert_decision(secret_source)?;
                Ok(String::new())
            }),
        ),
        (
            "observation_id",
            store.commit(intent("secret-observation", '4'), |envelope| {
                envelope.insert_observation(secret_observation)?;
                Ok(String::new())
            }),
        ),
        (
            "dependency_object_id",
            store.commit(intent("secret-dependency", '5'), |envelope| {
                envelope.insert_observation(secret_dependency)?;
                Ok(String::new())
            }),
        ),
    ] {
        assert_eq!(result.unwrap_err(), KernelError::InvalidInput, "{label}");
    }
    assert_eq!(
        inspect_i64(
            directory.path(),
            "SELECT COUNT(*) FROM object_registry WHERE object_kind IN ('decision','observation')"
        ),
        0
    );
    assert!(
        !family_bytes(directory.path())
            .windows(SECRET.len())
            .any(|window| window == SECRET.as_bytes())
    );
}

#[test]
fn a_secret_bearing_selector_cannot_reach_another_decision() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    seed_domain(&store);
    // A live decision whose ids equal the placeholder form a redacted selector
    // would collapse to.
    let mut placeholder = decision(1);
    placeholder.decision_id = "decision-<ANTHROPIC_API_KEY_REDACTED>".to_string();
    placeholder.object_id = "object-<ANTHROPIC_API_KEY_REDACTED>".to_string();
    store
        .commit(intent("placeholder", '1'), |envelope| {
            envelope.insert_decision(placeholder)?;
            Ok(String::new())
        })
        .unwrap();

    let event = DecisionEventSpec {
        event_kind: "note".to_string(),
        payload: DecisionEventPayload {
            summary: "appended through a redacted selector".to_string(),
        },
        evidence_id: None,
        recorded_at: 1,
    };
    let error = store
        .commit(intent("append", '2'), |envelope| {
            envelope.append_decision_event(&format!("decision-{SECRET}"), event)?;
            Ok(String::new())
        })
        .unwrap_err();
    assert_eq!(error, KernelError::InvalidInput);
    for (label, result) in [
        (
            "correct",
            store.commit(intent("correct", '3'), |envelope| {
                let mut replacement = decision(2);
                replacement.source_revision = 2;
                envelope.correct_decision(&format!("object-{SECRET}"), replacement)?;
                Ok(String::new())
            }),
        ),
        (
            "retire",
            store.commit(intent("retire", '4'), |envelope| {
                envelope.retire_decision(&format!("object-{SECRET}"))?;
                Ok(String::new())
            }),
        ),
    ] {
        assert_eq!(result.unwrap_err(), KernelError::InvalidInput, "{label}");
    }
    assert_eq!(
        inspect_i64(directory.path(), "SELECT COUNT(*) FROM decision_events"),
        0,
        "a redacted selector reached the placeholder-named decision"
    );
    assert_eq!(
        inspect_i64(
            directory.path(),
            "SELECT COUNT(*) FROM decisions WHERE invalidated_commit_seq IS NOT NULL"
        ),
        0
    );
}

#[test]
fn corrections_reject_cross_domain_replacements() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    seed_domain(&store);
    store
        .commit(intent("second-domain", '1'), |envelope| {
            envelope.insert_domain(DomainSpec {
                domain_id: "domain-2".to_string(),
                object_id: "domain-object-2".to_string(),
                name: "fixture-2".to_string(),
                source_kind: "fixture".to_string(),
                source_id: "domain-2".to_string(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
            })?;
            Ok(String::new())
        })
        .unwrap();
    store
        .commit(intent("cross-domain-seed", '2'), |envelope| {
            envelope.insert_decision(decision(1))?;
            envelope.insert_observation(observation(1, "decision-object-1"))?;
            Ok(String::new())
        })
        .unwrap();

    let moved_decision = store
        .commit(intent("cross-domain-decision", '3'), |envelope| {
            let mut replacement = decision(2);
            replacement.domain_id = "domain-2".to_string();
            replacement.source_revision = 2;
            envelope.correct_decision("decision-object-1", replacement)?;
            Ok(String::new())
        })
        .unwrap_err();
    assert_eq!(moved_decision, KernelError::InvalidInput);

    let moved_observation = store
        .commit(intent("cross-domain-observation", '4'), |envelope| {
            let mut replacement = observation(2, "decision-object-1");
            replacement.domain_id = "domain-2".to_string();
            replacement.source_revision = 2;
            envelope.correct_observation("observation-object-1", replacement)?;
            Ok(String::new())
        })
        .unwrap_err();
    assert_eq!(moved_observation, KernelError::InvalidInput);

    assert_eq!(
        inspect_i64(
            directory.path(),
            "SELECT COUNT(*) FROM object_registry WHERE invalidated_commit_seq IS NOT NULL"
        ),
        0
    );
}

fn subject_request(object_id: &str, kind: EventKind) -> AdmissionRequest {
    AdmissionRequest {
        candidate_id: None,
        subject_object_id: Some(object_id.to_string()),
        source_class: Some(SourceClass::TrustedLocalCode),
        taint_class: Some(TaintClass::CurrentCode),
        event: AdmissionEvent {
            kind,
            trigger_object_id: None,
            approval_object_id: None,
            evidence_id: None,
            reason: format!("{kind:?}"),
        },
    }
}

fn live_decision_objects(store: &KernelStore) -> Vec<(String, Option<String>)> {
    let tip = store.tip().unwrap();
    store
        .object_history_as_of(tip)
        .unwrap()
        .objects
        .into_iter()
        .filter(|object| object.object_kind == "decision")
        .map(|object| {
            (
                object.object_id,
                object
                    .invalidated_commit_seq
                    .map(|_| object.superseded_by.unwrap_or_default()),
            )
        })
        .collect()
}

#[test]
fn supersede_decision_replaces_a_live_predecessor_and_folds_merges_into_a_survivor() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    seed_domain(&store);
    store
        .commit(intent("seed", '1'), |envelope| {
            envelope.insert_decision(decision(1))?;
            envelope.insert_decision(decision(2))?;
            Ok(String::new())
        })
        .unwrap();

    // Revision: the replacement is a new row, the predecessor names it.
    let revised = store
        .commit(intent("revise", '2'), |envelope| {
            let outcome = envelope.supersede_decision("decision-object-1", decision(3))?;
            assert_eq!(outcome.object_id, "decision-object-3");
            Ok(String::new())
        })
        .unwrap();
    let live = store.known_as_of(revised.commit_seq).unwrap();
    let live_ids: Vec<_> = live
        .objects
        .iter()
        .filter(|object| object.object_kind == "decision")
        .map(|object| object.object_id.as_str())
        .collect();
    assert_eq!(live_ids, ["decision-object-2", "decision-object-3"]);
    assert!(live_decision_objects(&store).contains(&(
        "decision-object-1".to_string(),
        Some("decision-object-3".to_string())
    )));

    // Merge: two predecessors name one already-live survivor in one envelope;
    // no row is written for the survivor.
    store
        .commit(intent("survivor", '3'), |envelope| {
            envelope.insert_decision(decision(4))?;
            Ok(String::new())
        })
        .unwrap();
    let merged = store
        .commit(intent("merge", '4'), |envelope| {
            let first = envelope.supersede_decision("decision-object-2", decision(4))?;
            let second = envelope.supersede_decision("decision-object-3", decision(4))?;
            assert_eq!(first.decision_id, "decision-4");
            assert_eq!(second.object_id, "decision-object-4");
            Ok(String::new())
        })
        .unwrap();
    let live = store.known_as_of(merged.commit_seq).unwrap();
    let live_ids: Vec<_> = live
        .objects
        .iter()
        .filter(|object| object.object_kind == "decision")
        .map(|object| object.object_id.as_str())
        .collect();
    assert_eq!(live_ids, ["decision-object-4"]);
    let history = live_decision_objects(&store);
    for predecessor in ["decision-object-2", "decision-object-3"] {
        assert!(history.contains(&(
            predecessor.to_string(),
            Some("decision-object-4".to_string())
        )));
    }
    assert_eq!(
        inspect_i64(
            directory.path(),
            "SELECT COUNT(*) FROM decisions WHERE object_id='decision-object-4'"
        ),
        1
    );
    assert_eq!(
        inspect_i64(
            directory.path(),
            "SELECT COUNT(*) FROM change_event WHERE commit_seq=(SELECT MAX(commit_seq) FROM commit_log)"
        ),
        2
    );

    // A survivor cannot supersede itself.
    let self_reference = store
        .commit(intent("self", '5'), |envelope| {
            envelope.supersede_decision("decision-object-4", decision(4))?;
            Ok(String::new())
        })
        .unwrap_err();
    assert_eq!(self_reference, KernelError::InvalidInput);
}

#[test]
fn supersede_decision_refuses_a_quarantined_predecessor() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    seed_domain(&store);
    store
        .commit(intent("seed", '1'), |envelope| {
            envelope.insert_decision(decision(1))?;
            envelope
                .record_admission(subject_request("decision-object-1", EventKind::Quarantine))?;
            Ok(String::new())
        })
        .unwrap();

    let refused = store
        .commit(intent("supersede", '2'), |envelope| {
            envelope.supersede_decision("decision-object-1", decision(2))?;
            Ok(String::new())
        })
        .unwrap_err();
    assert_eq!(refused, KernelError::AdmissionPolicy);
    assert_eq!(
        inspect_i64(
            directory.path(),
            "SELECT COUNT(*) FROM object_registry WHERE invalidated_commit_seq IS NOT NULL"
        ),
        0
    );
    let missing = store
        .commit(intent("missing", '3'), |envelope| {
            envelope.supersede_decision("decision-object-9", decision(2))?;
            Ok(String::new())
        })
        .unwrap_err();
    assert_eq!(missing, KernelError::NotFound);
}

/// A supersession whose replacement is already live folds the predecessor
/// into that survivor, so the survivor's lineage is judged as well.
#[test]
fn a_fold_refuses_a_quarantined_survivor() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    seed_domain(&store);
    store
        .commit(intent("seed", '1'), |envelope| {
            envelope.insert_decision(decision(1))?;
            envelope.insert_decision(decision(2))?;
            envelope.insert_decision(decision(3))?;
            envelope
                .record_admission(subject_request("decision-object-3", EventKind::Quarantine))?;
            Ok(String::new())
        })
        .unwrap();

    // The survivor's revision advances past the predecessor's, so only the
    // lineage guard stands between this fold and the write.
    let barred_survivor = store
        .commit(intent("fold-into-quarantined", '2'), |envelope| {
            envelope.supersede_decision("decision-object-2", decision(3))?;
            Ok(String::new())
        })
        .unwrap_err();
    assert_eq!(barred_survivor, KernelError::AdmissionPolicy);
    assert_eq!(
        inspect_i64(
            directory.path(),
            "SELECT COUNT(*) FROM object_registry WHERE invalidated_commit_seq IS NOT NULL OR superseded_by IS NOT NULL"
        ),
        0
    );

    store
        .commit(intent("fold", '3'), |envelope| {
            envelope.supersede_decision("decision-object-1", decision(2))?;
            Ok(String::new())
        })
        .unwrap();
    assert_eq!(
        inspect_i64(
            directory.path(),
            "SELECT COUNT(*) FROM object_registry WHERE object_id = 'decision-object-1' AND superseded_by = 'decision-object-2'"
        ),
        1
    );
}

#[test]
fn a_poisoned_envelope_refuses_every_later_slice_mutation() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    seed_domain(&store);
    store
        .commit(intent("first", '1'), |envelope| {
            envelope.insert_decision(decision(1))?;
            Ok(String::new())
        })
        .unwrap();

    // The failed insert writes `decision-object-2` to `object_registry` before
    // detecting duplicate `decision-1`; the callback swallows that failure.
    let mut duplicate = decision(2);
    duplicate.decision_id = "decision-1".to_string();
    let error = store
        .commit(intent("poison", '2'), |envelope| {
            assert_eq!(
                envelope.insert_decision(duplicate).unwrap_err(),
                KernelError::Conflict
            );
            // Valid on their own, but the envelope already recorded a failure.
            assert_eq!(
                envelope.insert_decision(decision(3)).unwrap_err(),
                KernelError::Conflict
            );
            assert_eq!(
                envelope
                    .insert_observation(observation(1, "decision-object-1"))
                    .unwrap_err(),
                KernelError::Conflict
            );
            assert_eq!(
                envelope.retire_decision("decision-object-1").unwrap_err(),
                KernelError::Conflict
            );
            Ok("swallowed".to_string())
        })
        .unwrap_err();
    assert_eq!(error, KernelError::Conflict);
    assert_eq!(
        inspect_i64(
            directory.path(),
            "SELECT COUNT(*) FROM object_registry WHERE object_kind IN ('decision','observation')"
        ),
        1
    );
    assert_eq!(
        inspect_i64(
            directory.path(),
            "SELECT COUNT(*) FROM object_registry WHERE object_id='decision-object-2'"
        ),
        0,
        "a swallowed failure committed a registry row"
    );
    assert_eq!(
        inspect_i64(directory.path(), "SELECT COUNT(*) FROM commit_log"),
        2
    );
}

#[test]
fn a_correction_cannot_relabel_a_predecessor_below_its_class() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    seed_domain(&store);
    store
        .commit(intent("seed-secret", 'b'), |envelope| {
            let mut secret = decision(1);
            secret.sensitivity = Sensitivity::Secret;
            envelope.insert_decision(secret)?;
            let mut secret = observation(1, "decision-object-1");
            secret.sensitivity = Sensitivity::Secret;
            envelope.insert_observation(secret)?;
            // A live normal decision that a later correction will try to fold
            // the secret predecessor into.
            let mut survivor = decision(5);
            survivor.source_revision = 5;
            envelope.insert_decision(survivor)?;
            Ok(String::new())
        })
        .unwrap();

    // A replacement asserting normal is written at the predecessor's secret.
    store
        .commit(intent("correct-down", 'c'), |envelope| {
            let mut replacement = decision(2);
            replacement.source_revision = 2;
            replacement.sensitivity = Sensitivity::Normal;
            envelope.correct_decision("decision-object-1", replacement)?;
            let mut replacement = observation(2, "decision-object-2");
            replacement.source_revision = 2;
            replacement.sensitivity = Sensitivity::Normal;
            envelope.correct_observation("observation-object-1", replacement)?;
            Ok(String::new())
        })
        .unwrap();
    let connection = Connection::open(directory.path().join("kernel.sqlite")).unwrap();
    let classes: Vec<(String, String)> = connection
        .prepare(
            "SELECT object_id,sensitivity_class FROM object_registry
             WHERE object_id IN ('decision-object-2','observation-object-2')
             ORDER BY object_id",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        classes,
        vec![
            ("decision-object-2".to_string(), "secret".to_string()),
            ("observation-object-2".to_string(), "secret".to_string()),
        ]
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT sensitivity_class FROM decisions WHERE object_id='decision-object-2'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "secret"
    );

    // Folding the (now secret) decision into the live normal survivor would
    // publish it under the survivor's weaker class, so the fold is refused and
    // the predecessor stays live.
    let error = store
        .commit(intent("fold-down", 'd'), |envelope| {
            let mut into_survivor = decision(5);
            into_survivor.source_revision = 6;
            envelope.correct_decision("decision-object-2", into_survivor)?;
            Ok(String::new())
        })
        .unwrap_err();
    assert_eq!(error, KernelError::InvalidInput);
    assert_eq!(
        inspect_i64(
            directory.path(),
            "SELECT COUNT(*) FROM object_registry
             WHERE object_id='decision-object-2' AND invalidated_commit_seq IS NULL"
        ),
        1
    );

    // The class the correction itself asserts binds the fold the same way: a
    // normal predecessor corrected with a secret replacement cannot land in a
    // normal survivor either.
    store
        .commit(intent("normal-predecessor", 'e'), |envelope| {
            let mut normal = decision(7);
            normal.source_revision = 7;
            envelope.insert_decision(normal)?;
            Ok(String::new())
        })
        .unwrap();
    let error = store
        .commit(intent("fold-secret-assertion", 'f'), |envelope| {
            let mut into_survivor = decision(5);
            into_survivor.source_revision = 8;
            into_survivor.sensitivity = Sensitivity::Secret;
            envelope.correct_decision("decision-object-7", into_survivor)?;
            Ok(String::new())
        })
        .unwrap_err();
    assert_eq!(error, KernelError::InvalidInput);
    assert_eq!(
        inspect_i64(
            directory.path(),
            "SELECT COUNT(*) FROM object_registry
             WHERE object_id='decision-object-7' AND invalidated_commit_seq IS NULL"
        ),
        1
    );
}

#[test]
fn a_decision_citing_evidence_is_classified_no_lower_than_that_evidence() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    seed_domain(&store);
    let handle = store
        .ingest_artifact(ArtifactIngestRequest {
            intent: intent("secret-evidence", '1'),
            payload: b"secret fixture evidence".to_vec(),
            evidence_id: "secret-evidence".to_string(),
            object_id: "secret-evidence-object".to_string(),
            object_kind: "evidence".to_string(),
            domain_id: "domain".to_string(),
            source_kind: "fixture".to_string(),
            source_id: "evidence".to_string(),
            source_revision: 1,
            media_type: "text/plain".to_string(),
            retention_class: "canonical".to_string(),
            retain_until: None,
            asserted_sensitivity: Sensitivity::Secret,
            provider_egress: ProviderEgress::RemoteAllowed,
            provenance: Some(RepositoryProvenance {
                repository_id: "fixture".to_string(),
                revision: "abc123".to_string(),
            }),
        })
        .unwrap();
    assert_eq!(handle.evidence_id, "secret-evidence");

    // Both a decision and an observation asserting normal over that evidence
    // are stored at the evidence's class.
    store
        .commit(intent("cite-secret", '2'), |envelope| {
            let mut cites = decision(1);
            cites.evidence_id = Some("secret-evidence".to_string());
            cites.sensitivity = Sensitivity::Normal;
            envelope.insert_decision(cites)?;
            let mut observes = observation(1, "decision-object-1");
            observes.evidence_id = Some("secret-evidence".to_string());
            observes.sensitivity = Sensitivity::Normal;
            envelope.insert_observation(observes)?;
            Ok(String::new())
        })
        .unwrap();
    let connection = Connection::open(directory.path().join("kernel.sqlite")).unwrap();
    let classes: Vec<(String, String)> = connection
        .prepare(
            "SELECT object_id,sensitivity_class FROM object_registry
             WHERE object_id IN ('decision-object-1','observation-object-1') ORDER BY object_id",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        classes,
        vec![
            ("decision-object-1".to_string(), "secret".to_string()),
            ("observation-object-1".to_string(), "secret".to_string()),
        ]
    );
    let rows: (String, String) = connection
        .query_row(
            "SELECT (SELECT sensitivity_class FROM decisions WHERE decision_id='decision-1'),
                    (SELECT sensitivity_class FROM observations WHERE observation_id='observation-1')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(rows, ("secret".to_string(), "secret".to_string()));
}

#[test]
fn a_decision_serves_no_lower_than_its_cited_evidence_reads_today() {
    use kernel::Surface;

    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    seed_domain(&store);
    let payload = b"normal now, secret later".to_vec();
    let ingest = |sensitivity: Sensitivity| ArtifactIngestRequest {
        intent: intent("evidence-later-secret", '1'),
        payload: payload.clone(),
        evidence_id: "later-secret".to_string(),
        object_id: "later-secret-object".to_string(),
        object_kind: "evidence".to_string(),
        domain_id: "domain".to_string(),
        source_kind: "fixture".to_string(),
        source_id: "evidence".to_string(),
        source_revision: 1,
        media_type: "text/plain".to_string(),
        retention_class: "canonical".to_string(),
        retain_until: None,
        asserted_sensitivity: sensitivity,
        provider_egress: ProviderEgress::RemoteAllowed,
        provenance: Some(RepositoryProvenance {
            repository_id: "fixture".to_string(),
            revision: "abc123".to_string(),
        }),
    };
    store.ingest_artifact(ingest(Sensitivity::Normal)).unwrap();
    store
        .commit(intent("cite-and-admit", '2'), |envelope| {
            let mut cites = decision(1);
            cites.evidence_id = Some("later-secret".to_string());
            envelope.insert_decision(cites)?;
            envelope.record_admission(AdmissionRequest {
                candidate_id: None,
                subject_object_id: Some("decision-object-1".to_string()),
                source_class: Some(SourceClass::TrustedLocalCode),
                taint_class: Some(TaintClass::CurrentCode),
                event: AdmissionEvent {
                    kind: EventKind::Other,
                    trigger_object_id: None,
                    approval_object_id: None,
                    evidence_id: None,
                    reason: "fixture".to_string(),
                },
            })?;
            Ok(String::new())
        })
        .unwrap();
    let tip = store.tip().unwrap();
    let served = store.visible_as_of(Surface::ExplicitSearch, tip).unwrap();
    assert!(
        served
            .rows
            .iter()
            .any(|row| row.object.object_id == "decision-object-1"),
        "the decision serves while its evidence is normal"
    );

    // An idempotent replay tightens the evidence to secret. The decision's own
    // rows are immutable and still say normal; serving reads the evidence.
    store.ingest_artifact(ingest(Sensitivity::Secret)).unwrap();
    let tip = store.tip().unwrap();
    let served = store.visible_as_of(Surface::ExplicitSearch, tip).unwrap();
    assert!(
        !served
            .rows
            .iter()
            .any(|row| row.object.object_id == "decision-object-1"),
        "a decision citing now-secret evidence kept serving"
    );
}

#[test]
fn decision_pages_examine_a_bounded_row_count_and_cut_before_the_byte_bound() {
    use std::num::{NonZeroU64, NonZeroUsize};

    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    seed_domain(&store);
    let inserted = store
        .commit(intent("decisions", '1'), |envelope| {
            for index in 1..=5 {
                envelope.insert_decision(decision(index))?;
            }
            Ok(String::new())
        })
        .unwrap();
    store
        .commit(intent("retire", '2'), |envelope| {
            envelope.retire_decision("decision-object-2")?;
            envelope.retire_decision("decision-object-3")?;
            Ok(String::new())
        })
        .unwrap();
    let tip = store.tip().unwrap();
    let budget = kernel::applicability::EvalBudget::unbounded();
    let rows = |n| NonZeroUsize::new(n).unwrap();
    let bytes = NonZeroU64::new(1 << 20).unwrap();
    let live = |page: &kernel::DecisionPage| -> Vec<String> {
        page.decisions
            .iter()
            .map(|entry| {
                assert_eq!(entry.object.object_id, entry.decision.object_id);
                entry.object.object_id.clone()
            })
            .collect()
    };

    // Three rows are examined, two of them invalidated, so the page returns one live decision and continues.
    let first = store
        .decision_page_within_budget(tip, None, rows(3), bytes, &budget)
        .unwrap();
    assert_eq!(live(&first), ["decision-object-1"]);
    assert_eq!(first.next.as_deref(), Some("decision-object-3"));
    let second = store
        .decision_page_within_budget(tip, first.next.as_deref(), rows(3), bytes, &budget)
        .unwrap();
    assert_eq!(live(&second), ["decision-object-4", "decision-object-5"]);
    assert_eq!(second.next, None);

    // Read at the insert commit, every decision is live, and each registry row reads as it stood at that commit.
    let earlier = store
        .decision_page_within_budget(inserted.commit_seq, None, rows(8), bytes, &budget)
        .unwrap();
    assert_eq!(earlier.decisions.len(), 5);
    for entry in &earlier.decisions {
        assert_eq!(
            (
                entry.object.invalidated_commit_seq,
                entry.object.superseded_by.as_deref()
            ),
            (None, None),
            "{} carries post-snapshot registry columns",
            entry.object.object_id
        );
    }

    // A byte bound smaller than two payloads cuts the page after one live decision; one larger payload is still returned alone.
    let one = NonZeroU64::new(1).unwrap();
    let cut = store
        .decision_page_within_budget(tip, None, rows(8), one, &budget)
        .unwrap();
    assert_eq!(live(&cut), ["decision-object-1"]);
    assert_eq!(cut.next.as_deref(), Some("decision-object-3"));

    // Twin bounds one byte apart separate a cut before the crossing decision from a cut after it.
    let length = |object: &str| -> u64 {
        Connection::open_with_flags(
            directory.path().join("kernel.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap()
        .query_row(
            "SELECT length(decision_payload) FROM decisions WHERE object_id=?1",
            [object],
            |row| row.get::<_, i64>(0),
        )
        .unwrap() as u64
    };
    let both = length("decision-object-1") + length("decision-object-4");
    let short = store
        .decision_page_within_budget(
            tip,
            None,
            rows(8),
            NonZeroU64::new(both - 1).unwrap(),
            &budget,
        )
        .unwrap();
    assert_eq!(live(&short), ["decision-object-1"]);
    assert_eq!(short.next.as_deref(), Some("decision-object-3"));
    let exact = store
        .decision_page_within_budget(tip, None, rows(8), NonZeroU64::new(both).unwrap(), &budget)
        .unwrap();
    assert_eq!(live(&exact), ["decision-object-1", "decision-object-4"]);
    assert_eq!(exact.next.as_deref(), Some("decision-object-4"));

    // A page of only invalidated rows returns nothing live and still moves the cursor.
    let empty = store
        .decision_page_within_budget(tip, Some("decision-object-1"), rows(2), bytes, &budget)
        .unwrap();
    assert!(empty.decisions.is_empty());
    assert_eq!(empty.next.as_deref(), Some("decision-object-3"));
    // Exactly `max_rows` rows left: the page ends without a cursor.
    let last = store
        .decision_page_within_budget(tip, Some("decision-object-3"), rows(2), bytes, &budget)
        .unwrap();
    assert_eq!(live(&last), ["decision-object-4", "decision-object-5"]);
    assert_eq!(last.next, None);
    let lineage = |page: &kernel::DecisionPage| -> Vec<(String, i64)> {
        page.decisions
            .iter()
            .map(|entry| (entry.object.object_id.clone(), entry.lineage_commit_seq))
            .collect()
    };
    assert_eq!(
        lineage(&last),
        [
            ("decision-object-4".to_string(), inserted.commit_seq),
            ("decision-object-5".to_string(), inserted.commit_seq),
        ]
    );

    // Every decision shares one source lineage, so correcting 4 with 5's spec folds 4 into the live 5 without rewriting 5's row.
    let folded = store
        .commit(intent("fold", '3'), |envelope| {
            envelope.correct_decision("decision-object-4", decision(5))?;
            Ok(String::new())
        })
        .unwrap();
    let after_fold = store
        .decision_page_within_budget(folded.commit_seq, None, rows(8), bytes, &budget)
        .unwrap();
    assert_eq!(
        lineage(&after_fold),
        [
            ("decision-object-1".to_string(), inserted.commit_seq),
            ("decision-object-5".to_string(), folded.commit_seq),
        ]
    );
    assert_eq!(
        after_fold.decisions[1].object.created_commit_seq,
        inserted.commit_seq
    );
    // Read before the fold, the survivor still reports its creation commit.
    let before_fold = store
        .decision_page_within_budget(tip, Some("decision-object-4"), rows(8), bytes, &budget)
        .unwrap();
    assert_eq!(
        lineage(&before_fold),
        [("decision-object-5".to_string(), inserted.commit_seq)]
    );
    // A snapshot before the fold keeps the predecessor's live registry state.
    let predecessor = store
        .decision_page_within_budget(tip, Some("decision-object-3"), rows(1), bytes, &budget)
        .unwrap();
    assert_eq!(live(&predecessor), ["decision-object-4"]);
    assert_eq!(
        (
            predecessor.decisions[0].object.invalidated_commit_seq,
            predecessor.decisions[0].object.superseded_by.as_deref()
        ),
        (None, None)
    );

    let cancelled = kernel::applicability::EvalBudget::unbounded();
    cancelled.cancel();
    assert_eq!(
        store.decision_page_within_budget(tip, None, rows(3), bytes, &cancelled),
        Err(KernelError::Deadline)
    );
}

fn ingest_evidence(store: &KernelStore, key: &str, digest: char) -> String {
    store
        .ingest_artifact(ArtifactIngestRequest {
            intent: intent(&format!("evidence-{key}"), digest),
            payload: format!("evidence {key}").into_bytes(),
            evidence_id: format!("evidence-{key}"),
            object_id: format!("evidence-object-{key}"),
            object_kind: "evidence".to_string(),
            domain_id: "domain".to_string(),
            source_kind: "fixture".to_string(),
            source_id: format!("evidence-{key}"),
            source_revision: 1,
            media_type: "text/plain".to_string(),
            retention_class: "canonical".to_string(),
            retain_until: None,
            asserted_sensitivity: Sensitivity::Normal,
            provider_egress: ProviderEgress::RemoteAllowed,
            provenance: None,
        })
        .unwrap()
        .evidence_id
}

/// Appends events to `decision_id` in one commit, each citing its evidence or nothing.
fn cite_all(
    store: &KernelStore,
    decision_id: &str,
    key: &str,
    digest: char,
    cited: &[Option<&str>],
) {
    store
        .commit(intent(&format!("cite-{key}"), digest), |envelope| {
            for evidence_id in cited {
                envelope.append_decision_event(
                    decision_id,
                    DecisionEventSpec {
                        event_kind: "evidence".to_string(),
                        payload: DecisionEventPayload {
                            summary: format!("cites {key}"),
                        },
                        evidence_id: evidence_id.map(str::to_string),
                        recorded_at: 10,
                    },
                )?;
            }
            Ok(String::new())
        })
        .unwrap();
}

fn cite(store: &KernelStore, key: &str, digest: char, evidence_id: &str) {
    cite_all(store, "decision-1", key, digest, &[Some(evidence_id)]);
}

/// The evidence a decision cites, itself or through its events: most recently cited first, deduplicated, bounded, as of the snapshot read, with each row's liveness; a decision unknown at the snapshot cites nothing and an unrelated decision's citations are not its own.
#[test]
fn linked_decision_evidence_is_the_decisions_own_citations_newest_first_and_bounded() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    seed_domain(&store);
    let budget = kernel::applicability::EvalBudget::unbounded();
    let linked = |requested: i64, max: usize| -> Vec<(String, bool)> {
        store
            .linked_decision_evidence_within_budget(
                "decision-object-1",
                requested,
                std::num::NonZeroUsize::new(max).unwrap(),
                &budget,
            )
            .unwrap()
            .into_iter()
            .map(|row| (row.evidence_id, row.live))
            .collect()
    };
    let before = store.tip().unwrap();
    let own = ingest_evidence(&store, "own", '1');
    store
        .commit(intent("decision-1", '2'), |envelope| {
            envelope.insert_decision(DecisionSpec {
                evidence_id: Some(own.clone()),
                ..decision(1)
            })?;
            envelope.insert_decision(decision(2))?;
            Ok(String::new())
        })
        .unwrap();
    assert!(linked(before, 8).is_empty(), "unknown at the snapshot");
    assert_eq!(linked(store.tip().unwrap(), 8), vec![(own.clone(), true)]);

    let first = ingest_evidence(&store, "first", '3');
    cite(&store, "first", '4', &first);
    let at_first = store.tip().unwrap();
    let second = ingest_evidence(&store, "second", '5');
    cite(&store, "second", '6', &second);
    // Citing the decision's own evidence again moves it to the front once.
    cite(&store, "own-again", '7', &own);
    let unrelated = ingest_evidence(&store, "unrelated", '8');
    cite_all(&store, "decision-2", "unrelated", '9', &[Some(&unrelated)]);
    // An event that cites nothing adds no row.
    cite_all(&store, "decision-1", "nothing", 'b', &[None]);
    let tip = store.tip().unwrap();
    assert_eq!(
        linked(tip, 8),
        vec![
            (own.clone(), true),
            (second.clone(), true),
            (first.clone(), true)
        ]
    );
    assert_eq!(
        linked(tip, 2),
        vec![(own.clone(), true), (second, true)],
        "the bound keeps the most recently cited"
    );
    assert_eq!(
        linked(at_first, 8),
        vec![(first.clone(), true), (own.clone(), true)],
        "a later citation is not visible at an earlier snapshot"
    );
    // A retired decision's evidence can retire too; its rows read as not live.
    store
        .commit(intent("retire", 'a'), |envelope| {
            envelope.retire_decision("decision-object-1")?;
            envelope.retire_evidence("evidence-object-first")?;
            Ok(String::new())
        })
        .unwrap();
    let retired = linked(store.tip().unwrap(), 8);
    assert!(retired.contains(&(first, false)), "{retired:?}");
    assert!(retired.contains(&(own, true)));
}

/// Two rows cited in one commit tie on their citation, and the bound keeps the lower evidence id; a decision with no evidence of its own reports only its events' citations.
#[test]
fn linked_decision_evidence_breaks_ties_by_evidence_id() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    seed_domain(&store);
    store
        .commit(intent("decision-1", '1'), |envelope| {
            envelope.insert_decision(decision(1))?;
            Ok(String::new())
        })
        .unwrap();
    let b = ingest_evidence(&store, "b", '2');
    let a = ingest_evidence(&store, "a", '3');
    cite_all(&store, "decision-1", "both", '4', &[Some(&b), Some(&a)]);
    let linked = |max: usize| -> Vec<String> {
        store
            .linked_decision_evidence_within_budget(
                "decision-object-1",
                store.tip().unwrap(),
                std::num::NonZeroUsize::new(max).unwrap(),
                &kernel::applicability::EvalBudget::unbounded(),
            )
            .unwrap()
            .into_iter()
            .map(|row| row.evidence_id)
            .collect()
    };
    assert_eq!(linked(8), vec![a.clone(), b]);
    assert_eq!(linked(1), vec![a]);
}

/// The read examines only the decision's newest `LINKED_EVENT_WINDOW` × `max` events, evidence-free events included, so a citation just past that window is absent even when fewer than `max` rows return.
#[test]
fn linked_decision_evidence_reads_only_the_newest_event_window() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    seed_domain(&store);
    store
        .commit(intent("decision-1", '1'), |envelope| {
            envelope.insert_decision(decision(1))?;
            Ok(String::new())
        })
        .unwrap();
    let max = std::num::NonZeroUsize::new(2).unwrap();
    let window = max.get() * kernel::LINKED_EVENT_WINDOW;
    let old = ingest_evidence(&store, "old", '2');
    cite(&store, "old", '3', &old);
    let recent = ingest_evidence(&store, "recent", '4');
    let mut newer = vec![None; window - 2];
    newer.push(Some(recent.as_str()));
    cite_all(&store, "decision-1", "newer", '5', &newer);
    let linked = || -> Vec<String> {
        store
            .linked_decision_evidence_within_budget(
                "decision-object-1",
                store.tip().unwrap(),
                max,
                &kernel::applicability::EvalBudget::unbounded(),
            )
            .unwrap()
            .into_iter()
            .map(|row| row.evidence_id)
            .collect()
    };
    assert_eq!(
        linked(),
        vec![recent.clone(), old],
        "the window's oldest event"
    );
    cite_all(&store, "decision-1", "one-more", '6', &[None]);
    assert_eq!(linked(), vec![recent], "one event past the window");
}

/// The batch read returns, in request order, what each decision's own read returns at the same snapshot: repeats, unknown ids, and an empty request included.
#[test]
fn linked_decisions_evidence_answers_each_decision_as_its_own_read_does() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    seed_domain(&store);
    let own = ingest_evidence(&store, "own", '1');
    store
        .commit(intent("decisions", '2'), |envelope| {
            envelope.insert_decision(DecisionSpec {
                evidence_id: Some(own.clone()),
                ..decision(1)
            })?;
            envelope.insert_decision(decision(2))?;
            Ok(String::new())
        })
        .unwrap();
    let first = ingest_evidence(&store, "first", '3');
    cite_all(&store, "decision-2", "first", '4', &[Some(&first)]);
    let at_first = store.tip().unwrap();
    let second = ingest_evidence(&store, "second", '5');
    cite_all(&store, "decision-1", "second", '6', &[Some(&second)]);
    let budget = kernel::applicability::EvalBudget::unbounded();
    let max = std::num::NonZeroUsize::new(8).unwrap();
    let ids = [
        "decision-object-2",
        "unknown",
        "decision-object-1",
        "decision-object-2",
    ];
    for requested in [at_first, store.tip().unwrap()] {
        let batch = store
            .linked_decisions_evidence_within_budget(&ids, requested, max, &budget)
            .unwrap();
        let single: Vec<_> = ids
            .iter()
            .map(|id| {
                store
                    .linked_decision_evidence_within_budget(id, requested, max, &budget)
                    .unwrap()
            })
            .collect();
        assert_eq!(batch, single);
    }
    assert_eq!(
        store
            .linked_decisions_evidence_within_budget(&ids, store.tip().unwrap(), max, &budget)
            .unwrap()[2]
            .iter()
            .map(|row| row.evidence_id.as_str())
            .collect::<Vec<_>>(),
        [second.as_str(), own.as_str()]
    );
    assert!(
        store
            .linked_decisions_evidence_within_budget(&[], store.tip().unwrap(), max, &budget)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store.linked_decisions_evidence_within_budget(&ids, store.tip().unwrap() + 1, max, &budget),
        Err(KernelError::FutureSnapshot)
    );
}

/// The read agrees with the specified model at every snapshot and bound: walk the newest `LINKED_EVENT_WINDOW` × `max` events at or before the snapshot, keep each cited id at its latest citation with the decision's own evidence cited at its creation, and order by that citation newest first, ties by evidence id. Commits carry one to three events, so citations tie, and evidence repeats, so ids move to the front.
#[test]
fn linked_decision_evidence_matches_the_window_model_under_seeded_citations() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    seed_domain(&store);
    let pool: Vec<String> = (0..10)
        .map(|n| ingest_evidence(&store, &format!("pool-{n}"), '1'))
        .collect();
    store
        .commit(intent("decision-1", '2'), |envelope| {
            envelope.insert_decision(DecisionSpec {
                evidence_id: Some(pool[0].clone()),
                ..decision(1)
            })?;
            Ok(String::new())
        })
        .unwrap();
    let created = store.tip().unwrap();
    let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = move |bound: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % bound
    };
    // Every appended event as (commit sequence, cited evidence), oldest first.
    let mut events: Vec<(i64, Option<String>)> = Vec::new();
    let mut snapshots = vec![created];
    for commit in 0..40 {
        let cited: Vec<Option<&str>> = (0..=next(3))
            .map(|_| {
                let pick = next(12);
                (pick < 10).then(|| pool[usize::try_from(pick).unwrap()].as_str())
            })
            .collect();
        cite_all(
            &store,
            "decision-1",
            &format!("seeded-{commit}"),
            '3',
            &cited,
        );
        let tip = store.tip().unwrap();
        events.extend(cited.iter().map(|id| (tip, id.map(str::to_string))));
        snapshots.push(tip);
    }
    let budget = kernel::applicability::EvalBudget::unbounded();
    for &requested in &snapshots {
        for max in [1usize, 2, 3, 8] {
            let mut cited: Vec<(String, i64)> = vec![(pool[0].clone(), created)];
            for (at, id) in events
                .iter()
                .rev()
                .filter(|(at, _)| *at <= requested)
                .take(max * kernel::LINKED_EVENT_WINDOW)
            {
                let Some(id) = id else { continue };
                match cited.iter_mut().find(|(known, _)| known == id) {
                    Some(entry) => entry.1 = entry.1.max(*at),
                    None => cited.push((id.clone(), *at)),
                }
            }
            cited.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
            cited.truncate(max);
            let expected: Vec<(String, bool)> =
                cited.into_iter().map(|(id, _)| (id, true)).collect();
            let actual: Vec<(String, bool)> = store
                .linked_decision_evidence_within_budget(
                    "decision-object-1",
                    requested,
                    std::num::NonZeroUsize::new(max).unwrap(),
                    &budget,
                )
                .unwrap()
                .into_iter()
                .map(|row| (row.evidence_id, row.live))
                .collect();
            assert_eq!(actual, expected, "snapshot {requested}, max {max}");
        }
    }
}

fn branch_scope(scope_id: &str) -> kernel::ScopeSpec {
    kernel::ScopeSpec {
        scope_id: scope_id.to_string(),
        object_id: format!("{scope_id}-object"),
        domain_id: "domain".to_string(),
        source_kind: "fixture".to_string(),
        source_id: scope_id.to_string(),
        source_revision: 1,
        sensitivity: Sensitivity::Normal,
        terms: vec![kernel::ScopeTermSpec {
            dimension: "branch".to_string(),
            operator: "exact".to_string(),
            exact_value: Some(scope_id.to_string()),
            ..kernel::ScopeTermSpec::default()
        }],
    }
}

/// An evidence row is cited within a set of scopes when it is live and a live
/// decision, observation, or decision event (through its live decision) whose
/// scope the predicate accepts cites it. The predicate sees each citing scope
/// at most once, never an unscoped or retired citer's, and the walk stops at
/// the first accepted scope; a retired row and an unknown id are cited
/// nowhere.
#[test]
fn evidence_cited_in_offers_each_live_citing_scope_once_and_stops_at_the_first_accepted() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    seed_domain(&store);
    let evidence = ingest_evidence(&store, "cited", 'e');
    // The verdict and the scopes offered, in order, when `accepted` names the
    // only scope the predicate takes.
    let cited_in = |evidence_id: &str, accepted: Option<&str>| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let (_, seen) = store
            .preview(deadline, |preview| {
                let mut offered = Vec::new();
                let cited = preview.evidence_cited_in(evidence_id, |scope_id| {
                    offered.push(scope_id.to_string());
                    Ok(Some(scope_id) == accepted)
                })?;
                Ok((cited, offered))
            })
            .unwrap();
        seen
    };
    assert_eq!(cited_in(&evidence, None), (false, Vec::new()));
    assert_eq!(cited_in("evidence-unknown", None), (false, Vec::new()));

    store
        .commit(intent("citers", 'c'), |envelope| {
            for scope_id in ["scope-b", "scope-a", "scope-c", "scope-d"] {
                envelope.insert_scope(branch_scope(scope_id))?;
            }
            let citing = |index: i64, scope_id: Option<&str>, evidence_id: Option<&str>| {
                let mut spec = decision(index);
                spec.scope_id = scope_id.map(str::to_string);
                spec.evidence_id = evidence_id.map(str::to_string);
                spec
            };
            envelope.insert_decision(citing(1, Some("scope-b"), Some(&evidence)))?;
            envelope.insert_decision(citing(2, Some("scope-b"), Some(&evidence)))?;
            envelope.insert_decision(citing(3, None, Some(&evidence)))?;
            envelope.insert_decision(citing(4, Some("scope-c"), None))?;
            envelope.insert_decision(citing(5, Some("scope-d"), Some(&evidence)))?;
            let mut observed = observation(1, "decision-object-1");
            observed.scope_id = Some("scope-a".to_string());
            observed.evidence_id = Some(evidence.clone());
            envelope.insert_observation(observed)?;
            Ok(String::new())
        })
        .unwrap();
    cite_all(&store, "decision-4", "event", 'd', &[Some(&evidence)]);
    store
        .commit(intent("retire-citer", 'f'), |envelope| {
            envelope.retire_decision("decision-object-5")?;
            Ok(String::new())
        })
        .unwrap();
    let (cited, mut offered) = cited_in(&evidence, None);
    assert!(!cited);
    offered.sort();
    assert_eq!(offered, ["scope-a", "scope-b", "scope-c"]);
    for accepted in ["scope-a", "scope-b", "scope-c"] {
        let (cited, offered) = cited_in(&evidence, Some(accepted));
        assert!(cited, "{accepted}");
        assert_eq!(offered.last().map(String::as_str), Some(accepted));
        assert_eq!(
            offered
                .iter()
                .filter(|scope| scope.as_str() == accepted)
                .count(),
            1
        );
    }
    // A scope only a retired or uncited row names is never offered.
    assert!(!cited_in(&evidence, Some("scope-d")).0);

    // Evidence retires only once nothing live cites it.
    let retired = ingest_evidence(&store, "retired", '7');
    store
        .commit(intent("retire-evidence", '8'), |envelope| {
            envelope.retire_evidence("evidence-object-retired")?;
            Ok(String::new())
        })
        .unwrap();
    assert_eq!(cited_in(&retired, None), (false, Vec::new()));
}

/// Artifact deletion invalidates an evidence row that live scoped decisions
/// still cite; the liveness read refuses it before any citer offers a scope.
#[test]
fn evidence_cited_in_refuses_a_deleted_evidence_row_that_live_decisions_still_cite() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    seed_domain(&store);
    let evidence = ingest_evidence(&store, "deleted", 'd');
    store
        .commit(intent("deleted-citer", 'c'), |envelope| {
            envelope.insert_scope(branch_scope("scope-own"))?;
            let mut spec = decision(1);
            spec.scope_id = Some("scope-own".to_string());
            spec.evidence_id = Some(evidence.clone());
            envelope.insert_decision(spec)?;
            Ok(String::new())
        })
        .unwrap();
    let cited_in = |evidence_id: &str| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let (_, seen) = store
            .preview(deadline, |preview| {
                let mut offered = Vec::new();
                let cited = preview.evidence_cited_in(evidence_id, |scope_id| {
                    offered.push(scope_id.to_string());
                    Ok(scope_id == "scope-own")
                })?;
                Ok((cited, offered))
            })
            .unwrap();
        seen
    };
    assert_eq!(cited_in(&evidence), (true, vec!["scope-own".to_string()]));

    store
        .delete_artifact(ArtifactDeletionRequest {
            intent: intent("delete-cited-evidence", 'f'),
            identity: ArtifactDeletionIdentity::EvidenceId(evidence.clone()),
            kind: ArtifactDeletionKind::Delete,
            operator_id: None,
            target_locator: None,
            reason: None,
            deleted_at: 42,
        })
        .unwrap();
    let connection = Connection::open_with_flags(
        directory.path().join("kernel.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let (evidence_retired, citer_live): (bool, bool) = connection
        .query_row(
            "SELECT (SELECT invalidated_commit_seq IS NOT NULL FROM evidence_meta WHERE evidence_id=?1),
                    (SELECT invalidated_commit_seq IS NULL FROM decisions WHERE evidence_id=?1)",
            [&evidence],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(evidence_retired && citer_live);
    assert_eq!(cited_in(&evidence), (false, Vec::new()));
}

/// `evidence_cited_in` agrees with a model of seeded citations for every
/// accepted scope set: the evidence is cited within the set exactly when a
/// live citer's scope is in it, each scope is offered at most once and only
/// when a live row cites the evidence under it, an accepted answer ends at the
/// first accepted scope, and a refused answer offered every citing scope.
/// Citers of one evidence row interleave in row order across scopes, kinds,
/// retirements, and unscoped rows, so a refused scope often precedes an
/// accepted one.
#[test]
fn evidence_cited_in_matches_the_citation_model_under_seeded_citers() {
    use std::collections::BTreeSet;

    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    seed_domain(&store);
    let scopes = ["scope-0", "scope-1", "scope-2", "scope-3"];
    let pool: Vec<String> = (0..6)
        .map(|n| ingest_evidence(&store, &format!("model-{n}"), '1'))
        .collect();
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = move |bound: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        usize::try_from(seed % bound).unwrap()
    };
    // Each live citing row as (evidence index, object id, scope), and each
    // decision's scope, so events and retirements can follow it.
    #[derive(Clone)]
    struct Citer {
        evidence: usize,
        owner: String,
        scope: Option<&'static str>,
    }
    let mut citers: Vec<Citer> = Vec::new();
    let mut decisions: Vec<(String, String, Option<&'static str>)> = Vec::new();
    store
        .commit(intent("model-scopes", '2'), |envelope| {
            for scope_id in scopes {
                envelope.insert_scope(branch_scope(scope_id))?;
            }
            envelope.insert_decision(decision(0))?;
            Ok(String::new())
        })
        .unwrap();
    for round in 0..24_i64 {
        let mut retired: Vec<String> = Vec::new();
        store
            .commit(intent(&format!("model-round-{round}"), '3'), |envelope| {
                for slot in 0..4_i64 {
                    let index = 1 + round * 4 + slot;
                    let scope = [
                        None,
                        Some(scopes[0]),
                        Some(scopes[1]),
                        Some(scopes[2]),
                        Some(scopes[3]),
                    ][next(5)];
                    let evidence = next(pool.len() as u64 + 1);
                    let cited = pool.get(evidence).cloned();
                    match next(3) {
                        0 => {
                            let mut spec = decision(index);
                            spec.scope_id = scope.map(str::to_string);
                            spec.evidence_id = cited.clone();
                            envelope.insert_decision(spec)?;
                            let object_id = format!("decision-object-{index}");
                            if cited.is_some() {
                                citers.push(Citer {
                                    evidence,
                                    owner: object_id.clone(),
                                    scope,
                                });
                            }
                            decisions.push((format!("decision-{index}"), object_id, scope));
                        }
                        1 => {
                            let mut spec = observation(index, "decision-object-0");
                            spec.scope_id = scope.map(str::to_string);
                            spec.evidence_id = cited.clone();
                            envelope.insert_observation(spec)?;
                            if cited.is_some() {
                                citers.push(Citer {
                                    evidence,
                                    owner: format!("observation-object-{index}"),
                                    scope,
                                });
                            }
                        }
                        _ if !decisions.is_empty() && cited.is_some() => {
                            let (decision_id, object_id, scope) =
                                decisions[next(decisions.len() as u64)].clone();
                            envelope.append_decision_event(
                                &decision_id,
                                DecisionEventSpec {
                                    event_kind: "evidence".to_string(),
                                    payload: DecisionEventPayload {
                                        summary: format!("model event {index}"),
                                    },
                                    evidence_id: cited.clone(),
                                    recorded_at: index,
                                },
                            )?;
                            citers.push(Citer {
                                evidence,
                                owner: object_id,
                                scope,
                            });
                        }
                        _ => {}
                    }
                }
                if round % 3 == 2 && !citers.is_empty() {
                    let owner = citers[next(citers.len() as u64)].owner.clone();
                    if owner.starts_with("decision-object-") {
                        envelope.retire_decision(&owner)?;
                        decisions.retain(|(_, object_id, _)| *object_id != owner);
                    } else {
                        envelope.retire_observation(&owner)?;
                    }
                    retired.push(owner);
                }
                Ok(String::new())
            })
            .unwrap();
        citers.retain(|citer| !retired.contains(&citer.owner));
    }

    let deadline = || std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut accepted_answers = 0;
    for (evidence, evidence_id) in pool.iter().enumerate() {
        let citing: BTreeSet<&str> = citers
            .iter()
            .filter(|citer| citer.evidence == evidence)
            .filter_map(|citer| citer.scope)
            .collect();
        for mask in 0..(1_u32 << scopes.len()) {
            let accepted: BTreeSet<&str> = scopes
                .iter()
                .enumerate()
                .filter(|(bit, _)| mask & (1 << bit) != 0)
                .map(|(_, scope)| *scope)
                .collect();
            let (_, (cited, offered)) = store
                .preview(deadline(), |preview| {
                    let mut offered: Vec<String> = Vec::new();
                    let cited = preview.evidence_cited_in(evidence_id, |scope_id| {
                        offered.push(scope_id.to_string());
                        Ok(accepted.contains(scope_id))
                    })?;
                    Ok((cited, offered))
                })
                .unwrap();
            let case = format!("evidence {evidence}, accepted {accepted:?}, offered {offered:?}");
            assert_eq!(cited, !citing.is_disjoint(&accepted), "{case}");
            let distinct: BTreeSet<&str> = offered.iter().map(String::as_str).collect();
            assert_eq!(distinct.len(), offered.len(), "{case}");
            assert!(distinct.is_subset(&citing), "{case}");
            let (last, earlier) = match offered.split_last() {
                Some((last, earlier)) => (Some(last.as_str()), earlier),
                None => (None, &[][..]),
            };
            assert!(
                earlier
                    .iter()
                    .all(|scope| !accepted.contains(scope.as_str())),
                "{case}"
            );
            if cited {
                accepted_answers += 1;
                assert!(last.is_some_and(|scope| accepted.contains(scope)), "{case}");
            } else {
                assert_eq!(distinct, citing, "{case}");
            }
        }
    }
    assert!(accepted_answers > 0);
}

fn accepted_citation_steps(history: i64) -> u64 {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    seed_domain(&store);
    let evidence = ingest_evidence(&store, "history", 'e');
    let citing = |index: i64, scope_id: &str| {
        let mut spec = decision(index);
        spec.scope_id = Some(scope_id.to_string());
        spec.evidence_id = Some(evidence.clone());
        spec
    };
    store
        .commit(intent("history-citers", 'c'), |envelope| {
            for scope_id in ["scope-foreign", "scope-own"] {
                envelope.insert_scope(branch_scope(scope_id))?;
            }
            envelope.insert_decision(decision(0))?;
            for index in 1..=history {
                envelope.insert_decision(citing(index, "scope-own"))?;
                let mut observed = observation(index, "decision-object-0");
                observed.scope_id = Some("scope-own".to_string());
                observed.evidence_id = Some(evidence.clone());
                envelope.insert_observation(observed)?;
                envelope.insert_decision(citing(history + index, "scope-foreign"))?;
            }
            Ok(String::new())
        })
        .unwrap();
    store
        .commit(intent("history-retire", 'f'), |envelope| {
            for index in 1..=history {
                envelope.retire_decision(&format!("decision-object-{index}"))?;
                envelope.retire_observation(&format!("observation-object-{index}"))?;
            }
            envelope.insert_decision(citing(3 * history + 1, "scope-own"))?;
            Ok(String::new())
        })
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let (_, (steps, cited)) = store
        .preview(deadline, |preview| {
            preview.sqlite_steps_for_test(|envelope| {
                envelope.evidence_cited_in(&evidence, |scope_id| Ok(scope_id == "scope-own"))
            })
        })
        .unwrap();
    assert!(cited, "history {history}");
    steps
}

#[test]
fn an_accepted_evidence_check_costs_the_same_however_long_the_citation_history() {
    let short = accepted_citation_steps(1);
    let long = accepted_citation_steps(200);
    assert!(
        long <= short + short / 4,
        "a check over 200 retired and foreign citers took {long} SQLite steps; over 1, {short}"
    );
}

/// The envelope read answers as the reader-side read does at the committed tip the envelope
/// extends: a decision retired before that tip and a decision inserted earlier in the same
/// envelope both read as absent.
#[test]
fn an_envelope_reads_the_decisions_live_at_the_tip_it_extends() {
    let directory = tempfile::tempdir().unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    seed_domain(&store);
    store
        .commit(intent("decisions", '1'), |envelope| {
            envelope.insert_decision(decision(1))?;
            envelope.insert_decision(decision(2))?;
            Ok(String::new())
        })
        .unwrap();
    store
        .commit(intent("retire", '2'), |envelope| {
            Ok(envelope.retire_decision("decision-object-2")?.result_json())
        })
        .unwrap();
    let requested = [
        "decision-object-1".to_string(),
        "decision-object-2".to_string(),
        "decision-object-3".to_string(),
    ];
    let expected = store
        .decisions_for_objects_as_of(&requested, store.tip().unwrap())
        .unwrap();
    assert_eq!(
        expected
            .iter()
            .map(|row| row.object_id.as_str())
            .collect::<Vec<_>>(),
        ["decision-object-1"]
    );
    let mut read = None;
    store
        .commit(intent("read", '3'), |envelope| {
            envelope.insert_decision(decision(3))?;
            read = Some(envelope.decisions_for_objects(&requested)?);
            Ok(String::new())
        })
        .unwrap();
    assert_eq!(read.unwrap(), expected);
}

//! Trusted causality against a real store: every class the reader derives is
//! compared with what the test wrote through the public API or refused at
//! commit, never with another projection of the same reader.

#![cfg(feature = "test-support")]

mod claim_fixture;

use std::num::NonZeroU64;

use claim_fixture::{
    DOMAIN, Fixture, MAX_DETAIL_BYTES, PRODUCER, admission, decision, derived, direct, intent,
    request,
};
use kernel::{
    CLAIM_CAUSALITY_KIND, CausalClass, CausalOperation, ClaimCausalityError,
    DERIVED_FROM_DEPENDENCY_KIND, KernelError, ObservationPayload, ObservationSpec,
    ParentReference, ScopeSpec, Sensitivity, UnknownReason,
};
use rusqlite::Connection;

fn column_text(root: &std::path::Path, sql: &str) -> Option<String> {
    Connection::open_with_flags(
        root.join("kernel.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
    .query_row(sql, [], |row| row.get(0))
    .unwrap()
}

fn parents(list: &[(&str, i64)]) -> Vec<ParentReference> {
    list.iter()
        .map(|(object_id, revision)| ParentReference {
            object_id: object_id.to_string(),
            revision: *revision,
        })
        .collect()
}

#[test]
fn direct_observation_rests_on_live_exact_evidence() {
    let fixture = Fixture::open();
    fixture.admit_decision(1, 1);
    let (evidence_id, digest) = fixture.retain("acquisition", "observed text");
    let (redacted_id, redacted_digest) = fixture.retain_redacted(
        "redacted",
        "token sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678 here",
    );

    for (key, subject, revision, evidence, expected) in [
        (
            "wrong-digest",
            "decision-object-1",
            1,
            direct(&evidence_id, &"0".repeat(64)),
            ClaimCausalityError::ArtifactMismatch,
        ),
        (
            "malformed-digest",
            "decision-object-1",
            1,
            direct(&evidence_id, "not-a-digest"),
            ClaimCausalityError::ArtifactMismatch,
        ),
        (
            "missing-evidence",
            "decision-object-1",
            1,
            direct("evidence-nowhere", &digest),
            ClaimCausalityError::EvidenceMissing,
        ),
        (
            "inexact-evidence",
            "decision-object-1",
            1,
            direct(&redacted_id, &redacted_digest),
            ClaimCausalityError::EvidenceNotExact,
        ),
        (
            "wrong-revision",
            "decision-object-1",
            2,
            direct(&evidence_id, &digest),
            ClaimCausalityError::SubjectRevisionMismatch,
        ),
        (
            "not-a-decision",
            "domain-object",
            1,
            direct(&evidence_id, &digest),
            ClaimCausalityError::SubjectNotDecision,
        ),
    ] {
        let refused = fixture.record(key, request(subject, revision, evidence));
        assert_eq!(refused, Err(expected), "{key}");
    }
    let before = fixture.tip();

    let recorded = fixture
        .record(
            "direct",
            request("decision-object-1", 1, direct(&evidence_id, &digest)),
        )
        .unwrap();
    assert_eq!(recorded.replaced_object_id, None);
    assert_eq!(recorded.operation, CausalOperation::Insert);
    let recorded_at = fixture.tip();
    assert_eq!(recorded_at, before + 1, "refusals landed no commit");
    let reading = fixture.reading("decision-object-1", recorded_at);
    assert_eq!(
        reading.class,
        CausalClass::DirectObservation {
            acquisition_evidence_id: evidence_id.clone(),
            artifact_digest: digest.clone(),
        }
    );
    assert_eq!(reading.tip, recorded_at);
    let record = reading.record.expect("record summary");
    assert_eq!(record.producer, PRODUCER);
    assert_eq!(record.operation, Some(CausalOperation::Insert));
    assert_eq!(record.object_id, recorded.object_id);
    assert_eq!(record.created_commit_seq, recorded_at);

    // The record cites the evidence, so retirement is refused while it is live.
    let retire = fixture.store.commit(intent("retire-evidence"), |envelope| {
        envelope.retire_evidence("evidence-object-acquisition")?;
        Ok(String::new())
    });
    assert_eq!(retire, Err(KernelError::Conflict));

    // Retiring the record withdraws the class from later snapshots only.
    fixture
        .store
        .commit(intent("retire-record"), |envelope| {
            envelope.retire_observation(&recorded.object_id)?;
            Ok(String::new())
        })
        .unwrap();
    assert_eq!(
        fixture.class("decision-object-1", fixture.tip()),
        CausalClass::Unknown(UnknownReason::NoRecord)
    );
    assert_eq!(
        fixture.class("decision-object-1", recorded_at),
        CausalClass::DirectObservation {
            acquisition_evidence_id: evidence_id.clone(),
            artifact_digest: digest.clone(),
        }
    );

    // With the record gone the evidence can retire; the earlier snapshot still
    // reads its own live evidence.
    fixture
        .store
        .commit(intent("retire-evidence-again"), |envelope| {
            envelope.retire_evidence("evidence-object-acquisition")?;
            Ok(String::new())
        })
        .unwrap();
    assert!(!fixture.class("decision-object-1", recorded_at).is_unknown());

    // A retired decision is no longer a subject.
    fixture
        .store
        .commit(intent("retire-decision"), |envelope| {
            envelope.retire_decision("decision-object-1")?;
            Ok(String::new())
        })
        .unwrap();
    assert_eq!(
        fixture.record(
            "retired-subject",
            request("decision-object-1", 1, direct(&evidence_id, &digest))
        ),
        Err(ClaimCausalityError::SubjectNotDecision)
    );
}

#[test]
fn records_inherit_the_subjects_scope_and_sensitivity() {
    let fixture = Fixture::open();
    fixture
        .store
        .commit(intent("scope"), |envelope| {
            envelope.insert_scope(ScopeSpec {
                scope_id: "scope-1".to_string(),
                object_id: "scope-object-1".to_string(),
                domain_id: DOMAIN.to_string(),
                source_kind: "fixture".to_string(),
                source_id: "scope".to_string(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
                terms: Vec::new(),
            })?;
            let mut scoped = decision(1, 1);
            scoped.scope_id = Some("scope-1".to_string());
            scoped.sensitivity = Sensitivity::Sensitive;
            envelope.insert_decision(scoped)?;
            envelope.record_admission(admission("decision-object-1"))?;
            Ok(String::new())
        })
        .unwrap();
    let recorded = fixture
        .record(
            "derived",
            request("decision-object-1", 1, derived(&[("domain-object", 1)])),
        )
        .unwrap();
    let scope = column_text(
        fixture.root.path(),
        &format!(
            "SELECT scope_id FROM observations WHERE observation_id='{}'",
            recorded.observation_id
        ),
    );
    assert_eq!(scope.as_deref(), Some("scope-1"));
    let sensitivity = column_text(
        fixture.root.path(),
        &format!(
            "SELECT sensitivity_class FROM object_registry WHERE object_id='{}'",
            recorded.object_id
        ),
    );
    assert_eq!(sensitivity.as_deref(), Some("sensitive"));
}

#[test]
fn derived_reinjection_rests_on_exact_live_parents() {
    let fixture = Fixture::open();
    fixture.admit_decision(1, 1);
    fixture.admit_decision(2, 1);
    let subject = "decision-object-1";
    for (key, evidence, expected) in [
        ("no-parents", derived(&[]), ClaimCausalityError::NoParents),
        (
            "self-parent",
            derived(&[(subject, 1)]),
            ClaimCausalityError::ParentIsSubject,
        ),
        (
            "duplicate",
            derived(&[("decision-object-2", 1), ("decision-object-2", 1)]),
            ClaimCausalityError::DuplicateParent,
        ),
        (
            "missing-parent",
            derived(&[("decision-object-9", 1)]),
            ClaimCausalityError::ParentMissing,
        ),
        (
            "parent-revision",
            derived(&[("decision-object-2", 4)]),
            ClaimCausalityError::ParentRevisionMismatch,
        ),
    ] {
        let refused = fixture.record(key, request(subject, 1, evidence));
        assert_eq!(refused, Err(expected), "{key}");
    }
    let too_many: Vec<(String, i64)> = (0..17)
        .map(|index| (format!("decision-object-{}", index + 10), 1))
        .collect();
    let too_many: Vec<(&str, i64)> = too_many
        .iter()
        .map(|(id, revision)| (id.as_str(), *revision))
        .collect();
    assert_eq!(
        fixture.record("too-many", request(subject, 1, derived(&too_many))),
        Err(ClaimCausalityError::TooManyParents)
    );

    let recorded = fixture
        .record(
            "derived",
            request(
                subject,
                1,
                derived(&[("decision-object-2", 1), ("domain-object", 1)]),
            ),
        )
        .unwrap();
    let recorded_at = fixture.tip();
    let expected = CausalClass::DerivedReinjection {
        parents: parents(&[("decision-object-2", 1), ("domain-object", 1)]),
    };
    assert_eq!(fixture.class(subject, recorded_at), expected);
    assert_eq!(
        fixture
            .store
            .observation_dependency_targets_for_test(
                &recorded.observation_id,
                DERIVED_FROM_DEPENDENCY_KIND
            )
            .unwrap(),
        ["decision-object-2", "domain-object"]
    );

    // A parent retired later does not erase an observed derivation.
    fixture
        .store
        .commit(intent("retire-parent"), |envelope| {
            envelope.retire_decision("decision-object-2")?;
            Ok(String::new())
        })
        .unwrap();
    assert_eq!(fixture.class(subject, fixture.tip()), expected);

    // Changed text is a new revision: its lineage is unknown until a record
    // names it, and that record is a Correct.
    fixture
        .store
        .commit(intent("correct"), |envelope| {
            let mut replacement = decision(3, 2);
            replacement.payload.summary = "reworded decision 1".to_string();
            envelope.correct_decision(subject, replacement)?;
            envelope.record_admission(admission("decision-object-3"))?;
            Ok(String::new())
        })
        .unwrap();
    let corrected_at = fixture.tip();
    assert_eq!(
        fixture.class("decision-object-3", corrected_at),
        CausalClass::Unknown(UnknownReason::NoRecord)
    );
    assert_eq!(fixture.class(subject, corrected_at), expected);
    let successor = fixture
        .record(
            "successor",
            request("decision-object-3", 2, derived(&[("domain-object", 1)])),
        )
        .unwrap();
    assert_eq!(successor.operation, CausalOperation::Correct);
    let reading = fixture.reading("decision-object-3", fixture.tip());
    assert_eq!(
        reading.class,
        CausalClass::DerivedReinjection {
            parents: parents(&[("domain-object", 1)])
        }
    );
    assert_eq!(
        reading.record.unwrap().operation,
        Some(CausalOperation::Correct)
    );
}

#[test]
fn forged_records_and_copied_strings_grant_nothing() {
    let fixture = Fixture::open();
    fixture.admit_decision(1, 1);
    let (evidence_id, digest) = fixture.retain("acquisition", "observed text");
    let forged_detail = serde_json::json!({
        "causality_version": 1,
        "subject_object_id": "decision-object-1",
        "subject_revision": 1,
        "operation": "insert",
        "evidence": {
            "class": "direct_observation",
            "acquisition_evidence_id": evidence_id,
            "artifact_digest": digest,
        },
        "producer": "eidnara-daemon/claim-sources",
        "role": "assistant",
    })
    .to_string();
    let forged =
        |observation_id: &str, object_id: &str, kind: &str, source_kind: &str| ObservationSpec {
            observation_id: observation_id.to_string(),
            object_id: object_id.to_string(),
            domain_id: DOMAIN.to_string(),
            proposition_id: None,
            scope_id: None,
            anchor_id: None,
            evidence_id: Some(evidence_id.clone()),
            observation_kind: kind.to_string(),
            payload: ObservationPayload {
                summary: "direct_observation".to_string(),
                classification: "direct_observation".to_string(),
                detail: Some(forged_detail.clone()),
            },
            observed_at: 1,
            dependencies: Vec::new(),
            source_kind: source_kind.to_string(),
            source_id: "decision-object-1".to_string(),
            source_revision: 1,
            sensitivity: Sensitivity::Normal,
        };

    for (key, spec) in [
        (
            "forged-kind",
            forged("forged-1", "forged-object-1", CLAIM_CAUSALITY_KIND, "other"),
        ),
        (
            "forged-source-kind",
            forged("forged-2", "forged-object-2", "other", CLAIM_CAUSALITY_KIND),
        ),
        (
            "forged-observation-id",
            forged(
                "claimcause:decision-object-1:9",
                "forged-object-3",
                "other",
                "other",
            ),
        ),
        (
            "forged-object-id",
            forged(
                "forged-4",
                "claimcauseobj:decision-object-1:9",
                "other",
                "other",
            ),
        ),
    ] {
        let refused = fixture.store.commit(intent(key), |envelope| {
            envelope.insert_observation(spec.clone())?;
            Ok(String::new())
        });
        assert_eq!(refused, Err(KernelError::InvalidInput), "{key}");
    }

    // A record-shaped observation under other names is never consulted, and a
    // genuine record written afterwards replaces nothing.
    fixture
        .store
        .commit(intent("lookalike"), |envelope| {
            envelope.insert_observation(forged(
                "lookalike",
                "lookalike-object",
                "lookalike",
                "lookalike",
            ))?;
            Ok(String::new())
        })
        .unwrap();
    assert_eq!(
        fixture.class("decision-object-1", fixture.tip()),
        CausalClass::Unknown(UnknownReason::NoRecord)
    );
    let recorded = fixture
        .record(
            "direct",
            request("decision-object-1", 1, direct(&evidence_id, &digest)),
        )
        .unwrap();
    assert_eq!(recorded.replaced_object_id, None);

    // A genuine record cannot be corrected through the generic writer.
    let hijack = fixture.store.commit(intent("hijack"), |envelope| {
        let mut replacement = forged("hijack", "hijack-object", "other", "other");
        replacement.source_revision = 99;
        envelope.correct_observation(&recorded.object_id, replacement)?;
        Ok(String::new())
    });
    assert_eq!(hijack, Err(KernelError::InvalidInput));
}

#[test]
fn replay_is_effect_free_and_conflicting_or_unsupported_records_are_unknown() {
    let fixture = Fixture::open();
    fixture.admit_decision(1, 1);
    let (evidence_id, digest) = fixture.retain("acquisition", "observed text");
    let direct_request = request("decision-object-1", 1, direct(&evidence_id, &digest));
    let first = fixture.record("direct", direct_request.clone()).unwrap();
    let first_at = fixture.tip();
    let replay = fixture
        .store
        .commit(intent("direct"), |_| panic!("replay must not execute"))
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.commit_seq, first_at);
    let direct_class = CausalClass::DirectObservation {
        acquisition_evidence_id: evidence_id.clone(),
        artifact_digest: digest.clone(),
    };
    let derived_class = CausalClass::DerivedReinjection {
        parents: parents(&[("domain-object", 1)]),
    };

    // A later record for the same subject replaces the first; old snapshots keep the old class.
    let second = fixture
        .record(
            "derived",
            request("decision-object-1", 1, derived(&[("domain-object", 1)])),
        )
        .unwrap();
    assert_eq!(
        second.replaced_object_id.as_deref(),
        Some(first.object_id.as_str())
    );
    assert_eq!(
        fixture.class("decision-object-1", fixture.tip()),
        derived_class
    );
    assert_eq!(fixture.class("decision-object-1", first_at), direct_class);

    // Two records for one subject in one commit are refused.
    let twice = fixture.store.commit(intent("twice"), |envelope| {
        envelope.record_claim_causality(&direct_request).unwrap();
        let again = envelope.record_claim_causality(&direct_request);
        assert_eq!(again, Err(ClaimCausalityError::DuplicateSubject));
        Ok(String::new())
    });
    assert_eq!(twice, Err(KernelError::InvalidInput));

    // Out-of-band edits to the one unguarded column never grant a class.
    let payload_of = |id: &str| {
        format!(
            "SELECT CAST(observation_payload AS TEXT) FROM observations WHERE observation_id='{id}'"
        )
    };
    let original = column_text(fixture.root.path(), &payload_of(&second.observation_id)).unwrap();
    let rewrite = |from: &str, to: &str| {
        fixture.sql(&format!(
            "UPDATE observations SET observation_payload=CAST('{}' AS BLOB)
             WHERE observation_id='{}';",
            original.replace(from, to).replace('\'', "''"),
            second.observation_id
        ));
    };
    rewrite("causality_version\\\":1", "causality_version\\\":2");
    assert_eq!(
        fixture.class("decision-object-1", fixture.tip()),
        CausalClass::Unknown(UnknownReason::UnsupportedVersion)
    );
    rewrite("subject_revision\\\":1", "subject_revision\\\":7");
    assert_eq!(
        fixture.class("decision-object-1", fixture.tip()),
        CausalClass::Unknown(UnknownReason::SubjectMismatch)
    );
    rewrite("domain-object", "decision-object-1");
    assert_eq!(
        fixture.class("decision-object-1", fixture.tip()),
        CausalClass::Unknown(UnknownReason::Malformed),
        "detail parents that disagree with the derived_from rows"
    );
    fixture.sql(&format!(
        "UPDATE observations SET observation_payload=X'7b7d' WHERE observation_id='{}';",
        second.observation_id
    ));
    let reading = fixture.reading("decision-object-1", fixture.tip());
    assert_eq!(
        reading.class,
        CausalClass::Unknown(UnknownReason::Malformed)
    );
    assert_eq!(
        reading.record.as_ref().map(|record| record.operation),
        Some(None),
        "the row identity is reported even when its detail does not decode"
    );

    // Evidence invalidated behind a live direct record yields Unknown at
    // snapshots after the invalidation only.
    fixture.sql(&format!(
        "UPDATE evidence_meta SET invalidated_commit_seq={} WHERE evidence_id='{evidence_id}';",
        first_at + 1
    ));
    assert_eq!(fixture.class("decision-object-1", first_at), direct_class);
    fixture.sql(&format!(
        "UPDATE evidence_meta SET invalidated_commit_seq={first_at} WHERE evidence_id='{evidence_id}';"
    ));
    assert_eq!(
        fixture.class("decision-object-1", first_at),
        CausalClass::Unknown(UnknownReason::EvidenceUnavailable)
    );

    // A second live record for one subject makes its class Unknown.
    fixture.sql(&format!(
        "INSERT INTO object_registry(object_id,object_kind,domain_id,source_kind,source_id,
             source_revision,created_commit_seq,sensitivity_class)
         VALUES('claimcauseobj:decision-object-1:oob','observation','{DOMAIN}',
                '{CLAIM_CAUSALITY_KIND}','decision-object-1',999,{tip},'normal');
         INSERT INTO observations(observation_id,object_id,observation_kind,observation_payload,
             observed_at,created_commit_seq,sensitivity_class)
         SELECT 'claimcause:decision-object-1:oob','claimcauseobj:decision-object-1:oob',
                observation_kind,observation_payload,observed_at,{tip},sensitivity_class
         FROM observations WHERE observation_id='{}';",
        second.observation_id,
        tip = fixture.tip(),
    ));
    let reading = fixture.reading("decision-object-1", fixture.tip());
    assert_eq!(
        reading.class,
        CausalClass::Unknown(UnknownReason::Conflicting)
    );
    assert_eq!(reading.record, None);
}

#[test]
fn oversized_payloads_read_as_unknown_and_records_survive_reopen() {
    let fixture = Fixture::open();
    fixture.admit_decision(1, 1);
    let (evidence_id, digest) = fixture.retain("acquisition", "observed text");
    let recorded = fixture
        .record(
            "direct",
            request("decision-object-1", 1, direct(&evidence_id, &digest)),
        )
        .unwrap();
    let tip = fixture.tip();
    let tight = NonZeroU64::new(8).unwrap();
    let reading = fixture
        .store
        .causal_class_as_of("decision-object-1", tip, tight)
        .unwrap();
    assert_eq!(
        reading.class,
        CausalClass::Unknown(UnknownReason::Oversized)
    );
    assert_eq!(
        reading
            .record
            .as_ref()
            .map(|record| record.object_id.as_str()),
        Some(recorded.object_id.as_str())
    );
    assert_eq!(
        fixture
            .store
            .causal_class_as_of("decision-object-9", tip, MAX_DETAIL_BYTES),
        Err(KernelError::NotFound)
    );
    assert_eq!(
        fixture
            .store
            .causal_class_as_of("decision-object-1", tip + 1, MAX_DETAIL_BYTES),
        Err(KernelError::FutureSnapshot)
    );

    let before = fixture.reading("decision-object-1", tip);
    let fixture = fixture.reopen();
    let after = fixture.reading("decision-object-1", tip);
    assert_eq!(after, before);
    assert_eq!(
        after.class,
        CausalClass::DirectObservation {
            acquisition_evidence_id: evidence_id,
            artifact_digest: digest,
        }
    );
}

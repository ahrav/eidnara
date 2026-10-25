#![cfg(feature = "test-support")]
//! `Envelope::insert_anchor` proofs: every committed anchor row decodes with
//! the engine's decoder and shares its envelope with its registry row, and a
//! refused anchor keeps both tables at their prior rows.

#[path = "support/applicability_fixtures.rs"]
mod applicability_fixtures;
#[path = "support/git_fixtures.rs"]
mod git_fixtures;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use applicability_fixtures::{candidate, checkout};
use git_fixtures::{commit_snapshot, init_repo};
use kernel::applicability::{
    ApplicabilityCandidate, ApplicabilityEngine, ApplicabilityState, EvalBudget,
    MAX_OBJECT_PAYLOAD_BYTES,
};
use kernel::{
    ANCHOR_INSERT_KIND, AnchorCapture, AnchorCondition, AnchorDecodeError, AnchorKind,
    AnchorRowSpec, AnchorSpec, AnchorValidationError, AnchorValue, CommitIntent, DecisionPayload,
    DecisionSpec, DomainSpec, GitCondition, KernelError, KernelStore, PatchIdCapture, Sensitivity,
    validate_anchor, validate_anchor_row,
};
use kernel::{QueryContext, ScopeMatchContext};
use rusqlite::{Connection, OpenFlags, OptionalExtension};

const SECRET: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678";

fn oid(byte: u8) -> String {
    format!("{byte:02x}").repeat(20)
}

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "kernel-anchor-write-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:0>64}", key.len()),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(5)
}

fn seeded_store(root: &std::path::Path) -> KernelStore {
    let store = KernelStore::open(root).unwrap();
    store
        .commit_before(deadline(), intent("domain"), |envelope| {
            envelope.insert_domain(DomainSpec {
                domain_id: "domain".to_string(),
                object_id: "domain-object".to_string(),
                name: "fixture".to_string(),
                source_kind: "fixture".to_string(),
                source_id: "domain".to_string(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
            })?;
            Ok(String::new())
        })
        .unwrap();
    store
}

fn capture(byte: u8) -> AnchorCapture {
    AnchorCapture {
        commit_oid: oid(byte),
        tree_oid: Some(oid(byte.wrapping_add(1))),
        patch_id: Some(PatchIdCapture {
            algorithm: "eidnara.patch-id.v1".to_string(),
            value: oid(byte.wrapping_add(2)),
        }),
        changed_paths: vec!["src/lib.rs".to_string()],
    }
}

fn spec(name: &str, value: AnchorValue) -> AnchorSpec {
    AnchorSpec {
        anchor_id: format!("anchor-{name}"),
        object_id: format!("anchor-object-{name}"),
        domain_id: "domain".to_string(),
        source_kind: "fixture".to_string(),
        source_id: "anchor".to_string(),
        source_revision: 1,
        sensitivity: Sensitivity::Normal,
        value,
    }
}

fn decision(name: &str, anchor_id: &str) -> DecisionSpec {
    DecisionSpec {
        decision_id: format!("decision-{name}"),
        object_id: format!("decision-object-{name}"),
        domain_id: "domain".to_string(),
        proposition_id: None,
        scope_id: None,
        anchor_id: Some(anchor_id.to_string()),
        evidence_id: None,
        decision_kind: "architecture".to_string(),
        payload: DecisionPayload {
            summary: format!("decision {name}"),
            rationale: "anchored".to_string(),
        },
        source_kind: "fixture".to_string(),
        source_id: "decision".to_string(),
        source_revision: 1,
        sensitivity: Sensitivity::Normal,
    }
}

/// One valid typed value per anchor kind, in `AnchorKind::ALL` order, with the
/// condition written out from the same inputs.
fn every_kind() -> Vec<(&'static str, AnchorValue, AnchorCondition)> {
    let captures = |captures: &[AnchorCapture]| {
        captures
            .iter()
            .map(|capture| (capture.commit_oid.clone(), capture.clone()))
            .collect::<BTreeMap<_, _>>()
    };
    vec![
        (
            "exact",
            AnchorValue::Exact {
                value: "pinned-token".to_string(),
            },
            AnchorCondition::Exact {
                value: "pinned-token".to_string(),
            },
        ),
        (
            "reachable-from",
            AnchorValue::ReachableFrom {
                oid: oid(0x1a),
                captures: vec![capture(0x1a)],
            },
            AnchorCondition::Git(GitCondition::ReachableFrom {
                oid: oid(0x1a),
                captures: captures(&[capture(0x1a)]),
            }),
        ),
        (
            "reachable-between",
            AnchorValue::ReachableBetween {
                start_oid: oid(0x2a),
                end_oid: oid(0x3a),
                captures: vec![capture(0x3a), capture(0x2a)],
            },
            AnchorCondition::Git(GitCondition::ReachableBetween {
                start_oid: oid(0x2a),
                end_oid: oid(0x3a),
                captures: captures(&[capture(0x2a), capture(0x3a)]),
            }),
        ),
        (
            "deployment",
            AnchorValue::DeploymentRevision {
                revision: "deploy-42".to_string(),
            },
            AnchorCondition::DeploymentRevision {
                revision: "deploy-42".to_string(),
            },
        ),
        (
            "config",
            AnchorValue::ConfigRevision {
                revision: "config-7".to_string(),
            },
            AnchorCondition::ConfigRevision {
                revision: "config-7".to_string(),
            },
        ),
        (
            "platform",
            AnchorValue::PlatformVersion {
                range: ">=1.2, <2".to_string(),
            },
            AnchorCondition::PlatformVersion {
                req: semver::VersionReq::parse(">=1.2, <2").unwrap(),
            },
        ),
        (
            "interval",
            AnchorValue::WallClockInterval {
                start_ms: 1_000,
                end_ms: 2_000,
            },
            AnchorCondition::WallClockInterval {
                start_ms: 1_000,
                end_ms: 2_000,
            },
        ),
    ]
}

fn read_only(root: &std::path::Path) -> Connection {
    Connection::open_with_flags(root.join("kernel.sqlite"), OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
}

fn stored_row(connection: &Connection, anchor_id: &str) -> Option<AnchorRowSpec> {
    connection
        .query_row(
            "SELECT anchor_id,anchor_kind,exact_value,reachable_from_oid,
                    reachable_between_start_oid,reachable_between_end_oid,deployment_revision,
                    config_revision,platform_version_range,wall_clock_start,wall_clock_end,payload
             FROM anchors WHERE anchor_id=?1",
            [anchor_id],
            |row| {
                Ok(AnchorRowSpec {
                    anchor_id: row.get(0)?,
                    anchor_kind: row.get(1)?,
                    exact_value: row.get(2)?,
                    reachable_from_oid: row.get(3)?,
                    reachable_between_start_oid: row.get(4)?,
                    reachable_between_end_oid: row.get(5)?,
                    deployment_revision: row.get(6)?,
                    config_revision: row.get(7)?,
                    platform_version_range: row.get(8)?,
                    wall_clock_start: row.get(9)?,
                    wall_clock_end: row.get(10)?,
                    payload: row.get(11)?,
                })
            },
        )
        .optional()
        .unwrap()
}

fn count(connection: &Connection, sql: &str, id: &str) -> i64 {
    connection.query_row(sql, [id], |row| row.get(0)).unwrap()
}

fn assert_nothing_written(root: &std::path::Path, spec: &AnchorSpec) {
    let connection = read_only(root);
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM object_registry WHERE object_id=?1",
            &spec.object_id
        ),
        0
    );
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM anchors WHERE anchor_id=?1",
            &spec.anchor_id
        ),
        0
    );
}

fn assert_anchor_rows_well_formed(root: &std::path::Path) {
    let connection = read_only(root);
    let ids: Vec<String> = connection
        .prepare("SELECT anchor_id FROM anchors")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for id in ids {
        let row = stored_row(&connection, &id).unwrap();
        let condition =
            validate_anchor_row(&row).expect("every stored anchor row passes the engine's checks");
        if let AnchorCondition::Git(git) = condition {
            let (anchored, captures) = match &git {
                GitCondition::ReachableFrom { oid, captures } => (vec![oid], captures),
                GitCondition::ReachableBetween {
                    start_oid,
                    end_oid,
                    captures,
                } => (vec![start_oid, end_oid], captures),
            };
            assert!(
                captures.keys().all(|commit| anchored.contains(&commit)),
                "anchor {id} stores a capture of a commit it does not name"
            );
        }
        let orphaned: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM anchors a
                 LEFT JOIN object_registry r
                   ON r.object_id=a.object_id AND r.object_kind='anchor'
                  AND r.created_commit_seq=a.created_commit_seq
                 WHERE a.anchor_id=?1 AND r.object_id IS NULL",
                [&id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(orphaned, 0, "anchor {id} has its registry row");
    }
    let early: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM decisions d JOIN anchors a ON a.anchor_id=d.anchor_id
             WHERE a.created_commit_seq > d.created_commit_seq",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        early, 0,
        "every decision names an anchor committed at or before it"
    );
}

#[test]
fn every_anchor_kind_commits_a_row_the_engine_decodes_and_a_decision_can_reference() {
    let directory = tempfile::tempdir().unwrap();
    let store = seeded_store(directory.path());
    for (name, value, expected) in every_kind() {
        let mut spec = spec(name, value.clone());
        if name == "deployment" {
            spec.sensitivity = Sensitivity::Sensitive;
        }
        assert_eq!(
            validate_anchor_row(&validate_anchor(&spec).unwrap()),
            Ok(expected.clone())
        );
        let anchor_id = spec.anchor_id.clone();
        let receipt = store
            .commit_before(deadline(), intent(name), |envelope| {
                let outcome = envelope.insert_anchor(spec.clone())?;
                assert_eq!(outcome.anchor_id, spec.anchor_id);
                assert_eq!(outcome.object_id, spec.object_id);
                envelope.insert_decision(decision(name, &outcome.anchor_id))?;
                Ok(outcome.result_json())
            })
            .unwrap();
        assert!(receipt.result.contains(&anchor_id));

        let connection = read_only(directory.path());
        let row = stored_row(&connection, &anchor_id).expect("anchor row committed");
        assert_eq!(
            row,
            validate_anchor(&spec).unwrap(),
            "{name}: the stored row is the validated row"
        );
        let decoded = AnchorCondition::decode(&row).expect("stored anchor decodes");
        assert_eq!(
            decoded, expected,
            "{name}: stored row decodes to the request"
        );
        assert_eq!(row.anchor_kind, value.kind().as_str());
        let (object_kind, sensitivity, created): (String, String, i64) = connection
            .query_row(
                "SELECT r.object_kind,a.sensitivity_class,r.created_commit_seq
                 FROM anchors a JOIN object_registry r ON r.object_id=a.object_id
                 WHERE a.anchor_id=?1 AND a.created_commit_seq=r.created_commit_seq",
                [&anchor_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(object_kind, "anchor");
        assert_eq!(sensitivity, spec.sensitivity.as_str());
        assert_eq!(created, receipt.commit_seq);
        let (change_kind, outbox_kind): (String, String) = connection
            .query_row(
                "SELECT e.change_kind,o.object_kind FROM change_event e
                 JOIN outbox o ON o.commit_seq=e.commit_seq AND o.ordinal=e.ordinal
                 WHERE e.object_id=?1",
                [&spec.object_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(change_kind, ANCHOR_INSERT_KIND);
        assert_eq!(outbox_kind, "anchor");
    }
    assert_eq!(
        every_kind()
            .iter()
            .map(|(_, value, _)| value.kind())
            .collect::<Vec<_>>(),
        AnchorKind::ALL
    );
    assert_anchor_rows_well_formed(directory.path());
}

#[test]
fn git_anchors_with_an_empty_capture_list_store_a_null_payload() {
    let directory = tempfile::tempdir().unwrap();
    let store = seeded_store(directory.path());
    let spec = spec(
        "bare",
        AnchorValue::ReachableFrom {
            oid: oid(0x4b),
            captures: Vec::new(),
        },
    );
    store
        .commit_before(deadline(), intent("bare"), |envelope| {
            Ok(envelope.insert_anchor(spec.clone())?.result_json())
        })
        .unwrap();
    let row = stored_row(&read_only(directory.path()), &spec.anchor_id).unwrap();
    assert_eq!(row.payload, None);
    assert_eq!(row.reachable_from_oid, Some(oid(0x4b)));
}

#[test]
fn a_decision_inserted_before_its_anchor_is_not_found_and_the_commit_rolls_back() {
    let directory = tempfile::tempdir().unwrap();
    let store = seeded_store(directory.path());
    let spec = spec(
        "late",
        AnchorValue::ReachableFrom {
            oid: oid(0x5c),
            captures: Vec::new(),
        },
    );
    let refused = store
        .commit_before(deadline(), intent("late"), |envelope| {
            envelope.insert_decision(decision("late", &spec.anchor_id))?;
            envelope.insert_anchor(spec.clone())?;
            Ok(String::new())
        })
        .unwrap_err();
    assert_eq!(refused, KernelError::NotFound);
    assert_nothing_written(directory.path(), &spec);
    assert_eq!(
        count(
            &read_only(directory.path()),
            "SELECT COUNT(*) FROM decisions WHERE decision_id=?1",
            "decision-late"
        ),
        0
    );
}

#[test]
fn a_decision_failure_after_the_anchor_rolls_back_both() {
    let directory = tempfile::tempdir().unwrap();
    let store = seeded_store(directory.path());
    let spec = spec(
        "rollback",
        AnchorValue::ReachableFrom {
            oid: oid(0x6d),
            captures: Vec::new(),
        },
    );
    let refused = store
        .commit_before(deadline(), intent("rollback"), |envelope| {
            let outcome = envelope.insert_anchor(spec.clone())?;
            let mut orphan = decision("rollback", &outcome.anchor_id);
            orphan.domain_id = "missing".to_string();
            envelope.insert_decision(orphan)?;
            Ok(String::new())
        })
        .unwrap_err();
    assert_eq!(refused, KernelError::NotFound);
    assert_nothing_written(directory.path(), &spec);
    assert_anchor_rows_well_formed(directory.path());
}

#[test]
fn invalid_anchors_are_refused_as_invalid_input_with_a_typed_cause() {
    let directory = tempfile::tempdir().unwrap();
    let store = seeded_store(directory.path());
    let oversized = vec![AnchorCapture {
        changed_paths: vec!["p".repeat(MAX_OBJECT_PAYLOAD_BYTES)],
        ..capture(0x7e)
    }];
    let reachable = |oid: String| AnchorValue::ReachableFrom {
        oid,
        captures: Vec::new(),
    };
    let mut cases = vec![
        (
            spec("uppercase", reachable(oid(0xab).to_uppercase())),
            AnchorValidationError::Decode(AnchorDecodeError::InvalidOid(AnchorKind::ReachableFrom)),
        ),
        (
            spec("short", reachable(oid(0xab)[..39].to_string())),
            AnchorValidationError::Decode(AnchorDecodeError::InvalidOid(AnchorKind::ReachableFrom)),
        ),
        (
            spec(
                "oversized",
                AnchorValue::ReachableFrom {
                    oid: oid(0x7e),
                    captures: oversized,
                },
            ),
            AnchorValidationError::ExceedsBounds,
        ),
        (
            spec(
                "inverted",
                AnchorValue::WallClockInterval {
                    start_ms: 2_000,
                    end_ms: 2_000,
                },
            ),
            AnchorValidationError::Decode(AnchorDecodeError::InvalidInterval),
        ),
        (
            spec(
                "empty-value",
                AnchorValue::Exact {
                    value: String::new(),
                },
            ),
            AnchorValidationError::Decode(AnchorDecodeError::MissingValue(AnchorKind::Exact)),
        ),
        (
            spec(
                "range",
                AnchorValue::PlatformVersion {
                    range: "not a range".to_string(),
                },
            ),
            AnchorValidationError::Decode(AnchorDecodeError::InvalidVersionRange),
        ),
        (
            spec(
                "duplicate-capture",
                AnchorValue::ReachableFrom {
                    oid: oid(0x7f),
                    captures: vec![capture(0x7f), capture(0x7f)],
                },
            ),
            AnchorValidationError::Decode(AnchorDecodeError::DuplicateCapture),
        ),
        (
            spec(
                "unanchored-capture",
                AnchorValue::ReachableFrom {
                    oid: oid(0x21),
                    captures: vec![capture(0x22)],
                },
            ),
            AnchorValidationError::UnanchoredCapture,
        ),
        (
            spec(
                "unanchored-between-capture",
                AnchorValue::ReachableBetween {
                    start_oid: oid(0x23),
                    end_oid: oid(0x24),
                    captures: vec![capture(0x23), capture(0x25)],
                },
            ),
            AnchorValidationError::UnanchoredCapture,
        ),
        (
            spec(
                "secret-capture",
                AnchorValue::ReachableFrom {
                    oid: oid(0x26),
                    captures: vec![AnchorCapture {
                        changed_paths: vec![SECRET.to_string()],
                        ..capture(0x26)
                    }],
                },
            ),
            AnchorValidationError::SecretValue,
        ),
        (
            spec(
                "secret-capture-without-patch-id",
                AnchorValue::ReachableFrom {
                    oid: oid(0x27),
                    captures: vec![AnchorCapture {
                        patch_id: None,
                        changed_paths: vec![SECRET.to_string()],
                        ..capture(0x27)
                    }],
                },
            ),
            AnchorValidationError::SecretValue,
        ),
        (
            spec(
                "secret-capture-without-tree",
                AnchorValue::ReachableBetween {
                    start_oid: oid(0x28),
                    end_oid: oid(0x29),
                    captures: vec![AnchorCapture {
                        tree_oid: None,
                        changed_paths: vec!["src/lib.rs".to_string(), SECRET.to_string()],
                        ..capture(0x29)
                    }],
                },
            ),
            AnchorValidationError::SecretValue,
        ),
        (
            spec(
                "secret-tree-oid",
                AnchorValue::ReachableFrom {
                    oid: oid(0x2b),
                    captures: vec![AnchorCapture {
                        tree_oid: Some(SECRET.to_string()),
                        patch_id: None,
                        ..capture(0x2b)
                    }],
                },
            ),
            AnchorValidationError::SecretValue,
        ),
        (
            spec(
                "secret-patch-id",
                AnchorValue::ReachableFrom {
                    oid: oid(0x2c),
                    captures: vec![AnchorCapture {
                        tree_oid: None,
                        patch_id: Some(PatchIdCapture {
                            algorithm: "eidnara.patch-id.v1".to_string(),
                            value: SECRET.to_string(),
                        }),
                        ..capture(0x2c)
                    }],
                },
            ),
            AnchorValidationError::SecretValue,
        ),
        (
            spec(
                "escaped-secret-capture",
                AnchorValue::ReachableFrom {
                    oid: oid(0x2d),
                    captures: vec![AnchorCapture {
                        changed_paths: vec!["password\t=hunter2hunter2hunter2".to_string()],
                        ..capture(0x2d)
                    }],
                },
            ),
            AnchorValidationError::SecretValue,
        ),
        (
            spec(
                "secret-value",
                AnchorValue::DeploymentRevision {
                    revision: format!("deploy {SECRET}"),
                },
            ),
            AnchorValidationError::SecretValue,
        ),
    ];
    let valid = || spec("identity", reachable(oid(0x11)));
    let mut negative = valid();
    negative.source_revision = -1;
    cases.push((negative, AnchorValidationError::InvalidIdentity));
    let mut blank = valid();
    blank.source_id = " ".to_string();
    cases.push((blank, AnchorValidationError::InvalidIdentity));
    let mut empty_anchor = valid();
    empty_anchor.anchor_id = String::new();
    cases.push((empty_anchor, AnchorValidationError::InvalidIdentity));
    let mut secret_identity = valid();
    secret_identity.object_id = SECRET.to_string();
    cases.push((secret_identity, AnchorValidationError::InvalidIdentity));

    for (index, (spec, cause)) in cases.into_iter().enumerate() {
        assert_eq!(validate_anchor(&spec), Err(cause.clone()), "{cause:?}");
        let refused = store
            .commit_before(
                deadline(),
                intent(&format!("invalid-{index}")),
                |envelope| {
                    envelope.insert_anchor(spec.clone())?;
                    Ok(String::new())
                },
            )
            .unwrap_err();
        assert_eq!(refused, KernelError::InvalidInput, "{cause:?}");
        assert_nothing_written(directory.path(), &spec);
    }
    assert_eq!(
        count(
            &read_only(directory.path()),
            "SELECT COUNT(*) FROM object_registry WHERE object_kind=?1",
            "anchor"
        ),
        0
    );
}

#[test]
fn a_raw_row_with_an_unowned_column_is_conflicting_columns() {
    let row = AnchorRowSpec {
        anchor_id: "raw".to_string(),
        anchor_kind: "reachable_from".to_string(),
        reachable_from_oid: Some(oid(0x12)),
        exact_value: Some("leftover".to_string()),
        ..AnchorRowSpec::default()
    };
    assert_eq!(
        validate_anchor_row(&row),
        Err(AnchorValidationError::Decode(
            AnchorDecodeError::ConflictingColumns(AnchorKind::ReachableFrom)
        ))
    );
    let unknown = AnchorRowSpec {
        anchor_kind: "path".to_string(),
        ..row
    };
    assert_eq!(
        validate_anchor_row(&unknown),
        Err(AnchorValidationError::Decode(
            AnchorDecodeError::UnknownKind("path".to_string())
        ))
    );
}

#[test]
fn a_missing_domain_is_not_found() {
    let directory = tempfile::tempdir().unwrap();
    let store = seeded_store(directory.path());
    let mut spec = spec(
        "orphan",
        AnchorValue::Exact {
            value: "token".to_string(),
        },
    );
    spec.domain_id = "missing".to_string();
    let refused = store
        .commit_before(deadline(), intent("orphan"), |envelope| {
            envelope.insert_anchor(spec.clone())?;
            Ok(String::new())
        })
        .unwrap_err();
    assert_eq!(refused, KernelError::NotFound);
    assert_nothing_written(directory.path(), &spec);
}

#[test]
fn a_duplicate_anchor_id_is_a_conflict_within_and_across_commits() {
    let directory = tempfile::tempdir().unwrap();
    let store = seeded_store(directory.path());
    let first = spec(
        "dup",
        AnchorValue::ReachableFrom {
            oid: oid(0x13),
            captures: Vec::new(),
        },
    );
    let mut same_anchor = first.clone();
    same_anchor.object_id = "anchor-object-dup-second".to_string();

    let within = store
        .commit_before(deadline(), intent("within"), |envelope| {
            envelope.insert_anchor(first.clone())?;
            envelope.insert_anchor(same_anchor.clone())?;
            Ok(String::new())
        })
        .unwrap_err();
    assert_eq!(within, KernelError::Conflict);
    assert_nothing_written(directory.path(), &first);
    assert_nothing_written(directory.path(), &same_anchor);

    store
        .commit_before(deadline(), intent("first"), |envelope| {
            Ok(envelope.insert_anchor(first.clone())?.result_json())
        })
        .unwrap();
    let mut same_object = first.clone();
    same_object.anchor_id = "anchor-dup-second".to_string();
    for (key, retry) in [("second", &same_anchor), ("third", &same_object)] {
        let across = store
            .commit_before(deadline(), intent(key), |envelope| {
                envelope.insert_anchor(retry.clone())?;
                Ok(String::new())
            })
            .unwrap_err();
        assert_eq!(across, KernelError::Conflict, "{key}");
    }
    let connection = read_only(directory.path());
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM anchors WHERE object_id=?1",
            &first.object_id
        ),
        1
    );
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM object_registry WHERE object_id=?1",
            &same_anchor.object_id
        ),
        0
    );
    assert!(stored_row(&connection, &same_object.anchor_id).is_none());
    assert_anchor_rows_well_formed(directory.path());
}

/// A `reachable_from` row written through `insert_anchor` with no capture
/// payload evaluates `Current` at its commit and at a descendant, and
/// `Historical` once HEAD moves to a rewrite that drops the commit.
#[test]
fn a_written_reachable_from_anchor_holds_through_descendants_and_not_after_a_rewrite() {
    let directory = tempfile::tempdir().unwrap();
    let fixture = init_repo(&directory.path().join("repo"));
    let base = commit_snapshot(&fixture.repo, "main", &[], &[("a.txt", "a\n")], "base", 1);
    let anchored = commit_snapshot(
        &fixture.repo,
        "main",
        &[base],
        &[("a.txt", "b\n")],
        "anchored",
        2,
    );
    let descendant = commit_snapshot(
        &fixture.repo,
        "main",
        &[anchored],
        &[("a.txt", "c\n")],
        "descendant",
        3,
    );
    let rewritten = commit_snapshot(
        &fixture.repo,
        "rewrite",
        &[base],
        &[("a.txt", "d\n")],
        "rewritten",
        4,
    );
    let store = seeded_store(&directory.path().join("kernel"));
    let spec = spec(
        "capture",
        AnchorValue::ReachableFrom {
            oid: anchored.to_string(),
            captures: Vec::new(),
        },
    );
    store
        .commit_before(deadline(), intent("capture"), |envelope| {
            Ok(envelope.insert_anchor(spec.clone())?.result_json())
        })
        .unwrap();
    let row = stored_row(
        &read_only(&directory.path().join("kernel")),
        &spec.anchor_id,
    )
    .unwrap();
    let object = ApplicabilityCandidate {
        anchor: Some(row),
        ..candidate("captured-memory")
    };
    for (head, state) in [
        (anchored, ApplicabilityState::Current),
        (descendant, ApplicabilityState::Current),
        (rewritten, ApplicabilityState::Historical),
    ] {
        let snapshot = checkout(&fixture, head);
        let batch = ApplicabilityEngine::new().evaluate_batch(
            &snapshot,
            &QueryContext::default(),
            &ScopeMatchContext::new(),
            std::slice::from_ref(&object),
            &EvalBudget::unbounded(),
        );
        assert_eq!(batch.objects[0].state, state, "HEAD {head}");
    }
}

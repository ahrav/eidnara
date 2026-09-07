//! O8, restart and backup restore identical canonical state: the same-root
//! digest over every declared table survives a reopen; a backup taken at
//! commit `N` restores to the digest observed right after `backup` returned
//! (capture pins are part of that state) with `commit_seq` back at `N`;
//! outbox positions never regress or get reused across prune plus reopen;
//! each restore fault rolls the live family back to its pre-restore state;
//! and an intent replayed after restore is served from its receipt.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

use kernel::schema::KERNEL_SCHEMA_COMPONENT_NAMES;
use kernel::{
    ArtifactDeletionKind, ArtifactErrorKind, ArtifactHandle, BackupRequest, ConsumerAbandonment,
    DecisionEventPayload, DecisionEventSpec, RestoreFault, ScopeSpec, ScopeTermSpec, Sensitivity,
};

use crate::fixtures::{
    DOMAIN, admit_request, admitted_domain, code_observation, decision, deletion, domain, ingest,
    intent, observation, root_domain, staging,
};
use crate::harness::Proof;

/// Declared tables `assert_seeded` skips: the seed leaves them empty.
/// The first eight have no public writer.
/// Ingest removes its own `artifact_ingestion_reservations` row before returning.
/// A purge removes its own `artifact_pending_unlinks` row once the object is unlinked.
const UNSEEDED_TABLES: &[&str] = &[
    "entities",
    "entity_aliases",
    "propositions",
    "predicate_schemas",
    "anchors",
    "asserted_edges",
    "relation_registry",
    "candidate_scores",
    "artifact_ingestion_reservations",
    "artifact_pending_unlinks",
];

/// Declared tables `backup` fills; `assert_seeded` skips them until a backup has run.
const BACKUP_TABLES: &[&str] = &["capture_pins", "capture_pin_refs"];

/// A detector-recognizable secret that causes redacted text to create a `durable_text_redactions` row.
const SECRET: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678";

/// A newly declared table must be seeded or listed as unseeded, so a digest
/// equality cannot silently become empty-vs-empty for it.
fn assert_seeded(proof: &Proof, after_backup: bool) {
    for table in UNSEEDED_TABLES.iter().chain(BACKUP_TABLES) {
        assert!(
            KERNEL_SCHEMA_COMPONENT_NAMES.contains(table),
            "{table} is not a declared table"
        );
        assert!(
            !(UNSEEDED_TABLES.contains(table) && BACKUP_TABLES.contains(table)),
            "{table} listed as both unseeded and backup-populated"
        );
    }
    for table in KERNEL_SCHEMA_COMPONENT_NAMES {
        if UNSEEDED_TABLES.contains(table) || (!after_backup && BACKUP_TABLES.contains(table)) {
            continue;
        }
        assert!(proof.count_table(table) > 0, "{table} not seeded");
    }
}

fn max_outbox_position(proof: &Proof) -> i64 {
    proof
        .db()
        .query_row(
            "SELECT COALESCE(MAX(outbox_position),0) FROM outbox",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

struct Seeded {
    proof: Proof,
    /// Index of a pre-backup intent, replayed after restore.
    replay_index: usize,
    /// The one artifact reference the seed leaves live.
    kept: ArtifactHandle,
}

/// Seeds every table `assert_seeded` checks.
fn seeded() -> Seeded {
    let mut proof = Proof::open();
    proof.commit(intent("seed"), |envelope| {
        envelope.insert_domain(root_domain())?;
        envelope.insert_domain(domain(1))?;
        Ok(String::new())
    });
    proof.commit(intent("consumer"), |envelope| {
        envelope.register_outbox_consumer("search", 10)?;
        envelope.register_outbox_consumer("retired", 10)?;
        Ok(String::new())
    });
    proof.commit(intent("slice"), |envelope| {
        envelope.insert_decision(decision(1))?;
        envelope.insert_observation(observation(2, "decision-object-1"))?;
        envelope.append_decision_event(
            "decision-1",
            DecisionEventSpec {
                event_kind: "status".to_string(),
                payload: DecisionEventPayload {
                    summary: "seeded".to_string(),
                },
                evidence_id: None,
                recorded_at: 11,
            },
        )?;
        // A secret in a term value populates `durable_text_redactions`.
        envelope.insert_scope(ScopeSpec {
            scope_id: "scope-1".to_string(),
            object_id: "scope-object-1".to_string(),
            domain_id: DOMAIN.to_string(),
            source_kind: "fixture".to_string(),
            source_id: "scope".to_string(),
            source_revision: 1,
            sensitivity: Sensitivity::Normal,
            terms: vec![ScopeTermSpec {
                dimension: "branch".to_string(),
                operator: "exact".to_string(),
                exact_value: Some(format!("feature/{SECRET}")),
                ..ScopeTermSpec::default()
            }],
        })?;
        Ok(String::new())
    });
    proof.store().rebuild_alignment().unwrap();
    proof
        .store()
        .stage_candidate(staging("run-1", "candidate-1", "name"))
        .unwrap();
    let (_, replay_index) = proof.commit(intent("admit"), |envelope| {
        let trigger = code_observation("candidate-1");
        let request = admit_request("candidate-1", &trigger.observation_id);
        envelope.insert_observation(trigger)?;
        envelope.admit_domain_candidate(request, admitted_domain("candidate-1", "name"))?;
        Ok(String::new())
    });
    // One artifact stays referenced so a backup has something to pin.
    let kept = proof
        .store()
        .ingest_artifact(ingest("kept", b"kept", Sensitivity::Normal))
        .unwrap();
    let deleted = proof
        .store()
        .ingest_artifact(ingest("deleted", b"deleted", Sensitivity::Sensitive))
        .unwrap();
    proof
        .store()
        .delete_artifact(deletion("delete", &deleted.digest))
        .unwrap();
    let purged = proof
        .store()
        .ingest_artifact(ingest("purged", b"purged", Sensitivity::Normal))
        .unwrap();
    let mut purge = deletion("purge", &purged.digest);
    purge.kind = ArtifactDeletionKind::Purge;
    purge.operator_id = Some("operator".to_string());
    purge.target_locator = Some("incident://seed".to_string());
    purge.reason = Some("seed".to_string());
    proof.store().delete_artifact(purge).unwrap();
    // `search` stays registered for the outbox proofs; `retired` is abandoned.
    proof.commit(intent("abandon"), |envelope| {
        envelope.abandon_outbox_consumer(
            "retired",
            ConsumerAbandonment {
                operator_id: "operator".to_string(),
                reason: "seed".to_string(),
                abandoned_at: 20,
                barrier_id: None,
            },
        )?;
        Ok(String::new())
    });
    proof
        .store()
        .mark_outbox_published_through(max_outbox_position(&proof), 30)
        .unwrap();
    assert_seeded(&proof, false);
    Seeded {
        proof,
        replay_index,
        kept,
    }
}

fn private_dir() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    directory
}

fn backup_request(destination: &std::path::Path) -> BackupRequest {
    BackupRequest {
        destination_directory: destination.to_path_buf(),
        deadline: Instant::now() + Duration::from_secs(30),
        capture_pin_expires_at: None,
    }
}

#[test]
fn seeded_state_digest_survives_reopen() {
    let Seeded { mut proof, .. } = seeded();
    let before = proof.digest();
    let known = proof.store().known_as_of(proof.tip()).unwrap();
    proof.restart();
    assert_eq!(proof.digest(), before);
    assert_eq!(proof.store().known_as_of(proof.tip()).unwrap(), known);
}

#[test]
fn backup_and_restore_reproduce_every_database_table_and_the_commit_seq() {
    let Seeded {
        mut proof,
        replay_index,
        kept,
    } = seeded();
    let destination = private_dir();
    let backup = proof
        .store()
        .backup(backup_request(destination.path()))
        .unwrap();
    let captured = proof.tip();
    assert_eq!(backup.captured_commit_seq, captured);
    // The fixture leaves `evidence-kept` live and invalidates `evidence-deleted`
    // and `evidence-purged`, so capture owes exactly the live one. Comparing
    // identities catches a capture that pins a stale row or misses the live one.
    assert_eq!(backup.evidence_refs, ["evidence-kept"]);
    let capture_pin_id = backup.capture_pin_id.clone().unwrap();
    let pinned = proof
        .db()
        .prepare(
            "SELECT evidence_id FROM capture_pin_refs WHERE capture_pin_id=?1 ORDER BY evidence_id",
        )
        .unwrap()
        .query_map([&capture_pin_id], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(pinned, ["evidence-kept"]);
    // `backup` commits its capture pins before copying, so the digest to
    // restore to is the one observed after it returns.
    let expected = proof.digest();
    assert_seeded(&proof, true);
    assert_eq!(kept.evidence_id, "evidence-kept");
    assert_eq!(proof.store().read_artifact(&kept).unwrap(), b"kept");

    proof.commit(intent("after-backup"), |envelope| {
        envelope.insert_domain(domain(2))?;
        Ok(String::new())
    });
    let orphan = proof
        .store()
        .ingest_artifact(ingest("post-backup", b"post-backup", Sensitivity::Normal))
        .unwrap();
    proof
        .store()
        .delete_artifact(deletion("delete-kept", &kept.digest))
        .unwrap();
    assert_eq!(
        proof.store().read_artifact(&kept).unwrap_err().kind(),
        ArtifactErrorKind::ReferenceUnavailable,
        "positive control: the pre-backup reference is invalidated"
    );
    let pre_restore = proof.digest();
    assert_ne!(pre_restore, expected, "positive control: state moved on");

    assert_eq!(
        proof.store().restore(&backup.destination_path).unwrap(),
        captured
    );
    // `restore` displaces only the `kernel.sqlite` family (`displace_family` in `backup.rs`); the `artifacts/` tree is never touched, so its `cas_*` digests keep the post-backup listing while every table digest returns to the backup. commentlint: allow(JUDGE)
    let assert_restored = |proof: &Proof, what: &str| {
        let after = proof.digest();
        assert_eq!(after.tables.len(), expected.tables.len(), "{what}");
        for (table, hash) in &after.tables {
            let anchor = if table.starts_with("cas_") {
                &pre_restore
            } else {
                &expected
            };
            assert_eq!(hash, anchor.table(table), "{what}: {table}");
        }
        // The post-backup object is an orphan: bytes on disk, no live reference.
        let path = proof
            .path()
            .join("artifacts/objects")
            .join(&orphan.digest[..2])
            .join(&orphan.digest[2..]);
        assert!(
            path.exists(),
            "{what}: restore unlinked the orphan {path:?}"
        );
        assert_eq!(
            proof.store().read_artifact(&orphan).unwrap_err().kind(),
            ArtifactErrorKind::ReferenceUnavailable,
            "{what}: the orphan's reference survived restore"
        );
        assert_eq!(
            proof.store().read_artifact(&kept).unwrap(),
            b"kept",
            "{what}: the restored reference does not read"
        );
    };
    assert_restored(&proof, "after restore");
    assert_eq!(proof.tip(), captured);
    proof.restart();
    assert_restored(&proof, "after restart");

    // A pre-backup intent replays from its restored receipt.
    let replayed = proof.replay(replay_index);
    assert!(replayed.replayed);
    assert_restored(&proof, "after replay");
}

#[test]
fn outbox_position_never_regresses_or_reuses_across_prune_and_reopen() {
    let Seeded { mut proof, .. } = seeded();
    let tip = proof.tip();
    proof.store().acknowledge_outbox("search", tip, 40).unwrap();
    let high_water = max_outbox_position(&proof);
    let pruned = proof.store().prune_outbox().unwrap();
    assert!(pruned.deleted > 0, "positive control: prune removed rows");
    let last_before_restart = proof.tip();
    proof.restart();
    proof.commit(intent("after-prune"), |envelope| {
        envelope.insert_domain(domain(3))?;
        Ok(String::new())
    });
    // Each open bumps `writer_fence.writer_epoch` and every commit stamps it,
    // so the epoch never decreases along `commit_seq` and steps up across
    // the restart.
    let epochs = proof
        .db()
        .prepare("SELECT commit_seq, writer_epoch FROM commit_log ORDER BY commit_seq")
        .unwrap()
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(
        epochs.windows(2).all(|pair| pair[0].1 <= pair[1].1),
        "writer_epoch decreased along commit_seq: {epochs:?}"
    );
    let epoch_at = |seq: i64| {
        epochs
            .iter()
            .find(|(commit_seq, _)| *commit_seq == seq)
            .map(|(_, epoch)| *epoch)
            .unwrap_or_else(|| panic!("commit_seq {seq} missing from commit_log"))
    };
    assert!(
        epoch_at(proof.tip()) > epoch_at(last_before_restart),
        "the first commit after restart did not step the writer epoch: {epochs:?}"
    );
    let positions = proof
        .db()
        .prepare("SELECT outbox_position FROM outbox ORDER BY outbox_position")
        .unwrap()
        .query_map([], |row| row.get::<_, i64>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(
        !positions.is_empty(),
        "positive control: the after-prune commit emitted outbox rows"
    );
    assert!(
        positions.iter().all(|position| *position > high_water),
        "{positions:?}"
    );
}

#[test]
fn each_restore_fault_rolls_back_to_the_pre_restore_state() {
    for &fault in RestoreFault::ALL {
        let Seeded { mut proof, .. } = seeded();
        let destination = private_dir();
        let backup = proof
            .store()
            .backup(backup_request(destination.path()))
            .unwrap();
        let restored = proof.digest();
        proof.commit(intent("after-backup"), |envelope| {
            envelope.insert_domain(domain(2))?;
            Ok(String::new())
        });
        let pre_restore = proof.digest();
        assert_ne!(pre_restore, restored);
        // Every restore fault fires before the restored family is published,
        // so recovery rolls the live family back rather than completing.
        proof
            .fault_restore(&backup.destination_path, fault)
            .assert_same(&pre_restore, &format!("{fault:?} did not roll back"));
        // The store is writable afterwards and the digest is stable.
        proof.commit(intent("after-fault"), |envelope| {
            envelope.insert_domain(domain(4))?;
            Ok(String::new())
        });
        let after = proof.digest();
        proof.restart();
        assert_eq!(proof.digest(), after, "{fault:?}");
    }
}

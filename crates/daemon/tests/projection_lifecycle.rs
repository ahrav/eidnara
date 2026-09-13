//! The projection lifecycle record outside the disposable `search/` family: a rebuild or authorized-recovery intent survives deleting the database and matches an independent ledger after reopen; replays reconcile to one record and conflicts change nothing; the record is written only under the gate's admission and never enables a hook; and a process cut before or after the record's rename or directory sync leaves a complete prior or new record, never a mixture.

mod support;

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use daemon::projection_gates::{Denial, EntryPoint, HookGate, ProjectionHook};
use daemon::projection_lifecycle::{
    CONTROL_DIR, CONTROL_RECORD, Cause, ConsumerBinding, ControlState, EpisodeAccounting,
    IntentRefusal, LifecycleIntent, LifecycleRequest, ProjectionLifecycle, RecoveryTarget,
    Transition, WriteBarrier,
};
use daemon::search_projection::SearchProjection;
use host_runtime::LifecycleTransactionLock;
use serde_json::{Value, json};
use support::projection_gate::{identity, open_gate, passing_evaluator};

const CHILD_ROOT: &str = "EIDNARA_LIFECYCLE_CHILD_ROOT";
const CHILD_CUT: &str = "EIDNARA_LIFECYCLE_CHILD_CUT";
const CHILD_BARRIER: &str = "EIDNARA_LIFECYCLE_BARRIER";
const NOW: i64 = 1_700_000_000_000;

fn rebuild_request() -> LifecycleRequest {
    LifecycleRequest {
        transition: Transition::Rebuilding,
        selected_generation: "gen-2".to_owned(),
        kernel_incarnation_id: "incarnation-a".to_owned(),
        consumer: ConsumerBinding {
            consumer_id: "search".to_owned(),
            generation_id: "gen-2".to_owned(),
        },
        cause: Cause::EmbeddingModelMismatch,
        attempt_id: "attempt-1".to_owned(),
        recovery_target: Some(RecoveryTarget { commit_seq: 41 }),
        allowance: 2,
        deadline: NOW + 60_000,
        authorization_ref: None,
    }
}

#[test]
fn fixing_a_target_preserves_original_request_replay_and_never_moves_it() {
    let root = tempfile::tempdir().unwrap();
    let gate = open_gate();
    let lifecycle = ProjectionLifecycle::open(root.path()).unwrap();
    let request = LifecycleRequest {
        recovery_target: None,
        ..rebuild_request()
    };
    lifecycle.record(&gate, &request, NOW).unwrap();
    assert_eq!(
        lifecycle.fix_target(&gate, RecoveryTarget { commit_seq: -1 }),
        Err(IntentRefusal::IllegalCombination)
    );
    let fixed = lifecycle
        .fix_target(&gate, RecoveryTarget { commit_seq: 41 })
        .unwrap();
    assert_eq!(
        fixed.recovery_target,
        Some(RecoveryTarget { commit_seq: 41 })
    );
    assert_eq!(
        lifecycle
            .fix_target(&gate, RecoveryTarget { commit_seq: 41 })
            .unwrap(),
        fixed
    );
    assert!(matches!(
        lifecycle.fix_target(&gate, RecoveryTarget { commit_seq: 42 }),
        Err(IntentRefusal::Conflict { .. })
    ));
    let replay = lifecycle.record(&gate, &request, NOW + 1).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.intent, fixed);
    let other = LifecycleRequest {
        recovery_target: Some(RecoveryTarget { commit_seq: 42 }),
        ..request
    };
    assert!(matches!(
        lifecycle.record(&gate, &other, NOW + 2),
        Err(IntentRefusal::Conflict { .. })
    ));
    drop(lifecycle);
    assert_eq!(
        ProjectionLifecycle::open(root.path()).unwrap().read(),
        ControlState::Intent(fixed)
    );
}

fn recovery_request() -> LifecycleRequest {
    LifecycleRequest {
        transition: Transition::AuthorizedRecovery,
        cause: Cause::DisabledRecovery,
        attempt_id: "attempt-r1".to_owned(),
        authorization_ref: Some("ops:ticket-77".to_owned()),
        ..rebuild_request()
    }
}

#[test]
fn control_lock_refuses_without_waiting_and_allows_explicit_retry() {
    let root = tempfile::tempdir().unwrap();
    let lifecycle = ProjectionLifecycle::open(root.path()).unwrap();
    let lock = std::fs::File::open(root.path().join(CONTROL_DIR)).unwrap();
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive).unwrap();
    let gate = open_gate();
    assert_eq!(
        lifecycle.record(&gate, &rebuild_request(), NOW),
        Err(IntentRefusal::WouldBlock)
    );
    assert_eq!(
        ProjectionLifecycle::open(root.path()).err().unwrap().kind(),
        std::io::ErrorKind::WouldBlock
    );
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::Unlock).unwrap();
    assert!(
        !lifecycle
            .record(&gate, &rebuild_request(), NOW)
            .unwrap()
            .replayed
    );
}

#[test]
fn private_cleanup_preserves_live_families_and_revoked_lease_handoffs() {
    let root = tempfile::tempdir().unwrap();
    let gate = open_gate();
    let lifecycle = ProjectionLifecycle::open(root.path()).unwrap();
    lifecycle.record(&gate, &rebuild_request(), NOW).unwrap();
    let main = SearchProjection::open(root.path()).unwrap();
    let private_home = root.path().join(CONTROL_DIR).join("replacement");
    let private = SearchProjection::open(&private_home).unwrap();
    let path = private.path().to_path_buf();
    let mut handoffs = 0;
    assert_eq!(
        lifecycle.delete_replacement_family(&gate, None, || handoffs += 1),
        Err(IntentRefusal::FamilyHeld)
    );
    assert_eq!(handoffs, 1);
    assert!(path.exists());
    let mut lease = private.close().1;
    let close_gate = std::sync::Arc::clone(&gate);
    let lifecycle = lifecycle.with_write_barrier_for_test(move |barrier| {
        if barrier == WriteBarrier::BeforeFamilyRemoval {
            close_gate.close();
        }
    });
    assert_eq!(
        lifecycle.delete_replacement_family(&gate, None, || {
            lease.take();
        }),
        Err(IntentRefusal::Revoked)
    );
    assert!(lease.is_some(), "revocation precedes lease handoff");
    assert!(path.exists());
    assert!(main.path().exists());
    drop(lifecycle);
    gate.install(passing_evaluator(
        &identity("test-incarnation", 8),
        0,
        &ProjectionHook::ALL,
    ));
    let lifecycle = ProjectionLifecycle::open(root.path()).unwrap();
    lifecycle
        .delete_replacement_family(&gate, None, || {
            lease.take();
        })
        .unwrap();
    assert!(!path.exists());
    assert!(
        main.path().exists(),
        "private cleanup cannot select another data home"
    );
    let private = SearchProjection::open(&private_home).unwrap();
    lease = private.close().1;
    let before = fs::read(record_path(root.path())).unwrap();
    assert_eq!(
        lifecycle.delete_replacement_family(&gate, None, || {
            lease.take();
            gate.close();
        }),
        Err(IntentRefusal::Revoked)
    );
    assert!(
        lease.is_none(),
        "the callback relinquished the original owner"
    );
    assert!(path.exists(), "revocation during handoff stops deletion");
    assert_eq!(fs::read(record_path(root.path())).unwrap(), before);
    gate.install(passing_evaluator(
        &identity("test-incarnation", 8),
        0,
        &ProjectionHook::ALL,
    ));
    lifecycle
        .delete_replacement_family(&gate, None, || {
            lease.take();
        })
        .unwrap();
    assert!(!path.exists());
    assert!(main.path().exists());
}

#[test]
fn capture_replacement_and_stage_reversal_require_owned_cleanup() {
    use daemon::projection_lifecycle::ReplacementCapture;
    let root = tempfile::tempdir().unwrap();
    let lifecycle = ProjectionLifecycle::open(root.path()).unwrap();
    let transaction = LifecycleTransactionLock::acquire_exclusive(Some(root.path())).unwrap();
    let gate = open_gate();
    lifecycle.record(&gate, &rebuild_request(), NOW).unwrap();
    let capture = ReplacementCapture {
        hold_id: "a".repeat(32),
        snapshot: 40,
        lease_epoch: 1,
        source_policy_version: "source-policy.v1".to_owned(),
        expires_at: NOW + 30_000,
        stage: None,
    };
    lifecycle
        .record_capture(&gate, &transaction, Some(capture.clone()), None)
        .unwrap();
    let other = ReplacementCapture {
        hold_id: "b".repeat(32),
        ..capture.clone()
    };
    assert!(matches!(
        lifecycle.record_capture(
            &gate,
            &transaction,
            Some(other.clone()),
            Some(&capture.hold_id)
        ),
        Err(IntentRefusal::Conflict { .. })
    ));
    assert!(matches!(
        lifecycle.record_capture(&gate, &transaction, None, Some(&other.hold_id)),
        Err(IntentRefusal::Conflict { .. })
    ));
    for changed in [
        ReplacementCapture {
            snapshot: 41,
            ..capture.clone()
        },
        ReplacementCapture {
            lease_epoch: 2,
            ..capture.clone()
        },
        ReplacementCapture {
            source_policy_version: "different".to_owned(),
            ..capture.clone()
        },
        ReplacementCapture {
            expires_at: NOW + 40_000,
            ..capture.clone()
        },
    ] {
        let before = fs::read(record_path(root.path())).unwrap();
        assert!(matches!(
            lifecycle.record_capture(&gate, &transaction, Some(changed), Some(&capture.hold_id)),
            Err(IntentRefusal::Conflict { .. })
        ));
        assert_eq!(fs::read(record_path(root.path())).unwrap(), before);
    }
    // A first certificate is bound to the capture it certifies: another hold or snapshot is refused with the record unchanged.
    for unbound in [
        certificate(&"b".repeat(32)),
        daemon::search_seed::SeedVerification {
            snapshot_commit_seq: 41,
            ..certificate(&capture.hold_id)
        },
    ] {
        let before = fs::read(record_path(root.path())).unwrap();
        assert!(matches!(
            lifecycle.record_capture(
                &gate,
                &transaction,
                Some(ReplacementCapture {
                    stage: Some(Box::new(unbound)),
                    ..capture.clone()
                }),
                Some(&capture.hold_id)
            ),
            Err(IntentRefusal::Conflict { .. })
        ));
        assert_eq!(fs::read(record_path(root.path())).unwrap(), before);
    }
    let certificate = certificate(&capture.hold_id);
    let staged = ReplacementCapture {
        stage: Some(Box::new(certificate)),
        ..capture.clone()
    };
    lifecycle
        .record_capture(
            &gate,
            &transaction,
            Some(staged.clone()),
            Some(&capture.hold_id),
        )
        .unwrap();
    lifecycle
        .record_capture(
            &gate,
            &transaction,
            Some(staged.clone()),
            Some(&capture.hold_id),
        )
        .unwrap();
    assert!(matches!(
        lifecycle.record_capture(
            &gate,
            &transaction,
            Some(capture.clone()),
            Some(&capture.hold_id)
        ),
        Err(IntentRefusal::Conflict { .. })
    ));
    let expected = staged.stage.as_ref().unwrap().stage_manifest().digest();
    assert_eq!(
        ProjectionLifecycle::protected_generations(root.path(), &transaction).unwrap(),
        std::collections::BTreeSet::from([expected.clone()])
    );
    lifecycle
        .record_capture(&gate, &transaction, None, Some(&capture.hold_id))
        .unwrap();
    let cleared = fs::read(record_path(root.path())).unwrap();
    lifecycle
        .record_capture(&gate, &transaction, None, Some(&capture.hold_id))
        .unwrap();
    assert_eq!(fs::read(record_path(root.path())).unwrap(), cleared);
    // A cleared capture refuses a cleanup that still names the old hold.
    assert!(matches!(
        lifecycle.delete_replacement_family(&gate, Some(&capture.hold_id), || ()),
        Err(IntentRefusal::Conflict { .. })
    ));
    assert_eq!(
        lifecycle.record_capture(&gate, &transaction, Some(staged), None),
        Err(IntentRefusal::IllegalCombination)
    );
    lifecycle
        .record_capture(&gate, &transaction, Some(other), None)
        .unwrap();
}

fn certificate(hold_id: &str) -> daemon::search_seed::SeedVerification {
    serde_json::from_value(serde_json::json!({
        "schema": 1, "schema_version": 3, "kernel_incarnation_id": "incarnation-a",
        "projection_policy_version": "source-policy.v1", "identity_contract_version": "search-projection-identity-v2",
        "limit_manifest_protocol_version": "limits.v1", "embedding_model": "model", "tokenizer_fingerprint": "tokenizer",
        "vector_dimension": 8, "generation_epoch": 1, "generation_id": "gen-2", "generation_state": "building",
        "snapshot_commit_seq": 40, "checkpoint_commit_seq": 41, "occurrences": 0, "tombstones": 0,
        "hold_id": hold_id, "pending_jobs": 0, "admitted_jobs": 0, "vectors": 0, "bytes": 0, "sha256": "a".repeat(64),
    })).unwrap()
}

#[test]
fn pinning_validates_digest_spelling_and_binds_a_prepared_certificate() {
    for prepared in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let gate = open_gate();
        let transaction = LifecycleTransactionLock::acquire_exclusive(Some(root.path())).unwrap();
        let lifecycle = ProjectionLifecycle::open(root.path()).unwrap();
        lifecycle.record(&gate, &rebuild_request(), NOW).unwrap();
        let mut capture = daemon::projection_lifecycle::ReplacementCapture {
            hold_id: "a".repeat(32),
            snapshot: 40,
            lease_epoch: 1,
            source_policy_version: "source-policy.v1".to_owned(),
            expires_at: NOW + 30_000,
            stage: None,
        };
        let expected = if prepared {
            lifecycle
                .record_capture(&gate, &transaction, Some(capture.clone()), None)
                .unwrap();
            capture.stage = Some(Box::new(certificate(&capture.hold_id)));
            lifecycle
                .record_capture(
                    &gate,
                    &transaction,
                    Some(capture.clone()),
                    Some(&capture.hold_id),
                )
                .unwrap();
            capture.stage.as_ref().unwrap().stage_manifest().digest()
        } else {
            "a".repeat(64)
        };
        let before = fs::read(record_path(root.path())).unwrap();
        for invalid in [
            String::new(),
            "a".repeat(63),
            "a".repeat(65),
            "A".repeat(64),
            "g".repeat(64),
        ] {
            assert_eq!(
                lifecycle.pin_seed(&gate, &transaction, &invalid),
                Err(IntentRefusal::InvalidDigest)
            );
            assert_eq!(fs::read(record_path(root.path())).unwrap(), before);
        }
        if prepared {
            assert_ne!(expected, "0".repeat(64));
            assert!(matches!(
                lifecycle.pin_seed(&gate, &transaction, &"0".repeat(64)),
                Err(IntentRefusal::Conflict { .. })
            ));
            assert_eq!(fs::read(record_path(root.path())).unwrap(), before);
        }
        let pinned = lifecycle.pin_seed(&gate, &transaction, &expected).unwrap();
        assert_eq!(
            pinned.staged_seed_digest.as_deref(),
            Some(expected.as_str())
        );
        assert_eq!(
            lifecycle.pin_seed(&gate, &transaction, &expected).unwrap(),
            pinned
        );
        drop(lifecycle);
        assert_eq!(
            ProjectionLifecycle::open(root.path()).unwrap().read(),
            ControlState::Intent(pinned)
        );
    }
}

#[test]
fn capture_and_target_writes_keep_upstream_revocation_and_size_guards() {
    let root = tempfile::tempdir().unwrap();
    let gate = open_gate();
    let lifecycle = ProjectionLifecycle::open(root.path()).unwrap();
    let transaction = LifecycleTransactionLock::acquire_exclusive(Some(root.path())).unwrap();
    let request = LifecycleRequest {
        recovery_target: None,
        ..rebuild_request()
    };
    lifecycle.record(&gate, &request, NOW).unwrap();
    let before = lifecycle.read();
    let mut capture = daemon::projection_lifecycle::ReplacementCapture {
        hold_id: "a".repeat(32),
        snapshot: 40,
        lease_epoch: 1,
        source_policy_version: "p".repeat(daemon::projection_lifecycle::MAX_RECORD_BYTES as usize),
        expires_at: NOW + 30_000,
        stage: None,
    };
    assert_eq!(
        lifecycle.record_capture(&gate, &transaction, Some(capture.clone()), None),
        Err(IntentRefusal::Oversized)
    );
    assert_eq!(lifecycle.read(), before);
    capture.source_policy_version = "source-policy.v1".to_owned();
    let close_gate = std::sync::Arc::clone(&gate);
    let lifecycle = lifecycle.with_write_barrier_for_test(move |barrier| {
        if barrier == WriteBarrier::BeforeRename {
            close_gate.close();
        }
    });
    assert_eq!(
        lifecycle.record_capture(&gate, &transaction, Some(capture), None),
        Err(IntentRefusal::Revoked)
    );
    assert_eq!(lifecycle.read(), before);
    gate.install(passing_evaluator(
        &identity("test-incarnation", 8),
        0,
        &ProjectionHook::ALL,
    ));
    assert_eq!(
        lifecycle.fix_target(&gate, RecoveryTarget { commit_seq: 41 }),
        Err(IntentRefusal::Revoked)
    );
    assert_eq!(lifecycle.read(), before);
}

/// The encoded size `record` reserves for `intent`: the disabled record wrapping it with every optional field at its widest.
fn reserved(intent: &LifecycleIntent) -> usize {
    serde_json::to_vec(&json!({
        "schema": 3,
        "handoff": intent,
        "recorded_at": i64::MAX,
        "episodes": {"allowance": u32::MAX, "consumed": u32::MAX, "deadline": i64::MAX},
        "through": i64::MAX,
        "deregistered": true,
    }))
    .unwrap()
    .len()
}

/// The record the test expects, built from the request alone.
fn expected(request: &LifecycleRequest, consumed: u32) -> LifecycleIntent {
    LifecycleIntent {
        schema: 2,
        transition: request.transition,
        selected_generation: request.selected_generation.clone(),
        kernel_incarnation_id: request.kernel_incarnation_id.clone(),
        consumer: request.consumer.clone(),
        cause: request.cause,
        attempt_id: request.attempt_id.clone(),
        recovery_target: request.recovery_target,
        episodes: EpisodeAccounting {
            allowance: request.allowance,
            consumed,
            deadline: request.deadline,
        },
        authorization_ref: request.authorization_ref.clone(),
        staged_seed_digest: None,
        replacement_capture: None,
        recorded_at: NOW,
        prior_disabled: None,
    }
}

fn record_path(data_home: &Path) -> PathBuf {
    data_home.join(CONTROL_DIR).join(CONTROL_RECORD)
}

/// Reads the record's JSON directly, outside the API under test.
fn raw_record(data_home: &Path) -> Value {
    serde_json::from_slice(&fs::read(record_path(data_home)).unwrap()).unwrap()
}

/// The recovery record as the ledger spells it, every persisted field written out by hand.
fn recovery_ledger(consumed: u32) -> Value {
    json!({
        "schema": 2,
        "transition": "AuthorizedRecovery",
        "selected_generation": "gen-2",
        "kernel_incarnation_id": "incarnation-a",
        "consumer": { "consumer_id": "search", "generation_id": "gen-2" },
        "cause": "DisabledRecovery",
        "attempt_id": "attempt-r1",
        "recovery_target": { "commit_seq": 41 },
        "episodes": { "allowance": 2, "consumed": consumed, "deadline": NOW + 60_000 },
        "authorization_ref": "ops:ticket-77",
        "staged_seed_digest": null,
        "recorded_at": NOW,
    })
}

fn family_entries(data_home: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(data_home.join("search"))
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Deletion requires an admitted recorded intent, no live holder of the family's storage lease, and an unexpired, unexhausted episode allowance; the intent persists independently of the deleted family.
#[test]
fn intent_survives_deleting_the_disposable_family_and_matches_the_ledger_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let gate = open_gate();
    let projection = SearchProjection::open(dir.path()).unwrap();
    let database = projection.path().to_path_buf();
    assert!(database.exists());

    let lifecycle = ProjectionLifecycle::open(dir.path()).unwrap();
    assert_eq!(lifecycle.read(), ControlState::Absent);
    assert_eq!(
        lifecycle.delete_disposable_family(&gate, NOW),
        Err(IntentRefusal::NoIntent),
        "nothing is deleted before an intent explains the deletion"
    );
    assert!(database.exists());
    // A stray rollback journal is part of the family the deletion owns.
    fs::write(
        dir.path().join("search").join("search.sqlite-journal"),
        b"j",
    )
    .unwrap();

    let request = recovery_request();
    let recorded = lifecycle.record(&gate, &request, NOW).unwrap();
    assert!(!recorded.replayed);
    assert_eq!(recorded.intent, expected(&request, 0));
    let consumed = lifecycle.consume_episode(&gate, NOW + 1).unwrap();
    assert_eq!(consumed.consumed, 1);
    assert_eq!(
        lifecycle.delete_disposable_family(&HookGate::closed(), NOW + 1),
        Err(IntentRefusal::Denied(Denial::NoManifest)),
        "the record alone does not authorize deleting the database"
    );
    assert!(database.exists());
    assert_eq!(
        lifecycle.delete_disposable_family(&gate, NOW + 1),
        Err(IntentRefusal::FamilyHeld),
        "a live projection holds the family's storage lease; the database is not unlinked under it"
    );
    assert!(database.exists());
    drop(projection);
    assert_eq!(
        lifecycle.delete_disposable_family(&gate, NOW + 60_001),
        Err(IntentRefusal::DeadlineExpired),
        "the deadline bounds the deletion as it bounds the episodes"
    );
    assert!(database.exists());
    let deleted = lifecycle.delete_disposable_family(&gate, NOW + 1).unwrap();
    assert_eq!(deleted, expected(&request, 1));
    assert!(!database.exists());
    assert!(
        family_entries(dir.path())
            .iter()
            .all(|name| !name.starts_with("search.sqlite")),
        "the database and every journal are gone; the storage lease is not the family's: {:?}",
        family_entries(dir.path())
    );
    drop(lifecycle);

    for reopen in 0..2 {
        let lifecycle = ProjectionLifecycle::open(dir.path()).unwrap();
        assert_eq!(
            lifecycle.read(),
            ControlState::Intent(expected(&request, 1)),
            "reopen {reopen}"
        );
        assert_eq!(
            raw_record(dir.path()),
            recovery_ledger(1),
            "reopen {reopen}"
        );
        assert_eq!(
            fs::metadata(record_path(dir.path()))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    // The projection reopens as a fresh database; the intent, not the database, carries the episode.
    let projection = SearchProjection::open(dir.path()).unwrap();
    let identity = projection
        .read(|conn| retrieval::read_identity(conn))
        .unwrap();
    assert_eq!(identity, None);
    drop(projection);
    let lifecycle = ProjectionLifecycle::open(dir.path()).unwrap();
    // A restart renews no allowance: one episode remains, then none, and none past the deadline.
    assert_eq!(
        lifecycle.consume_episode(&gate, NOW + 60_001),
        Err(IntentRefusal::DeadlineExpired)
    );
    assert_eq!(
        lifecycle.consume_episode(&gate, NOW + 2).unwrap().consumed,
        2
    );
    assert_eq!(
        lifecycle.consume_episode(&gate, NOW + 3),
        Err(IntentRefusal::AllowanceExhausted)
    );
    assert_eq!(
        lifecycle.delete_disposable_family(&gate, NOW + 3),
        Err(IntentRefusal::AllowanceExhausted),
        "an exhausted intent authorizes no further deletion"
    );
    assert_eq!(raw_record(dir.path()), recovery_ledger(2));
}

/// A request whose budget can never run an episode is refused before anything is read: zero allowance, or a deadline already passed.
#[test]
fn a_request_with_no_runnable_episode_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let gate = open_gate();
    let lifecycle = ProjectionLifecycle::open(dir.path()).unwrap();
    assert_eq!(
        lifecycle.record(
            &gate,
            &LifecycleRequest {
                allowance: 0,
                ..recovery_request()
            },
            NOW
        ),
        Err(IntentRefusal::AllowanceExhausted)
    );
    assert_eq!(
        lifecycle.record(
            &gate,
            &LifecycleRequest {
                deadline: NOW - 1,
                ..recovery_request()
            },
            NOW
        ),
        Err(IntentRefusal::DeadlineExpired)
    );
    assert_eq!(lifecycle.read(), ControlState::Absent);
    assert!(!record_path(dir.path()).exists());
    // A deadline equal to `now` still admits the request, as it still admits an episode.
    lifecycle
        .record(
            &gate,
            &LifecycleRequest {
                deadline: NOW,
                ..recovery_request()
            },
            NOW,
        )
        .unwrap();
    assert_eq!(lifecycle.consume_episode(&gate, NOW).unwrap().consumed, 1);
}

use std::os::unix::fs::PermissionsExt;

/// AC3, AC5: a replayed request reconciles to the one record; a request differing in attempt, incarnation, or authorization is refused and the record is byte-identical afterwards; a recovery without authorization, or with a malformed reference, is refused before anything is read.
#[test]
fn replays_reconcile_to_one_intent_and_conflicts_change_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let gate = open_gate();
    let lifecycle = ProjectionLifecycle::open(dir.path()).unwrap();
    let request = recovery_request();
    lifecycle.record(&gate, &request, NOW).unwrap();
    let bytes = fs::read(record_path(dir.path())).unwrap();

    // The success response was lost; the same request again is the same record.
    let replay = lifecycle.record(&gate, &request, NOW + 5).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.intent, expected(&request, 0));
    assert_eq!(fs::read(record_path(dir.path())).unwrap(), bytes);

    // A replay after an episode is spent keeps the spent episode.
    assert_eq!(
        lifecycle.consume_episode(&gate, NOW + 6).unwrap().consumed,
        1
    );
    let bytes = fs::read(record_path(dir.path())).unwrap();
    let replay = lifecycle.record(&gate, &request, NOW + 7).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.intent, expected(&request, 1));
    assert_eq!(fs::read(record_path(dir.path())).unwrap(), bytes);
    assert_eq!(
        lifecycle.consume_episode(&gate, NOW + 8).unwrap().consumed,
        2
    );
    assert_eq!(
        lifecycle.consume_episode(&gate, NOW + 8),
        Err(IntentRefusal::AllowanceExhausted)
    );
    let bytes = fs::read(record_path(dir.path())).unwrap();

    let conflicts = [
        LifecycleRequest {
            attempt_id: "attempt-r2".to_owned(),
            ..request.clone()
        },
        LifecycleRequest {
            kernel_incarnation_id: "incarnation-b".to_owned(),
            ..request.clone()
        },
        LifecycleRequest {
            authorization_ref: Some("ops:ticket-78".to_owned()),
            ..request.clone()
        },
        LifecycleRequest {
            recovery_target: Some(RecoveryTarget { commit_seq: 42 }),
            ..request.clone()
        },
        rebuild_request(),
    ];
    for conflict in conflicts {
        assert_eq!(
            lifecycle.record(&gate, &conflict, NOW + 9),
            Err(IntentRefusal::Conflict {
                attempt_id: "attempt-r1".to_owned()
            }),
            "{conflict:?}"
        );
        assert_eq!(fs::read(record_path(dir.path())).unwrap(), bytes);
    }

    let fresh = tempfile::tempdir().unwrap();
    let lifecycle = ProjectionLifecycle::open(fresh.path()).unwrap();
    assert_eq!(
        lifecycle.record(
            &gate,
            &LifecycleRequest {
                consumer: ConsumerBinding {
                    consumer_id: " ".to_owned(),
                    generation_id: "gen-2".to_owned(),
                },
                ..rebuild_request()
            },
            NOW
        ),
        Err(IntentRefusal::InvalidConsumer)
    );
    assert_eq!(
        lifecycle.record(
            &gate,
            &LifecycleRequest {
                authorization_ref: None,
                ..recovery_request()
            },
            NOW
        ),
        Err(IntentRefusal::MissingAuthorization)
    );
    assert_eq!(
        lifecycle.record(
            &gate,
            &LifecycleRequest {
                authorization_ref: Some("ops ticket".to_owned()),
                ..recovery_request()
            },
            NOW
        ),
        Err(IntentRefusal::InvalidAuthorization)
    );
    assert_eq!(
        lifecycle.record(
            &gate,
            &LifecycleRequest {
                cause: Cause::SchemaMismatch,
                ..recovery_request()
            },
            NOW
        ),
        Err(IntentRefusal::IllegalCombination),
        "a recovery's cause is the disabled projection"
    );
    assert_eq!(
        lifecycle.record(
            &gate,
            &LifecycleRequest {
                authorization_ref: Some("ops:ticket-77".to_owned()),
                ..rebuild_request()
            },
            NOW
        ),
        Err(IntentRefusal::IllegalCombination),
        "a rebuild persists no authorization it never needed"
    );
    assert_eq!(lifecycle.read(), ControlState::Absent);
    assert_eq!(
        lifecycle.consume_episode(&gate, NOW),
        Err(IntentRefusal::NoIntent),
        "a restart with no intent has no allowance to spend"
    );
}

/// AC3: duplicate requests in flight at once, as a lost response produces, leave exactly one record; a conflicting attempt racing them is refused and the record afterwards is one of the replays.
#[test]
fn concurrent_duplicate_requests_leave_exactly_one_intent() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let outcomes: Vec<Result<bool, IntentRefusal>> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..8)
            .map(|worker| {
                let root = root.clone();
                scope.spawn(move || {
                    let gate = open_gate();
                    let deadline = std::time::Instant::now() + Duration::from_secs(5);
                    let lifecycle = loop {
                        match ProjectionLifecycle::open(&root) {
                            Ok(lifecycle) => break lifecycle,
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                assert!(std::time::Instant::now() < deadline);
                                std::thread::yield_now();
                            }
                            Err(error) => panic!("{error}"),
                        }
                    };
                    let request = if worker % 4 == 3 {
                        LifecycleRequest {
                            attempt_id: format!("attempt-other-{worker}"),
                            ..recovery_request()
                        }
                    } else {
                        recovery_request()
                    };
                    loop {
                        match lifecycle.record(&gate, &request, NOW) {
                            Err(IntentRefusal::WouldBlock) => {
                                assert!(std::time::Instant::now() < deadline);
                                std::thread::yield_now();
                            }
                            result => break result.map(|recorded| recorded.replayed),
                        }
                    }
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect()
    });
    let written = outcomes
        .iter()
        .filter(|outcome| **outcome == Ok(false))
        .count();
    let replayed = outcomes
        .iter()
        .filter(|outcome| **outcome == Ok(true))
        .count();
    let refused = outcomes.iter().filter(|outcome| outcome.is_err()).count();
    let survivor = ProjectionLifecycle::open(&root).unwrap().read();
    match &survivor {
        ControlState::Intent(intent) if intent.attempt_id == "attempt-r1" => {
            assert_eq!((written, replayed), (1, 5), "{outcomes:?}");
            assert_eq!(refused, 2, "{outcomes:?}");
        }
        ControlState::Intent(intent) => {
            assert!(
                intent.attempt_id.starts_with("attempt-other-"),
                "{intent:?}"
            );
            assert_eq!(written, 1, "{outcomes:?}");
            assert_eq!(replayed, 0, "{outcomes:?}");
            assert_eq!(refused, 7, "{outcomes:?}");
        }
        other => panic!("{other:?}"),
    }
    assert!(
        outcomes
            .iter()
            .filter_map(|outcome| outcome.as_ref().err())
            .all(|refusal| matches!(refusal, IntentRefusal::Conflict { .. })),
        "{outcomes:?}"
    );
}

#[test]
fn a_shared_handle_serializes_its_own_threads() {
    let dir = tempfile::tempdir().unwrap();
    let gate = open_gate();
    let lifecycle = ProjectionLifecycle::open(dir.path()).unwrap();
    lifecycle
        .record(
            &gate,
            &LifecycleRequest {
                allowance: 4,
                ..recovery_request()
            },
            NOW,
        )
        .unwrap();
    // The first arrival at `BeforeRename` reports itself and waits to be released; every later arrival passes.
    let (reached_tx, reached_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let park = std::sync::Mutex::new(Some((reached_tx, release_rx)));
    let lifecycle = lifecycle.with_write_barrier_for_test(move |barrier| {
        if barrier != WriteBarrier::BeforeRename {
            return;
        }
        let parked = park.lock().unwrap().take();
        if let Some((reached, release)) = parked {
            reached.send(()).unwrap();
            release.recv().unwrap();
        }
    });
    let outcomes = std::thread::scope(|scope| {
        let first = scope.spawn(|| lifecycle.consume_episode(&gate, NOW + 1));
        reached_rx.recv_timeout(Duration::from_secs(30)).unwrap();
        let (result_tx, result_rx) = mpsc::channel();
        let shared = (&lifecycle, &gate);
        let second = scope.spawn(move || {
            result_tx
                .send(shared.0.consume_episode(shared.1, NOW + 2))
                .unwrap()
        });
        let blocked = result_rx.recv_timeout(Duration::from_secs(30));
        // Released before the assertion so a failure does not leave the parked thread for the scope to wait on.
        release_tx.send(()).unwrap();
        let first = first.join().unwrap();
        second.join().unwrap();
        assert_eq!(blocked.unwrap(), Err(IntentRefusal::WouldBlock));
        [first, lifecycle.consume_episode(&gate, NOW + 2)]
    });
    assert!(
        outcomes.iter().all(Result::is_ok),
        "both episodes are spent: {outcomes:?}"
    );
    assert_eq!(
        lifecycle.read(),
        ControlState::Intent(expected(
            &LifecycleRequest {
                allowance: 4,
                ..recovery_request()
            },
            2
        )),
        "two episodes are consumed, not one written over the other"
    );
}

/// AC5: the lifecycle entry runs under the gate. A closed gate refuses the request and writes nothing; a valid record enables no hook by itself; a corrupt, oversized, or world-readable record is unavailable and refuses both a new request and an episode.
#[test]
fn the_lifecycle_entry_is_gated_and_control_state_never_enables_a_hook() {
    let dir = tempfile::tempdir().unwrap();
    let lifecycle = ProjectionLifecycle::open(dir.path()).unwrap();
    let closed = HookGate::closed();
    assert_eq!(
        lifecycle.record(&closed, &rebuild_request(), NOW),
        Err(IntentRefusal::Denied(Denial::NoManifest))
    );
    assert_eq!(lifecycle.read(), ControlState::Absent);
    assert!(!record_path(dir.path()).exists());
    assert_eq!(
        closed
            .ledger()
            .last()
            .map(|entry| (entry.hook, entry.entry)),
        Some((ProjectionHook::EmbeddingBootstrap, EntryPoint::Reload))
    );

    let open = open_gate();
    lifecycle.record(&open, &rebuild_request(), NOW).unwrap();
    assert_eq!(
        closed
            .admit(ProjectionHook::EmbeddingBootstrap, EntryPoint::Startup)
            .err(),
        Some(Denial::NoManifest),
        "a recorded intent is not evidence"
    );
    // A recovery is judged under the backfill hook, not the bootstrap hook.
    let backfill_only = HookGate::closed();
    backfill_only.install(support::projection_gate::passing_evaluator(
        &support::projection_gate::identity("k", 8),
        0,
        &[ProjectionHook::EmbeddingBootstrap],
    ));
    let other = tempfile::tempdir().unwrap();
    assert_eq!(
        ProjectionLifecycle::open(other.path()).unwrap().record(
            &backfill_only,
            &recovery_request(),
            NOW
        ),
        Err(IntentRefusal::Denied(Denial::Disabled(
            ProjectionHook::EmbeddingBackfill
        )))
    );

    type Corruption = fn(&Path);
    let corruptions: [(&str, Corruption); 8] = [
        ("truncated", |path: &Path| {
            let mut bytes = fs::read(path).unwrap();
            bytes.truncate(bytes.len() / 2);
            fs::write(path, bytes).unwrap();
        }),
        ("unknown field", |path: &Path| {
            let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            value["approved"] = Value::Bool(true);
            fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
        }),
        ("other schema", |path: &Path| {
            let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            value["schema"] = Value::from(3);
            fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
        }),
        ("world readable", |path: &Path| {
            fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
        }),
        ("oversized", |path: &Path| {
            fs::write(path, vec![b' '; 64 * 1024 + 1]).unwrap();
        }),
        ("recovery without authorization", |path: &Path| {
            let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            value["transition"] = Value::from("AuthorizedRecovery");
            fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
        }),
        ("blank consumer", |path: &Path| {
            let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            value["consumer"]["consumer_id"] = Value::from(" ");
            fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
        }),
        ("certificate of another hold", |path: &Path| {
            let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            value["replacement_capture"] = serde_json::json!({
                "hold_id": "b".repeat(32), "snapshot": 40, "lease_epoch": 1,
                "source_policy_version": "source-policy.v1", "expires_at": NOW + 30_000,
                "stage": certificate(&"a".repeat(32)),
            });
            fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
        }),
    ];
    for (name, corrupt) in corruptions {
        let dir = tempfile::tempdir().unwrap();
        let lifecycle = ProjectionLifecycle::open(dir.path()).unwrap();
        lifecycle.record(&open, &rebuild_request(), NOW).unwrap();
        corrupt(&record_path(dir.path()));
        let state = lifecycle.read();
        assert!(
            matches!(state, ControlState::Unavailable(_)),
            "{name}: {state:?}"
        );
        assert!(
            matches!(
                lifecycle.record(&open, &rebuild_request(), NOW),
                Err(IntentRefusal::Unavailable(_))
            ),
            "{name}"
        );
        assert!(
            matches!(
                lifecycle.consume_episode(&open, NOW),
                Err(IntentRefusal::Unavailable(_))
            ),
            "{name}"
        );
        assert!(
            matches!(
                lifecycle.delete_disposable_family(&open, NOW),
                Err(IntentRefusal::Unavailable(_))
            ),
            "{name}: an untrusted record deletes nothing"
        );
    }

    // `record` rejects a request whose serialized record exceeds `MAX_RECORD_BYTES` before writing it.
    let dir = tempfile::tempdir().unwrap();
    let lifecycle = ProjectionLifecycle::open(dir.path()).unwrap();
    assert_eq!(
        lifecycle.record(
            &open,
            &LifecycleRequest {
                attempt_id: "a".repeat(64 * 1024),
                ..rebuild_request()
            },
            NOW
        ),
        Err(IntentRefusal::Oversized)
    );
    assert_eq!(lifecycle.read(), ControlState::Absent);
    assert!(!record_path(dir.path()).exists());

    // A record that fits at `consumed: 0` but not once `consumed` gains a digit is refused up front, so every granted episode stays consumable.
    let dir = tempfile::tempdir().unwrap();
    let lifecycle = ProjectionLifecycle::open(dir.path()).unwrap();
    let base = LifecycleRequest {
        allowance: 10,
        ..rebuild_request()
    };
    let slack = 64 * 1024 - serde_json::to_vec(&expected(&base, 0)).unwrap().len();
    let near_cap = LifecycleRequest {
        attempt_id: format!("{}{}", base.attempt_id, "a".repeat(slack)),
        ..base.clone()
    };
    assert_eq!(
        serde_json::to_vec(&expected(&near_cap, 0)).unwrap().len(),
        64 * 1024
    );
    assert_eq!(
        lifecycle.record(&open, &near_cap, NOW),
        Err(IntentRefusal::Oversized)
    );
    assert_eq!(lifecycle.read(), ControlState::Absent);

    // The record reserves the seed digest it may later pin as well as the terminal `consumed`: a request that fits only without the digest is refused up front, so an accepted intent can always be pinned and every episode consumed.
    let dir = tempfile::tempdir().unwrap();
    let lifecycle = ProjectionLifecycle::open(dir.path()).unwrap();
    let digest = "d".repeat(64);
    let transaction = LifecycleTransactionLock::acquire_exclusive(Some(dir.path())).unwrap();
    let terminal = LifecycleIntent {
        staged_seed_digest: Some(digest.clone()),
        ..expected(&base, base.allowance)
    };
    // One byte short of fitting with the digest at the terminal size; without the digest it fits.
    let slack = 64 * 1024 + 1 - reserved(&terminal);
    let pin_cap = LifecycleRequest {
        attempt_id: format!("{}{}", base.attempt_id, "a".repeat(slack)),
        ..base.clone()
    };
    assert!(reserved(&expected(&pin_cap, base.allowance)) <= 64 * 1024);
    assert_eq!(
        lifecycle.record(&open, &pin_cap, NOW),
        Err(IntentRefusal::Oversized)
    );
    assert_eq!(lifecycle.read(), ControlState::Absent);
    // One byte less padding fits with the digest: the record is accepted, pinned, and consumed to its allowance.
    let fits = LifecycleRequest {
        attempt_id: format!("{}{}", base.attempt_id, "a".repeat(slack - 1)),
        ..base.clone()
    };
    lifecycle.record(&open, &fits, NOW).unwrap();
    let pinned = lifecycle.pin_seed(&open, &transaction, &digest).unwrap();
    assert_eq!(pinned.staged_seed_digest.as_deref(), Some(digest.as_str()));
    for _ in 0..base.allowance {
        lifecycle.consume_episode(&open, NOW).unwrap();
    }

    // A target-less `record` reserves space for `RecoveryTarget { commit_seq: i64::MAX }` so `fix_target` stays within the 64 KiB intent limit.
    let dir = tempfile::tempdir().unwrap();
    let lifecycle = ProjectionLifecycle::open(dir.path()).unwrap();
    let transaction = LifecycleTransactionLock::acquire_exclusive(Some(dir.path())).unwrap();
    let untargeted = LifecycleRequest {
        recovery_target: None,
        ..base.clone()
    };
    let terminal = LifecycleIntent {
        staged_seed_digest: Some(digest.clone()),
        recovery_target: Some(RecoveryTarget {
            commit_seq: i64::MAX,
        }),
        ..expected(&untargeted, base.allowance)
    };
    let slack = 64 * 1024 + 1 - reserved(&terminal);
    let target_cap = LifecycleRequest {
        attempt_id: format!("{}{}", base.attempt_id, "a".repeat(slack)),
        ..untargeted.clone()
    };
    assert_eq!(
        lifecycle.record(&open, &target_cap, NOW),
        Err(IntentRefusal::Oversized)
    );
    assert_eq!(lifecycle.read(), ControlState::Absent);
    let fits = LifecycleRequest {
        attempt_id: format!("{}{}", base.attempt_id, "a".repeat(slack - 1)),
        ..untargeted
    };
    lifecycle.record(&open, &fits, NOW).unwrap();
    let fixed = lifecycle
        .fix_target(
            &open,
            RecoveryTarget {
                commit_seq: i64::MAX,
            },
        )
        .unwrap();
    assert_eq!(
        fixed.recovery_target,
        Some(RecoveryTarget {
            commit_seq: i64::MAX
        })
    );
    lifecycle.pin_seed(&open, &transaction, &digest).unwrap();
    for _ in 0..base.allowance {
        lifecycle.consume_episode(&open, NOW).unwrap();
    }

    // A record accepted by `record` still fits after `disable` adds terminal fields.
    let dir = tempfile::tempdir().unwrap();
    let lifecycle = ProjectionLifecycle::open(dir.path()).unwrap();
    let terminal = LifecycleIntent {
        staged_seed_digest: Some(digest.clone()),
        ..expected(&base, base.allowance)
    };
    let accepted = LifecycleRequest {
        attempt_id: format!(
            "{}{}",
            base.attempt_id,
            "a".repeat(64 * 1024 - reserved(&terminal))
        ),
        ..base.clone()
    };
    // `disable` latches its gate, so this block uses its own.
    let disabling = open_gate();
    lifecycle.record(&disabling, &accepted, NOW).unwrap();
    lifecycle.disable(&disabling, NOW).unwrap();
    let mut stored: Value =
        serde_json::from_slice(&fs::read(record_path(dir.path())).unwrap()).unwrap();
    stored["handoff"] = serde_json::to_value(LifecycleIntent {
        staged_seed_digest: Some(digest.clone()),
        ..expected(&accepted, base.allowance)
    })
    .unwrap();
    stored["recorded_at"] = json!(i64::MAX);
    stored["episodes"] = json!({
        "allowance": u32::MAX,
        "consumed": u32::MAX,
        "deadline": i64::MAX,
    });
    stored["through"] = json!(i64::MAX);
    stored["deregistered"] = json!(true);
    assert!(serde_json::to_vec(&stored).unwrap().len() <= 64 * 1024);

    // A well-formed record the daemon did not write: a symlink to one.
    let dir = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let lifecycle = ProjectionLifecycle::open(dir.path()).unwrap();
    ProjectionLifecycle::open(elsewhere.path())
        .unwrap()
        .record(&open, &rebuild_request(), NOW)
        .unwrap();
    std::os::unix::fs::symlink(record_path(elsewhere.path()), record_path(dir.path())).unwrap();
    assert!(matches!(lifecycle.read(), ControlState::Unavailable(_)));
    assert!(matches!(
        lifecycle.consume_episode(&open, NOW),
        Err(IntentRefusal::Unavailable(_))
    ));

    // A schema 1 record has no `staged_seed_digest`; it is another schema, not a corrupt record.
    let dir = tempfile::tempdir().unwrap();
    let lifecycle = ProjectionLifecycle::open(dir.path()).unwrap();
    lifecycle.record(&open, &rebuild_request(), NOW).unwrap();
    let path = record_path(dir.path());
    let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["schema"] = Value::from(1);
    value.as_object_mut().unwrap().remove("staged_seed_digest");
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(
        lifecycle.read(),
        ControlState::Unavailable("schema 1".to_owned()),
        "a schema 1 record is refused through the schema arm"
    );

    // Authorization withdrawn after the fact: a gate that no longer enables the backfill hook stops the recorded recovery from spending episodes or deleting the database.
    let dir = tempfile::tempdir().unwrap();
    let lifecycle = ProjectionLifecycle::open(dir.path()).unwrap();
    lifecycle.record(&open, &recovery_request(), NOW).unwrap();
    let withdrawn = HookGate::closed();
    withdrawn.install(passing_evaluator(
        &identity("k", 8),
        0,
        &[ProjectionHook::EmbeddingBootstrap],
    ));
    assert_eq!(
        lifecycle.consume_episode(&withdrawn, NOW),
        Err(IntentRefusal::Denied(Denial::Disabled(
            ProjectionHook::EmbeddingBackfill
        )))
    );
    assert_eq!(
        lifecycle.delete_disposable_family(&withdrawn, NOW),
        Err(IntentRefusal::Denied(Denial::Disabled(
            ProjectionHook::EmbeddingBackfill
        )))
    );
    assert_eq!(raw_record(dir.path()), recovery_ledger(0));

    // Revoking the gate at `BeforeRename` or `BeforeFamilyRemoval` prevents the record update or family removal.
    let dir = tempfile::tempdir().unwrap();
    let racing = open_gate();
    let closer = racing.clone();
    let lifecycle = ProjectionLifecycle::open(dir.path())
        .unwrap()
        .with_write_barrier_for_test(move |barrier| {
            if barrier == WriteBarrier::BeforeRename {
                closer.close();
            }
        });
    assert_eq!(
        lifecycle.record(&racing, &rebuild_request(), NOW),
        Err(IntentRefusal::Revoked)
    );
    assert_eq!(lifecycle.read(), ControlState::Absent);
    assert_eq!(
        fs::read_dir(dir.path().join(CONTROL_DIR)).unwrap().count(),
        0
    );

    let dir = tempfile::tempdir().unwrap();
    SearchProjection::open(dir.path()).unwrap();
    let racing = open_gate();
    let closer = racing.clone();
    let lifecycle = ProjectionLifecycle::open(dir.path())
        .unwrap()
        .with_write_barrier_for_test(move |barrier| {
            if barrier == WriteBarrier::BeforeFamilyRemoval {
                closer.close();
            }
        });
    lifecycle.record(&racing, &rebuild_request(), NOW).unwrap();
    assert_eq!(
        lifecycle.delete_disposable_family(&racing, NOW),
        Err(IntentRefusal::Revoked)
    );
    assert!(dir.path().join("search").join("search.sqlite").exists());
}

// ---- Process cuts at the record's write barriers ---------------------------

fn cut_name(barrier: WriteBarrier) -> &'static str {
    match barrier {
        WriteBarrier::BeforeRename => "before-rename",
        WriteBarrier::AfterRename => "after-rename",
        WriteBarrier::AfterDirectorySync => "after-directory-sync",
        WriteBarrier::BeforeFamilyRemoval => "before-family-removal",
    }
}

fn parse_cut(name: &str) -> WriteBarrier {
    match name {
        "before-rename" => WriteBarrier::BeforeRename,
        "after-rename" => WriteBarrier::AfterRename,
        "after-directory-sync" => WriteBarrier::AfterDirectorySync,
        other => panic!("unknown cut {other}"),
    }
}

/// The child records the prior intent, then consumes one episode and parks at the named barrier of that rewrite until the parent kills it.
#[test]
#[ignore = "re-executed by the process-cut test with its environment set"]
fn cut_child_entrypoint_reexecuted_by_the_parent() {
    let (root, cut) = match (std::env::var(CHILD_ROOT), std::env::var(CHILD_CUT)) {
        (Err(std::env::VarError::NotPresent), Err(std::env::VarError::NotPresent)) => return,
        (Ok(root), Ok(cut)) => (root, cut),
        (root, cut) => panic!("incomplete child environment: {root:?}, {cut:?}"),
    };
    let cut = parse_cut(&cut);
    let root = PathBuf::from(root);
    let gate = open_gate();
    let lifecycle = ProjectionLifecycle::open(&root).unwrap();
    lifecycle.record(&gate, &recovery_request(), NOW).unwrap();
    let lifecycle = lifecycle.with_write_barrier_for_test(move |barrier| {
        if barrier == cut {
            let mut stdout = std::io::stdout().lock();
            writeln!(stdout, "{CHILD_BARRIER} {}", cut_name(cut)).unwrap();
            stdout.flush().unwrap();
            drop(stdout);
            loop {
                std::thread::park();
            }
        }
    });
    let accounting = lifecycle.consume_episode(&gate, NOW).unwrap();
    panic!("the child was not killed at its barrier: {accounting:?}");
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn run_cut_child(root: &Path, cut: WriteBarrier) {
    let mut child = ChildGuard(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "cut_child_entrypoint_reexecuted_by_the_parent",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_ROOT, root)
            .env(CHILD_CUT, cut_name(cut))
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let stdout = child.0.stdout.take().unwrap();
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let barrier = BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
            .find(|line| line.contains(CHILD_BARRIER));
        let _ = tx.send(barrier);
    });
    let barrier = rx.recv_timeout(Duration::from_secs(120)).unwrap().unwrap();
    // The harness prints the test's name ahead of its first output line.
    assert!(
        barrier.ends_with(&format!("{CHILD_BARRIER} {}", cut_name(cut))),
        "{barrier}"
    );
    drop(child);
}

/// AC4: killed at each barrier of the rewrite, the record read after two reopens is the complete prior record before the rename and the complete new record after it; no reopen sees a field of one and a field of the other, and a temp file left by the cut is never adopted.
#[test]
fn process_cuts_at_every_write_barrier_leave_a_complete_prior_or_new_record() {
    for cut in [
        WriteBarrier::BeforeRename,
        WriteBarrier::AfterRename,
        WriteBarrier::AfterDirectorySync,
    ] {
        let dir = tempfile::tempdir().unwrap();
        run_cut_child(dir.path(), cut);
        let consumed = match cut {
            WriteBarrier::BeforeRename | WriteBarrier::BeforeFamilyRemoval => 0,
            WriteBarrier::AfterRename | WriteBarrier::AfterDirectorySync => 1,
        };
        let temps_before: Vec<_> = fs::read_dir(dir.path().join(CONTROL_DIR))
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert_eq!(
            temps_before.len(),
            usize::from(cut == WriteBarrier::BeforeRename),
            "{cut:?}: the temp exists only while the rename is pending"
        );
        for reopen in 0..2 {
            let lifecycle = ProjectionLifecycle::open(dir.path()).unwrap();
            assert_eq!(
                lifecycle.read(),
                ControlState::Intent(expected(&recovery_request(), consumed)),
                "{cut:?} reopen {reopen}"
            );
            let entries: Vec<String> = fs::read_dir(dir.path().join(CONTROL_DIR))
                .unwrap()
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect();
            assert_eq!(
                entries,
                vec![CONTROL_RECORD.to_owned()],
                "{cut:?} reopen {reopen}"
            );
        }
        let lifecycle = ProjectionLifecycle::open(dir.path()).unwrap();
        let after = lifecycle.consume_episode(&open_gate(), NOW).unwrap();
        assert_eq!(after.consumed, consumed + 1);
    }
}

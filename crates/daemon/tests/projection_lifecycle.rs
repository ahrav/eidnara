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

fn recovery_request() -> LifecycleRequest {
    LifecycleRequest {
        transition: Transition::AuthorizedRecovery,
        cause: Cause::DisabledRecovery,
        attempt_id: "attempt-r1".to_owned(),
        authorization_ref: Some("ops:ticket-77".to_owned()),
        ..rebuild_request()
    }
}

/// The record the test expects, built from the request alone.
fn expected(request: &LifecycleRequest, consumed: u32) -> LifecycleIntent {
    LifecycleIntent {
        schema: 1,
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
        recorded_at: NOW,
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
        "schema": 1,
        "transition": "AuthorizedRecovery",
        "selected_generation": "gen-2",
        "kernel_incarnation_id": "incarnation-a",
        "consumer": { "consumer_id": "search", "generation_id": "gen-2" },
        "cause": "DisabledRecovery",
        "attempt_id": "attempt-r1",
        "recovery_target": { "commit_seq": 41 },
        "episodes": { "allowance": 2, "consumed": consumed, "deadline": NOW + 60_000 },
        "authorization_ref": "ops:ticket-77",
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
    let identity = projection.read(retrieval::read_identity).unwrap();
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
                    let lifecycle = ProjectionLifecycle::open(&root).unwrap();
                    let request = if worker % 4 == 3 {
                        LifecycleRequest {
                            attempt_id: format!("attempt-other-{worker}"),
                            ..recovery_request()
                        }
                    } else {
                        recovery_request()
                    };
                    lifecycle
                        .record(&gate, &request, NOW)
                        .map(|recorded| recorded.replayed)
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

/// Two threads sharing one handle spend the allowance one episode at a time. The first writer is parked after it read `consumed = 0` and before its rename; a second writer on the same handle must wait for it rather than read the same `consumed = 0`, or the first writer's rename would erase the second's episode.
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
        // The first writer holds the handle's lock while parked; the second must block here.
        let second = scope.spawn(|| lifecycle.consume_episode(&gate, NOW + 2));
        std::thread::sleep(Duration::from_millis(300));
        let second_ran_early = second.is_finished();
        // Released before the assertion so a failure does not leave the parked thread for the scope to wait on.
        release_tx.send(()).unwrap();
        assert!(
            !second_ran_early,
            "the second writer ran while the first held the lock"
        );
        [first.join().unwrap(), second.join().unwrap()]
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
    let corruptions: [(&str, Corruption); 5] = [
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
            value["schema"] = Value::from(2);
            fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
        }),
        ("world readable", |path: &Path| {
            fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
        }),
        ("oversized", |path: &Path| {
            fs::write(path, vec![b' '; 64 * 1024 + 1]).unwrap();
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
}

// ---- Process cuts at the record's write barriers ---------------------------

fn cut_name(barrier: WriteBarrier) -> &'static str {
    match barrier {
        WriteBarrier::BeforeRename => "before-rename",
        WriteBarrier::AfterRename => "after-rename",
        WriteBarrier::AfterDirectorySync => "after-directory-sync",
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
            WriteBarrier::BeforeRename => 0,
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

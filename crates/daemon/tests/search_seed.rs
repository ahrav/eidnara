//! Closed search seeds against a real projection and the host lifecycle store: state committed into the WAL survives quiesce, checkpoint, and close and reopens without that WAL against an authored ledger; the staged object carries exactly the verified bytes and report; a checkpoint-blocking reader bounds progress without journal loss and a retry succeeds; active handles, workers, and a closed gate refuse the seed; tampered or truncated bytes fail verification without selecting or removing anything; host staging and validation are unchanged beside a staged seed; and a process cut at every quiesce barrier reopens to the same rows.

mod support;

use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use daemon::embedding_supervisor::{
    EmbeddingSupervisor, Maintained, SliceBounds, SliceKind, SliceOutcome, SupervisorEvent,
};
use daemon::projection_gates::{Denial, HookGate};
use daemon::projection_lifecycle::{
    Cause, ConsumerBinding, IntentRefusal, LifecycleRequest, ProjectionLifecycle, RecoveryTarget,
    Transition,
};
use daemon::search_projection::SearchProjection;
use daemon::search_seed::{
    ClosedSeed, SEED_FILE, SEED_REPORT_FILE, SEED_TARGET, SeedBarrier, SeedBounds, SeedRefusal,
    quiesce, quiesce_with_barrier_for_test, stage, verify_closed,
};
use host_runtime::generation::{CurrentProfile, GenerationStore, SourceSpec, StageMeta};
use host_runtime::synapse::SynapseLimits;
use kernel::applicability::EvalBudget;
use kernel::{ArtifactDestination, ProjectScope};
use retrieval::ProjectionIdentity;
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::embedding_fixtures::{
    Corpus, NOW, PROJECT, TestEngine, bounds, component, identity, kernel_incarnation_id,
    search_path,
};
use support::projection_gate::open_gate;
use tokio::sync::mpsc::unbounded_channel;

const CHILD_ROOT: &str = "EIDNARA_SEARCH_SEED_CHILD_ROOT";
const CHILD_CUT: &str = "EIDNARA_SEARCH_SEED_CHILD_CUT";
const CHILD_BARRIER: &str = "EIDNARA_SEARCH_SEED_BARRIER";
const CHILD_LEDGER: &str = "EIDNARA_SEARCH_SEED_LEDGER";

fn seed_bounds() -> SeedBounds {
    SeedBounds {
        checkpoint_attempts: NonZeroU32::new(3).unwrap(),
        attempt_wait: Duration::from_millis(20),
        max_bytes: 64 << 20,
    }
}

fn unbounded() -> EvalBudget {
    EvalBudget::new(
        Some(std::time::Instant::now() + Duration::from_secs(60)),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
}

/// The rows, tombstones, and work of a closed database, read on a connection of the test's own that neither creates nor replays a log.
fn independent_state(path: &Path) -> (Vec<String>, i64, Vec<(String, String)>, i64) {
    let conn = Connection::open_with_flags(
        format!("file:{}?immutable=1", path.display()),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .unwrap();
    let occurrences: Vec<String> = conn
        .prepare("SELECT occurrence_id FROM occurrences ORDER BY occurrence_id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let tombstones: i64 = conn
        .query_row("SELECT count(*) FROM occurrence_tombstones", [], |row| {
            row.get(0)
        })
        .unwrap();
    let jobs: Vec<(String, String)> = conn
        .prepare("SELECT occurrence_id,state FROM embedding_jobs ORDER BY occurrence_id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let vectors: i64 = conn
        .query_row("SELECT count(*) FROM occurrence_vectors", [], |row| {
            row.get(0)
        })
        .unwrap();
    (occurrences, tombstones, jobs, vectors)
}

fn sha256_of(path: &Path) -> String {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn wal_len(path: &Path) -> u64 {
    fs::metadata(path.with_extension("sqlite-wal")).map_or(0, |metadata| metadata.len())
}

/// A projection with one embedded row and one pending row, its commits sitting in the WAL.
struct Fixture {
    corpus: Corpus,
    projection: Arc<SearchProjection>,
    embedded: String,
    pending: String,
    expected: ProjectionIdentity,
    snapshot: i64,
}

async fn embedded_fixture(root: &Path) -> Fixture {
    let corpus = Corpus::open(root);
    corpus.seed();
    let embedded_object = corpus.publish("embedded", "embedded text");
    let (projection, rows) = corpus.bootstrap(root);
    let snapshot = corpus.tip();
    let embedded = support::embedding_fixtures::occurrence_of(&rows, &embedded_object).to_string();
    let projection = Arc::new(projection);
    let engine = TestEngine::new();
    let synapse = Arc::new(component(&engine, SynapseLimits::default()));
    let (sender, mut events) = unbounded_channel();
    let supervisor = EmbeddingSupervisor::new(
        Maintained {
            gate: open_gate(),
            kernel: Arc::clone(&corpus.kernel),
            projection: Arc::clone(&projection),
            synapse,
            project: ProjectScope::new(PROJECT).unwrap(),
            destination: ArtifactDestination::Remote,
        },
        SliceBounds {
            dispatch: bounds(),
            sweep_candidates: std::num::NonZeroUsize::new(16).unwrap(),
            slice: Duration::from_secs(10),
            idle: Duration::from_millis(20),
        },
        Arc::new(|| NOW),
        sender,
    );
    let running = tokio::spawn(Arc::clone(&supervisor).run());
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match events.recv().await.unwrap() {
                SupervisorEvent::SliceEnded {
                    kind: SliceKind::Backfill,
                    outcome: SliceOutcome::Backfill { published: 1, .. },
                } => break,
                SupervisorEvent::Stopped(stop) => panic!("{stop:?}"),
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    supervisor.shutdown(Duration::from_secs(5)).await.unwrap();
    running.await.unwrap();
    drop(supervisor);
    // A second row published after the backfill has durable pending work and no vector.
    let pending_object = corpus.publish("pending", "pending text");
    let rows = corpus.export();
    let pending = support::embedding_fixtures::occurrence_of(&rows, &pending_object).to_string();
    let identities = retrieval::batch::row_identities(&rows);
    let batch = retrieval::batch::batch_from_rows(
        &rows,
        &identities,
        retrieval::batch::MutationIdentity {
            kernel_incarnation_id: kernel_incarnation_id(root),
            hold_id: "0123456789abcdef0123456789abcdef".to_string(),
            snapshot_commit_seq: snapshot,
            through_commit_seq: corpus.tip(),
        },
        Some(support::embedding_fixtures::GENERATION),
    )
    .unwrap();
    projection
        .apply_batch(&batch, support::embedding_fixtures::batch_bounds(), 3)
        .unwrap();
    let expected = identity(&kernel_incarnation_id(root));
    Fixture {
        corpus,
        projection,
        embedded,
        pending,
        expected,
        snapshot,
    }
}

fn store_root() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn host_meta() -> StageMeta {
    StageMeta {
        target: "linux-x64-gnu".to_owned(),
        release_contract_sha256: "a".repeat(64),
        inputs_lock_sha256: "b".repeat(64),
        source_payload_manifest_sha256: "unqualified-dev-manifest".to_owned(),
    }
}

/// AC1, AC3, AC6: committed state in a real WAL reopens without that WAL after quiesce and matches the authored ledger; the staged object holds exactly the verified bytes and report under the seed's identity; staging the same seed again finds the same object; staging selects nothing while a host generation promoted beside it does; protected reclamation keeps the seed.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_quiesced_seed_reopens_without_its_wal_and_stages_exactly_its_verified_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = embedded_fixture(dir.path()).await;
    let path = search_path(dir.path());
    assert!(wal_len(&path) > 0, "the commits sit in the WAL");
    let ledger_rows = {
        let mut rows = vec![fixture.embedded.clone(), fixture.pending.clone()];
        rows.sort();
        rows
    };
    let ledger_jobs = {
        let mut jobs = vec![
            (fixture.embedded.clone(), "embedded".to_owned()),
            (fixture.pending.clone(), "pending".to_owned()),
        ];
        jobs.sort();
        jobs
    };

    let gate = open_gate();
    let mut wal_after_checkpoint = None;
    let seed = quiesce_with_barrier_for_test(
        fixture.projection,
        &gate,
        0,
        &fixture.expected,
        seed_bounds(),
        &unbounded(),
        &mut |barrier| {
            if barrier == SeedBarrier::AfterCheckpoint {
                wal_after_checkpoint = Some(wal_len(&path));
            }
        },
    )
    .unwrap();
    assert_eq!(
        wal_after_checkpoint,
        Some(0),
        "the checkpoint itself truncated the log, before the close"
    );
    assert!(!path.with_extension("sqlite-wal").exists());
    assert!(!path.with_extension("sqlite-shm").exists());
    assert_eq!(
        independent_state(&path),
        (ledger_rows.clone(), 0, ledger_jobs.clone(), 1)
    );
    let digest = sha256_of(&path);
    assert_eq!(seed.verification.sha256, digest);
    assert_eq!(
        (
            seed.verification.occurrences,
            seed.verification.tombstones,
            seed.verification.pending_jobs,
            seed.verification.vectors,
            seed.verification.generation_id.as_str(),
        ),
        (2, 0, 1, 1, support::embedding_fixtures::GENERATION)
    );
    assert_eq!(seed.verification.identity(), fixture.expected);
    assert_eq!(
        seed.verification.checkpoint_commit_seq,
        fixture.corpus.tip()
    );

    let root = store_root();
    let store = GenerationStore::open(Some(root.path())).unwrap();
    let staged = stage(&seed, &store, dir.path(), &BTreeSet::new()).unwrap();
    assert_eq!(
        store.read_current().unwrap(),
        CurrentProfile::Absent,
        "staging selects nothing"
    );
    let validated = store.validate(&staged.digest).unwrap();
    assert_eq!(validated.manifest.target, SEED_TARGET);
    assert_eq!(
        validated.manifest.source_payload_manifest_sha256.as_deref(),
        Some(digest.as_str())
    );
    let staged_paths: Vec<&str> = validated
        .manifest
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    assert_eq!(staged_paths, [SEED_FILE, SEED_REPORT_FILE]);
    let staged_seed = validated.descriptor_root_path().join(SEED_FILE);
    assert_eq!(sha256_of(&staged_seed), digest);
    assert_eq!(
        independent_state(&staged_seed),
        (ledger_rows, 0, ledger_jobs, 1),
        "the staged object reopens on its own to the ledger"
    );
    let report: Value = serde_json::from_slice(
        &fs::read(validated.descriptor_root_path().join(SEED_REPORT_FILE)).unwrap(),
    )
    .unwrap();
    assert_eq!(
        report,
        json!({
            "schema": 1,
            "schema_version": retrieval::SCHEMA_VERSION,
            "kernel_incarnation_id": kernel_incarnation_id(dir.path()),
            "projection_policy_version": support::projection_gate::POLICY,
            "identity_contract_version": support::projection_gate::CONTRACT,
            "limit_manifest_protocol_version": support::projection_gate::LIMITS,
            "embedding_model": support::projection_gate::MODEL,
            "tokenizer_fingerprint": support::projection_gate::FINGERPRINT,
            "vector_dimension": 8,
            "generation_epoch": 1,
            "generation_id": support::embedding_fixtures::GENERATION,
            "generation_state": "building",
            "snapshot_commit_seq": fixture.snapshot,
            "checkpoint_commit_seq": fixture.corpus.tip(),
            "occurrences": 2,
            "tombstones": 0,
            "pending_jobs": 1,
            "admitted_jobs": 0,
            "vectors": 1,
            "bytes": fs::metadata(&path).unwrap().len(),
            "sha256": digest,
        })
    );

    // A lost response: staging the same seed again finds the same object and publishes nothing twice.
    let again = stage(&seed, &store, dir.path(), &BTreeSet::new()).unwrap();
    assert_eq!(again.digest, staged.digest);
    let generations = fs::read_dir(store.root().join("generations"))
        .unwrap()
        .flatten()
        .count();
    assert_eq!(generations, 1);

    // A host generation promoted beside the seed: host validation is what it was, the seed stays unselected, and protected reclamation keeps both.
    let host_src = tempfile::tempdir().unwrap();
    fs::write(host_src.path().join("model.bin"), b"model").unwrap();
    let host_digest = store
        .stage_and_promote(
            &[SourceSpec {
                rel_path: "model.bin".to_owned(),
                source: host_src.path().join("model.bin"),
                executable: false,
                expected_size: None,
                expected_sha256: None,
            }],
            &host_meta(),
            &BTreeSet::new(),
        )
        .unwrap();
    assert_eq!(
        store.read_current().unwrap(),
        CurrentProfile::Current(host_digest.clone())
    );
    assert_eq!(
        store.validate(&host_digest).unwrap().manifest.target,
        "linux-x64-gnu"
    );
    // The seed's digest is pinned to the lifecycle intent; the reclaimer's protected set is read from there.
    let lifecycle = ProjectionLifecycle::open(dir.path()).unwrap();
    lifecycle
        .record(
            &gate,
            &LifecycleRequest {
                transition: Transition::Rebuilding,
                selected_generation: support::embedding_fixtures::GENERATION.to_owned(),
                kernel_incarnation_id: kernel_incarnation_id(dir.path()),
                consumer: ConsumerBinding {
                    consumer_id: "search".to_owned(),
                    generation_id: support::embedding_fixtures::GENERATION.to_owned(),
                },
                cause: Cause::DeletedAfterPruning,
                attempt_id: "attempt-seed".to_owned(),
                recovery_target: Some(RecoveryTarget {
                    commit_seq: fixture.corpus.tip(),
                }),
                allowance: 1,
                deadline: NOW + 60_000,
                authorization_ref: None,
            },
            NOW,
        )
        .unwrap();
    let pinned = lifecycle.pin_seed(&gate, &staged.digest).unwrap();
    assert_eq!(
        pinned.staged_seed_digest.as_deref(),
        Some(staged.digest.as_str())
    );
    assert_eq!(
        lifecycle.pin_seed(&gate, &host_digest).unwrap_err(),
        IntentRefusal::Conflict {
            attempt_id: "attempt-seed".to_owned()
        },
        "one transition builds from one seed"
    );
    let protected: BTreeSet<String> = pinned.staged_seed_digest.into_iter().collect();
    let pruned = store.prune(&protected).unwrap();
    assert_eq!(pruned.removed_generations, 0);
    store.validate(&staged.digest).unwrap();
    let pruned = store.prune(&BTreeSet::new()).unwrap();
    assert_eq!(
        pruned.removed_generations, 1,
        "an unprotected seed is reclaimed"
    );
    assert!(store.validate(&staged.digest).is_err());
    store.validate(&host_digest).unwrap();
}

/// AC2: a reader holding a snapshot blocks the checkpoint; the seed reports bounded blocked progress, the log is intact, nothing is staged, and the projection is handed back; releasing the reader lets a retry succeed.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_checkpoint_blocking_reader_bounds_progress_and_a_retry_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = embedded_fixture(dir.path()).await;
    let path = search_path(dir.path());
    let before = wal_len(&path);
    assert!(before > 0);
    let reader = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    reader
        .execute_batch("BEGIN; SELECT count(*) FROM occurrences;")
        .unwrap();

    let gate = open_gate();
    let refused = quiesce(
        fixture.projection,
        &gate,
        0,
        &fixture.expected,
        seed_bounds(),
        &unbounded(),
    )
    .unwrap_err();
    match refused.refusal {
        SeedRefusal::CheckpointBlocked {
            attempts,
            wal_frames,
            checkpointed,
        } => {
            // The reader's snapshot holds the log open: its frames may all be copied into the database, but the log cannot be reset while the snapshot is read from it.
            assert_eq!(attempts, 3);
            assert!(wal_frames > 0, "{wal_frames}");
            assert!(checkpointed <= wal_frames, "{checkpointed} <= {wal_frames}");
        }
        other => panic!("{other:?}"),
    }
    let projection = refused.projection.unwrap();
    assert!(
        wal_len(&path) > 0,
        "a blocked checkpoint removes no journal"
    );
    // The projection still serves.
    let count: i64 = projection
        .read(|conn| Ok(conn.query_row("SELECT count(*) FROM occurrences", [], |row| row.get(0))?))
        .unwrap();
    assert_eq!(count, 2);

    reader.execute_batch("COMMIT;").unwrap();
    drop(reader);
    let seed = quiesce(
        projection,
        &gate,
        0,
        &fixture.expected,
        seed_bounds(),
        &unbounded(),
    )
    .unwrap();
    assert_eq!(wal_len(&path), 0);
    assert_eq!(seed.verification.occurrences, 2);
}

/// AC5: another handle, a running worker, or a closed gate refuses the seed before the checkpoint, and the projection is returned untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn active_handles_workers_and_a_closed_gate_refuse_the_seed() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = embedded_fixture(dir.path()).await;
    let path = search_path(dir.path());
    let before = wal_len(&path);
    let gate = open_gate();

    let refused = quiesce(
        Arc::clone(&fixture.projection),
        &HookGate::closed(),
        0,
        &fixture.expected,
        seed_bounds(),
        &unbounded(),
    )
    .unwrap_err();
    assert_eq!(refused.refusal, SeedRefusal::Denied(Denial::NoManifest));
    assert!(refused.projection.is_some());
    drop(refused);

    let refused = quiesce(
        Arc::clone(&fixture.projection),
        &gate,
        2,
        &fixture.expected,
        seed_bounds(),
        &unbounded(),
    )
    .unwrap_err();
    assert_eq!(refused.refusal, SeedRefusal::ActiveWorkers(2));
    assert!(refused.projection.is_some());
    drop(refused);

    let other = Arc::clone(&fixture.projection);
    let refused = quiesce(
        Arc::clone(&fixture.projection),
        &gate,
        0,
        &fixture.expected,
        seed_bounds(),
        &unbounded(),
    )
    .unwrap_err();
    assert_eq!(refused.refusal, SeedRefusal::ActiveConnections(2));
    drop(refused);
    drop(other);
    assert_eq!(wal_len(&path), before, "a refused seed checkpoints nothing");

    let seed = quiesce(
        fixture.projection,
        &gate,
        0,
        &ProjectionIdentity {
            embedding_model: "other".to_owned(),
            ..fixture.expected.clone()
        },
        seed_bounds(),
        &unbounded(),
    );
    let refused = seed.unwrap_err();
    assert_eq!(refused.refusal, SeedRefusal::IdentityMismatch);
    assert!(
        refused.projection.is_none(),
        "the file is closed; the caller reopens it"
    );
}

/// AC5: a readable database with a corrupt identity, a missing checkpoint, a lost pending row, or truncated bytes fails verification or staging; the store selects nothing and stages nothing for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn corrupt_identity_missing_work_or_truncated_bytes_fail_without_selecting_anything() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = embedded_fixture(dir.path()).await;
    let path = search_path(dir.path());
    let gate = open_gate();
    let seed = quiesce(
        fixture.projection,
        &gate,
        0,
        &fixture.expected,
        seed_bounds(),
        &unbounded(),
    )
    .unwrap();
    let root = store_root();
    let store = GenerationStore::open(Some(root.path())).unwrap();
    let pristine = fs::read(&path).unwrap();
    assert!(
        SearchProjection::open(dir.path()).is_err(),
        "the held seed keeps the projection's lease"
    );
    let verification = seed.verification.clone();
    let path = seed.release();
    let reopen_seed = || ClosedSeed::for_test(path.clone(), verification.clone());

    let tamper = |sql: &str| {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(sql).unwrap();
        drop(conn);
        // The tampering connection leaves no log of its own behind.
        let _ = fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite-shm"));
    };
    let cases: [(&str, &str, SeedRefusal); 6] = [
        (
            "corrupt identity",
            "UPDATE projection_identity SET embedding_model='other'",
            SeedRefusal::IdentityMismatch,
        ),
        (
            "missing checkpoint",
            "DELETE FROM projection_checkpoint",
            SeedRefusal::Checkpoint,
        ),
        (
            "missing pending work",
            "PRAGMA foreign_keys=OFF; DELETE FROM embedding_jobs WHERE state='pending'",
            SeedRefusal::BytesChanged,
        ),
        (
            "missing generation",
            "UPDATE vector_generations SET embedding_model='other'",
            SeedRefusal::Generation,
        ),
        (
            "admitted work",
            "UPDATE embedding_jobs SET state='admitted' WHERE state='pending'",
            SeedRefusal::AdmittedWork(1),
        ),
        (
            "orphan vector",
            "PRAGMA foreign_keys=OFF; INSERT INTO occurrence_vectors(occurrence_id,generation_id,vector,vector_dimension,input_bytes,input_tokens,completed_at) VALUES('ghost','gen-1',zeroblob(32),8,1,1,1)",
            SeedRefusal::ForeignKeys(1),
        ),
    ];
    for (name, sql, refusal) in cases {
        tamper(sql);
        let verification = verify_closed(&path, &fixture.expected, u64::MAX);
        let staged = stage(&reopen_seed(), &store, dir.path(), &BTreeSet::new());
        assert_eq!(staged.unwrap_err(), refusal, "{name}");
        if refusal == SeedRefusal::BytesChanged {
            assert_ne!(verification.unwrap(), reopen_seed().verification, "{name}");
        } else {
            assert_eq!(verification.unwrap_err(), refusal, "{name}");
        }
        assert_eq!(
            store.read_current().unwrap(),
            CurrentProfile::Absent,
            "{name}"
        );
        assert_eq!(
            fs::read_dir(store.root().join("generations"))
                .unwrap()
                .count(),
            0,
            "{name}: nothing staged"
        );
        fs::write(&path, &pristine).unwrap();
    }
    fs::write(&path, &pristine[..pristine.len() / 2]).unwrap();
    assert!(matches!(
        stage(&reopen_seed(), &store, dir.path(), &BTreeSet::new()),
        Err(SeedRefusal::Store(_) | SeedRefusal::Integrity(_) | SeedRefusal::BytesChanged)
    ));
    assert_eq!(
        fs::read_dir(store.root().join("generations"))
            .unwrap()
            .count(),
        0
    );
    fs::write(&path, &pristine).unwrap();
    assert_eq!(
        verify_closed(&path, &fixture.expected, pristine.len() as u64 - 1).unwrap_err(),
        SeedRefusal::TooLarge {
            bytes: pristine.len() as u64,
            max: pristine.len() as u64 - 1,
        }
    );

    // A live connection leaves its sidecars: the file alone is not the database.
    let live = Connection::open(&path).unwrap();
    live.execute_batch("BEGIN; SELECT count(*) FROM occurrences;")
        .unwrap();
    assert_eq!(
        verify_closed(&path, &fixture.expected, u64::MAX).unwrap_err(),
        SeedRefusal::SidecarPresent("sqlite-wal".to_owned())
    );
    live.execute_batch("COMMIT;").unwrap();
    drop(live);

    // Catch-up after the seed was certified: the reopened projection commits more, and the old certificate no longer names the bytes.
    let projection = SearchProjection::open(dir.path()).unwrap();
    projection
        .write(|conn| {
            conn.execute(
                "UPDATE projection_checkpoint SET checkpoint_commit_seq=checkpoint_commit_seq+1, updated_at=9",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    drop(projection);
    assert_eq!(
        stage(&reopen_seed(), &store, dir.path(), &BTreeSet::new()).unwrap_err(),
        SeedRefusal::BytesChanged
    );
    assert_eq!(
        fs::read_dir(store.root().join("generations"))
            .unwrap()
            .count(),
        0
    );
    stage(
        &reopen_seed_after(&path, &fixture.expected),
        &store,
        dir.path(),
        &BTreeSet::new(),
    )
    .unwrap();
}

/// The seed a fresh verification of `path` certifies.
fn reopen_seed_after(path: &Path, expected: &ProjectionIdentity) -> ClosedSeed {
    ClosedSeed::for_test(
        path.to_path_buf(),
        verify_closed(path, expected, u64::MAX).unwrap(),
    )
}

// ---- Process cuts at the quiesce barriers -----------------------------------

fn cut_name(barrier: SeedBarrier) -> &'static str {
    match barrier {
        SeedBarrier::BeforeCheckpoint => "before-checkpoint",
        SeedBarrier::AfterCheckpoint => "after-checkpoint",
        SeedBarrier::AfterClose => "after-close",
    }
}

fn parse_cut(name: &str) -> SeedBarrier {
    match name {
        "before-checkpoint" => SeedBarrier::BeforeCheckpoint,
        "after-checkpoint" => SeedBarrier::AfterCheckpoint,
        "after-close" => SeedBarrier::AfterClose,
        other => panic!("unknown cut {other}"),
    }
}

/// The child builds the fixture, prints its ledger, and quiesces until it parks at the named barrier.
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
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(3)
        .enable_all()
        .build()
        .unwrap();
    let fixture = runtime.block_on(embedded_fixture(&root));
    let mut stdout = std::io::stdout().lock();
    writeln!(
        stdout,
        "{CHILD_LEDGER} {} {}",
        fixture.embedded, fixture.pending
    )
    .unwrap();
    stdout.flush().unwrap();
    drop(stdout);
    let gate = open_gate();
    let outcome = quiesce_with_barrier_for_test(
        fixture.projection,
        &gate,
        0,
        &fixture.expected,
        seed_bounds(),
        &unbounded(),
        &mut |barrier| {
            if barrier == cut {
                let mut stdout = std::io::stdout().lock();
                writeln!(stdout, "{CHILD_BARRIER} {}", cut_name(cut)).unwrap();
                stdout.flush().unwrap();
                drop(stdout);
                loop {
                    std::thread::park();
                }
            }
        },
    );
    panic!("the child was not killed at its barrier: {outcome:?}");
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

fn run_cut_child(root: &Path, cut: SeedBarrier) -> (String, String) {
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
        let mut ledger = None;
        let mut barrier = None;
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(start) = line.find(CHILD_LEDGER) {
                ledger = Some(line[start..].to_string());
            } else if line.contains(CHILD_BARRIER) {
                barrier = Some(line);
                break;
            }
        }
        let _ = tx.send((ledger, barrier));
    });
    let (ledger, barrier) = rx.recv_timeout(Duration::from_secs(120)).unwrap();
    let barrier = barrier.unwrap();
    assert!(
        barrier.ends_with(&format!("{CHILD_BARRIER} {}", cut_name(cut))),
        "{barrier}"
    );
    drop(child);
    let ledger = ledger.unwrap();
    let mut fields = ledger.split(' ').skip(1).map(str::to_owned);
    (fields.next().unwrap(), fields.next().unwrap())
}

/// AC4: killed before or after the checkpoint and after the close, the projection reopens twice through SQLite recovery to exactly the ledger's rows, tombstones, and work, and a later quiesce still closes the same seed.
#[test]
fn process_cuts_at_every_quiesce_barrier_reopen_to_the_ledger() {
    for cut in [
        SeedBarrier::BeforeCheckpoint,
        SeedBarrier::AfterCheckpoint,
        SeedBarrier::AfterClose,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (embedded, pending) = run_cut_child(dir.path(), cut);
        let mut rows = vec![embedded.clone(), pending.clone()];
        rows.sort();
        let mut ledger_jobs = vec![
            (embedded.clone(), "embedded".to_owned()),
            (pending.clone(), "pending".to_owned()),
        ];
        ledger_jobs.sort();
        let path = search_path(dir.path());
        if cut == SeedBarrier::BeforeCheckpoint {
            assert!(wal_len(&path) > 0, "{cut:?}: the log holds the commits");
        }
        for reopen in 0..2 {
            let projection = SearchProjection::open(dir.path()).unwrap();
            let (occurrences, jobs, vectors): (Vec<String>, Vec<(String, String)>, i64) = projection
                .read(|conn| {
                    let occurrences = conn
                        .prepare("SELECT occurrence_id FROM occurrences ORDER BY occurrence_id")?
                        .query_map([], |row| row.get(0))?
                        .collect::<Result<_, _>>()?;
                    let jobs = conn
                        .prepare("SELECT occurrence_id,state FROM embedding_jobs ORDER BY occurrence_id")?
                        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                        .collect::<Result<_, _>>()?;
                    let vectors =
                        conn.query_row("SELECT count(*) FROM occurrence_vectors", [], |row| row.get(0))?;
                    Ok((occurrences, jobs, vectors))
                })
                .unwrap();
            assert_eq!(occurrences, rows, "{cut:?} reopen {reopen}");
            assert_eq!(jobs, ledger_jobs, "{cut:?} reopen {reopen}");
            assert_eq!(vectors, 1, "{cut:?} reopen {reopen}");
            drop(projection);
        }
        let projection = Arc::new(SearchProjection::open(dir.path()).unwrap());
        let expected = identity(&kernel_incarnation_id(dir.path()));
        let seed = quiesce(
            projection,
            &open_gate(),
            0,
            &expected,
            seed_bounds(),
            &unbounded(),
        )
        .unwrap();
        assert_eq!(seed.verification.occurrences, 2, "{cut:?}");
        assert_eq!(seed.verification.vectors, 1, "{cut:?}");
        assert_eq!(wal_len(&path), 0, "{cut:?}");
    }
}

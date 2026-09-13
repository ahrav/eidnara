use super::*;
use daemon::projection_gates::{EntryPoint, ProjectionHook};
use daemon::projection_lifecycle::{DisabledIntent, IntentRefusal};
use daemon::search_replacement::selection::disable::DisableEvent;
use kernel::{ArtifactDeletionIdentity, ArtifactDeletionKind, ArtifactDeletionRequest};
use retrieval::batch::{Invalidation, MutationIdentity, ProjectionBatch, apply_batch};
use sha2::{Digest, Sha256};

fn gate_for(root: &Path) -> Arc<HookGate> {
    let gate = Arc::new(HookGate::for_home(root));
    gate.install(cleanup_evaluator(root));
    gate
}

pub(super) fn cleanup_evaluator(root: &Path) -> daemon::projection_gates::EvidenceEvaluator {
    let mut evaluator =
        support::projection_gate::passing_evaluator(&spec(root).identity, 0, &ProjectionHook::ALL);
    for (key, value) in [
        ("retry_attempts", 6),
        ("physical_drain_ms", 60_000),
        ("B_recovery_ms", 60_000),
    ] {
        evaluator.manifest.limits.insert(key.to_owned(), value);
    }
    evaluator
}

fn disabled(root: &Path) -> DisabledIntent {
    match ProjectionLifecycle::open(root).unwrap().read() {
        ControlState::Disabled(intent) => intent,
        other => panic!("{other:?}"),
    }
}

fn remove_fixture_consumer(corpus: &Corpus) {
    corpus
        .kernel
        .acknowledge_outbox(fixtures::CONSUMER, corpus.tip(), now())
        .unwrap();
    corpus
        .kernel
        .commit(intent("remove-fixture-consumer"), |e| {
            e.deregister_outbox_consumer(fixtures::CONSUMER, now())?;
            Ok(String::new())
        })
        .unwrap();
}

fn admissions(selection: &SearchSelection, corpus: &Corpus, gate: &HookGate) -> (usize, usize) {
    let queries = usize::from(
        selection
            .pin(&corpus.kernel, gate, &budget(Duration::from_secs(10)))
            .is_ok(),
    );
    let hooks = EntryPoint::ALL
        .into_iter()
        .flat_map(|entry| ProjectionHook::ALL.map(|hook| (entry, hook)))
        .filter(|(entry, hook)| gate.admit(*hook, *entry).is_ok())
        .count();
    (queries, hooks)
}

fn apply_prefix(reader: &SearchReader, through: i64, invalidations: Vec<Invalidation>) {
    let checkpoint = reader
        .coverage(&budget(Duration::from_secs(10)))
        .unwrap()
        .checkpoint;
    reader
        .projection()
        .write(|conn| {
            apply_batch(
                conn,
                &ProjectionBatch {
                    identity: MutationIdentity {
                        kernel_incarnation_id: reader.handoff().kernel_incarnation_id.clone(),
                        hold_id: checkpoint.hold_id,
                        snapshot_commit_seq: checkpoint.snapshot_commit_seq,
                        through_commit_seq: through,
                    },
                    records: Vec::new(),
                    invalidations,
                    generation_id: Some(&reader.consumer().generation_id),
                },
                batch_bounds(),
                now(),
            )?;
            Ok(())
        })
        .unwrap();
}

#[test]
fn production_gate_construction_requires_a_home_and_external_stops_cancel_existing_grants() {
    use daemon::projection_gates::Denial;
    let source = include_str!("../../src/projection_gates.rs");
    let constructors: Vec<_> = source
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("pub fn ") && line.ends_with("-> Self {"))
        .collect();
    assert_eq!(
        constructors,
        [
            "pub fn for_home(data_home: &Path) -> Self {",
            "pub fn closed() -> Self {"
        ]
    );
    assert!(source.contains("#[cfg(feature = \"test-support\")]\n    pub fn closed() -> Self"));
    assert!(source.contains("    fn empty() -> Self"));
    for unavailable in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let corpus = Corpus::open(root.path());
        corpus.seed();
        let gate = gate_for(root.path());
        let selection = build_selected(root.path(), &corpus, &gate);
        let grant = gate
            .admit(ProjectionHook::EmbeddingBackfill, EntryPoint::Dispatch)
            .unwrap();
        if unavailable {
            std::fs::write(
                root.path().join("search-lifecycle/intent.json"),
                b"incomplete record",
            )
            .unwrap();
        } else {
            selection
                .begin_disable(&gate_for(root.path()), &mut |_| {})
                .unwrap();
        }
        assert!(!grant.invalidated.is_cancelled());
        assert_eq!(
            gate.admit(ProjectionHook::EmbeddingBackfill, EntryPoint::Dispatch)
                .unwrap_err(),
            Denial::RecoveryRequired
        );
        assert!(grant.invalidated.is_cancelled());
        gate.install(cleanup_evaluator(root.path()));
        assert_eq!(
            gate.admit(ProjectionHook::EmbeddingBackfill, EntryPoint::Dispatch)
                .unwrap_err(),
            Denial::RecoveryRequired
        );
        assert_eq!(
            gate_for(root.path())
                .admit(ProjectionHook::EmbeddingBackfill, EntryPoint::Startup)
                .unwrap_err(),
            Denial::RecoveryRequired
        );
    }
}

#[tokio::test]
async fn cancelled_caller_budget_does_not_consume_an_episode_or_install_an_envelope() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    let gate = gate_for(root.path());
    let mut selection = build_selected(root.path(), &corpus, &gate);
    let before = selection.begin_disable(&gate, &mut |_| {}).unwrap();
    let cancelled = budget(Duration::from_secs(20));
    cancelled.cancel();
    assert!(matches!(
        selection
            .reconcile_disabled(
                &corpus.kernel,
                &gate,
                &spec(root.path()),
                &cancelled,
                &mut |_| {}
            )
            .await,
        Err(BuildError::Expired)
    ));
    assert_eq!(disabled(root.path()), before);
    assert!(
        selection
            .reconcile_disabled(
                &corpus.kernel,
                &gate,
                &spec(root.path()),
                &budget(Duration::from_secs(20)),
                &mut |_| {}
            )
            .await
            .unwrap()
            .deregistered
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_shutdown_and_owner_drop_retain_native_permits_and_pins_until_exit() {
    use daemon::embedding_supervisor::{Maintained, SliceBounds, Stop, SupervisorEvent};
    for drop_owner in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let corpus = Corpus::open(root.path());
        corpus.seed();
        remove_fixture_consumer(&corpus);
        corpus.publish("source", "bytes");
        corpus.publish("next-source", "next bytes");
        let gate = gate_for(root.path());
        let mut selection = build_selected(root.path(), &corpus, &gate);
        let reader = selection
            .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
            .unwrap();
        let weak = Arc::downgrade(reader.projection());
        let path = reader.projection().path().to_owned();
        let seed_directory = std::fs::File::open(
            GenerationStore::open(Some(root.path()))
                .unwrap()
                .root()
                .join("generations")
                .join(reader.digest()),
        )
        .unwrap();
        let engine = fixtures::TestEngine::new();
        let block = engine.block_calls();
        let release = fixtures::GateGuard(Arc::clone(&block));
        let synapse = Arc::new(fixtures::component(
            &engine,
            host_runtime::synapse::SynapseLimits::default(),
        ));
        let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
        selection
            .start_maintenance(
                Maintained {
                    gate: Arc::clone(&gate),
                    kernel: Arc::clone(&corpus.kernel),
                    projection: Arc::clone(reader.projection()),
                    synapse: Arc::clone(&synapse),
                    project: kernel::ProjectScope::new(fixtures::PROJECT).unwrap(),
                    destination: kernel::ArtifactDestination::Remote,
                },
                SliceBounds {
                    dispatch: fixtures::bounds(),
                    sweep_candidates: NonZeroUsize::new(16).unwrap(),
                    slice: Duration::from_secs(30),
                    idle: Duration::from_millis(20),
                },
                Arc::new(|| fixtures::NOW),
                events,
            )
            .unwrap();
        drop(reader);
        let raw = Connection::open(&path).unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            while engine.calls() != 1
                || raw
                    .query_row(
                        "SELECT count(*) FROM embedding_jobs WHERE state='admitted'",
                        [],
                        |r| r.get::<_, i64>(0),
                    )
                    .unwrap()
                    != 1
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let held: (String, i64) = raw
            .query_row(
                "SELECT host_job_id,attempts FROM embedding_jobs WHERE state='admitted'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        if drop_owner {
            drop(selection);
            tokio::time::timeout(Duration::from_secs(10), async {
                while !matches!(
                    received.recv().await.expect("supervisor event stream"),
                    SupervisorEvent::Stopped(Stop::Shutdown)
                ) {}
            })
            .await
            .unwrap();
            assert_eq!((engine.calls(), engine.completed()), (1, 0));
            assert!(weak.upgrade().is_some());
            assert!(
                rustix::fs::flock(
                    &seed_directory,
                    rustix::fs::FlockOperation::NonBlockingLockExclusive
                )
                .is_err()
            );
            assert!(synapse.embed_blocking(&["probe"]).is_err());
            assert_eq!(synapse.job_status(&held.0), Some("running"));
            assert_eq!(
                corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
                Some(corpus.tip())
            );
            drop(release);
            tokio::time::timeout(Duration::from_secs(10), async {
                while weak.upgrade().is_some() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert_eq!((engine.calls(), engine.completed()), (1, 1));
            rustix::fs::flock(
                &seed_directory,
                rustix::fs::FlockOperation::NonBlockingLockExclusive,
            )
            .unwrap();
            assert_eq!(
                raw.query_row(
                    "SELECT count(*) FROM embedding_jobs WHERE state='pending'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                1
            );
            assert!(matches!(
                ProjectionLifecycle::open(root.path()).unwrap().read(),
                ControlState::Intent(_)
            ));
            continue;
        }
        let before = selection.begin_disable(&gate, &mut |_| {}).unwrap();
        let mut entered_drain = false;
        let cancelled = tokio::time::timeout(
            Duration::from_millis(20),
            selection.reconcile_disabled(
                &corpus.kernel,
                &gate,
                &spec(root.path()),
                &budget(Duration::from_secs(20)),
                &mut |event| {
                    if event == DisableEvent::DrainStarted {
                        entered_drain = true;
                    }
                },
            ),
        )
        .await;
        assert!(cancelled.is_err());
        assert!(entered_drain);
        let expired = selection
            .reconcile_disabled(
                &corpus.kernel,
                &gate,
                &spec(root.path()),
                &budget(Duration::from_millis(20)),
                &mut |_| {},
            )
            .await;
        assert!(
            matches!(
                expired,
                Err(BuildError::UnresolvedDrain(
                    daemon::embedding_supervisor::Unresolved { native: 1, .. }
                ))
            ),
            "{expired:?}"
        );
        let waiting = disabled(root.path());
        assert_eq!(waiting.handoff, before.handoff);
        assert_eq!(waiting.episodes.unwrap().consumed, 0);
        assert_eq!((engine.calls(), engine.completed()), (1, 0));
        assert_eq!(synapse.job_status(&held.0), Some("running"));
        assert!(synapse.embed_blocking(&["probe"]).is_err());
        assert!(weak.upgrade().is_some());
        assert!(
            rustix::fs::flock(
                &seed_directory,
                rustix::fs::FlockOperation::NonBlockingLockExclusive
            )
            .is_err()
        );
        assert_eq!(
            raw.query_row(
                "SELECT host_job_id,attempts FROM embedding_jobs WHERE state='admitted'",
                [],
                |r| { Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)) }
            )
            .unwrap(),
            held
        );
        assert_eq!(
            corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
            Some(corpus.tip())
        );
        drop(release);
        let done = selection
            .reconcile_disabled(
                &corpus.kernel,
                &gate,
                &spec(root.path()),
                &budget(Duration::from_secs(20)),
                &mut |_| {},
            )
            .await
            .unwrap();
        assert!(done.deregistered);
        assert_eq!((engine.calls(), engine.completed()), (1, 1));
        assert!(weak.upgrade().is_none());
        rustix::fs::flock(
            &seed_directory,
            rustix::fs::FlockOperation::NonBlockingLockExclusive,
        )
        .unwrap();
        assert_eq!(admissions(&selection, &corpus, &gate), (0, 0));
        assert!(synapse.embed_blocking(&["probe"]).is_ok());
    }
}

#[tokio::test]
async fn absent_limits_and_unauthorized_recovery_preserve_disabled_obligations() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    let gate = gate_for(root.path());
    let mut selection = build_selected(root.path(), &corpus, &gate);
    let original = selection.begin_disable(&gate, &mut |_| {}).unwrap();
    gate.close();
    assert!(matches!(
        selection
            .reconcile_disabled(
                &corpus.kernel,
                &gate,
                &spec(root.path()),
                &budget(Duration::from_secs(20)),
                &mut |_| {}
            )
            .await,
        Err(BuildError::Denied(_))
    ));
    assert_eq!(disabled(root.path()), original);
    let request = LifecycleRequest {
        transition: Transition::AuthorizedRecovery,
        cause: Cause::DisabledRecovery,
        authorization_ref: Some("operator:unapproved".to_owned()),
        ..request(None, &spec(root.path()).identity)
    };
    let fresh = gate_for(root.path());
    assert!(
        ProjectionLifecycle::open(root.path())
            .unwrap()
            .record(&fresh, &request, now())
            .is_err()
    );
    assert_eq!(admissions(&selection, &corpus, &fresh), (0, 0));
}

#[tokio::test]
async fn held_local_transaction_blocks_ack_until_its_actual_release() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    corpus.publish("source", "bytes");
    let gate = gate_for(root.path());
    let mut selection = build_selected(root.path(), &corpus, &gate);
    let reader = selection
        .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
        .unwrap();
    let projection = Arc::clone(reader.projection());
    drop(reader);
    let before = corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap();
    let (entered, at_transaction) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let writer = std::thread::spawn(move || {
        projection
            .write(|conn| {
                conn.execute("UPDATE embedding_jobs SET updated_at=updated_at+1", [])?;
                entered.send(()).unwrap();
                wait.recv().unwrap();
                Ok(())
            })
            .unwrap()
    });
    at_transaction
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    selection.begin_disable(&gate, &mut |_| {}).unwrap();
    let mut events = Vec::new();
    let blocked = selection
        .reconcile_disabled(
            &corpus.kernel,
            &gate,
            &spec(root.path()),
            &budget(Duration::from_secs(20)),
            &mut |e| events.push(e),
        )
        .await;
    assert!(
        matches!(blocked, Err(BuildError::Intent(IntentRefusal::FamilyHeld))),
        "{blocked:?}"
    );
    assert!(!events.contains(&DisableEvent::LocalReleased));
    assert!(!events.contains(&DisableEvent::Acknowledged));
    assert_eq!(
        corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
        before
    );
    release.send(()).unwrap();
    writer.join().unwrap();
    assert!(
        selection
            .reconcile_disabled(
                &corpus.kernel,
                &gate,
                &spec(root.path()),
                &budget(Duration::from_secs(20)),
                &mut |_| {}
            )
            .await
            .unwrap()
            .deregistered
    );
}

#[tokio::test]
async fn reader_and_validation_refusals_keep_the_same_selection_owner_for_retry() {
    for held_reader in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let corpus = Corpus::open(root.path());
        corpus.seed();
        corpus.publish("source", "bytes");
        let gate = gate_for(root.path());
        let mut selection = build_selected(root.path(), &corpus, &gate);
        let reader = selection
            .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
            .unwrap();
        let path = reader.projection().path().to_owned();
        let raw = Connection::open(&path).unwrap();
        let mut reader = Some(reader);
        if !held_reader {
            drop(reader.take());
            raw.execute(
                "UPDATE projection_identity SET tokenizer_fingerprint='wrong'",
                [],
            )
            .unwrap();
        }
        selection
            .begin_disable(&gate_for(root.path()), &mut |_| {})
            .unwrap();
        if let Some(reader) = &reader {
            assert!(reader.coverage(&budget(Duration::from_secs(10))).is_err());
        }
        let before = corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap();
        let failed = selection
            .reconcile_disabled(
                &corpus.kernel,
                &gate,
                &spec(root.path()),
                &budget(Duration::from_secs(20)),
                &mut |_| {},
            )
            .await;
        assert!(failed.is_err());
        if held_reader {
            assert!(matches!(
                failed,
                Err(BuildError::Intent(IntentRefusal::FamilyHeld))
            ));
        }
        drop(reader);
        assert!(
            daemon::search_projection::SearchProjection::open(
                path.parent().unwrap().parent().unwrap()
            )
            .is_err(),
            "the manager still owns the connection after refusal"
        );
        assert_eq!(
            corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
            before
        );
        raw.execute(
            "UPDATE projection_identity SET tokenizer_fingerprint=?1",
            [&spec(root.path()).identity.tokenizer_fingerprint],
        )
        .unwrap();
        assert!(
            selection
                .reconcile_disabled(
                    &corpus.kernel,
                    &gate,
                    &spec(root.path()),
                    &budget(Duration::from_secs(20)),
                    &mut |_| {}
                )
                .await
                .unwrap()
                .deregistered
        );
    }
}

#[tokio::test]
async fn empty_disabled_selection_completes_without_a_consumer_prefix_or_envelope() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    remove_fixture_consumer(&corpus);
    let gate = gate_for(root.path());
    let mut selection = selector(root.path());
    assert!(
        selection
            .begin_disable(&gate, &mut |_| {})
            .unwrap()
            .handoff
            .is_none()
    );
    gate.close();
    let expired = budget(Duration::ZERO);
    for _ in 0..2 {
        let done = selection
            .reconcile_disabled(
                &corpus.kernel,
                &gate,
                &spec(root.path()),
                &expired,
                &mut |_| {},
            )
            .await
            .unwrap();
        assert!(done.handoff.is_none() && done.through.is_none() && done.episodes.is_none());
        assert!(!done.deregistered);
        assert_eq!(disabled(root.path()), done);
        assert_eq!(
            admissions(&selection, &corpus, &gate_for(root.path())),
            (0, 0)
        );
    }
}

#[tokio::test]
async fn missing_handoff_with_a_selected_profile_is_not_empty_completion() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    corpus.publish("source", "bytes");
    let gate = gate_for(root.path());
    let mut selection = build_selected(root.path(), &corpus, &gate);
    selection.begin_disable(&gate, &mut |_| {}).unwrap();
    let path = root.path().join("search-lifecycle/intent.json");
    let mut lost: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    lost["handoff"] = serde_json::Value::Null;
    std::fs::write(&path, serde_json::to_vec(&lost).unwrap()).unwrap();
    let before = disabled(root.path());
    for _ in 0..2 {
        let result = selection
            .reconcile_disabled(
                &corpus.kernel,
                &gate,
                &spec(root.path()),
                &budget(Duration::from_secs(20)),
                &mut |_| {},
            )
            .await;
        assert!(matches!(
            result,
            Err(BuildError::Invalid(
                "disabled handoff missing for owned selection"
            ))
        ));
        assert_eq!(disabled(root.path()), before);
        assert_eq!(
            corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
            Some(corpus.tip())
        );
        selection = selector(root.path());
    }
}

#[tokio::test]
async fn disable_registration_and_barrier_matrix_uses_only_the_released_local_prefix() {
    for lagging in [false, true] {
        for last in [false, true] {
            for barrier in [false, true] {
                let root = tempfile::tempdir().unwrap();
                let corpus = Corpus::open(root.path());
                corpus.seed();
                if last {
                    remove_fixture_consumer(&corpus);
                }
                corpus.publish("source", "source bytes");
                let gate = gate_for(root.path());
                let mut selection = build_selected(root.path(), &corpus, &gate);
                let selected_target = corpus.tip();
                assert_eq!(admissions(&selection, &corpus, &gate), (1, 48));
                let reader = selection
                    .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                    .unwrap();
                let original = reader
                    .coverage(&budget(Duration::from_secs(10)))
                    .unwrap()
                    .checkpoint;
                assert_eq!(original.checkpoint_commit_seq, selected_target);
                let mut invalidations = Vec::new();
                let barrier_id = if barrier {
                    let occurrence = reader
                        .read(&budget(Duration::from_secs(10)), |c| {
                            Ok(
                                c.query_row("SELECT occurrence_id FROM occurrences", [], |r| {
                                    r.get::<_, String>(0)
                                })?,
                            )
                        })
                        .unwrap();
                    let deletion = corpus
                        .kernel
                        .delete_artifact(ArtifactDeletionRequest {
                            intent: intent("delete-source"),
                            identity: ArtifactDeletionIdentity::Digest(format!(
                                "{:x}",
                                Sha256::digest(b"source bytes")
                            )),
                            kind: ArtifactDeletionKind::Delete,
                            operator_id: None,
                            target_locator: None,
                            reason: None,
                            deleted_at: now(),
                        })
                        .unwrap();
                    invalidations.push(Invalidation {
                        occurrence_id: occurrence,
                        tombstone: retrieval::Tombstone {
                            invalidated_commit_seq: deletion.commit_seq,
                            reason: retrieval::TombstoneReason::Retired,
                        },
                    });
                    Some(deletion.barrier_id)
                } else {
                    corpus
                        .kernel
                        .commit(intent("control-only"), |_| Ok(String::new()))
                        .unwrap();
                    None
                };
                let target = if lagging {
                    selected_target
                } else {
                    corpus.tip()
                };
                if !lagging {
                    apply_prefix(&reader, target, invalidations);
                }
                let path = reader.projection().path().to_owned();
                let mut closed = false;
                let recorded = selection
                    .begin_disable(&gate, &mut |event| {
                        if event == DisableEvent::AdmissionClosed {
                            closed = true;
                            assert_eq!(admissions(&selection, &corpus, &gate), (0, 0));
                            assert!(reader.coverage(&budget(Duration::from_secs(10))).is_err());
                        }
                    })
                    .unwrap();
                assert!(closed);
                gate.install(cleanup_evaluator(root.path()));
                assert_eq!(admissions(&selection, &corpus, &gate), (0, 0));
                drop(reader);
                let mut ledger = Vec::new();
                let outcome = selection
                    .reconcile_disabled(
                        &corpus.kernel,
                        &gate,
                        &spec(root.path()),
                        &budget(Duration::from_secs(20)),
                        &mut |e| ledger.push(e),
                    )
                    .await;
                if lagging {
                    assert!(
                        matches!(
                            outcome,
                            Err(BuildError::Kernel(kernel::KernelError::ConsumerPending))
                        ),
                        "{outcome:?}"
                    );
                    assert_eq!(
                        corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
                        Some(target)
                    );
                } else {
                    assert!(outcome.unwrap().deregistered);
                    assert_eq!(
                        corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
                        None
                    );
                    assert!(ledger.contains(&DisableEvent::Deregistered));
                }
                assert!(
                    ledger
                        .iter()
                        .position(|e| *e == DisableEvent::LocalReleased)
                        .unwrap()
                        < ledger
                            .iter()
                            .position(|e| *e == DisableEvent::Acknowledged)
                            .unwrap()
                );
                assert_eq!(disabled(root.path()).handoff, recorded.handoff);
                assert_eq!(disabled(root.path()).through, Some(target));
                let raw = Connection::open(root.path().join("kernel/kernel.sqlite")).unwrap();
                assert_eq!(
                    raw.query_row("SELECT count(*) FROM outbox_consumers", [], |r| r
                        .get::<_, i64>(0))
                        .unwrap(),
                    i64::from(!last) + i64::from(lagging)
                );
                assert_eq!(
                    raw.query_row("SELECT count(*) FROM consumer_abandonments", [], |r| r
                        .get::<_, i64>(0))
                        .unwrap(),
                    0
                );
                if let Some(id) = barrier_id {
                    assert_eq!(raw.query_row("SELECT acknowledged_at IS NOT NULL FROM deletion_backfill_barrier_consumers WHERE barrier_id=?1 AND consumer_id=?2", rusqlite::params![id, CONSUMER], |r| r.get::<_, bool>(0)).unwrap(), !lagging);
                }
                if last && !lagging {
                    let before = raw
                        .query_row("SELECT count(*) FROM outbox", [], |r| r.get::<_, i64>(0))
                        .unwrap();
                    assert!(matches!(
                        corpus.kernel.prune_outbox(),
                        Err(kernel::KernelError::NoRequiredConsumers)
                    ));
                    let lag = corpus.kernel.outbox_lag(now()).unwrap();
                    use daemon::kernel_routes::{UnavailableReason, serving};
                    assert_eq!(
                        serving::decide(&lag),
                        serving::ServingDecision::Unavailable(
                            UnavailableReason::NoRequiredConsumer
                        )
                    );
                    assert_eq!(
                        serving::decide_for_tip_read(&lag),
                        serving::ServingDecision::Available
                    );
                    assert_eq!(
                        raw.query_row("SELECT count(*) FROM outbox", [], |r| r.get::<_, i64>(0))
                            .unwrap(),
                        before
                    );
                    let unauthorized = LifecycleRequest {
                        transition: Transition::AuthorizedRecovery,
                        cause: Cause::DisabledRecovery,
                        authorization_ref: Some("operator:unapproved".to_owned()),
                        ..request(None, &spec(root.path()).identity)
                    };
                    assert!(
                        ProjectionLifecycle::open(root.path())
                            .unwrap()
                            .record(&gate, &unauthorized, now())
                            .is_err()
                    );
                }
                assert!(path.exists());
                for _ in 0..2 {
                    let fresh_gate = gate_for(root.path());
                    let fresh_selection = selector(root.path());
                    assert!(
                        fresh_selection
                            .reopen(
                                &corpus.kernel,
                                &fresh_gate,
                                &budget(Duration::from_secs(10))
                            )
                            .is_err()
                    );
                    assert_eq!(admissions(&fresh_selection, &corpus, &fresh_gate), (0, 0));
                    assert!(
                        ReplacementBuilder::open(
                            root.path(),
                            &corpus.kernel,
                            &fresh_gate,
                            spec(root.path())
                        )
                        .is_err()
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn commit_between_ack_and_deregister_stays_blocked_without_renewing_target_or_budget() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    remove_fixture_consumer(&corpus);
    corpus.publish("source", "bytes");
    let gate = gate_for(root.path());
    let mut selection = build_selected(root.path(), &corpus, &gate);
    let target = corpus.tip();
    let original = selection.begin_disable(&gate, &mut |_| {}).unwrap();
    let work = budget(Duration::from_secs(20));
    let mut fired = 0;
    for _ in 0..6 {
        let result = selection
            .reconcile_disabled(
                &corpus.kernel,
                &gate,
                &spec(root.path()),
                &work,
                &mut |event| {
                    if event == DisableEvent::Acknowledged {
                        fired += 1;
                        corpus.publish(&format!("late-{fired}"), "late bytes");
                    }
                },
            )
            .await;
        assert!(
            matches!(
                result,
                Err(BuildError::Kernel(kernel::KernelError::ConsumerPending))
            ),
            "{result:?}"
        );
        assert_eq!(
            corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
            Some(target)
        );
        let current = disabled(root.path());
        assert_eq!(current.handoff, original.handoff);
        assert_eq!(current.through, Some(target));
        assert_eq!(
            current.episodes.unwrap().deadline,
            original.recorded_at + 60_000
        );
        assert_eq!(current.episodes.unwrap().consumed, fired);
        gate.install(cleanup_evaluator(root.path()));
    }
    assert_eq!(fired, 6);
    assert!(matches!(
        selection
            .reconcile_disabled(
                &corpus.kernel,
                &gate,
                &spec(root.path()),
                &work,
                &mut |_| {}
            )
            .await,
        Err(BuildError::Intent(IntentRefusal::AllowanceExhausted))
    ));
    assert_eq!(disabled(root.path()).episodes.unwrap().consumed, 6);
}

pub(crate) fn disable_child(root: &Path, cut: &str) {
    let corpus = Corpus::open(root);
    corpus.seed();
    remove_fixture_consumer(&corpus);
    corpus.publish("source", "bytes");
    let gate = gate_for(root);
    let mut selection = build_selected(root, &corpus, &gate);
    let before_ack = corpus
        .kernel
        .outbox_consumer_checkpoint(CONSUMER)
        .unwrap()
        .unwrap();
    corpus
        .kernel
        .commit(intent("after-selection-control"), |_| Ok(String::new()))
        .unwrap();
    let target = corpus.tip();
    assert!(before_ack < target);
    let reader = selection
        .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
        .unwrap();
    apply_prefix(&reader, target, Vec::new());
    drop(reader);
    println!(
        "WITNESS {}",
        serde_json::json!({"target":target,"before_ack":before_ack})
    );
    if matches!(cut, "disable-before-rename" | "disable-after-rename") {
        let boundary = cut.to_owned();
        let gate = Arc::clone(&gate);
        selection = selection.with_disable_write_barrier_for_test(move |event| {
            use daemon::projection_lifecycle::WriteBarrier;
            assert!(
                gate.admit(ProjectionHook::EmbeddingBackfill, EntryPoint::Dispatch)
                    .is_err()
            );
            if (boundary == "disable-before-rename" && event == WriteBarrier::BeforeRename)
                || (boundary == "disable-after-rename" && event == WriteBarrier::AfterRename)
            {
                park(&boundary);
            }
        });
    }
    let mut observe = |event| {
        let name = match event {
            DisableEvent::AdmissionClosed => "disable-before-intent",
            DisableEvent::IntentPersisted => "disable-after-intent",
            DisableEvent::Acknowledged => "disable-lost-ack",
            DisableEvent::BeforeDeregister => "disable-before-deregister",
            DisableEvent::Deregistered => "disable-after-deregister",
            _ => "",
        };
        if name == cut {
            park(cut);
        }
    };
    selection.begin_disable(&gate, &mut observe).unwrap();
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(selection.reconcile_disabled(
            &corpus.kernel,
            &gate,
            &spec(root),
            &budget(Duration::from_secs(20)),
            &mut observe,
        ))
        .unwrap();
    panic!("cut did not fire");
}

#[tokio::test]
async fn disable_process_cuts_and_lost_ack_reconcile_twice_without_reenable() {
    for cut in [
        "disable-before-intent",
        "disable-before-rename",
        "disable-after-rename",
        "disable-after-intent",
        "disable-lost-ack",
        "disable-before-deregister",
        "disable-after-deregister",
    ] {
        let root = tempfile::tempdir().unwrap();
        let witness = kill_child_at(root.path(), cut);
        let target = witness["WITNESS"]["target"].as_i64().unwrap();
        let before_ack = witness["WITNESS"]["before_ack"].as_i64().unwrap();
        let mut completed = None;
        for reopen in 0..2 {
            let corpus = Corpus::open(root.path());
            let gate = gate_for(root.path());
            let mut selection = selector(root.path());
            let state = ProjectionLifecycle::open(root.path()).unwrap().read();
            let checkpoint = corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap();
            if matches!(cut, "disable-before-intent" | "disable-before-rename") && reopen == 0 {
                assert!(matches!(state, ControlState::Intent(_)));
                assert_eq!(checkpoint, Some(before_ack));
                assert_eq!(
                    gate.admit_all(&ProjectionHook::ALL, EntryPoint::Startup)
                        .unwrap()
                        .len(),
                    12
                );
                selection.begin_disable(&gate, &mut |_| {}).unwrap();
            } else {
                let ControlState::Disabled(record) = state else {
                    panic!("cut lost Disabled intent");
                };
                if reopen == 1 {
                    assert_eq!(Some(&record), completed.as_ref());
                    assert!(record.deregistered);
                    assert_eq!(checkpoint, None);
                } else {
                    let advanced = matches!(
                        cut,
                        "disable-lost-ack"
                            | "disable-before-deregister"
                            | "disable-after-deregister"
                    );
                    assert_eq!(record.through, advanced.then_some(target));
                    assert!(!record.deregistered);
                    assert_eq!(record.episodes.map(|e| e.consumed), advanced.then_some(1));
                    assert_eq!(
                        checkpoint,
                        if cut == "disable-after-deregister" {
                            None
                        } else if advanced {
                            Some(target)
                        } else {
                            Some(before_ack)
                        }
                    );
                }
            }
            assert_eq!(admissions(&selection, &corpus, &gate), (0, 0));
            let result = selection
                .reconcile_disabled(
                    &corpus.kernel,
                    &gate,
                    &spec(root.path()),
                    &budget(if reopen == 1 {
                        Duration::ZERO
                    } else {
                        Duration::from_secs(20)
                    }),
                    &mut |_| {},
                )
                .await
                .unwrap();
            assert!(result.deregistered);
            assert_eq!(result.through, Some(target));
            assert_eq!(
                corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
                None
            );
            assert_eq!(admissions(&selection, &corpus, &gate), (0, 0));
            if let Some(completed) = &completed {
                assert_eq!(&result, completed);
            }
            completed = Some(result);
        }
    }
}

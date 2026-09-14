use super::*;
use daemon::search_projection::SearchProjectionError;
use daemon::search_replacement::selection::retirement::RetirementEvent;
use host_runtime::lifecycle::PAYLOAD_MANIFEST_DIGEST_LEN;
use kernel::{
    ArtifactDeletionIdentity, ArtifactDeletionKind, ArtifactDeletionRequest, ConsumerObligation,
};
use retrieval::ProjectionError;
use sha2::{Digest, Sha256};

#[path = "../../../kernel/tests/support/canonical_state.rs"]
mod canonical_state;

struct RetirementCase {
    corpus: Corpus,
    gate: Arc<HookGate>,
    selection: SearchSelection,
    old: Option<SearchReader>,
    old_digest: String,
    old_checkpoint: i64,
    target: i64,
    expected: Vec<ConsumerObligation>,
}

impl RetirementCase {
    fn new(root: &Path) -> Self {
        let corpus = Corpus::open(root);
        corpus.seed();
        let mut expected = Vec::new();
        for key in ["alpha", "beta"] {
            let object = corpus.publish(key, key);
            expected.push(ConsumerObligation {
                kind: "source".to_owned(),
                identity: object,
                artifact_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
                commit_seq: corpus.tip(),
                invalidated_commit_seq: None,
            });
        }
        let gate = open_gate();
        let selection = build_selected(root, &corpus, &gate);
        let old = selection
            .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(30)))
            .unwrap();
        let old_digest = old.digest().to_owned();
        let old_checkpoint = corpus
            .kernel
            .outbox_consumer_checkpoint(CONSUMER)
            .unwrap()
            .unwrap();
        for index in 0..2 {
            let deletion = corpus
                .kernel
                .delete_artifact(ArtifactDeletionRequest {
                    intent: intent(&format!("delete-{index}")),
                    identity: ArtifactDeletionIdentity::Digest(
                        expected[index].artifact_digest.clone(),
                    ),
                    kind: ArtifactDeletionKind::Delete,
                    operator_id: None,
                    target_locator: None,
                    reason: None,
                    deleted_at: now(),
                })
                .unwrap();
            expected[index].invalidated_commit_seq = Some(deletion.commit_seq);
            expected.push(ConsumerObligation {
                kind: "barrier".to_owned(),
                identity: deletion.barrier_id,
                artifact_digest: deletion.digest,
                commit_seq: deletion.commit_seq,
                invalidated_commit_seq: Some(deletion.commit_seq),
            });
        }
        expected.sort_by(|a, b| (&a.kind, &a.identity).cmp(&(&b.kind, &b.identity)));
        let candidate = next_candidate(root, &corpus, &gate);
        let target = candidate.staged().verification.checkpoint_commit_seq;
        selection.select(candidate, &mut |_| Ok(())).unwrap();
        Self {
            corpus,
            gate,
            selection,
            old: Some(old),
            old_digest,
            old_checkpoint,
            target,
            expected,
        }
    }

    fn retire(
        &self,
        root: &Path,
        observe: &mut dyn FnMut(RetirementEvent),
    ) -> Result<(), BuildError> {
        self.selection.retire(
            &self.corpus.kernel,
            &self.gate,
            &spec(root),
            &budget(Duration::from_secs(30)),
            observe,
        )
    }

    fn assert_receipt(&self) {
        let reader = self
            .selection
            .pin(
                &self.corpus.kernel,
                &self.gate,
                &budget(Duration::from_secs(10)),
            )
            .unwrap();
        reader.read(&budget(Duration::from_secs(10)), |conn| {
            assert_eq!(conn.query_row("SELECT generation_id,old_consumer_id,old_family,selected_family,through_commit_seq,obligation_count FROM retirement_receipts", [], |row| {
                Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?,row.get::<_,i64>(4)?,row.get::<_,i64>(5)?))
            })?, (fixtures::GENERATION.to_owned(),CONSUMER.to_owned(),self.old_digest.clone(),reader.digest().to_owned(),self.target,self.expected.len() as i64));
            let rows = conn.prepare("SELECT kind,identity,artifact_digest,commit_seq,invalidated_commit_seq,disposition FROM retirement_dispositions ORDER BY kind,identity")?
                .query_map([], |row| {
                    assert_eq!(row.get::<_,String>(5)?, "removed");
                    Ok(ConsumerObligation {kind:row.get(0)?,identity:row.get(1)?,artifact_digest:row.get(2)?,commit_seq:row.get(3)?,invalidated_commit_seq:row.get(4)?})
                })?.collect::<rusqlite::Result<Vec<_>>>()?;
            assert_eq!(rows, self.expected);
            Ok(())
        }).unwrap();
    }

    fn assert_no_receipt(&self) {
        self.selection
            .pin(
                &self.corpus.kernel,
                &self.gate,
                &budget(Duration::from_secs(10)),
            )
            .unwrap()
            .read(&budget(Duration::from_secs(10)), |conn| {
                for table in ["retirement_receipts", "retirement_dispositions"] {
                    assert_eq!(
                        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row
                            .get::<_, i64>(0))?,
                        0
                    );
                }
                Ok(())
            })
            .unwrap();
    }
}

fn barrier_memberships(root: &Path) -> Vec<(String, String, i64, String, String)> {
    Connection::open(root.join("kernel/kernel.sqlite")).unwrap()
        .prepare("SELECT bc.barrier_id,bc.consumer_id,bc.required_checkpoint_commit_seq,b.artifact_digest,b.artifact_reference FROM deletion_backfill_barrier_consumers bc JOIN deletion_backfill_barriers b USING(barrier_id) ORDER BY bc.barrier_id,bc.consumer_id").unwrap()
        .query_map([], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).unwrap()
        .collect::<rusqlite::Result<Vec<_>>>().unwrap()
}

#[tokio::test]
async fn default_disable_uses_certified_retirement_then_preserves_the_selected_pending_obligation()
{
    use daemon::search_replacement::selection::disable::DisableEvent;
    let root = tempfile::tempdir().unwrap();
    let mut case = RetirementCase::new(root.path());
    case.gate
        .install(super::disable::cleanup_evaluator(root.path()));
    let old_path = case.old.as_ref().unwrap().projection().path().to_owned();
    let selected = case
        .selection
        .pin(
            &case.corpus.kernel,
            &case.gate,
            &budget(Duration::from_secs(10)),
        )
        .unwrap();
    let selected_path = selected.projection().path().to_owned();
    drop(selected);
    drop(case.old.take());
    let original = case
        .selection
        .begin_disable(&case.gate, &mut |_| {})
        .unwrap();
    let mut ledger = Vec::new();
    let result = case
        .selection
        .reconcile_disabled(
            &case.corpus.kernel,
            &case.gate,
            &spec(root.path()),
            &budget(Duration::from_secs(20)),
            &mut |event| ledger.push(event),
        )
        .await;
    assert!(
        matches!(
            result,
            Err(BuildError::Kernel(kernel::KernelError::ConsumerPending))
        ),
        "{result:?}"
    );
    assert!(ledger.contains(&DisableEvent::Retirement(RetirementEvent::Removed)));
    assert!(!old_path.exists());
    assert_eq!(
        case.corpus
            .kernel
            .outbox_consumer_checkpoint(CONSUMER)
            .unwrap(),
        None
    );
    assert_eq!(
        case.corpus
            .kernel
            .outbox_consumer_checkpoint("second-consumer")
            .unwrap(),
        Some(case.target)
    );
    let raw = Connection::open(selected_path).unwrap();
    assert_eq!(
        raw.query_row(
            "SELECT through_commit_seq FROM retirement_receipts WHERE old_consumer_id=?1",
            [CONSUMER],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        case.target
    );
    assert_eq!(
        raw.query_row(
            "SELECT count(*) FROM retirement_dispositions WHERE disposition='removed'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        case.expected.len() as i64
    );
    match ProjectionLifecycle::open(root.path()).unwrap().read() {
        ControlState::Disabled(current) => {
            assert_eq!(current.handoff, original.handoff);
            assert_eq!(current.through, Some(case.target));
            assert!(!current.deregistered);
        }
        other => panic!("{other:?}"),
    }
}

/// A manifest too small for the retirement obligation transaction refuses disabled cleanup before any retirement write, and the selected projection and consumer checkpoint stay unchanged.
#[tokio::test]
async fn disabled_cleanup_charges_the_retirement_bounds_before_retiring() {
    use daemon::projection_gates::Denial;
    use daemon::projection_lifecycle::MAX_RECORD_BYTES;
    for (name, max) in [
        ("local_transaction_rows", 5_000),
        ("local_transaction_bytes", 4 << 20),
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut case = RetirementCase::new(root.path());
        let mut evaluator = super::disable::cleanup_evaluator(root.path());
        evaluator.manifest.limits.insert(name.to_owned(), max);
        case.gate.install(evaluator);
        let mut config = spec(root.path());
        config.retirement.max_obligations = NonZeroUsize::new(10_000).unwrap();
        config.retirement.max_obligation_bytes = NonZeroU64::new(8 << 20).unwrap();
        let expected = match name {
            "local_transaction_rows" => 10_001,
            _ => (8 << 20) + MAX_RECORD_BYTES,
        };
        let old_path = case.old.as_ref().unwrap().projection().path().to_owned();
        let selected_path = case
            .selection
            .pin(
                &case.corpus.kernel,
                &case.gate,
                &budget(Duration::from_secs(10)),
            )
            .unwrap()
            .projection()
            .path()
            .to_owned();
        drop(case.old.take());
        case.selection
            .begin_disable(&case.gate, &mut |_| {})
            .unwrap();
        let mut ledger = Vec::new();
        let result = case
            .selection
            .reconcile_disabled(
                &case.corpus.kernel,
                &case.gate,
                &config,
                &budget(Duration::from_secs(20)),
                &mut |event| ledger.push(event),
            )
            .await;
        assert!(
            matches!(
                result,
                Err(BuildError::Denied(Denial::LimitExceeded {
                    limit: ref denied,
                    observed,
                    max: cap,
                })) if denied == name && observed == expected && cap == max
            ),
            "{name}: {result:?}"
        );
        assert!(ledger.iter().all(|event| !matches!(
            event,
            daemon::search_replacement::selection::disable::DisableEvent::Retirement(_)
        )));
        assert!(old_path.exists());
        let raw = Connection::open(selected_path).unwrap();
        for table in ["retirement_receipts", "retirement_dispositions"] {
            assert_eq!(
                raw.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
        assert_eq!(
            case.corpus
                .kernel
                .outbox_consumer_checkpoint(CONSUMER)
                .unwrap(),
            Some(case.old_checkpoint)
        );
    }
}

/// A projection reference upgraded from a `Weak` while retirement runs is a live holder. Local release is refused rather than recorded over it, and the retry after the holder drops proceeds.
#[tokio::test]
async fn a_projection_upgraded_during_retirement_blocks_local_release() {
    use daemon::search_replacement::selection::disable::DisableEvent;
    let root = tempfile::tempdir().unwrap();
    let mut case = RetirementCase::new(root.path());
    case.gate
        .install(super::disable::cleanup_evaluator(root.path()));
    let selected = case
        .selection
        .pin(
            &case.corpus.kernel,
            &case.gate,
            &budget(Duration::from_secs(10)),
        )
        .unwrap();
    let weak = Arc::downgrade(selected.projection());
    drop(selected);
    drop(case.old.take());
    case.selection
        .begin_disable(&case.gate, &mut |_| {})
        .unwrap();
    let mut held = None;
    let mut ledger = Vec::new();
    let result = case
        .selection
        .reconcile_disabled(
            &case.corpus.kernel,
            &case.gate,
            &spec(root.path()),
            &budget(Duration::from_secs(20)),
            &mut |event| {
                if event == DisableEvent::Retirement(RetirementEvent::Removed) {
                    held = weak.upgrade();
                }
                ledger.push(event);
            },
        )
        .await;
    assert!(
        held.is_some(),
        "the weak reference upgraded during retirement"
    );
    assert!(
        matches!(
            result,
            Err(BuildError::Intent(
                daemon::projection_lifecycle::IntentRefusal::FamilyHeld
            ))
        ),
        "{result:?}"
    );
    assert!(!ledger.contains(&DisableEvent::LocalReleased));
    match ProjectionLifecycle::open(root.path()).unwrap().read() {
        ControlState::Disabled(current) => assert_eq!(current.through, None),
        other => panic!("{other:?}"),
    }
    drop(held);
    let mut ledger = Vec::new();
    let result = case
        .selection
        .reconcile_disabled(
            &case.corpus.kernel,
            &case.gate,
            &spec(root.path()),
            &budget(Duration::from_secs(20)),
            &mut |event| ledger.push(event),
        )
        .await;
    assert!(
        matches!(
            result,
            Err(BuildError::Kernel(kernel::KernelError::ConsumerPending))
        ),
        "{result:?}"
    );
    assert!(ledger.contains(&DisableEvent::LocalReleased));
    match ProjectionLifecycle::open(root.path()).unwrap().read() {
        ControlState::Disabled(current) => assert_eq!(current.through, Some(case.target)),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn disabled_receipt_refusal_retains_the_selected_owner_and_original_target() {
    let root = tempfile::tempdir().unwrap();
    let mut case = RetirementCase::new(root.path());
    drop(case.old.take());
    let interrupted = budget(Duration::from_secs(20));
    assert!(matches!(
        case.selection.retire(
            &case.corpus.kernel,
            &case.gate,
            &spec(root.path()),
            &interrupted,
            &mut |event| {
                if event == RetirementEvent::LocalReleased {
                    interrupted.cancel();
                }
            }
        ),
        Err(BuildError::Expired)
    ));
    let reader = case
        .selection
        .pin(
            &case.corpus.kernel,
            &case.gate,
            &budget(Duration::from_secs(10)),
        )
        .unwrap();
    let path = reader.projection().path().to_owned();
    let weak = Arc::downgrade(reader.projection());
    reader.projection().write(|conn| {
        assert_eq!(conn.execute("UPDATE retirement_receipts SET obligation_count=obligation_count+1 WHERE old_consumer_id=?1", [CONSUMER])?, 1);
        Ok(())
    }).unwrap();
    drop(reader);
    case.gate
        .install(super::disable::cleanup_evaluator(root.path()));
    case.selection
        .begin_disable(&case.gate, &mut |_| {})
        .unwrap();
    assert!(
        case.selection
            .reconcile_disabled(
                &case.corpus.kernel,
                &case.gate,
                &spec(root.path()),
                &budget(Duration::from_secs(20)),
                &mut |_| {}
            )
            .await
            .is_err()
    );
    assert!(weak.upgrade().is_some());
    assert_eq!(
        case.corpus
            .kernel
            .outbox_consumer_checkpoint(CONSUMER)
            .unwrap(),
        Some(case.old_checkpoint)
    );
    let raw = Connection::open(path).unwrap();
    assert_eq!(raw.execute("UPDATE retirement_receipts SET obligation_count=obligation_count-1 WHERE old_consumer_id=?1", [CONSUMER]).unwrap(), 1);
    assert!(matches!(
        case.selection
            .reconcile_disabled(
                &case.corpus.kernel,
                &case.gate,
                &spec(root.path()),
                &budget(Duration::from_secs(20)),
                &mut |_| {}
            )
            .await,
        Err(BuildError::Kernel(kernel::KernelError::ConsumerPending))
    ));
    assert_eq!(
        case.corpus
            .kernel
            .outbox_consumer_checkpoint(CONSUMER)
            .unwrap(),
        None
    );
    assert_eq!(
        case.corpus
            .kernel
            .outbox_consumer_checkpoint("second-consumer")
            .unwrap(),
        Some(case.target)
    );
    assert_eq!(
        raw.query_row(
            "SELECT through_commit_seq FROM retirement_receipts WHERE old_consumer_id=?1",
            [CONSUMER],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        case.target
    );
}

#[test]
fn missing_corrupt_and_healthy_old_databases_retire_from_authority_not_new_checkpoint() {
    for damage in ["missing", "corrupt", "healthy"] {
        let root = tempfile::tempdir().unwrap();
        let mut case = RetirementCase::new(root.path());
        let path = case.old.as_ref().unwrap().projection().path().to_owned();
        assert_eq!(
            case.corpus
                .kernel
                .outbox_consumer_checkpoint(CONSUMER)
                .unwrap(),
            Some(case.old_checkpoint)
        );
        let canonical = || {
            let mut digest = canonical_state::digest(
                &root.path().join("kernel"),
                canonical_state::Profile::SameRoot,
            );
            for table in [
                "commit_log",
                "outbox",
                "operation_receipts",
                "change_event",
                "sqlite_sequence",
                "outbox_consumers",
                "capture_pins",
                "capture_pin_refs",
                "deletion_backfill_barriers",
                "deletion_backfill_barrier_consumers",
            ] {
                assert!(
                    digest.tables.remove(table).is_some(),
                    "declared bookkeeping table {table}"
                );
            }
            digest
        };
        let before = canonical();
        let membership_before = barrier_memberships(root.path());
        drop(case.old.take());
        match damage {
            "missing" => std::fs::remove_file(&path).unwrap(),
            "corrupt" => std::fs::write(&path, b"corrupt old database").unwrap(),
            _ => {}
        }
        case.retire(root.path(), &mut |_| {}).unwrap();
        case.assert_receipt();
        assert!(!path.exists());
        assert!(
            !path
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("bootstrap.json")
                .exists()
        );
        assert!(
            !GenerationStore::open(Some(root.path()))
                .unwrap()
                .root()
                .join("generations")
                .join(&case.old_digest)
                .exists()
        );
        assert_eq!(
            case.corpus
                .kernel
                .outbox_consumer_checkpoint(CONSUMER)
                .unwrap(),
            None
        );
        assert_eq!(canonical(), before);
        assert_eq!(barrier_memberships(root.path()), membership_before);
        let raw = Connection::open(root.path().join("kernel/kernel.sqlite")).unwrap();
        assert_eq!(
            raw.query_row("SELECT count(*) FROM consumer_abandonments", [], |row| row
                .get::<_, i64>(
                0
            ))
            .unwrap(),
            0
        );
        assert_eq!(raw.query_row("SELECT count(*) FROM deletion_backfill_barrier_consumers WHERE consumer_id=?1 AND acknowledged_at IS NOT NULL",[CONSUMER],|row|row.get::<_,i64>(0)).unwrap(),2);
        case.retire(root.path(), &mut |_| {}).unwrap();
        case.assert_receipt();
    }
}

#[test]
fn native_worker_cancellation_and_cleanup_failure_do_not_certify_removal() {
    let root = tempfile::tempdir().unwrap();
    let mut case = RetirementCase::new(root.path());
    let engine = fixtures::TestEngine::new();
    let native_gate = engine.block_calls();
    let release = fixtures::GateGuard(Arc::clone(&native_gate));
    let component = fixtures::component(&engine, host_runtime::synapse::SynapseLimits::default());
    let worker_pin = case.old.as_ref().unwrap().clone();
    let cancelled = budget(Duration::from_secs(30));
    let worker_budget = cancelled.clone();
    let worker = std::thread::spawn(move || {
        component.embed_blocking(&["alpha"]).unwrap();
        assert!(worker_budget.is_exhausted());
        drop(worker_pin);
    });
    let start = std::time::Instant::now();
    while engine.calls() == 0 {
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::yield_now();
    }
    cancelled.cancel();
    for _ in 0..2 {
        let error = case.retire(root.path(), &mut |_| {}).unwrap_err();
        assert!(
            matches!(
                error,
                BuildError::Projection(SearchProjectionError::Store(storage::StoreError::Lease(
                    lease::LeaseError::Held { .. }
                )))
            ),
            "{error:?}"
        );
        case.assert_no_receipt();
        drop(case.old.take());
    }
    assert_eq!(
        case.corpus
            .kernel
            .outbox_consumer_checkpoint(CONSUMER)
            .unwrap(),
        Some(case.old_checkpoint)
    );
    drop(release);
    worker.join().unwrap();
    let foreign = root
        .path()
        .join("search-families")
        .join(&case.old_digest)
        .join("unowned-residue");
    std::fs::write(&foreign, b"residue").unwrap();
    let error = case.retire(root.path(), &mut |_| {}).unwrap_err();
    assert!(
        matches!(error, BuildError::Invalid("unreconciled family residue")),
        "{error:?}"
    );
    case.assert_no_receipt();
    assert_eq!(
        case.corpus
            .kernel
            .outbox_consumer_checkpoint(CONSUMER)
            .unwrap(),
        Some(case.old_checkpoint)
    );
    std::fs::remove_file(foreign).unwrap();
    case.retire(root.path(), &mut |_| {}).unwrap();
    case.assert_receipt();
}

#[test]
fn a_stale_inspection_copy_of_the_old_database_is_owned_residue_not_a_permanent_wedge() {
    let root = tempfile::tempdir().unwrap();
    let mut case = RetirementCase::new(root.path());
    let database = case.old.as_ref().unwrap().projection().path().to_owned();
    drop(case.old.take());
    let scratch = database.parent().unwrap().join(format!(
        ".inspect-{}-4242-1",
        database.file_name().unwrap().to_string_lossy()
    ));
    std::fs::create_dir(&scratch).unwrap();
    std::fs::copy(&database, scratch.join(database.file_name().unwrap())).unwrap();
    case.retire(root.path(), &mut |_| {}).unwrap();
    case.assert_receipt();
    assert!(!scratch.exists());
    assert!(!database.exists());
    assert_eq!(
        case.corpus
            .kernel
            .outbox_consumer_checkpoint(CONSUMER)
            .unwrap(),
        None
    );
}

#[test]
fn a_tampered_retiring_binding_never_becomes_available_after_reopen() {
    for (path, value) in [
        ("consumer/consumer_id", serde_json::json!("second-consumer")),
        ("seed/generation_id", serde_json::json!("ghost-generation")),
        (
            "seed/kernel_incarnation_id",
            serde_json::json!("other-incarnation"),
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut case = RetirementCase::new(root.path());
        drop(case.old.take());
        let certificate = case
            .selection
            .pin(
                &case.corpus.kernel,
                &case.gate,
                &budget(Duration::from_secs(10)),
            )
            .unwrap()
            .projection()
            .path()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("bootstrap.json");
        let RetirementCase {
            corpus,
            gate,
            selection,
            ..
        } = case;
        drop(selection);
        let mut bootstrap: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&certificate).unwrap()).unwrap();
        *bootstrap.pointer_mut(&format!("/retiring/{path}")).unwrap() = value;
        std::fs::write(&certificate, serde_json::to_vec(&bootstrap).unwrap()).unwrap();
        let selection = selector(root.path());
        assert!(
            selection
                .reopen(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                .is_err(),
            "{path}"
        );
        assert!(
            selection
                .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                .is_err(),
            "{path}"
        );
    }
}

#[test]
fn ownership_certificate_survives_a_missing_database_before_replacement_selection() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    corpus.publish("old", "old source");
    let gate = open_gate();
    let selection = build_selected(root.path(), &corpus, &gate);
    let reader = selection
        .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
        .unwrap();
    let home = reader
        .projection()
        .path()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned();
    let database = reader.projection().path().to_owned();
    drop(reader);
    drop(selection);
    let path = home.join("bootstrap.json");
    let mut certificate: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    certificate["schema"] = serde_json::json!(1);
    certificate.as_object_mut().unwrap().remove("retiring");
    std::fs::write(path, serde_json::to_vec(&certificate).unwrap()).unwrap();
    let unavailable = selector(root.path());
    let error = unavailable
        .reopen(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
        .unwrap_err();
    assert!(
        matches!(error, BuildError::Invalid("bootstrap binding mismatch")),
        "{error:?}"
    );
    let error = unavailable
        .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
        .err()
        .unwrap();
    assert!(
        matches!(
            error,
            BuildError::Invalid("search unavailable; rebuild required")
        ),
        "{error:?}"
    );
    std::fs::remove_file(database).unwrap();
    let candidate = next_candidate(root.path(), &corpus, &gate);
    let selection = selector(root.path());
    selection.select(candidate, &mut |_| Ok(())).unwrap();
    selection
        .retire(
            &corpus.kernel,
            &gate,
            &spec(root.path()),
            &budget(Duration::from_secs(30)),
            &mut |_| {},
        )
        .unwrap();
    assert_eq!(
        corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
        None
    );
}

#[test]
fn incomplete_or_corrupt_receipts_and_missing_authority_refuse_acknowledgement() {
    for corruption in ["authority", "omitted-disposition", "count", "identity"] {
        let root = tempfile::tempdir().unwrap();
        let mut case = RetirementCase::new(root.path());
        drop(case.old.take());
        if corruption == "authority" {
            let raw = Connection::open(root.path().join("kernel/kernel.sqlite")).unwrap();
            raw.execute(
                "DELETE FROM observations WHERE object_id=?1",
                [&case
                    .expected
                    .iter()
                    .find(|o| o.kind == "source")
                    .unwrap()
                    .identity],
            )
            .unwrap();
        } else {
            let interrupted = budget(Duration::from_secs(30));
            let error = case
                .selection
                .retire(
                    &case.corpus.kernel,
                    &case.gate,
                    &spec(root.path()),
                    &interrupted,
                    &mut |event| {
                        if event == RetirementEvent::LocalReleased {
                            interrupted.cancel();
                        }
                    },
                )
                .unwrap_err();
            assert!(matches!(error, BuildError::Expired), "{error:?}");
            let reader = case
                .selection
                .pin(
                    &case.corpus.kernel,
                    &case.gate,
                    &budget(Duration::from_secs(10)),
                )
                .unwrap();
            reader
                .projection()
                .write(|conn| {
                    let sql = match corruption {
                        "omitted-disposition" => "DELETE FROM retirement_dispositions WHERE kind='barrier' AND identity=?1",
                        "count" => "UPDATE retirement_receipts SET obligation_count=obligation_count+1 WHERE receipt_id=(SELECT receipt_id FROM retirement_dispositions WHERE identity=?1)",
                        "identity" => "UPDATE retirement_dispositions SET identity=(CASE substr(identity,1,1) WHEN 'x' THEN 'y' ELSE 'x' END)||substr(identity,2) WHERE identity=?1",
                        _ => unreachable!(),
                    };
                    let charge = || conn.query_row("SELECT count(*),sum(length(CAST(identity AS BLOB))) FROM retirement_dispositions",[],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?)));
                    let before = charge()?;
                    assert_eq!(conn.execute(sql, [&case.expected[0].identity])?,1);
                    if corruption == "identity" { assert_eq!(charge()?, before); }
                    Ok(())
                })
                .unwrap();
        }
        let error = case.retire(root.path(), &mut |_| {}).unwrap_err();
        match corruption {
            "authority" => assert!(
                matches!(
                    error,
                    BuildError::Kernel(kernel::KernelError::CorruptCanonicalRow)
                ),
                "{error:?}"
            ),
            "count" => assert!(
                matches!(
                    error,
                    BuildError::Projection(SearchProjectionError::Projection(
                        ProjectionError::MutationConflict
                    ))
                ),
                "{error:?}"
            ),
            _ => assert!(
                matches!(
                    error,
                    BuildError::Projection(SearchProjectionError::Projection(
                        ProjectionError::CorruptRow
                    ))
                ),
                "{error:?}"
            ),
        }
        assert_eq!(
            case.corpus
                .kernel
                .outbox_consumer_checkpoint(CONSUMER)
                .unwrap(),
            Some(case.old_checkpoint)
        );
    }
}

#[test]
fn tip_between_ack_and_deregister_retains_consumer_and_fixed_receipt() {
    let root = tempfile::tempdir().unwrap();
    let mut case = RetirementCase::new(root.path());
    drop(case.old.take());
    let memberships = barrier_memberships(root.path());
    let result = case.retire(root.path(), &mut |event| {
        if event == RetirementEvent::Acknowledged {
            case.corpus
                .kernel
                .commit(intent("concurrent-tip"), |_| Ok(String::new()))
                .unwrap();
        }
    });
    assert!(matches!(
        result,
        Err(BuildError::Kernel(kernel::KernelError::ConsumerPending))
    ));
    assert_eq!(
        case.corpus
            .kernel
            .outbox_consumer_checkpoint(CONSUMER)
            .unwrap(),
        Some(case.target)
    );
    assert!(matches!(
        case.retire(root.path(), &mut |_| {}),
        Err(BuildError::Kernel(kernel::KernelError::ConsumerPending))
    ));
    case.assert_receipt();
    assert_eq!(barrier_memberships(root.path()), memberships);
}

#[test]
fn inventory_bounds_and_cancelled_acknowledgement_preserve_old_checkpoint() {
    let root = tempfile::tempdir().unwrap();
    let case = RetirementCase::new(root.path());
    case.assert_no_receipt();
    let target = case
        .corpus
        .kernel
        .capture_commit_read_target_within_budget(&budget(Duration::from_secs(10)))
        .unwrap();
    for (rows, bytes) in [(1, 1 << 20), (256, 1)] {
        assert_eq!(
            case.corpus.kernel.consumer_obligations_within_budget(
                &budget(Duration::from_secs(10)),
                CONSUMER,
                target,
                NonZeroUsize::new(rows).unwrap(),
                NonZeroU64::new(bytes).unwrap(),
            ),
            Err(kernel::ConsumerObligationError::InventoryBound)
        );
    }
    let cancelled = budget(Duration::from_secs(10));
    cancelled.cancel();
    assert_eq!(
        case.corpus.kernel.acknowledge_outbox_within_budget(
            &cancelled,
            CONSUMER,
            case.target,
            now(),
            target.incarnation,
        ),
        Err(kernel::KernelError::Deadline)
    );
    assert_eq!(
        case.corpus
            .kernel
            .outbox_consumer_checkpoint(CONSUMER)
            .unwrap(),
        Some(case.old_checkpoint)
    );
}

#[test]
fn a_maximal_obligation_bound_is_refused_before_the_gate_is_charged() {
    let root = tempfile::tempdir().unwrap();
    let mut case = RetirementCase::new(root.path());
    drop(case.old.take());
    let mut config = spec(root.path());
    config.retirement.max_obligations = NonZeroUsize::MAX;
    let result = case.selection.retire(
        &case.corpus.kernel,
        &case.gate,
        &config,
        &budget(Duration::from_secs(30)),
        &mut |_| {},
    );
    assert!(
        matches!(result, Err(BuildError::InventoryBound)),
        "{result:?}"
    );
    case.assert_no_receipt();
    assert_eq!(
        case.corpus
            .kernel
            .outbox_consumer_checkpoint(CONSUMER)
            .unwrap(),
        Some(case.old_checkpoint)
    );
}

#[test]
fn census_bound_is_independent_of_the_per_batch_persist_limit() {
    let root = tempfile::tempdir().unwrap();
    let mut case = RetirementCase::new(root.path());
    drop(case.old.take());
    assert!(case.expected.len() > 1);
    let mut config = spec(root.path());
    config.retirement.max_obligations = NonZeroUsize::new(case.expected.len() - 1).unwrap();
    let error = case
        .selection
        .retire(
            &case.corpus.kernel,
            &case.gate,
            &config,
            &budget(Duration::from_secs(30)),
            &mut |event| panic!("premature effect {event:?}"),
        )
        .unwrap_err();
    assert!(matches!(error, BuildError::InventoryBound), "{error:?}");
    case.assert_no_receipt();
    assert_eq!(
        case.corpus
            .kernel
            .outbox_consumer_checkpoint(CONSUMER)
            .unwrap(),
        Some(case.old_checkpoint)
    );
    config.retirement.max_obligations = NonZeroUsize::new(case.expected.len()).unwrap();
    config.episode.batch.persist.max_records = NonZeroUsize::MIN;
    case.selection
        .retire(
            &case.corpus.kernel,
            &case.gate,
            &config,
            &budget(Duration::from_secs(30)),
            &mut |_| {},
        )
        .unwrap();
    case.assert_receipt();
    assert_eq!(
        case.corpus
            .kernel
            .outbox_consumer_checkpoint(CONSUMER)
            .unwrap(),
        None
    );
}

#[test]
fn the_receipt_transaction_charge_covers_every_disposition_row() {
    let root = tempfile::tempdir().unwrap();
    let mut case = RetirementCase::new(root.path());
    drop(case.old.take());
    let config = spec(root.path());
    // The limit leaves room for `max_obligation_bytes` and one `MAX_RECORD_BYTES`; each disposition row adds a digest and `removed`.
    let censused = config.retirement.max_obligation_bytes.get() + MAX_RECORD_BYTES;
    let mut evaluator = support::projection_gate::passing_evaluator(
        &config.identity,
        0,
        &daemon::projection_gates::ProjectionHook::ALL,
    );
    evaluator
        .manifest
        .limits
        .insert("local_transaction_bytes".to_owned(), censused);
    case.gate.install(evaluator);
    let error = case.retire(root.path(), &mut |_| {}).unwrap_err();
    match error {
        BuildError::Denied(daemon::projection_gates::Denial::LimitExceeded {
            limit,
            observed,
            max,
        }) => {
            assert_eq!(limit, "local_transaction_bytes");
            assert_eq!(max, censused);
            let per_row = (PAYLOAD_MANIFEST_DIGEST_LEN + "removed".len()) as u64;
            assert_eq!(
                observed,
                censused + config.retirement.max_obligations.get() as u64 * per_row
            );
        }
        other => panic!("{other:?}"),
    }
    case.assert_no_receipt();
}

#[test]
fn receipt_ack_waits_for_local_release_and_gate_denial_keeps_obligations() {
    let root = tempfile::tempdir().unwrap();
    let mut case = RetirementCase::new(root.path());
    drop(case.old.take());
    let mut evaluator = support::projection_gate::passing_evaluator(
        &spec(root.path()).identity,
        0,
        &daemon::projection_gates::ProjectionHook::ALL,
    );
    evaluator
        .manifest
        .limits
        .insert("physical_drain_ms".to_owned(), 0);
    case.gate.install(evaluator);
    let error = case.retire(root.path(), &mut |_| {}).unwrap_err();
    assert!(
        matches!(error, BuildError::Denied(daemon::projection_gates::Denial::LimitExceeded {
        ref limit, observed, max: 0
    }) if limit=="physical_drain_ms" && observed>0),
        "{error:?}"
    );
    case.assert_no_receipt();
    assert_eq!(
        case.corpus
            .kernel
            .outbox_consumer_checkpoint(CONSUMER)
            .unwrap(),
        Some(case.old_checkpoint)
    );
    case.gate
        .install(support::projection_gate::passing_evaluator(
            &spec(root.path()).identity,
            0,
            &daemon::projection_gates::ProjectionHook::ALL,
        ));
    let reader = case
        .selection
        .pin(
            &case.corpus.kernel,
            &case.gate,
            &budget(Duration::from_secs(10)),
        )
        .unwrap();
    let path = reader.projection().path().to_owned();
    let mut events = Vec::new();
    case.retire(root.path(), &mut |event| {
        events.push(event);
        if event == RetirementEvent::BeforeAcknowledgement {
            let raw = Connection::open(&path).unwrap();
            raw.busy_timeout(Duration::ZERO).unwrap();
            raw.execute_batch("BEGIN IMMEDIATE; ROLLBACK").unwrap();
            assert_eq!(
                case.corpus
                    .kernel
                    .outbox_consumer_checkpoint(CONSUMER)
                    .unwrap(),
                Some(case.old_checkpoint)
            );
        }
    })
    .unwrap();
    assert_eq!(
        events,
        vec![
            RetirementEvent::BeforeCleanup,
            RetirementEvent::Removed,
            RetirementEvent::BeforeReceiptCommit,
            RetirementEvent::LocalReleased,
            RetirementEvent::BeforeAcknowledgement,
            RetirementEvent::Acknowledged
        ]
    );
}

#[test]
fn quarantined_selector_preserves_old_bytes_before_any_removal() {
    let root = tempfile::tempdir().unwrap();
    let mut case = RetirementCase::new(root.path());
    let database = case.old.as_ref().unwrap().projection().path().to_owned();
    let certificate = database
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("bootstrap.json");
    drop(case.old.take());
    let bytes = std::fs::read(&database).unwrap();
    let metadata = std::fs::read(&certificate).unwrap();
    let store = GenerationStore::open(Some(root.path())).unwrap();
    let profile = store
        .root()
        .join(host_runtime::generation::SEARCH_PROFILE_NAME);
    let selected = std::fs::read(&profile).unwrap();
    let mut reached = false;
    let error = case
        .retire(root.path(), &mut |event| {
            if event == RetirementEvent::BeforeCleanup {
                reached = true;
                std::fs::write(&profile, br#"{"schema":999,"current":"unknown"}"#).unwrap();
                assert_eq!(
                    store.read_search_current().unwrap(),
                    CurrentProfile::Quarantined
                );
            }
        })
        .unwrap_err();
    assert!(reached);
    assert!(
        matches!(error, BuildError::Invalid("unknown selector references")),
        "{error:?}"
    );
    assert_eq!(std::fs::read(&database).unwrap(), bytes);
    assert_eq!(std::fs::read(&certificate).unwrap(), metadata);
    store.validate(&case.old_digest).unwrap();
    case.assert_no_receipt();
    assert_eq!(
        case.corpus
            .kernel
            .outbox_consumer_checkpoint(CONSUMER)
            .unwrap(),
        Some(case.old_checkpoint)
    );
    std::fs::write(profile, selected).unwrap();
    case.retire(root.path(), &mut |_| {}).unwrap();
    case.assert_receipt();
}

#[test]
fn retirement_and_active_open_require_full_certificates_not_matching_prefixes() {
    for corruption in ["partial", "binding"] {
        let root = tempfile::tempdir().unwrap();
        let mut case = RetirementCase::new(root.path());
        let database = case.old.as_ref().unwrap().projection().path().to_owned();
        let path = database
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("bootstrap.json");
        drop(case.old.take());
        let bytes = std::fs::read(&database).unwrap();
        let certificate = std::fs::read(&path).unwrap();
        let damaged = if corruption == "partial" {
            certificate[..certificate.len() / 2].to_vec()
        } else {
            let mut json: serde_json::Value = serde_json::from_slice(&certificate).unwrap();
            json["intent"]["consumer"]["consumer_id"] = serde_json::json!("foreign-consumer");
            serde_json::to_vec(&json).unwrap()
        };
        std::fs::write(&path, damaged).unwrap();
        let error = case.retire(root.path(), &mut |_| {}).unwrap_err();
        match corruption {
            "partial" => assert!(
                matches!(error, BuildError::Invalid("old bootstrap corrupt")),
                "{error:?}"
            ),
            _ => assert!(
                matches!(error, BuildError::Invalid("old cleanup binding mismatch")),
                "{error:?}"
            ),
        }
        assert_eq!(std::fs::read(&database).unwrap(), bytes);
        case.assert_no_receipt();
        assert_eq!(
            case.corpus
                .kernel
                .outbox_consumer_checkpoint(CONSUMER)
                .unwrap(),
            Some(case.old_checkpoint)
        );
        std::fs::write(path, certificate).unwrap();
        let reader = case
            .selection
            .pin(
                &case.corpus.kernel,
                &case.gate,
                &budget(Duration::from_secs(10)),
            )
            .unwrap();
        let selected = reader
            .projection()
            .path()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("bootstrap.json");
        let certificate = std::fs::read(&selected).unwrap();
        std::fs::write(&selected, &certificate[..certificate.len() / 2]).unwrap();
        let error = selector(root.path())
            .reopen(
                &case.corpus.kernel,
                &case.gate,
                &budget(Duration::from_secs(10)),
            )
            .unwrap_err();
        assert!(
            matches!(error, BuildError::Invalid("bootstrap corrupt")),
            "{error:?}"
        );
        std::fs::write(selected, certificate).unwrap();
        case.retire(root.path(), &mut |_| {}).unwrap();
        case.assert_receipt();
    }
}

#[test]
fn completeness_walk_requires_every_page_before_certifying_exact_target() {
    let root = tempfile::tempdir().unwrap();
    let mut case = RetirementCase::new(root.path());
    let database = case.old.as_ref().unwrap().projection().path().to_owned();
    drop(case.old.take());
    let raw = Connection::open(root.path().join("kernel/kernel.sqlite")).unwrap();
    let commits: i64 = raw
        .query_row(
            "SELECT count(*) FROM commit_log WHERE commit_seq>?1 AND commit_seq<=?2",
            [case.old_checkpoint, case.target],
            |row| row.get(0),
        )
        .unwrap();
    assert!(commits > 1);
    let mut config = spec(root.path());
    config.episode.commits.max_commits = NonZeroUsize::MIN;
    config.episode.max_source_pages = NonZeroUsize::MIN;
    let error = case
        .selection
        .retire(
            &case.corpus.kernel,
            &case.gate,
            &config,
            &budget(Duration::from_secs(30)),
            &mut |event| panic!("premature effect {event:?}"),
        )
        .unwrap_err();
    assert!(matches!(error, BuildError::InventoryBound), "{error:?}");
    assert!(database.exists());
    case.assert_no_receipt();
    assert_eq!(
        case.corpus
            .kernel
            .outbox_consumer_checkpoint(CONSUMER)
            .unwrap(),
        Some(case.old_checkpoint)
    );
    config.episode.max_source_pages = NonZeroUsize::new(commits as usize).unwrap();
    let materialized = case.corpus.kernel.materialized_outbox_rows_for_test();
    case.selection
        .retire(
            &case.corpus.kernel,
            &case.gate,
            &config,
            &budget(Duration::from_secs(30)),
            &mut |_| {},
        )
        .unwrap();
    assert_eq!(
        case.corpus.kernel.materialized_outbox_rows_for_test(),
        materialized,
        "the completeness walk selected outbox payloads"
    );
    case.assert_receipt();
    assert_eq!(
        case.corpus
            .kernel
            .outbox_consumer_checkpoint(CONSUMER)
            .unwrap(),
        None
    );
}

#[test]
fn lost_ack_response_replays_receipt_while_old_consumer_remains_registered() {
    let root = tempfile::tempdir().unwrap();
    let mut case = RetirementCase::new(root.path());
    drop(case.old.take());
    let memberships = barrier_memberships(root.path());
    let reader = case
        .selection
        .pin(
            &case.corpus.kernel,
            &case.gate,
            &budget(Duration::from_secs(10)),
        )
        .unwrap();
    let path = reader.projection().path().to_owned();
    let mut acknowledged = false;
    let error = case
        .retire(root.path(), &mut |event| {
            if event == RetirementEvent::Acknowledged {
                acknowledged = true;
                let mut denied = support::projection_gate::passing_evaluator(
                    &spec(root.path()).identity,
                    0,
                    &daemon::projection_gates::ProjectionHook::ALL,
                );
                for enabled in denied.manifest.enabled.values_mut() {
                    *enabled = false;
                }
                case.gate.install(denied);
            }
        })
        .unwrap_err();
    assert!(acknowledged);
    assert!(matches!(error, BuildError::Expired), "{error:?}");
    assert_eq!(
        case.corpus
            .kernel
            .outbox_consumer_checkpoint(CONSUMER)
            .unwrap(),
        Some(case.target)
    );
    let stored_times = || {
        Connection::open(&path)
            .unwrap()
            .query_row(
                "SELECT retired_at,recorded_at FROM retirement_receipts",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .unwrap()
    };
    let before = stored_times();
    case.gate
        .install(support::projection_gate::passing_evaluator(
            &spec(root.path()).identity,
            0,
            &daemon::projection_gates::ProjectionHook::ALL,
        ));
    case.assert_receipt();
    let mut replay = Vec::new();
    case.retire(root.path(), &mut |event| {
        replay.push(event);
        if event == RetirementEvent::BeforeAcknowledgement {
            assert_eq!(
                case.corpus
                    .kernel
                    .outbox_consumer_checkpoint(CONSUMER)
                    .unwrap(),
                Some(case.target)
            );
        }
    })
    .unwrap();
    assert_eq!(
        replay,
        vec![
            RetirementEvent::LocalReleased,
            RetirementEvent::BeforeAcknowledgement,
            RetirementEvent::Acknowledged
        ]
    );
    case.assert_receipt();
    assert_eq!(stored_times(), before);
    assert_eq!(barrier_memberships(root.path()), memberships);
    assert_eq!(
        case.corpus
            .kernel
            .outbox_consumer_checkpoint(CONSUMER)
            .unwrap(),
        None
    );
}

pub(crate) fn retirement_child(root: &Path, cut: &str) {
    let mut case = RetirementCase::new(root);
    drop(case.old.take());
    let ledger: Vec<_> = case
        .expected
        .iter()
        .map(|fact| {
            (
                &fact.kind,
                &fact.identity,
                &fact.artifact_digest,
                fact.commit_seq,
                fact.invalidated_commit_seq,
                "removed",
            )
        })
        .collect();
    println!(
        "WITNESS {}",
        serde_json::json!({"old":case.old_digest,"old_ack":case.old_checkpoint,"target":case.target,"ledger":ledger})
    );
    case.retire(root, &mut |event| {
        if event == RetirementEvent::BeforeReceiptCommit && cut == "retire-after-commit" {
            storage::after_commit_for_test(|| park("retire-after-commit"));
        }
        let name = match event {
            RetirementEvent::BeforeCleanup => "retire-before-cleanup",
            RetirementEvent::Removed => "retire-removed",
            RetirementEvent::BeforeReceiptCommit => "retire-before-commit",
            RetirementEvent::LocalReleased => "retire-local-release",
            RetirementEvent::BeforeAcknowledgement => "retire-before-ack",
            RetirementEvent::Acknowledged => "retire-after-ack",
        };
        if name == cut {
            park(cut);
        }
    })
    .unwrap();
    panic!("cut did not fire");
}

#[test]
fn receipt_process_cuts_recover_twice_and_replay_lost_ack_response() {
    for cut in [
        "retire-before-cleanup",
        "retire-removed",
        "retire-before-commit",
        "retire-after-commit",
        "retire-local-release",
        "retire-before-ack",
        "retire-after-ack",
    ] {
        let root = tempfile::tempdir().unwrap();
        let witness = kill_child_at(root.path(), cut)["WITNESS"].clone();
        for reopen in 0..2 {
            let corpus = Corpus::open(root.path());
            if reopen == 0 {
                let expected = if cut == "retire-after-ack" {
                    witness["target"].as_i64()
                } else {
                    witness["old_ack"].as_i64()
                };
                assert_eq!(
                    corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
                    expected,
                    "{cut}"
                );
            }
            let gate = open_gate();
            let selection = selector(root.path());
            selection
                .retire(
                    &corpus.kernel,
                    &gate,
                    &spec(root.path()),
                    &budget(Duration::from_secs(30)),
                    &mut |_| {},
                )
                .unwrap();
            assert_eq!(
                corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
                None
            );
            let reader = selection
                .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                .unwrap();
            reader
                .read(&budget(Duration::from_secs(10)), |conn| {
                    assert_eq!(
                        conn.query_row("SELECT count(*) FROM retirement_receipts", [], |row| row
                            .get::<_, i64>(
                            0
                        ))?,
                        1
                    );
                    let ledger = conn.prepare("SELECT kind,identity,artifact_digest,commit_seq,invalidated_commit_seq,disposition FROM retirement_dispositions ORDER BY kind,identity")?
                        .query_map([],|row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,i64>(3)?,row.get::<_,Option<i64>>(4)?,row.get::<_,String>(5)?)))?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    assert_eq!(serde_json::to_value(ledger).unwrap(),witness["ledger"],"{cut}, reopen {reopen}");
                    Ok(())
                })
                .unwrap();
        }
    }
}

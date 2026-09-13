use super::*;
use daemon::search_replacement::selection::{SearchReader, SearchSelection, SelectionEvent};
use host_runtime::generation::{GenerationError, ProfileEvent};
use retrieval::coverage::CoverageBounds;

fn coverage_bounds() -> CoverageBounds {
    CoverageBounds {
        max_live_per_class: NonZeroUsize::new(256).unwrap(),
        max_tombstoned_per_class: NonZeroUsize::new(256).unwrap(),
    }
}

fn selector(root: &Path) -> SearchSelection {
    SearchSelection::new(root, spec(root).identity, coverage_bounds())
}

fn build_selected(root: &Path, corpus: &Corpus, gate: &HookGate) -> SearchSelection {
    let config = spec(root);
    record(root, gate, None, &config.identity);
    let candidate = ReplacementBuilder::open(root, &corpus.kernel, gate, config)
        .unwrap()
        .build(&budget(Duration::from_secs(30)), &mut |_| {})
        .unwrap();
    let selection = selector(root);
    selection.select(candidate, &mut |_| Ok(())).unwrap();
    selection
}

fn next_candidate<'a>(
    root: &Path,
    corpus: &'a Corpus,
    gate: &'a HookGate,
) -> daemon::search_replacement::VerifiedReplacement<'a> {
    candidate_for(root, corpus, gate, "second")
}

fn candidate_for<'a>(
    root: &Path,
    corpus: &'a Corpus,
    gate: &'a HookGate,
    label: &str,
) -> daemon::search_replacement::VerifiedReplacement<'a> {
    // The prior-family fixture archives its unfinished episode; it does not model coordinator completion.
    std::fs::rename(
        root.join("search-lifecycle/intent.json"),
        root.join(format!("search-lifecycle/prior-{label}-intent.json")),
    )
    .unwrap();
    let mut config = spec(root);
    config.generation.generation_id = format!("{label}-vector-generation");
    let mut next = request(None, &config.identity);
    next.consumer.consumer_id = format!("{label}-consumer");
    next.consumer.generation_id = config.generation.generation_id.clone();
    next.attempt_id = format!("{label}-attempt");
    next.selected_generation = match GenerationStore::open(Some(root))
        .unwrap()
        .read_search_current()
        .unwrap()
    {
        CurrentProfile::Current(digest) => digest,
        other => panic!("{other:?}"),
    };
    ProjectionLifecycle::open(root)
        .unwrap()
        .record(gate, &next, now())
        .unwrap();
    ReplacementBuilder::open(root, &corpus.kernel, gate, config)
        .unwrap()
        .build(&budget(Duration::from_secs(30)), &mut |_| {})
        .unwrap()
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Observation {
    rows: Rows,
    checkpoint: i64,
    pending: Vec<String>,
    generation: String,
}

fn observe(reader: &SearchReader) -> Observation {
    reader.read(&budget(Duration::from_secs(10)), |conn| {
        let rows = conn.prepare("SELECT o.source_object_id,o.class,p.bytes,t.invalidated_commit_seq FROM occurrences o JOIN payloads p USING(payload_id) LEFT JOIN occurrence_tombstones t USING(occurrence_id) ORDER BY o.source_object_id")?
            .query_map([], |row| Ok((row.get(0)?, (row.get(1)?,row.get(2)?,row.get(3)?))))?.collect::<rusqlite::Result<_>>()?;
        let checkpoint = conn.query_row("SELECT checkpoint_commit_seq FROM projection_checkpoint", [], |row| row.get(0))?;
        let pending = conn.prepare("SELECT o.source_object_id FROM embedding_jobs j JOIN occurrences o USING(occurrence_id) WHERE j.state='pending' ORDER BY o.source_object_id")?.query_map([], |row| row.get(0))?.collect::<rusqlite::Result<_>>()?;
        let generation = conn.query_row("SELECT generation_id FROM vector_generations", [], |row| row.get(0))?;
        Ok(Observation { rows, checkpoint, pending, generation })
    }).unwrap()
}

#[test]
fn readers_pin_complete_old_or_new_prefix_and_keep_cancelled_physical_workers_owned() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    let base = corpus.publish("base", "old bytes");
    let gate = open_gate();
    let selection = build_selected(root.path(), &corpus, &gate);
    let old = selection
        .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
        .unwrap();
    let old_digest = old.digest().to_owned();
    let old_path = old.projection().path().to_owned();
    let before = observe(&old);
    assert_eq!(before.rows[&base].1, b"old bytes");
    let old_ack = corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap();
    let late = corpus.publish("late", "new bytes");
    let candidate = next_candidate(root.path(), &corpus, &gate);
    let target = candidate.staged().verification.checkpoint_commit_seq;
    let new_digest = candidate.staged().digest.clone();
    let (ready, running) = mpsc::channel();
    let engine = fixtures::TestEngine::new();
    let native_gate = engine.block_calls();
    let release = fixtures::GateGuard(Arc::clone(&native_gate));
    let component = fixtures::component(&engine, host_runtime::synapse::SynapseLimits::default());
    let worker_pin = old.clone();
    let cancelled = budget(Duration::from_secs(30));
    let worker_budget = cancelled.clone();
    let worker = std::thread::spawn(move || {
        ready.send(()).unwrap();
        let vectors = component.embed_blocking(&["old bytes"]).unwrap();
        assert_eq!(vectors, vec![fixtures::TestEngine::vector_for("old bytes")]);
        assert!(worker_budget.is_exhausted());
        observe(&worker_pin)
    });
    running.recv().unwrap();
    let started = std::time::Instant::now();
    while engine.calls() == 0 {
        assert!(started.elapsed() < Duration::from_secs(10));
        std::thread::yield_now();
    }
    selection
        .select(candidate, &mut |event| {
            if event == SelectionEvent::Profile(ProfileEvent::BeforeRename) {
                assert_eq!(
                    observe(
                        &selection
                            .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                            .unwrap()
                    ),
                    before
                );
            }
            Ok(())
        })
        .unwrap();
    cancelled.cancel();
    let new = selection
        .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
        .unwrap();
    assert_eq!(new.digest(), new_digest);
    let after = observe(&new);
    assert_eq!(after.checkpoint, target);
    assert_eq!(after.generation, "second-vector-generation");
    assert_eq!(after.rows.len(), 2);
    assert_eq!(after.rows[&late].1, b"new bytes");
    assert_eq!(after.pending, expected_pending(&after.rows));
    assert_eq!(observe(&old), before);
    assert_eq!(
        corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
        old_ack
    );
    let lock =
        host_runtime::LifecycleTransactionLock::acquire_exclusive(Some(root.path())).unwrap();
    assert!(
        !ProjectionLifecycle::protected_generations(root.path(), &lock)
            .unwrap()
            .contains(&old_digest)
    );
    drop(lock);
    assert!(matches!(
        selection.reclaim(&old_digest),
        Err(BuildError::Projection(
            daemon::search_projection::SearchProjectionError::Store(storage::StoreError::Lease(
                lease::LeaseError::Held { .. }
            ))
        ))
    ));
    drop(old);
    assert!(matches!(
        selection.reclaim(&old_digest),
        Err(BuildError::Projection(
            daemon::search_projection::SearchProjectionError::Store(storage::StoreError::Lease(
                lease::LeaseError::Held { .. }
            ))
        ))
    ));
    let store = GenerationStore::open(Some(root.path())).unwrap();
    {
        let lock =
            host_runtime::LifecycleTransactionLock::acquire_exclusive(Some(root.path())).unwrap();
        store
            .prune(&ProjectionLifecycle::protected_generations(root.path(), &lock).unwrap())
            .unwrap();
        store.validate(&old_digest).unwrap();
        store.validate(&new_digest).unwrap();
    }
    assert_eq!(engine.completed(), 0);
    drop(release);
    assert_eq!(worker.join().unwrap(), before);
    selection.reclaim(&old_digest).unwrap();
    assert!(!old_path.exists());
    assert!(
        !old_path
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("bootstrap.json")
            .exists()
    );
    selection.reclaim(&old_digest).unwrap();
    assert!(selection.reclaim(&new_digest).is_err());
    assert_eq!(store.read_current().unwrap(), CurrentProfile::Absent);
}

#[test]
fn all_five_identity_changes_require_rebuild_on_selection_and_reopen() {
    for field in ["schema", "tokenizer", "model", "policy", "contract"] {
        let root = tempfile::tempdir().unwrap();
        let corpus = Corpus::open(root.path());
        corpus.seed();
        corpus.publish("base", "bytes");
        let gate = open_gate();
        let selection = build_selected(root.path(), &corpus, &gate);
        drop(selection);
        let mut changed = spec(root.path()).identity;
        match field {
            "schema" => changed.schema_version += 1,
            "tokenizer" => changed.tokenizer_fingerprint.push('x'),
            "model" => changed.embedding_model.push('x'),
            "policy" => changed.projection_policy_version.push('x'),
            "contract" => changed.identity_contract_version.push('x'),
            _ => unreachable!(),
        }
        let unavailable = SearchSelection::new(root.path(), changed.clone(), coverage_bounds());
        gate.install(support::projection_gate::passing_evaluator(
            &changed,
            0,
            &daemon::projection_gates::ProjectionHook::ALL,
        ));
        assert!(
            unavailable
                .reopen(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                .is_err(),
            "{field}"
        );
        assert!(
            unavailable
                .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                .is_err()
        );
        gate.install(support::projection_gate::passing_evaluator(
            &spec(root.path()).identity,
            0,
            &daemon::projection_gates::ProjectionHook::ALL,
        ));
        let candidate = next_candidate(root.path(), &corpus, &gate);
        assert!(
            matches!(
                unavailable.select(candidate, &mut |_| Ok(())),
                Err(error) if matches!(error.error, BuildError::Mutation(
                    retrieval::ProjectionError::IdentityMismatch
                ))
            ),
            "{field}"
        );
    }
}

#[test]
fn readable_semantic_corruption_never_becomes_available_after_reopen() {
    for sql in [
        "UPDATE projection_identity SET identity_contract_version='foreign'",
        "DELETE FROM embedding_jobs",
        "UPDATE projection_checkpoint SET checkpoint_commit_seq=checkpoint_commit_seq+1",
        "UPDATE occurrences SET tuple=x'00'",
        "UPDATE occurrences SET sensitivity='secret'",
        "UPDATE occurrences SET created_commit_seq=created_commit_seq-1",
        "UPDATE embedding_jobs SET job_id='wrong-job'",
    ] {
        let root = tempfile::tempdir().unwrap();
        let corpus = Corpus::open(root.path());
        corpus.seed();
        corpus.publish("base", "bytes");
        let gate = open_gate();
        let selection = build_selected(root.path(), &corpus, &gate);
        let reader = selection
            .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
            .unwrap();
        let path = reader.projection().path().to_owned();
        drop(reader);
        drop(selection);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(sql).unwrap();
        assert_eq!(
            conn.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        drop(conn);
        for _ in 0..2 {
            let selection = selector(root.path());
            assert!(
                selection
                    .reopen(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                    .is_err(),
                "{sql}"
            );
            assert!(
                selection
                    .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                    .is_err()
            );
        }
    }
}

#[test]
fn tampered_certificate_intent_never_becomes_available_after_reopen() {
    for (field, value) in [
        ("schema", serde_json::json!(999)),
        ("authorization_ref", serde_json::json!("op:1")),
    ] {
        let root = tempfile::tempdir().unwrap();
        let corpus = Corpus::open(root.path());
        corpus.seed();
        corpus.publish("base", "bytes");
        let gate = open_gate();
        let selection = build_selected(root.path(), &corpus, &gate);
        let reader = selection
            .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
            .unwrap();
        let certificate = reader
            .projection()
            .path()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("bootstrap.json");
        drop(reader);
        drop(selection);
        let mut bootstrap: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&certificate).unwrap()).unwrap();
        bootstrap["intent"][field] = value;
        std::fs::write(&certificate, serde_json::to_vec(&bootstrap).unwrap()).unwrap();
        let selection = selector(root.path());
        assert!(
            selection
                .reopen(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                .is_err(),
            "{field}"
        );
        assert!(
            selection
                .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                .is_err()
        );
    }
}

fn park(cut: &str) {
    println!("BARRIER {cut}");
    std::io::stdout().flush().unwrap();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap();
    panic!("parent must kill child");
}

pub(super) fn selection_child(root: &Path, cut: &str) {
    let corpus = Corpus::open(root);
    corpus.seed();
    corpus.publish("base", "before selection");
    let gate = open_gate();
    let selection = build_selected(root, &corpus, &gate);
    let old = selection
        .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
        .unwrap();
    let before = observe(&old);
    let old_digest = old.digest().to_owned();
    let old_ack = corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap();
    corpus.publish("late", "after selection");
    let candidate = next_candidate(root, &corpus, &gate);
    let new_digest = candidate.staged().digest.clone();
    let after_rows = rows(candidate.path());
    let after = Observation {
        pending: expected_pending(&after_rows),
        rows: after_rows,
        checkpoint: candidate.staged().verification.checkpoint_commit_seq,
        generation: candidate.staged().verification.generation_id.clone(),
    };
    println!(
        "WITNESS {}",
        serde_json::json!({"before":before,"after":after,"old":old_digest,"new":new_digest,"old_ack":old_ack})
    );
    selection
        .select(candidate, &mut |event| {
            if cut == "select-metadata-prefix" && event == SelectionEvent::MetadataWritten {
                let path = root
                    .join("search-families")
                    .join(&new_digest)
                    .join("bootstrap.json");
                let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
                file.set_len(file.metadata().unwrap().len() / 2).unwrap();
                park(cut);
            }
            let name = match event {
                SelectionEvent::MetadataWritten => "select-metadata",
                SelectionEvent::Copied => "select-copy",
                SelectionEvent::Profile(event) => match event {
                    ProfileEvent::BeforeRename => "select-before-rename",
                    ProfileEvent::AfterRename => "select-after-rename",
                    ProfileEvent::BeforeDirectorySync => "select-before-sync",
                    ProfileEvent::AfterDirectorySync => "select-after-sync",
                },
            };
            if cut == name {
                park(cut);
            }
            Ok(())
        })
        .unwrap();
    if cut.starts_with("active-") {
        let reader = selection
            .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
            .unwrap();
        let last = corpus.publish("active", "first active write");
        let cp = reader
            .coverage(&budget(Duration::from_secs(10)))
            .unwrap()
            .checkpoint;
        let sources = corpus.export();
        let identities = retrieval::batch::row_identities(&sources);
        let target = corpus.tip();
        let mut expected = observe(&reader);
        let prior = serde_json::to_value(&expected).unwrap();
        expected.rows.insert(
            last,
            ("messages".to_owned(), b"first active write".to_vec(), None),
        );
        expected.checkpoint = target;
        expected.pending = expected_pending(&expected.rows);
        println!(
            "WITNESS {}",
            serde_json::json!({"before":prior,"after":expected,"old_ack":old_ack})
        );
        let batch = retrieval::batch::batch_from_rows(
            &sources,
            &identities,
            retrieval::batch::MutationIdentity {
                kernel_incarnation_id: spec(root).identity.kernel_incarnation_id,
                hold_id: cp.hold_id.clone(),
                snapshot_commit_seq: cp.snapshot_commit_seq,
                through_commit_seq: target,
            },
            Some("second-vector-generation"),
        )
        .unwrap();
        reader
            .projection()
            .write(|conn| {
                retrieval::batch::apply_batch(conn, &batch, batch_bounds(), now())?;
                if cut == "active-before-commit" {
                    park(cut);
                }
                Ok(())
            })
            .unwrap();
        if cut == "active-after-commit" {
            park(cut);
        }
    }
    panic!("cut did not fire");
}

#[test]
fn selector_process_cuts_reopen_twice_without_releasing_old_consumers() {
    for cut in [
        "select-before-rename",
        "select-after-rename",
        "select-before-sync",
        "select-after-sync",
    ] {
        let root = tempfile::tempdir().unwrap();
        let witness = kill_child_at(root.path(), cut)["WITNESS"].clone();
        for _ in 0..2 {
            let corpus = Corpus::open(root.path());
            let gate = open_gate();
            let selection = selector(root.path());
            selection
                .reopen(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                .unwrap();
            let reader = selection
                .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                .unwrap();
            let expected = if cut == "select-before-rename" {
                "before"
            } else {
                "after"
            };
            assert_eq!(
                serde_json::to_value(observe(&reader)).unwrap(),
                witness[expected]
            );
            assert_eq!(
                corpus
                    .kernel
                    .outbox_consumer_checkpoint(CONSUMER)
                    .unwrap()
                    .unwrap(),
                witness["old_ack"].as_i64().unwrap()
            );
        }
    }
}

#[test]
fn lost_selector_reply_reconciles_durable_pointer() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    corpus.publish("base", "bytes");
    let gate = open_gate();
    let selection = build_selected(root.path(), &corpus, &gate);
    corpus.publish("late", "late bytes");
    let candidate = next_candidate(root.path(), &corpus, &gate);
    let digest = candidate.staged().digest.clone();
    assert!(
        selection
            .select(candidate, &mut |event| if event
                == SelectionEvent::Profile(ProfileEvent::AfterDirectorySync)
            {
                Err(GenerationError::NativePayloadInvalid {
                    detail: "lost reply",
                })
            } else {
                Ok(())
            })
            .is_err()
    );
    for _ in 0..2 {
        selection
            .reopen(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
            .unwrap();
        assert_eq!(
            selection
                .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                .unwrap()
                .digest(),
            digest
        );
    }
}

#[test]
fn first_active_commit_recovers_its_own_wal_twice_without_seed_digest_comparison() {
    use sha2::Digest;
    for cut in ["active-before-commit", "active-after-commit"] {
        let root = tempfile::tempdir().unwrap();
        let witness = kill_child_at(root.path(), cut)["WITNESS"].clone();
        for _ in 0..2 {
            let corpus = Corpus::open(root.path());
            let gate = open_gate();
            let selection = selector(root.path());
            selection
                .reopen(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                .unwrap();
            let reader = selection
                .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                .unwrap();
            assert_eq!(
                serde_json::to_value(observe(&reader)).unwrap(),
                witness[if cut == "active-before-commit" {
                    "before"
                } else {
                    "after"
                }]
            );
            reader.coverage(&budget(Duration::from_secs(10))).unwrap();
            reader
                .projection()
                .checkpoint_truncate(std::time::Instant::now() + Duration::from_secs(10))
                .unwrap();
            let store = GenerationStore::open(Some(root.path())).unwrap();
            let seed = store.validate(reader.digest()).unwrap();
            let immutable_hash = &seed
                .manifest
                .files
                .iter()
                .find(|file| file.path == "search.sqlite")
                .unwrap()
                .sha256;
            let active_hash = format!(
                "{:x}",
                sha2::Sha256::digest(std::fs::read(reader.projection().path()).unwrap())
            );
            assert_ne!(&active_hash, immutable_hash);
            assert_eq!(
                corpus
                    .kernel
                    .outbox_consumer_checkpoint(CONSUMER)
                    .unwrap()
                    .unwrap(),
                witness["old_ack"].as_i64().unwrap()
            );
        }
    }
}

#[test]
fn pinned_reader_rejudges_changed_canonical_eligibility() {
    use retrieval::eligibility::Disposition;
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    let object = corpus.publish("base", "bytes");
    let gate = open_gate();
    let selection = build_selected(root.path(), &corpus, &gate);
    let old = selection
        .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
        .unwrap();
    let project = kernel::ProjectScope::new(fixtures::PROJECT).unwrap();
    assert_eq!(
        old.authorize(
            &corpus.kernel,
            &project,
            kernel::ArtifactDestination::Remote,
            &budget(Duration::from_secs(10))
        )
        .unwrap()
        .occurrences[0]
            .disposition,
        Disposition::Eligible
    );
    corpus.retire(&object);
    let candidate = next_candidate(root.path(), &corpus, &gate);
    selection.select(candidate, &mut |_| Ok(())).unwrap();
    assert!(matches!(
        old.authorize(
            &corpus.kernel,
            &project,
            kernel::ArtifactDestination::Remote,
            &budget(Duration::from_secs(10))
        )
        .unwrap()
        .occurrences[0]
            .disposition,
        Disposition::PolicyExcluded(_)
    ));
    assert_eq!(observe(&old).rows.len(), 1);
}

#[test]
fn retention_loss_at_the_final_selector_boundary_cannot_publish_a_candidate() {
    for fault in ["expiry", "purge"] {
        let root = tempfile::tempdir().unwrap();
        let corpus = Corpus::open(root.path());
        corpus.seed();
        corpus.publish("base", "base bytes");
        let gate = open_gate();
        let selection = build_selected(root.path(), &corpus, &gate);
        let old = selection
            .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
            .unwrap();
        let digest = old.digest().to_owned();
        let candidate = next_candidate(root.path(), &corpus, &gate);
        let hold = candidate.staged().verification.hold_id.clone();
        let mut fired = false;
        let failure = selection
            .select(candidate, &mut |event| {
                if event == SelectionEvent::Profile(ProfileEvent::BeforeRename) {
                    fired = true;
                    if fault == "expiry" {
                        Connection::open(root.path().join("kernel/kernel.sqlite"))
                            .unwrap()
                            .execute(
                                "UPDATE capture_pins SET expires_at=0 WHERE capture_pin_id=?1",
                                [&hold],
                            )
                            .unwrap();
                    } else {
                        corpus
                            .kernel
                            .delete_artifact(kernel::ArtifactDeletionRequest {
                                intent: intent("purge-selection"),
                                identity: kernel::ArtifactDeletionIdentity::EvidenceId(
                                    "evidence-base".to_owned(),
                                ),
                                kind: kernel::ArtifactDeletionKind::Purge,
                                operator_id: Some("operator".to_owned()),
                                target_locator: Some("fixture".to_owned()),
                                reason: Some("fixture".to_owned()),
                                deleted_at: now(),
                            })
                            .unwrap();
                    }
                }
                Ok(())
            })
            .unwrap_err();
        let expected = if fault == "expiry" {
            kernel::SourceHoldInvalidity::Expired
        } else {
            kernel::SourceHoldInvalidity::PurgeDegraded
        };
        assert!(
            matches!(failure.error, BuildError::Hold(kernel::SourceHoldError::Invalid(reason)) if reason == expected)
        );
        assert!(fired);
        assert_eq!(
            GenerationStore::open(Some(root.path()))
                .unwrap()
                .read_search_current()
                .unwrap(),
            CurrentProfile::Current(digest.clone())
        );
        assert_eq!(
            selection
                .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                .unwrap()
                .digest(),
            digest
        );
        assert!(
            corpus
                .kernel
                .outbox_consumer_checkpoint(CONSUMER)
                .unwrap()
                .is_some()
        );
    }
}

#[test]
fn incomplete_candidate_with_incompatible_prior_remains_unavailable_and_gates_stay_closed() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    corpus.publish("base", "bytes");
    let gate = open_gate();
    let prior = build_selected(root.path(), &corpus, &gate);
    drop(prior);
    let store = GenerationStore::open(Some(root.path())).unwrap();
    let digest = match store.read_search_current().unwrap() {
        CurrentProfile::Current(digest) => digest,
        _ => unreachable!(),
    };
    Connection::open(
        root.path()
            .join("search-families")
            .join(digest)
            .join("search/search.sqlite"),
    )
    .unwrap()
    .execute(
        "UPDATE projection_identity SET projection_policy_version='wrong'",
        [],
    )
    .unwrap();
    let selection = selector(root.path());
    assert!(
        selection
            .reopen(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
            .is_err()
    );
    let candidate = next_candidate(root.path(), &corpus, &gate);
    Connection::open(candidate.path())
        .unwrap()
        .execute("DELETE FROM embedding_jobs", [])
        .unwrap();
    assert!(selection.select(candidate, &mut |_| Ok(())).is_err());
    assert!(
        selection
            .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
            .is_err()
    );
    assert!(
        selection
            .reopen(
                &corpus.kernel,
                &HookGate::closed(),
                &budget(Duration::from_secs(10))
            )
            .is_err()
    );
}

#[test]
fn active_vector_coverage_survives_reopen_and_invalid_vectors_are_refused() {
    use sha2::Digest;
    for state in [
        "pending",
        "embedded",
        "nonfinite",
        "missing-vector",
        "wrong-dimension",
        "foreign-jobs",
        "excluded-class-job",
    ] {
        let root = tempfile::tempdir().unwrap();
        let corpus = Corpus::open(root.path());
        corpus.seed();
        corpus.publish("base", "bytes");
        if state == "excluded-class-job" {
            corpus.publish_class("tool", "tool bytes", "raw_tool_spans");
        }
        let gate = open_gate();
        let selection = build_selected(root.path(), &corpus, &gate);
        let reader = selection
            .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
            .unwrap();
        reader.projection().write(|conn| {
            let vector = if state == "nonfinite" { vec![f32::NAN; fixtures::DIMS] } else { fixtures::TestEngine::vector_for("bytes") };
            if state != "missing-vector" {
                conn.execute("INSERT INTO occurrence_vectors SELECT occurrence_id,generation_id,?1,?2,5,1,0 FROM embedding_jobs", rusqlite::params![retrieval::vectors::encode(&vector), fixtures::DIMS as i64])?;
            }
            if state != "pending" { conn.execute("UPDATE embedding_jobs SET state='embedded'", [])?; }
            if state == "wrong-dimension" { conn.execute("UPDATE occurrence_vectors SET vector_dimension=7,vector=zeroblob(28)", [])?; }
            if state == "foreign-jobs" {
                conn.execute("INSERT INTO vector_generations SELECT 'foreign',embedding_model,tokenizer_fingerprint,vector_dimension,generation_epoch,state,created_at,updated_at FROM vector_generations", [])?;
                conn.execute("UPDATE embedding_jobs SET generation_id='foreign',state='pending'", [])?;
            }
            if state == "excluded-class-job" {
                let tool: String = conn.query_row("SELECT occurrence_id FROM occurrences WHERE class='raw_tool_spans'", [], |row| row.get(0))?;
                let mut hasher = sha2::Sha256::new();
                hasher.update(tool.as_bytes());
                hasher.update([0x1f]);
                hasher.update(fixtures::GENERATION.as_bytes());
                conn.execute(
                    "INSERT INTO embedding_jobs(job_id,occurrence_id,generation_id,state,created_at,updated_at)
                     SELECT ?1,?2,generation_id,'pending',created_at,updated_at FROM embedding_jobs LIMIT 1",
                    rusqlite::params![format!("{:x}", hasher.finalize()), tool],
                )?;
            }
            Ok(())
        }).unwrap();
        drop(reader);
        drop(selection);
        for _ in 0..2 {
            let selection = selector(root.path());
            let result = selection.reopen(&corpus.kernel, &gate, &budget(Duration::from_secs(10)));
            if matches!(
                state,
                "nonfinite"
                    | "missing-vector"
                    | "wrong-dimension"
                    | "foreign-jobs"
                    | "excluded-class-job"
            ) {
                assert!(result.is_err(), "{state}");
            } else {
                result.unwrap();
                let report = selection
                    .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                    .unwrap()
                    .coverage(&budget(Duration::from_secs(10)))
                    .unwrap();
                assert_eq!(
                    report
                        .classes
                        .iter()
                        .map(|class| class.valid_vectors)
                        .sum::<usize>(),
                    1
                );
                assert_eq!(
                    report
                        .classes
                        .iter()
                        .map(|class| class.missing)
                        .sum::<usize>(),
                    0
                );
            }
        }
    }
}

#[test]
fn concurrent_queries_observe_one_family_for_rows_checkpoint_and_job_set() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    let base = corpus.publish("base", "old bytes");
    let gate = open_gate();
    let selection = build_selected(root.path(), &corpus, &gate);
    let old = selection
        .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
        .unwrap();
    let old_checkpoint = old
        .coverage(&budget(Duration::from_secs(10)))
        .unwrap()
        .checkpoint
        .checkpoint_commit_seq;
    let late = corpus.publish("late", "new bytes");
    let candidate = next_candidate(root.path(), &corpus, &gate);
    let target = candidate.staged().verification.checkpoint_commit_seq;
    let before_rows =
        BTreeMap::from([(base, ("messages".to_owned(), b"old bytes".to_vec(), None))]);
    let mut after_rows = before_rows.clone();
    after_rows.insert(late, ("messages".to_owned(), b"new bytes".to_vec(), None));
    let before = Observation {
        pending: expected_pending(&before_rows),
        rows: before_rows,
        checkpoint: old_checkpoint,
        generation: fixtures::GENERATION.to_owned(),
    };
    let after = Observation {
        pending: expected_pending(&after_rows),
        rows: after_rows,
        checkpoint: target,
        generation: "second-vector-generation".to_owned(),
    };
    let ready = std::sync::Barrier::new(9);
    let published = std::sync::Barrier::new(9);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                let pinned = selection
                    .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                    .unwrap();
                assert_eq!(observe(&pinned), before);
                ready.wait();
                published.wait();
                for _ in 0..20 {
                    assert_eq!(observe(&pinned), before);
                    let current = selection
                        .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                        .unwrap();
                    assert_eq!(observe(&current), after);
                }
            });
        }
        ready.wait();
        selection.select(candidate, &mut |_| Ok(())).unwrap();
        published.wait();
    });
}

#[test]
fn revoked_gate_refuses_existing_pins_without_releasing_the_family() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    corpus.publish("base", "bytes");
    let gate = open_gate();
    let selection = build_selected(root.path(), &corpus, &gate);
    let reader = selection
        .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
        .unwrap();
    let changed = spec(root.path()).identity;
    gate.install(support::projection_gate::passing_evaluator(
        &changed,
        0,
        &[],
    ));
    assert!(
        reader
            .read(&budget(Duration::from_secs(10)), |_| Ok(()))
            .is_err()
    );
    assert!(
        selection
            .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
            .is_err()
    );
}

#[test]
fn postpublication_validation_failure_withdraws_old_selection() {
    for cut in [ProfileEvent::AfterRename, ProfileEvent::AfterDirectorySync] {
        let root = tempfile::tempdir().unwrap();
        let corpus = Corpus::open(root.path());
        corpus.seed();
        corpus.publish("base", "base bytes");
        let gate = open_gate();
        let selection = build_selected(root.path(), &corpus, &gate);
        let old = selection
            .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
            .unwrap();
        let candidate = next_candidate(root.path(), &corpus, &gate);
        let new_digest = candidate.staged().digest.clone();
        let hold = candidate.staged().verification.hold_id.clone();
        let mut fired = false;
        let failure = selection
            .select(candidate, &mut |event| {
                if event == SelectionEvent::Profile(cut) {
                    fired = true;
                    Connection::open(root.path().join("kernel/kernel.sqlite"))
                        .unwrap()
                        .execute(
                            "UPDATE capture_pins SET expires_at=0 WHERE capture_pin_id=?1",
                            [&hold],
                        )
                        .unwrap();
                }
                Ok(())
            })
            .unwrap_err();
        assert!(fired);
        assert!(matches!(
            failure.error,
            BuildError::Hold(kernel::SourceHoldError::Invalid(
                kernel::SourceHoldInvalidity::Expired
            ))
        ));
        assert!(
            selection
                .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                .is_err()
        );
        assert_eq!(
            GenerationStore::open(Some(root.path()))
                .unwrap()
                .read_search_current()
                .unwrap(),
            CurrentProfile::Current(new_digest.clone())
        );
        failure
            .reconcile(&selection, &budget(Duration::from_secs(10)))
            .unwrap();
        assert_eq!(
            selection
                .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                .unwrap()
                .digest(),
            new_digest
        );
        assert_ne!(old.digest(), new_digest);
    }
}

#[test]
fn preparation_failure_retains_candidate_and_retries_only_its_owned_partial_family() {
    for cut in [
        SelectionEvent::MetadataWritten,
        SelectionEvent::Copied,
        SelectionEvent::Profile(ProfileEvent::BeforeRename),
    ] {
        let root = tempfile::tempdir().unwrap();
        let corpus = Corpus::open(root.path());
        corpus.seed();
        corpus.publish("base", "bytes");
        let gate = open_gate();
        let selection = build_selected(root.path(), &corpus, &gate);
        let old = selection
            .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
            .unwrap();
        let candidate = next_candidate(root.path(), &corpus, &gate);
        let new_digest = candidate.staged().digest.clone();
        let mut fired = false;
        let failure = selection
            .select(candidate, &mut |event| {
                if event == cut {
                    fired = true;
                    return Err(GenerationError::NativePayloadInvalid {
                        detail: "lost preparation reply",
                    });
                }
                Ok(())
            })
            .unwrap_err();
        assert!(fired);
        failure
            .reconcile(&selection, &budget(Duration::from_secs(10)))
            .unwrap();
        assert_eq!(
            selection
                .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                .unwrap()
                .digest(),
            old.digest()
        );
        failure.retry(&selection, &mut |_| Ok(())).unwrap();
        let new = selection
            .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
            .unwrap();
        assert_eq!(new.digest(), new_digest);
        assert_eq!(new.handoff(), &control(root.path()));
        assert!(new.handoff().replacement_capture.is_some());
        assert!(
            corpus
                .kernel
                .outbox_consumer_checkpoint(CONSUMER)
                .unwrap()
                .is_some()
        );
    }
}

#[test]
fn preparation_process_cuts_recover_using_the_durable_certificate_twice() {
    for cut in ["select-copy", "select-metadata", "select-metadata-prefix"] {
        let root = tempfile::tempdir().unwrap();
        let witness = kill_child_at(root.path(), cut)["WITNESS"].clone();
        let corpus = Corpus::open(root.path());
        let gate = open_gate();
        let selection = selector(root.path());
        let captured = control(root.path());
        for _ in 0..2 {
            let handoff = selection.recover_partial(&gate).unwrap();
            assert_eq!(handoff, captured);
            let home = root
                .path()
                .join("search-families")
                .join(witness["new"].as_str().unwrap());
            assert!(!home.join("search/search.sqlite").exists());
            assert!(!home.join("bootstrap.json").exists());
            selection
                .reopen(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                .unwrap();
            assert_eq!(
                selection
                    .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                    .unwrap()
                    .digest(),
                witness["old"].as_str().unwrap()
            );
        }
    }
}

#[test]
fn foreign_partial_certificate_refuses_cleanup_and_preserves_candidate_ownership() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    corpus.publish("base", "bytes");
    let gate = open_gate();
    let selection = build_selected(root.path(), &corpus, &gate);
    let candidate = next_candidate(root.path(), &corpus, &gate);
    let digest = candidate.staged().digest.clone();
    let failure = selection
        .select(candidate, &mut |event| {
            if event == SelectionEvent::MetadataWritten {
                return Err(GenerationError::NativePayloadInvalid {
                    detail: "stop before copy",
                });
            }
            Ok(())
        })
        .unwrap_err();
    let path = root
        .path()
        .join("search-families")
        .join(digest)
        .join("bootstrap.json");
    let certificate = std::fs::read(&path).unwrap();
    std::fs::write(&path, b"foreign owner").unwrap();
    let failure = failure.retry(&selection, &mut |_| Ok(())).unwrap_err();
    assert!(matches!(
        failure.error,
        BuildError::Invalid("foreign family certificate")
    ));
    assert_eq!(std::fs::read(&path).unwrap(), b"foreign owner");
    std::fs::write(path, certificate).unwrap();
    failure.retry(&selection, &mut |_| Ok(())).unwrap();
}

#[test]
fn same_manager_reopen_reuses_live_pins_and_quarantines_corruption() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    corpus.publish("base", "bytes");
    let gate = open_gate();
    let selection = build_selected(root.path(), &corpus, &gate);
    let old = selection
        .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
        .unwrap();
    for _ in 0..2 {
        selection
            .reopen(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
            .unwrap();
        let new = selection
            .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
            .unwrap();
        assert!(std::ptr::eq(old.projection(), new.projection()));
    }
    assert!(matches!(
        selector(root.path()).reopen(&corpus.kernel, &gate, &budget(Duration::from_secs(10))),
        Err(BuildError::Projection(
            daemon::search_projection::SearchProjectionError::Store(storage::StoreError::Lease(
                lease::LeaseError::Held { .. }
            ))
        ))
    ));
    old.projection()
        .write(|conn| {
            conn.execute("DELETE FROM embedding_jobs", [])?;
            Ok(())
        })
        .unwrap();
    assert!(
        selection
            .reopen(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
            .is_err()
    );
    assert!(
        old.read(&budget(Duration::from_secs(10)), |_| Ok(()))
            .is_err()
    );
    assert!(
        selection
            .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
            .is_err()
    );
}

#[test]
fn same_manager_identity_corruption_withdraws_existing_and_new_pins() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    corpus.publish("base", "bytes");
    let gate = open_gate();
    let selection = build_selected(root.path(), &corpus, &gate);
    let old = selection
        .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
        .unwrap();
    old.projection()
        .write(|conn| {
            conn.execute(
                "UPDATE projection_identity SET projection_policy_version='foreign'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(
        selection
            .reopen(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
            .is_err()
    );
    assert!(
        selection
            .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
            .is_err()
    );
    assert!(
        old.read(&budget(Duration::from_secs(10)), |_| Ok(()))
            .is_err()
    );
}

#[test]
fn transient_reopen_failures_keep_the_live_family_without_quarantine() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    corpus.publish("base", "bytes");
    let gate = open_gate();
    let selection = build_selected(root.path(), &corpus, &gate);
    let old = selection
        .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
        .unwrap();
    let before = observe(&old);
    let reopen_reuses_live_family = || {
        selection
            .reopen(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
            .unwrap();
        let current = selection
            .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
            .unwrap();
        assert!(std::ptr::eq(old.projection(), current.projection()));
        assert_eq!(observe(&current), before);
    };

    let cancelled = budget(Duration::from_secs(10));
    cancelled.cancel();
    assert!(matches!(
        selection.reopen(&corpus.kernel, &gate, &cancelled),
        Err(BuildError::Expired)
    ));
    assert_eq!(observe(&old), before);
    reopen_reuses_live_family();

    assert!(matches!(
        selection.reopen(
            &corpus.kernel,
            &gate,
            &kernel::applicability::EvalBudget::unbounded()
        ),
        Err(BuildError::Invalid("selection requires a finite budget"))
    ));
    assert_eq!(observe(&old), before);
    reopen_reuses_live_family();

    // The reader releases after 1.5 seconds, so a quarantine that waits for the connection
    // completes and fails the assertion below instead of deadlocking the scope.
    let held = std::sync::Barrier::new(2);
    let contended = std::thread::scope(|scope| {
        scope.spawn(|| {
            old.projection()
                .read(|_| {
                    held.wait();
                    std::thread::sleep(Duration::from_millis(1500));
                    Ok(())
                })
                .unwrap();
        });
        held.wait();
        selection.reopen(&corpus.kernel, &gate, &budget(Duration::from_millis(300)))
    });
    assert!(matches!(
        contended,
        Err(BuildError::Projection(
            daemon::search_projection::SearchProjectionError::Store(storage::StoreError::Deadline)
        ))
    ));
    assert!(old.projection().quarantine().is_none());
    assert_eq!(observe(&old), before);
    reopen_reuses_live_family();

    // Installing an evaluator invalidates `old`'s grant, so only the family is checked afterwards.
    let denied = spec(root.path()).identity;
    gate.install(support::projection_gate::passing_evaluator(&denied, 0, &[]));
    assert!(matches!(
        selection.reopen(&corpus.kernel, &gate, &budget(Duration::from_secs(10))),
        Err(BuildError::Denied(_))
    ));
    gate.install(support::projection_gate::passing_evaluator(
        &spec(root.path()).identity,
        0,
        &daemon::projection_gates::ProjectionHook::ALL,
    ));
    assert!(old.projection().quarantine().is_none());
    reopen_reuses_live_family();
}

#[test]
fn sweep_reclaims_unreferenced_families_and_retains_selected_protected_and_leased_ones() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    corpus.publish("base", "bytes");
    let gate = open_gate();
    let selection = build_selected(root.path(), &corpus, &gate);
    let old = selection
        .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
        .unwrap();
    let families = root.path().join("search-families");
    let family = |digest: &str| families.join(digest);
    let old_digest = old.digest().to_owned();

    corpus.publish("late", "late bytes");
    let candidate = next_candidate(root.path(), &corpus, &gate);
    let selected_digest = candidate.staged().digest.clone();
    selection.select(candidate, &mut |_| Ok(())).unwrap();

    corpus.publish("later", "later bytes");
    let candidate = candidate_for(root.path(), &corpus, &gate, "third");
    let partial_digest = candidate.staged().digest.clone();
    let failure = selection
        .select(candidate, &mut |event| {
            if event == SelectionEvent::Copied {
                return Err(GenerationError::NativePayloadInvalid {
                    detail: "lost copy reply",
                });
            }
            Ok(())
        })
        .unwrap_err();
    assert!(family(&partial_digest).join("bootstrap.json").is_file());
    drop(failure);

    // The partial family is protected by the live intent; the old one is leased by `old`.
    let report = selection.sweep().unwrap();
    assert_eq!((report.removed, report.retained), (0, 1));
    for digest in [&old_digest, &selected_digest, &partial_digest] {
        assert!(family(digest).is_dir(), "{digest}");
    }

    std::fs::rename(
        root.path().join("search-lifecycle/intent.json"),
        root.path().join("search-lifecycle/abandoned-intent.json"),
    )
    .unwrap();
    let report = selection.sweep().unwrap();
    assert_eq!((report.removed, report.retained), (1, 1));
    assert!(!family(&partial_digest).exists());
    assert!(family(&old_digest).is_dir());
    assert!(family(&selected_digest).is_dir());
    assert!(
        old.read(&budget(Duration::from_secs(10)), |_| Ok(()))
            .is_ok()
    );

    drop(old);
    let report = selection.sweep().unwrap();
    assert_eq!((report.removed, report.retained), (1, 0));
    assert!(!family(&old_digest).exists());
    assert!(family(&selected_digest).is_dir());
    assert_eq!(
        selection
            .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
            .unwrap()
            .digest(),
        selected_digest
    );
    assert_eq!(
        std::fs::read_dir(&families).unwrap().count(),
        1,
        "only the selected family remains"
    );
}

#[test]
fn within_tip_checkpoint_without_source_application_is_not_a_complete_prefix() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    corpus.publish("base", "bytes");
    let gate = open_gate();
    let selection = build_selected(root.path(), &corpus, &gate);
    let old = selection
        .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
        .unwrap();
    corpus.publish("missing", "not applied");
    let tip = corpus.tip();
    old.projection()
        .write(|conn| {
            conn.execute(
                "UPDATE projection_checkpoint SET checkpoint_commit_seq=?1",
                [tip],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        old.projection()
            .read(|conn| Ok(
                conn.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))?
            ))
            .unwrap(),
        "ok"
    );
    drop(old);
    for _ in 0..2 {
        assert!(matches!(
            selection.reopen(&corpus.kernel, &gate, &budget(Duration::from_secs(10))),
            Err(BuildError::Invalid("canonical prefix inventory mismatch"))
        ));
        assert!(
            selection
                .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                .is_err()
        );
    }
}

#[test]
fn copy_cancellation_is_observed_before_selection() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    corpus.publish("base", "bytes");
    let gate = open_gate();
    let config = spec(root.path());
    record(root.path(), &gate, None, &config.identity);
    let work = budget(Duration::from_secs(30));
    let candidate = ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, config)
        .unwrap()
        .build(&work, &mut |_| {})
        .unwrap();
    let selection = selector(root.path());
    let mut fired = false;
    let failure = selection
        .select(candidate, &mut |event| {
            if event == SelectionEvent::Copied {
                fired = true;
                work.cancel();
            }
            Ok(())
        })
        .unwrap_err();
    assert!(fired);
    assert!(matches!(failure.error, BuildError::Expired));
    assert_eq!(
        GenerationStore::open(Some(root.path()))
            .unwrap()
            .read_search_current()
            .unwrap(),
        CurrentProfile::Absent
    );
}

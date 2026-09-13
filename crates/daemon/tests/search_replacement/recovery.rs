use super::*;
use daemon::projection_gates::ProjectionHook;
use daemon::search_replacement::selection::recovery::{RecoveryEvent, RecoveryProgress};

fn home_gate(root: &Path, config: &ReplacementSpec) -> HookGate {
    let gate = HookGate::for_home(root);
    gate.install(support::projection_gate::passing_evaluator(
        &config.identity,
        0,
        &ProjectionHook::ALL,
    ));
    gate
}

fn finish(
    selection: &mut SearchSelection,
    corpus: &Corpus,
    gate: &HookGate,
    config: &ReplacementSpec,
) {
    let budget = budget(Duration::from_secs(20));
    for _ in 0..2 {
        if selection
            .recover_slice(&corpus.kernel, gate, config, &budget, &mut |_| {})
            .unwrap()
            == RecoveryProgress::Current
        {
            return;
        }
    }
    panic!("scheduled slices did not reach Current");
}

fn current(root: &Path) -> daemon::projection_lifecycle::LifecycleIntent {
    match ProjectionLifecycle::open(root).unwrap().read() {
        ControlState::Current(intent) => intent,
        state => panic!("not Current: {state:?}"),
    }
}

fn consumer_progress(root: &Path, consumer: &str) -> (i64, i64) {
    Connection::open(root.join("kernel/kernel.sqlite"))
        .unwrap()
        .query_row(
            "SELECT checkpoint_commit_seq,updated_at FROM outbox_consumers WHERE consumer_id=?1",
            [consumer],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
}

fn select_next(
    root: &Path,
    corpus: &Corpus,
    gate: &HookGate,
    selection: &mut SearchSelection,
    config: &mut ReplacementSpec,
) -> daemon::projection_lifecycle::LifecycleIntent {
    let prior = current(root);
    let mut next = request(None, &config.identity);
    next.selected_generation = prior.staged_seed_digest.unwrap();
    next.attempt_id = "next-operation".to_owned();
    next.consumer.consumer_id = "next-consumer".to_owned();
    next.consumer.generation_id = "next-generation".to_owned();
    config.generation.generation_id = next.consumer.generation_id.clone();
    ProjectionLifecycle::open(root)
        .unwrap()
        .record(gate, &next, now())
        .unwrap();
    assert_eq!(
        selection
            .recover_slice(
                &corpus.kernel,
                gate,
                config,
                &budget(Duration::from_secs(20)),
                &mut |_| {}
            )
            .unwrap(),
        RecoveryProgress::Selected
    );
    control(root)
}

#[test]
fn completed_observation_accepts_historical_deadline_without_writes_or_old_target_ack() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    corpus.publish("base", "unchanged source");
    let config = spec(root.path());
    let gate = home_gate(root.path(), &config);
    let request = request(None, &config.identity);
    ProjectionLifecycle::open(root.path())
        .unwrap()
        .record(&gate, &request, now())
        .unwrap();
    let writes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = writes.clone();
    let mut selection = selector(root.path()).with_recovery_write_barrier_for_test(move |_| {
        count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    });
    let operation = budget(Duration::from_secs(20));
    for expected in [RecoveryProgress::Selected, RecoveryProgress::Current] {
        assert_eq!(
            selection
                .recover_slice(&corpus.kernel, &gate, &config, &operation, &mut |_| {})
                .unwrap(),
            expected
        );
    }
    let completed = current(root.path());
    drop(selection);
    let shift = completed.episodes.deadline - now() + 60_000;
    let control_path = root.path().join("search-lifecycle/intent.json");
    let certificate_path = root
        .path()
        .join("search-families")
        .join(completed.staged_seed_digest.as_ref().unwrap())
        .join("bootstrap.json");
    // The real completed family supplies every non-time field of this historical fixture.
    for (path, field) in [(&control_path, "current"), (&certificate_path, "intent")] {
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        value[field]["episodes"]["deadline"] =
            serde_json::json!(completed.episodes.deadline - shift);
        value[field]["recorded_at"] = serde_json::json!(completed.recorded_at - shift);
        std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }
    let mut expected = completed;
    expected.episodes.deadline -= shift;
    expected.recorded_at -= shift;
    let done = current(root.path());
    assert_eq!(done, expected);
    assert_eq!(done.episodes.consumed, 2);
    let bytes = std::fs::read(&control_path).unwrap();
    let written = writes.load(std::sync::atomic::Ordering::Relaxed);
    assert!(now() > done.episodes.deadline);
    let count = writes.clone();
    let mut selection = selector(root.path()).with_recovery_write_barrier_for_test(move |_| {
        count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    });
    for extend in [false, true] {
        let observation = budget(Duration::from_secs(10));
        if extend {
            let reader = selection.pin(&corpus.kernel, &gate, &observation).unwrap();
            corpus
                .kernel
                .commit(intent("legitimate-empty-extension"), |_| Ok(String::new()))
                .unwrap();
            disable::apply_prefix(&reader, corpus.tip(), Vec::new());
            corpus
                .kernel
                .acknowledge_outbox(CONSUMER, corpus.tip(), now())
                .unwrap();
            assert!(corpus.tip() > done.recovery_target.unwrap().commit_seq);
        }
        let ack = consumer_progress(root.path(), CONSUMER);
        let mut events = Vec::new();
        assert_eq!(
            selection
                .recover_slice(&corpus.kernel, &gate, &config, &observation, &mut |event| {
                    events.push(event)
                })
                .unwrap(),
            RecoveryProgress::Current
        );
        assert_eq!(events, vec![RecoveryEvent::Current]);
        assert_eq!(current(root.path()), done);
        assert_eq!(std::fs::read(&control_path).unwrap(), bytes);
        assert_eq!(writes.load(std::sync::atomic::Ordering::Relaxed), written);
        assert_eq!(consumer_progress(root.path(), CONSUMER), ack);
        assert_eq!(
            selection
                .pin(&corpus.kernel, &gate, &observation)
                .unwrap()
                .coverage(&observation)
                .unwrap()
                .checkpoint
                .checkpoint_commit_seq,
            ack.0
        );
    }
    let expired = budget(Duration::ZERO);
    assert!(
        selection
            .recover_slice(&corpus.kernel, &gate, &config, &expired, &mut |_| {})
            .is_err()
    );
    let mut stale =
        support::projection_gate::passing_evaluator(&config.identity, 0, &ProjectionHook::ALL);
    stale.evidence.identity.embedding_model = "stale".to_owned();
    gate.install(stale);
    assert!(
        selection
            .recover_slice(
                &corpus.kernel,
                &gate,
                &config,
                &budget(Duration::from_secs(10)),
                &mut |_| {}
            )
            .is_err()
    );
    gate.install(support::projection_gate::passing_evaluator(
        &config.identity,
        0,
        &ProjectionHook::ALL,
    ));
    let profile = GenerationStore::open(Some(root.path()))
        .unwrap()
        .root()
        .join(host_runtime::generation::SEARCH_PROFILE_NAME);
    let mut wrong: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&profile).unwrap()).unwrap();
    wrong["current"] = serde_json::json!("0".repeat(64));
    std::fs::write(&profile, serde_json::to_vec(&wrong).unwrap()).unwrap();
    assert!(
        selection
            .recover_slice(
                &corpus.kernel,
                &gate,
                &config,
                &budget(Duration::from_secs(10)),
                &mut |_| {}
            )
            .is_err()
    );
    assert_eq!(std::fs::read(&control_path).unwrap(), bytes);
    assert_eq!(writes.load(std::sync::atomic::Ordering::Relaxed), written);
}

#[test]
fn raced_operation_cannot_debit_the_replacement_record() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    corpus.publish("base", "bytes");
    let config = spec(root.path());
    let gate = home_gate(root.path(), &config);
    record(root.path(), &gate, None, &config.identity);
    let mut selection = selector(root.path());
    assert_eq!(
        selection
            .recover_slice(
                &corpus.kernel,
                &gate,
                &config,
                &budget(Duration::from_secs(20)),
                &mut |_| {}
            )
            .unwrap(),
        RecoveryProgress::Selected
    );
    let mut replacement = control(root.path());
    replacement.attempt_id = "intervening-operation".to_owned();
    replacement.episodes.consumed = 0;
    let ack = consumer_progress(root.path(), CONSUMER);
    let mut reached = false;
    let result = selection.recover_slice(
        &corpus.kernel,
        &gate,
        &config,
        &budget(Duration::from_secs(20)),
        &mut |event| {
            if event == RecoveryEvent::BeforeEpisode {
                reached = true;
                std::fs::write(
                    root.path().join("search-lifecycle/intent.json"),
                    serde_json::to_vec(&replacement).unwrap(),
                )
                .unwrap();
            }
        },
    );
    assert!(reached);
    assert!(matches!(
        result,
        Err(
            daemon::search_replacement::selection::recovery::RecoveryFailure::Blocked(
                BuildError::Intent(daemon::projection_lifecycle::IntentRefusal::Conflict { .. })
            )
        )
    ));
    assert_eq!(control(root.path()), replacement);
    assert_eq!(consumer_progress(root.path(), CONSUMER), ack);
    assert!(database(root.path()).is_file());
}

#[test]
fn completion_decoder_rejects_ambiguous_schema_and_invalid_accounting() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    let config = spec(root.path());
    let gate = home_gate(root.path(), &config);
    record(root.path(), &gate, None, &config.identity);
    let mut selection = selector(root.path());
    finish(&mut selection, &corpus, &gate, &config);
    let lifecycle = ProjectionLifecycle::open(root.path()).unwrap();
    let path = root.path().join("search-lifecycle/intent.json");
    let bytes = std::fs::read(&path).unwrap();
    let valid: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    for damage in [
        "active",
        "disabled",
        "unknown-schema",
        "mixed",
        "counter",
        "target",
        "time",
    ] {
        let mut invalid = valid.clone();
        match damage {
            "active" => invalid["schema"] = serde_json::json!(2),
            "disabled" => invalid["schema"] = serde_json::json!(3),
            "unknown-schema" => invalid["schema"] = serde_json::json!(999),
            "mixed" => invalid["handoff"] = invalid["current"].clone(),
            "counter" => invalid["current"]["episodes"]["consumed"] = serde_json::json!(4),
            "target" => invalid["current"]["recovery_target"]["commit_seq"] = serde_json::json!(-1),
            "time" => invalid["current"]["recorded_at"] = serde_json::json!(-1),
            _ => unreachable!(),
        }
        std::fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
        assert!(
            matches!(lifecycle.read(), ControlState::Unavailable(_)),
            "{damage}"
        );
    }
    std::fs::write(path, bytes).unwrap();
    assert!(matches!(lifecycle.read(), ControlState::Current(_)));
}

#[tokio::test]
async fn immutable_binding_and_local_prefix_refuse_before_retiring_old_bytes() {
    for surface in ["recovery", "disable"] {
        for damage in ["digest", "target", "local-prefix"] {
            if surface == "disable" && damage == "local-prefix" {
                continue;
            }
            let root = tempfile::tempdir().unwrap();
            let corpus = Corpus::open(root.path());
            corpus.seed();
            corpus.publish("base", "old bytes");
            let mut config = spec(root.path());
            let gate = home_gate(root.path(), &config);
            record(root.path(), &gate, None, &config.identity);
            let mut selection = selector(root.path());
            finish(&mut selection, &corpus, &gate, &config);
            let old_digest = current(root.path()).staged_seed_digest.unwrap();
            let expected = select_next(root.path(), &corpus, &gate, &mut selection, &mut config);
            let path = root.path().join("search-lifecycle/intent.json");
            let selected_path = selection
                .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                .unwrap()
                .projection()
                .path()
                .to_owned();
            let old_ack = consumer_progress(root.path(), CONSUMER);
            let new_ack = consumer_progress(root.path(), "next-consumer");
            if surface == "disable" {
                gate.install(disable::cleanup_evaluator(root.path()));
                selection.begin_disable(&gate, &mut |_| {}).unwrap();
            }
            if damage != "local-prefix" {
                let mut value: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                let operation = if surface == "disable" {
                    &mut value["handoff"]
                } else {
                    &mut value
                };
                if damage == "digest" {
                    operation["staged_seed_digest"] = serde_json::json!("0".repeat(64));
                } else {
                    operation["recovery_target"]["commit_seq"] =
                        serde_json::json!(expected.recovery_target.unwrap().commit_seq + 1);
                }
                std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            }
            if surface == "disable" {
                assert!(
                    selection
                        .reconcile_disabled(
                            &corpus.kernel,
                            &gate,
                            &config,
                            &budget(Duration::from_secs(20)),
                            &mut |_| {}
                        )
                        .await
                        .is_err()
                );
            } else {
                let mut charged = false;
                let mut retired = false;
                assert!(selection.recover_slice(&corpus.kernel, &gate, &config, &budget(Duration::from_secs(20)), &mut |event| {
                    if event == RecoveryEvent::BeforeEpisode && damage == "local-prefix" {
                        charged = true;
                        Connection::open(&selected_path).unwrap().execute("UPDATE projection_checkpoint SET checkpoint_commit_seq=checkpoint_commit_seq+1", []).unwrap();
                    }
                    retired |= matches!(event, RecoveryEvent::Retirement(_));
                }).is_err());
                assert!(!retired);
                assert_eq!(charged, damage == "local-prefix");
                assert_eq!(
                    control(root.path()).episodes.consumed,
                    if charged { 2 } else { 1 }
                );
            }
            assert_eq!(consumer_progress(root.path(), CONSUMER), old_ack);
            assert_eq!(consumer_progress(root.path(), "next-consumer"), new_ack);
            assert!(
                root.path()
                    .join("search-families")
                    .join(&old_digest)
                    .join("search/search.sqlite")
                    .is_file()
            );
            GenerationStore::open(Some(root.path()))
                .unwrap()
                .validate(&old_digest)
                .unwrap();
            assert!(database(root.path()).is_file());
        }
    }
}

#[test]
fn cleanup_revocation_and_changed_hold_preserve_owned_files_and_capture() {
    for fault in ["revocation", "hold"] {
        let root = tempfile::tempdir().unwrap();
        let corpus = Corpus::open(root.path());
        corpus.seed();
        corpus.publish("base", "bytes");
        let config = spec(root.path());
        let gate = Arc::new(home_gate(root.path(), &config));
        record(root.path(), &gate, None, &config.identity);
        let mut selection = selector(root.path());
        assert_eq!(
            selection
                .recover_slice(
                    &corpus.kernel,
                    &gate,
                    &config,
                    &budget(Duration::from_secs(20)),
                    &mut |_| {}
                )
                .unwrap(),
            RecoveryProgress::Selected
        );
        let expected = control(root.path());
        let digest = expected.staged_seed_digest.clone().unwrap();
        let hold = expected
            .replacement_capture
            .as_ref()
            .unwrap()
            .hold_id
            .clone();
        let ack = consumer_progress(root.path(), CONSUMER);
        let saved = Arc::new(std::sync::Mutex::new(None));
        let observed = saved.clone();
        let home = root.path().to_owned();
        let revoke = gate.clone();
        selection = selection.with_recovery_write_barrier_for_test(move |event| {
            if event != daemon::projection_lifecycle::WriteBarrier::BeforeFamilyRemoval {
                return;
            }
            let mut saved = observed.lock().unwrap();
            if saved.is_some() {
                return;
            }
            let path = home.join("search-lifecycle/intent.json");
            let bytes = std::fs::read(&path).unwrap();
            *saved = Some(bytes.clone());
            if fault == "revocation" {
                revoke.close();
            } else {
                let mut record: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                record["replacement_capture"]["hold_id"] = serde_json::json!("b".repeat(32));
                std::fs::write(path, serde_json::to_vec(&record).unwrap()).unwrap();
            }
        });
        assert!(
            selection
                .recover_slice(
                    &corpus.kernel,
                    &gate,
                    &config,
                    &budget(Duration::from_secs(20)),
                    &mut |_| {}
                )
                .is_err()
        );
        let saved = saved
            .lock()
            .unwrap()
            .clone()
            .expect("cleanup boundary fired");
        let charged: daemon::projection_lifecycle::LifecycleIntent =
            serde_json::from_slice(&saved).unwrap();
        assert_eq!(charged.episodes.consumed, 2);
        assert!(database(root.path()).is_file());
        GenerationStore::open(Some(root.path()))
            .unwrap()
            .validate(&digest)
            .unwrap();
        let released: bool = Connection::open(root.path().join("kernel/kernel.sqlite"))
            .unwrap()
            .query_row(
                "SELECT released_at IS NOT NULL FROM capture_pins WHERE capture_pin_id=?1",
                [&hold],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!released);
        assert_eq!(consumer_progress(root.path(), CONSUMER).0, ack.0);
        std::fs::write(root.path().join("search-lifecycle/intent.json"), saved).unwrap();
        gate.install(support::projection_gate::passing_evaluator(
            &config.identity,
            0,
            &ProjectionHook::ALL,
        ));
        finish(&mut selection, &corpus, &gate, &config);
        assert_eq!(current(root.path()).episodes.consumed, 3);
        assert_hold_released(root.path(), &hold);
        assert!(!database(root.path()).exists());
    }
}

fn source_at(root: &Path) -> source_fixture::Fixture {
    let kernel_root = tempfile::Builder::new()
        .prefix("kernel")
        .rand_bytes(0)
        .tempdir_in(root)
        .unwrap();
    let corpus = Corpus::open(root);
    corpus.seed();
    source_fixture::Fixture {
        root: kernel_root,
        store: corpus.kernel,
        ledger: BTreeMap::new(),
    }
}

fn populate(source: &mut source_fixture::Fixture) {
    for class in CLASSES {
        source.publish(class, class, 1, &format!("exact {class}\r\nλ"));
    }
}

#[test]
fn deletion_after_confirmed_pruning_rebuilds_all_search_state() {
    let root = tempfile::tempdir().unwrap();
    let mut source = source_at(root.path());
    populate(&mut source);
    let corpus = Corpus {
        kernel: source.store.clone(),
    };
    let config = spec(root.path());
    let gate = home_gate(root.path(), &config);
    record(root.path(), &gate, None, &config.identity);
    let mut selection = selector(root.path());
    finish(&mut selection, &corpus, &gate, &config);
    corpus
        .kernel
        .acknowledge_outbox(fixtures::CONSUMER, corpus.tip(), now())
        .unwrap();
    source.publish_and_prune();
    assert_eq!(source.count("SELECT count(*) FROM outbox"), 0);
    let expected = source_rows(&source);
    let old = current(root.path());
    let old_hold = selection
        .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
        .unwrap()
        .coverage(&budget(Duration::from_secs(10)))
        .unwrap()
        .checkpoint
        .hold_id;
    drop(selection);
    for entry in std::fs::read_dir(root.path()).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == "kernel" {
            continue;
        }
        if entry.file_type().unwrap().is_dir() {
            std::fs::remove_dir_all(entry.path()).unwrap();
        } else {
            std::fs::remove_file(entry.path()).unwrap();
        }
    }
    assert!(matches!(
        ProjectionLifecycle::open(root.path()).unwrap().read(),
        ControlState::Absent
    ));
    record(root.path(), &gate, None, &config.identity);
    let mut selection = selector(root.path());
    corpus
        .kernel
        .commit(intent("empty-after-prune"), |_| Ok(String::new()))
        .unwrap();
    let mut captured = None;
    assert_eq!(
        selection
            .recover_slice(
                &corpus.kernel,
                &gate,
                &config,
                &budget(Duration::from_secs(20)),
                &mut |event| {
                    if let RecoveryEvent::Build(BuildEvent::Captured { snapshot, hold_id }) = event
                    {
                        captured = Some((snapshot, hold_id));
                    }
                }
            )
            .unwrap(),
        RecoveryProgress::Selected
    );
    let (snapshot, hold) = captured.expect("fresh capture is required after deleting search state");
    assert_ne!(hold, old_hold);
    assert!(snapshot > old.recovery_target.unwrap().commit_seq);
    finish(&mut selection, &corpus, &gate, &config);
    assert_ne!(
        current(root.path()).staged_seed_digest,
        old.staged_seed_digest
    );
    assert_eq!(
        current(root.path()).recovery_target.unwrap().commit_seq,
        snapshot
    );
    let reader = selection
        .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
        .unwrap();
    assert_eq!(observe(&reader).rows, expected);
    assert_eq!(observe(&reader).pending, expected_pending(&expected));
    assert_eq!(
        corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
        current(root.path()).recovery_target.map(|t| t.commit_seq)
    );
}

#[test]
fn every_identity_mismatch_and_each_class_corruption_rebuilds_to_current() {
    let cases = [
        ("schema_version", Cause::SchemaMismatch),
        ("tokenizer_fingerprint", Cause::TokenizerMismatch),
        ("embedding_model", Cause::EmbeddingModelMismatch),
        ("projection_policy_version", Cause::ProjectionPolicyMismatch),
        ("identity_contract_version", Cause::IdentityContractMismatch),
    ]
    .into_iter()
    .chain(CLASSES.map(|class| (class, Cause::Corruption)));
    for (damage, cause) in cases {
        let root = tempfile::tempdir().unwrap();
        let mut source = source_at(root.path());
        populate(&mut source);
        let corpus = Corpus {
            kernel: source.store.clone(),
        };
        let mut config = spec(root.path());
        let gate = home_gate(root.path(), &config);
        record(root.path(), &gate, None, &config.identity);
        let mut selection = selector(root.path());
        finish(&mut selection, &corpus, &gate, &config);
        let old = current(root.path());
        let path = selection
            .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
            .unwrap()
            .projection()
            .path()
            .to_owned();
        drop(selection);
        let raw = Connection::open(path).unwrap();
        if cause == Cause::Corruption {
            assert_eq!(raw.execute("UPDATE payloads SET bytes=zeroblob(length(bytes)) WHERE payload_id IN (SELECT payload_id FROM occurrences WHERE class=?1)", [damage]).unwrap(), 1);
        } else {
            let value = if damage == "schema_version" {
                "3"
            } else {
                "'wrong-identity'"
            };
            assert_eq!(
                raw.execute(
                    &format!("UPDATE projection_identity SET {damage}={value}"),
                    []
                )
                .unwrap(),
                1
            );
        }
        drop(raw);
        let mut selection = selector(root.path());
        assert!(
            selection
                .reopen(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                .is_err(),
            "{damage}"
        );
        config.generation.generation_id = "repaired-generation".to_owned();
        let mut next = request(None, &config.identity);
        next.selected_generation = old.staged_seed_digest.unwrap();
        next.consumer.consumer_id = "repaired-consumer".to_owned();
        next.consumer.generation_id = config.generation.generation_id.clone();
        next.attempt_id = "repair-attempt".to_owned();
        next.cause = cause;
        ProjectionLifecycle::open(root.path())
            .unwrap()
            .record(&gate, &next, now())
            .unwrap();
        finish(&mut selection, &corpus, &gate, &config);
        let reader = selection
            .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
            .unwrap();
        let expected = source_rows(&source);
        assert_eq!(observe(&reader).rows, expected, "{damage}");
        assert_eq!(observe(&reader).pending, expected_pending(&expected));
        assert_eq!(
            corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
            None
        );
        assert_eq!(current(root.path()).cause, cause);
    }
}

fn park_recovery_child(
    root: &Path,
    cut: &str,
    rows: &Rows,
    started: &daemon::projection_lifecycle::LifecycleIntent,
) -> ! {
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("search-lifecycle/intent.json")).unwrap())
            .unwrap();
    let saved = value.get("current").unwrap_or(&value);
    println!(
        "WITNESS {}",
        serde_json::json!({"rows":rows, "intent":saved, "started":started})
    );
    println!("BARRIER {cut}");
    std::io::stdout().flush().unwrap();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap();
    panic!("parent did not kill child");
}

pub(crate) fn recovery_child(root: &Path, cut: &str) {
    let mut source = source_at(root);
    populate(&mut source);
    let corpus = Corpus {
        kernel: source.store.clone(),
    };
    let config = spec(root);
    let gate = home_gate(root, &config);
    let mut selection = selector(root);
    selection.begin_disable(&gate, &mut |_| {}).unwrap();
    let mut request = request(None, &config.identity);
    request.transition = Transition::AuthorizedRecovery;
    request.cause = Cause::DisabledRecovery;
    request.authorization_ref = Some("operator:child-recovery".to_owned());
    selection
        .begin_authorized_recovery(
            &corpus.kernel,
            &gate,
            &request,
            &budget(Duration::from_secs(20)),
        )
        .unwrap();
    let started = control(root);
    let writing_current = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let armed = writing_current.clone();
    let child_root = root.to_owned();
    let child_cut = cut.to_owned();
    let expected = source_rows(&source);
    let original = started.clone();
    selection = selection.with_recovery_write_barrier_for_test(move |event| {
        use daemon::projection_lifecycle::WriteBarrier;
        if !armed.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        let name = match event {
            WriteBarrier::BeforeRename => "recovery-current-before-rename",
            WriteBarrier::AfterRename => "recovery-current-after-rename",
            WriteBarrier::AfterDirectorySync => "recovery-current-after-sync",
            _ => return,
        };
        if name == child_cut {
            park_recovery_child(&child_root, &child_cut, &expected, &original);
        }
    });
    let grant = budget(Duration::from_secs(20));
    let mut observer = |event: RecoveryEvent| {
        if event == RecoveryEvent::BeforeCurrent {
            writing_current.store(true, std::sync::atomic::Ordering::Release);
        }
        let name = match event {
            RecoveryEvent::Build(BuildEvent::Pinned) => "recovery-pinned",
            RecoveryEvent::Selection(SelectionEvent::MetadataWritten) => "recovery-partial",
            RecoveryEvent::BeforeFinalAcknowledgement => "recovery-before-ack",
            RecoveryEvent::FinalAcknowledged => "recovery-after-ack",
            RecoveryEvent::BeforeCurrent => "recovery-before-current",
            RecoveryEvent::Current => "recovery-after-current",
            _ => return,
        };
        if name == cut {
            park_recovery_child(root, cut, &source_rows(&source), &started);
        }
    };
    for _ in 0..2 {
        selection
            .recover_slice(&corpus.kernel, &gate, &config, &grant, &mut observer)
            .unwrap();
    }
    panic!("cut not reached");
}

#[test]
fn real_process_cuts_reconcile_twice_without_renewing_authority_target_or_allowance() {
    for cut in [
        "recovery-pinned",
        "recovery-partial",
        "recovery-before-ack",
        "recovery-after-ack",
        "recovery-before-current",
        "recovery-after-current",
        "recovery-current-before-rename",
        "recovery-current-after-rename",
        "recovery-current-after-sync",
    ] {
        let root = tempfile::tempdir().unwrap();
        let observations = kill_child_at(root.path(), cut);
        let witness = &observations["WITNESS"];
        let before: daemon::projection_lifecycle::LifecycleIntent =
            serde_json::from_value(witness["intent"].clone()).unwrap();
        let expected: Rows = serde_json::from_value(witness["rows"].clone()).unwrap();
        let was_current = matches!(
            cut,
            "recovery-after-current"
                | "recovery-current-after-rename"
                | "recovery-current-after-sync"
        );
        let was_selected = !matches!(cut, "recovery-pinned" | "recovery-partial");
        assert_eq!(before.episodes.consumed, if was_selected { 2 } else { 1 });
        let pre_state = ProjectionLifecycle::open(root.path()).unwrap().read();
        assert_eq!(
            pre_state,
            if was_current {
                ControlState::Current(before.clone())
            } else {
                ControlState::Intent(before.clone())
            }
        );
        let profile = GenerationStore::open(Some(root.path()))
            .unwrap()
            .read_search_current()
            .unwrap();
        let path = match profile {
            CurrentProfile::Current(digest) => root
                .path()
                .join("search-families")
                .join(digest)
                .join("search/search.sqlite"),
            CurrentProfile::Absent => database(root.path()),
            state => panic!("{state:?}"),
        };
        let local: i64 = Connection::open(path)
            .unwrap()
            .query_row(
                "SELECT checkpoint_commit_seq FROM projection_checkpoint",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let ack: i64 = Connection::open(root.path().join("kernel/kernel.sqlite"))
            .unwrap()
            .query_row(
                "SELECT checkpoint_commit_seq FROM outbox_consumers WHERE consumer_id=?1",
                [CONSUMER],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            (local, ack),
            (
                before.recovery_target.unwrap().commit_seq,
                before.recovery_target.unwrap().commit_seq
            )
        );
        for reopen in 0..2 {
            let corpus = Corpus::open(root.path());
            let config = spec(root.path());
            let gate = home_gate(root.path(), &config);
            let mut selection = selector(root.path());
            selection
                .reopen(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                .unwrap();
            assert_eq!(
                selection
                    .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                    .is_ok(),
                was_selected || reopen == 1
            );
            let writes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let count = writes.clone();
            selection = selection.with_recovery_write_barrier_for_test(move |_| {
                count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            });
            let record_bytes =
                std::fs::read(root.path().join("search-lifecycle/intent.json")).unwrap();
            let ack_before = consumer_progress(root.path(), CONSUMER);
            finish(&mut selection, &corpus, &gate, &config);
            if was_current || reopen == 1 {
                assert_eq!(writes.load(std::sync::atomic::Ordering::Relaxed), 0);
                assert_eq!(
                    std::fs::read(root.path().join("search-lifecycle/intent.json")).unwrap(),
                    record_bytes
                );
                assert_eq!(consumer_progress(root.path(), CONSUMER), ack_before);
            }
            let done = current(root.path());
            assert_eq!(done.recovery_target, before.recovery_target, "{cut}");
            assert_eq!(done.authorization_ref, before.authorization_ref);
            assert_eq!(done.attempt_id, before.attempt_id);
            assert_eq!(done.recorded_at, before.recorded_at);
            assert_eq!(done.episodes.deadline, before.episodes.deadline);
            assert_eq!(done.episodes.allowance, before.episodes.allowance);
            assert_eq!(
                done.episodes.consumed,
                if was_current { 2 } else { 3 },
                "{cut}"
            );
            let reader = selection
                .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                .unwrap();
            assert_eq!(observe(&reader).rows, expected);
            assert_eq!(observe(&reader).pending, expected_pending(&expected));
            assert_eq!(
                corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
                done.recovery_target.map(|t| t.commit_seq)
            );
            assert!(done.replacement_capture.is_none());
            assert!(!database(root.path()).exists());
        }
    }
}

#[test]
fn interrupted_export_and_same_lineage_restore_complete_from_fresh_authority() {
    for cut in [
        "before-commit",
        "after-commit",
        "before-ack",
        "after-ack",
        "before-stage",
        "after-stage",
        "down-restore",
    ] {
        let root = tempfile::tempdir().unwrap();
        let observations = kill_child_at(root.path(), cut);
        let original = control(root.path());
        let target = original.recovery_target.unwrap().commit_seq;
        let mut expected: Rows = serde_json::from_value(
            observations["WITNESS"][if cut == "down-restore" {
                "before"
            } else {
                "after"
            }]
            .clone(),
        )
        .unwrap();
        if cut == "down-restore" {
            let corpus = Corpus::open(root.path());
            let backup: std::path::PathBuf =
                serde_json::from_value(observations["BACKUP"].clone()).unwrap();
            corpus.kernel.restore(backup).unwrap();
            let object = corpus.publish("restored-only", "fresh restored bytes");
            expected.insert(
                object,
                (
                    "messages".to_owned(),
                    b"fresh restored bytes".to_vec(),
                    None,
                ),
            );
            assert!(corpus.tip() <= target);
            for n in corpus.tip()..target {
                corpus
                    .kernel
                    .commit(intent(&format!("restore-pad-{n}")), |_| Ok(String::new()))
                    .unwrap();
            }
            assert_eq!(corpus.tip(), target);
        }
        expected.retain(|_, (_, _, dead)| dead.is_none());
        for _ in 0..2 {
            let corpus = Corpus::open(root.path());
            let config = spec(root.path());
            let gate = home_gate(root.path(), &config);
            let mut selection = selector(root.path());
            finish(&mut selection, &corpus, &gate, &config);
            let reader = selection
                .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                .unwrap();
            assert_eq!(observe(&reader).rows, expected, "{cut}");
            assert_eq!(observe(&reader).pending, expected_pending(&expected));
            let done = current(root.path());
            assert_eq!(done.episodes.allowance, original.episodes.allowance);
            assert_eq!(done.episodes.deadline, original.episodes.deadline);
            assert_eq!(done.episodes.consumed, 3);
            assert_eq!(done.recovery_target, original.recovery_target);
            assert_eq!(
                reader
                    .coverage(&budget(Duration::from_secs(10)))
                    .unwrap()
                    .checkpoint
                    .snapshot_commit_seq,
                target
            );
        }
    }
}

#[test]
fn advancing_tip_blocks_retirement_before_consuming_a_completion_slice() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    corpus.publish("base", "bytes");
    let mut config = spec(root.path());
    let gate = home_gate(root.path(), &config);
    record(root.path(), &gate, None, &config.identity);
    let mut selection = selector(root.path());
    finish(&mut selection, &corpus, &gate, &config);
    let fixed = select_next(root.path(), &corpus, &gate, &mut selection, &mut config);
    corpus.publish("moving-tip", "must not be silently acknowledged");
    let result = selection.recover_slice(
        &corpus.kernel,
        &gate,
        &config,
        &budget(Duration::from_secs(20)),
        &mut |_| {},
    );
    assert!(matches!(
        result,
        Err(
            daemon::search_replacement::selection::recovery::RecoveryFailure::Blocked(
                BuildError::Kernel(kernel::KernelError::ConsumerPending)
            )
        )
    ));
    assert_eq!(control(root.path()), fixed);
    assert!(
        corpus
            .kernel
            .outbox_consumer_checkpoint(CONSUMER)
            .unwrap()
            .is_some()
    );
}

#[test]
fn positive_lag_five_class_ledger_requires_the_next_scheduled_slice() {
    let root = tempfile::tempdir().unwrap();
    let mut source = source_fixture::Fixture::open();
    let corpus = Corpus {
        kernel: source.store.clone(),
    };
    let incarnation = corpus
        .kernel
        .database_incarnation_id_within_budget(&budget(Duration::from_secs(10)))
        .unwrap();
    let config = spec_with_identity(identity(&incarnation));
    for class in CLASSES {
        let text = format!("  {class}\r\n\tλ\0exact ");
        if class == "raw_tool_spans" {
            let evidence = source.retain("raw", &text);
            source.publish_span(
                class,
                class,
                1,
                &text,
                Some((2, text.len() as u64 - 1)),
                evidence,
            );
        } else {
            source.publish(class, class, 1, &text);
        }
    }
    let gate = home_gate(root.path(), &config);
    record(root.path(), &gate, None, &config.identity);
    let original = control(root.path());
    let mut selection =
        SearchSelection::new(root.path(), config.identity.clone(), coverage_bounds());
    let mut snapshot = None;
    let mut target = None;
    let mut acknowledgements = Vec::new();
    let grant = budget(Duration::from_secs(20));
    assert_eq!(
        selection
            .recover_slice(&corpus.kernel, &gate, &config, &grant, &mut |event| {
                match event {
                    RecoveryEvent::Build(BuildEvent::Captured { snapshot: s, .. }) => {
                        snapshot = Some(s);
                        let object = source
                            .live_entry("canonical_claims", "canonical_claims")
                            .object_id
                            .clone();
                        source.retire(&object);
                        source.publish("messages", "messages", 2, "revised message");
                        for class in CLASSES {
                            source.publish(
                                class,
                                &format!("late-{class}"),
                                1,
                                &format!("late {class}\r\n"),
                            );
                        }
                    }
                    RecoveryEvent::Build(BuildEvent::TargetFixed(t)) => target = Some(t),
                    RecoveryEvent::Build(BuildEvent::CatchUp(EpisodeEvent::Acknowledged {
                        through,
                    })) => acknowledgements.push(through),
                    _ => {}
                }
            })
            .unwrap(),
        RecoveryProgress::Selected
    );
    assert!(target.unwrap() > snapshot.unwrap());
    assert_eq!(acknowledgements.last().copied(), target);
    assert!(matches!(
        ProjectionLifecycle::open(root.path()).unwrap().read(),
        ControlState::Intent(_)
    ));
    let withheld = control(root.path());
    selection.reopen(&corpus.kernel, &gate, &grant).unwrap();
    assert!(selection.pin(&corpus.kernel, &gate, &grant).is_ok());
    assert!(!grant.is_exhausted());
    assert_eq!(control(root.path()), withheld);
    assert_eq!(withheld.episodes.consumed, 1);
    assert!(matches!(
        ProjectionLifecycle::open(root.path()).unwrap().read(),
        ControlState::Intent(_)
    ));
    assert_eq!(
        selection
            .recover_slice(&corpus.kernel, &gate, &config, &grant, &mut |_| {})
            .unwrap(),
        RecoveryProgress::Current
    );
    let done = current(root.path());
    assert_eq!(done.recovery_target.unwrap().commit_seq, target.unwrap());
    assert_eq!(done.episodes.consumed, 2);
    assert_eq!(done.episodes.allowance, original.episodes.allowance);
    assert_eq!(done.episodes.deadline, original.episodes.deadline);
    assert!(!database(root.path()).exists());
    assert!(done.replacement_capture.is_none());
    let reader = selection.pin(&corpus.kernel, &gate, &grant).unwrap();
    let expected: Rows = source
        .ledger
        .iter()
        .map(|(object, row)| {
            (
                object.clone(),
                (
                    row.class.clone(),
                    row.selected_text().as_bytes().to_vec(),
                    row.invalidated,
                ),
            )
        })
        .collect();
    assert_eq!(observe(&reader).rows, expected);
    assert_eq!(observe(&reader).pending, expected_pending(&expected));
    assert_eq!(
        corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
        target
    );
    reader.read(&grant, |conn| {
        for row in source.ledger.values() {
            let stored = conn.query_row("SELECT revision,span_start,span_end FROM occurrences WHERE source_object_id=?1", [&row.object_id], |r|
                Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, Option<i64>>(2)?)))?;
            assert_eq!(stored, (row.revision, row.span.map(|s| s.0 as i64), row.span.map(|s| s.1 as i64)));
        }
        assert_eq!(conn.query_row("SELECT count(*) FROM occurrence_vectors", [], |r| r.get::<_, i64>(0))?, 0);
        assert_eq!(conn.query_row("SELECT generation_id FROM vector_generations", [], |r| r.get::<_, String>(0))?, config.generation.generation_id);
        assert_eq!(conn.query_row("SELECT count(*) FROM embedding_jobs WHERE state='pending' AND generation_id<>?1", [&config.generation.generation_id], |r| r.get::<_, i64>(0))?, 0);
        Ok(())
    }).unwrap();
}

#[test]
fn restart_pending_and_pinned_candidate_recover_without_resetting_the_envelope() {
    for pinned in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let corpus = Corpus::open(root.path());
        corpus.seed();
        let object = corpus.publish("base", "bytes stay exact");
        let config = spec(root.path());
        let gate = home_gate(root.path(), &config);
        record(root.path(), &gate, None, &config.identity);
        if pinned {
            let candidate =
                ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, config.clone())
                    .unwrap()
                    .build(&budget(Duration::from_secs(20)), &mut |_| {})
                    .unwrap();
            drop(candidate);
        } else {
            let mut selection = selector(root.path());
            assert_eq!(
                selection
                    .recover_slice(
                        &corpus.kernel,
                        &gate,
                        &config,
                        &budget(Duration::from_secs(20)),
                        &mut |_| {}
                    )
                    .unwrap(),
                RecoveryProgress::Selected
            );
        }
        let before = control(root.path());
        drop(corpus);
        for _ in 0..2 {
            let corpus = Corpus::open(root.path());
            let gate = home_gate(root.path(), &config);
            let mut selection = selector(root.path());
            finish(&mut selection, &corpus, &gate, &config);
            let done = current(root.path());
            assert_eq!(done.recovery_target, before.recovery_target);
            assert_eq!(done.episodes.allowance, before.episodes.allowance);
            assert_eq!(done.episodes.deadline, before.episodes.deadline);
            assert_eq!(done.authorization_ref, before.authorization_ref);
            assert!(done.episodes.consumed <= done.episodes.allowance);
            let reader = selection
                .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
                .unwrap();
            assert_eq!(observe(&reader).pending, vec![object.clone()]);
            assert_eq!(observe(&reader).rows[&object].1, b"bytes stay exact");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn current_keeps_dispatched_vectors_and_remaining_pending_across_restart() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    let first = corpus.publish("first", "first exact input");
    let second = corpus.publish("second", "second exact input");
    let config = spec(root.path());
    let gate = home_gate(root.path(), &config);
    record(root.path(), &gate, None, &config.identity);
    let mut selection = selector(root.path());
    let grant = budget(Duration::from_secs(20));
    assert_eq!(
        selection
            .recover_slice(&corpus.kernel, &gate, &config, &grant, &mut |_| {})
            .unwrap(),
        RecoveryProgress::Selected
    );
    let reader = selection.pin(&corpus.kernel, &gate, &grant).unwrap();
    let engine = fixtures::TestEngine::new();
    let synapse = fixtures::component(&engine, host_runtime::synapse::SynapseLimits::default());
    let mut bounds = fixtures::bounds();
    bounds.max_jobs = NonZeroUsize::MIN;
    bounds.grant = fixtures::grant(3, now() + 20_000);
    let project = kernel::ProjectScope::new(fixtures::PROJECT).unwrap();
    let mut dispatcher = daemon::embedding_dispatch::EmbeddingDispatcher::new(
        &corpus.kernel,
        reader.projection(),
        &synapse,
    );
    let end = dispatcher
        .run_pass(
            fixtures::eligibility(&project),
            &bounds,
            &grant,
            now(),
            &mut |_| {},
        )
        .unwrap();
    assert!(end.is_none(), "{end:?}");
    assert_eq!(engine.calls(), 1);
    drop(dispatcher);
    drop(reader);
    finish(&mut selection, &corpus, &gate, &config);
    drop(selection);
    drop(corpus);
    for _ in 0..2 {
        let corpus = Corpus::open(root.path());
        let gate = home_gate(root.path(), &config);
        let mut selection = selector(root.path());
        finish(&mut selection, &corpus, &gate, &config);
        let reader = selection.pin(&corpus.kernel, &gate, &grant).unwrap();
        let report = reader.coverage(&grant).unwrap();
        let messages = report.class(kernel::source_identity::OccurrenceClass::Messages);
        assert_eq!(
            (
                messages.lexical,
                messages.valid_vectors,
                messages.missing,
                messages.pending
            ),
            (2, 1, 1, 1)
        );
        reader.read(&grant, |conn| {
            let (object, generation, vector): (String, String, Vec<u8>) = conn.query_row("SELECT o.source_object_id,v.generation_id,v.vector FROM occurrence_vectors v JOIN occurrences o USING(occurrence_id)", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
            let text = if object == first { "first exact input" } else { assert_eq!(object, second); "second exact input" };
            assert_eq!(generation, config.generation.generation_id);
            assert_eq!(vector, retrieval::vectors::encode(&fixtures::TestEngine::vector_for(text)));
            assert_eq!(conn.query_row("SELECT count(*) FROM embedding_jobs WHERE state='pending' AND generation_id=?1", [&generation], |r| r.get::<_,i64>(0))?, 1);
            Ok(())
        }).unwrap();
    }
}

#[tokio::test]
async fn explicit_recovery_bootstraps_deregistered_and_pending_disabled_consumers() {
    for (deregister, completed) in [(false, false), (false, true), (true, false), (true, true)] {
        let root = tempfile::tempdir().unwrap();
        let corpus = Corpus::open(root.path());
        corpus.seed();
        let object = corpus.publish("base", "canonical truth");
        let mut config = spec(root.path());
        let gate = home_gate(root.path(), &config);
        record(root.path(), &gate, None, &config.identity);
        let mut selection = selector(root.path());
        assert_eq!(
            selection
                .recover_slice(
                    &corpus.kernel,
                    &gate,
                    &config,
                    &budget(Duration::from_secs(20)),
                    &mut |_| {}
                )
                .unwrap(),
            RecoveryProgress::Selected
        );
        let old = if completed {
            finish(&mut selection, &corpus, &gate, &config);
            current(root.path())
        } else {
            control(root.path())
        };
        let capture = old.replacement_capture.clone();
        gate.install(disable::cleanup_evaluator(root.path()));
        selection.begin_disable(&gate, &mut |_| {}).unwrap();
        if deregister {
            let disabled = selection
                .reconcile_disabled(
                    &corpus.kernel,
                    &gate,
                    &config,
                    &budget(Duration::from_secs(20)),
                    &mut |_| {},
                )
                .await
                .unwrap();
            assert!(disabled.deregistered);
        } else {
            corpus.publish("late", "late canonical bytes");
            assert!(
                selection
                    .reconcile_disabled(
                        &corpus.kernel,
                        &gate,
                        &config,
                        &budget(Duration::from_secs(20)),
                        &mut |_| {}
                    )
                    .await
                    .is_err()
            );
            assert!(
                corpus
                    .kernel
                    .outbox_consumer_checkpoint(CONSUMER)
                    .unwrap()
                    .is_some()
            );
        }
        config.generation.generation_id = "recovery-generation".to_owned();
        let mut next = request(None, &config.identity);
        next.transition = Transition::AuthorizedRecovery;
        next.cause = Cause::DisabledRecovery;
        next.selected_generation = old.staged_seed_digest.unwrap();
        next.consumer.consumer_id = "recovery-consumer".to_owned();
        next.consumer.generation_id = config.generation.generation_id.clone();
        next.attempt_id = "explicit-recovery".to_owned();
        let stopped = ProjectionLifecycle::open(root.path()).unwrap().read();
        assert!(
            selection
                .begin_authorized_recovery(
                    &corpus.kernel,
                    &gate,
                    &next,
                    &budget(Duration::from_secs(20))
                )
                .is_err()
        );
        assert_eq!(
            ProjectionLifecycle::open(root.path()).unwrap().read(),
            stopped
        );
        next.authorization_ref = Some("operator:fixture-recovery".to_owned());
        let mut stale =
            support::projection_gate::passing_evaluator(&config.identity, 0, &ProjectionHook::ALL);
        stale.evidence.identity.embedding_model = "stale-model".to_owned();
        gate.install(stale);
        assert!(
            selection
                .begin_authorized_recovery(
                    &corpus.kernel,
                    &gate,
                    &next,
                    &budget(Duration::from_secs(20))
                )
                .is_err()
        );
        assert_eq!(
            ProjectionLifecycle::open(root.path()).unwrap().read(),
            stopped
        );
        gate.install(support::projection_gate::passing_evaluator(
            &config.identity,
            0,
            &ProjectionHook::ALL,
        ));
        selection
            .begin_authorized_recovery(
                &corpus.kernel,
                &gate,
                &next,
                &budget(Duration::from_secs(20)),
            )
            .unwrap();
        finish(&mut selection, &corpus, &gate, &config);
        let done = current(root.path());
        assert_eq!(done.authorization_ref, next.authorization_ref);
        assert!(done.prior_disabled.is_none());
        if let Some(capture) = capture {
            assert_hold_released(root.path(), &capture.hold_id);
        }
        assert!(!database(root.path()).exists());
        assert_eq!(done.attempt_id, next.attempt_id);
        assert_eq!(
            corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
            None
        );
        assert_eq!(
            corpus
                .kernel
                .outbox_consumer_checkpoint("recovery-consumer")
                .unwrap(),
            done.recovery_target.map(|t| t.commit_seq)
        );
        let reader = selection
            .pin(&corpus.kernel, &gate, &budget(Duration::from_secs(10)))
            .unwrap();
        assert_eq!(observe(&reader).rows[&object].1, b"canonical truth");
        assert_eq!(observe(&reader).generation, "recovery-generation");
    }
}

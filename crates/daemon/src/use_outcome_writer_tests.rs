use super::*;
use crate::use_outcome_writer::{Serve, UseOutcomeWrite, record_serve};
use kernel::KernelError;
use kernel_route_fixtures::{MEMORY_DOMAIN, commit_verified_memory, ensure_domain, intent};

fn bound(project: &Path) -> SessionBinding {
    binding(project.to_str().unwrap(), "ses")
}

/// Commits the project's scope and one verified memory decision per `(object_id, summary)`.
fn seed_memories(kernel: &kernel::KernelStore, project: &Path, memories: &[(&str, &str)]) {
    let binding = bound(project);
    let scope_id = binding.kernel_project.scope_id();
    kernel
        .commit(intent(&format!("scope-{}", memories.len())), |envelope| {
            ensure_domain(envelope, MEMORY_DOMAIN)?;
            if envelope.object_state(&scope_id)?.is_none() {
                envelope.insert_scope(binding.kernel_project.scope_spec(MEMORY_DOMAIN))?;
            }
            Ok(String::new())
        })
        .unwrap();
    for (object_id, summary) in memories {
        commit_verified_memory(
            kernel,
            object_id,
            object_id,
            &scope_id,
            "PROJECT_RULES",
            summary,
        );
    }
}

fn served(handler: &Handler, project: &Path) -> Vec<kernel::UseOutcomeRecord> {
    let binding = bound(project);
    handler
        .kernel
        .kernel_store()
        .unwrap()
        .served_set(binding.kernel_project.digest(), &binding.harness, "ses")
        .unwrap()
        .unwrap_or_default()
}

fn counts(handler: &Handler) -> Vec<(&'static str, u64)> {
    UseOutcomeWrite::ALL
        .iter()
        .map(|outcome| (outcome.name(), handler.use_outcome_counts.get(*outcome)))
        .filter(|(_, count)| *count > 0)
        .collect()
}

fn kernel_commits(handler: &Handler) -> i64 {
    handler.kernel.kernel_store().unwrap().tip().unwrap()
}

/// The first HARD fold over a Canonical composition records one serve with the snapshot's ids, `served_commit_seq = known_as_of`, the digest, the revision, and the primary role; a fold whose composition changed records a second serve.
#[tokio::test(flavor = "current_thread")]
async fn a_hard_fold_records_its_frozen_composition_once_per_served_set() {
    let producer = Arc::new(ProducerState::default());
    let (handler, _store, _dir, project) =
        handler_with_store_and_kernel(producer, default_test_config()).await;
    let kernel = handler.kernel.kernel_store().unwrap();
    seed_memories(
        &kernel,
        &project,
        &[("mem-b", "Rule b."), ("mem-a", "Rule a.")],
    );

    let first = call_transform(&handler, vec![ck("m1", 1, "hello")]).await;
    assert_eq!(first["action"], "HARD", "{first}");
    let composition = &first["project_memory"];
    assert_eq!(composition["kind"], "canonical", "{first}");
    let records = served(&handler, &project);
    assert_eq!(records.len(), 1);
    let serve = &records[0].serve;
    assert_eq!(records[0].decision_object_ids, ["mem-a", "mem-b"]);
    assert_eq!(
        serve.served_set_digest,
        kernel::served_set_digest(["mem-a", "mem-b"])
    );
    assert_eq!(
        serve.served_commit_seq,
        composition["known_as_of"].as_i64().unwrap()
    );
    assert_eq!(
        serve.composition_revision,
        composition["revision"].as_u64().unwrap()
    );
    assert_eq!(serve.session_role, kernel::SessionRole::Primary);
    assert_eq!(counts(&handler), [("written", 1)]);

    let quiet = call_transform(&handler, vec![ck("m1", 1, "hello"), ck("m2", 2, "again")]).await;
    assert_ne!(quiet["action"], "HARD", "{quiet}");
    assert_eq!(
        served(&handler, &project).len(),
        1,
        "a pass that froze nothing records nothing"
    );

    seed_memories(&kernel, &project, &[("mem-c", "Rule c.")]);
    let changed = call_transform(
        &handler,
        vec![
            ck("m1", 1, "hello"),
            ck("m2", 2, "again"),
            ck("m3", 3, "more"),
        ],
    )
    .await;
    assert_eq!(changed["action"], "HARD", "{changed}");
    let records = served(&handler, &project);
    assert_eq!(records.len(), 2);
    assert_eq!(records[1].decision_object_ids, ["mem-a", "mem-b", "mem-c"]);
    assert_eq!(counts(&handler), [("written", 2)]);
}

/// A write at an equal served-set digest replays and adds no commit; the same key under another harness or role is a conflict that leaves the first record unchanged.
#[tokio::test(flavor = "current_thread")]
async fn an_equal_digest_replays_and_another_harness_or_role_conflicts() {
    let producer = Arc::new(ProducerState::default());
    let (handler, _store, _dir, project) =
        handler_with_store_and_kernel(producer, default_test_config()).await;
    let kernel = handler.kernel.kernel_store().unwrap();
    seed_memories(&kernel, &project, &[("mem-a", "Rule a.")]);
    let snapshot = canonical_memory::CanonicalMemorySnapshot::new(
        kernel.tip().unwrap(),
        false,
        vec![canonical_memory::CanonicalMemory {
            object_id: "mem-a".to_string(),
            category: "PROJECT_RULES".to_string(),
            content: "Rule a.".to_string(),
        }],
    );
    let binding = bound(&project);
    let serve = |harness: &'static str, is_subagent: bool| Serve {
        project: &binding.kernel_project,
        harness,
        session_id: "ses",
        is_subagent,
        snapshot: &snapshot,
        recorded_at: 1,
    };
    assert_eq!(
        record_serve(&handler.kernel, &serve("daemon-test", false)),
        Some(UseOutcomeWrite::Written)
    );
    let tip = kernel_commits(&handler);
    assert_eq!(
        record_serve(&handler.kernel, &serve("daemon-test", false)),
        Some(UseOutcomeWrite::Replayed)
    );
    assert_eq!(kernel_commits(&handler), tip, "a replay commits nothing");
    assert_eq!(
        record_serve(&handler.kernel, &serve("other-harness", false)),
        Some(UseOutcomeWrite::Conflict)
    );
    assert_eq!(
        record_serve(&handler.kernel, &serve("daemon-test", true)),
        Some(UseOutcomeWrite::Conflict)
    );
    let records = served(&handler, &project);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].serve.session_role, kernel::SessionRole::Primary);
}

/// A route bound with memory disabled takes no read and records nothing; a pass whose read was withheld records nothing; an Available composition with no rows records a serve with no decision rows.
#[tokio::test(flavor = "current_thread")]
async fn only_an_available_composition_is_recorded() {
    let producer = Arc::new(ProducerState::default());
    let (handler, _store, _dir, project) =
        handler_with_store_and_kernel(Arc::clone(&producer), default_test_config()).await;
    let kernel = handler.kernel.kernel_store().unwrap();
    seed_memories(&kernel, &project, &[]);
    let empty = call_transform(&handler, vec![ck("m1", 1, "hello")]).await;
    assert_eq!(empty["action"], "HARD", "{empty}");
    let records = served(&handler, &project);
    assert_eq!(records.len(), 1, "{empty}");
    assert!(records[0].decision_object_ids.is_empty());

    for engine in [
        default_test_config(),
        fold_authority_handler_tests::native_config(),
    ] {
        let (disabled, _store, _dir, disabled_project) =
            handler_with_store_and_kernel(Arc::clone(&producer), default_test_config()).await;
        disabled.bind_route(
            test_route(7),
            SessionBinding {
                config: DaemonConfig {
                    memory_enabled: false,
                    ..engine
                },
                ..bound(&disabled_project)
            },
        );
        seed_memories(
            &disabled.kernel.kernel_store().unwrap(),
            &disabled_project,
            &[("mem-a", "Rule a.")],
        );
        let response = call_transform(&disabled, vec![ck("m1", 1, "hello")]).await;
        assert_eq!(response["action"], "HARD", "{response}");
        assert!(served(&disabled, &disabled_project).is_empty());
        assert!(
            counts(&disabled).is_empty(),
            "a disabled route attempts no write"
        );
    }

    for engine in [
        default_test_config(),
        fold_authority_handler_tests::native_config(),
    ] {
        let (withheld, _store, _dir, withheld_project) =
            handler_with_store(Arc::clone(&producer), default_test_config());
        withheld.bind_route(
            test_route(7),
            SessionBinding {
                config: engine,
                ..bound(&withheld_project)
            },
        );
        let response = call_transform(&withheld, vec![ck("m1", 1, "hello")]).await;
        assert_eq!(response["project_memory"]["kind"], "withheld", "{response}");
        assert_eq!(response["action"], "HARD", "{response}");
        assert!(
            counts(&withheld).is_empty(),
            "a withheld read attempts no write"
        );
    }
}

/// A Kernel that fails after the fold's Memory Store commit leaves the response bytes and the frozen composition equal to a run with no fault, the pass answers `ok`, the outcome is counted, and a later HARD fold at the same served set records the serve.
#[tokio::test(flavor = "current_thread")]
async fn a_failed_write_changes_nothing_the_harness_receives_and_a_later_fold_repairs_it() {
    for (fault, name) in [
        (KernelError::Deadline, "deadline"),
        (KernelError::Busy, "deadline"),
        (KernelError::Held, "kernel_unavailable"),
        (KernelError::FenceLost, "kernel_unavailable"),
    ] {
        let producer = Arc::new(ProducerState::default());
        let (clean, clean_store, _dir, clean_project) =
            handler_with_store_and_kernel(Arc::clone(&producer), default_test_config()).await;
        seed_memories(
            &clean.kernel.kernel_store().unwrap(),
            &clean_project,
            &[("mem-a", "Rule a.")],
        );
        let (faulted, faulted_store, _dir, faulted_project) =
            handler_with_store_and_kernel(Arc::clone(&producer), default_test_config()).await;
        seed_memories(
            &faulted.kernel.kernel_store().unwrap(),
            &faulted_project,
            &[("mem-a", "Rule a.")],
        );
        *faulted.use_outcome_fault.lock().unwrap() = Some(Some(fault));

        let expected = call_transform(&clean, vec![ck("m1", 1, "hello")]).await;
        let observed = call_transform(&faulted, vec![ck("m1", 1, "hello")]).await;
        assert_eq!(observed["status"], "ok", "{name}: {observed}");
        for field in [
            "messages",
            "project_memory",
            "action",
            "decision",
            "committed",
            "boundary",
            "materialize_reason",
            "history_summarizer",
        ] {
            assert_eq!(observed[field], expected[field], "{name}: {field}");
        }
        assert_eq!(
            faulted_store.load("ses").unwrap().meta.project_memory,
            clean_store.load("ses").unwrap().meta.project_memory,
            "{name}"
        );
        assert!(served(&faulted, &faulted_project).is_empty(), "{name}");
        assert_eq!(counts(&faulted), [(name, 1)], "{name}");

        // Context pressure folds HARD again over the unchanged composition.
        let repaired = call_transform_with_usage(&faulted, big_messages(), 48_000, 50_000).await;
        assert_eq!(
            repaired["project_memory"], observed["project_memory"],
            "{name}: the served set is unchanged"
        );
        let records = served(&faulted, &faulted_project);
        assert_eq!(records.len(), 1, "{name}");
        assert_eq!(records[0].decision_object_ids, ["mem-a"], "{name}");
    }
}

/// A fold attempt discarded by a compare-and-swap conflict leaves no Kernel row; the retried attempt records exactly one serve.
#[tokio::test(flavor = "current_thread")]
async fn a_discarded_attempt_leaves_no_serve_and_the_retry_records_one() {
    let producer = Arc::new(ProducerState::default());
    let (handler, store, _dir, project) =
        handler_with_store_and_kernel(producer, default_test_config()).await;
    seed_memories(
        &handler.kernel.kernel_store().unwrap(),
        &project,
        &[("mem-a", "Rule a.")],
    );
    // A session of its own keeps the process-wide attempt hook from firing on another test's pass.
    let session = "discarded-attempt";
    let session_binding = binding(project.to_str().unwrap(), session);
    handler.bind_route(test_route(9), session_binding.clone());
    let rival = Arc::clone(&store);
    let raced = Arc::new(Mutex::new(None));
    let fired = Arc::clone(&raced);
    crate::transform::install_transform_attempt_hook(session, move || {
        let loaded = rival.load(session).unwrap();
        rival
            .commit(session, loaded.row_version, &loaded.core, &loaded.meta)
            .unwrap();
        *fired.lock().unwrap() = Some(loaded.meta.project_memory);
    });
    let mut raced_request = request(vec![ck("m1", 1, "hello")]);
    raced_request["session_id"] = json!(session);
    let response = call_transform_request_on_channel(&handler, 9, raced_request).await;
    assert_eq!(
        *raced.lock().unwrap(),
        Some(None),
        "the rival committed before the attempt's commit, while no composition was stored"
    );
    assert_eq!(response["action"], "HARD", "{response}");
    let after = store.load(session).unwrap();
    assert!(
        after.row_version > Some(1),
        "the rival's commit and the retried attempt's commit both landed"
    );
    assert!(after.meta.project_memory.is_some());
    let records = handler
        .kernel
        .kernel_store()
        .unwrap()
        .served_set(
            session_binding.kernel_project.digest(),
            &session_binding.harness,
            session,
        )
        .unwrap()
        .unwrap_or_default();
    assert_eq!(records.len(), 1);
    assert_eq!(counts(&handler), [("written", 1)]);
}

/// On the additive engine a subagent pass that freezes a Canonical composition records the `subagent` role; on the `eidnara_folds` engine the same request freezes no composition and records nothing.
#[tokio::test(flavor = "current_thread")]
async fn the_session_role_follows_the_pass_subagent_flag_on_each_engine() {
    for (config, is_subagent, expected) in [
        (
            fold_authority_handler_tests::native_config(),
            true,
            Some(kernel::SessionRole::Subagent),
        ),
        (
            fold_authority_handler_tests::native_config(),
            false,
            Some(kernel::SessionRole::Primary),
        ),
        (default_test_config(), true, None),
    ] {
        let producer = Arc::new(ProducerState::default());
        let (handler, _store, _dir, project) =
            handler_with_store_and_kernel(producer, default_test_config()).await;
        handler.bind_route(
            test_route(7),
            SessionBinding {
                config,
                ..bound(&project)
            },
        );
        seed_memories(
            &handler.kernel.kernel_store().unwrap(),
            &project,
            &[("mem-a", "Rule a.")],
        );
        let mut subagent = request(vec![ck("m1", 1, "hello")]);
        subagent["is_subagent"] = json!(is_subagent);
        let response = call_transform_request(&handler, subagent).await;
        assert_eq!(response["status"], "ok", "{response}");
        if expected.is_none() {
            assert_ne!(response["action"], "HARD", "{response}");
        }
        let roles: Vec<_> = served(&handler, &project)
            .iter()
            .map(|record| record.serve.session_role)
            .collect();
        assert_eq!(
            roles,
            expected.into_iter().collect::<Vec<_>>(),
            "{response}"
        );
    }
}

/// A fresh fold over an equal served set at a later `known_as_of` replays the first record: the serve key names the served set, not the snapshot sequence.
#[tokio::test(flavor = "current_thread")]
async fn an_equal_served_set_at_a_later_known_as_of_replays() {
    let producer = Arc::new(ProducerState::default());
    let (handler, _store, _dir, project) =
        handler_with_store_and_kernel(producer, default_test_config()).await;
    let kernel = handler.kernel.kernel_store().unwrap();
    seed_memories(&kernel, &project, &[("mem-a", "Rule a.")]);
    let first = call_transform(&handler, vec![ck("m1", 1, "hello")]).await;
    let deleted = handler
        .core
        .handle_session_delete_value(
            test_route(7),
            &json!({"method":"session.delete","session_id":"ses"}),
        )
        .await;
    assert!(
        matches!(deleted, PreparedOutcome::Response(_)),
        "{deleted:?}"
    );
    kernel
        .commit(intent("unrelated"), |envelope| {
            ensure_domain(envelope, "unrelated")?;
            Ok(String::new())
        })
        .unwrap();
    let later = call_transform(&handler, vec![ck("m1", 1, "hello")]).await;
    assert_eq!(later["action"], "HARD", "{later}");
    let first_seq = first["project_memory"]["known_as_of"].as_i64().unwrap();
    assert!(later["project_memory"]["known_as_of"].as_i64().unwrap() > first_seq);
    let records = served(&handler, &project);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].serve.served_commit_seq, first_seq);
    assert_eq!(counts(&handler), [("written", 1), ("replayed", 1)]);
}

/// A disposition committed between a HARD fold and the next pass hides the disputed decision from the next composition, so the fold it requests records the smaller served set.
#[tokio::test(flavor = "current_thread")]
async fn a_dispute_between_folds_records_the_next_served_set_without_it() {
    use kernel::{EventKind, SourceClass, TaintClass};
    let producer = Arc::new(ProducerState::default());
    let (handler, _store, _dir, project) =
        handler_with_store_and_kernel(producer, default_test_config()).await;
    let kernel = handler.kernel.kernel_store().unwrap();
    seed_memories(
        &kernel,
        &project,
        &[("mem-a", "Rule a."), ("mem-b", "Rule b.")],
    );
    let _ = call_transform(&handler, vec![ck("m1", 1, "hello")]).await;
    kernel
        .commit(intent("dispute"), |envelope| {
            envelope.record_admission(kernel_route_fixtures::admission(
                "mem-a",
                EventKind::MarkDisputed,
                None,
                (SourceClass::TrustedLocalCode, TaintClass::CurrentCode),
            ))?;
            Ok(String::new())
        })
        .unwrap();
    let next = call_transform(&handler, vec![ck("m1", 1, "hello"), ck("m2", 2, "again")]).await;
    assert_eq!(next["action"], "HARD", "{next}");
    let records = served(&handler, &project);
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].decision_object_ids, ["mem-a", "mem-b"]);
    assert_eq!(records[1].decision_object_ids, ["mem-b"]);
    let disputed = kernel.served_sessions_for("mem-a").unwrap();
    assert_eq!(disputed.len(), 1);
    assert!(disputed[0].disputed_after_serve);
}

/// A pass whose read was withheld records nothing, and the first pass that reads the same rows Available folds and records them.
#[tokio::test(flavor = "current_thread")]
async fn a_withheld_read_then_an_available_read_of_the_same_rows_records_once() {
    let producer = Arc::new(ProducerState::default());
    let (handler, _store, dir, project) =
        handler_with_store(Arc::clone(&producer), default_test_config());
    let withheld = call_transform(&handler, vec![ck("m1", 1, "hello")]).await;
    assert_eq!(withheld["project_memory"]["kind"], "withheld", "{withheld}");
    assert_eq!(withheld["action"], "HARD", "{withheld}");
    handler.kernel.set_expected(true);
    handler
        .kernel
        .open(
            dir.path().join("kernel"),
            StoreOpenPolicy::default(),
            handler.cancel.clone(),
        )
        .await;
    seed_memories(
        &handler.kernel.kernel_store().unwrap(),
        &project,
        &[("mem-a", "Rule a.")],
    );
    let available =
        call_transform(&handler, vec![ck("m1", 1, "hello"), ck("m2", 2, "again")]).await;
    assert_eq!(
        available["project_memory"]["kind"], "canonical",
        "{available}"
    );
    assert_eq!(available["action"], "HARD", "{available}");
    let records = served(&handler, &project);
    assert_eq!(records.len(), 1);
    assert_eq!(counts(&handler), [("written", 1)]);
}

/// A wrapup reads consent from the live config, not the route's frozen one; it composes its prompt from its own read and records no serve, because only a transform pass that froze a composition writes one.
#[tokio::test(flavor = "current_thread")]
async fn a_wrapup_under_a_config_that_differs_from_the_binding_records_no_serve() {
    let producer = Arc::new(ProducerState::default());
    let (handler, _store, _dir, project) =
        handler_with_store_and_kernel(Arc::clone(&producer), default_test_config()).await;
    handler.bind_route(
        test_route(7),
        SessionBinding {
            config: DaemonConfig {
                memory_enabled: false,
                ..default_test_config()
            },
            ..bound(&project)
        },
    );
    seed_memories(
        &handler.kernel.kernel_store().unwrap(),
        &project,
        &[("mem-a", "Rule a.")],
    );
    cache_wrapup_messages(&handler, wrapup_messages(80, 800));
    let body = tool_body(
        handler
            .dispatch_value(
                test_route(7),
                json!({ "method": "session.wrapup", "session_id": "ses" }),
            )
            .await,
    );
    assert_eq!(body["ok"], json!(true), "{body}");
    assert!(
        producer.starts.load(Ordering::SeqCst) >= 1,
        "the wrapup fired"
    );
    assert!(served(&handler, &project).is_empty());
    assert!(counts(&handler).is_empty());
}

/// The outcome vocabulary is closed: the counters and the log line carry these names and nothing else.
#[test]
fn the_write_outcomes_are_a_closed_vocabulary() {
    assert_eq!(
        UseOutcomeWrite::ALL.map(UseOutcomeWrite::name),
        [
            "written",
            "replayed",
            "conflict",
            "deadline",
            "kernel_unavailable",
            "refused"
        ]
    );
}

/// A Kernel that is starting at the write, a Kernel writer held past the write budget, a same-key serve under another role on a later fold, and a read at sequence 0 each leave the response alone and count their outcome; the held writer bounds the measured wait.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_real_failure_paths_count_their_outcomes_and_bound_the_wait() {
    let producer = Arc::new(ProducerState::default());
    let (starting, _store, _dir, project) =
        handler_with_store(Arc::clone(&producer), default_test_config());
    starting.kernel.set_expected(true);
    let snapshot = canonical_memory::CanonicalMemorySnapshot::new(3, false, Vec::new());
    let binding = bound(&project);
    let serve = Serve {
        project: &binding.kernel_project,
        harness: "daemon-test",
        session_id: "ses",
        is_subagent: false,
        snapshot: &snapshot,
        recorded_at: 1,
    };
    assert_eq!(
        record_serve(&starting.kernel, &serve),
        None,
        "a Kernel that is not Ready attempts no write"
    );

    let (held, _store, _dir, project) =
        handler_with_store_and_kernel(Arc::clone(&producer), default_test_config()).await;
    let kernel = held.kernel.kernel_store().unwrap();
    seed_memories(&kernel, &project, &[]);
    let snapshot =
        canonical_memory::CanonicalMemorySnapshot::new(kernel.tip().unwrap(), false, Vec::new());
    let binding = bound(&project);
    let serve = Serve {
        project: &binding.kernel_project,
        harness: "daemon-test",
        session_id: "ses",
        is_subagent: false,
        snapshot: &snapshot,
        recorded_at: 1,
    };
    let (holding, release) = std::sync::mpsc::channel::<()>();
    let (entered, writer_held) = std::sync::mpsc::channel::<()>();
    let holder = {
        let kernel = Arc::clone(&kernel);
        std::thread::spawn(move || {
            kernel
                .commit(intent("hold-writer"), |_| {
                    entered.send(()).unwrap();
                    release.recv().unwrap();
                    Ok(String::new())
                })
                .unwrap();
        })
    };
    writer_held.recv().unwrap();
    let started = std::time::Instant::now();
    let outcome = record_serve(&held.kernel, &serve);
    let waited = started.elapsed();
    holding.send(()).unwrap();
    holder.join().unwrap();
    assert_eq!(outcome, Some(UseOutcomeWrite::Deadline));
    eprintln!("use-outcome writer wait under a held writer: {waited:?}");
    assert!(
        waited >= std::time::Duration::from_millis(250)
            && waited < std::time::Duration::from_secs(5),
        "the wait is bounded by the write budget: {waited:?}"
    );
    assert!(served(&held, &project).is_empty());
    assert_eq!(
        record_serve(&held.kernel, &serve),
        Some(UseOutcomeWrite::Written)
    );
    let as_subagent = Serve {
        is_subagent: true,
        ..serve
    };
    for _ in 0..2 {
        assert_eq!(
            record_serve(&held.kernel, &as_subagent),
            Some(UseOutcomeWrite::Conflict),
            "a conflict is not repaired by a later fold"
        );
    }
    let records = served(&held, &project);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].serve.session_role, kernel::SessionRole::Primary);

    let (fresh, _store, _dir, fresh_project) =
        handler_with_store_and_kernel(Arc::clone(&producer), default_test_config()).await;
    assert_eq!(fresh.kernel.kernel_store().unwrap().tip().unwrap(), 0);
    let response = call_transform(&fresh, vec![ck("m1", 1, "hello")]).await;
    assert_eq!(response["action"], "HARD", "{response}");
    assert_eq!(response["project_memory"]["known_as_of"], 0, "{response}");
    assert!(
        counts(&fresh).is_empty(),
        "a read at sequence 0 names no commit, so no write is attempted"
    );
    assert!(served(&fresh, &fresh_project).is_empty());
}

/// A recorded served set replays from a reader while another writer holds the Kernel writer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_recorded_served_set_replays_while_another_writer_holds_the_writer() {
    let producer = Arc::new(ProducerState::default());
    let (handler, _store, _dir, project) =
        handler_with_store_and_kernel(producer, default_test_config()).await;
    let kernel = handler.kernel.kernel_store().unwrap();
    seed_memories(
        &kernel,
        &project,
        &[("mem-a", "Rule a."), ("mem-b", "Rule b.")],
    );
    let snapshot = canonical_memory::CanonicalMemorySnapshot::new(
        kernel.tip().unwrap(),
        false,
        ["mem-a", "mem-b"]
            .map(|object_id| canonical_memory::CanonicalMemory {
                object_id: object_id.to_string(),
                category: "PROJECT_RULES".to_string(),
                content: format!("Rule {object_id}."),
            })
            .to_vec(),
    );
    let binding = bound(&project);
    let serve = Serve {
        project: &binding.kernel_project,
        harness: "daemon-test",
        session_id: "ses",
        is_subagent: false,
        snapshot: &snapshot,
        recorded_at: 1,
    };
    assert_eq!(
        record_serve(&handler.kernel, &serve),
        Some(UseOutcomeWrite::Written)
    );
    let (holding, release) = std::sync::mpsc::channel::<()>();
    let (entered, writer_held) = std::sync::mpsc::channel::<()>();
    let holder = {
        let kernel = Arc::clone(&kernel);
        std::thread::spawn(move || {
            kernel
                .commit(intent("hold-writer"), |_| {
                    entered.send(()).unwrap();
                    release.recv().unwrap();
                    Ok(String::new())
                })
                .unwrap();
        })
    };
    writer_held.recv().unwrap();
    let started = std::time::Instant::now();
    let replayed = record_serve(&handler.kernel, &serve);
    let waited = started.elapsed();
    let conflict = record_serve(
        &handler.kernel,
        &Serve {
            is_subagent: true,
            ..serve
        },
    );
    let other_session = Serve {
        session_id: "other",
        ..serve
    };
    let deadline = record_serve(&handler.kernel, &other_session);
    holding.send(()).unwrap();
    holder.join().unwrap();
    assert_eq!(replayed, Some(UseOutcomeWrite::Replayed));
    eprintln!("use-outcome replay under a held writer: {waited:?}");
    assert!(
        waited < std::time::Duration::from_millis(250),
        "a replay answers before the write budget ends: {waited:?}"
    );
    assert_eq!(conflict, Some(UseOutcomeWrite::Conflict));
    assert_eq!(deadline, Some(UseOutcomeWrite::Deadline));
    let served_in = |session_id: &str| {
        kernel
            .served_set(binding.kernel_project.digest(), "daemon-test", session_id)
            .unwrap()
            .unwrap_or_default()
    };
    let records = served_in("ses");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].decision_object_ids, ["mem-a", "mem-b"]);
    assert_eq!(records[0].serve.session_role, kernel::SessionRole::Primary);
    assert!(served_in("other").is_empty());
    assert_eq!(
        record_serve(&handler.kernel, &other_session),
        Some(UseOutcomeWrite::Written)
    );

    let (entered, writer_held) = std::sync::mpsc::channel::<()>();
    let brief = {
        let kernel = Arc::clone(&kernel);
        std::thread::spawn(move || {
            kernel
                .commit(intent("hold-writer-briefly"), |_| {
                    entered.send(()).unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    Ok(String::new())
                })
                .unwrap();
        })
    };
    writer_held.recv().unwrap();
    let started = std::time::Instant::now();
    let written = record_serve(
        &handler.kernel,
        &Serve {
            session_id: "third",
            ..serve
        },
    );
    let waited = started.elapsed();
    brief.join().unwrap();
    assert_eq!(
        written,
        Some(UseOutcomeWrite::Written),
        "a new served set waits for a writer released within the budget"
    );
    assert!(waited < std::time::Duration::from_millis(250), "{waited:?}");
    let third = served_in("third");
    assert_eq!(third.len(), 1);
    assert_eq!(third[0].decision_object_ids, ["mem-a", "mem-b"]);
}

/// The outcome vocabulary indexes its counters in order, and the Kernel errors the writer meets map to their closed names.
#[test]
fn every_kernel_error_maps_to_a_closed_outcome() {
    for (index, outcome) in UseOutcomeWrite::ALL.iter().enumerate() {
        assert_eq!(*outcome as usize, index);
    }
    for (error, outcome) in [
        (KernelError::Conflict, UseOutcomeWrite::Conflict),
        (KernelError::Busy, UseOutcomeWrite::Deadline),
        (KernelError::Deadline, UseOutcomeWrite::Deadline),
        (KernelError::FenceLost, UseOutcomeWrite::KernelUnavailable),
        (KernelError::Held, UseOutcomeWrite::KernelUnavailable),
        (KernelError::InvalidInput, UseOutcomeWrite::Refused),
        (KernelError::NotFound, UseOutcomeWrite::Refused),
    ] {
        assert_eq!(UseOutcomeWrite::from_error(error), outcome, "{error:?}");
    }
}

/// An emergency pass that reruns after its HARD first run records one serve: every attempt of the pass freezes the same pinned read, and the first settled write covers the rerun.
#[tokio::test(flavor = "current_thread")]
async fn an_emergency_pass_that_reruns_records_one_serve() {
    let producer = Arc::new(ProducerState::default());
    let (handler, _store, _dir, project) =
        handler_with_store_and_kernel(Arc::clone(&producer), default_test_config()).await;
    seed_memories(
        &handler.kernel.kernel_store().unwrap(),
        &project,
        &[("mem-a", "Rule a.")],
    );
    let emergency = call_transform_with_usage(&handler, big_messages(), 48_000, 50_000).await;
    assert_eq!(
        emergency["history_summarizer"]["fired"], true,
        "{emergency}"
    );
    let runs = tool_body(handler.handle_session_status_value(
        test_route(7),
        &json!({ "method": "session.status", "session_id": "ses" }),
    ))["pass_trace"]["scheduler_history"]
        .as_array()
        .unwrap()
        .len();
    assert!(runs >= 2, "the pass reran");
    assert_eq!(served(&handler, &project).len(), 1);
    assert_eq!(counts(&handler), [("written", 1)]);
}

/// Within one rerunning pass, a write that met a Kernel that was not Ready is retried by the rerun, while any commit outcome settles the pass, including a lost fence after the writer wait: one writer wait per pass at most.
#[tokio::test(flavor = "current_thread")]
async fn only_an_unready_kernel_leaves_the_pass_to_retry_its_serve() {
    for (fault, expected, records) in [
        (None, vec![("written", 1), ("kernel_unavailable", 1)], 1),
        (Some(KernelError::Deadline), vec![("deadline", 1)], 0),
        (Some(KernelError::Conflict), vec![("conflict", 1)], 0),
        (
            Some(KernelError::FenceLost),
            vec![("kernel_unavailable", 1)],
            0,
        ),
    ] {
        let producer = Arc::new(ProducerState::default());
        let (handler, _store, _dir, project) =
            handler_with_store_and_kernel(Arc::clone(&producer), default_test_config()).await;
        seed_memories(
            &handler.kernel.kernel_store().unwrap(),
            &project,
            &[("mem-a", "Rule a.")],
        );
        *handler.use_outcome_fault.lock().unwrap() = Some(fault);
        let emergency = call_transform_with_usage(&handler, big_messages(), 48_000, 50_000).await;
        assert_eq!(emergency["status"], "ok", "{fault:?}: {emergency}");
        assert_eq!(counts(&handler), expected, "{fault:?}");
        assert_eq!(served(&handler, &project).len(), records, "{fault:?}");
    }
}

/// A committed HARD fold whose boundary read then fails answers an error, and the composition its commit froze is still recorded once.
#[tokio::test(flavor = "current_thread")]
async fn a_fold_whose_boundary_read_fails_after_its_commit_records_its_serve() {
    let producer = Arc::new(ProducerState::default());
    let (handler, _store, dir, project) =
        handler_with_store_and_kernel(Arc::clone(&producer), default_test_config()).await;
    seed_memories(
        &handler.kernel.kernel_store().unwrap(),
        &project,
        &[("mem-a", "Rule a.")],
    );
    let descriptor = dev_descriptor_at(dir.path().join("data").to_str().unwrap());
    let StorageBackend::Sqlite { path } = descriptor.backend else {
        panic!("the test store is SQLite");
    };
    // A session of its own keeps the process-wide attempt hook from firing on another test's pass.
    let session = "boundary-read-fails";
    handler.bind_route(test_route(9), binding(project.to_str().unwrap(), session));
    crate::transform::install_transform_attempt_hook(
        &format!("rendered_boundary:{session}"),
        move || {
            rusqlite::Connection::open(&path)
                .unwrap()
                .execute(
                    "ALTER TABLE history_segments RENAME TO history_segments_moved",
                    [],
                )
                .unwrap();
        },
    );
    let mut boundary_request = request(vec![ck("m1", 1, "hello")]);
    boundary_request["session_id"] = json!(session);
    let outcome = handler
        .handle_transform_applied_for_test(test_route(9), boundary_request)
        .await;
    assert!(
        matches!(outcome, PreparedOutcome::Error { .. }),
        "{outcome:?}"
    );
    let binding = bound(&project);
    let records = handler
        .kernel
        .kernel_store()
        .unwrap()
        .served_set(binding.kernel_project.digest(), &binding.harness, session)
        .unwrap()
        .unwrap_or_default();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].decision_object_ids, ["mem-a"]);
    assert_eq!(counts(&handler), [("written", 1)]);
}

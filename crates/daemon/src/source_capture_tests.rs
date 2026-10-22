//! Source capture through the transform handler against a real memory store and a real kernel store: consent, the background slot, dispositions, response independence, restart replay, shutdown, and disclosure.

use std::num::NonZeroUsize;
use std::sync::mpsc;

use kernel::source_identity::OccurrenceClass;
use kernel::{ArtifactHandle, KernelStore, LiveDescriptor, SourceClass, Surface, TaintClass};

use super::*;
use crate::harness_sources::MAX_SOURCE_BATCH_UNITS;

const SESSION: &str = "ses";
/// A literal that must never reach a log line, a counter, or the health report.
const SENTINEL: &str = "capture-sentinel-7f3a";

fn capture_config(enabled: bool) -> DaemonConfig {
    let mut config = default_test_config();
    config.model_chain.clear();
    config.source_capture_enabled = enabled;
    config
}

async fn capture_handler(
    config: DaemonConfig,
    kernel: bool,
) -> (Handler, tempfile::TempDir, PathBuf) {
    let producer = Arc::new(ProducerState::default());
    let (handler, _store, dir, project) = if kernel {
        handler_with_store_and_kernel(producer, config.clone()).await
    } else {
        handler_with_store(producer, config.clone())
    };
    rebind(&handler, &project, config);
    (handler, dir, project)
}

fn rebind(handler: &Handler, project: &Path, config: DaemonConfig) {
    let mut bound = binding_with_harness(project.to_str().unwrap(), "opencode", SESSION);
    bound.config = config;
    handler.bind_route(test_route(7), bound);
}

fn opencode_user(id: &str, created: u64, text: &str) -> Value {
    json!({
        "info": { "id": id, "sessionID": SESSION, "role": "user", "time": { "created": created } },
        "parts": [{ "type": "text", "text": text }],
    })
}

/// An assistant turn with text, a `bash` tool that failed, and a `read` tool that completed.
fn opencode_assistant_with_tools(id: &str, completed: u64) -> Value {
    json!({
        "info": { "id": id, "sessionID": SESSION, "role": "assistant",
                  "time": { "created": completed - 10, "completed": completed } },
        "parts": [
            { "type": "text", "text": "Running the checks." },
            { "type": "tool", "callID": "call_bash", "tool": "bash",
              "state": { "status": "error", "error": "exit 2: cargo not found\n",
                         "input": { "command": "cargo test" },
                         "time": { "start": completed - 5, "end": completed - 4 } } },
            { "type": "tool", "callID": "call_read", "tool": "read",
              "state": { "status": "completed", "output": "fn main() {}\n",
                         "input": { "filePath": "src/main.rs" }, "title": "src/main.rs",
                         "metadata": {},
                         "time": { "start": completed - 3, "end": completed - 2 } } },
        ],
    })
}

fn transform_with(native: &[Value], base: &str) -> Value {
    let decoded = codec::decode_opencode(native);
    let messages: Vec<IngressMessage> = decoded
        .messages
        .into_iter()
        .map(|mut message| {
            message.ck.meta.created_at_ms = Some(1_000 + message.ordinal as i64);
            message
        })
        .collect();
    let mut request = request_with_usage(messages, 1_000, 200_000);
    request["serializer_profile"] = json!("opencode-aisdk");
    request["native_messages"] = json!(native);
    request["base_revision"] = json!(base);
    request
}

/// Runs one pass as the transport does: the response settles, then its after-response work starts. Returns the response without its timings and its output revisions, which name the daemon incarnation that served them.
async fn pass(handler: &Handler, request: Value) -> Value {
    let request = handler.test_client_prepare_request(request);
    let mut outcome = handler
        .handle_transform_for_test(test_route(7), request.clone())
        .await;
    let after = outcome.take_after_response();
    let applied = handler.test_client_apply(&request, outcome);
    let mut response: Value = match applied {
        PreparedOutcome::Response(bytes) => serde_json::from_slice(&bytes).unwrap(),
        PreparedOutcome::Error { code, message } => panic!("unexpected error {code}: {message}"),
        other => panic!("unexpected outcome {other:?}"),
    };
    if let Some(after) = after {
        after.run();
    }
    let fields = response.as_object_mut().unwrap();
    fields.remove("timings");
    fields.remove("output_revision");
    fields.remove("previous_output_revision");
    response
}

async fn settle(handler: &Handler) {
    for _ in 0..500 {
        if handler.source_capture.is_idle() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the capture job never finished");
}

fn report(handler: &Handler) -> Value {
    handler.source_capture.report(false, now_ms())
}

fn kernel_store(handler: &Handler) -> Arc<KernelStore> {
    handler.kernel.kernel_store().unwrap()
}

fn live(store: &KernelStore, class: OccurrenceClass) -> Vec<LiveDescriptor> {
    let budget = kernel::applicability::EvalBudget::new(
        Some(Instant::now() + Duration::from_secs(5)),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    );
    let tip = store.tip().unwrap();
    let mut rows = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let page = store
            .live_source_descriptors(
                class,
                tip,
                after.as_deref(),
                NonZeroUsize::new(64).unwrap(),
                &budget,
            )
            .unwrap();
        rows.extend(page.rows);
        match page.next {
            Some(next) => after = Some(next),
            None => return rows,
        }
    }
}

fn bytes_of(store: &KernelStore, row: &LiveDescriptor) -> String {
    String::from_utf8(
        store
            .read_artifact(&ArtifactHandle {
                digest: row.detail.artifact_digest.clone(),
                evidence_id: row.detail.evidence_id.clone(),
            })
            .unwrap(),
    )
    .unwrap()
}

fn identity_of(row: &LiveDescriptor) -> Vec<(&str, &str)> {
    row.detail
        .identity
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

/// AC1, AC2 (bytes), AC3: a user text block, a failed `bash` tool, and a completed `read` tool publish with their native identity and exact bytes; the read output admits as an untrusted tool result and stores `Sensitive`; a second pass publishes nothing new, and after the memo is lost the kernel answers from its receipts.
#[tokio::test]
async fn an_opencode_window_publishes_its_native_sources_once_and_replays_after_memo_loss() {
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    let window = [
        opencode_user("msg_u1", 1_700_000_000_000, "please run the tests"),
        opencode_assistant_with_tools("msg_a1", 1_700_000_001_000),
    ];
    pass(&handler, transform_with(&window, "base-1")).await;
    settle(&handler).await;
    let store = kernel_store(&handler);
    let digest = kernel_routes::ProjectBinding::new(&project)
        .digest()
        .to_owned();

    let messages = live(&store, OccurrenceClass::Messages);
    let user: Vec<&LiveDescriptor> = messages
        .iter()
        .filter(|row| identity_of(row)[3] == ("message_id", "msg_u1"))
        .collect();
    assert_eq!(user.len(), 1, "one descriptor per user text block");
    assert_eq!(
        identity_of(user[0]),
        vec![
            ("project_id", digest.as_str()),
            ("harness", "opencode"),
            ("session_id", SESSION),
            ("message_id", "msg_u1"),
            ("block_index", "0"),
        ]
    );
    assert_eq!(bytes_of(&store, user[0]), "please run the tests");

    let tools = live(&store, OccurrenceClass::RawToolSpans);
    let error = tools
        .iter()
        .find(|row| row.detail.representation == "tool_error")
        .expect("the failed bash call publishes its error");
    assert_eq!(identity_of(error)[4], ("tool_call_id", "call_bash"));
    assert_eq!(bytes_of(&store, error), "exit 2: cargo not found\n");
    let output = tools
        .iter()
        .find(|row| row.detail.representation == "tool_output")
        .expect("the read call publishes its output");
    assert_eq!(identity_of(output)[4], ("tool_call_id", "call_read"));
    assert_eq!(output.sensitivity, kernel::Sensitivity::Sensitive);
    let admission: (String, String) = rusqlite::Connection::open_with_flags(
        project
            .parent()
            .unwrap()
            .join("kernel")
            .join("kernel.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
    .query_row(
        "SELECT source_class,taint_class FROM admission_decisions WHERE subject_object_id=?1",
        [&output.object_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .unwrap();
    assert_eq!(
        admission,
        (
            SourceClass::TrustedToolResult.as_str().to_owned(),
            TaintClass::ToolUntrustedOutput.as_str().to_owned()
        )
    );
    let published = report(&handler)["units_published"].as_u64().unwrap();
    assert!(published >= 4, "{}", report(&handler));

    // The same window again: the memo answers and nothing new reaches the kernel.
    let tip = store.tip().unwrap();
    pass(&handler, transform_with(&window, "base-2")).await;
    settle(&handler).await;
    assert_eq!(store.tip().unwrap(), tip);
    assert!(report(&handler)["units_memo_skipped"].as_u64().unwrap() >= published);

    // A restart empties the memo; the kernel replays its receipts and publishes nothing new.
    handler.source_capture.forget_memo_for_test();
    pass(&handler, transform_with(&window, "base-3")).await;
    settle(&handler).await;
    assert_eq!(store.tip().unwrap(), tip);
    assert_eq!(
        report(&handler)["units_replayed"].as_u64().unwrap(),
        published
    );
}

/// AC9, gate half: with the key off, `memory_enabled` off, or a subagent session, no handoff is offered and nothing reaches the kernel.
#[tokio::test]
async fn capture_off_memory_off_or_a_subagent_session_runs_no_adapter() {
    let window = [opencode_user("msg_u1", 1_700_000_000_000, "hello")];
    for (label, config, subagent) in [
        ("key off", capture_config(false), false),
        (
            "memory off",
            DaemonConfig {
                memory_enabled: false,
                ..capture_config(true)
            },
            false,
        ),
        ("subagent", capture_config(true), true),
    ] {
        let (handler, _dir, _project) = capture_handler(config, true).await;
        let mut request = transform_with(&window, "base-1");
        request["is_subagent"] = json!(subagent);
        let tip = kernel_store(&handler).tip().unwrap();
        pass(&handler, request).await;
        settle(&handler).await;
        let report = report(&handler);
        for (name, value) in report.as_object().unwrap() {
            if name != "oldest_deferred_age_ms" {
                assert_eq!(value, 0, "{label}: {name}");
            }
        }
        assert_eq!(kernel_store(&handler).tip().unwrap(), tip, "{label}");
    }
}

/// AC7 and AC8 (last clause): the response is the same bytes with capture on and off, each pair served in one kernel state: ready, starting, writer held, and slot full (F22). A second pass during an outstanding batch is refused `queue_full` and retains nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_response_is_identical_whatever_capture_and_the_kernel_are_doing() {
    let window = [
        opencode_user("msg_u1", 1_700_000_000_000, "please run the tests"),
        opencode_assistant_with_tools("msg_a1", 1_700_000_001_000),
    ];
    let (baseline_handler, _baseline_dir, _) = capture_handler(capture_config(false), true).await;
    let baseline = pass(&baseline_handler, transform_with(&window, "base-1")).await;
    let second_baseline = pass(&baseline_handler, transform_with(&window, "base-2")).await;

    // Kernel ready.
    let (handler, _dir, _) = capture_handler(capture_config(true), true).await;
    assert_eq!(
        pass(&handler, transform_with(&window, "base-1")).await,
        baseline
    );
    settle(&handler).await;

    // Kernel starting: the pass returns the same bytes and the units stay pending.
    let (starting_off, _starting_off_dir, _) = capture_handler(capture_config(false), false).await;
    let starting_baseline = pass(&starting_off, transform_with(&window, "base-1")).await;
    let (starting, _starting_dir, _) = capture_handler(capture_config(true), false).await;
    assert_eq!(
        pass(&starting, transform_with(&window, "base-1")).await,
        starting_baseline
    );
    settle(&starting).await;
    assert_eq!(report(&starting)["kernel_unavailable"], 1);

    // Writer busy: another commit holds the writer; the response does not wait for it.
    let (busy, _busy_dir, _) = capture_handler(capture_config(true), true).await;
    let store = kernel_store(&busy);
    let (release, held) = mpsc::channel::<()>();
    let (started_tx, started_rx) = mpsc::channel::<()>();
    let holder = std::thread::spawn({
        let store = Arc::clone(&store);
        move || {
            store
                .commit(
                    kernel::CommitIntent {
                        producer: "source-capture-test".to_owned(),
                        operation_key: "hold-writer".to_owned(),
                        request_digest: "0".repeat(64),
                        actor: "test".to_owned(),
                        cause: "hold".to_owned(),
                    },
                    |_| {
                        started_tx.send(()).unwrap();
                        held.recv().unwrap();
                        Ok(String::new())
                    },
                )
                .unwrap();
        }
    });
    started_rx.recv().unwrap();
    let started = Instant::now();
    assert_eq!(
        pass(&busy, transform_with(&window, "base-1")).await,
        baseline
    );
    assert!(
        started.elapsed() < CAPTURE_TEST_RESPONSE_BOUND,
        "the response waited on the held writer"
    );
    release.send(()).unwrap();
    holder.join().unwrap();
    settle(&busy).await;

    // Slot full: the first batch is held inside publication; a second pass is refused and answers the same bytes.
    let (full, _full_dir, _) = capture_handler(capture_config(true), true).await;
    let (entered_tx, entered_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let go_rx = Arc::new(Mutex::new(go_rx));
    let entered_tx = Arc::new(Mutex::new(entered_tx));
    *full.source_capture.before_publish.lock().unwrap() = Some(Arc::new(move || {
        entered_tx.lock().unwrap().send(()).unwrap();
        go_rx.lock().unwrap().recv().unwrap();
    }));
    assert_eq!(
        pass(&full, transform_with(&window, "base-1")).await,
        baseline
    );
    tokio::task::spawn_blocking(move || entered_rx.recv().unwrap())
        .await
        .unwrap();
    let retained = report(&full)["retained_bytes"].as_u64().unwrap();
    assert_eq!(
        pass(&full, transform_with(&window, "base-2")).await,
        second_baseline
    );
    assert_eq!(report(&full)["refused_queue_full"], 1);
    assert_eq!(
        report(&full)["retained_bytes"].as_u64().unwrap(),
        retained,
        "the refused handoff retained nothing"
    );
    assert_eq!(report(&full)["active"], 1);
    go_tx.send(()).unwrap();
    settle(&full).await;
    assert_eq!(report(&full)["retained_bytes"], 0);
    assert_eq!(report(&full)["completed"], 1);
}

const CAPTURE_TEST_RESPONSE_BOUND: Duration = Duration::from_secs(3);

/// AC8, window form: a 33-unit window publishes 32 and reports one deferred; the next pass publishes the deferred unit first.
#[tokio::test]
async fn a_33_unit_window_publishes_32_and_the_next_pass_publishes_the_deferred_one() {
    let (handler, _dir, _) = capture_handler(capture_config(true), true).await;
    let window: Vec<Value> = (0..=MAX_SOURCE_BATCH_UNITS)
        .map(|index| {
            opencode_user(
                &format!("msg_{index:03}"),
                1_700_000_000_000 + index as u64,
                &format!("message {index}"),
            )
        })
        .collect();
    pass(&handler, transform_with(&window, "base-1")).await;
    settle(&handler).await;
    let store = kernel_store(&handler);
    assert_eq!(
        live(&store, OccurrenceClass::Messages).len(),
        MAX_SOURCE_BATCH_UNITS
    );
    assert_eq!(report(&handler)["units_deferred"], 1);
    assert!(report(&handler)["oldest_deferred_age_ms"].is_u64());

    pass(&handler, transform_with(&window, "base-2")).await;
    settle(&handler).await;
    assert_eq!(
        live(&store, OccurrenceClass::Messages).len(),
        MAX_SOURCE_BATCH_UNITS + 1
    );
    assert_eq!(
        report(&handler)["units_memo_skipped"].as_u64().unwrap(),
        MAX_SOURCE_BATCH_UNITS as u64
    );
    assert!(report(&handler)["oldest_deferred_age_ms"].is_null());
}

/// R1 liveness (F6): kernel `Starting` on pass N leaves the unit pending, and pass N+1 with the kernel ready publishes it.
#[tokio::test]
async fn a_unit_pending_behind_a_starting_kernel_publishes_on_the_next_ready_pass() {
    let (handler, dir, _) = capture_handler(capture_config(true), false).await;
    let window = [opencode_user("msg_u1", 1_700_000_000_000, "first")];
    pass(&handler, transform_with(&window, "base-1")).await;
    settle(&handler).await;
    assert_eq!(report(&handler)["kernel_unavailable"], 1);
    handler
        .kernel
        .open(
            dir.path().join("kernel"),
            StoreOpenPolicy::default(),
            handler.cancel.clone(),
        )
        .await;
    pass(&handler, transform_with(&window, "base-2")).await;
    settle(&handler).await;
    assert_eq!(
        live(&kernel_store(&handler), OccurrenceClass::Messages).len(),
        1
    );
}

/// D2 and OQ5: an OpenCode user message edited in place keeps its identity and revision, so its changed bytes conflict, become terminal, and are never offered to the kernel again; the original bytes stay published.
#[tokio::test]
async fn changed_bytes_under_one_identity_are_terminal_and_never_retried() {
    let (handler, _dir, _) = capture_handler(capture_config(true), true).await;
    pass(
        &handler,
        transform_with(
            &[opencode_user("msg_u1", 1_700_000_000_000, "original")],
            "base-1",
        ),
    )
    .await;
    settle(&handler).await;
    let edited = [opencode_user(
        "msg_u1",
        1_700_000_000_000,
        "edited in place",
    )];
    pass(&handler, transform_with(&edited, "base-2")).await;
    settle(&handler).await;
    assert_eq!(report(&handler)["units_identity_conflict"], 1);
    let store = kernel_store(&handler);
    let tip = store.tip().unwrap();
    pass(&handler, transform_with(&edited, "base-3")).await;
    settle(&handler).await;
    assert_eq!(report(&handler)["units_identity_conflict"], 1);
    assert_eq!(store.tip().unwrap(), tip);
    let rows = live(&store, OccurrenceClass::Messages);
    assert_eq!(rows.len(), 1);
    assert_eq!(bytes_of(&store, &rows[0]), "original");
}

/// C4: shutdown with a batch in flight accepts no new handoff, lets the batch finish, and releases the slot and the charge; a job that panics releases them too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_drains_an_in_flight_batch_and_a_panicking_job_releases_its_slot() {
    let window = [opencode_user("msg_u1", 1_700_000_000_000, "drained")];

    let (panicking, _panic_dir, _) = capture_handler(capture_config(true), true).await;
    *panicking.source_capture.before_publish.lock().unwrap() =
        Some(Arc::new(|| panic!("injected worker panic")));
    pass(&panicking, transform_with(&window, "base-1")).await;
    settle(&panicking).await;
    assert_eq!(report(&panicking)["worker_panicked"], 1);
    assert_eq!(report(&panicking)["retained_bytes"], 0);
    assert_eq!(report(&panicking)["active"], 0);
    *panicking.source_capture.before_publish.lock().unwrap() = None;
    pass(&panicking, transform_with(&window, "base-2")).await;
    settle(&panicking).await;
    assert_eq!(
        live(&kernel_store(&panicking), OccurrenceClass::Messages).len(),
        1,
        "the slot is free again after the panic"
    );

    let (handler, _dir, _) = capture_handler(capture_config(true), true).await;
    let (entered_tx, entered_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let go_rx = Arc::new(Mutex::new(go_rx));
    let entered_tx = Arc::new(Mutex::new(entered_tx));
    *handler.source_capture.before_publish.lock().unwrap() = Some(Arc::new(move || {
        entered_tx.lock().unwrap().send(()).unwrap();
        go_rx.lock().unwrap().recv().unwrap();
    }));
    pass(&handler, transform_with(&window, "base-1")).await;
    tokio::task::spawn_blocking(move || entered_rx.recv().unwrap())
        .await
        .unwrap();
    let store = kernel_store(&handler);
    let handler = Arc::new(handler);
    let shutdown = tokio::spawn({
        let handler = Arc::clone(&handler);
        async move { handler.shutdown().await }
    });
    while !handler.cancel.is_cancelled() {
        tokio::task::yield_now().await;
    }
    // A later window is charged, but its job is refused once admission has closed.
    let later = [opencode_user("msg_u2", 1_700_000_000_500, "too late")];
    let request = handler.test_client_prepare_request(transform_with(&later, "base-2"));
    let mut outcome = handler
        .handle_transform_for_test(test_route(7), request)
        .await;
    let after = outcome
        .take_after_response()
        .expect("the later window is charged");
    assert!(
        !shutdown.is_finished(),
        "shutdown waits for the in-flight batch"
    );
    go_tx.send(()).unwrap();
    shutdown.await.unwrap().unwrap();
    assert!(handler.source_capture.is_idle());
    after.run();
    assert_eq!(report(&handler)["refused_shutting_down"], 1);
    assert_eq!(
        report(&handler)["discarded"],
        1,
        "the in-flight batch stopped cooperatively"
    );
    assert!(handler.source_capture.is_idle());
    assert_eq!(report(&handler)["retained_bytes"], 0);
    assert_eq!(report(&handler)["admitted"], 1);
    assert!(
        live(&store, OccurrenceClass::Messages)
            .iter()
            .all(|row| identity_of(row)[3] != ("message_id", "msg_u2"))
    );
}

/// AC6 (AE6): after publication and a kernel reopen, no captured unit is visible to AutoInject and no memory decision was created.
#[tokio::test]
async fn captured_units_create_no_decision_and_stay_hidden_from_auto_inject_after_reopen() {
    let (handler, dir, _) = capture_handler(capture_config(true), true).await;
    let store = kernel_store(&handler);
    let decisions_before = store
        .slice_as_of(store.tip().unwrap())
        .unwrap()
        .decisions
        .len();
    let window = [
        opencode_user("msg_u1", 1_700_000_000_000, "remember this fact"),
        opencode_assistant_with_tools("msg_a1", 1_700_000_001_000),
    ];
    pass(&handler, transform_with(&window, "base-1")).await;
    settle(&handler).await;
    let captured: Vec<String> = [OccurrenceClass::Messages, OccurrenceClass::RawToolSpans]
        .into_iter()
        .flat_map(|class| live(&store, class))
        .map(|row| row.object_id)
        .collect();
    assert!(!captured.is_empty());
    drop(store);
    drop(handler);
    let reopened = KernelStore::open(dir.path().join("kernel")).unwrap();
    let tip = reopened.tip().unwrap();
    let visible = reopened
        .visible_as_of_in_scope(Surface::AutoInject, tip, Some(&captured), None)
        .unwrap();
    assert!(
        visible.rows.is_empty(),
        "a captured unit is visible to AutoInject"
    );
    assert_eq!(
        reopened.slice_as_of(tip).unwrap().decisions.len(),
        decisions_before,
        "capture created a memory decision"
    );
}

/// AC18: no counter or health field carries unit text, and every refusal path leaves only codes and counts.
#[tokio::test]
async fn the_capture_report_carries_no_unit_text() {
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    for (base, text) in [
        ("base-1", SENTINEL.to_owned()),
        ("base-2", format!("{SENTINEL} changed")),
    ] {
        let window = [opencode_user("msg_sentinel_u1", 1_700_000_000_000, &text)];
        pass(&handler, transform_with(&window, base)).await;
        settle(&handler).await;
    }
    assert_eq!(report(&handler)["units_identity_conflict"], 1);
    // Adapter and window-too-large refusals report through the same counters.
    let binding = kernel_routes::ProjectBinding::new(&project);
    let offer = |window: &[Arc<Value>]| {
        handler.source_capture.offer(source_capture::Offer {
            profile: SerializerProfile::OpencodeAiSdk,
            session_id: SESSION.to_owned(),
            project: binding.clone(),
            window: window.to_vec(),
            epoch: handler.source_capture.epoch(),
        })
    };
    let foreign = vec![Arc::new(json!({
        "info": { "id": "msg_sentinel_u2", "sessionID": SENTINEL, "role": "user",
                  "time": { "created": 1_700_000_000_001u64 } },
        "parts": [{ "type": "text", "text": SENTINEL }],
    }))];
    let job = offer(&foreign)
        .unwrap()
        .into_job(
            Arc::clone(&handler.kernel),
            Arc::clone(&handler.capture_commit_gate),
            handler.cancel.clone(),
        )
        .unwrap();
    assert_eq!(job.run().await, source_capture::Count::Completed);
    assert_eq!(
        report(&handler)["adapter_refused"],
        1,
        "the record names another session"
    );
    let huge = vec![Arc::new(json!(SENTINEL.repeat(2 * 1024 * 1024)))];
    assert!(offer(&huge).is_none());
    assert_eq!(report(&handler)["refused_window_too_large"], 1);
    let health = handler.health().await;
    let text = format!("{} {:?}", report(&handler), health.metrics);
    assert!(!text.contains(SENTINEL), "{text}");
    assert!(!text.contains("msg_sentinel"), "{text}");
    assert!(!text.contains(binding.digest()), "{text}");
}

fn pi_row(id: &str, parent: Option<&str>, message: Value) -> Value {
    json!({ "type": "message", "id": id, "parentId": parent, "message": message })
}

/// A Pi window whose rows carry the native entry type and parent publishes its user text and each tool result text item.
#[tokio::test]
async fn a_pi_window_publishes_user_text_and_tool_results() {
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    let mut bound = binding_with_harness(project.to_str().unwrap(), "pi", SESSION);
    bound.config = capture_config(true);
    handler.bind_route(test_route(7), bound);
    let rows = vec![
        pi_row(
            "pi-u1",
            None,
            json!({ "role": "user", "content": "list the files", "timestamp": 1_700_000_000_000u64 }),
        ),
        pi_row(
            "pi-a1",
            Some("pi-u1"),
            json!({ "role": "assistant", "timestamp": 1_700_000_000_100u64, "stopReason": "toolUse",
                    "usage": {}, "content": [
                        { "type": "text", "text": "Listing." },
                        { "type": "toolCall", "id": "call_ls", "name": "bash", "arguments": { "command": "ls" } }
                    ] }),
        ),
        pi_row(
            "pi-t1",
            Some("pi-a1"),
            json!({ "role": "toolResult", "toolCallId": "call_ls", "toolName": "bash", "isError": false,
                    "timestamp": 1_700_000_000_200u64,
                    "content": [ { "type": "text", "text": "Cargo.toml\nsrc\n" } ] }),
        ),
    ];
    let decoded =
        codec::pi::decode_pi_rows(&rows.iter().cloned().map(Arc::new).collect::<Vec<_>>()).unwrap();
    let messages: Vec<IngressMessage> = decoded.messages;
    let mut request = request_with_usage(messages, 1_000, 200_000);
    request["serializer_profile"] = json!("pi");
    request["native_messages"] = json!(rows);
    request["base_revision"] = json!("base-1");
    pass(&handler, request).await;
    settle(&handler).await;
    let store = kernel_store(&handler);
    let messages = live(&store, OccurrenceClass::Messages);
    assert!(
        messages
            .iter()
            .any(|row| bytes_of(&store, row) == "list the files")
    );
    let tools = live(&store, OccurrenceClass::RawToolSpans);
    assert_eq!(tools.len(), 1);
    assert_eq!(bytes_of(&store, &tools[0]), "Cargo.toml\nsrc\n");
    assert_eq!(identity_of(&tools[0])[3], ("parent_message_id", "pi-a1"));
}

fn pi_request(rows: &[Value], base: &str) -> Value {
    let decoded =
        codec::pi::decode_pi_rows(&rows.iter().cloned().map(Arc::new).collect::<Vec<_>>()).unwrap();
    let mut request = request_with_usage(decoded.messages, 1_000, 200_000);
    request["serializer_profile"] = json!("pi");
    request["native_messages"] = json!(rows);
    request["base_revision"] = json!(base);
    request
}

/// OQ19 measurement, run by hand: `cargo nextest run -p daemon --all-features --run-ignored only -E 'test(capture_latency)' --no-capture`. For an OpenCode and a Pi workload, interleaves capture-disabled and capture-enabled passes over one window and reports response medians, p95, spread, and the capture batch's writer hold.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "measurement"]
async fn capture_latency_measurement() {
    const RUNS: usize = 10;
    let opencode: Vec<Value> = (0..24u64)
        .flat_map(|index| {
            let mut assistant = opencode_assistant_with_tools(
                &format!("msg_a{index:03}"),
                1_700_000_001_000 + index * 2_000,
            );
            for part in assistant["parts"].as_array_mut().unwrap() {
                if let Some(call) = part["callID"].as_str().map(str::to_owned) {
                    part["callID"] = json!(format!("{call}_{index}"));
                }
            }
            [
                opencode_user(
                    &format!("msg_u{index:03}"),
                    1_700_000_000_000 + index * 2_000,
                    &"user text ".repeat(40),
                ),
                assistant,
            ]
        })
        .collect();
    let pi: Vec<Value> = (0..24u64)
        .flat_map(|index| {
            let stamp = 1_700_000_000_000 + index * 2_000;
            let (user, assistant, result) = (
                format!("pi-u{index}"),
                format!("pi-a{index}"),
                format!("pi-t{index}"),
            );
            let call = format!("call_{index}");
            [
                pi_row(
                    &user,
                    None,
                    json!({ "role": "user", "content": "user text ".repeat(40), "timestamp": stamp }),
                ),
                pi_row(
                    &assistant,
                    Some(&user),
                    json!({ "role": "assistant", "timestamp": stamp + 100, "stopReason": "toolUse",
                            "usage": {}, "content": [
                                { "type": "text", "text": "Listing." },
                                { "type": "toolCall", "id": call, "name": "bash", "arguments": {} }
                            ] }),
                ),
                pi_row(
                    &result,
                    Some(&assistant),
                    json!({ "role": "toolResult", "toolCallId": call, "toolName": "bash", "isError": false,
                            "timestamp": stamp + 200,
                            "content": [ { "type": "text", "text": "output line\n".repeat(20) } ] }),
                ),
            ]
        })
        .collect();
    for workload in ["opencode", "pi"] {
        let (off, _off_dir, project_off) = capture_handler(capture_config(false), true).await;
        let (on, _on_dir, project_on) = capture_handler(capture_config(true), true).await;
        if workload == "pi" {
            for (handler, project, enabled) in
                [(&off, &project_off, false), (&on, &project_on, true)]
            {
                let mut bound = binding_with_harness(project.to_str().unwrap(), "pi", SESSION);
                bound.config = capture_config(enabled);
                handler.bind_route(test_route(7), bound);
            }
        }
        let mut samples: [Vec<f64>; 2] = [Vec::new(), Vec::new()];
        for run in 0..RUNS {
            for (arm, handler) in [(0, &off), (1, &on)] {
                let base = format!("base-{run}");
                let request = if workload == "pi" {
                    pi_request(&pi, &base)
                } else {
                    transform_with(&opencode, &base)
                };
                let started = Instant::now();
                pass(handler, request).await;
                samples[arm].push(started.elapsed().as_secs_f64() * 1_000.0);
                settle(handler).await;
            }
        }
        for (arm, label) in [(0, "capture off"), (1, "capture on")] {
            let mut values = samples[arm].clone();
            values.sort_by(f64::total_cmp);
            let median = values[values.len() / 2];
            let p95 = values[(values.len() * 95).div_ceil(100) - 1];
            eprintln!(
                "capture latency {workload} {label}: median {median:.2} ms, p95 {p95:.2} ms, min {:.2} ms, max {:.2} ms, runs {}",
                values[0],
                values[values.len() - 1],
                values.len()
            );
        }
        eprintln!(
            "capture writer hold {workload}: last {} max {} published {}",
            report(&on)["publish_ms_last"],
            report(&on)["publish_ms_max"],
            report(&on)["units_published"]
        );
    }
}

/// C5: a delete that lands after the handoff invalidates the job under the commit gate; it publishes nothing, and the next pass captures the window again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delete_after_the_handoff_publishes_nothing_for_the_queued_window() {
    let (handler, _dir, _) = capture_handler(capture_config(true), true).await;
    let (entered_tx, entered_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let go_rx = Arc::new(Mutex::new(go_rx));
    let entered_tx = Arc::new(Mutex::new(entered_tx));
    *handler.source_capture.before_publish.lock().unwrap() = Some(Arc::new(move || {
        entered_tx.lock().unwrap().send(()).unwrap();
        go_rx.lock().unwrap().recv().unwrap();
    }));
    let window = [opencode_user(
        "msg_u1",
        1_700_000_000_000,
        "deleted before publication",
    )];
    pass(&handler, transform_with(&window, "base-1")).await;
    tokio::task::spawn_blocking(move || entered_rx.recv().unwrap())
        .await
        .unwrap();
    {
        let _gate = handler.capture_commit_gate.lock().unwrap();
        handler.source_capture.invalidate_sessions();
    }
    go_tx.send(()).unwrap();
    settle(&handler).await;
    assert_eq!(report(&handler)["session_invalidated"], 1);
    let store = kernel_store(&handler);
    assert!(live(&store, OccurrenceClass::Messages).is_empty());
    *handler.source_capture.before_publish.lock().unwrap() = None;
    pass(&handler, transform_with(&window, "base-2")).await;
    settle(&handler).await;
    assert_eq!(live(&store, OccurrenceClass::Messages).len(), 1);
}

/// C3: a budget spent before the kernel writer is acquired commits nothing; the response was already returned, and the slot and charges are released.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_spent_budget_before_the_writer_commits_nothing() {
    let (handler, _dir, _) = capture_handler(capture_config(true), true).await;
    *handler.source_capture.before_publish.lock().unwrap() = Some(Arc::new(|| {
        std::thread::sleep(source_capture::CAPTURE_BUDGET + Duration::from_millis(100))
    }));
    let store = kernel_store(&handler);
    let tip = store.tip().unwrap();
    let window = [opencode_user("msg_u1", 1_700_000_000_000, "too slow")];
    let started = Instant::now();
    pass(&handler, transform_with(&window, "base-1")).await;
    assert!(started.elapsed() < CAPTURE_TEST_RESPONSE_BOUND);
    for _ in 0..1_000 {
        if handler.source_capture.is_idle() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(report(&handler)["deadline"], 1);
    assert_eq!(store.tip().unwrap(), tip);
    assert_eq!(report(&handler)["retained_bytes"], 0);
}

/// F5: another writer publishes a unit between the handoff and this batch. Equal bytes and context replay; another sensitivity conflicts, and that unit becomes terminal.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_concurrent_publication_of_a_unit_replays_or_conflicts() {
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    pass(
        &handler,
        transform_with(
            &[opencode_user("msg_seed", 1_700_000_000_000, "seed")],
            "base-0",
        ),
    )
    .await;
    settle(&handler).await;
    let store = kernel_store(&handler);
    let binding = kernel_routes::ProjectBinding::new(&project);
    let session = crate::harness_sources::SessionIdentity {
        project_id: binding.digest().to_owned(),
        harness: crate::harness_sources::Harness::OpenCode,
        session_id: SESSION.to_owned(),
    };
    for (base, sensitivity, message, outcome) in [
        (
            "base-1",
            kernel::Sensitivity::Sensitive,
            "msg_same",
            "units_replayed",
        ),
        (
            "base-2",
            kernel::Sensitivity::Normal,
            "msg_other",
            "units_identity_conflict",
        ),
    ] {
        let record = opencode_user(message, 1_700_000_000_100, "raced");
        let unit = crate::harness_sources::opencode_units(&session, &record)
            .unwrap()
            .remove(0);
        let (store, scope_id) = (Arc::clone(&store), binding.scope_id());
        *handler.source_capture.before_publish.lock().unwrap() = Some(Arc::new(move || {
            crate::harness_sources::SourcePublisher {
                kernel: &store,
                domain_id: "harness-capture",
                scope_id: Some(&scope_id),
                egress: kernel::ProviderEgress::LocalOnly,
                sensitivity,
            }
            .publish(&unit, now_ms())
            .unwrap();
        }));
        pass(&handler, transform_with(&[record], base)).await;
        settle(&handler).await;
        assert_eq!(report(&handler)[outcome], 1, "{outcome}");
        *handler.source_capture.before_publish.lock().unwrap() = None;
    }
    let tip = store.tip().unwrap();
    pass(
        &handler,
        transform_with(
            &[opencode_user("msg_other", 1_700_000_000_100, "raced")],
            "base-3",
        ),
    )
    .await;
    settle(&handler).await;
    assert_eq!(
        store.tip().unwrap(),
        tip,
        "the terminal unit is never offered again"
    );
    assert_eq!(report(&handler)["units_identity_conflict"], 1);
    assert_eq!(
        report(&handler)["units_memo_skipped"],
        1,
        "the memo, not a second kernel conflict, answers for the terminal unit"
    );
}

/// F25: a revision that leads the clock is a retryable refusal; the unit waits as pending, and the report shows its age.
#[tokio::test]
async fn a_revision_lead_leaves_the_unit_pending() {
    let (handler, _dir, _) = capture_handler(capture_config(true), true).await;
    let ahead = now_ms() as u64 + 2 * 60 * 60 * 1_000;
    pass(
        &handler,
        transform_with(
            &[opencode_user("msg_ahead", ahead, "from the future")],
            "base-1",
        ),
    )
    .await;
    settle(&handler).await;
    assert_eq!(report(&handler)["units_refused_retryable"], 1);
    assert!(report(&handler)["oldest_deferred_age_ms"].is_u64());
    // A pending unit is offered to the kernel again on the next pass.
    pass(
        &handler,
        transform_with(
            &[opencode_user("msg_ahead", ahead, "from the future")],
            "base-2",
        ),
    )
    .await;
    settle(&handler).await;
    assert_eq!(report(&handler)["units_refused_retryable"], 2);
    assert_eq!(report(&handler)["units_memo_skipped"], 0);
    assert!(live(&kernel_store(&handler), OccurrenceClass::Messages).is_empty());
}

/// C2: the supervisor's own budget refuses a second window while the first holds most of it, and retains nothing for the refusal.
#[tokio::test]
async fn a_window_past_the_remaining_budget_is_refused_resident_short() {
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    let binding = kernel_routes::ProjectBinding::new(&project);
    let window = vec![Arc::new(json!("x".repeat(9 * 1024 * 1024)))];
    let offer = || {
        handler.source_capture.offer(source_capture::Offer {
            profile: SerializerProfile::OpencodeAiSdk,
            session_id: SESSION.to_owned(),
            project: binding.clone(),
            window: window.clone(),
            epoch: handler.source_capture.epoch(),
        })
    };
    let held = offer().expect("the first window fits");
    let retained = report(&handler)["retained_bytes"].as_u64().unwrap();
    assert!(offer().is_none());
    assert_eq!(report(&handler)["refused_resident_short"], 1);
    assert_eq!(report(&handler)["retained_bytes"], retained);
    drop(held);
    assert_eq!(report(&handler)["retained_bytes"], 0);
}

/// AC10 (U3): Pi rows that carry their entry's `type` and `parentId` publish their tool results, rows without them are counted adapter refusals, and the wire response is the same bytes either way.
#[tokio::test]
async fn pi_entry_identity_on_the_row_decides_capture_and_never_the_response() {
    let rows = |identity: bool| -> Vec<Value> {
        let mut rows = vec![
            pi_row(
                "pi-u1",
                None,
                json!({ "role": "user", "content": "list the files", "timestamp": 1_700_000_000_000u64 }),
            ),
            pi_row(
                "pi-a1",
                Some("pi-u1"),
                json!({ "role": "assistant", "timestamp": 1_700_000_000_100u64, "stopReason": "toolUse",
                        "usage": {}, "content": [
                            { "type": "toolCall", "id": "call_ls", "name": "bash", "arguments": {} }
                        ] }),
            ),
            pi_row(
                "pi-t1",
                Some("pi-a1"),
                json!({ "role": "toolResult", "toolCallId": "call_ls", "toolName": "bash", "isError": false,
                        "timestamp": 1_700_000_000_200u64,
                        "content": [ { "type": "text", "text": "Cargo.toml\n" }, { "type": "text", "text": "src\n" } ] }),
            ),
        ];
        if !identity {
            for row in &mut rows {
                let row = row.as_object_mut().unwrap();
                row.remove("type");
                row.remove("parentId");
            }
        }
        rows
    };
    let mut responses = Vec::new();
    for identity in [true, false] {
        let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
        let mut bound = binding_with_harness(project.to_str().unwrap(), "pi", SESSION);
        bound.config = capture_config(true);
        handler.bind_route(test_route(7), bound);
        // The wire response: the recipe the daemon returns, before a client applies it.
        let request = handler.test_client_prepare_request(pi_request(&rows(identity), "base-1"));
        let mut outcome = handler
            .handle_transform_for_test(test_route(7), request)
            .await;
        let after = outcome.take_after_response();
        let PreparedOutcome::Response(bytes) = outcome else {
            panic!("unexpected outcome {outcome:?}");
        };
        let mut response: Value = serde_json::from_slice(&bytes).unwrap();
        for field in ["timings", "output_revision", "previous_output_revision"] {
            response.as_object_mut().unwrap().remove(field);
        }
        responses.push(response);
        after.expect("the window is charged").run();
        settle(&handler).await;
        let tools = live(&kernel_store(&handler), OccurrenceClass::RawToolSpans);
        if identity {
            assert_eq!(tools.len(), 2, "one unit per tool result text item");
            assert_eq!(report(&handler)["adapter_refused"], 0);
        } else {
            assert!(tools.is_empty());
            assert_eq!(report(&handler)["adapter_refused"], 3);
        }
    }
    assert_eq!(responses[0], responses[1]);
}

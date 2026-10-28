//! Source capture through the transform handler against a real memory store and a real kernel store: consent, the background slot, dispositions, response independence, restart replay, shutdown, and disclosure.

use std::num::NonZeroUsize;
use std::sync::mpsc;

use kernel::source_identity::OccurrenceClass;
use kernel::{ArtifactHandle, KernelStore, LiveDescriptor, SourceClass, Surface, TaintClass};

use super::*;
use crate::harness_sources::MAX_SOURCE_BATCH_UNITS;
use crate::source_capture::{Count, Settlement};

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
    pass_on(handler, test_route(7), request).await
}

async fn pass_on(handler: &Handler, route: RouteHandle, request: Value) -> Value {
    let request = handler.test_client_prepare_request(request);
    let mut outcome = handler
        .handle_transform_for_test(route, request.clone())
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

/// The Kernel tip minus the use-outcome serves the handler wrote: each written serve is one commit, and a
/// document pass that folds HARD over a new served set writes one beside whatever capture commits.
fn capture_tip(handler: &Handler) -> i64 {
    let serves = handler
        .use_outcome_writes
        .get(crate::use_outcome_writer::UseOutcomeWrite::Written);
    kernel_store(handler).tip().unwrap() - i64::try_from(serves).unwrap()
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

/// AC1, AC2 (bytes and outcome), AC3 (with the outcome's tool name): a user text block, a failed `bash` tool, and a completed `read` tool publish with their native identity and exact bytes; the read output admits as an untrusted tool result and stores `Sensitive`; a second pass publishes nothing new, and after the memo is lost the kernel answers from its receipts.
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
    // AC2: the failed call's record says so; the fixture's error part reports no exit code.
    let record = error
        .detail
        .outcome
        .as_ref()
        .expect("a tool descriptor stores its record");
    assert_eq!(
        (record.status, record.tool_name.as_str(), record.exit_code),
        (kernel::ToolStatus::Error, "bash", None)
    );
    let output = tools
        .iter()
        .find(|row| row.detail.representation == "tool_output")
        .expect("the read call publishes its output");
    assert_eq!(identity_of(output)[4], ("tool_call_id", "call_read"));
    // AC3: the read call's record names its tool and its byte length.
    let record = output
        .detail
        .outcome
        .as_ref()
        .expect("a tool descriptor stores its record");
    assert_eq!(
        (record.status, record.tool_name.as_str(), record.byte_length),
        (kernel::ToolStatus::Completed, "read", 13)
    );
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

async fn capture_job(
    handler: &Handler,
    project: &Path,
    session_id: &str,
    window: &[Value],
) -> source_capture::Count {
    handler
        .source_capture
        .offer(source_capture::Offer {
            profile: SerializerProfile::OpencodeAiSdk,
            session_id: session_id.to_owned(),
            project: kernel_routes::ProjectBinding::new(project),
            window: window.iter().cloned().map(Arc::new).collect(),
            epoch: handler.source_capture.epoch(),
            documents: None,
            candidates: false,
        })
        .unwrap()
        .into_job(
            Arc::clone(&handler.kernel),
            Arc::clone(&handler.capture_commit_gate),
            handler.cancel.clone(),
        )
        .unwrap()
        .run()
        .await
}

/// The session identity scopes memo entries, so equal message ids and text in two sessions publish independently and each session's next pass hits its own memo entry.
#[tokio::test]
async fn equal_native_ids_in_two_sessions_publish_and_memoize_apart() {
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    let window = |session: &str| {
        [json!({
            "info": { "id": "msg_u1", "sessionID": session, "role": "user",
                      "time": { "created": 1_700_000_000_000u64 } },
            "parts": [{ "type": "text", "text": "same id, same text" }],
        })]
    };
    for _ in 0..2 {
        for session in ["ses_a", "ses_b"] {
            let end = capture_job(&handler, &project, session, &window(session)).await;
            assert_eq!(end, source_capture::Count::Completed);
        }
    }
    assert_eq!(report(&handler)["units_published"], 2);
    assert_eq!(report(&handler)["units_memo_skipped"], 2);
    assert_eq!(report(&handler)["units_replayed"], 0);
    let sessions: std::collections::BTreeSet<String> =
        live(&kernel_store(&handler), OccurrenceClass::Messages)
            .iter()
            .map(|row| identity_of(row)[2].1.to_owned())
            .collect();
    assert_eq!(sessions, ["ses_a".to_owned(), "ses_b".to_owned()].into());
}

/// A message id containing the memo key separator U+001F receives a terminal refusal, preserving the encodable sibling's memo entry: the sibling publishes once and is a memo hit on the next pass.
#[tokio::test]
async fn an_identity_that_does_not_encode_never_shadows_a_memo_entry() {
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    let window = [
        opencode_user("msg_ok", 1_700_000_000_000, "encodable"),
        opencode_user("msg_ok\u{1f}0", 1_700_000_000_000, "encodable"),
    ];
    for _ in 0..2 {
        let end = capture_job(&handler, &project, SESSION, &window).await;
        assert_eq!(end, source_capture::Count::Completed);
    }
    assert_eq!(report(&handler)["units_published"], 1);
    assert_eq!(report(&handler)["units_memo_skipped"], 1);
    assert!(report(&handler)["units_refused_terminal"].as_u64().unwrap() >= 1);
    let rows = live(&kernel_store(&handler), OccurrenceClass::Messages);
    assert_eq!(rows.len(), 1);
    assert_eq!(identity_of(&rows[0])[3], ("message_id", "msg_ok"));
}

#[tokio::test]
async fn a_secret_bearing_unit_is_terminal_and_its_siblings_publish() {
    const SECRET: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678";
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    let window = [
        opencode_user("msg_secret", 1_700_000_000_000, &format!("token {SECRET}")),
        opencode_user("msg_clean", 1_700_000_000_001, "a clean sibling"),
    ];
    let end = capture_job(&handler, &project, SESSION, &window).await;
    assert_eq!(
        end,
        source_capture::Count::Completed,
        "{}",
        report(&handler)
    );
    assert_eq!(report(&handler)["units_published"], 1);
    assert_eq!(report(&handler)["units_refused_terminal"], 1);
    assert_eq!(report(&handler)["kernel_error"], 0);

    let store = kernel_store(&handler);
    let tip = store.tip().unwrap();
    let end = capture_job(&handler, &project, SESSION, &window).await;
    assert_eq!(end, source_capture::Count::Completed);
    assert_eq!(report(&handler)["units_memo_skipped"], 2);
    assert_eq!(store.tip().unwrap(), tip);
    let rows = live(&store, OccurrenceClass::Messages);
    assert_eq!(rows.len(), 1);
    assert_eq!(bytes_of(&store, &rows[0]), "a clean sibling");
}

#[tokio::test]
async fn a_changed_repeat_in_one_window_counts_one_identity_conflict() {
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    let window = [
        opencode_user("msg_twice", 1_700_000_000_000, "first bytes"),
        opencode_user("msg_twice", 1_700_000_000_000, "first bytes"),
        opencode_user("msg_twice", 1_700_000_000_000, "second bytes"),
    ];
    for _ in 0..2 {
        let end = capture_job(&handler, &project, SESSION, &window).await;
        assert_eq!(end, source_capture::Count::Completed);
    }
    assert_eq!(report(&handler)["units_published"], 1);
    assert_eq!(report(&handler)["units_identity_conflict"], 1);
    assert_eq!(report(&handler)["units_memo_skipped"], 1);
    let store = kernel_store(&handler);
    let rows = live(&store, OccurrenceClass::Messages);
    assert_eq!(rows.len(), 1);
    assert_eq!(bytes_of(&store, &rows[0]), "first bytes");
}

/// The window passes the offer's charge, but its units repeat a 1 MiB message id: their copies exceed the budget, so the job stops adapting, publishes nothing, and releases every charge.
#[tokio::test]
async fn unit_copies_past_the_budget_end_resident_short_and_publish_nothing() {
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    let parts: Vec<Value> = (0..64)
        .map(|_| json!({ "type": "text", "text": "" }))
        .collect();
    let window = [json!({
        "info": { "id": "m".repeat(1 << 20), "sessionID": SESSION, "role": "user",
                  "time": { "created": 1_700_000_000_000u64 } },
        "parts": parts,
    })];
    let tip = kernel_store(&handler).tip().unwrap();
    let end = capture_job(&handler, &project, SESSION, &window).await;
    assert_eq!(end, source_capture::Count::RefusedResidentShort);
    assert_eq!(report(&handler)["retained_bytes"], 0);
    assert_eq!(kernel_store(&handler).tip().unwrap(), tip);
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
            documents: None,
            candidates: false,
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

#[tokio::test]
async fn the_capture_report_reaches_host_status_field_for_field() {
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    let window = [opencode_user("msg_u1", 1_700_000_000_000, "hello")];
    assert_eq!(
        capture_job(&handler, &project, SESSION, &window).await,
        source_capture::Count::Completed
    );
    let metrics = handler.health().await.metrics.expect("context metrics");
    let composite = host_runtime::HealthReport {
        status: host_runtime::HealthStatus::Ok,
        detail: None,
        metrics: Some(json!({
            "components": { "context": { "status": "ok", "metrics": metrics.clone() } }
        })),
    };
    let status: Value = serde_json::from_slice(&host_runtime::host_status_response_json(
        &composite,
        json!({ "state": "healthy" }),
    ))
    .unwrap();
    let forwarded = &status["metrics"]["components"]["context"]["metrics"]["source_capture"];
    assert_eq!(forwarded, &metrics["source_capture"]);
    assert_eq!(forwarded["units_published"], 1);
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

/// A budget spent before the kernel writer is acquired drops the pass's document reads with its window: both configured reads, the text and the missing file, count as `documents_refused`, and nothing of the document reaches the kernel.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_spent_budget_counts_the_document_reads_it_drops() {
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    std::fs::write(project.join("ARCHITECTURE.md"), "# Architecture\n").unwrap();
    *handler.source_capture.before_publish.lock().unwrap() = Some(Arc::new(|| {
        std::thread::sleep(source_capture::CAPTURE_BUDGET + Duration::from_millis(100))
    }));
    let store = kernel_store(&handler);
    let tip = store.tip().unwrap();
    document_pass(&handler, "base-1").await;
    for _ in 0..1_000 {
        if handler.source_capture.is_idle() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(report(&handler)["deadline"], 1);
    assert_eq!(
        report(&handler)["documents_refused"],
        2,
        "{}",
        report(&handler)
    );
    assert_eq!(store.tip().unwrap(), tip);
    assert!(live_document(&store, "ARCHITECTURE.md").is_empty());
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
            documents: None,
            candidates: false,
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

/// AC10 (U3): Pi rows that carry their entry's `type` and `parentId` publish their tool results, rows without them are counted adapter refusals, and the wire response is the same bytes either way; `codec::pi::tests::re_rendered_rows_carry_no_entry_identity` pins the rows the daemon re-renders.
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

/// C2: the memo keys and digests a job builds are copies the budget charges. A window whose adapter copies fit the remaining budget but whose keys do not ends resident short and publishes nothing.
#[tokio::test]
async fn identity_keys_past_the_remaining_budget_end_resident_short() {
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    let binding = kernel_routes::ProjectBinding::new(&project);
    let filler = vec![Arc::new(json!("x".repeat(9 * 1024 * 1024)))];
    let held = handler
        .source_capture
        .offer(source_capture::Offer {
            profile: SerializerProfile::OpencodeAiSdk,
            session_id: SESSION.to_owned(),
            project: binding,
            window: filler,
            epoch: handler.source_capture.epoch(),
            documents: None,
            candidates: false,
        })
        .expect("the filler fits");
    let parts: Vec<Value> = (0..9_000)
        .map(|_| json!({ "type": "text", "text": "" }))
        .collect();
    let window = [json!({
        "info": { "id": "m".repeat(500), "sessionID": SESSION, "role": "user",
                  "time": { "created": 1_700_000_000_000u64 } },
        "parts": parts,
    })];
    let tip = kernel_store(&handler).tip().unwrap();
    let end = capture_job(&handler, &project, SESSION, &window).await;
    assert_eq!(end, source_capture::Count::RefusedResidentShort);
    assert_eq!(kernel_store(&handler).tip().unwrap(), tip);
    assert_eq!(report(&handler)["units_published"], 0);
    drop(held);
    assert_eq!(report(&handler)["retained_bytes"], 0);
}

/// D13: a same-key conflict inside the commit makes that unit terminal and leaves its sibling pending, so the report shows the sibling's age and the next pass publishes it from the memo's pending set.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_conflict_in_the_commit_leaves_the_sibling_pending() {
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
    let raced = opencode_user("msg_raced", 1_700_000_000_100, "raced");
    let unit = crate::harness_sources::opencode_units(&session, &raced)
        .unwrap()
        .remove(0);
    let (racing_store, scope_id) = (Arc::clone(&store), binding.scope_id());
    *handler.source_capture.before_publish.lock().unwrap() = Some(Arc::new(move || {
        crate::harness_sources::SourcePublisher {
            kernel: &racing_store,
            domain_id: "harness-capture",
            scope_id: Some(&scope_id),
            egress: kernel::ProviderEgress::LocalOnly,
            sensitivity: kernel::Sensitivity::Normal,
        }
        .publish(&unit, now_ms())
        .unwrap();
    }));
    let window = [
        raced,
        opencode_user("msg_sibling", 1_700_000_000_200, "a clean sibling"),
    ];
    pass(&handler, transform_with(&window, "base-1")).await;
    settle(&handler).await;
    assert_eq!(report(&handler)["units_identity_conflict"], 1);
    assert_eq!(report(&handler)["kernel_error"], 1);
    assert!(
        report(&handler)["oldest_deferred_age_ms"].is_u64(),
        "the sibling waits as pending: {}",
        report(&handler)
    );
    *handler.source_capture.before_publish.lock().unwrap() = None;
    pass(&handler, transform_with(&window, "base-2")).await;
    settle(&handler).await;
    assert_eq!(report(&handler)["units_published"], 2);
    assert_eq!(
        report(&handler)["units_memo_skipped"],
        1,
        "the memo answers for the terminal unit"
    );
    assert_eq!(report(&handler)["oldest_deferred_age_ms"], Value::Null);
    let published: Vec<String> = live(&store, OccurrenceClass::Messages)
        .iter()
        .map(|row| identity_of(row)[3].1.to_owned())
        .collect();
    assert!(
        published.contains(&"msg_sibling".to_owned()),
        "{published:?}"
    );
}

/// An assistant turn whose one `bash` call settled at `end` with `output`.
fn opencode_rerun(end: u64, output: &str) -> Value {
    json!({
        "info": { "id": "msg_rerun", "sessionID": SESSION, "role": "assistant",
                  "time": { "created": 1_700_000_000_000u64, "completed": end + 10 } },
        "parts": [
            { "type": "tool", "callID": "call_rerun", "tool": "bash",
              "state": { "status": "completed", "output": output, "input": { "command": "cargo test" },
                         "metadata": { "exit": 0, "truncated": false },
                         "time": { "start": end - 5, "end": end } } },
        ],
    })
}

/// AC14 through the capture path (`replay_after_memo_loss`, `reversed_generation_delivery`): the OpenCode call re-runs in place with a new `state.time.end`. Generation 1 publishes; after a restart empties the memo, generation 2 retires it in its own commit; a window still showing generation 1 replays and retires nothing; an older generation that never published is superseded and terminal. The call keeps exactly one live generation throughout.
#[tokio::test]
async fn a_rerun_keeps_one_live_generation_across_a_restart() {
    let (handler, _dir, _project) = capture_handler(capture_config(true), true).await;
    let user = opencode_user("msg_u1", 1_700_000_000_000, "run the tests");
    let live_generations = |store: &KernelStore| -> Vec<String> {
        live(store, OccurrenceClass::RawToolSpans)
            .iter()
            .filter(|row| identity_of(row)[4] == ("tool_call_id", "call_rerun"))
            .map(|row| identity_of(row)[5].1.to_owned())
            .collect()
    };
    pass(
        &handler,
        transform_with(
            &[
                user.clone(),
                opencode_rerun(1_700_000_001_000, "1 failed\n"),
            ],
            "base-1",
        ),
    )
    .await;
    settle(&handler).await;
    let store = kernel_store(&handler);
    assert_eq!(live_generations(&store), ["1700000001000"]);

    handler.source_capture.forget_memo_for_test();
    pass(
        &handler,
        transform_with(
            &[user.clone(), opencode_rerun(1_700_000_002_000, "ok\n")],
            "base-2",
        ),
    )
    .await;
    settle(&handler).await;
    assert_eq!(live_generations(&store), ["1700000002000"]);

    handler.source_capture.forget_memo_for_test();
    let tip = store.tip().unwrap();
    pass(
        &handler,
        transform_with(
            &[user, opencode_rerun(1_700_000_001_000, "1 failed\n")],
            "base-3",
        ),
    )
    .await;
    settle(&handler).await;
    assert_eq!(
        store.tip().unwrap(),
        tip,
        "a redelivered generation commits nothing"
    );
    assert_eq!(live_generations(&store), ["1700000002000"]);

    // A generation older than the live one that never published is superseded, retains nothing, and is not offered again.
    let stale = [
        opencode_user("msg_u1", 1_700_000_000_000, "run the tests"),
        opencode_rerun(1_700_000_001_500, "flaky\n"),
    ];
    pass(&handler, transform_with(&stale, "base-4")).await;
    settle(&handler).await;
    assert_eq!(store.tip().unwrap(), tip);
    assert_eq!(report(&handler)["units_superseded"], 1);
    pass(&handler, transform_with(&stale, "base-5")).await;
    settle(&handler).await;
    assert_eq!(
        report(&handler)["units_superseded"],
        1,
        "a superseded unit is terminal"
    );
    assert_eq!(live_generations(&store), ["1700000002000"]);
}

pub(super) async fn delete_session(handler: &Handler, session_id: &str) -> Value {
    delete_on(&handler.core, session_id).await
}

async fn delete_on(core: &Arc<HandlerCore>, session_id: &str) -> Value {
    let outcome = core
        .handle_session_delete_value(
            test_route(7),
            &json!({ "method": "session.delete", "session_id": session_id }),
        )
        .await;
    match outcome {
        PreparedOutcome::Response(bytes) => serde_json::from_slice(&bytes).unwrap(),
        PreparedOutcome::Error { code, message } => json!({ "error": code, "message": message }),
        other => panic!("unexpected outcome {other:?}"),
    }
}

/// The live captured descriptors naming `session_id`, across both classes.
fn live_for_session(store: &KernelStore, session_id: &str) -> Vec<LiveDescriptor> {
    [OccurrenceClass::Messages, OccurrenceClass::RawToolSpans]
        .into_iter()
        .flat_map(|class| live(store, class))
        .filter(|row| identity_of(row)[2] == ("session_id", session_id))
        .collect()
}

/// AC5 and AC15 (`sc-delete-retires-before-memory-store-delete`, `sc-delete-retry-recovers-crash-between-stores`): `session.delete` retires every live `messages` and `raw_tool_spans` descriptor the session owns before the memory-store delete. The retired rows stay readable as history, and another session of the project keeps its descriptors. An application-fault cut between the Kernel commit and the memory-store delete (F23) leaves the memory rows; one retry answers `ok`, commits nothing new, and completes the delete. The cut runs in one process; process restart and power loss are not exercised.
#[tokio::test]
async fn session_delete_retires_captured_sources_and_a_retry_recovers_a_stop_between_stores() {
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    let window = [
        opencode_user("msg_u1", 1_700_000_000_000, "please run the tests"),
        opencode_assistant_with_tools("msg_a1", 1_700_000_001_000),
    ];
    pass(&handler, transform_with(&window, "base-1")).await;
    settle(&handler).await;
    let store = kernel_store(&handler);
    // Another session of the same project, captured through its own route.
    let mut other = binding_with_harness(project.to_str().unwrap(), "opencode", "ses_other");
    other.config = capture_config(true);
    handler.bind_route(test_route(8), other);
    let other_window: Vec<Value> = window
        .iter()
        .map(|message| {
            let mut message = message.clone();
            message["info"]["sessionID"] = json!("ses_other");
            message
        })
        .collect();
    let mut request = transform_with(&other_window, "base-o");
    request["session_id"] = json!("ses_other");
    pass_on(&handler, test_route(8), request).await;
    settle(&handler).await;
    let captured = live_for_session(&store, SESSION);
    assert!(captured.len() >= 4, "{}", captured.len());
    let others = live_for_session(&store, "ses_other").len();
    assert!(others >= 4, "{others}");

    // F23, an application-fault cut: the Kernel retirement commits, then the delete returns before the memory store.
    handler
        .source_capture
        .stop_after_retirement
        .store(true, std::sync::atomic::Ordering::Release);
    let stopped = delete_session(&handler, SESSION).await;
    assert_eq!(stopped["error"], "injected_stop", "{stopped}");
    assert!(
        live_for_session(&store, SESSION).is_empty(),
        "the Kernel step committed"
    );
    let tip = store.tip().unwrap();

    let retried = delete_session(&handler, SESSION).await;
    assert_eq!(retried["ok"], true, "{retried}");
    assert!(
        retried["deleted_rows"].as_u64().unwrap() > 0,
        "the memory rows were still there"
    );
    assert_eq!(store.tip().unwrap(), tip, "the retry retires nothing new");
    assert_eq!(
        live_for_session(&store, "ses_other").len(),
        others,
        "another session is untouched"
    );
    for row in &captured {
        let (_, states) = store
            .object_states(std::slice::from_ref(&row.object_id))
            .unwrap();
        let state = states[0].as_ref().expect("a retired row stays as history");
        assert!(state.object.invalidated_commit_seq.is_some());
        assert_eq!(
            bytes_of(&store, row).is_empty(),
            row.detail.artifact_digest == kernel::source_identity::payload_id(b"")
        );
    }
}

/// OQ12 (fail closed): a Kernel still starting answers a retryable `kernel_unavailable`, and the memory store keeps the session; once the Kernel is ready, the retry completes.
#[tokio::test]
async fn session_delete_with_the_kernel_starting_deletes_nothing() {
    let (handler, dir, _project) = capture_handler(capture_config(true), false).await;
    handler.kernel.set_expected(true);
    assert_eq!(handler.kernel.state(), kernel_routes::KernelState::Starting);
    pass(
        &handler,
        transform_with(
            &[opencode_user("msg_u1", 1_700_000_000_000, "hi")],
            "base-1",
        ),
    )
    .await;
    let refused = delete_session(&handler, SESSION).await;
    assert_eq!(refused["error"], "kernel_unavailable", "{refused}");
    // Once the Kernel is ready, the retry completes.
    handler
        .kernel
        .open(
            dir.path().join("kernel"),
            StoreOpenPolicy::default(),
            handler.cancel.clone(),
        )
        .await;
    assert_eq!(handler.kernel.state(), kernel_routes::KernelState::Ready);
    let deleted = delete_session(&handler, SESSION).await;
    assert_eq!(deleted["ok"], true, "{deleted}");
    assert!(
        deleted["deleted_rows"].as_u64().unwrap() > 0,
        "the refused delete left the session's rows: {deleted}"
    );
}

/// AC15 second clause (`sc-deleted-session-not-resurrected-by-queued-capture`): a delete that completes while a capture job is parked before its first publication drops the pending units. Released afterwards, the job rechecks its ticket under the gate, counts the session invalidated, and commits nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delete_before_the_first_publication_drops_the_pending_capture() {
    let (handler, _dir, _project) = capture_handler(capture_config(true), true).await;
    let (entered, wait_entered) = mpsc::channel::<()>();
    let (release, wait_release) = mpsc::channel::<()>();
    let wait_release = Arc::new(Mutex::new(wait_release));
    *handler.source_capture.before_publish.lock().unwrap() = Some(Arc::new(move || {
        entered.send(()).unwrap();
        wait_release.lock().unwrap().recv().unwrap();
    }));
    pass(
        &handler,
        transform_with(
            &[opencode_user("msg_u1", 1_700_000_000_000, "secret plan")],
            "base-1",
        ),
    )
    .await;
    tokio::task::spawn_blocking(move || wait_entered.recv().unwrap())
        .await
        .unwrap();
    // The job is parked before it takes the gate; the delete runs to completion first.
    assert_eq!(delete_session(&handler, SESSION).await["ok"], true);
    let store = kernel_store(&handler);
    let tip = store.tip().unwrap();
    release.send(()).unwrap();
    *handler.source_capture.before_publish.lock().unwrap() = None;
    settle(&handler).await;
    assert_eq!(report(&handler)["session_invalidated"], 1);
    assert_eq!(
        store.tip().unwrap(),
        tip,
        "the released job commits nothing"
    );
    assert!(live_for_session(&store, SESSION).is_empty());
}

/// D11 scope and the cut between pages: the delete retires exactly the bound project, harness, and session; the same session id under another project or another harness keeps its descriptors. A retirement stopped after its first page leaves the rest live, and one `session.delete` then answers `ok` with nothing of the session live; pages of one row walk another project's messages and tool results to the end.
#[tokio::test]
async fn session_delete_retires_only_its_own_captures_and_recovers_a_stop_between_pages() {
    let (handler, dir, project) = capture_handler(capture_config(true), true).await;
    let window = [
        opencode_user("msg_u1", 1_700_000_000_000, "please run the tests"),
        opencode_assistant_with_tools("msg_a1", 1_700_000_001_000),
    ];
    pass(&handler, transform_with(&window, "base-1")).await;
    settle(&handler).await;
    // The same session id in another project.
    let elsewhere = dir.path().join("project-b");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let mut bound = binding_with_harness(elsewhere.to_str().unwrap(), "opencode", SESSION);
    bound.config = capture_config(true);
    handler.bind_route(test_route(8), bound);
    pass_on(&handler, test_route(8), transform_with(&window, "base-b")).await;
    settle(&handler).await;
    // The same session id under the Pi harness of the same project.
    let mut bound = binding_with_harness(project.to_str().unwrap(), "pi", SESSION);
    bound.config = capture_config(true);
    handler.bind_route(test_route(9), bound);
    let rows = vec![pi_row(
        "pi-u1",
        None,
        json!({ "role": "user", "content": "pi words", "timestamp": 1_700_000_000_000u64 }),
    )];
    pass_on(&handler, test_route(9), pi_request(&rows, "base-p")).await;
    settle(&handler).await;

    let store = kernel_store(&handler);
    let digest = kernel_routes::ProjectBinding::new(&project)
        .digest()
        .to_owned();
    let owned = |row: &LiveDescriptor| {
        identity_of(row)[0] == ("project_id", digest.as_str())
            && identity_of(row)[1] == ("harness", "opencode")
    };
    let all = live_for_session(&store, SESSION);
    let mine = all.iter().filter(|row| owned(row)).count();
    let others = all.len() - mine;
    assert!(mine >= 4 && others >= 2, "{mine} {others}");

    // A retirement that stops after one page of two rows.
    let session = source_capture::CapturedSession {
        project_id: &digest,
        harness: "opencode",
        session_id: SESSION,
    };
    let done =
        source_capture::retire_session_pages(&store, session, NonZeroUsize::new(2).unwrap(), 1)
            .unwrap();
    assert!(!done);
    let remaining = live_for_session(&store, SESSION);
    assert_eq!(remaining.iter().filter(|row| owned(row)).count(), mine - 2);

    let deleted = delete_session(&handler, SESSION).await;
    assert_eq!(deleted["ok"], true, "{deleted}");
    assert!(
        deleted["deleted_rows"].as_u64().unwrap() > 0,
        "the memory rows waited for the retry"
    );
    let after = live_for_session(&store, SESSION);
    assert!(after.iter().all(|row| !owned(row)));
    assert_eq!(
        after.len(),
        others,
        "another project or harness keeps its captures"
    );

    // Pages of one row walk the other project's messages and tool results to the end and leave the Pi rows.
    let digest_b = kernel_routes::ProjectBinding::new(&elsewhere)
        .digest()
        .to_owned();
    let project_b = source_capture::CapturedSession {
        project_id: &digest_b,
        harness: "opencode",
        session_id: SESSION,
    };
    assert!(
        source_capture::retire_session_pages(
            &store,
            project_b,
            NonZeroUsize::new(1).unwrap(),
            usize::MAX,
        )
        .unwrap()
    );
    let left = live_for_session(&store, SESSION);
    assert!(!left.is_empty());
    assert!(
        left.iter()
            .all(|row| identity_of(row)[1] == ("harness", "pi"))
    );
}

/// C8-style page receipts: a page's key and digest depend on the session's identity fields and the page's ids only, so equal inputs give one key and any change gives another; committing the same page again replays.
#[tokio::test]
async fn session_retirement_pages_are_keyed_by_session_and_ids() {
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    let digest = kernel_routes::ProjectBinding::new(&project)
        .digest()
        .to_owned();
    let session = |project_id, harness, session_id| source_capture::CapturedSession {
        project_id,
        harness,
        session_id,
    };
    let page = vec!["srcdesc:a:1".to_owned(), "srcdesc:b:1".to_owned()];
    let key =
        |session, page: &[String]| source_capture::session_page_intent(session, page).operation_key;
    let base = key(session(&digest, "opencode", SESSION), &page);
    assert_eq!(base, key(session(&digest, "opencode", SESSION), &page));
    for other in [
        key(session("other", "opencode", SESSION), &page),
        key(session(&digest, "pi", SESSION), &page),
        key(session(&digest, "opencode", "ses_2"), &page),
        key(session(&digest, "opencode", SESSION), &page[..1]),
    ] {
        assert_ne!(other, base);
    }

    // A page committed again replays.
    pass(
        &handler,
        transform_with(
            &[opencode_user("msg_u1", 1_700_000_000_000, "hello")],
            "base-1",
        ),
    )
    .await;
    settle(&handler).await;
    let store = kernel_store(&handler);
    let ids: Vec<String> = live_for_session(&store, SESSION)
        .into_iter()
        .map(|row| row.object_id)
        .collect();
    let session = session(&digest, "opencode", SESSION);
    let intent = source_capture::session_page_intent(session, &ids);
    let retire = |envelope: &mut kernel::Envelope<'_>| {
        crate::harness_sources::retire_descriptors(envelope, &ids)
    };
    assert!(!store.commit(intent.clone(), retire).unwrap().replayed);
    assert!(store.commit(intent, retire).unwrap().replayed);
}

/// The harness capture stamps on a descriptor is the one `session.delete` retires by: a route whose claimed harness differs from its serializer profile's captures nothing.
#[tokio::test]
async fn a_route_claiming_another_harness_captures_nothing() {
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    let mut bound = binding_with_harness(project.to_str().unwrap(), "pi", SESSION);
    bound.config = capture_config(true);
    handler.bind_route(test_route(7), bound);
    let store = kernel_store(&handler);
    let tip = store.tip().unwrap();
    pass(
        &handler,
        transform_with(
            &[opencode_user("msg_u1", 1_700_000_000_000, "hello")],
            "base-1",
        ),
    )
    .await;
    settle(&handler).await;
    assert_eq!(report(&handler)["admitted"], 0);
    assert_eq!(store.tip().unwrap(), tip);
}

/// Retirement past one inventory snapshot: with one row per page, 24 captured descriptors take more pages than one snapshot lists. A bound of five pages retires exactly five rows and reports the inventory unfinished; the next call resumes from a fresh pass, crosses the snapshot boundary, and leaves nothing of the session live; a retry commits nothing.
#[tokio::test]
async fn session_retirement_crosses_inventory_snapshots_and_stops_at_its_page_bound() {
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    let window: Vec<Value> = (0..6u64)
        .flat_map(|index| {
            let mut assistant = opencode_assistant_with_tools(
                &format!("msg_a{index}"),
                1_700_000_001_000 + index * 2_000,
            );
            for part in assistant["parts"].as_array_mut().unwrap() {
                if let Some(call) = part["callID"].as_str().map(str::to_owned) {
                    part["callID"] = json!(format!("{call}_{index}"));
                }
            }
            [
                opencode_user(
                    &format!("msg_u{index}"),
                    1_700_000_000_000 + index * 2_000,
                    "please run the tests",
                ),
                assistant,
            ]
        })
        .collect();
    pass(&handler, transform_with(&window, "base-1")).await;
    settle(&handler).await;
    let store = kernel_store(&handler);
    let captured = live_for_session(&store, SESSION).len();
    assert_eq!(captured, 24);
    let digest = kernel_routes::ProjectBinding::new(&project)
        .digest()
        .to_owned();
    let session = source_capture::CapturedSession {
        project_id: &digest,
        harness: "opencode",
        session_id: SESSION,
    };
    let one = NonZeroUsize::new(1).unwrap();
    assert!(!source_capture::retire_session_pages(&store, session, one, 5).unwrap());
    assert_eq!(live_for_session(&store, SESSION).len(), captured - 5);
    assert!(source_capture::retire_session_pages(&store, session, one, usize::MAX).unwrap());
    assert!(live_for_session(&store, SESSION).is_empty());
    let tip = store.tip().unwrap();
    assert!(source_capture::retire_session_pages(&store, session, one, usize::MAX).unwrap());
    assert_eq!(store.tip().unwrap(), tip, "a retry commits nothing");
}

/// The live `repository_documents` descriptors of `path`, as `(revision, object id)`.
fn live_document(store: &KernelStore, path: &str) -> Vec<(String, String)> {
    live(store, OccurrenceClass::RepositoryDocuments)
        .into_iter()
        .filter(|row| identity_of(row)[2] == ("relative_path", path))
        .map(|row| (row.detail.revision.clone(), row.object_id))
        .collect()
}

/// One pass as the first of a new session of the project bound to route 7. A session's first pass composes its prompt block, which is where the pass reads the configured documents.
async fn document_pass(handler: &Handler, base: &str) -> Value {
    document_pass_with(handler, base, "hello").await
}

/// [`document_pass`] over a window of one user message holding `text`.
async fn document_pass_with(handler: &Handler, base: &str, text: &str) -> Value {
    let session = format!("ses-{base}");
    let mut bound = handler
        .bindings
        .lock()
        .expect("bindings mutex")
        .get(&test_route(7))
        .expect("route 7 is bound")
        .clone();
    bound.session = session.clone();
    let routes = handler
        .bindings
        .lock()
        .expect("bindings mutex")
        .values()
        .count();
    let route = test_route(100 + u16::try_from(routes).expect("few routes"));
    handler.bind_route(route, bound);
    let mut request = transform_with(&[opencode_user("msg_u1", 1_700_000_000_000, text)], base);
    request["session_id"] = json!(session);
    let response = pass_on(handler, route, request).await;
    settle(handler).await;
    response
}

/// `(invalidated_commit_seq, superseded_by)` of a descriptor row.
fn descriptor_state(project: &Path, object_id: &str) -> (Option<i64>, Option<String>) {
    rusqlite::Connection::open_with_flags(
        project
            .parent()
            .unwrap()
            .join("kernel")
            .join("kernel.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
    .query_row(
        "SELECT invalidated_commit_seq,superseded_by FROM observations WHERE object_id=?1",
        [object_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .unwrap()
}

/// AC4 and AC17 (`sc-repodoc-one-live-revision-above-historical-head`, `sc-repodoc-raw-bytes-separate-from-prompt-hash`): a configured document read for the prompt is captured as one lineage keyed by path. Its first revision is 1. An unchanged file publishes nothing. An edit supersedes, and the earlier revision names its successor. A whitespace-only edit publishes a new revision of the raw bytes while the prompt's canonical hash stays. A verified removal retires the lineage, and a recreated file republishes above the historical head. The descriptors admit as untrusted repository text, store `Sensitive`, and create no decision.
#[tokio::test]
async fn a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires() {
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    let doc = project.join("ARCHITECTURE.md");
    std::fs::write(&doc, "# Architecture\n\nOne daemon.\n").unwrap();
    document_pass(&handler, "base-1").await;
    let store = kernel_store(&handler);
    let first = live_document(&store, "ARCHITECTURE.md");
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].0, "1");
    let row = live(&store, OccurrenceClass::RepositoryDocuments).remove(0);
    assert_eq!(
        bytes_of(&store, &row),
        "# Architecture\n\nOne daemon.\n",
        "raw bytes"
    );
    assert_eq!(row.sensitivity, kernel::Sensitivity::Sensitive);
    let decisions = store
        .slice_as_of(store.tip().unwrap())
        .unwrap()
        .decisions
        .len();

    // A pass that reuses its prompt block reads no documents, so an edit waits for the next compose read.
    pass(
        &handler,
        transform_with(
            &[opencode_user("msg_u1", 1_700_000_000_000, "hello")],
            "base-s1",
        ),
    )
    .await;
    settle(&handler).await;
    std::fs::write(&doc, "# Architecture\n\nUncomposed.\n").unwrap();
    pass(
        &handler,
        transform_with(
            &[opencode_user("msg_u1", 1_700_000_000_000, "hello")],
            "base-s1",
        ),
    )
    .await;
    settle(&handler).await;
    assert_eq!(live_document(&store, "ARCHITECTURE.md"), first);
    std::fs::write(&doc, "# Architecture\n\nOne daemon.\n").unwrap();

    // Unchanged: nothing commits.
    let tip = capture_tip(&handler);
    document_pass(&handler, "base-2").await;
    assert_eq!(capture_tip(&handler), tip);
    assert_eq!(
        report(&handler)["documents_unchanged"],
        2,
        "route 7's composing first pass and this pass each found the file unchanged"
    );

    // An edit supersedes revision 1.
    std::fs::write(&doc, "# Architecture\n\nTwo daemons.\n").unwrap();
    document_pass(&handler, "base-3").await;
    let second = live_document(&store, "ARCHITECTURE.md");
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].0, "2");
    assert_eq!(
        descriptor_state(&project, &first[0].1).1.as_deref(),
        Some(second[0].1.as_str()),
        "a reference pinned to revision 1 reports its successor"
    );
    let scope = handler
        .bindings
        .lock()
        .expect("bindings mutex")
        .get(&test_route(7))
        .expect("route 7 is bound")
        .kernel_project
        .scope()
        .clone();
    let stale = store
        .guard_current_input(
            &kernel::CurrentInputExpectation {
                object_id: row.object_id.clone(),
                source_revision: 1,
                occurrence_id: row.detail.occurrence_id.clone(),
                payload_id: row.detail.payload_id.clone(),
                artifact_digest: row.detail.artifact_digest.clone(),
            },
            kernel::EligibilityBinding {
                project: &scope,
                destination: kernel::ArtifactDestination::Local,
            },
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap()
        .expect_err("revision 1 is no longer current");
    assert_eq!(stale.reason(), &kernel::StaleInput::Superseded);
    drop(stale);

    // A whitespace-only edit: new raw bytes, same canonical prompt hash.
    let hash = crate::project_docs::read_project_docs_canonical(
        project.to_str().unwrap(),
        crate::project_docs::default_document_paths_for_test(),
    )
    .canonical_hash;
    std::fs::write(&doc, "# Architecture  \n\nTwo daemons.\n\n").unwrap();
    assert_eq!(
        crate::project_docs::read_project_docs_canonical(
            project.to_str().unwrap(),
            crate::project_docs::default_document_paths_for_test(),
        )
        .canonical_hash,
        hash
    );
    document_pass(&handler, "base-4").await;
    assert_eq!(live_document(&store, "ARCHITECTURE.md")[0].0, "3");
    let third = live(&store, OccurrenceClass::RepositoryDocuments).remove(0);
    assert_eq!(
        bytes_of(&store, &third),
        "# Architecture  \n\nTwo daemons.\n\n",
        "the artifact holds the raw edited bytes"
    );

    // Removal retires; recreation republishes above the historical head.
    std::fs::remove_file(&doc).unwrap();
    document_pass(&handler, "base-5").await;
    assert!(live_document(&store, "ARCHITECTURE.md").is_empty());
    assert_eq!(report(&handler)["documents_retired"], 1);
    std::fs::write(&doc, "# Architecture\n\nBack.\n").unwrap();
    document_pass(&handler, "base-6").await;
    assert_eq!(live_document(&store, "ARCHITECTURE.md")[0].0, "4");
    // A second removal retires revision 4 under its own receipt, and a missing file with no live revision commits nothing.
    std::fs::remove_file(&doc).unwrap();
    document_pass(&handler, "base-7").await;
    assert!(live_document(&store, "ARCHITECTURE.md").is_empty());
    assert_eq!(report(&handler)["documents_retired"], 2);
    let tip = capture_tip(&handler);
    let missing = report(&handler)["documents_missing"].as_u64().unwrap();
    document_pass(&handler, "base-8").await;
    assert_eq!(capture_tip(&handler), tip);
    assert_eq!(
        report(&handler)["documents_missing"].as_u64().unwrap(),
        missing + 2
    );
    std::fs::write(&doc, "# Architecture\n\nBack.\n").unwrap();
    document_pass(&handler, "base-9").await;
    assert_eq!(live_document(&store, "ARCHITECTURE.md")[0].0, "5");

    let object_id = live_document(&store, "ARCHITECTURE.md")[0].1.clone();
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
        [&object_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .unwrap();
    assert_eq!(
        admission,
        (
            SourceClass::UntrustedRepoText.as_str().to_owned(),
            TaintClass::RepoUntrustedText.as_str().to_owned()
        )
    );
    assert_eq!(
        store
            .slice_as_of(store.tip().unwrap())
            .unwrap()
            .decisions
            .len(),
        decisions,
        "document capture creates no decision"
    );
}

/// A `repository_documents` lineage another domain owns is left alone: a missing local file at its path retires nothing and counts as missing, as the text path refuses to supersede a row the capture does not own.
#[tokio::test]
async fn a_missing_document_retires_only_the_captures_own_lineage() {
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    document_pass(&handler, "base-0").await;
    let store = kernel_store(&handler);
    let binding = kernel_routes::ProjectBinding::new(&project);
    store
        .commit(
            kernel::CommitIntent {
                producer: "source-capture-test".to_owned(),
                operation_key: "other-producer-domain".to_owned(),
                request_digest: "1".repeat(64),
                actor: "test".to_owned(),
                cause: "another producer's domain".to_owned(),
            },
            |envelope| {
                crate::kernel_routes::commit::ensure_domain(
                    envelope,
                    "other-producer",
                    &mut HashSet::new(),
                )?;
                Ok(String::new())
            },
        )
        .unwrap();
    let digest = binding.digest().to_owned();
    let unit = crate::harness_sources::SourceUnit {
        class: OccurrenceClass::RepositoryDocuments,
        identity: vec![
            ("project_id", digest.clone()),
            ("repository_id", digest),
            ("relative_path", "ARCHITECTURE.md".to_owned()),
        ],
        revision: "1".to_owned(),
        representation: crate::harness_sources::Representation::Text,
        text: "# Foreign\n".to_owned(),
        role: "document".to_owned(),
        outcome: None,
    };
    crate::harness_sources::SourcePublisher {
        kernel: &store,
        domain_id: "other-producer",
        scope_id: Some(&binding.scope_id()),
        egress: kernel::ProviderEgress::LocalOnly,
        sensitivity: kernel::Sensitivity::Sensitive,
    }
    .publish(&unit, now_ms())
    .unwrap();
    let foreign = live_document(&store, "ARCHITECTURE.md");
    assert_eq!(foreign.len(), 1);
    let missing = report(&handler)["documents_missing"].as_u64().unwrap();
    document_pass(&handler, "base-1").await;
    assert_eq!(
        live_document(&store, "ARCHITECTURE.md"),
        foreign,
        "another domain's lineage stays live"
    );
    assert_eq!(report(&handler)["documents_retired"], 0);
    assert_eq!(
        report(&handler)["documents_missing"].as_u64().unwrap(),
        missing + 2
    );
}

/// A rename is a retirement at the old path and a publication at the new one, in one pass.
#[tokio::test]
async fn a_renamed_document_retires_the_old_path_and_publishes_the_new() {
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    std::fs::write(project.join("STRUCTURE.md"), "# Layout\n").unwrap();
    document_pass(&handler, "base-1").await;
    std::fs::rename(
        project.join("STRUCTURE.md"),
        project.join("ARCHITECTURE.md"),
    )
    .unwrap();
    document_pass(&handler, "base-2").await;
    let store = kernel_store(&handler);
    assert!(live_document(&store, "STRUCTURE.md").is_empty());
    assert_eq!(live_document(&store, "ARCHITECTURE.md")[0].0, "1");
    assert_eq!(report(&handler)["documents_retired"], 1);
}

/// A refused window batch leaves the pass's documents to their own batch: a window whose unit conflicts still captures the edited document its pass composed.
#[tokio::test]
async fn a_refused_window_still_captures_its_documents() {
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    let doc = project.join("ARCHITECTURE.md");
    std::fs::write(&doc, "# Architecture\n").unwrap();
    let window = |text: &str| [opencode_user("msg_u1", 1_700_000_000_000, text)];
    pass(&handler, transform_with(&window("original"), "base-1")).await;
    settle(&handler).await;
    std::fs::write(&doc, "# Architecture\n\nEdited.\n").unwrap();
    let mut request = transform_with(&window("edited in place"), "base-2");
    request["render_config"] = json!("recompose");
    pass(&handler, request).await;
    settle(&handler).await;
    assert_eq!(report(&handler)["units_identity_conflict"], 1);
    let store = kernel_store(&handler);
    assert_eq!(live_document(&store, "ARCHITECTURE.md")[0].0, "2");
}

/// The text of a pass's documents is charged to the retained budget with its window, once as read and once as its unit.
#[tokio::test]
async fn a_documents_text_is_charged_with_its_window() {
    let retained = |document: Option<String>| async move {
        let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
        if let Some(document) = document {
            std::fs::write(project.join("ARCHITECTURE.md"), document).unwrap();
        }
        let (entered_tx, entered_rx) = mpsc::channel::<()>();
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let go_rx = Arc::new(Mutex::new(go_rx));
        let entered_tx = Arc::new(Mutex::new(entered_tx));
        *handler.source_capture.before_publish.lock().unwrap() = Some(Arc::new(move || {
            entered_tx.lock().unwrap().send(()).unwrap();
            go_rx.lock().unwrap().recv().unwrap();
        }));
        pass(
            &handler,
            transform_with(
                &[opencode_user("msg_u1", 1_700_000_000_000, "hello")],
                "base-1",
            ),
        )
        .await;
        tokio::task::spawn_blocking(move || entered_rx.recv().unwrap())
            .await
            .unwrap();
        let retained = report(&handler)["retained_bytes"].as_u64().unwrap();
        go_tx.send(()).unwrap();
        settle(&handler).await;
        retained
    };
    let without = retained(None).await;
    let with = retained(Some("x".repeat(60 * 1024))).await;
    assert!(with - without >= 2 * 60 * 1024, "{without} {with}");
}

/// AC17: the kernel store serves every project of the home, so a project's document is found among many projects' live documents. An unchanged file publishes nothing and a removal retires its own revision.
#[tokio::test]
async fn a_document_is_judged_against_its_own_project_among_many() {
    let (handler, dir, project) = capture_handler(capture_config(true), true).await;
    for other in 0..10u16 {
        let root = dir.path().join(format!("other-{other}"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("ARCHITECTURE.md"), format!("# Other {other}\n")).unwrap();
        let session = format!("ses-other-{other}");
        let mut bound = binding_with_harness(root.to_str().unwrap(), "opencode", &session);
        bound.config = capture_config(true);
        handler.bind_route(test_route(200 + other), bound);
        let mut request = transform_with(
            &[opencode_user("msg_u1", 1_700_000_000_000, "hello")],
            "base-o",
        );
        request["session_id"] = json!(session);
        pass_on(&handler, test_route(200 + other), request).await;
        settle(&handler).await;
    }
    let doc = project.join("ARCHITECTURE.md");
    std::fs::write(&doc, "# Architecture\n").unwrap();
    document_pass(&handler, "base-1").await;
    let store = kernel_store(&handler);
    assert_eq!(live(&store, OccurrenceClass::RepositoryDocuments).len(), 11);
    let tip = capture_tip(&handler);
    document_pass(&handler, "base-2").await;
    assert_eq!(
        capture_tip(&handler),
        tip,
        "an unchanged file publishes nothing"
    );
    assert_eq!(report(&handler)["documents_unchanged"], 1);
    std::fs::remove_file(&doc).unwrap();
    document_pass(&handler, "base-3").await;
    assert_eq!(report(&handler)["documents_retired"], 1);
    assert_eq!(live(&store, OccurrenceClass::RepositoryDocuments).len(), 10);
}

/// AC17 and F18 (`sc-repodoc-only-verified-missing-retires`): a symlink, a text past the 64 KiB unit cap, a file past the 256 KiB read cap, non-UTF-8 bytes, and a permission error are each counted and retire nothing; only a missing file retires.
#[tokio::test]
async fn unreadable_documents_are_counted_and_retire_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let (handler, dir, project) = capture_handler(capture_config(true), true).await;
    let doc = project.join("STRUCTURE.md");
    std::fs::write(&doc, "# Structure\n").unwrap();
    document_pass(&handler, "base-1").await;
    let store = kernel_store(&handler);
    let live_before = live_document(&store, "STRUCTURE.md");
    assert_eq!(live_before.len(), 1);
    let elsewhere = dir.path().join("elsewhere.md");
    std::fs::write(&elsewhere, "linked").unwrap();
    type Arrange<'a> = Box<dyn Fn() + 'a>;
    let cases: Vec<(&str, Arrange<'_>)> = vec![
        (
            "documents_not_regular",
            Box::new(|| {
                std::fs::remove_file(&doc).unwrap();
                std::os::unix::fs::symlink(&elsewhere, &doc).unwrap();
            }),
        ),
        (
            "documents_oversize",
            Box::new(|| {
                std::fs::remove_file(&doc).unwrap();
                std::fs::write(&doc, "x".repeat(70 * 1024)).unwrap();
            }),
        ),
        (
            "documents_oversize",
            Box::new(|| {
                std::fs::write(&doc, "x".repeat(300 * 1024)).unwrap();
            }),
        ),
        (
            "documents_not_utf8",
            Box::new(|| {
                std::fs::write(&doc, [0xff, 0xfe, 0x00]).unwrap();
            }),
        ),
        (
            // Root reads a file of mode 0o000, so a root runner sees the text unchanged and the count stays.
            "documents_unreadable",
            Box::new(|| {
                std::fs::write(&doc, "# Structure\n").unwrap();
                std::fs::set_permissions(&doc, std::fs::Permissions::from_mode(0o000)).unwrap();
            }),
        ),
    ];
    let root = rustix::process::geteuid().is_root();
    for (round, (count, arrange)) in cases.iter().enumerate() {
        if root && *count == "documents_unreadable" {
            continue;
        }
        arrange();
        let before = report(&handler)[*count].as_u64().unwrap();
        document_pass(&handler, &format!("base-x{round}")).await;
        assert_eq!(
            report(&handler)[*count].as_u64().unwrap(),
            before + 1,
            "{count}"
        );
        assert_eq!(
            live_document(&store, "STRUCTURE.md"),
            live_before,
            "{count} retires nothing"
        );
    }
    std::fs::set_permissions(&doc, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(report(&handler)["documents_retired"], 0);
}

/// The response bytes are the same with capture on and off while a configured document is read for the prompt; a route that injects no documents captures none.
#[tokio::test]
async fn document_capture_leaves_the_response_and_respects_inject_docs() {
    let response = |enabled: bool, inject_docs: bool| async move {
        let mut config = capture_config(enabled);
        config.inject_docs = inject_docs;
        let (handler, dir, project) = capture_handler(config, true).await;
        std::fs::write(project.join("ARCHITECTURE.md"), "# Architecture\n").unwrap();
        let response = document_pass(&handler, "base-1").await;
        let captured = live_document(&kernel_store(&handler), "ARCHITECTURE.md").len();
        (response, captured, dir)
    };
    let (on, captured_on, _on_dir) = response(true, true).await;
    let (off, captured_off, _off_dir) = response(false, true).await;
    assert_eq!(on, off);
    assert_eq!((captured_on, captured_off), (1, 0));
    let (_, captured_without_docs, _dir) = response(true, false).await;
    assert_eq!(captured_without_docs, 0);
}

#[tokio::test]
async fn an_emergency_rerun_captures_the_documents_its_served_block_carries() {
    let mut config = default_test_config();
    config.source_capture_enabled = true;
    let producer = Arc::new(ProducerState::default());
    let (handler, _store, _dir, project) =
        handler_with_store_and_kernel(Arc::clone(&producer), config.clone()).await;
    rebind(&handler, &project, config);
    let doc = project.join("ARCHITECTURE.md");
    std::fs::write(&doc, "# Architecture\n").unwrap();
    let native: Vec<Value> = (1..=80u64)
        .map(|ordinal| {
            opencode_user(
                &format!("msg_{ordinal:03}"),
                1_700_000_000_000 + ordinal,
                &format!("message {ordinal} {}", "word ".repeat(800)),
            )
        })
        .collect();
    pass(&handler, transform_with(&native, "base-1")).await;
    settle(&handler).await;
    let store = kernel_store(&handler);
    assert_eq!(live_document(&store, "ARCHITECTURE.md")[0].0, "1");

    std::fs::write(&doc, "# Architecture\n\nEdited.\n").unwrap();
    let mut request = transform_with(&native, "base-2");
    request["usage"] = json!(ModuleUsage {
        current_total_input_tokens: 48_000,
        context_limit_tokens: 50_000,
        ..ModuleUsage::default()
    });
    let response = pass(&handler, request).await;
    settle(&handler).await;
    assert_eq!(response["history_summarizer"]["fired"], true);
    assert_eq!(response["action"], "SOFT");
    assert!(response.to_string().contains("Edited."));
    let current = live_document(&store, "ARCHITECTURE.md");
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].0, "2", "{}", report(&handler));
    let row = live(&store, OccurrenceClass::RepositoryDocuments)
        .into_iter()
        .find(|row| row.object_id == current[0].1)
        .expect("a live descriptor");
    assert_eq!(bytes_of(&store, &row), "# Architecture\n\nEdited.\n");
}

fn count(handler: &Handler, name: &str) -> u64 {
    report(handler)[name].as_u64().unwrap()
}

/// Deletes the artifact stored under `digest`; a purge names its operator, target, and reason, and `fault` stops a purge after its commit, before the unlink, leaving a pending unlink.
fn delete_artifact(
    store: &KernelStore,
    digest: &str,
    kind: kernel::ArtifactDeletionKind,
    fault: Option<kernel::ArtifactDeletionFault>,
) {
    let purge = kind == kernel::ArtifactDeletionKind::Purge;
    let request = kernel::ArtifactDeletionRequest {
        intent: kernel::CommitIntent {
            producer: "capture-test".to_owned(),
            operation_key: format!("delete:{digest}:{kind:?}"),
            request_digest: digest.to_owned(),
            actor: "test".to_owned(),
            cause: "readmission".to_owned(),
        },
        identity: kernel::ArtifactDeletionIdentity::Digest(digest.to_owned()),
        kind,
        operator_id: purge.then(|| "operator".to_owned()),
        target_locator: purge.then(|| "incident://readmission".to_owned()),
        reason: purge.then(|| "readmission".to_owned()),
        deleted_at: now_ms(),
    };
    match fault {
        None => {
            store.delete_artifact(request).unwrap();
        }
        Some(fault) => {
            let error = store
                .delete_artifact_with_fault_for_test(request, fault)
                .unwrap_err();
            assert_eq!(error.kind(), kernel::ArtifactErrorKind::PurgeUnlinkPending);
        }
    }
}

/// Publishes `text` as one unit and returns the payload digest its descriptor cites, after checking that the pre-screen's digest of the same bytes is that digest.
async fn published_digest(handler: &Handler, project: &Path, text: &str) -> String {
    let window = [opencode_user("msg_a", 1_700_000_000_000, text)];
    let end = capture_job(handler, project, SESSION, &window).await;
    assert_eq!(end, source_capture::Count::Completed);
    let rows = live(&kernel_store(handler), OccurrenceClass::Messages);
    assert_eq!(rows.len(), 1);
    let digest = kernel::source_identity::payload_id(text.as_bytes());
    assert_eq!(rows[0].detail.artifact_digest, digest);
    digest
}

/// The purged unit's bytes under a new identity, beside a sibling.
fn readmission_window(text: &str) -> [Value; 2] {
    [
        opencode_user("msg_again", 1_700_000_000_001, text),
        opencode_user("msg_sibling", 1_700_000_000_002, "a sibling"),
    ]
}

/// AE5: a unit whose bytes a purge tombstone or a pending unlink names is refused alone before the commit, its sibling publishes in the same pass, and the next pass answers both from the memo.
#[tokio::test]
async fn a_blocked_digest_is_refused_alone_and_its_sibling_publishes() {
    for fault in [None, Some(kernel::ArtifactDeletionFault::AfterCommit)] {
        let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
        let digest = published_digest(&handler, &project, "purged bytes").await;
        let store = kernel_store(&handler);
        delete_artifact(&store, &digest, kernel::ArtifactDeletionKind::Purge, fault);
        let (published, terminal, errors) = (
            count(&handler, "units_published"),
            count(&handler, "units_refused_terminal"),
            count(&handler, "kernel_error"),
        );
        let window = readmission_window("purged bytes");
        let end = capture_job(&handler, &project, SESSION, &window).await;
        assert_eq!(end, source_capture::Count::Completed, "{fault:?}");
        assert_eq!(
            count(&handler, "units_published") - published,
            1,
            "{fault:?}"
        );
        assert_eq!(
            count(&handler, "units_refused_terminal") - terminal,
            1,
            "{fault:?}"
        );
        assert_eq!(count(&handler, "kernel_error") - errors, 0, "{fault:?}");

        let skipped = count(&handler, "units_memo_skipped");
        let tip = store.tip().unwrap();
        let end = capture_job(&handler, &project, SESSION, &window).await;
        assert_eq!(end, source_capture::Count::Completed, "{fault:?}");
        assert_eq!(
            count(&handler, "units_memo_skipped") - skipped,
            2,
            "{fault:?}"
        );
        assert_eq!(store.tip().unwrap(), tip, "{fault:?}");
        let texts: Vec<String> = live(&store, OccurrenceClass::Messages)
            .iter()
            .map(|row| identity_of(row)[3].1.to_owned())
            .collect();
        assert!(texts.contains(&"msg_sibling".to_owned()), "{fault:?}");
        assert!(!texts.contains(&"msg_again".to_owned()), "{fault:?}");
    }
}

/// A purge that commits after screening and before the batch commit fails that one pass with the unnamed conflict; the next pass screens the unit out and publishes its sibling.
#[tokio::test]
async fn a_purge_between_screen_and_commit_fails_one_pass_then_converges() {
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    let digest = published_digest(&handler, &project, "raced bytes").await;
    let store = kernel_store(&handler);
    let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    *handler
        .source_capture
        .between_screen_and_commit
        .lock()
        .unwrap() = Some(Arc::new({
        let store = Arc::clone(&store);
        let fired = Arc::clone(&fired);
        move || {
            if !fired.swap(true, std::sync::atomic::Ordering::SeqCst) {
                delete_artifact(&store, &digest, kernel::ArtifactDeletionKind::Purge, None);
            }
        }
    }));
    let window = readmission_window("raced bytes");
    let (published, terminal, errors) = (
        count(&handler, "units_published"),
        count(&handler, "units_refused_terminal"),
        count(&handler, "kernel_error"),
    );
    let end = capture_job(&handler, &project, SESSION, &window).await;
    assert_eq!(end, source_capture::Count::KernelError);
    assert!(fired.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(count(&handler, "kernel_error") - errors, 1);
    assert_eq!(count(&handler, "units_published"), published);

    let end = capture_job(&handler, &project, SESSION, &window).await;
    assert_eq!(end, source_capture::Count::Completed);
    assert_eq!(count(&handler, "kernel_error") - errors, 1);
    assert_eq!(count(&handler, "units_published") - published, 1);
    assert_eq!(count(&handler, "units_refused_terminal") - terminal, 1);

    let skipped = count(&handler, "units_memo_skipped");
    let tip = store.tip().unwrap();
    let end = capture_job(&handler, &project, SESSION, &window).await;
    assert_eq!(end, source_capture::Count::Completed);
    assert_eq!(count(&handler, "units_memo_skipped") - skipped, 2);
    assert_eq!(store.tip().unwrap(), tip);
}

/// A unit whose descriptor receipt is stored replays it after its bytes are purged, with the memo empty as after a restart; the same bytes under a new identity are refused.
#[tokio::test]
async fn a_purged_unit_with_a_stored_receipt_replays() {
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    let digest = published_digest(&handler, &project, "replayed bytes").await;
    let store = kernel_store(&handler);
    delete_artifact(&store, &digest, kernel::ArtifactDeletionKind::Purge, None);
    handler.source_capture.forget_memo_for_test();
    let (replayed, terminal) = (
        count(&handler, "units_replayed"),
        count(&handler, "units_refused_terminal"),
    );
    let window = [opencode_user("msg_a", 1_700_000_000_000, "replayed bytes")];
    let end = capture_job(&handler, &project, SESSION, &window).await;
    assert_eq!(end, source_capture::Count::Completed);
    assert_eq!(count(&handler, "units_replayed") - replayed, 1);
    assert_eq!(count(&handler, "units_refused_terminal"), terminal);

    let window = [opencode_user("msg_b", 1_700_000_000_001, "replayed bytes")];
    let end = capture_job(&handler, &project, SESSION, &window).await;
    assert_eq!(end, source_capture::Count::Completed);
    assert_eq!(count(&handler, "units_refused_terminal") - terminal, 1);
}

/// A digest whose only state is a reclaiming reservation is not refused terminally: ingest refuses the batch retryably, the units stay pending, and they publish once reclamation finishes. A descriptor-cited digest is kept for replay, so the reclaimed bytes are stored as plain evidence.
#[tokio::test]
async fn a_reclaiming_digest_stays_pending_and_publishes_after_reclamation() {
    const DAY_MS: i64 = 24 * 60 * 60 * 1_000;
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    published_digest(&handler, &project, "an earlier unit").await;
    let store = kernel_store(&handler);
    let digest = store
        .ingest_artifact(kernel::ArtifactIngestRequest {
            intent: kernel::CommitIntent {
                producer: "capture-test".to_owned(),
                operation_key: "plain-evidence".to_owned(),
                request_digest: kernel::source_identity::payload_id(b"plain-evidence"),
                actor: "test".to_owned(),
                cause: "readmission".to_owned(),
            },
            payload: b"reclaimed bytes".to_vec(),
            evidence_id: "evidence-plain".to_owned(),
            object_id: "object-plain".to_owned(),
            object_kind: "evidence".to_owned(),
            domain_id: "harness-capture".to_owned(),
            source_kind: "repository".to_owned(),
            source_id: "src/plain".to_owned(),
            source_revision: 1,
            media_type: "text/plain".to_owned(),
            retention_class: "canonical".to_owned(),
            retain_until: None,
            asserted_sensitivity: kernel::Sensitivity::Normal,
            provider_egress: kernel::ProviderEgress::RemoteAllowed,
            provenance: None,
        })
        .unwrap()
        .digest;
    assert_eq!(
        digest,
        kernel::source_identity::payload_id(b"reclaimed bytes")
    );
    delete_artifact(&store, &digest, kernel::ArtifactDeletionKind::Delete, None);
    let after_grace = now_ms() + 16 * DAY_MS;
    assert!(
        store
            .run_staging_maintenance_with_fault_for_test(
                after_grace,
                kernel::ArtifactGcFault::AfterReclaiming,
            )
            .is_err()
    );
    let (_, blocked) = store
        .preview(Instant::now() + Duration::from_secs(5), |preview| {
            preview.artifact_readmission_blocked(&digest)
        })
        .unwrap();
    assert!(!blocked);
    let (published, terminal) = (
        count(&handler, "units_published"),
        count(&handler, "units_refused_terminal"),
    );
    let window = readmission_window("reclaimed bytes");
    let end = capture_job(&handler, &project, SESSION, &window).await;
    assert_eq!(end, source_capture::Count::KernelError);
    assert_eq!(count(&handler, "units_published"), published);
    assert_eq!(count(&handler, "units_refused_terminal"), terminal);

    store.run_staging_maintenance(after_grace).unwrap();
    let end = capture_job(&handler, &project, SESSION, &window).await;
    assert_eq!(end, source_capture::Count::Completed);
    assert_eq!(count(&handler, "units_published") - published, 2);
    assert_eq!(count(&handler, "units_refused_terminal"), terminal);
}

/// Commits every file of `root` as a new repository's only commit and returns `git:` and its id.
fn commit_repository(root: &Path) -> String {
    use super::source_corpus::git;
    git(root, &["init", "-q", "-b", "main"]).unwrap();
    git(root, &["add", "-A"]).unwrap();
    git(root, &["commit", "-q", "-m", "documents"]).unwrap();
    format!("git:{}", git(root, &["rev-parse", "HEAD"]).unwrap().trim())
}

/// One document pass at `root` on `route`, as the first pass of a new session there.
async fn document_pass_at(handler: &Handler, root: &Path, route: u16, base: &str) {
    let session = format!("ses-{route}-{base}");
    let mut bound = binding_with_harness(root.to_str().unwrap(), "opencode", &session);
    bound.config = capture_config(true);
    handler.bind_route(test_route(route), bound);
    let mut request = transform_with(&[opencode_user("msg_u1", 1_700_000_000_000, "hello")], base);
    request["session_id"] = json!(session);
    pass_on(handler, test_route(route), request).await;
    settle(handler).await;
}

/// The `(repository_id, revision)` of every live capture of `path`.
fn document_identities(store: &KernelStore, path: &str) -> Vec<(String, String)> {
    live(store, OccurrenceClass::RepositoryDocuments)
        .into_iter()
        .filter(|row| identity_of(row)[2] == ("relative_path", path))
        .map(|row| {
            (
                identity_of(&row)[1].1.to_owned(),
                row.detail.revision.clone(),
            )
        })
        .collect()
}

/// AE2 and `ri-memo-fixed-for-incarnation-and-restart-converges`: a document captured while the project was no repository keeps that identity for the incarnation; after a restart the derived identity retires the legacy row and publishes one row under `git:R`. The adapter refuses each pass's window message, whose session differs from the pass's, so the document is the only publication.
#[tokio::test]
async fn a_legacy_document_row_moves_to_the_derived_repository_identity_after_restart() {
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    std::fs::write(project.join("ARCHITECTURE.md"), "# Architecture\n").unwrap();
    document_pass(&handler, "base-1").await;
    let store = kernel_store(&handler);
    let project_id = identity_of(&live(&store, OccurrenceClass::RepositoryDocuments)[0])[0]
        .1
        .to_owned();
    assert_eq!(
        document_identities(&store, "ARCHITECTURE.md"),
        [(project_id.clone(), "1".to_owned())]
    );
    let repository = commit_repository(&project);

    let retired = count(&handler, "documents_retired");
    document_pass(&handler, "base-2").await;
    assert_eq!(count(&handler, "documents_retired"), retired);
    assert_eq!(
        document_identities(&store, "ARCHITECTURE.md"),
        [(project_id.clone(), "1".to_owned())]
    );

    handler.source_capture.forget_memo_for_test();
    let published = count(&handler, "units_published");
    document_pass(&handler, "base-3").await;
    assert_eq!(count(&handler, "documents_retired") - retired, 1);
    assert_eq!(count(&handler, "units_published") - published, 1);
    assert_eq!(
        document_identities(&store, "ARCHITECTURE.md"),
        [(repository.clone(), "1".to_owned())]
    );

    handler.source_capture.forget_memo_for_test();
    let unchanged = count(&handler, "documents_unchanged");
    document_pass(&handler, "base-4").await;
    assert_eq!(count(&handler, "documents_retired") - retired, 1);
    assert_eq!(count(&handler, "documents_unchanged") - unchanged, 1);
}

/// `ri-stale-identity-row-retired-before-new-revision`: a crash after the stale row retires and before the new lineage publishes leaves the path with no live row; the next pass publishes the new lineage's first revision and retires nothing.
#[tokio::test]
async fn a_crash_between_retire_and_publish_leaves_no_live_row_and_the_next_pass_publishes() {
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    std::fs::write(project.join("ARCHITECTURE.md"), "# Architecture\n").unwrap();
    document_pass(&handler, "base-1").await;
    let store = kernel_store(&handler);
    let repository = commit_repository(&project);
    handler.source_capture.forget_memo_for_test();
    *handler
        .source_capture
        .between_document_retire_and_publish
        .lock()
        .unwrap() = Some(Arc::new(|| {
        panic!("injected crash between retire and publish")
    }));
    document_pass(&handler, "base-2").await;
    assert_eq!(count(&handler, "worker_panicked"), 1);
    assert_eq!(document_identities(&store, "ARCHITECTURE.md"), []);

    *handler
        .source_capture
        .between_document_retire_and_publish
        .lock()
        .unwrap() = None;
    let retired = count(&handler, "documents_retired");
    document_pass(&handler, "base-3").await;
    assert_eq!(count(&handler, "documents_retired"), retired);
    assert_eq!(
        document_identities(&store, "ARCHITECTURE.md"),
        [(repository, "1".to_owned())]
    );
}

/// AE3: two worktrees of one repository bind two projects that share one repository identity, so each holds its own lineage and alternating passes commit nothing.
#[tokio::test]
async fn two_worktrees_share_a_repository_identity_and_keep_their_lineages() {
    use super::source_corpus::git;
    let (handler, dir, project) = capture_handler(capture_config(true), true).await;
    std::fs::write(project.join("ARCHITECTURE.md"), "# Architecture\n").unwrap();
    let repository = commit_repository(&project);
    let worktree = dir.path().join("worktree");
    git(
        &project,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "side",
            worktree.to_str().unwrap(),
        ],
    )
    .unwrap();
    document_pass_at(&handler, &project, 300, "base-1").await;
    document_pass_at(&handler, &worktree, 301, "base-1").await;
    let store = kernel_store(&handler);
    let rows: Vec<LiveDescriptor> = live(&store, OccurrenceClass::RepositoryDocuments);
    assert_eq!(rows.len(), 2);
    assert_ne!(identity_of(&rows[0])[0], identity_of(&rows[1])[0]);
    for row in &rows {
        assert_eq!(identity_of(row)[1], ("repository_id", repository.as_str()));
    }
    let tip = capture_tip(&handler);
    let retired = count(&handler, "documents_retired");
    for base in ["base-2", "base-3"] {
        document_pass_at(&handler, &project, 300, base).await;
        document_pass_at(&handler, &worktree, 301, base).await;
    }
    assert_eq!(count(&handler, "documents_retired"), retired);
    let revisions: Vec<(String, String)> = live(&store, OccurrenceClass::RepositoryDocuments)
        .iter()
        .map(|row| (row.object_id.clone(), row.detail.revision.clone()))
        .collect();
    assert_eq!(
        revisions,
        rows.iter()
            .map(|row| (row.object_id.clone(), row.detail.revision.clone()))
            .collect::<Vec<_>>()
    );
    assert_eq!(capture_tip(&handler), tip);
}

/// `ri-failed-derivation-refuses-pass-and-memoizes-nothing`: a derivation that cannot read an object or outlasts its share of the job's budget refuses both of the pass's document reads, retires and publishes no document, and memoizes nothing, so the next pass derives and publishes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_derivation_refuses_the_documents_and_the_next_pass_derives() {
    use super::source_corpus::git;
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    std::fs::write(project.join("ARCHITECTURE.md"), "# Architecture\n").unwrap();
    let repository = commit_repository(&project);
    let root = repository.strip_prefix("git:").unwrap();
    let object = project
        .join(".git/objects")
        .join(&root[..2])
        .join(&root[2..]);
    let saved = std::fs::read(&object).unwrap();
    std::fs::remove_file(&object).unwrap();
    let store = kernel_store(&handler);
    let refused = count(&handler, "documents_refused");
    document_pass(&handler, "base-1").await;
    assert_eq!(count(&handler, "documents_refused") - refused, 2);
    assert_eq!(document_identities(&store, "ARCHITECTURE.md"), []);
    std::fs::write(&object, saved).unwrap();
    assert!(git(&project, &["cat-file", "-e", root]).is_ok());

    *handler
        .source_capture
        .before_repository_derivation
        .lock()
        .unwrap() = Some(Arc::new(|| {
        std::thread::sleep(source_capture::REPOSITORY_WALK_BUDGET + Duration::from_millis(100));
    }));
    // The route's first pass of its own session composes the block, so it reads the documents, and its window message publishes in the window's commit after the walk's share of the budget ran out.
    let (deadlines, published) = (
        count(&handler, "deadline"),
        count(&handler, "units_published"),
    );
    pass(
        &handler,
        transform_with(
            &[opencode_user(
                "msg_window",
                1_700_000_000_000,
                "window text",
            )],
            "base-2",
        ),
    )
    .await;
    for _ in 0..1_000 {
        if handler.source_capture.is_idle() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(count(&handler, "deadline") - deadlines, 1);
    assert_eq!(count(&handler, "documents_refused") - refused, 4);
    assert_eq!(
        count(&handler, "units_published") - published,
        1,
        "{}",
        report(&handler)
    );
    assert_eq!(document_identities(&store, "ARCHITECTURE.md"), []);

    *handler
        .source_capture
        .before_repository_derivation
        .lock()
        .unwrap() = None;
    document_pass(&handler, "base-3").await;
    assert_eq!(count(&handler, "documents_refused") - refused, 4);
    assert_eq!(
        document_identities(&store, "ARCHITECTURE.md"),
        [(repository, "1".to_owned())]
    );
}

/// A shallow checkout falls back to the project on every pass; unshallowing it moves the document at the next incarnation, retiring the fallback row once.
#[tokio::test]
async fn a_shallow_checkout_falls_back_until_a_restart_after_unshallowing() {
    use super::source_corpus::git;
    let (handler, dir, project) = capture_handler(capture_config(true), true).await;
    let source = dir.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("ARCHITECTURE.md"), "# Architecture\n").unwrap();
    let repository = commit_repository(&source);
    std::fs::write(source.join("ARCHITECTURE.md"), "# Architecture\n\nTwo.\n").unwrap();
    git(&source, &["commit", "-q", "-am", "second"]).unwrap();
    let url = format!("file://{}", source.display());
    git(&project, &["init", "-q", "-b", "main"]).unwrap();
    git(&project, &["fetch", "-q", "--depth", "1", &url, "main"]).unwrap();
    git(&project, &["checkout", "-q", "FETCH_HEAD"]).unwrap();
    let store = kernel_store(&handler);
    document_pass(&handler, "base-1").await;
    let fallback = document_identities(&store, "ARCHITECTURE.md");
    assert_eq!(fallback.len(), 1);
    assert_ne!(fallback[0].0, repository);
    handler.source_capture.forget_memo_for_test();
    document_pass(&handler, "base-2").await;
    assert_eq!(document_identities(&store, "ARCHITECTURE.md"), fallback);

    git(&project, &["fetch", "-q", "--unshallow", &url, "main"]).unwrap();
    let retired = count(&handler, "documents_retired");
    document_pass(&handler, "base-3").await;
    assert_eq!(count(&handler, "documents_retired"), retired);
    assert_eq!(document_identities(&store, "ARCHITECTURE.md"), fallback);

    handler.source_capture.forget_memo_for_test();
    document_pass(&handler, "base-4").await;
    assert_eq!(count(&handler, "documents_retired") - retired, 1);
    assert_eq!(
        document_identities(&store, "ARCHITECTURE.md"),
        [(repository, "1".to_owned())]
    );
}

/// `ri-stale-identity-row-retired-before-new-revision`, the general case: a path holding live rows under two other repository identities retires both, each in its own commit, before its current lineage publishes.
#[tokio::test]
async fn every_stale_lineage_of_a_path_retires_before_the_current_one_publishes() {
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    std::fs::write(project.join("ARCHITECTURE.md"), "# Architecture\n").unwrap();
    document_pass(&handler, "base-1").await;
    let store = kernel_store(&handler);
    let project_id = identity_of(&live(&store, OccurrenceClass::RepositoryDocuments)[0])[0]
        .1
        .to_owned();
    let scope_id = format!("project:{project_id}");
    let publisher = crate::harness_sources::SourcePublisher {
        kernel: &store,
        domain_id: "harness-capture",
        scope_id: Some(&scope_id),
        egress: kernel::ProviderEgress::LocalOnly,
        sensitivity: kernel::Sensitivity::Sensitive,
    };
    let unit = crate::harness_sources::SourceUnit {
        class: OccurrenceClass::RepositoryDocuments,
        identity: vec![
            ("project_id", project_id.clone()),
            ("repository_id", "git:an-earlier-root".to_owned()),
            ("relative_path", "ARCHITECTURE.md".to_owned()),
        ],
        revision: "1".to_owned(),
        representation: crate::harness_sources::Representation::Text,
        text: "# Architecture\n".to_owned(),
        role: "document".to_owned(),
        outcome: None,
    };
    let published =
        crate::harness_sources::publish_screened(&publisher, &[unit], now_ms(), None).unwrap();
    assert!(matches!(
        published[0],
        crate::harness_sources::UnitDisposition::Published(_)
    ));
    assert_eq!(document_identities(&store, "ARCHITECTURE.md").len(), 2);
    let repository = commit_repository(&project);
    handler.source_capture.forget_memo_for_test();
    let retired = count(&handler, "documents_retired");
    document_pass(&handler, "base-2").await;
    assert_eq!(count(&handler, "documents_retired") - retired, 2);
    assert_eq!(
        document_identities(&store, "ARCHITECTURE.md"),
        [(repository, "1".to_owned())]
    );
}

fn docs_config(paths: &[&str]) -> DaemonConfig {
    let mut config = capture_config(true);
    config.project_doc_files = crate::project_docs::admit_document_paths(paths).unwrap();
    config
}

/// A configured list of three present files, one changed, publishes one revision and counts two unchanged, each row under its configured path; a path that leaves the list is neither read nor retired.
#[tokio::test]
async fn a_configured_document_list_captures_each_entry_under_its_configured_path() {
    let paths = ["docs/ARCHITECTURE.md", "notes/design/plan.md", "README.md"];
    let (handler, _dir, project) = capture_handler(docs_config(&paths), true).await;
    rebind(&handler, &project, docs_config(&paths));
    for path in paths {
        let file = project.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, format!("# {path}\n")).unwrap();
    }
    document_pass(&handler, "base-1").await;
    let store = kernel_store(&handler);
    let mut captured: Vec<String> = live(&store, OccurrenceClass::RepositoryDocuments)
        .iter()
        .map(|row| identity_of(row)[2].1.to_owned())
        .collect();
    captured.sort_unstable();
    assert_eq!(
        captured,
        ["README.md", "docs/ARCHITECTURE.md", "notes/design/plan.md"]
    );

    std::fs::write(project.join("README.md"), "# README\n\nChanged.\n").unwrap();
    let (published, unchanged) = (
        count(&handler, "units_published"),
        count(&handler, "documents_unchanged"),
    );
    document_pass(&handler, "base-2").await;
    assert_eq!(count(&handler, "units_published") - published, 1);
    assert_eq!(count(&handler, "documents_unchanged") - unchanged, 2);
    assert_eq!(
        document_identities(&store, "README.md")
            .into_iter()
            .map(|(_, revision)| revision)
            .collect::<Vec<_>>(),
        ["2"]
    );

    rebind(&handler, &project, docs_config(&paths[..2]));
    std::fs::remove_file(project.join("README.md")).unwrap();
    let (retired, missing, tip) = (
        count(&handler, "documents_retired"),
        count(&handler, "documents_missing"),
        capture_tip(&handler),
    );
    document_pass(&handler, "base-3").await;
    assert_eq!(count(&handler, "documents_retired"), retired);
    assert_eq!(count(&handler, "documents_missing"), missing);
    assert_eq!(capture_tip(&handler), tip);
    assert_eq!(document_identities(&store, "README.md").len(), 1);
}

fn nested_path_of(bytes: usize) -> String {
    let mut path = String::new();
    while path.len() + 201 < bytes {
        path.push_str(&"d".repeat(200));
        path.push('/');
    }
    path.push_str(&"f".repeat(bytes - path.len() - 3));
    path.push_str(".md");
    path
}

#[tokio::test]
async fn the_longest_admissible_path_captures_beside_a_sibling() {
    let long = nested_path_of(kernel::source_identity::MAX_IDENTITY_VALUE_BYTES);
    assert_eq!(
        long.len(),
        kernel::source_identity::MAX_IDENTITY_VALUE_BYTES
    );
    let paths = ["ARCHITECTURE.md", long.as_str()];
    let (handler, _dir, project) = capture_handler(docs_config(&paths), true).await;
    rebind(&handler, &project, docs_config(&paths));
    for path in paths {
        let file = project.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "# document\n").unwrap();
    }
    let refused = count(&handler, "documents_refused");
    document_pass(&handler, "base-1").await;
    let store = kernel_store(&handler);
    assert_eq!(count(&handler, "documents_refused"), refused);
    assert_eq!(document_identities(&store, "ARCHITECTURE.md").len(), 1);
    assert_eq!(document_identities(&store, &long).len(), 1);
}

/// A configured path whose parent is a symlink out of the root is unreadable and captures nothing; a nested parent that vanishes is unreadable and leaves the live row live.
#[cfg(unix)]
#[tokio::test]
async fn a_nested_document_through_a_symlink_or_an_absent_parent_is_unreadable() {
    let paths = ["docs/A.md", "notes/B.md"];
    let (handler, dir, project) = capture_handler(docs_config(&paths), true).await;
    rebind(&handler, &project, docs_config(&paths));
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("A.md"), "OUTSIDE").unwrap();
    std::os::unix::fs::symlink(&outside, project.join("docs")).unwrap();
    std::fs::create_dir_all(project.join("notes")).unwrap();
    std::fs::write(project.join("notes/B.md"), "# B\n").unwrap();
    let unreadable = count(&handler, "documents_unreadable");
    document_pass(&handler, "base-1").await;
    let store = kernel_store(&handler);
    assert_eq!(count(&handler, "documents_unreadable") - unreadable, 1);
    assert_eq!(document_identities(&store, "docs/A.md"), []);
    assert_eq!(document_identities(&store, "notes/B.md").len(), 1);

    std::fs::remove_dir_all(project.join("notes")).unwrap();
    let retired = count(&handler, "documents_retired");
    document_pass(&handler, "base-2").await;
    assert_eq!(count(&handler, "documents_unreadable") - unreadable, 3);
    assert_eq!(count(&handler, "documents_retired"), retired);
    assert_eq!(document_identities(&store, "notes/B.md").len(), 1);
}

#[tokio::test]
async fn the_stale_identity_inventory_runs_once_per_path_and_incarnation() {
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    std::fs::write(project.join("ARCHITECTURE.md"), "# Architecture\n").unwrap();
    let repository = commit_repository(&project);
    let sweeps = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = Arc::clone(&sweeps);
    *handler.source_capture.before_stale_sweep.lock().unwrap() = Some(Arc::new(move || {
        counted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }));
    let store = kernel_store(&handler);
    document_pass(&handler, "base-1").await;
    assert_eq!(sweeps.load(std::sync::atomic::Ordering::Relaxed), 1);
    let unchanged = count(&handler, "documents_unchanged");
    for base in ["base-2", "base-3"] {
        document_pass(&handler, base).await;
    }
    assert_eq!(count(&handler, "documents_unchanged") - unchanged, 2);
    assert_eq!(sweeps.load(std::sync::atomic::Ordering::Relaxed), 1);

    handler.source_capture.forget_memo_for_test();
    document_pass(&handler, "base-4").await;
    assert_eq!(sweeps.load(std::sync::atomic::Ordering::Relaxed), 2);
    assert_eq!(
        document_identities(&store, "ARCHITECTURE.md"),
        [(repository, "1".to_owned())]
    );
}

#[tokio::test]
async fn a_project_keyed_row_beside_the_derived_row_retires_at_the_next_incarnation() {
    let (handler, _dir, project) = capture_handler(capture_config(true), true).await;
    std::fs::write(project.join("ARCHITECTURE.md"), "# Architecture\n").unwrap();
    document_pass(&handler, "base-1").await;
    let store = kernel_store(&handler);
    let project_id = identity_of(&live(&store, OccurrenceClass::RepositoryDocuments)[0])[0]
        .1
        .to_owned();
    let repository = commit_repository(&project);
    handler.source_capture.forget_memo_for_test();
    document_pass(&handler, "base-2").await;
    assert_eq!(
        document_identities(&store, "ARCHITECTURE.md"),
        [(repository.clone(), "1".to_owned())]
    );

    let edited = "# Architecture\n\nEdited.\n";
    std::fs::write(project.join("ARCHITECTURE.md"), edited).unwrap();
    let scope_id = format!("project:{project_id}");
    let publisher = crate::harness_sources::SourcePublisher {
        kernel: &store,
        domain_id: "harness-capture",
        scope_id: Some(&scope_id),
        egress: kernel::ProviderEgress::LocalOnly,
        sensitivity: kernel::Sensitivity::Sensitive,
    };
    let unit = crate::harness_sources::SourceUnit {
        class: OccurrenceClass::RepositoryDocuments,
        identity: vec![
            ("project_id", project_id.clone()),
            ("repository_id", project_id.clone()),
            ("relative_path", "ARCHITECTURE.md".to_owned()),
        ],
        revision: "2".to_owned(),
        representation: crate::harness_sources::Representation::Text,
        text: edited.to_owned(),
        role: "document".to_owned(),
        outcome: None,
    };
    let published =
        crate::harness_sources::publish_screened(&publisher, &[unit], now_ms(), None).unwrap();
    assert!(matches!(
        published[0],
        crate::harness_sources::UnitDisposition::Published(_)
    ));
    let mut both = document_identities(&store, "ARCHITECTURE.md");
    both.sort_unstable();
    let mut expected = vec![
        (repository.clone(), "1".to_owned()),
        (project_id, "2".to_owned()),
    ];
    expected.sort_unstable();
    assert_eq!(both, expected);

    handler.source_capture.forget_memo_for_test();
    let (retired, units) = (
        count(&handler, "documents_retired"),
        count(&handler, "units_published"),
    );
    document_pass(&handler, "base-3").await;
    assert_eq!(count(&handler, "documents_retired") - retired, 1);
    assert_eq!(count(&handler, "units_published") - units, 1);
    let rows = live(&store, OccurrenceClass::RepositoryDocuments);
    assert_eq!(
        document_identities(&store, "ARCHITECTURE.md"),
        [(repository, "2".to_owned())]
    );
    assert_eq!(
        rows[0].detail.artifact_digest,
        crate::project_docs::sha256_hex(edited.as_bytes())
    );
}

/// The configuration the citation tests submit capture work under: memory
/// capture on, and source capture as given.
fn citing_config(source_capture: bool) -> DaemonConfig {
    let mut config = capture_config(source_capture);
    config.memory_auto_capture = true;
    config
}

fn bind_pi(handler: &Handler, project: &Path, config: DaemonConfig) {
    let mut bound = binding_with_harness(project.to_str().unwrap(), "pi", SESSION);
    bound.config = config;
    handler.bind_route(test_route(7), bound);
}

/// Captures `messages` as `(id, text)` user sources and submits one memory per
/// source quoting `quotes[i]`; returns the submission's state.
async fn capture_quotes(handler: &Handler, messages: &[(&str, &str)], quotes: &[&str]) -> Value {
    let submission = prepare_submission(handler, test_route(7), SESSION, messages, quotes).await;
    submit_state(handler, test_route(7), &submission).await
}

/// Captures `messages` on `route` and leases them; returns the submission
/// that answers with one memory per source quoting `quotes[i]`.
async fn prepare_submission(
    handler: &Handler,
    route: RouteHandle,
    session: &str,
    messages: &[(&str, &str)],
    quotes: &[&str],
) -> Value {
    let request = json!({"method":"memory.capture","session_id":session,"messages":messages.iter().map(|(id, text)| json!({"id":id,"role":"user","text":text})).collect::<Vec<_>>()});
    assert_eq!(
        tool_body(handler.handle_memory_capture(route, &request).await)["state"],
        "accepted"
    );
    let next =
        json!({"method":"memory.capture.next","session_id":session,"model":"custom-native/model"});
    let work = tool_body(handler.handle_native_capture_next(route, &next).await);
    assert_eq!(work["state"], "work", "{work}");
    let submission = submission_for(&work, session, quotes);
    let output: Value = serde_json::from_str(submission["output"].as_str().unwrap()).unwrap();
    assert!(
        output["decisions"].as_array().unwrap().len() >= messages.len(),
        "{output}"
    );
    submission
}

/// Answers a leased batch: each message quotes the first quote it contains,
/// and a message that contains none answers with no memory. The lease spans
/// the project's pending jobs, so a pass's auto-captured jobs may share it.
fn submission_for(work: &Value, session: &str, quotes: &[&str]) -> Value {
    let prompt: Value = serde_json::from_str(work["prompt"].as_str().unwrap()).unwrap();
    let decisions: Vec<Value> = prompt["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| {
            let text = message["text"].as_str().unwrap();
            let memories: Vec<Value> = quotes
                .iter()
                .find(|quote| text.contains(*quote))
                .map(|quote| json!({"category":"CONFIG_VALUES","content":quote,"quote":quote}))
                .into_iter()
                .collect();
            json!({"message_id":message["id"],"memories":memories})
        })
        .collect();
    let output = json!({"version":1,"decisions":decisions});
    json!({"method":"memory.capture.submit","session_id":session,"lease":work["lease"],"model":work["model"],"output":output.to_string()})
}

/// One committed capture decision as the Kernel stores it.
#[derive(Debug)]
struct CapturedDecision {
    decision_id: String,
    object_id: String,
    anchor_id: Option<String>,
    evidence_id: Option<String>,
    sensitivity: String,
    rationale: Value,
    events: Vec<(String, Option<String>)>,
}

fn kernel_db(dir: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open_with_flags(
        dir.join("kernel/kernel.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
}

/// Every live capture decision keyed by the message its rationale names.
fn captured_decisions(dir: &Path) -> BTreeMap<String, CapturedDecision> {
    let db = kernel_db(dir);
    let mut statement = db
        .prepare(
            "SELECT d.decision_id,d.object_id,d.anchor_id,d.evidence_id,d.sensitivity_class,
                    d.decision_payload
             FROM decisions d JOIN object_registry r ON r.object_id=d.object_id
             WHERE r.source_id='memory_capture' AND d.invalidated_commit_seq IS NULL",
        )
        .unwrap();
    let rows: Vec<CapturedDecision> = statement
        .query_map([], |row| {
            let payload: Vec<u8> = row.get(5)?;
            let payload: Value = serde_json::from_slice(&payload).unwrap();
            Ok(CapturedDecision {
                decision_id: row.get(0)?,
                object_id: row.get(1)?,
                anchor_id: row.get(2)?,
                evidence_id: row.get(3)?,
                sensitivity: row.get(4)?,
                rationale: serde_json::from_str(payload["rationale"].as_str().unwrap()).unwrap(),
                events: Vec::new(),
            })
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    rows.into_iter()
        .map(|mut decision| {
            decision.events = db
                .prepare(
                    "SELECT event_kind,evidence_id FROM decision_events
                     WHERE decision_id=?1 ORDER BY event_ordinal",
                )
                .unwrap()
                .query_map([&decision.decision_id], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            (
                decision.rationale["message"].as_str().unwrap().to_owned(),
                decision,
            )
        })
        .collect()
}

/// The oid of each anchor row, by anchor id.
fn anchor_oids(dir: &Path) -> BTreeMap<String, String> {
    kernel_db(dir)
        .prepare("SELECT anchor_id,reachable_from_oid FROM anchors")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// The evidence id of each live `messages` descriptor, by message id and block index.
fn message_evidence(store: &KernelStore) -> BTreeMap<(String, usize), String> {
    live(store, OccurrenceClass::Messages)
        .iter()
        .map(|row| {
            let identity = identity_of(row);
            let field = |name: &str| {
                identity
                    .iter()
                    .find(|(field, _)| *field == name)
                    .unwrap()
                    .1
                    .to_owned()
            };
            (
                (field("message_id"), field("block_index").parse().unwrap()),
                row.detail.evidence_id.clone(),
            )
        })
        .collect()
}

fn counter(handler: &Handler, name: &str) -> u64 {
    handler.source_capture.report(false, 0)[name]
        .as_u64()
        .unwrap()
}

/// A secret source capture refuses to retain, so its block has no descriptor.
const CITED_SECRET: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678";
const CITED_SOURCES: [(&str, &str); 6] = [
    ("pi-one", "Use port 4321 for staging."),
    (
        "pi-four",
        "The build uses cargo.\nTests run on nextest.\nDeploys go to region us-west-2.\nLogs ship to the collector.",
    ),
    ("pi-span", "The cache lives in\nthe shared volume."),
    ("pi-edge", "Retries are capped.\nBackoff doubles each time."),
    (
        "pi-first",
        concat!(
            "Rotate the key sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678.",
            "\nThe key rotates weekly."
        ),
    ),
    ("pi-absent", "The staging host is blue."),
];
const CITED_QUOTES: [&str; 6] = [
    "Use port 4321 for staging.",
    "Deploys go to region us-west-2.",
    "lives in\nthe shared",
    "\nBackoff doubles",
    "The key rotates weekly.",
    "The staging host is blue.",
];

/// Publishes every source but the last as a Pi window of text items, one
/// item per `\n`-separated line, and returns the HEAD commit of the project
/// repository.
async fn publish_cited_sources(handler: &Handler, project: &Path) -> String {
    let head = crate::memory_capture::head_fixtures::repository(project, 1).remove(0);
    assert!(CITED_SOURCES[4].1.contains(CITED_SECRET));
    let rows: Vec<Value> = CITED_SOURCES[..5]
        .iter()
        .enumerate()
        .map(|(index, (id, text))| {
            let content: Vec<Value> = text
                .split('\n')
                .map(|item| json!({"type":"text","text":item}))
                .collect();
            pi_row(
                id,
                None,
                json!({"role":"user","content":content,"timestamp":1_700_000_000_000u64 + index as u64}),
            )
        })
        .collect();
    bind_pi(handler, project, capture_config(true));
    pass(handler, pi_request(&rows, "base-1")).await;
    settle(handler).await;
    bind_pi(handler, project, citing_config(true));
    head
}

/// AC3 (anchor), AC4, AC11, and the counters: four published messages cite
/// by `block`, `block` in the third of four items, `span`, and `first`; an
/// unpublished sibling reports `descriptor_not_found`. Every decision of a job
/// names the job's anchor at the submit-time HEAD, and no admission or class
/// differs between cited and uncited decisions.
#[tokio::test]
async fn captured_memories_cite_the_blocks_holding_their_quotes_and_anchor_at_head() {
    let (handler, dir, project) = capture_handler(citing_config(true), true).await;
    let head = publish_cited_sources(&handler, &project).await;
    let store = kernel_store(&handler);
    let evidence = message_evidence(&store);
    assert_eq!(
        evidence.len(),
        1 + 4 + 2 + 2 + 1,
        "the secret block has no descriptor"
    );
    assert_eq!(
        capture_quotes(&handler, &CITED_SOURCES, &CITED_QUOTES).await,
        "processed"
    );
    let decisions = captured_decisions(dir.path());
    assert_eq!(decisions.len(), 6);
    let cited = |message: &str, rule: &str, blocks: &[usize]| {
        let decision = &decisions[message];
        let expected: Vec<String> = blocks
            .iter()
            .map(|block| evidence[&(message.to_owned(), *block)].clone())
            .collect();
        assert_eq!(decision.rationale["citation"], "cited", "{message}");
        assert_eq!(decision.rationale["citation_block"], rule, "{message}");
        assert_eq!(
            decision.rationale["citation_blocks"],
            json!(blocks),
            "{message}"
        );
        assert_eq!(
            decision.rationale["citation_evidence"],
            json!(expected),
            "{message}"
        );
        assert_eq!(
            decision.events,
            expected
                .into_iter()
                .map(|id| (
                    crate::memory_capture::CITATION_EVENT_KIND.to_owned(),
                    Some(id)
                ))
                .collect::<Vec<_>>(),
            "{message}"
        );
    };
    cited("pi-one", "block", &[0]);
    cited("pi-four", "block", &[2]);
    cited("pi-span", "span", &[0, 1]);
    cited("pi-edge", "span", &[0, 1]);
    cited("pi-first", "first", &[1]);
    let absent = &decisions["pi-absent"];
    assert_eq!(absent.rationale["citation"], "descriptor_not_found");
    assert!(absent.events.is_empty());
    assert!(absent.rationale.get("citation_evidence").is_none());

    let anchors = anchor_oids(dir.path());
    assert_eq!(anchors.len(), 6, "one anchor per job");
    for decision in decisions.values() {
        assert_eq!(decision.rationale["anchor"], "anchored");
        assert_eq!(anchors[decision.anchor_id.as_ref().unwrap()], head);
        assert_eq!(decision.evidence_id, None);
        assert_eq!(decision.sensitivity, "normal");
    }
    let admissions: Vec<(String, String, String, String, i64)> = kernel_db(dir.path())
        .prepare(
            "SELECT a.source_class,a.taint_class,a.maturity,a.effective_maturity,a.elevated_support
             FROM admission_decisions a JOIN object_registry r ON r.object_id=a.subject_object_id
             WHERE r.source_id='memory_capture' AND r.object_kind='decision'",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(admissions.len(), 6, "{admissions:?}");
    assert!(
        admissions
            .iter()
            .all(|admission| *admission == admissions[0]),
        "{admissions:?}"
    );
    let (source, taint, ..) = admissions[0].clone();
    assert_eq!(
        (source.as_str(), taint.as_str()),
        ("model_inference", "assistant_inference")
    );
    let object_ids: Vec<&str> = decisions
        .values()
        .map(|decision| decision.object_id.as_str())
        .collect();
    let served = store
        .commit(
            kernel::CommitIntent {
                producer: "citation-test".into(),
                operation_key: "served".into(),
                request_digest: "0".repeat(64),
                actor: "test".into(),
                cause: "read".into(),
            },
            |envelope| {
                let rows = envelope.served_rows_for(&object_ids, None)?;
                let visibility: BTreeSet<String> = rows
                    .values()
                    .map(|row| {
                        format!(
                            "{:?}/{:?}/{:?}",
                            row.visibility(Surface::AutoInject),
                            row.visibility(Surface::ExplicitSearch),
                            row.object.sensitivity
                        )
                    })
                    .collect();
                Ok(serde_json::to_string(&visibility).unwrap())
            },
        )
        .unwrap();
    let visibility: BTreeSet<String> = serde_json::from_str(&served.result).unwrap();
    assert_eq!(
        visibility.into_iter().collect::<Vec<_>>(),
        vec!["Hidden/Labeled/Normal".to_owned()],
        "cited and uncited memories serve alike"
    );

    assert_eq!(counter(&handler, "decisions_cited"), 5);
    assert_eq!(counter(&handler, "decisions_descriptor_not_found"), 1);
    assert_eq!(counter(&handler, "decisions_anchored"), 6);
    for name in [
        "decisions_source_capture_off",
        "decisions_source_id_unjoinable",
        "decisions_publish_budget_expired",
        "decisions_head_unavailable",
    ] {
        assert_eq!(counter(&handler, name), 0, "{name}");
    }
}

/// AC6, AC7, and the hashed-id reason: with source capture off no lookup
/// cites; a hashed source id reports `source_id_unjoinable`; a plain
/// directory and an unborn repository write no anchor and count
/// `head_unavailable`; a detached HEAD anchors at its commit.
#[tokio::test]
async fn misses_carry_one_closed_reason_each_and_the_counters_follow_them() {
    let hashed = format!("{}:0", "c".repeat(64));
    for (fixture, source_capture) in [
        ("plain", false),
        ("unborn", true),
        ("sha256", true),
        ("detached", true),
    ] {
        let (handler, dir, project) = capture_handler(citing_config(source_capture), true).await;
        let expected_oid = match fixture {
            "unborn" => {
                gix::init(&project).unwrap();
                None
            }
            "sha256" => {
                crate::memory_capture::head_fixtures::repository(&project, 1);
                crate::memory_capture::head_fixtures::declare_sha256(&project);
                None
            }
            "detached" => {
                let first = crate::memory_capture::head_fixtures::repository(&project, 2).remove(0);
                crate::memory_capture::head_fixtures::detach(&project, &first);
                Some(first)
            }
            _ => None,
        };
        bind_pi(&handler, &project, citing_config(source_capture));
        let sources = [
            ("native-user-1", "Use port 4321 for staging."),
            (hashed.as_str(), "Production listens on 8080."),
        ];
        let quotes = ["Use port 4321 for staging.", "Production listens on 8080."];
        assert_eq!(
            capture_quotes(&handler, &sources, &quotes).await,
            "processed",
            "{fixture}"
        );
        let decisions = captured_decisions(dir.path());
        let reason = |message: &str| decisions[message].rationale["citation"].clone();
        if source_capture {
            assert_eq!(reason("native-user-1"), "descriptor_not_found", "{fixture}");
            assert_eq!(reason(&hashed), "source_id_unjoinable", "{fixture}");
            assert_eq!(counter(&handler, "decisions_source_id_unjoinable"), 1);
        } else {
            assert_eq!(reason("native-user-1"), "source_capture_off");
            assert_eq!(reason(&hashed), "source_capture_off");
            assert_eq!(counter(&handler, "decisions_source_capture_off"), 2);
        }
        for decision in decisions.values() {
            assert!(decision.events.is_empty(), "{fixture}");
            match &expected_oid {
                Some(oid) => {
                    assert_eq!(decision.rationale["anchor"], "anchored");
                    assert_eq!(
                        &anchor_oids(dir.path())[decision.anchor_id.as_ref().unwrap()],
                        oid
                    );
                }
                None => {
                    assert_eq!(
                        decision.rationale["anchor"], "head_unavailable",
                        "{fixture}"
                    );
                    assert_eq!(decision.anchor_id, None);
                }
            }
        }
        let (anchored, unavailable) = if expected_oid.is_some() {
            (2, 0)
        } else {
            (0, 2)
        };
        assert_eq!(
            counter(&handler, "decisions_anchored"),
            anchored,
            "{fixture}"
        );
        assert_eq!(
            counter(&handler, "decisions_head_unavailable"),
            unavailable,
            "{fixture}"
        );
        if expected_oid.is_none() {
            assert!(anchor_oids(dir.path()).is_empty(), "{fixture}");
        }
    }
}

/// AC5: an output naming a source outside the batch is refused before any
/// publish, so no lookup, anchor, or decision runs for it.
#[tokio::test]
async fn an_invented_source_id_is_refused_before_any_citation_lookup() {
    let (handler, dir, project) = capture_handler(citing_config(true), true).await;
    publish_cited_sources(&handler, &project).await;
    let request = json!({"method":"memory.capture","session_id":SESSION,"messages":[{"id":"pi-one","role":"user","text":CITED_SOURCES[0].1}]});
    assert_eq!(
        tool_body(handler.handle_memory_capture(test_route(7), &request).await)["state"],
        "accepted"
    );
    let next =
        json!({"method":"memory.capture.next","session_id":SESSION,"model":"custom-native/model"});
    let work = tool_body(
        handler
            .handle_native_capture_next(test_route(7), &next)
            .await,
    );
    assert_eq!(work["state"], "work", "{work}");
    // The prompt names its sources `source_1` and on; the native id is not one of them.
    let output = json!({"version":1,"decisions":[{"message_id":"pi-one","memories":[{"category":"CONFIG_VALUES","content":"x","quote":"Use port"}]}]});
    let submission = json!({"method":"memory.capture.submit","session_id":SESSION,"lease":work["lease"],"model":work["model"],"output":output.to_string()});
    assert_eq!(
        tool_body(
            handler
                .handle_native_capture_submit(test_route(7), &submission)
                .await
        )["state"],
        "processed",
        "a refused output settles its jobs as failed extractions"
    );
    let key = kernel_routes::ProjectBinding::new(&project)
        .digest()
        .to_owned();
    let status = handler
        .store()
        .unwrap()
        .memory_capture_status(&key)
        .unwrap();
    assert_eq!(status.completed, 0, "{status:?}");
    assert!(captured_decisions(dir.path()).is_empty());
    assert!(anchor_oids(dir.path()).is_empty());
    assert_eq!(counter(&handler, "decisions_anchored"), 0);
}

/// AC12: the cited message's `harness_capture` evidence survives the expiry
/// sweep past its retention deadline while the citing decision is live, and
/// retiring it is refused.
#[tokio::test]
async fn cited_capture_evidence_survives_expiry_and_refuses_retirement() {
    let (handler, dir, project) = capture_handler(citing_config(true), true).await;
    publish_cited_sources(&handler, &project).await;
    assert_eq!(
        capture_quotes(&handler, &CITED_SOURCES[..1], &CITED_QUOTES[..1]).await,
        "processed"
    );
    let decisions = captured_decisions(dir.path());
    let cited = decisions["pi-one"].events[0].1.clone().unwrap();
    let store = kernel_store(&handler);
    store.expire_harness_captures(i64::MAX / 2).unwrap();
    let db = kernel_db(dir.path());
    let (object_id, live): (String, bool) = db
        .query_row(
            "SELECT object_id,invalidated_commit_seq IS NULL FROM evidence_meta WHERE evidence_id=?1",
            [&cited],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(live, "a cited capture outlives its retention deadline");
    let refused = store
        .commit(
            kernel::CommitIntent {
                producer: "citation-test".into(),
                operation_key: "retire".into(),
                request_digest: "1".repeat(64),
                actor: "test".into(),
                cause: "retire".into(),
            },
            |envelope| Ok(envelope.retire_evidence(&object_id)?.result_json()),
        )
        .unwrap_err();
    assert_eq!(refused, kernel::KernelError::Conflict);
}

/// AC13 and the replay clause of the counters: a publish whose completion the
/// store refuses replays its receipt on the retry, leaving one anchor row per
/// job, one decision per memory, the same events, and the counters unchanged.
#[tokio::test]
async fn a_replayed_publish_adds_no_rows_and_no_counts() {
    let (handler, dir, project) = capture_handler(citing_config(true), true).await;
    publish_cited_sources(&handler, &project).await;
    let store = handler.store().unwrap();
    store
        .execute_tag_sql_for_test(
            "CREATE TRIGGER hold_plans BEFORE UPDATE ON memory_capture_jobs
             WHEN NEW.commit_seq IS NOT NULL BEGIN SELECT RAISE(ABORT,'held'); END",
        )
        .unwrap();
    assert_eq!(
        capture_quotes(&handler, &CITED_SOURCES[..2], &CITED_QUOTES[..2]).await,
        "processed"
    );
    let first = format!("{:?}", captured_decisions(dir.path()));
    let anchors = anchor_oids(dir.path());
    let cited = counter(&handler, "decisions_cited");
    assert_eq!(cited, 2);
    store
        .execute_tag_sql_for_test(
            "DROP TRIGGER hold_plans; UPDATE memory_capture_jobs SET retry_at_ms=0",
        )
        .unwrap();
    let next =
        json!({"method":"memory.capture.next","session_id":SESSION,"model":"custom-native/model"});
    assert_eq!(
        tool_body(
            handler
                .handle_native_capture_next(test_route(7), &next)
                .await
        )["state"],
        "ready"
    );
    assert_eq!(format!("{:?}", captured_decisions(dir.path())), first);
    assert_eq!(anchor_oids(dir.path()), anchors);
    assert_eq!(counter(&handler, "decisions_cited"), cited);
    assert_eq!(counter(&handler, "decisions_anchored"), 2);
}

/// The settlement key of a Pi session's source publish.
fn pi_key(project: &Path, session: &str) -> crate::harness_sources::SessionIdentity {
    crate::harness_sources::SessionIdentity {
        project_id: kernel_routes::ProjectBinding::new(project)
            .digest()
            .to_owned(),
        harness: crate::harness_sources::Harness::Pi,
        session_id: session.to_owned(),
    }
}

fn settlement(handler: &Handler, project: &Path, session: &str) -> Option<Settlement> {
    handler
        .source_capture
        .settlement_for_test(&pi_key(project, session))
}

/// Submits that armed a wait on a `Running` entry.
fn waits(handler: &Handler) -> u64 {
    handler
        .source_capture
        .settlement_waits
        .load(Ordering::Relaxed)
}

/// Polls until `waits` reaches `count`, so an assertion about a parked
/// submit runs only once it is parked.
async fn await_waits(handler: &Handler, count: u64) {
    for _ in 0..1_000 {
        if waits(handler) >= count {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no submit armed wait {count}");
}

async fn submit_state(handler: &Handler, route: RouteHandle, submission: &Value) -> Value {
    tool_body(
        handler
            .handle_native_capture_submit(route, submission)
            .await,
    )["state"]
        .clone()
}

fn spawn_submit(
    handler: &Arc<Handler>,
    route: RouteHandle,
    submission: Value,
) -> tokio::task::JoinHandle<Value> {
    let handler = Arc::clone(handler);
    tokio::spawn(async move { submit_state(&handler, route, &submission).await })
}

/// A Pi user message row; each test's sources carry their own ids, so the
/// decisions map keys stay distinct across sessions.
fn user_row(id: &str, text: &str, offset: u64) -> Value {
    pi_row(
        id,
        None,
        json!({"role":"user","content":text,"timestamp":1_700_000_000_000u64 + offset}),
    )
}

const SOURCE: (&str, &str) = ("pi-settled", "Use port 4321 for staging.");
const OTHER: (&str, &str) = ("pi-other", "Retry the deploy after 90 seconds.");

/// Parks the next source publish inside its job, before the capture commit
/// gate; returns a receiver signalled when the job parks and a sender that
/// releases it.
fn park_source_publish(handler: &Handler) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (entered_tx, entered_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let go_rx = Arc::new(Mutex::new(go_rx));
    let entered_tx = Arc::new(Mutex::new(entered_tx));
    *handler.source_capture.before_publish.lock().unwrap() = Some(Arc::new(move || {
        entered_tx.lock().unwrap().send(()).unwrap();
        let _ = go_rx.lock().unwrap().recv();
    }));
    (entered_rx, go_tx)
}

async fn wait_parked(entered: mpsc::Receiver<()>) {
    tokio::task::spawn_blocking(move || {
        entered
            .recv_timeout(Duration::from_secs(30))
            .expect("the source job parks before it ends")
    })
    .await
    .unwrap();
}

/// A handler whose project is a one-commit repository, bound for Pi capture
/// with source capture on.
async fn settling_handler() -> (Arc<Handler>, tempfile::TempDir, PathBuf) {
    let (handler, dir, project) = capture_handler(citing_config(true), true).await;
    crate::memory_capture::head_fixtures::repository(&project, 1);
    bind_pi(&handler, &project, citing_config(true));
    (Arc::new(handler), dir, project)
}

/// Runs a pass of `SOURCE` on `SESSION` whose source job parks before the
/// gate; returns the release sender.
async fn parked_pass(handler: &Handler) -> mpsc::Sender<()> {
    let (entered, go) = park_source_publish(handler);
    pass(
        handler,
        pi_request(&[user_row(SOURCE.0, SOURCE.1, 0)], "base-1"),
    )
    .await;
    wait_parked(entered).await;
    go
}

/// AC8: a submit after the pass's publish ended arms no wait and cites.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_submit_after_the_publish_ended_arms_no_wait_and_cites() {
    let (handler, dir, project) = settling_handler().await;
    pass(
        &handler,
        pi_request(&[user_row(SOURCE.0, SOURCE.1, 0)], "base-1"),
    )
    .await;
    settle(&handler).await;
    assert_eq!(
        settlement(&handler, &project, SESSION),
        Some(Settlement::Ended {
            outcome: Count::Completed
        })
    );
    assert_eq!(
        capture_quotes(&handler, &[SOURCE], &[SOURCE.1]).await,
        "processed"
    );
    assert_eq!(waits(&handler), 0);
    assert_eq!(
        captured_decisions(dir.path())[SOURCE.0].rationale["citation"],
        "cited"
    );
}

/// AC8 and AC9 on the paused tokio clock: a submit after the publish ended
/// takes zero virtual time; a submit against a parked publish is pending one
/// millisecond before `accepted_at + 5 s`, runs no lookup, and is released
/// exactly at the deadline with `publish_budget_expired`.
#[tokio::test(start_paused = true)]
async fn on_the_paused_clock_the_wait_is_zero_when_settled_and_ends_exactly_at_the_deadline() {
    let (handler, dir, project) = settling_handler().await;
    // Each job's std budget starts from its tokio acceptance instant, which
    // the paused clock places this far past the test's real start, so a slow
    // setup cannot spend a budget before its job runs.
    tokio::time::advance(Duration::from_secs(120)).await;
    pass(
        &handler,
        pi_request(&[user_row(OTHER.0, OTHER.1, 0)], "base-0"),
    )
    .await;
    settle(&handler).await;
    let started = tokio::time::Instant::now();
    assert_eq!(
        capture_quotes(&handler, &[OTHER], &[OTHER.1]).await,
        "processed"
    );
    assert_eq!(
        tokio::time::Instant::now(),
        started,
        "no virtual time elapsed"
    );
    assert_eq!(
        captured_decisions(dir.path())[OTHER.0].rationale["citation"],
        "cited"
    );

    let publishes = handler
        .source_capture
        .citation_publishes
        .load(Ordering::Relaxed);
    let go = parked_pass(&handler).await;
    let Some(Settlement::Running { accepted_at }) = settlement(&handler, &project, SESSION) else {
        panic!("the parked job holds a Running entry");
    };
    let submission =
        prepare_submission(&handler, test_route(7), SESSION, &[SOURCE], &[SOURCE.1]).await;
    let submit = spawn_submit(&handler, test_route(7), submission);
    for _ in 0..10_000 {
        if waits(&handler) > 0 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(waits(&handler), 1, "the submit armed its wait");
    let deadline = accepted_at + crate::source_capture::CAPTURE_BUDGET;
    tokio::time::advance(deadline - tokio::time::Instant::now() - Duration::from_millis(1)).await;
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    assert!(
        !submit.is_finished(),
        "pending one millisecond before the deadline"
    );
    assert_eq!(
        handler
            .source_capture
            .citation_publishes
            .load(Ordering::Relaxed),
        publishes,
        "no lookup ran before the deadline"
    );
    tokio::time::advance(Duration::from_millis(1)).await;
    assert_eq!(submit.await.unwrap(), "processed");
    assert_eq!(tokio::time::Instant::now(), deadline);
    assert_eq!(
        handler
            .source_capture
            .citation_publishes
            .load(Ordering::Relaxed),
        publishes + 1,
        "the lookup ran after the deadline"
    );
    assert_eq!(
        captured_decisions(dir.path())[SOURCE.0].rationale["citation"],
        "publish_budget_expired"
    );
    go.send(()).unwrap();
    settle(&handler).await;
}

/// A submit that arrives while the pass's publish runs waits outside the
/// capture commit gate, runs its lookup only after the publish ends, and
/// cites the descriptor the publish wrote.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_submit_during_the_publish_waits_outside_the_gate_and_then_cites() {
    let (handler, dir, project) = settling_handler().await;
    let go = parked_pass(&handler).await;
    assert!(matches!(
        settlement(&handler, &project, SESSION),
        Some(Settlement::Running { .. })
    ));
    let submission =
        prepare_submission(&handler, test_route(7), SESSION, &[SOURCE], &[SOURCE.1]).await;
    let submit = spawn_submit(&handler, test_route(7), submission);
    await_waits(&handler, 1).await;
    assert!(
        !submit.is_finished(),
        "the submit waits on the running publish"
    );
    // The hook parks the job before the gate, so a free gate here means the
    // waiting submit holds none.
    assert!(
        handler.capture_commit_gate.try_lock().is_ok(),
        "the waiting submit holds no capture commit gate"
    );
    assert_eq!(
        handler
            .source_capture
            .citation_publishes
            .load(Ordering::Relaxed),
        0,
        "no lookup ran while the entry was Running"
    );
    assert!(captured_decisions(dir.path()).is_empty());
    go.send(()).unwrap();
    assert_eq!(submit.await.unwrap(), "processed");
    assert_eq!(
        captured_decisions(dir.path())[SOURCE.0].rationale["citation"],
        "cited"
    );
}

/// The same wait for an OpenCode session: the handoff and the submit build
/// equal settlement keys for OpenCode too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_opencode_submit_during_the_publish_waits_and_then_cites() {
    let (handler, dir, _project) = capture_handler(citing_config(true), true).await;
    let handler = Arc::new(handler);
    let (entered, go) = park_source_publish(&handler);
    let source = ("msg_oc1", "Use port 4321 for staging.");
    let window = [opencode_user(source.0, 1_700_000_000_000, source.1)];
    pass(&handler, transform_with(&window, "base-1")).await;
    wait_parked(entered).await;
    let submission =
        prepare_submission(&handler, test_route(7), SESSION, &[source], &[source.1]).await;
    let submit = spawn_submit(&handler, test_route(7), submission);
    await_waits(&handler, 1).await;
    go.send(()).unwrap();
    assert_eq!(submit.await.unwrap(), "processed");
    assert_eq!(
        captured_decisions(dir.path())[source.0].rationale["citation"],
        "cited"
    );
}

/// A lease spans the project's pending jobs: a submit on another session
/// whose batch holds this session's job waits on this session's publish and
/// cites it, while its own job reads its own refused handoff.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_submit_on_another_session_waits_on_each_job_session_it_carries() {
    let (handler, dir, project) = settling_handler().await;
    let go = parked_pass(&handler).await;
    let mut other = binding_with_harness(project.to_str().unwrap(), "pi", "ses-y");
    other.config = capture_config(true);
    handler.bind_route(test_route(8), other.clone());
    let mut request = pi_request(&[user_row(OTHER.0, OTHER.1, 1)], "base-y");
    request["session_id"] = json!("ses-y");
    pass_on(&handler, test_route(8), request).await;
    assert_eq!(
        settlement(&handler, &project, "ses-y"),
        Some(Settlement::Refused)
    );
    other.config = citing_config(true);
    handler.bind_route(test_route(8), other);
    let submission = prepare_submission(
        &handler,
        test_route(8),
        "ses-y",
        &[OTHER],
        &[OTHER.1, SOURCE.1],
    )
    .await;
    let carried: Value = serde_json::from_str(submission["output"].as_str().unwrap()).unwrap();
    assert_eq!(
        carried["decisions"].as_array().unwrap().len(),
        2,
        "the batch carries the parked session's job"
    );
    let submit = spawn_submit(&handler, test_route(8), submission);
    await_waits(&handler, 1).await;
    assert!(!submit.is_finished());
    go.send(()).unwrap();
    assert_eq!(submit.await.unwrap(), "processed");
    let decisions = captured_decisions(dir.path());
    assert_eq!(decisions[SOURCE.0].rationale["citation"], "cited");
    assert_eq!(
        decisions[OTHER.0].rationale["citation"], "publish_budget_expired",
        "each job reads its own session's outcome"
    );
}

/// AC9: a publish that outlasts its budget releases the waiting submit at
/// the budget deadline measured from the handoff's acceptance; the job then
/// ends on its own budget, and a later submit settles at once on that
/// `Deadline` end.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_publish_past_its_budget_releases_the_submit_at_the_deadline() {
    let (handler, dir, project) = settling_handler().await;
    let go = parked_pass(&handler).await;
    let Some(Settlement::Running { accepted_at }) = settlement(&handler, &project, SESSION) else {
        panic!("the parked job holds a Running entry");
    };
    let submission =
        prepare_submission(&handler, test_route(7), SESSION, &[SOURCE], &[SOURCE.1]).await;
    let submit = spawn_submit(&handler, test_route(7), submission);
    await_waits(&handler, 1).await;
    assert_eq!(
        handler
            .source_capture
            .citation_publishes
            .load(Ordering::Relaxed),
        0
    );
    assert_eq!(submit.await.unwrap(), "processed");
    let waited = tokio::time::Instant::now() - accepted_at;
    assert!(
        waited >= crate::source_capture::CAPTURE_BUDGET
            && waited < crate::source_capture::CAPTURE_BUDGET + Duration::from_secs(2),
        "released at the deadline: {waited:?}"
    );
    let decision = &captured_decisions(dir.path())[SOURCE.0];
    assert_eq!(decision.rationale["citation"], "publish_budget_expired");
    assert!(decision.events.is_empty());
    assert_eq!(counter(&handler, "decisions_publish_budget_expired"), 1);
    go.send(()).unwrap();
    settle(&handler).await;
    assert_eq!(
        settlement(&handler, &project, SESSION),
        Some(Settlement::Ended {
            outcome: Count::Deadline
        }),
        "the stalled job ends on its own budget"
    );
    let started = std::time::Instant::now();
    assert_eq!(
        capture_quotes(&handler, &[OTHER], &[OTHER.1]).await,
        "processed"
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(waits(&handler), 1, "an Ended entry arms no wait");
    assert_eq!(
        captured_decisions(dir.path())[OTHER.0].rationale["citation"],
        "publish_budget_expired"
    );
}

/// AC10: a handoff refused because another session's job holds the slot
/// settles at once as `publish_budget_expired`; a hashed source id skips the
/// wait even with a `Running` entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_queue_full_refusal_settles_at_once_and_hashed_ids_skip_the_wait() {
    let (handler, dir, project) = settling_handler().await;
    // The parked pass captures no memory jobs, so the later batches carry
    // only their own session's jobs.
    bind_pi(&handler, &project, capture_config(true));
    let go = parked_pass(&handler).await;
    bind_pi(&handler, &project, citing_config(true));
    let mut other = binding_with_harness(project.to_str().unwrap(), "pi", "ses-y");
    other.config = capture_config(true);
    handler.bind_route(test_route(8), other.clone());
    let mut request = pi_request(&[user_row(OTHER.0, OTHER.1, 1)], "base-y");
    request["session_id"] = json!("ses-y");
    pass_on(&handler, test_route(8), request).await;
    assert_eq!(report(&handler)["refused_queue_full"], 1);
    assert_eq!(
        settlement(&handler, &project, "ses-y"),
        Some(Settlement::Refused)
    );
    other.config = citing_config(true);
    handler.bind_route(test_route(8), other);
    let submission =
        prepare_submission(&handler, test_route(8), "ses-y", &[OTHER], &[OTHER.1]).await;
    let started = std::time::Instant::now();
    assert_eq!(
        submit_state(&handler, test_route(8), &submission).await,
        "processed"
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(waits(&handler), 0);
    assert_eq!(
        captured_decisions(dir.path())[OTHER.0].rationale["citation"],
        "publish_budget_expired"
    );

    let hashed = format!("{}:0", "d".repeat(64));
    let quote = "Production listens on 8080.";
    assert_eq!(
        capture_quotes(&handler, &[(hashed.as_str(), quote)], &[quote]).await,
        "processed"
    );
    assert_eq!(waits(&handler), 0, "a hashed source id arms no wait");
    assert!(matches!(
        settlement(&handler, &project, SESSION),
        Some(Settlement::Running { .. })
    ));
    assert_eq!(
        captured_decisions(dir.path())[&hashed].rationale["citation"],
        "source_id_unjoinable"
    );
    go.send(()).unwrap();
    settle(&handler).await;
}

/// Consent off arms no wait even while the session's publish runs, and the
/// decision reports `source_capture_off`; an error submit arms none either.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn consent_off_and_error_submits_arm_no_wait_while_a_publish_runs() {
    let (handler, dir, project) = settling_handler().await;
    let go = parked_pass(&handler).await;
    let submission =
        prepare_submission(&handler, test_route(7), SESSION, &[OTHER], &[OTHER.1]).await;
    let failed = json!({"method":"memory.capture.submit","session_id":SESSION,"lease":submission["lease"],"model":submission["model"],"error":"cancelled"});
    let started = std::time::Instant::now();
    assert_eq!(
        submit_state(&handler, test_route(7), &failed).await,
        "pending"
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(waits(&handler), 0, "an error submit arms no wait");

    // The failed jobs back off, so the consent-off submit captures its own.
    bind_pi(&handler, &project, citing_config(false));
    let third = ("pi-third", "Rotate the staging key every 30 days.");
    let submission =
        prepare_submission(&handler, test_route(7), SESSION, &[third], &[third.1]).await;
    let started = std::time::Instant::now();
    assert_eq!(
        submit_state(&handler, test_route(7), &submission).await,
        "processed"
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(waits(&handler), 0, "consent off arms no wait");
    assert_eq!(
        captured_decisions(dir.path())[third.0].rationale["citation"],
        "source_capture_off"
    );
    go.send(()).unwrap();
    settle(&handler).await;
}

/// A shutdown refusal on the handoff path records the session's settlement
/// as refused; the job and its `Running` entry exist only after the
/// response settles.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_settlement_is_written_after_the_response_and_a_shutdown_refusal_records_it() {
    let (handler, _dir, project) = settling_handler().await;
    let request = handler
        .test_client_prepare_request(pi_request(&[user_row(SOURCE.0, SOURCE.1, 0)], "base-1"));
    let mut outcome = handler
        .handle_transform_for_test(test_route(7), request.clone())
        .await;
    let after = outcome
        .take_after_response()
        .expect("the window is charged");
    assert!(matches!(
        handler.test_client_apply(&request, outcome),
        PreparedOutcome::Response(_)
    ));
    assert_eq!(
        settlement(&handler, &project, SESSION),
        None,
        "the response is complete before any job or entry exists"
    );
    after.run();
    settle(&handler).await;
    assert_eq!(
        settlement(&handler, &project, SESSION),
        Some(Settlement::Ended {
            outcome: Count::Completed
        })
    );

    let request =
        handler.test_client_prepare_request(pi_request(&[user_row(OTHER.0, OTHER.1, 5)], "base-2"));
    let mut outcome = handler
        .handle_transform_for_test(test_route(7), request)
        .await;
    let after = outcome
        .take_after_response()
        .expect("the window is charged");
    handler.shutdown().await.unwrap();
    after.run();
    assert_eq!(report(&handler)["refused_shutting_down"], 1);
    assert_eq!(
        settlement(&handler, &project, SESSION),
        Some(Settlement::Refused)
    );
}

/// A daemon cancel during the wait returns before the blocking hop, and a
/// dropped request future stops the same way: no decision is written, the
/// lease is released, and the jobs stay pending.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancel_or_a_dropped_request_during_the_wait_writes_no_decision_and_releases_the_lease() {
    for cancel in [false, true] {
        let (handler, dir, project) = settling_handler().await;
        let digest = kernel_routes::ProjectBinding::new(&project)
            .digest()
            .to_owned();
        let pending = || {
            handler
                .store()
                .unwrap()
                .memory_capture_status(&digest)
                .unwrap()
                .pending
        };
        let go = parked_pass(&handler).await;
        let submission =
            prepare_submission(&handler, test_route(7), SESSION, &[SOURCE], &[SOURCE.1]).await;
        let before = pending();
        let submit = spawn_submit(&handler, test_route(7), submission);
        await_waits(&handler, 1).await;
        if cancel {
            handler.cancel.cancel();
            assert_eq!(submit.await.unwrap(), "pending");
        } else {
            submit.abort();
            assert!(submit.await.unwrap_err().is_cancelled());
        }
        assert!(captured_decisions(dir.path()).is_empty(), "cancel {cancel}");
        assert_eq!(
            handler.native_capture.lock().unwrap().reserved_for_test(),
            0
        );
        assert_eq!(pending(), before, "the jobs stay pending");
        go.send(()).unwrap();
        settle(&handler).await;
        assert!(handler.source_capture.is_idle());
        assert!(!matches!(
            settlement(&handler, &project, SESSION),
            Some(Settlement::Running { .. })
        ));
    }
}

/// Every job end leaves its entry `Ended`: a completed publish and a
/// panicking one, whose slot is idle again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_job_end_leaves_an_ended_settlement_and_an_idle_slot() {
    let (handler, _dir, project) = settling_handler().await;
    *handler.source_capture.before_publish.lock().unwrap() =
        Some(Arc::new(|| panic!("injected worker panic")));
    pass(
        &handler,
        pi_request(&[user_row(SOURCE.0, SOURCE.1, 0)], "base-1"),
    )
    .await;
    settle(&handler).await;
    assert!(handler.source_capture.is_idle());
    assert_eq!(
        settlement(&handler, &project, SESSION),
        Some(Settlement::Ended {
            outcome: Count::WorkerPanicked
        })
    );
    *handler.source_capture.before_publish.lock().unwrap() = None;
    pass(
        &handler,
        pi_request(&[user_row(SOURCE.0, SOURCE.1, 1)], "base-2"),
    )
    .await;
    settle(&handler).await;
    assert_eq!(
        settlement(&handler, &project, SESSION),
        Some(Settlement::Ended {
            outcome: Count::Completed
        })
    );
}
/// A completed publish can leave a unit deferred: the entry is `Ended {
/// Completed }`, the submit arms no wait, the published unit cites, and the
/// deferred one reports `descriptor_not_found`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_completed_publish_with_a_deferred_unit_reports_descriptor_not_found() {
    let (handler, dir, project) = settling_handler().await;
    let ids: Vec<String> = (0..=MAX_SOURCE_BATCH_UNITS)
        .map(|index| format!("pi-{index:03}"))
        .collect();
    let texts: Vec<String> = (0..=MAX_SOURCE_BATCH_UNITS)
        .map(|index| format!("Deferred check {index:03} uses port {}.", 4000 + index))
        .collect();
    let rows: Vec<Value> = (0..=MAX_SOURCE_BATCH_UNITS)
        .map(|index| {
            pi_row(
                &ids[index],
                index.checked_sub(1).map(|parent| ids[parent].as_str()),
                json!({"role":"user","content":texts[index],"timestamp":1_700_000_000_000u64 + index as u64}),
            )
        })
        .collect();
    pass(&handler, pi_request(&rows, "base-1")).await;
    settle(&handler).await;
    assert_eq!(report(&handler)["units_deferred"], 1);
    assert_eq!(
        settlement(&handler, &project, SESSION),
        Some(Settlement::Ended {
            outcome: Count::Completed
        })
    );
    let (first, last) = (0, MAX_SOURCE_BATCH_UNITS);
    let sources = [
        (ids[first].as_str(), texts[first].as_str()),
        (ids[last].as_str(), texts[last].as_str()),
    ];
    let quotes = [texts[first].as_str(), texts[last].as_str()];
    assert_eq!(
        capture_quotes(&handler, &sources, &quotes).await,
        "processed"
    );
    // The pass auto-captured every message, so later leases carry the rest.
    let next =
        json!({"method":"memory.capture.next","session_id":SESSION,"model":"custom-native/model"});
    loop {
        let work = tool_body(
            handler
                .handle_native_capture_next(test_route(7), &next)
                .await,
        );
        if work["state"] != "work" {
            break;
        }
        let submission = submission_for(&work, SESSION, &quotes);
        assert_eq!(
            submit_state(&handler, test_route(7), &submission).await,
            "processed"
        );
    }
    assert_eq!(waits(&handler), 0);
    let deferred = live(&kernel_store(&handler), OccurrenceClass::Messages)
        .iter()
        .all(|row| identity_of(row)[3] != ("message_id", ids[last].as_str()));
    assert!(deferred, "the last unit is the deferred one");
    let decisions = captured_decisions(dir.path());
    assert_eq!(decisions[&ids[first]].rationale["citation"], "cited");
    assert_eq!(
        decisions[&ids[last]].rationale["citation"],
        "descriptor_not_found"
    );
}

//! Strict-miss replay of daemon model traffic: the `LlmExecutionBackend`
//! cassette digests only the pinned `BackendRequest` fields and stops at the
//! first miss, and MemoryReviewer traffic replays through a peer keyed on the
//! attempt-marker tuple or is declared excluded in the manifest.

#![cfg(feature = "test-support")]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use daemon::context_capabilities::{BackendDeclarations, CapabilitySource};
use daemon::memory_reviewer::model_request::{
    Message, MessagesRequest, ResponseAccounting, Role, SendError, Sender,
};
use eval_core::{
    BACKEND_COVERED_FIELDS, CassetteError, CassetteFile, Coverage, MARKERS, MissClass,
};
use host_runtime::CancellationToken;
use host_runtime::model_execution::backend::{
    BackendError, BackendEvent, BackendFuture, BackendRequest, BackendTerminal,
    ContextCapabilities, ErrorClass, EventSink, FinishReason, Harness, LlmExecutionBackend,
    OPENCODE_CONTEXT_CAPABILITIES, SinkStatus,
};
use memory_store::memory_reviewer_ledger::ResponseAllowance;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::time::Instant;

use support::eval_cassette::{CassetteBackend, ReviewerKey, record_of, serve_keyed};
use support::tls_peer::{Peer, no_wait, text_response};

const SUITE: &str = "crates/daemon/tests/eval_cassette.rs::";
const NAMESPACE: &str = "eval-run:fresh:7";
const CANARY: &str = "aws_access_key_id = AKIAQ7R3XM2ZT5WN6PBC";

type Transcript = (Vec<BackendEvent>, BackendTerminal);
type Mutation = (&'static str, Box<dyn Fn(&mut BackendRequest)>);
type Scenario = fn(&mut Coverage);

/// A backend with one fixed transcript per request and asymmetric declarations,
/// so a replay that answered from defaults would be caught.
struct Scripted {
    calls: AtomicUsize,
}

impl LlmExecutionBackend for Scripted {
    fn execute(
        &self,
        request: BackendRequest,
        events: EventSink,
        _: CancellationToken,
    ) -> BackendFuture {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            events.emit(BackendEvent::HarnessDispatch {
                harness: request.harness,
            });
            events.emit(BackendEvent::AssistantText {
                text: format!("echo:{}", request.prompt),
                finish_reason: Some(FinishReason::Completed),
            });
            if request.prompt.contains("overflow") {
                BackendTerminal::Failed(BackendError {
                    class: ErrorClass::ContextOverflow,
                    message: "provider refused the prompt".to_string(),
                    retry_after_secs: Some(3),
                    provider_code: Some("prompt_too_long".to_string()),
                })
            } else {
                BackendTerminal::Completed {
                    finish_reason: FinishReason::Completed,
                }
            }
        })
    }

    fn unavailable_reason(&self, harness: Harness) -> Option<&'static str> {
        (harness == Harness::Pi).then_some("closure_incomplete")
    }

    fn context_capabilities(&self, harness: Harness) -> ContextCapabilities {
        match harness {
            Harness::OpenCode => OPENCODE_CONTEXT_CAPABILITIES,
            Harness::Pi => ContextCapabilities::NONE,
        }
    }
}

fn request(prompt: &str) -> BackendRequest {
    BackendRequest {
        prompt: prompt.to_string(),
        system: Some("Answer plainly.".to_string()),
        provider: "anthropic".to_string(),
        model: "claude".to_string(),
        max_output_tokens: 64,
        temperature: Some(0.7),
        harness: Harness::OpenCode,
        session: "ses_1".to_string(),
        run_id: "model_execution-aa00-1".to_string(),
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// Runs one request and returns the emitted events with the terminal.
async fn run(backend: &Arc<dyn LlmExecutionBackend>, request: BackendRequest) -> Transcript {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = {
        let seen = seen.clone();
        EventSink::new(Arc::new(move |event| {
            seen.lock().unwrap().push(event);
            SinkStatus::Accepted
        }))
    };
    let terminal = backend
        .execute(request, sink, CancellationToken::new())
        .await;
    (seen.lock().unwrap().clone(), terminal)
}

fn record_two() -> (Arc<Scripted>, Value, Vec<Transcript>) {
    let real = Arc::new(Scripted {
        calls: AtomicUsize::new(0),
    });
    let recorder = CassetteBackend::recording(NAMESPACE, real.clone());
    let recording: Arc<dyn LlmExecutionBackend> = recorder.clone();
    let transcripts = runtime().block_on(async {
        vec![
            run(&recording, request("hello")).await,
            run(&recording, request("please overflow")).await,
        ]
    });
    (real, recorder.file().unwrap(), transcripts)
}

fn declared(
    backend: &Arc<dyn LlmExecutionBackend>,
) -> Vec<Result<ContextCapabilities, &'static str>> {
    ["opencode", "pi"]
        .iter()
        .map(|harness| BackendDeclarations::new(backend).declare(harness))
        .collect()
}

fn resigned(mut file: Value) -> Value {
    let parsed: CassetteFile = serde_json::from_value(file.clone()).unwrap();
    file["provenance"]["input_sha256"] = json!(parsed.input_sha256().unwrap());
    file
}

fn replay_preserves_the_transcript_and_the_declarations(coverage: &mut Coverage) {
    let (real, file, transcripts) = record_two();
    assert_eq!(real.calls.load(Ordering::SeqCst), 2);
    assert_eq!(file["cases"].as_array().unwrap().len(), 2);
    let replayer = CassetteBackend::replaying(NAMESPACE, &file).unwrap();
    let replay: Arc<dyn LlmExecutionBackend> = replayer.clone();
    let replayed = runtime().block_on(async {
        vec![
            run(&replay, request("hello")).await,
            run(&replay, request("please overflow")).await,
        ]
    });
    assert_eq!(replayed, transcripts);
    assert_eq!(
        real.calls.load(Ordering::SeqCst),
        2,
        "replay never reaches the real backend"
    );
    assert_eq!(replayer.refusals(), 0);
    let real_dyn: Arc<dyn LlmExecutionBackend> = real;
    assert_eq!(declared(&replay), declared(&real_dyn));
    coverage
        .record("rid_capabilities_read_during_cassette_run")
        .unwrap();
    // A cassette whose header declares defaults latches differently, and the
    // header is inside the provenance digest so the edit must be re-signed.
    let mut defaults = file.clone();
    defaults["declarations"]["pi"]["unavailable_reason"] = Value::Null;
    defaults["declarations"]["opencode"]["suppression"] = json!(false);
    assert!(matches!(
        CassetteBackend::replaying(NAMESPACE, &defaults).err(),
        Some(CassetteError::ProvenanceMismatch { .. })
    ));
    let other: Arc<dyn LlmExecutionBackend> =
        CassetteBackend::replaying(NAMESPACE, &resigned(defaults)).unwrap();
    assert_ne!(declared(&other), declared(&real_dyn));
}

fn miss(terminal: &BackendTerminal) -> &BackendError {
    match terminal {
        BackendTerminal::Failed(error)
            if error.provider_code.as_deref() == Some("cassette_miss") =>
        {
            error
        }
        other => panic!("not a cassette miss: {other:?}"),
    }
}

fn one_byte_in_each_covered_field_misses_and_the_dropped_fields_do_not(coverage: &mut Coverage) {
    let (_, file, _) = record_two();
    let mutations: Vec<Mutation> = vec![
        ("prompt", Box::new(|r| r.prompt.push('!'))),
        ("system", Box::new(|r| r.system = None)),
        ("provider", Box::new(|r| r.provider.push('x'))),
        ("model", Box::new(|r| r.model.push('x'))),
        ("max_output_tokens", Box::new(|r| r.max_output_tokens += 1)),
        ("temperature", Box::new(|r| r.temperature = Some(0.8))),
        ("harness", Box::new(|r| r.harness = Harness::Pi)),
    ];
    let mut covered: BTreeSet<&str> = BACKEND_COVERED_FIELDS.into_iter().collect();
    for (field, mutate) in mutations {
        assert!(covered.remove(field));
        let replayer = CassetteBackend::replaying(NAMESPACE, &file).unwrap();
        let replay: Arc<dyn LlmExecutionBackend> = replayer.clone();
        let runtime = runtime();
        // Consume the first entry so the miss names the second as nearest.
        runtime.block_on(run(&replay, request("hello")));
        let mut changed = request("hello");
        mutate(&mut changed);
        let expected =
            eval_core::request_digest(&record_of(&changed).unwrap().covered().unwrap()).unwrap();
        let (events, terminal) = runtime.block_on(run(&replay, changed));
        assert!(events.is_empty(), "{field}: a miss emits nothing");
        let error = miss(&terminal);
        assert_eq!(error.class, ErrorClass::Permanent);
        assert!(
            error.message.contains("turn 1 ModelRequestChanged"),
            "{}",
            error.message
        );
        let recorded = replayer.terminal().unwrap();
        assert_eq!(recorded.turn, 1);
        assert_eq!(recorded.class, MissClass::ModelRequestChanged);
        assert_eq!(recorded.request_digest, expected);
        assert_eq!(
            recorded.nearest_recorded.as_deref(),
            file["cases"][1]["request_digest"].as_str()
        );
        // The run stops at the miss: the recorded request is refused too.
        let (_, again) = runtime.block_on(run(&replay, request("please overflow")));
        miss(&again);
        assert_eq!(replayer.refusals(), 2);
    }
    assert!(covered.is_empty(), "{covered:?}");
    coverage
        .record("rid_rust_cassette_miss_constructed")
        .unwrap();

    for (label, relabel) in [
        (
            "run_id",
            Box::new(|r: &mut BackendRequest| r.run_id = "model_execution-bb11-9".to_string())
                as Box<dyn Fn(&mut BackendRequest)>,
        ),
        (
            "session",
            Box::new(|r: &mut BackendRequest| r.session = "ses_2".to_string()),
        ),
        (
            "temperature text",
            Box::new(|r: &mut BackendRequest| r.temperature = Some(0.70)),
        ),
    ] {
        let replayer = CassetteBackend::replaying(NAMESPACE, &file).unwrap();
        let replay: Arc<dyn LlmExecutionBackend> = replayer.clone();
        let mut relabelled = request("hello");
        relabel(&mut relabelled);
        let (_, terminal) = runtime().block_on(run(&replay, relabelled));
        assert_eq!(
            terminal,
            BackendTerminal::Completed {
                finish_reason: FinishReason::Completed
            },
            "{label}"
        );
        assert_eq!(replayer.refusals(), 0, "{label}");
    }
}

fn a_regenerated_frame_or_another_namespace_refuses_before_any_request(coverage: &mut Coverage) {
    let (_, file, _) = record_two();
    let mut regenerated = file.clone();
    regenerated["cases"][0]["response"]["events"][1]["assistant_text"]["text"] =
        json!("echo:hello ");
    assert!(matches!(
        CassetteBackend::replaying(NAMESPACE, &regenerated).err(),
        Some(CassetteError::ProvenanceMismatch { .. })
    ));
    assert!(matches!(
        CassetteBackend::replaying("eval-run:aged:7", &file).err(),
        Some(CassetteError::NamespaceMismatch { .. })
    ));
    coverage.record("rid_cassette_namespace_refused").unwrap();

    // An unencodable request and a planted secret are typed refusals at this boundary.
    let real = Arc::new(Scripted {
        calls: AtomicUsize::new(0),
    });
    let recorder = CassetteBackend::recording(NAMESPACE, real);
    let recording: Arc<dyn LlmExecutionBackend> = recorder.clone();
    let runtime = runtime();
    let mut unencodable = request("hello");
    unencodable.temperature = Some(f64::NAN);
    let (_, terminal) = runtime.block_on(run(&recording, unencodable));
    assert!(matches!(
        terminal,
        BackendTerminal::Failed(BackendError { provider_code: Some(code), .. }) if code == "cassette_request"
    ));
    let (_, terminal) = runtime.block_on(run(&recording, request(CANARY)));
    assert!(matches!(
        terminal,
        BackendTerminal::Failed(BackendError { provider_code: Some(code), .. }) if code == "redaction_refused"
    ));
    assert!(matches!(
        recorder.file().err(),
        Some(CassetteError::RedactionRefused(..))
    ));
}

fn reviewer_request(text: &str) -> MessagesRequest {
    MessagesRequest {
        model: "claude".to_string(),
        system: Some("Answer plainly.".to_string()),
        messages: vec![Message {
            role: Role::User,
            content: text.to_string(),
        }],
        max_tokens: 64,
        temperature: None,
    }
}

async fn send(sender: &Sender, request: &MessagesRequest) -> Result<String, SendError> {
    let deadline = Instant::now() + Duration::from_secs(10);
    sender
        .connect(deadline)
        .await?
        .handoff(request.body()?)?
        .complete(
            deadline,
            ResponseAllowance::FULL,
            &mut ResponseAccounting::default(),
        )
        .await
        .map(|answer| answer.text)
}

fn memory_reviewer_replays_through_the_keyed_peer(coverage: &mut Coverage) {
    runtime().block_on(async {
        // Record: one scripted exchange yields the body the marker tuple is built from.
        let mut recording_peer = Peer::start().await;
        let sender = recording_peer.sender_with_credential("cred-7");
        let served = recording_peer.serve(no_wait(), |_| text_response("cargo build"));
        let prompt = reviewer_request("What builds it?");
        assert_eq!(send(&sender, &prompt).await.unwrap(), "cargo build");
        let observed = served.await.unwrap();
        let key = ReviewerKey::of(&observed, sender.credential_id());
        // The key equals what production puts in its attempt marker.
        assert_eq!(key.provider, sender.provider_identity());
        assert_eq!(key.credential_id, sender.credential_id());
        assert_eq!(key.model, prompt.model);
        assert_eq!(
            key.body_digest,
            format!("{:x}", Sha256::digest(prompt.body().unwrap().as_bytes()))
        );

        // Replay: the same body hits; each other key field alone misses.
        let entries = BTreeMap::from([(key.clone(), text_response("cargo build"))]);
        let mut replay_peer = Peer::start().await;
        let sender = replay_peer.sender_with_credential("cred-7");
        let served = serve_keyed(&mut replay_peer, 1, entries.clone(), "cred-7");
        assert_eq!(send(&sender, &prompt).await.unwrap(), "cargo build");
        assert_eq!(served.await.unwrap().len(), 1);

        let mut other_model = prompt.clone();
        other_model.model = "claude-other".to_string();
        let misses: [(&str, MessagesRequest, &str); 3] = [
            ("body", reviewer_request("What builds it!"), "cred-7"),
            ("model", other_model, "cred-7"),
            ("credential", prompt.clone(), "cred-8"),
        ];
        for (label, request, credential) in misses {
            let mut peer = Peer::start().await;
            let sender = peer.sender_with_credential(credential);
            let served = serve_keyed(&mut peer, 2, entries.clone(), credential);
            let refused = send(&sender, &request).await.unwrap_err();
            assert!(
                matches!(refused, SendError::Status(409)),
                "{label}: {refused:?}"
            );
            // The peer stops at the first miss: the recorded body is refused afterwards.
            let after = send(&sender, &prompt).await.unwrap_err();
            assert!(
                matches!(after, SendError::Status(409)),
                "{label}: {after:?}"
            );
            let keys: Vec<ReviewerKey> = served
                .await
                .unwrap()
                .iter()
                .map(|observed| ReviewerKey::of(observed, credential))
                .collect();
            assert_eq!(keys.len(), 2, "{label}");
            assert_ne!(keys[0], key, "{label}");
        }
        // The provider identity is the dialled host, so another host misses too.
        let mut relocated = key.clone();
        relocated.provider = "api.anthropic.com/v1/messages@2023-06-01".to_string();
        let mut peer = Peer::start().await;
        let sender = peer.sender_with_credential("cred-7");
        let served = serve_keyed(
            &mut peer,
            1,
            BTreeMap::from([(relocated, text_response("x"))]),
            "cred-7",
        );
        assert!(matches!(
            send(&sender, &prompt).await.unwrap_err(),
            SendError::Status(409)
        ));
        served.await.unwrap();
    });
    coverage
        .record("rid_reviewer_cassette_miss_reached")
        .unwrap();
}

fn scenarios() -> [(&'static str, Scenario); 4] {
    [
        (
            "replay_preserves_the_transcript_and_the_declarations",
            replay_preserves_the_transcript_and_the_declarations,
        ),
        (
            "one_byte_in_each_covered_field_misses_and_the_dropped_fields_do_not",
            one_byte_in_each_covered_field_misses_and_the_dropped_fields_do_not,
        ),
        (
            "a_regenerated_frame_or_another_namespace_refuses_before_any_request",
            a_regenerated_frame_or_another_namespace_refuses_before_any_request,
        ),
        (
            "memory_reviewer_replays_through_the_keyed_peer",
            memory_reviewer_replays_through_the_keyed_peer,
        ),
    ]
}

fn run_scenario(name: &str) {
    let (_, scenario) = scenarios().into_iter().find(|(n, _)| *n == name).unwrap();
    let mut coverage = Coverage::default();
    scenario(&mut coverage);
    let marker = MARKERS
        .iter()
        .find(|m| m.test == format!("{SUITE}{name}"))
        .unwrap();
    assert!(
        coverage.fired().contains(marker.name),
        "{name} records its marker"
    );
}

#[test]
fn transcript_and_declarations() {
    run_scenario("replay_preserves_the_transcript_and_the_declarations");
}

#[test]
fn covered_field_misses() {
    run_scenario("one_byte_in_each_covered_field_misses_and_the_dropped_fields_do_not");
}

#[test]
fn provenance_and_namespace() {
    run_scenario("a_regenerated_frame_or_another_namespace_refuses_before_any_request");
}

#[test]
fn keyed_reviewer_peer() {
    run_scenario("memory_reviewer_replays_through_the_keyed_peer");
}

/// The completeness proof: one run of every scenario fires every marker this
/// suite owns.
#[test]
fn every_cassette_marker_fires_across_the_scenarios() {
    let mut coverage = Coverage::default();
    for (_, scenario) in scenarios() {
        scenario(&mut coverage);
    }
    coverage.complete(SUITE).unwrap();
}

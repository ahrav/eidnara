mod support;

use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use daemon::context_capabilities::{CapabilitySource, LatchedCapabilities, StaticDeclarations};
use daemon::dispatch::PreparedOutcome;
use daemon::edit_receipts::ReceiptLimits;
use host_runtime::model_execution::backend::{
    ContextCapabilities, EditClass, Harness, LlmExecutionBackend, OPENCODE_CONTEXT_CAPABILITIES,
    PI_CONTEXT_CAPABILITIES,
};
use serde_json::{Value, json};
use support::kernel_daemon::{KernelDaemon, SESSION, StartOptions};

const OCC_A: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const OCC_B: &str = "2222222222222222222222222222222222222222222222222222222222222222";

fn limits() -> ReceiptLimits {
    ReceiptLimits {
        max_keys: NonZeroUsize::new(16).unwrap(),
        retention: Duration::from_secs(120),
        append_allowance_bytes: 4096,
        replacement_capacity_bytes: 2048,
    }
}

fn whole(occurrence_id: &str) -> Value {
    json!({"occurrence_id": occurrence_id, "buffer_len": 100, "span": null})
}

fn prepare(project: &Path, action: &str, survivors: Value) -> Value {
    json!({
        "method": "retrieval.prepare",
        "v": 1,
        "session_id": SESSION,
        "project_root": project.to_str().unwrap(),
        "context_revision": "rev-1",
        "representation": "repr-1",
        "spans": [whole(OCC_A), whole(OCC_B)],
        "selection": [OCC_A, OCC_B],
        "action": action,
        "accounting_profile": "profile-a",
        "edit_bytes": 4,
        "survivors": survivors,
    })
}

async fn call(daemon: &KernelDaemon, request: Value) -> Value {
    match daemon.outcome(request).await {
        PreparedOutcome::Response(output) => output.json_for_test().unwrap().clone(),
        PreparedOutcome::Error { code, message } => json!({"error": code, "message": message}),
        PreparedOutcome::Streamed => panic!("streamed"),
    }
}

fn denied(value: &Value, class: &str, reason: &str) {
    assert_eq!(value["kind"], "terminal", "{value}");
    if reason == "unsupported" {
        assert_eq!(value["terminal"], "capability_unsupported", "{value}");
        assert!(value.get("reason").is_none(), "{value}");
    } else {
        assert_eq!(value["terminal"], "capability_undeclared", "{value}");
        assert_eq!(value["reason"], reason, "{value}");
    }
    assert_eq!(value["class"], class, "{value}");
    assert!(value.get("preparation_id").is_none(), "{value}");
}

async fn call_on(daemon: &KernelDaemon, route: host_runtime::RouteHandle, request: Value) -> Value {
    match daemon.outcome_on(route, request).await {
        PreparedOutcome::Response(output) => output.json_for_test().unwrap().clone(),
        PreparedOutcome::Error { code, message } => json!({"error": code, "message": message}),
        PreparedOutcome::Streamed => panic!("streamed"),
    }
}

struct Nothing;

impl LlmExecutionBackend for Nothing {
    fn execute(
        &self,
        _request: host_runtime::model_execution::backend::BackendRequest,
        _events: host_runtime::model_execution::backend::EventSink,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> host_runtime::model_execution::backend::BackendFuture {
        Box::pin(async { unreachable!("the capability test never runs a model") })
    }
}

/// A host build with no working adapter for the harness: the unavailable backend the production host installs for an absent or unavailable snapshot.
struct Unavailable;

impl LlmExecutionBackend for Unavailable {
    fn execute(
        &self,
        _request: host_runtime::model_execution::backend::BackendRequest,
        _events: host_runtime::model_execution::backend::EventSink,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> host_runtime::model_execution::backend::BackendFuture {
        Box::pin(async { unreachable!("the capability test never runs a model") })
    }

    fn unavailable_reason(&self, _harness: Harness) -> Option<&'static str> {
        Some("descriptor_absent")
    }
}

#[test]
fn an_unavailable_backend_is_an_unreadable_declaration_not_a_closed_one() {
    let source = Arc::new(Unavailable) as Arc<dyn CapabilitySource>;
    for harness in ["opencode", "pi"] {
        assert_eq!(
            LatchedCapabilities::read(Some(&source), harness),
            LatchedCapabilities::Unreadable("descriptor_absent"),
            "{harness}"
        );
    }
}

/// A source whose answer a test can change after a route has bound.
struct Mutable {
    answer: Mutex<ContextCapabilities>,
}

impl CapabilitySource for Mutable {
    fn declare(&self, _harness: &str) -> Result<ContextCapabilities, &'static str> {
        Ok(*self.answer.lock().unwrap())
    }
}

#[test]
fn a_backend_that_overrides_nothing_declares_no_class_and_the_harness_tables_are_recorded() {
    for harness in [Harness::OpenCode, Harness::Pi] {
        assert_eq!(
            Nothing.context_capabilities(harness),
            ContextCapabilities::NONE
        );
    }
    assert_eq!(
        OPENCODE_CONTEXT_CAPABILITIES,
        ContextCapabilities {
            suppression: true,
            replacement: true,
            cross_step_reuse: false,
        }
    );
    assert_eq!(PI_CONTEXT_CAPABILITIES, ContextCapabilities::NONE);
    let latched = LatchedCapabilities::read(
        Some(&(Arc::new(Nothing) as Arc<dyn CapabilitySource>)),
        "opencode",
    );
    for class in [
        EditClass::Suppression,
        EditClass::Replacement,
        EditClass::CrossStepReuse,
    ] {
        assert!(latched.gate(class).is_err());
    }
    assert_eq!(
        LatchedCapabilities::read(
            Some(&(Arc::new(Nothing) as Arc<dyn CapabilitySource>)),
            "other"
        ),
        LatchedCapabilities::Unreadable("unknown_harness")
    );
    assert_eq!(
        LatchedCapabilities::read(None, "opencode"),
        LatchedCapabilities::Unreadable("no_declaration")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_declaration_every_gated_class_is_denied_as_unreadable_and_append_still_works() {
    let daemon = KernelDaemon::start().await;
    daemon
        .handler()
        .set_edit_receipt_limits(Some(limits()))
        .unwrap();
    let project = daemon.project().to_owned();
    for (action, class) in [
        ("replace", "replacement"),
        ("suppress", "suppression"),
        ("reuse", "cross_step_reuse"),
    ] {
        denied(
            &call(&daemon, prepare(&project, action, json!([]))).await,
            class,
            "no_declaration",
        );
    }
    let mut oversized = prepare(&project, "replace", json!([]));
    oversized["edit_bytes"] = json!(1 << 20);
    denied(
        &call(&daemon, oversized).await,
        "replacement",
        "no_declaration",
    );
    let appended = call(&daemon, prepare(&project, "append", json!([]))).await;
    assert_eq!(appended["kind"], "prepared", "{appended}");
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_real_harness_tables_allow_exactly_the_recorded_classes() {
    let source: Arc<dyn CapabilitySource> = Arc::new(StaticDeclarations::new(vec![
        ("opencode".to_owned(), OPENCODE_CONTEXT_CAPABILITIES),
        ("pi".to_owned(), PI_CONTEXT_CAPABILITIES),
    ]));
    for (harness, allowed) in [
        (
            "opencode",
            [("replace", true), ("suppress", true), ("reuse", false)],
        ),
        (
            "pi",
            [("replace", false), ("suppress", false), ("reuse", false)],
        ),
    ] {
        let daemon = KernelDaemon::start_with(StartOptions {
            harness: harness.to_owned(),
            consumer_capabilities: vec!["replacement".to_owned(), "suppression".to_owned()],
            capability_source: Some(Arc::clone(&source)),
            ..StartOptions::default()
        })
        .await;
        daemon
            .handler()
            .set_edit_receipt_limits(Some(limits()))
            .unwrap();
        let project = daemon.project().to_owned();
        for (action, allowed) in allowed {
            let survivors = json!([whole(OCC_A), whole(OCC_B)]);
            let answer = call(&daemon, prepare(&project, action, survivors)).await;
            if allowed {
                assert_eq!(answer["kind"], "prepared", "{harness} {action}: {answer}");
            } else {
                let class = match action {
                    "replace" => "replacement",
                    "suppress" => "suppression",
                    _ => "cross_step_reuse",
                };
                denied(&answer, class, "unsupported");
            }
        }
        let appended = call(&daemon, prepare(&project, "append", json!([]))).await;
        assert_eq!(
            appended["kind"], "prepared",
            "{harness}: pure packing still works"
        );
        daemon.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_declaration_is_latched_at_bind_and_reread_by_a_new_bind() {
    let source = Arc::new(Mutable {
        answer: Mutex::new(ContextCapabilities::NONE),
    });
    let daemon = KernelDaemon::start_with(StartOptions {
        capability_source: Some(Arc::clone(&source) as Arc<dyn CapabilitySource>),
        ..StartOptions::default()
    })
    .await;
    daemon
        .handler()
        .set_edit_receipt_limits(Some(limits()))
        .unwrap();
    let project = daemon.project().to_owned();
    denied(
        &call(&daemon, prepare(&project, "replace", json!([]))).await,
        "replacement",
        "unsupported",
    );
    *source.answer.lock().unwrap() = ContextCapabilities {
        replacement: true,
        ..ContextCapabilities::NONE
    };
    denied(
        &call(&daemon, prepare(&project, "replace", json!([]))).await,
        "replacement",
        "unsupported",
    );
    let later = daemon.bind_another(9, "test").await;
    let prepared = call_on(&daemon, later, prepare(&project, "replace", json!([]))).await;
    assert_eq!(prepared["kind"], "prepared", "{prepared}");
    denied(
        &call(&daemon, prepare(&project, "replace", json!([]))).await,
        "replacement",
        "unsupported",
    );
    let key = prepared["preparation_id"].as_str().unwrap();
    let mut apply = json!({
        "method": "retrieval.apply",
        "v": 1,
        "session_id": SESSION,
        "project_root": project.to_str().unwrap(),
        "preparation_id": key,
        "context_revision": "rev-1",
        "representation": "repr-1",
        "spans": [whole(OCC_A), whole(OCC_B)],
        "selection": [OCC_A, OCC_B],
    });
    denied(
        &call(&daemon, apply.clone()).await,
        "replacement",
        "unsupported",
    );
    apply["project_root"] = json!(project.to_str().unwrap());
    let forwarded = call_on(&daemon, later, apply).await;
    assert_eq!(
        forwarded["kind"], "forwarded",
        "the route whose declaration allows the class applies it: {forwarded}"
    );
    let forwarded_identity = forwarded["forwarded_identity"].as_str().unwrap();
    let confirm = json!({
        "method": "retrieval.confirm",
        "v": 1,
        "session_id": SESSION,
        "project_root": project.to_str().unwrap(),
        "preparation_id": key,
        "forwarded_identity": forwarded_identity,
        "applied_identity": forwarded_identity,
        "outcome": "applied_replacement",
    });
    denied(
        &call(&daemon, confirm.clone()).await,
        "replacement",
        "unsupported",
    );
    let complete = call_on(&daemon, later, confirm).await;
    assert_eq!(
        complete["state"], "complete",
        "the route whose declaration allows the class confirms it: {complete}"
    );
    daemon.shutdown().await;

    let rebound = KernelDaemon::start_with(StartOptions {
        capability_source: Some(Arc::clone(&source) as Arc<dyn CapabilitySource>),
        ..StartOptions::default()
    })
    .await;
    rebound
        .handler()
        .set_edit_receipt_limits(Some(limits()))
        .unwrap();
    let project = rebound.project().to_owned();
    assert_eq!(
        call(&rebound, prepare(&project, "replace", json!([]))).await["kind"],
        "prepared"
    );
    denied(
        &call(&rebound, prepare(&project, "suppress", json!([]))).await,
        "suppression",
        "unsupported",
    );
    rebound.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn suppression_needs_whole_message_survivor_proof_for_every_selected_occurrence() {
    let daemon = KernelDaemon::start_with(StartOptions {
        capability_source: Some(Arc::new(StaticDeclarations::new(vec![(
            "test".to_owned(),
            ContextCapabilities {
                suppression: true,
                ..ContextCapabilities::NONE
            },
        )]))),
        ..StartOptions::default()
    })
    .await;
    daemon
        .handler()
        .set_edit_receipt_limits(Some(limits()))
        .unwrap();
    let project = daemon.project().to_owned();

    let none = call(&daemon, prepare(&project, "suppress", json!([]))).await;
    assert_eq!(none["outcome"], "preparation_failure", "{none}");
    assert_eq!(none["reason"], "no_survivor_proof");

    let shorter = call(
        &daemon,
        prepare(
            &project,
            "suppress",
            json!([
                whole(OCC_A),
                {"occurrence_id": OCC_B, "buffer_len": 40, "span": [0, 40]}
            ]),
        ),
    )
    .await;
    assert_eq!(shorter["outcome"], "preparation_failure", "{shorter}");
    assert_eq!(
        shorter["reason"], "unconfirmed_survivor",
        "a survivor's own length is not the context's"
    );
    let malformed = call(
        &daemon,
        prepare(
            &project,
            "suppress",
            json!([whole(OCC_A), {"occurrence_id": "zz", "buffer_len": 100, "span": null}]),
        ),
    )
    .await;
    assert_eq!(malformed["reason"], "malformed_survivor", "{malformed}");
    let misspelled = call(
        &daemon,
        prepare(
            &project,
            "suppress",
            json!([whole(OCC_A), {"occurrence_id": OCC_B, "buffer_len": 100, "spn": null}]),
        ),
    )
    .await;
    assert_eq!(
        misspelled["error"], "invalid_params",
        "a survivor entry is parsed as strictly as a span: {misspelled}"
    );
    let mut malformed_selection = prepare(&project, "suppress", json!([]));
    malformed_selection["selection"] = json!(["zz"]);
    let malformed_selection = call(&daemon, malformed_selection).await;
    assert_eq!(
        malformed_selection["error"], "invalid_params",
        "a malformed context is refused before the survivor proof is judged: {malformed_selection}"
    );

    for survivors in [
        json!([whole(OCC_A), whole(OCC_B), {"occurrence_id": OCC_B, "buffer_len": 100, "span": [0, 40]}]),
        json!([{"occurrence_id": OCC_B, "buffer_len": 100, "span": [0, 40]}, whole(OCC_B), whole(OCC_A)]),
    ] {
        let duplicated = call(&daemon, prepare(&project, "suppress", survivors)).await;
        assert_eq!(duplicated["outcome"], "preparation_failure", "{duplicated}");
        assert_eq!(
            duplicated["reason"], "malformed_survivor",
            "a duplicated occurrence is refused regardless of entry order: {duplicated}"
        );
    }

    let mut unknown_selection = prepare(&project, "suppress", json!([whole(OCC_A), whole(OCC_B)]));
    unknown_selection["spans"] = json!([whole(OCC_A)]);
    let unknown_selection = call(&daemon, unknown_selection).await;
    assert_eq!(
        unknown_selection["outcome"], "preparation_failure",
        "{unknown_selection}"
    );
    assert_eq!(
        unknown_selection["reason"], "selection_not_in_spans",
        "a selected occurrence absent from the context's spans is the context's defect, not the survivor proof's: {unknown_selection}"
    );
    for spans in [
        json!([whole(OCC_A), {"occurrence_id": OCC_A, "buffer_len": 40, "span": null}, whole(OCC_B)]),
        json!([{"occurrence_id": OCC_A, "buffer_len": 40, "span": null}, whole(OCC_A), whole(OCC_B)]),
    ] {
        let mut conflicting = prepare(&project, "suppress", json!([whole(OCC_A), whole(OCC_B)]));
        conflicting["spans"] = spans;
        let conflicting = call(&daemon, conflicting).await;
        assert_eq!(
            conflicting["reason"], "unconfirmed_survivor",
            "a context naming one occurrence with two lengths confirms no survivor for it, in either order: {conflicting}"
        );
    }
    let mut unknown_selection_no_proof = prepare(&project, "suppress", json!([]));
    unknown_selection_no_proof["spans"] = json!([whole(OCC_A)]);
    let unknown_selection_no_proof = call(&daemon, unknown_selection_no_proof).await;
    assert_eq!(
        unknown_selection_no_proof["reason"], "selection_not_in_spans",
        "the context's defect is named even when no survivor proof was sent: {unknown_selection_no_proof}"
    );

    let partial = call(
        &daemon,
        prepare(&project, "suppress", json!([whole(OCC_A)])),
    )
    .await;
    assert_eq!(partial["outcome"], "preparation_failure", "{partial}");
    assert_eq!(partial["reason"], "unconfirmed_survivor");

    let span_only = call(
        &daemon,
        prepare(
            &project,
            "suppress",
            json!([
                whole(OCC_A),
                {"occurrence_id": OCC_B, "buffer_len": 100, "span": [0, 40]}
            ]),
        ),
    )
    .await;
    assert_eq!(span_only["outcome"], "preparation_failure", "{span_only}");
    assert_eq!(span_only["reason"], "span_granularity");

    let confirmed = call(
        &daemon,
        prepare(&project, "suppress", json!([whole(OCC_A), whole(OCC_B)])),
    )
    .await;
    assert_eq!(confirmed["kind"], "prepared", "{confirmed}");

    let explicit_whole = call(
        &daemon,
        prepare(
            &project,
            "suppress",
            json!([
                whole(OCC_A),
                {"occurrence_id": OCC_B, "buffer_len": 100, "span": [0, 100]}
            ]),
        ),
    )
    .await;
    assert_eq!(explicit_whole["kind"], "prepared", "{explicit_whole}");
    daemon.shutdown().await;
}

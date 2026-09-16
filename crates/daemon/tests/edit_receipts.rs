mod support;

use std::num::NonZeroUsize;
use std::path::Path;
use std::time::Duration;

use daemon::dispatch::PreparedOutcome;
use std::sync::Arc;

use daemon::context_capabilities::StaticDeclarations;
use daemon::edit_receipts::{RETENTION_FLOOR, ReceiptLimits, ReceiptLimitsRefusal};
use host_runtime::model_execution::backend::ContextCapabilities;
use host_runtime::{BindOutcome, CompositeComponent, RouteHandle, RouteIdentity};
use serde_json::{Value, json};
use support::kernel_daemon::{KernelDaemon, SESSION, StartOptions};

const OCC_A: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const OCC_B: &str = "2222222222222222222222222222222222222222222222222222222222222222";

/// A daemon whose harness declares every gated class, so the receipt lifecycle can be exercised for each action.
async fn permissive_daemon() -> KernelDaemon {
    KernelDaemon::start_with(StartOptions {
        capability_source: Some(Arc::new(StaticDeclarations::new(vec![(
            "test".to_owned(),
            ContextCapabilities {
                suppression: true,
                replacement: true,
                cross_step_reuse: true,
            },
        )]))),
        ..StartOptions::default()
    })
    .await
}

fn limits() -> ReceiptLimits {
    ReceiptLimits {
        max_keys: NonZeroUsize::new(16).unwrap(),
        retention: Duration::from_secs(120),
        append_allowance_bytes: 4096,
        replacement_capacity_bytes: 2048,
    }
}

fn context(revision: &str, representation: &str, span_end: u64) -> Value {
    json!({
        "context_revision": revision,
        "representation": representation,
        "spans": [
            {"occurrence_id": OCC_A, "buffer_len": 100, "span": [0, span_end]},
            {"occurrence_id": OCC_B, "buffer_len": 40, "span": null},
        ],
        "selection": [OCC_A, OCC_B],
    })
}

fn envelope(method: &str, project: &Path, body: Value) -> Value {
    let mut request = json!({
        "method": method,
        "v": 1,
        "session_id": SESSION,
        "project_root": project.to_str().unwrap(),
    });
    for (key, value) in body.as_object().unwrap() {
        request[key] = value.clone();
    }
    request
}

fn prepare(project: &Path, ctx: Value, action: &str, edit_bytes: u64) -> Value {
    let mut body = ctx;
    body["action"] = json!(action);
    body["accounting_profile"] = json!("profile-a");
    body["edit_bytes"] = json!(edit_bytes);
    envelope("retrieval.prepare", project, body)
}

fn apply(project: &Path, preparation_id: &str, ctx: Value) -> Value {
    let mut body = ctx;
    body["preparation_id"] = json!(preparation_id);
    envelope("retrieval.apply", project, body)
}

fn confirm(
    project: &Path,
    preparation_id: &str,
    forwarded: &str,
    applied: Option<&str>,
    outcome: &str,
) -> Value {
    envelope(
        "retrieval.confirm",
        project,
        json!({
            "preparation_id": preparation_id,
            "forwarded_identity": forwarded,
            "applied_identity": applied,
            "outcome": outcome,
        }),
    )
}

/// The independent edit log: every `forwarded` answer is one effect the harness would apply.
struct Consumer<'a> {
    daemon: &'a KernelDaemon,
    project: std::path::PathBuf,
    edit_log: std::cell::RefCell<Vec<String>>,
}

impl<'a> Consumer<'a> {
    fn new(daemon: &'a KernelDaemon) -> Self {
        Self {
            daemon,
            project: daemon.project().to_owned(),
            edit_log: std::cell::RefCell::new(Vec::new()),
        }
    }

    async fn call(&self, request: Value) -> Value {
        match self.daemon.outcome(request).await {
            PreparedOutcome::Response(output) => {
                let value = output.json_for_test().unwrap().clone();
                if value["kind"] == "forwarded" {
                    self.edit_log
                        .borrow_mut()
                        .push(value["forwarded_identity"].as_str().unwrap().to_string());
                }
                value
            }
            PreparedOutcome::Error { code, message } => json!({"error": code, "message": message}),
            PreparedOutcome::Streamed => panic!("streamed"),
        }
    }

    async fn prepare(&self, ctx: Value, action: &str, edit_bytes: u64) -> Value {
        self.call(prepare(&self.project, ctx, action, edit_bytes))
            .await
    }

    async fn apply(&self, preparation_id: &str, ctx: Value) -> Value {
        self.call(apply(&self.project, preparation_id, ctx)).await
    }

    async fn confirm(
        &self,
        preparation_id: &str,
        forwarded: &str,
        applied: Option<&str>,
        outcome: &str,
    ) -> Value {
        self.call(confirm(
            &self.project,
            preparation_id,
            forwarded,
            applied,
            outcome,
        ))
        .await
    }

    fn effects(&self) -> Vec<String> {
        self.edit_log.borrow().clone()
    }
}

fn id(prepared: &Value) -> String {
    assert_eq!(prepared["kind"], "prepared", "{prepared}");
    prepared["preparation_id"].as_str().unwrap().to_string()
}

fn terminal(value: &Value) -> &str {
    assert_eq!(value["kind"], "terminal", "{value}");
    value["terminal"].as_str().unwrap()
}

async fn bind_other_project(daemon: &KernelDaemon) -> (RouteHandle, std::path::PathBuf) {
    let root = daemon.data_home().join("other-project");
    std::fs::create_dir_all(&root).unwrap();
    let route = RouteHandle {
        channel: 8,
        epoch: 1,
    };
    let identity = RouteIdentity {
        project_root: root.clone(),
        harness: "test".to_owned(),
        session: SESSION.to_owned(),
        consumer_module_id: None,
        consumer_launch_nonce: None,
        consumer_capabilities: Vec::new(),
        admission_facts: None,
        credential_fingerprints: std::collections::BTreeMap::new(),
    };
    assert!(matches!(
        daemon.handler().bind(route, identity).await,
        BindOutcome::Accept
    ));
    (route, root)
}

async fn call_on(daemon: &KernelDaemon, route: RouteHandle, request: Value) -> Value {
    match daemon
        .handler()
        .dispatch_value_for_test(route, request)
        .await
    {
        PreparedOutcome::Response(output) => output.json_for_test().unwrap().clone(),
        PreparedOutcome::Error { code, message } => json!({"error": code, "message": message}),
        PreparedOutcome::Streamed => panic!("streamed"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_receipt_belongs_to_the_project_that_prepared_it() {
    let daemon = KernelDaemon::start().await;
    let mut narrow = limits();
    narrow.max_keys = NonZeroUsize::new(1).unwrap();
    daemon
        .handler()
        .set_edit_receipt_limits(Some(narrow))
        .unwrap();
    let consumer = Consumer::new(&daemon);
    let (other_route, other_root) = bind_other_project(&daemon).await;
    let ctx = context("rev-1", "repr-1", 10);

    let key = id(&consumer.prepare(ctx.clone(), "append", 10).await);
    let elsewhere = call_on(&daemon, other_route, apply(&other_root, &key, ctx.clone())).await;
    assert_eq!(
        terminal(&elsewhere),
        "receipt_unavailable",
        "another project's route holds no receipt for the key: {elsewhere}"
    );
    let elsewhere = call_on(
        &daemon,
        other_route,
        confirm(&other_root, &key, "f", Some("f"), "append"),
    )
    .await;
    assert_eq!(terminal(&elsewhere), "receipt_unavailable");
    let forwarded = consumer.apply(&key, ctx.clone()).await;
    assert_eq!(
        forwarded["kind"], "forwarded",
        "the preparing project's route still forwards: {forwarded}"
    );

    let other_key = call_on(
        &daemon,
        other_route,
        prepare(&other_root, ctx.clone(), "append", 10),
    )
    .await;
    assert_eq!(
        other_key["kind"], "prepared",
        "the count bound is per project, so a full project does not refuse another: {other_key}"
    );
    let still = consumer.apply(&key, ctx.clone()).await;
    assert_eq!(
        still["state"], "in_flight",
        "another project's mint never evicts this project's receipt: {still}"
    );
    let forwarded = call_on(
        &daemon,
        other_route,
        apply(&other_root, id(&other_key).as_str(), ctx),
    )
    .await;
    assert_eq!(forwarded["kind"], "forwarded");
    assert_eq!(consumer.effects().len(), 1);
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_route_is_disabled_until_an_approved_limit_set_is_installed() {
    let daemon = permissive_daemon().await;
    let consumer = Consumer::new(&daemon);
    let ctx = context("rev-1", "repr-1", 10);
    assert_eq!(
        terminal(&consumer.prepare(ctx.clone(), "append", 10).await),
        "disabled"
    );
    assert_eq!(
        terminal(&consumer.apply("x-y", ctx.clone()).await),
        "disabled"
    );
    assert_eq!(
        terminal(&consumer.confirm("x-y", "f", None, "keep").await),
        "disabled"
    );
    let mut short = limits();
    short.retention = RETENTION_FLOOR - Duration::from_millis(1);
    assert_eq!(
        daemon.handler().set_edit_receipt_limits(Some(short)),
        Err(ReceiptLimitsRefusal::RetentionBelowFloor {
            retention: short.retention,
        })
    );
    assert_eq!(
        terminal(&consumer.prepare(ctx.clone(), "append", 10).await),
        "disabled",
        "a refused limit set installs nothing"
    );
    daemon
        .handler()
        .set_edit_receipt_limits(Some(limits()))
        .unwrap();
    assert_eq!(
        consumer.prepare(ctx, "append", 10).await["kind"],
        "prepared"
    );
    assert!(consumer.effects().is_empty());
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_key_and_digest_replays_the_known_outcome_with_one_effect() {
    let daemon = permissive_daemon().await;
    daemon
        .handler()
        .set_edit_receipt_limits(Some(limits()))
        .unwrap();
    let consumer = Consumer::new(&daemon);
    let ctx = context("rev-1", "repr-1", 10);

    let first = consumer.prepare(ctx.clone(), "append", 10).await;
    let second = consumer.prepare(ctx.clone(), "append", 10).await;
    assert_ne!(
        id(&first),
        id(&second),
        "two intents over one tuple are two keys"
    );
    assert_eq!(first["fingerprint"], second["fingerprint"]);
    assert_eq!(first["preparation_digest"], second["preparation_digest"]);
    let key = id(&first);

    let forwarded = consumer.apply(&key, ctx.clone()).await;
    assert_eq!(forwarded["kind"], "forwarded", "{forwarded}");
    assert_eq!(forwarded["action"], "append");
    assert_eq!(forwarded["edit_bytes"], 10);
    let effect = forwarded["forwarded_identity"]
        .as_str()
        .unwrap()
        .to_string();

    let duplicate = consumer.apply(&key, ctx.clone()).await;
    assert_eq!(duplicate["kind"], "receipt");
    assert_eq!(duplicate["state"], "in_flight");
    assert_eq!(duplicate["forwarded_identity"], effect);
    assert_eq!(consumer.effects(), vec![effect.clone()]);

    let conflicting = consumer.apply(&key, context("rev-1", "repr-1", 11)).await;
    assert_eq!(terminal(&conflicting), "conflict");

    let confirmed = consumer
        .confirm(&key, &effect, Some(&effect), "append")
        .await;
    assert_eq!(confirmed["state"], "complete");
    assert_eq!(confirmed["outcome"], "append");

    let replay = consumer.apply(&key, ctx.clone()).await;
    assert_eq!(replay["state"], "complete");
    assert_eq!(replay["outcome"], "append");
    let again = consumer
        .confirm(&key, &effect, Some(&effect), "append")
        .await;
    assert_eq!(again["state"], "complete");
    assert_eq!(
        terminal(&consumer.confirm(&key, &effect, Some(&effect), "keep").await),
        "conflict",
        "a second acknowledgment with another outcome is a conflict"
    );
    assert_eq!(consumer.effects(), vec![effect], "exactly one effect");

    let other = id(&second);
    let forwarded = consumer.apply(&other, ctx).await;
    assert_eq!(forwarded["kind"], "forwarded");
    assert_eq!(consumer.effects().len(), 2);
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_changed_context_between_prepare_and_apply_is_stale_and_forwards_nothing() {
    let daemon = permissive_daemon().await;
    daemon
        .handler()
        .set_edit_receipt_limits(Some(limits()))
        .unwrap();
    let consumer = Consumer::new(&daemon);
    let ctx = context("rev-1", "repr-1", 10);
    for changed in [
        context("rev-2", "repr-1", 10),
        context("rev-1", "repr-2", 10),
        context("rev-1", "repr-1", 11),
        {
            let mut compacted = context("rev-1", "repr-1", 10);
            compacted["spans"].as_array_mut().unwrap().pop();
            compacted
        },
    ] {
        let key = id(&consumer.prepare(ctx.clone(), "replace", 0).await);
        assert_eq!(
            terminal(&consumer.apply(&key, changed).await),
            "stale_preparation"
        );
        let still = consumer.apply(&key, ctx.clone()).await;
        assert_eq!(
            still["kind"], "forwarded",
            "a stale attempt leaves the preparation usable under its own context"
        );
    }
    assert_eq!(consumer.effects().len(), 4);
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_leaves_forwarded_and_unforwarded_keys_unknown_until_read_back() {
    let ctx = context("rev-1", "repr-1", 10);
    let (forwarded_key, forwarded_effect, prepared_key) = {
        let daemon = permissive_daemon().await;
        daemon
            .handler()
            .set_edit_receipt_limits(Some(limits()))
            .unwrap();
        let consumer = Consumer::new(&daemon);
        let forwarded_key = id(&consumer.prepare(ctx.clone(), "append", 10).await);
        let forwarded = consumer.apply(&forwarded_key, ctx.clone()).await;
        assert_eq!(forwarded["kind"], "forwarded");
        let effect = forwarded["forwarded_identity"]
            .as_str()
            .unwrap()
            .to_string();
        let prepared_key = id(&consumer.prepare(ctx.clone(), "append", 10).await);
        daemon.shutdown().await;
        (forwarded_key, effect, prepared_key)
    };

    let restarted = permissive_daemon().await;
    restarted
        .handler()
        .set_edit_receipt_limits(Some(limits()))
        .unwrap();
    let consumer = Consumer::new(&restarted);
    for key in [&forwarded_key, &prepared_key] {
        let answer = consumer.apply(key, ctx.clone()).await;
        assert_eq!(answer["kind"], "receipt", "{answer}");
        assert_eq!(answer["state"], "unknown");
        let again = consumer.apply(key, ctx.clone()).await;
        assert_eq!(again["state"], "unknown", "Unknown is sticky");
    }
    assert!(consumer.effects().is_empty(), "a retry never forwards");

    let without = consumer
        .confirm(&forwarded_key, &forwarded_effect, None, "append")
        .await;
    assert_eq!(without["state"], "unknown");
    let wrong = consumer
        .confirm(&forwarded_key, &forwarded_effect, Some("other"), "append")
        .await;
    assert_eq!(wrong["state"], "unknown");
    let malformed = consumer
        .confirm(&forwarded_key, "bogus", Some("bogus"), "append")
        .await;
    assert_eq!(malformed["state"], "unknown", "{malformed}");
    let read_back = consumer
        .confirm(
            &forwarded_key,
            &forwarded_effect,
            Some(&forwarded_effect),
            "append",
        )
        .await;
    assert_eq!(read_back["state"], "complete");
    assert_eq!(read_back["outcome"], "append");
    let after = consumer.apply(&forwarded_key, ctx.clone()).await;
    assert_eq!(after["state"], "complete", "a read-back is recorded");
    assert_eq!(after["outcome"], "append");
    assert_eq!(
        terminal(
            &consumer
                .confirm(
                    &forwarded_key,
                    &forwarded_effect,
                    Some(&forwarded_effect),
                    "keep"
                )
                .await
        ),
        "conflict"
    );
    assert!(consumer.effects().is_empty());
    restarted.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uninstalling_the_limit_set_drops_the_receipts_like_a_restart_so_a_read_back_still_lands() {
    let daemon = KernelDaemon::start().await;
    daemon
        .handler()
        .set_edit_receipt_limits(Some(limits()))
        .unwrap();
    let consumer = Consumer::new(&daemon);
    let ctx = context("rev-1", "repr-1", 10);
    let key = id(&consumer.prepare(ctx.clone(), "append", 10).await);
    let forwarded = consumer.apply(&key, ctx.clone()).await;
    assert_eq!(forwarded["kind"], "forwarded");
    let effect = forwarded["forwarded_identity"]
        .as_str()
        .unwrap()
        .to_string();

    daemon.handler().set_edit_receipt_limits(None).unwrap();
    assert_eq!(
        terminal(&consumer.apply(&key, ctx.clone()).await),
        "disabled"
    );
    daemon
        .handler()
        .set_edit_receipt_limits(Some(limits()))
        .unwrap();
    let answer = consumer.apply(&key, ctx.clone()).await;
    assert_eq!(answer["kind"], "receipt", "{answer}");
    assert_eq!(
        answer["state"], "unknown",
        "a key of the dropped store is unknown, not receipt_unavailable"
    );
    let read_back = consumer
        .confirm(&key, &effect, Some(&effect), "append")
        .await;
    assert_eq!(read_back["state"], "complete", "{read_back}");
    assert_eq!(consumer.effects().len(), 1, "nothing is forwarded twice");
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_acknowledgment_is_sticky_unknown_and_a_fenced_confirm_is_a_conflict() {
    let daemon = permissive_daemon().await;
    daemon
        .handler()
        .set_edit_receipt_limits(Some(limits()))
        .unwrap();
    let consumer = Consumer::new(&daemon);
    let ctx = context("rev-1", "repr-1", 10);
    let key = id(&consumer.prepare(ctx.clone(), "append", 10).await);
    let effect = consumer.apply(&key, ctx.clone()).await["forwarded_identity"]
        .as_str()
        .unwrap()
        .to_string();

    assert_eq!(
        terminal(
            &consumer
                .confirm(&key, "stale-forward", Some("stale-forward"), "append")
                .await
        ),
        "conflict",
        "an acknowledgment for another forward is fenced"
    );
    let lost = consumer.confirm(&key, &effect, None, "append").await;
    assert_eq!(lost["state"], "unknown");
    let retry = consumer.apply(&key, ctx.clone()).await;
    assert_eq!(retry["state"], "unknown");
    assert_eq!(consumer.effects(), vec![effect.clone()]);
    assert_eq!(
        terminal(&consumer.confirm(&key, "bogus", Some("bogus"), "keep").await),
        "conflict",
        "a read-back is judged against the recorded forward, not the caller's claim"
    );
    assert_eq!(
        terminal(&consumer.confirm(&key, &effect, Some("bogus"), "keep").await),
        "conflict"
    );
    let read_back = consumer
        .confirm(&key, &effect, Some(&effect), "append")
        .await;
    assert_eq!(read_back["state"], "complete");
    assert_eq!(consumer.apply(&key, ctx).await["outcome"], "append");

    let never_forwarded = id(&consumer
        .prepare(context("rev-9", "repr-1", 10), "append", 10)
        .await);
    assert_eq!(
        terminal(
            &consumer
                .confirm(&never_forwarded, "f", Some("f"), "append")
                .await
        ),
        "conflict",
        "a receipt alone never marks applied"
    );
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_count_bound_evicts_the_oldest_settled_key_and_never_an_in_flight_one() {
    let daemon = permissive_daemon().await;
    let mut narrow = limits();
    narrow.max_keys = NonZeroUsize::new(2).unwrap();
    daemon
        .handler()
        .set_edit_receipt_limits(Some(narrow))
        .unwrap();
    let consumer = Consumer::new(&daemon);
    let ctx = context("rev-1", "repr-1", 10);
    let first = id(&consumer.prepare(ctx.clone(), "append", 10).await);
    let second = id(&consumer.prepare(ctx.clone(), "append", 10).await);
    let third = id(&consumer.prepare(ctx.clone(), "append", 10).await);
    assert_eq!(
        terminal(&consumer.apply(&first, ctx.clone()).await),
        "receipt_unavailable"
    );
    assert_eq!(
        terminal(&consumer.confirm(&first, "f", Some("f"), "append").await),
        "receipt_unavailable"
    );
    assert_eq!(
        consumer.apply(&second, ctx.clone()).await["kind"],
        "forwarded",
        "a read never evicts"
    );
    assert_eq!(
        consumer.apply(&third, ctx.clone()).await["kind"],
        "forwarded"
    );
    let refused = consumer.prepare(ctx.clone(), "append", 10).await;
    assert_eq!(refused["outcome"], "preparation_failure", "{refused}");
    assert_eq!(refused["reason"], "receipt_capacity");
    assert_eq!(
        consumer.apply(&second, ctx.clone()).await["state"],
        "in_flight",
        "an in-flight receipt is never the victim"
    );
    assert_eq!(consumer.effects().len(), 2);

    let mut wider = limits();
    wider.max_keys = NonZeroUsize::new(8).unwrap();
    daemon
        .handler()
        .set_edit_receipt_limits(Some(wider))
        .unwrap();
    assert_eq!(
        consumer.apply(&second, ctx).await["state"],
        "in_flight",
        "a limits change keeps the receipts"
    );
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn outcomes_are_distinct_and_capacity_is_bound_before_preparation() {
    let daemon = permissive_daemon().await;
    daemon
        .handler()
        .set_edit_receipt_limits(Some(limits()))
        .unwrap();
    let consumer = Consumer::new(&daemon);
    let ctx = context("rev-1", "repr-1", 10);

    let over_append = consumer.prepare(ctx.clone(), "append", 4097).await;
    assert_eq!(over_append["kind"], "outcome");
    assert_eq!(over_append["outcome"], "preparation_failure");
    assert_eq!(over_append["reason"], "append_allowance");
    let over_replace = consumer.prepare(ctx.clone(), "replace", 2049).await;
    assert_eq!(over_replace["outcome"], "preparation_failure");
    assert_eq!(over_replace["reason"], "replacement_capacity");

    const OUTCOMES: [&str; 4] = [
        "keep",
        "append",
        "applied_replacement",
        "preparation_failure",
    ];
    let mut keys = Vec::new();
    for (action, outcome) in [
        ("append", OUTCOMES[0]),
        ("append", OUTCOMES[1]),
        ("replace", OUTCOMES[2]),
        ("replace", OUTCOMES[3]),
    ] {
        let key = id(&consumer.prepare(ctx.clone(), action, 0).await);
        let effect = consumer.apply(&key, ctx.clone()).await["forwarded_identity"]
            .as_str()
            .unwrap()
            .to_string();
        let confirmed = consumer
            .confirm(&key, &effect, Some(&effect), outcome)
            .await;
        assert_eq!(confirmed["state"], "complete");
        keys.push((key, outcome));
    }
    let read_back: Vec<String> = {
        let mut out = Vec::new();
        for (key, _) in &keys {
            out.push(
                consumer.apply(key, ctx.clone()).await["outcome"]
                    .as_str()
                    .unwrap()
                    .to_string(),
            );
        }
        out
    };
    assert_eq!(read_back, OUTCOMES.map(str::to_string).to_vec());

    let empty = id(&consumer.prepare(ctx.clone(), "replace", 0).await);
    let forwarded = consumer.apply(&empty, ctx.clone()).await;
    assert_eq!(forwarded["edit_bytes"], 0);
    let effect = forwarded["forwarded_identity"].as_str().unwrap();
    assert_eq!(
        consumer
            .confirm(&empty, effect, Some(effect), "applied_replacement")
            .await["outcome"],
        "applied_replacement"
    );

    let key = id(&consumer.prepare(ctx.clone(), "append", 1).await);
    let effect = consumer.apply(&key, ctx.clone()).await["forwarded_identity"]
        .as_str()
        .unwrap()
        .to_string();
    let applied = consumer
        .confirm(&key, &effect, Some(&effect), "applied")
        .await;
    assert_eq!(applied["error"], "invalid_params", "{applied}");
    let unknown = consumer
        .confirm(&key, &effect, Some(&effect), "unknown")
        .await;
    assert_eq!(unknown["error"], "invalid_params", "{unknown}");
    let mut misspelled_span = ctx.clone();
    misspelled_span["spans"][0] =
        json!({"occurrence_id": OCC_A, "buffer_len": 100, "spn": [0, 10]});
    let refused = consumer.apply(&key, misspelled_span).await;
    assert_eq!(
        refused["error"], "invalid_params",
        "a misspelled span key is refused instead of widening to the whole buffer: {refused}"
    );
    let mut foreign = apply(daemon.project(), &key, ctx);
    foreign["project_root"] = json!(daemon.project().join("elsewhere").to_str().unwrap());
    let refused = consumer.call(foreign).await;
    assert_eq!(refused["state"]["reason"], "project_mismatch", "{refused}");
    daemon.shutdown().await;
}

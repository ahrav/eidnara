//! The direct-host fixture's model backend recorded into a cassette through
//! the real ModelExecution route, then replayed strictly by a second fixture
//! process: the recorded answer comes back, a request the recording never saw
//! is refused, and the controlled backend is never consulted on replay.

#![cfg(all(unix, feature = "test-support"))]

mod support;

use eval_core::{CASSETTE_SCHEMA, Cassette};
use host_runtime::{RequestOptions, ResponseStream, TargetKind};
use serde_json::{Value, json};
use support::direct_host::{BUDGET, FixtureProcess, request_json, send_body};

const NAMESPACE: &str = "eval-run:fixture-cassette:1";

async fn subscribe(
    client: &host_runtime::Client,
    route: host_runtime::ClientRoute,
) -> ResponseStream {
    client
        .request_stream(
            route,
            serde_json::to_vec(&json!({
                "method": "session.subscribe",
                "params": {"from": "start"}
            }))
            .unwrap(),
            RequestOptions {
                timeout: BUDGET,
                ..RequestOptions::default()
            },
        )
        .await
        .expect("subscription starts")
}

async fn drain(stream: &mut ResponseStream) -> Vec<Value> {
    let mut items = Vec::new();
    while let Some(item) = stream.next().await.expect("subscription settles") {
        items.push(serde_json::from_slice(&item.body).unwrap());
    }
    items
}

/// Sends one prompt through the model_execution route and returns the run's
/// stream items.
async fn run(fixture: &FixtureProcess, session: &str, prompt: &str) -> Vec<Value> {
    let client = fixture.client().await;
    let route = fixture
        .open_route(
            &client,
            "model_execution",
            TargetKind::ManagementSurface,
            session,
        )
        .await;
    let sent = request_json(&client, route, send_body(prompt)).await;
    assert!(sent["run_id"].is_string(), "{sent}");
    let mut stream = subscribe(&client, route).await;
    let items = drain(&mut stream).await;
    client.close().await.expect("client closes");
    items
}

fn unit_types(items: &[Value]) -> Vec<&str> {
    items
        .iter()
        .filter_map(|item| item["unit"]["type"].as_str())
        .collect()
}

#[test]
fn the_fixture_records_its_backend_and_replays_it_strictly() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let cassette_dir = tempfile::tempdir().unwrap();
    let cassette = cassette_dir.path().join("fixture.cassette.json");

    // Record: the controlled backend answers, and the exchange is written at
    // shutdown.
    let record_root = tempfile::tempdir().unwrap();
    let recorder =
        FixtureProcess::start_recording(record_root.path().to_path_buf(), &cassette, NAMESPACE);
    let recorded = runtime.block_on(run(&recorder, "recorded-session", "what did we decide"));
    assert_eq!(
        unit_types(&recorded),
        ["run_started", "assistant_message", "run_finished"]
    );
    assert_eq!(
        recorded[1]["unit"]["message"]["content"][0]["text"],
        "fixture-success"
    );
    assert_eq!(recorder.counters(1)["started"], 1);
    recorder.shutdown();
    let file: Value = serde_json::from_slice(&std::fs::read(&cassette).unwrap()).unwrap();
    assert_eq!(file["schema"], json!(CASSETTE_SCHEMA));
    assert_eq!(file["namespace"], json!(NAMESPACE));
    let loaded = Cassette::replay(&file, NAMESPACE).unwrap();
    assert_eq!(loaded.cases().len(), 1, "one exchange recorded");
    assert_eq!(
        loaded.cases()[0].request["prompt"],
        json!("what did we decide")
    );

    // Replay: a second process answers from the file alone; the controlled
    // backend's counters stay at zero.
    let replay_root = tempfile::tempdir().unwrap();
    let replayer =
        FixtureProcess::start_replaying(replay_root.path().to_path_buf(), &cassette, NAMESPACE);
    let replayed = runtime.block_on(run(&replayer, "replayed-session", "what did we decide"));
    assert_eq!(
        unit_types(&replayed),
        ["run_started", "assistant_message", "run_finished"]
    );
    assert_eq!(
        replayed[1]["unit"]["message"]["content"][0]["text"],
        "fixture-success"
    );
    assert_eq!(
        replayer.counters(2)["started"],
        0,
        "the controlled backend was never consulted"
    );

    // Strict: a prompt the recording never saw is a typed refusal, and the
    // frame already consumed does not answer twice.
    let missed = runtime.block_on(run(&replayer, "missed-session", "what did we decide?"));
    let error = missed
        .iter()
        .find(|item| item["unit"]["type"] == "error")
        .unwrap_or_else(|| panic!("{missed:?}"));
    assert_eq!(error["unit"]["error"]["provider_code"], "cassette_miss");
    let again = runtime.block_on(run(&replayer, "again-session", "what did we decide"));
    assert!(
        again.iter().any(|item| item["unit"]["type"] == "error"),
        "the miss latched: {again:?}"
    );
    replayer.shutdown();

    // A file under another namespace, or edited, never starts.
    let mut foreign = file.clone();
    foreign["namespace"] = json!("eval-run:other:2");
    assert!(Cassette::replay(&foreign, NAMESPACE).is_err());
}

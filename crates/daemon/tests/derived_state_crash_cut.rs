#![cfg(all(unix, feature = "test-support"))]

mod support;

use std::time::{Duration, Instant};

use host_runtime::TargetKind;
use memory_store::MemoryStore;
use serde_json::{Value, json};
use support::direct_host::{BUDGET, FixtureProcess, Launch, request_json, wait_for_store};
use support::eval_surface::{block_on, tail};

const SESSION: &str = "crash-cut-session";

fn transform_request() -> Value {
    let (first, first_native) = tail(SESSION, "plan the survey cadence for the quarter", 1);
    let (second, second_native) = tail(SESSION, "and write the weekly schedule down", 2);
    json!({
        "kind": "transform",
        "v": 3,
        "boundary": null,
        "base_revision": "crash-cut-base-1",
        "session_id": SESSION,
        "serializer_profile": "opencode-aisdk",
        "render_config": "crash-cut-config",
        "serve_native": true,
        "native_messages": [first_native, second_native],
        "messages": [first, second],
    })
}

fn committed_row_version(fixture_root: &std::path::Path) -> Option<u64> {
    let descriptor = daemon::managed_store_descriptor(fixture_root).unwrap();
    let storage::StorageBackend::Sqlite { path } = &descriptor.backend else {
        panic!("the fixture store is SQLite");
    };
    let connection =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    connection
        .query_row(
            "SELECT row_version FROM cache_state WHERE session_id = ?1",
            [SESSION],
            |row| row.get::<_, i64>(0),
        )
        .ok()
        .map(|version| version as u64)
}

fn reopened_row_version(fixture_root: &std::path::Path) -> Option<u64> {
    let descriptor = daemon::managed_store_descriptor(fixture_root).unwrap();
    MemoryStore::open(&descriptor)
        .unwrap()
        .load(SESSION)
        .unwrap()
        .row_version
}

async fn tail_hygiene(fixture: &FixtureProcess) -> Value {
    let client = fixture.client().await;
    let route = fixture
        .open_route(&client, "context", TargetKind::ToolProvider, SESSION)
        .await;
    wait_for_store(&client, route, SESSION).await;
    let status = request_json(
        &client,
        route,
        json!({"method": "session.status", "v": 1, "session_id": SESSION}),
    )
    .await;
    client.close_route(route).await.expect("route closes");
    status["tail_hygiene"].clone()
}

#[test]
fn a_process_killed_between_commit_and_promotion_keeps_the_commit_and_forgets_the_derived_state() {
    block_on(async {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("halted-before-promotion");
        let config_home = root.path().join("config-home");
        std::fs::create_dir_all(config_home.join("eidnara")).unwrap();
        std::fs::write(
            config_home.join("eidnara").join("eidnara.jsonc"),
            r#"{ "history_summarizer": { "model": "test/model" } }"#,
        )
        .unwrap();
        let mut halted = Launch::at(root.path().to_path_buf())
            .config_home(&config_home)
            .env(
                "EIDNARA_FIXTURE_HALT_BEFORE_DERIVED_PROMOTION",
                marker.to_str().unwrap(),
            )
            .start();
        let client = halted.client().await;
        let route = halted
            .open_route(&client, "context", TargetKind::ToolProvider, SESSION)
            .await;
        wait_for_store(&client, route, SESSION).await;
        let pending = tokio::spawn(async move {
            support::direct_host::try_request_json(&client, route, transform_request()).await
        });
        let deadline = Instant::now() + BUDGET;
        while !marker.exists() {
            assert!(!pending.is_finished(), "the pass answered before promotion");
            assert!(
                Instant::now() < deadline,
                "the pass never reached promotion"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let committed = committed_row_version(root.path());
        assert!(committed.is_some(), "the pass committed before promotion");
        halted.kill();
        pending.abort();

        assert_eq!(reopened_row_version(root.path()), committed);
        let restarted = FixtureProcess::start_folding_at(root.path().to_path_buf());
        let tail_hygiene = tail_hygiene(&restarted).await;
        assert_eq!(tail_hygiene["evaluable"], json!(false), "{tail_hygiene}");
        assert_eq!(
            tail_hygiene["generation_invalidated"],
            json!(true),
            "{tail_hygiene}"
        );
        let _ = restarted.shutdown();
    });
}

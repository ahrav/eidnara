#![cfg(all(target_os = "linux", target_env = "gnu"))]
#![forbid(unsafe_code)]

mod support;

use std::time::{Duration, Instant};

use daemon::process_memory::C_HEAP_ARENAS;
use host_runtime::{Client, ClientRoute, TargetKind};
use serde_json::{Value, json};
use support::direct_host::{BUDGET, FixtureProcess, request_json, wait_for_store};

const MIB: u64 = 1024 * 1024;
const GLIBC_ARENA_HEAP_KIB: u64 = 64 * 1024;
const SESSION_GROWTH_BOUND_BYTES: u64 = 2 * MIB;
const SESSION_KEYED_HOLDERS: [&str; 4] = [
    "snapshots",
    "native_attach",
    "serialized_output",
    "boundary_token",
];

fn env_count(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn rss_anon_bytes(pid: u32) -> u64 {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).expect("process status");
    let kib: u64 = status
        .lines()
        .find_map(|line| line.strip_prefix("RssAnon:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|value| value.parse().ok())
        .expect("RssAnon in the process status");
    kib * 1024
}

struct Mapping {
    start: u64,
    end: u64,
    readable: bool,
    anonymous: bool,
}

fn mappings(pid: u32) -> Vec<Mapping> {
    std::fs::read_to_string(format!("/proc/{pid}/maps"))
        .expect("process maps")
        .lines()
        .map(|line| {
            let mut fields = line.split_whitespace();
            let (start, end) = fields
                .next()
                .and_then(|range| range.split_once('-'))
                .expect("address range");
            let permissions = fields.next().expect("permissions");
            Mapping {
                start: u64::from_str_radix(start, 16).expect("start address"),
                end: u64::from_str_radix(end, 16).expect("end address"),
                readable: permissions.starts_with('r'),
                anonymous: fields.nth(3).is_none(),
            }
        })
        .collect()
}

fn glibc_arena_heaps(pid: u32) -> usize {
    let mappings = mappings(pid);
    mappings
        .windows(2)
        .filter(|pair| {
            let (used, reserved) = (&pair[0], &pair[1]);
            used.anonymous
                && reserved.anonymous
                && used.readable
                && !reserved.readable
                && used.end == reserved.start
                && (reserved.end - used.start) / 1024 == GLIBC_ARENA_HEAP_KIB
        })
        .count()
}

fn tool_output(session: usize, turn: usize, bytes: usize) -> String {
    let mut text = String::with_capacity(bytes + 128);
    let mut line = 0;
    while text.len() < bytes {
        text.push_str(&format!(
            "src/module_{session}/file_{turn}.rs:{line}: fn handler_{line}(request: &Request) -> Result<Response, Error> {{\n"
        ));
        line += 1;
    }
    text
}

fn assistant(id: String, content: Value, stop_reason: &str, timestamp: u64) -> Value {
    json!({
        "id": id,
        "message": {
            "role": "assistant",
            "content": content,
            "api": "fixture",
            "provider": "fixture",
            "model": "deterministic",
            "usage": {
                "input": 1000, "output": 50, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 1050,
                "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0}
            },
            "stopReason": stop_reason,
            "timestamp": timestamp,
        }
    })
}

fn turn_rows(session: usize, turn: usize, output_bytes: usize) -> Vec<Value> {
    let stamp = 1_700_000_000_000_u64 + (session as u64) * 1_000_000 + (turn as u64) * 1_000;
    let call = format!("call-{session}-{turn}");
    let path = format!("src/module_{session}/file_{turn}.rs");
    vec![
        json!({
            "id": format!("s{session}-t{turn}-user"),
            "message": {
                "role": "user",
                "content": format!("Turn {turn}: update the handler in {path} and keep its interface."),
                "timestamp": stamp,
            }
        }),
        assistant(
            format!("s{session}-t{turn}-call"),
            json!([
                {"type": "text", "text": format!("Reading {path} before the edit.")},
                {"type": "toolCall", "id": call, "name": "read", "arguments": {"path": path}}
            ]),
            "toolUse",
            stamp + 1,
        ),
        json!({
            "id": format!("s{session}-t{turn}-result"),
            "message": {
                "role": "toolResult",
                "toolCallId": call,
                "toolName": "read",
                "content": [{"type": "text", "text": tool_output(session, turn, output_bytes)}],
                "isError": false,
                "timestamp": stamp + 2,
            }
        }),
        assistant(
            format!("s{session}-t{turn}-reply"),
            json!([{"type": "text", "text": format!("Updated handler_{turn}; the interface of {path} is unchanged.")}]),
            "stop",
            stamp + 3,
        ),
    ]
}

async fn run_session(fixture: &FixtureProcess, session: usize, turns: usize, output_bytes: usize) {
    let session_id = format!("soak-session-{session}");
    let client = fixture.client().await;
    let route = fixture
        .open_route(&client, "context", TargetKind::ToolProvider, &session_id)
        .await;
    wait_for_store(&client, route, &session_id).await;
    let mut rows: Vec<Value> = Vec::new();
    let mut previous_output: Option<String> = None;
    for turn in 0..turns {
        rows.extend(turn_rows(session, turn, output_bytes));
        let window_bytes: usize = rows.iter().map(|row| row.to_string().len()).sum();
        let mut body = json!({
            "kind": "transform",
            "boundary": null,
            "base_revision": format!("soak-{session}-{turn}"),
            "session_id": session_id,
            "serializer_profile": "pi",
            "render_config": "soak-config",
            "native_messages": rows,
            "usage": {
                "current_total_input_tokens": (window_bytes / 4) as u64,
                "context_limit_tokens": 200_000,
            },
        });
        if let Some(revision) = &previous_output {
            body["previous_output_revision"] = json!(revision);
        }
        let response = request_json(&client, route, body).await;
        assert_eq!(response["status"], "ok", "session {session} turn {turn}");
        previous_output = response["output_revision"].as_str().map(str::to_owned);
    }
    client
        .close_route(route)
        .await
        .expect("session route closes");
    client.close().await.expect("session client closes");
}

async fn status_after_teardown(client: &Client, route: ClientRoute) -> Value {
    let deadline = Instant::now() + BUDGET;
    loop {
        let status = request_json(client, route, json!({"kind": "status"})).await;
        let held: Vec<(&str, &Value)> = SESSION_KEYED_HOLDERS
            .iter()
            .map(|holder| (*holder, &status["memory_holders"][holder]["entry_count"]))
            .filter(|(_, entries)| **entries != json!(0))
            .collect();
        if held.is_empty() {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "session state survives its route: {held:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anonymous_memory_plateaus_across_sessions_and_session_state_ends_with_its_route() {
    let sessions = env_count("EIDNARA_SOAK_SESSIONS", 15);
    let turns = env_count("EIDNARA_SOAK_TURNS", 16);
    let output_bytes = env_count("EIDNARA_SOAK_OUTPUT_BYTES", 16 * 1024);
    let window = sessions / 3;
    assert!(window >= 2, "three windows of at least two sessions");

    let root = tempfile::tempdir().expect("state root");
    std::fs::create_dir_all(root.path().join("project")).expect("project root");
    let fixture = FixtureProcess::start_folding_at(root.path().to_path_buf());
    let pid = fixture.pid();
    let observer = fixture.client().await;
    let observer_route = fixture
        .open_route(
            &observer,
            "context",
            TargetKind::ToolProvider,
            "soak-observer",
        )
        .await;
    wait_for_store(&observer, observer_route, "soak-observer").await;

    let mut anonymous = Vec::with_capacity(sessions);
    for session in 0..sessions {
        run_session(&fixture, session, turns, output_bytes).await;
        let status = status_after_teardown(&observer, observer_route).await;
        let sampled = rss_anon_bytes(pid);
        let reported = status["process_memory"]["rss_anon_bytes"]
            .as_u64()
            .expect("the status reports anonymous resident bytes");
        assert!(
            reported.abs_diff(sampled) <= sampled / 2,
            "status reports {reported} bytes, /proc reports {sampled}"
        );
        assert!(
            status["process_memory"]["c_heap"]["in_use_bytes"].is_u64(),
            "{}",
            status["process_memory"]
        );
        anonymous.push(sampled);
    }

    println!(
        "anonymous resident MiB after each session: {:?}",
        anonymous
            .iter()
            .map(|bytes| bytes / MIB)
            .collect::<Vec<_>>()
    );
    let low_water =
        |range: std::ops::Range<usize>| *anonymous[range].iter().min().expect("a window");
    let middle = low_water(window..2 * window);
    let last = low_water(sessions - window..sessions);
    let allowed = SESSION_GROWTH_BOUND_BYTES * window as u64;
    assert!(
        last <= middle + allowed,
        "anonymous memory grew {} MiB over {window} sessions: {:?}",
        last.saturating_sub(middle) / MIB,
        anonymous
            .iter()
            .map(|bytes| bytes / MIB)
            .collect::<Vec<_>>()
    );

    let arena_heaps = glibc_arena_heaps(pid);
    assert!(
        arena_heaps <= C_HEAP_ARENAS,
        "{arena_heaps} glibc arena heaps exceed the limit of {C_HEAP_ARENAS}"
    );

    observer
        .close_route(observer_route)
        .await
        .expect("observer route closes");
    observer.close().await.expect("observer client closes");
    fixture.shutdown();
}

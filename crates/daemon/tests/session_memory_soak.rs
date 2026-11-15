#![cfg(all(target_os = "linux", target_env = "gnu"))]
#![forbid(unsafe_code)]

mod support;

use std::time::{Duration, Instant};

use daemon::process_memory::C_HEAP_ARENAS;
use host_runtime::{Client, ClientRoute, TargetKind};
use serde_json::{Value, json};
use support::direct_host::{BUDGET, FixtureProcess, Launch, request_json, wait_for_store};

const MIB: u64 = 1024 * 1024;
const GLIBC_ARENA_HEAP_BYTES: u64 = 64 * MIB;
const SESSION_GROWTH_BOUND_BYTES: u64 = MIB;
const SESSION_KEYED_HOLDERS: [&str; 4] = [
    "snapshots",
    "native_attach",
    "boundary_token",
    "tail_hygiene_memo",
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
    parse_mappings(&std::fs::read_to_string(format!("/proc/{pid}/maps")).expect("process maps"))
}

fn parse_mappings(maps: &str) -> Vec<Mapping> {
    maps.lines()
        .map(|line| {
            let mut fields = line.split_whitespace();
            let (start, end) = fields
                .next()
                .and_then(|range| range.split_once('-'))
                .expect("address range");
            let permissions = fields.next().expect("permissions");
            let inode = fields.nth(2).expect("inode");
            let label = fields.next();
            Mapping {
                start: u64::from_str_radix(start, 16).expect("start address"),
                end: u64::from_str_radix(end, 16).expect("end address"),
                readable: permissions.starts_with('r'),
                anonymous: inode == "0" && label.is_none_or(|label| label.starts_with("[anon:")),
            }
        })
        .collect()
}

/// Counts glibc arena heaps: a 64 MiB region aligned to its size, whose used part is readable
/// and whose unused tail is reserved without access. The kernel may merge the used part with a
/// readable mapping below it, so the used mapping may start before the region.
fn glibc_arena_heaps(mappings: &[Mapping]) -> usize {
    mappings
        .windows(2)
        .filter(|pair| {
            let (used, reserved) = (&pair[0], &pair[1]);
            used.anonymous
                && reserved.anonymous
                && used.readable
                && !reserved.readable
                && used.end == reserved.start
                && reserved.end % GLIBC_ARENA_HEAP_BYTES == 0
                && reserved.end - reserved.start < GLIBC_ARENA_HEAP_BYTES
                && used.start + GLIBC_ARENA_HEAP_BYTES <= reserved.end
        })
        .count()
}

#[test]
fn arena_heaps_are_counted_when_named_and_when_their_used_part_is_merged() {
    let maps = "\
7f0000000000-7f0000021000 rw-p 00000000 00:00 0
7f0000021000-7f0004000000 ---p 00000000 00:00 0
7f0007f00000-7f0008021000 rw-p 00000000 00:00 0
7f0008021000-7f000c000000 ---p 00000000 00:00 0
7f0010000000-7f0010021000 rw-p 00000000 00:00 0                          [anon:glibc: malloc arena]
7f0010021000-7f0014000000 ---p 00000000 00:00 0                          [anon:glibc: malloc arena]
7f0020000000-7f0020021000 rw-p 00000000 00:00 0
7f0020021000-7f0060000000 ---p 00000000 00:00 0
7f0070000000-7f0070021000 rw-p 00000000 08:01 131 /usr/lib/libc.so.6
7f0070021000-7f0074000000 ---p 00021000 08:01 131 /usr/lib/libc.so.6
7f0080001000-7f0080021000 rw-p 00000000 00:00 0
7f0080021000-7f0084000000 ---p 00000000 00:00 0
";
    assert_eq!(glibc_arena_heaps(&parse_mappings(maps)), 3);
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

async fn run_session(
    fixture: &FixtureProcess,
    session: usize,
    turns: usize,
    output_bytes: usize,
) -> Value {
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
    let live = request_json(&client, route, json!({"kind": "status"})).await;
    client
        .close_route(route)
        .await
        .expect("session route closes");
    client.close().await.expect("session client closes");
    live["memory_holders"].clone()
}

fn operator_set_arena_limit() -> bool {
    std::env::var_os("MALLOC_ARENA_MAX").is_some_and(|value| !value.is_empty())
        || std::env::var("GLIBC_TUNABLES").is_ok_and(|tunables| {
            tunables.split(':').any(|tunable| {
                tunable
                    .strip_prefix("glibc.malloc.arena_max=")
                    .is_some_and(|value| !value.is_empty())
            })
        })
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
    let sessions = env_count("EIDNARA_SOAK_SESSIONS", 40);
    let turns = env_count("EIDNARA_SOAK_TURNS", 6);
    let output_bytes = env_count("EIDNARA_SOAK_OUTPUT_BYTES", 384 * 1024);
    let window = sessions / 4;
    assert!(window >= 2, "four windows of at least two sessions");
    assert!(
        (turns * output_bytes) as u64 >= 2 * SESSION_GROWTH_BOUND_BYTES,
        "one session's request window is at least twice the growth allowed per session"
    );

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

    let heap_reported = support::glibc::exports_mallinfo2();
    let mut anonymous = Vec::with_capacity(sessions);
    for session in 0..sessions {
        let live = run_session(&fixture, session, turns, output_bytes).await;
        for holder in SESSION_KEYED_HOLDERS {
            assert!(
                live[holder]["entry_count"].as_u64().expect("entry count") >= 1,
                "a live session holds a `{holder}` entry: {live}"
            );
        }
        status_after_teardown(&observer, observer_route).await;
        let before = rss_anon_bytes(pid);
        let status = request_json(&observer, observer_route, json!({"kind": "status"})).await;
        let sampled = rss_anon_bytes(pid);
        let reported = status["process_memory"]["rss_anon_bytes"]
            .as_u64()
            .expect("the status reports anonymous resident bytes");
        let (low, high) = (before.min(sampled), before.max(sampled));
        assert!(
            (low - low / 2..=high + high / 2).contains(&reported),
            "status reports {reported} bytes, /proc reports {before} before it and {sampled} after"
        );
        assert_eq!(
            status["process_memory"]["c_heap"]["in_use_bytes"].is_u64(),
            heap_reported,
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
    let early = low_water(2 * window..3 * window);
    let late = low_water(sessions - window..sessions);
    let between = window as u64;
    assert!(
        late <= early + SESSION_GROWTH_BOUND_BYTES * between,
        "anonymous memory grew {} MiB over {between} sessions: {:?}",
        late.saturating_sub(early) / MIB,
        anonymous
            .iter()
            .map(|bytes| bytes / MIB)
            .collect::<Vec<_>>()
    );

    if !operator_set_arena_limit() {
        let status = status_after_teardown(&observer, observer_route).await;
        assert_eq!(
            status["process_memory"]["c_heap_arena_limit"], C_HEAP_ARENAS,
            "the fixture sets the arena limit before it starts threads"
        );
        let arena_heaps = glibc_arena_heaps(&mappings(pid));
        assert!(
            (1..C_HEAP_ARENAS).contains(&arena_heaps),
            "{arena_heaps} heaps outside the main arena under a limit of {C_HEAP_ARENAS} arenas"
        );
    }

    observer
        .close_route(observer_route)
        .await
        .expect("observer route closes");
    observer.close().await.expect("observer client closes");
    fixture.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_operator_arena_setting_keeps_the_daemon_from_setting_its_own() {
    let root = tempfile::tempdir().expect("state root");
    std::fs::create_dir_all(root.path().join("project")).expect("project root");
    let config_home = root.path().join("config-home");
    std::fs::create_dir_all(&config_home).expect("config home");
    let fixture = Launch::at(root.path().to_path_buf())
        .config_home(&config_home)
        .env("GLIBC_TUNABLES", "glibc.malloc.arena_max=8")
        .start();
    let client = fixture.client().await;
    let route = fixture
        .open_route(&client, "context", TargetKind::ToolProvider, "tuned")
        .await;
    wait_for_store(&client, route, "tuned").await;
    let status = request_json(&client, route, json!({"kind": "status"})).await;
    assert_eq!(status["process_memory"]["c_heap_arena_limit"], Value::Null);
    assert!(status["process_memory"]["rss_anon_bytes"].is_u64());
    client.close_route(route).await.expect("route closes");
    client.close().await.expect("client closes");
    fixture.shutdown();
}

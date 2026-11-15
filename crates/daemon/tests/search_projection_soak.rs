//! A long-running daemon keeps its search projection caught up while its store grows, commits keep arriving, the wall clock steps, and searches run.
//!
//! The daemon runs in process with the admission records the direct-host fixture installs for its production search route, the production query route limits, the claim-source runner, and the scheduled slice loop. The workload writes what a coding session writes to the kernel: source descriptors for messages and tool output, and memory decisions the claim-source runner publishes. The wall clock steps through `kernel::test_clock`, which the kernel's hold expiries and the daemon's record deadlines both read, while monotonic time keeps its pace, as after a host suspend.
//!
//! What the soak establishes is bounded by two conditions on the wall clock. The projection stays caught up without a rebuild while the clock has moved forward by less than the source hold's lifetime since the family was built (30 days under the fixture records); the steady phase keeps its steps inside that lifetime. Past it the hold expires and the next commit costs one rebuild of the whole store, which the last phase exercises with the workload still committing. A forward step longer than a rebuild record's remaining time that lands after the rebuild staged its seed leaves that record for the operator (`docs/search-projection.md`, *Expiry and renewal*); the soak takes no step during its rebuild.
//!
//! `EIDNARA_SOAK_DESCRIPTORS` and `EIDNARA_SOAK_SECONDS` scale the store and the steady phase; the defaults finish in a few minutes.

mod support;

#[path = "../examples/search_admission_records/records.rs"]
mod records;

use std::sync::Arc;
use std::time::{Duration, Instant};

use daemon::dispatch::PreparedOutcome;
use daemon::projection_gates::ProjectionHook;
use daemon::query_route::QueryRouteLimits;
use kernel::source_identity::Occurrence;
use kernel::{
    AdmissionEvent, AdmissionRequest, ArtifactIngestRequest, CommitIntent, DomainSpec, EventKind,
    KernelStore, ProviderEgress, Sensitivity, SourceClass, SourceDescriptorRequest, TaintClass,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::embedding_fixtures::{DIMS, FINGERPRINT, MODEL, identity, kernel_incarnation_id};
use support::kernel_daemon::{KernelDaemon, SESSION, StartOptions};
use support::projection_gate::{self, campaign_json, manifest_json_with, write_records};

/// The stepped wall clock is process-wide, so the tests of this binary run one at a time.
static CLOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The wall-clock steps one test made; dropping it steps the clock back by their sum, so the next test of the binary starts from the system clock.
#[derive(Default)]
struct ClockSteps(i64);

impl ClockSteps {
    fn step(&mut self, by_ms: i64) {
        kernel::test_clock::step(by_ms);
        self.0 += by_ms;
    }
}

impl Drop for ClockSteps {
    fn drop(&mut self) {
        kernel::test_clock::step(-self.0);
    }
}

const DOMAIN: &str = "soak-capture";
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// The daemon's slice loop idles this long once the family is at the tip, so a commit reaches the projection within one idle period and the slice that follows it.
const SLICE_IDLE: Duration = daemon::search_lifecycle_owner::SLICE_IDLE;

/// Words of ordinary engineering prose; a seeded sequence of them is one source text.
const WORDS: [&str; 48] = [
    "request",
    "handler",
    "returns",
    "cache",
    "entry",
    "after",
    "retry",
    "when",
    "the",
    "worker",
    "queue",
    "drains",
    "before",
    "shutdown",
    "config",
    "loads",
    "from",
    "file",
    "and",
    "falls",
    "back",
    "to",
    "defaults",
    "test",
    "asserts",
    "timeout",
    "error",
    "path",
    "logs",
    "each",
    "connection",
    "closes",
    "on",
    "idle",
    "parser",
    "rejects",
    "empty",
    "input",
    "index",
    "rebuilds",
    "nightly",
    "migration",
    "adds",
    "column",
    "with",
    "default",
    "value",
    "branch",
];

/// A deterministic text of about `bytes` bytes, distinct per `seed`.
fn prose(seed: u64, bytes: usize) -> String {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut text = String::with_capacity(bytes + 16);
    while text.len() < bytes {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        text.push_str(WORDS[(state % WORDS.len() as u64) as usize]);
        text.push(' ');
    }
    text
}

/// Source text sizes in bytes, cycled per descriptor: mostly short messages, some tool output of a few kilobytes, and an occasional large one.
const SIZES: [usize; 8] = [180, 420, 260, 900, 340, 2_400, 520, 12_000];

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "search-projection-soak".to_owned(),
        operation_key: key.to_owned(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_owned(),
        cause: "soak".to_owned(),
    }
}

/// Publishes descriptors `first..first + count` in one kernel commit, alternating message and tool-output occurrences, each over its own evidence artifact, as a harness transcript publication does.
fn publish(kernel: &KernelStore, scope: &str, first: u64, count: u64) {
    let mut staged = Vec::new();
    for index in first..first + count {
        let key = format!("d{index}");
        let text = prose(index, SIZES[(index % SIZES.len() as u64) as usize]);
        let handle = kernel
            .ingest_exact_artifact(ArtifactIngestRequest {
                intent: intent(&format!("artifact-{key}")),
                payload: text.as_bytes().to_vec(),
                evidence_id: format!("evidence-{key}"),
                object_id: format!("evidence-object-{key}"),
                object_kind: "evidence".to_owned(),
                domain_id: DOMAIN.to_owned(),
                source_kind: "tool_output".to_owned(),
                source_id: format!("native/{key}"),
                source_revision: 1,
                media_type: "text/plain".to_owned(),
                retention_class: "canonical".to_owned(),
                retain_until: None,
                asserted_sensitivity: Sensitivity::Normal,
                provider_egress: ProviderEgress::RemoteAllowed,
                provenance: None,
            })
            .unwrap();
        staged.push((index, key, text, handle));
    }
    kernel
        .commit(intent(&format!("publish-{first}")), |envelope| {
            for (index, key, text, handle) in &staged {
                let tool = !index.is_multiple_of(2);
                let identity: Vec<(&str, &str)> = if tool {
                    vec![
                        ("project_id", "soak"),
                        ("harness", "pi"),
                        ("session_id", "soak-session"),
                        ("parent_message_id", "message"),
                        ("tool_call_id", key),
                        ("result_revision", "1"),
                        ("block_index", "0"),
                    ]
                } else {
                    vec![
                        ("project_id", "soak"),
                        ("harness", "pi"),
                        ("session_id", "soak-session"),
                        ("message_id", key),
                        ("block_index", "0"),
                    ]
                };
                let outcome = envelope
                    .publish_source_descriptor(&SourceDescriptorRequest {
                        source_policy: kernel::SourceDescriptorPolicy::Native,
                        occurrence: Occurrence {
                            class: if tool { "raw_tool_spans" } else { "messages" },
                            identity: &identity,
                            revision: "1",
                            representation: if tool { "tool_output" } else { "text" },
                            span: None,
                        },
                        domain_id: DOMAIN,
                        scope_id: Some(scope),
                        evidence_id: &handle.evidence_id,
                        artifact_digest: &handle.digest,
                        buffer: text,
                        sensitivity: Sensitivity::Normal,
                        observed_at: 1,
                        expected_head: None,
                        outcome: None,
                    })
                    .unwrap();
                envelope.record_admission(AdmissionRequest {
                    candidate_id: None,
                    subject_object_id: Some(outcome.object_id),
                    source_class: Some(SourceClass::ExplicitUser),
                    taint_class: Some(TaintClass::UserExplicit),
                    event: AdmissionEvent {
                        kind: EventKind::Other,
                        trigger_object_id: None,
                        approval_object_id: None,
                        evidence_id: None,
                        reason: "soak".to_owned(),
                    },
                })?;
            }
            Ok(String::new())
        })
        .unwrap();
}

/// The unique word memory `index` carries, which only that memory matches.
fn marker(index: u64) -> String {
    format!("soakmarker{index:06}")
}

fn memory_id(index: u64) -> String {
    format!("soak-memory-{index}")
}

/// Commits one project memory through the kernel route; the claim-source runner publishes its source descriptor.
async fn remember(daemon: &KernelDaemon, index: u64) {
    let created = daemon
        .commit(
            &format!("memory-{index}"),
            vec![json!({"op": "insert_decision", "spec": {
                "decision_id": format!("soak-decision-{index}"),
                "object_id": memory_id(index),
                "domain_id": "memory",
                "decision_kind": "PROJECT_RULES",
                "payload": {
                    "summary": format!("{} {}", marker(index), prose(1_000_000 + index, 120)),
                    "rationale": "recorded during the soak",
                },
                "source_id": format!("soak-lineage-{index}"),
                "source_revision": 1,
            }})],
        )
        .await;
    assert_eq!(created["state"]["kind"], "available", "{created}");
}

/// One `retrieval.query`: `Ok` with the fused answer, or `Err` with the terminal or error code the route gave.
async fn query(daemon: &KernelDaemon, text: &str) -> Result<Value, String> {
    let outcome = daemon
        .outcome(json!({
            "method": "retrieval.query",
            "session_id": SESSION,
            "project_root": daemon.project().to_str().unwrap(),
            "query": text,
            "remaining_ms": 5_000,
            "destination": "local",
        }))
        .await;
    match outcome {
        PreparedOutcome::Response(output) => {
            let value = output.json_for_test().unwrap().clone();
            if value["kind"] == "fused" {
                Ok(value)
            } else {
                Err(format!(
                    "{}:{}",
                    value["terminal"].as_str().unwrap_or("?"),
                    value["reason"].as_str().unwrap_or("")
                ))
            }
        }
        PreparedOutcome::Error { code, .. } => Err(code),
        PreparedOutcome::Streamed => Err("streamed".to_owned()),
    }
}

/// Whether a fused answer ranks memory `index`.
fn ranks(answer: &Value, index: u64) -> bool {
    answer["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["canonical"]["decision_object_id"] == Value::String(memory_id(index)))
}

async fn status(daemon: &KernelDaemon, route: host_runtime::RouteHandle) -> Value {
    daemon
        .call_on(
            route,
            support::kernel_daemon::envelope(
                "search.lifecycle.status",
                daemon.project(),
                json!({}),
            ),
        )
        .await
}

/// Polls the status surface until `done` holds for it, within `limit`.
async fn status_until(
    daemon: &KernelDaemon,
    route: host_runtime::RouteHandle,
    limit: Duration,
    what: &str,
    done: impl Fn(&Value) -> bool,
) -> (Value, Duration) {
    let started = Instant::now();
    loop {
        let observed = status(daemon, route).await;
        if done(&observed) {
            return (observed, started.elapsed());
        }
        assert!(
            started.elapsed() < limit,
            "{what} did not hold within {limit:?}: {observed}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn caught_up(status: &Value) -> bool {
    status["record"]["state"] == "current"
        && status["admission"]["state"] == "admitted"
        && status["progress"]["lag_commits"] == 0
}

fn env_number(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .map(|value| {
            value
                .parse()
                .unwrap_or_else(|_| panic!("{name} is a number"))
        })
        .unwrap_or(default)
}

/// A daemon over an empty kernel with `manifest`'s limits installed, the production query route limits, and the claim-source runner on. Returns it with an operator route and the project's scope id, once the first build is Current.
async fn daemon_with(
    manifest: impl FnOnce(&retrieval::ProjectionIdentity) -> Value,
) -> (KernelDaemon, host_runtime::RouteHandle, String) {
    let data = tempfile::tempdir().unwrap();
    let home = data.path().to_owned();
    let kernel_root = home.join("eidnara").join("context");
    drop(KernelStore::open(kernel_root.join("kernel")).unwrap());
    let identity = identity(&kernel_incarnation_id(&kernel_root));
    write_records(&home, &manifest(&identity), &campaign_json(&identity));
    let daemon = KernelDaemon::start_with(StartOptions {
        data: Some(data),
        claim_sources: true,
        ..StartOptions::default()
    })
    .await;
    daemon
        .handler()
        .set_query_route_limits(Some(QueryRouteLimits::production()))
        .unwrap();
    let route = daemon.bind_another(2, "cli").await;
    // The first memory materializes the project's scope and the memory domain through the kernel route.
    remember(&daemon, 0).await;
    let scope = format!("project:{}", daemon.project_digest());
    daemon
        .store()
        .commit(intent("soak-domain"), |envelope| {
            envelope.insert_domain(DomainSpec {
                domain_id: DOMAIN.to_owned(),
                object_id: "soak-domain-object".to_owned(),
                name: "Soak capture".to_owned(),
                source_kind: "fixture".to_owned(),
                source_id: DOMAIN.to_owned(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
            })?;
            Ok(String::new())
        })
        .unwrap();
    status_until(
        &daemon,
        route,
        Duration::from_secs(60),
        "the first build",
        caught_up,
    )
    .await;
    (daemon, route, scope)
}

/// The records the direct-host fixture installs for its production search route, for the test lane's identity.
fn fixture_manifest(identity: &retrieval::ProjectionIdentity) -> Value {
    let bundle = json!({
        "model": MODEL,
        "fingerprint": FINGERPRINT,
        "dims": DIMS,
        "table_epoch": identity.generation_epoch,
    });
    let (manifest, _campaign) = records::records(&bundle).unwrap();
    manifest
}

fn percentile(sorted: &[u128], percent: usize) -> u128 {
    sorted[(sorted.len() * percent / 100).min(sorted.len() - 1)]
}

/// Under the fixture's production-route records, a store grown past the unit-sized bounds stays within the reader lag bound through several commits a second and wall-clock steps of minutes to days inside the hold's lifetime: at most one search in a hundred fails, every new memory is ranked within two idle periods, the status lag moves with the commits and returns to zero at the kernel's own tip, and no rebuild is recorded. A step past the hold lifetime then costs one rebuild, which completes while the workload keeps committing and serves again; searches during the build slice may answer `deadline`, and nothing else.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_grown_store_stays_caught_up_through_commits_clock_steps_and_searches() {
    let _clock = CLOCK.lock().await;
    let descriptors = env_number("EIDNARA_SOAK_DESCRIPTORS", 4_400);
    let steady = Duration::from_secs(env_number("EIDNARA_SOAK_SECONDS", 30));
    // The knobs scale the soak up. Below these values a class stays inside the unit-sized coverage bound of 682 live rows, or the steady phase ends before a clock step and a memory are sampled.
    assert!(
        descriptors >= 1_400 && steady >= Duration::from_secs(10),
        "EIDNARA_SOAK_DESCRIPTORS is at least 1400 and EIDNARA_SOAK_SECONDS at least 10"
    );
    let (daemon, route, scope) = daemon_with(fixture_manifest).await;
    let kernel = daemon.store();
    let lag_bound = i64::try_from(records::READER_LAG_COMMITS).unwrap();
    let hold_ms = i64::try_from(records::HOLD_MS).unwrap();
    let mut clock = ClockSteps::default();

    // Growth: descriptors arrive four to a commit while the slice loop catches up behind them.
    let growth_started = Instant::now();
    let grower = {
        let kernel = Arc::clone(&kernel);
        let scope = scope.clone();
        tokio::task::spawn_blocking(move || {
            for first in (0..descriptors).step_by(4) {
                publish(&kernel, &scope, first, 4.min(descriptors - first));
            }
        })
    };
    grower.await.unwrap();
    let grown_in = growth_started.elapsed();
    let (grown, drained_in) = status_until(
        &daemon,
        route,
        Duration::from_secs(600),
        "catch-up with the grown store",
        caught_up,
    )
    .await;
    assert_eq!(
        grown["progress"]["rebuilds"],
        json!({"recorded": 1, "completed": 1, "renewed": 0}),
        "growth is applied by catch-up under the first build's hold: {grown}"
    );

    // Steady phase: each tick of at least 250 ms publishes two descriptors (three kernel commits), commits a memory every fifth tick, and searches once; the wall clock steps every five seconds.
    let steps_ms: [i64; 8] = [
        20 * 60 * 1000,
        2 * 60 * 60 * 1000,
        26 * 60 * 60 * 1000,
        5 * 60 * 1000,
        63 * 60 * 60 * 1000,
        45 * 60 * 1000,
        3 * DAY_MS,
        17 * 60 * 1000,
    ];
    let mut next = descriptors;
    let mut memory = 1_u64;
    let mut pending: Vec<(u64, Instant)> = Vec::new();
    let mut freshness_ms: Vec<u128> = Vec::new();
    let mut latency_ms: Vec<u128> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut searches = 0_u64;
    let mut max_lag = 0_i64;
    let mut stepped_ms = 0_i64;
    let mut step = 0_usize;
    let started = Instant::now();
    let mut tick = 0_u64;
    let mut last_step = Instant::now();
    while started.elapsed() < steady {
        let tick_started = Instant::now();
        {
            let kernel = Arc::clone(&kernel);
            let scope = scope.clone();
            let first = next;
            tokio::task::spawn_blocking(move || publish(&kernel, &scope, first, 2))
                .await
                .unwrap();
            next += 2;
        }
        if tick.is_multiple_of(5) {
            remember(&daemon, memory).await;
            pending.push((memory, Instant::now()));
            memory += 1;
        }
        // The search asks for the oldest memory not yet seen ranked, or the newest ranked one once all are.
        let target = pending.first().map_or(memory - 1, |(index, _)| *index);
        let asked = Instant::now();
        searches += 1;
        match query(&daemon, &marker(target)).await {
            Ok(answer) => {
                latency_ms.push(asked.elapsed().as_millis());
                if let Some((index, committed)) = pending.first().copied()
                    && ranks(&answer, index)
                {
                    freshness_ms.push(committed.elapsed().as_millis());
                    pending.remove(0);
                }
            }
            Err(code) => errors.push(code),
        }
        let observed = status(&daemon, route).await;
        let progress = &observed["progress"];
        let lag = progress["lag_commits"].as_i64().unwrap_or(i64::MAX);
        // The reported lag is the distance between the two commits the same answer reports.
        assert_eq!(
            Some(lag),
            progress["tip"]
                .as_i64()
                .zip(progress["checkpoint"].as_i64())
                .map(|(tip, checkpoint)| tip - checkpoint),
            "{observed}"
        );
        max_lag = max_lag.max(lag);
        // Steps stop two days short of the hold's lifetime, so a longer steady phase keeps testing a family under its first hold.
        let by = steps_ms[step % steps_ms.len()];
        if last_step.elapsed() >= Duration::from_secs(5) && stepped_ms + by <= hold_ms - 2 * DAY_MS
        {
            clock.step(by);
            stepped_ms += by;
            step += 1;
            last_step = Instant::now();
        }
        tick += 1;
        tokio::time::sleep(Duration::from_millis(250).saturating_sub(tick_started.elapsed())).await;
    }
    // Every memory committed in the steady phase is ranked within two idle periods of the last commit.
    let drain_started = Instant::now();
    while let Some((index, committed)) = pending.first().copied() {
        if let Ok(answer) = query(&daemon, &marker(index)).await
            && ranks(&answer, index)
        {
            freshness_ms.push(committed.elapsed().as_millis());
            pending.remove(0);
            continue;
        }
        assert!(
            drain_started.elapsed() < 2 * SLICE_IDLE + Duration::from_secs(5),
            "memory {index} was never ranked"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // With the workload stopped, the status lag returns to zero at the tip the kernel itself reports.
    let (after_steady, _) = status_until(
        &daemon,
        route,
        2 * SLICE_IDLE + Duration::from_secs(5),
        "the steady phase's last commits",
        caught_up,
    )
    .await;
    assert_eq!(
        after_steady["progress"]["checkpoint"],
        json!(kernel.tip().unwrap()),
        "{after_steady}"
    );
    assert_eq!(
        after_steady["progress"]["tip"],
        json!(kernel.tip().unwrap()),
        "{after_steady}"
    );
    freshness_ms.sort_unstable();
    latency_ms.sort_unstable();

    // Hold expiry: the wall clock steps past the hold's lifetime, which costs one rebuild of the whole store. The workload keeps committing at the steady rate while the rebuild runs.
    clock.step(hold_ms + DAY_MS - stepped_ms);
    remember(&daemon, memory).await;
    let expiry_started = Instant::now();
    let mut rebuild_errors: Vec<String> = Vec::new();
    let mut rebuild_searches = 0_u64;
    let mut rebuild_commits = 0_u64;
    let rebuilt = loop {
        {
            let kernel = Arc::clone(&kernel);
            let scope = scope.clone();
            let first = next;
            tokio::task::spawn_blocking(move || publish(&kernel, &scope, first, 2))
                .await
                .unwrap();
            next += 2;
            rebuild_commits += 3;
        }
        rebuild_searches += 1;
        let answer = query(&daemon, &marker(memory)).await;
        if let Err(code) = &answer {
            rebuild_errors.push(code.clone());
        }
        let observed = status(&daemon, route).await;
        if observed["progress"]["rebuilds"]["completed"] == 2
            && observed["record"]["state"] == "current"
            && observed["admission"]["state"] == "admitted"
            && answer.is_ok_and(|answer| ranks(&answer, memory))
        {
            break observed;
        }
        assert!(
            expiry_started.elapsed() < Duration::from_secs(600),
            "the rebuild after the hold expired did not complete: {observed}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    let rebuilt_in = expiry_started.elapsed();

    let facts = kernel.facts(1).unwrap();
    println!(
        "[projection-soak] {}",
        json!({
            "descriptors": next,
            "memories": memory + 1,
            "kernel_commits": kernel.tip().unwrap(),
            "kernel_main_bytes": facts.main_file_bytes,
            "growth_seconds": grown_in.as_secs_f64(),
            "growth_drain_seconds": drained_in.as_secs_f64(),
            "steady_seconds": steady.as_secs(),
            "steady_ticks": tick,
            "clock_steps": step,
            "clock_stepped_days": stepped_ms as f64 / DAY_MS as f64,
            "searches": searches,
            "search_errors": errors.len(),
            "search_error_codes": errors.iter().take(8).collect::<Vec<_>>(),
            "search_latency_ms": {"p50": percentile(&latency_ms, 50), "p95": percentile(&latency_ms, 95), "max": latency_ms.last()},
            "max_lag_commits": max_lag,
            "freshness_ms": {"p50": percentile(&freshness_ms, 50), "p95": percentile(&freshness_ms, 95), "max": freshness_ms.last()},
            "rebuilds_after_steady": after_steady["progress"]["rebuilds"],
            "hold_expiry_rebuild_seconds": rebuilt_in.as_secs_f64(),
            "hold_expiry_searches": rebuild_searches,
            "hold_expiry_commits": rebuild_commits,
            "hold_expiry_search_errors": rebuild_errors.len(),
            "hold_expiry_search_error_codes": rebuild_errors.iter().collect::<std::collections::BTreeSet<_>>(),
            "rebuilds_at_end": rebuilt["progress"]["rebuilds"],
        })
    );

    assert!(
        (errors.len() as u64) * 100 <= searches,
        "{} of {searches} searches failed in the steady phase: {:?}",
        errors.len(),
        &errors[..errors.len().min(8)]
    );
    assert!(
        max_lag <= lag_bound,
        "the projection trailed the kernel by {max_lag} commits, above {lag_bound}"
    );
    assert!(
        max_lag > 0,
        "the status lag stayed at zero while every tick committed, so it did not measure the lag"
    );
    assert!(
        rebuild_errors.iter().all(|code| code == "deadline:"),
        "a search during the rebuild failed other than by waiting out the build slice: {rebuild_errors:?}"
    );
    let slowest = *freshness_ms.last().unwrap();
    assert!(
        slowest <= (2 * SLICE_IDLE + Duration::from_secs(5)).as_millis(),
        "a memory took {slowest} ms to be ranked"
    );
    assert_eq!(
        after_steady["progress"]["rebuilds"],
        json!({"recorded": 1, "completed": 1, "renewed": 0}),
        "{stepped_ms} ms of clock steps inside the hold lifetime record no rebuild: {after_steady}"
    );
    assert_eq!(
        after_steady["progress"]["blocked"],
        Value::Null,
        "{after_steady}"
    );
    assert_eq!(
        rebuilt["progress"]["rebuilds"],
        json!({"recorded": 2, "completed": 2, "renewed": 0}),
        "{rebuilt}"
    );
    assert_eq!(rebuilt["record"]["cause"], "CatchUpHoldLost", "{rebuilt}");
    daemon.shutdown().await;
}

/// A reader lag bound smaller than one idle period's commits, as an operator's manifest may set: growth pushes the family past the bound at a slice's observation, the status reports readers refused as stale, and the family's own catch-up episode still applies the commits, so the projection returns to the tip with no rebuild once the commits stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_family_past_a_small_reader_lag_bound_is_refused_to_readers_and_catches_up() {
    let _clock = CLOCK.lock().await;
    let (daemon, route, scope) = daemon_with(|identity| {
        let mut manifest = fixture_manifest(identity);
        manifest["limits"]["catchup_lag_commits"] = json!(8);
        manifest
    })
    .await;
    let kernel = daemon.store();
    // Growth runs until a status shows readers refused as stale, which a slice's observation produces at the first idle period that ends with more than eight commits behind it.
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let grower = {
        let stop = Arc::clone(&stop);
        tokio::task::spawn_blocking(move || {
            // The publisher ends on the flag, and on its own clock if the test ends before it sets the flag.
            let started = Instant::now();
            let mut first = 0_u64;
            while !stop.load(std::sync::atomic::Ordering::Acquire)
                && started.elapsed() < Duration::from_secs(90)
            {
                publish(&kernel, &scope, first, 4);
                first += 4;
            }
        })
    };
    let growth_started = Instant::now();
    let mut refused_as_stale = false;
    while !refused_as_stale && growth_started.elapsed() < Duration::from_secs(60) {
        let observed = status(&daemon, route).await;
        refused_as_stale = observed["admission"]["reason"] == "coverage_stale";
        tokio::task::yield_now().await;
    }
    stop.store(true, std::sync::atomic::Ordering::Release);
    grower.await.unwrap();
    assert!(
        refused_as_stale,
        "no status showed the family past a bound of 8 in a minute of commits, so this run reached no stale family"
    );
    let (caught_up_status, _) = status_until(
        &daemon,
        route,
        2 * SLICE_IDLE + Duration::from_secs(20),
        "catch-up of the stale family",
        caught_up,
    )
    .await;
    assert_eq!(
        caught_up_status["progress"]["rebuilds"],
        json!({"recorded": 1, "completed": 1, "renewed": 0}),
        "{caught_up_status}"
    );
    assert_eq!(caught_up_status["progress"]["blocked"], Value::Null);
    assert!(query(&daemon, &marker(0)).await.is_ok());
    daemon.shutdown().await;
}

/// Positive control for the soak's oracle: under the records the fixture installed before they were sized for a growing store (the integration tests' values with catch-up windows of 4,096 rows and 8 MiB), a class of the same workload passes its coverage bound of 682 live rows, the gate withdraws admission, searches stop being served, and the status surface names the bound that refused the family.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unit_sized_limits_stop_serving_a_grown_store_and_the_status_names_the_bound() {
    let _clock = CLOCK.lock().await;
    let (daemon, route, scope) = daemon_with(|identity| {
        manifest_json_with(
            identity,
            &ProjectionHook::ALL,
            &[
                ("catchup_lag_commits", projection_gate::LIMIT),
                ("local_transaction_rows", 2 * 4096),
                ("local_transaction_bytes", 4 * 4096 * 4096),
                ("catchup_batch_encoded_bytes", 2 * (8 << 20)),
                ("catchup_batch_source_bytes", 8 << 20),
                ("export_live_decoded_bytes", 2 * (8 << 20)),
            ],
        )
    })
    .await;
    assert!(query(&daemon, &marker(0)).await.is_ok());
    let kernel = daemon.store();
    tokio::task::spawn_blocking(move || {
        for first in (0..1_600_u64).step_by(4) {
            publish(&kernel, &scope, first, 4);
        }
    })
    .await
    .unwrap();
    let (blocked, _) = status_until(
        &daemon,
        route,
        Duration::from_secs(120),
        "a refused family with its reason",
        |status| {
            status["admission"]["state"] == "refused" && status["progress"]["blocked"].is_string()
        },
    )
    .await;
    assert_eq!(
        blocked["admission"]["reason"], "evidence_missing",
        "{blocked}"
    );
    let reason = blocked["progress"]["blocked"].as_str().unwrap_or_default();
    assert!(
        reason.contains("coverage exceeds its observation bound"),
        "{blocked}"
    );
    assert_eq!(blocked["progress"]["checkpoint"], Value::Null, "{blocked}");
    assert_eq!(
        query(&daemon, &marker(0)).await.unwrap_err(),
        "lane_unavailable:evidence_missing"
    );
    daemon.shutdown().await;
}

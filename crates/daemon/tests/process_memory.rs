#![cfg(all(target_os = "linux", target_env = "gnu"))]

mod support;

use std::time::{Duration, Instant};

use daemon::process_memory::{C_HEAP_RELEASE_THRESHOLD_BYTES, ProcessMemory};
use host_runtime::{BindOutcome, CompositeComponent, RouteHandle, RouteIdentity};
use serde_json::{Value, json};
use support::kernel_daemon::KernelDaemon;

static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct FragmentedCHeap {
    kept: Vec<Vec<u8>>,
    freed_bytes: u64,
    resident_before: u64,
}

impl FragmentedCHeap {
    fn new() -> Self {
        const CHUNK_BYTES: usize = 64 * 1024;
        const CHUNKS: usize = 4096;
        const KEEP_EVERY: usize = 64;
        let resident_before = resident();
        let chunks: Vec<Vec<u8>> = (0..CHUNKS).map(|_| vec![1_u8; CHUNK_BYTES]).collect();
        let kept: Vec<Vec<u8>> = chunks
            .into_iter()
            .enumerate()
            .filter_map(|(index, chunk)| (index % KEEP_EVERY == 0).then_some(chunk))
            .collect();
        let freed_bytes = (CHUNK_BYTES * (CHUNKS - kept.len())) as u64;
        assert!(freed_bytes / 2 > C_HEAP_RELEASE_THRESHOLD_BYTES);
        Self {
            kept,
            freed_bytes,
            resident_before,
        }
    }

    fn is_resident(&self) -> bool {
        resident() >= self.resident_before + self.freed_bytes / 2
    }

    async fn released(&self) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while self.is_resident() {
            assert!(
                Instant::now() < deadline,
                "no release returned the freed pages: {:?}",
                ProcessMemory::sample()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(self.kept.len(), 64);
    }
}

fn resident() -> u64 {
    ProcessMemory::sample()
        .rss_anon_bytes
        .expect("anonymous resident bytes")
}

async fn process_memory(daemon: &KernelDaemon) -> Value {
    daemon.call(json!({"kind": "status"})).await["process_memory"].clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_health_probe_releases_freed_c_heap_pages_once_per_growth_past_the_threshold() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let daemon = KernelDaemon::start().await;
    let releases_before = process_memory(&daemon).await["c_heap_releases"]
        .as_u64()
        .expect("release count");

    let heap = FragmentedCHeap::new();
    assert!(
        heap.is_resident(),
        "freed chunks stay resident until a release"
    );
    let _ = CompositeComponent::health(daemon.handler()).await;
    heap.released().await;

    let reported = process_memory(&daemon).await;
    assert_eq!(reported["c_heap_releases"], releases_before + 1);
    assert!(
        reported["c_heap_release_resident_drop_bytes"]
            .as_u64()
            .expect("drop")
            > heap.freed_bytes / 2
    );
    assert!(
        reported["c_heap"]["free_bytes"]
            .as_u64()
            .expect("free bytes")
            > heap.freed_bytes / 2
    );
    assert!(reported["c_heap"]["in_use_bytes"].is_u64());
    assert!(reported["rss_anon_bytes"].is_u64() && reported["rss_file_bytes"].is_u64());
    assert!(reported["threads"].as_u64().expect("threads") >= 1);

    let _ = CompositeComponent::health(daemon.handler()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        process_memory(&daemon).await["c_heap_releases"],
        releases_before + 1,
        "freed chunks whose pages are released start no second release"
    );
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_a_sessions_last_route_releases_freed_c_heap_pages() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let daemon = KernelDaemon::start().await;
    let route = RouteHandle {
        channel: 40,
        epoch: 1,
    };
    let identity = RouteIdentity {
        project_root: daemon.project().to_path_buf(),
        harness: "test".to_owned(),
        session: "session-that-ends".to_owned(),
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

    let heap = FragmentedCHeap::new();
    assert!(
        heap.is_resident(),
        "freed chunks stay resident until a release"
    );
    CompositeComponent::route_gone(daemon.handler(), route).await;
    heap.released().await;
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_arena_limit_is_left_unset_in_a_process_with_several_threads() {
    let _serial = ONE_AT_A_TIME.lock().await;
    assert!(ProcessMemory::sample().threads.expect("thread count") > 1);
    daemon::process_memory::limit_c_heap_arenas();
    assert_eq!(
        ProcessMemory::sample().to_json()["c_heap_arena_limit"],
        Value::Null
    );
}

#![cfg(all(target_os = "linux", target_env = "gnu"))]

use super::*;
use process_memory::{FragmentedCHeap, ProcessMemory};

async fn released(heap: &FragmentedCHeap) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !heap.is_released() {
        assert!(
            Instant::now() < deadline,
            "no release returned the freed pages: {:?}",
            ProcessMemory::sample()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_health_probe_releases_freed_c_heap_pages_past_the_threshold() {
    let producer = Arc::new(ProducerState::default());
    let (handler, _store, _dir, _project) = handler_with_store(producer, default_test_config());
    let heap = FragmentedCHeap::new();

    let _ = CompositeComponent::health(&handler).await;
    released(&heap).await;

    let status = response_of(handler.handle_status_value(&json!({"method": "status"})));
    let reported = &status["process_memory"];
    assert!(reported["c_heap_releases"].as_u64().unwrap() >= 1);
    assert!(reported["c_heap_released_bytes"].is_u64());
    assert!(reported["rss_anon_bytes"].is_u64() && reported["rss_file_bytes"].is_u64());
    assert!(reported["threads"].as_u64().unwrap() >= 1);
    assert!(reported["c_heap"]["free_bytes"].as_u64().unwrap() > heap.freed_bytes / 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_a_sessions_last_route_releases_freed_c_heap_pages_past_the_threshold() {
    let producer = Arc::new(ProducerState::default());
    let (handler, _store, _dir, _project) = handler_with_store(producer, default_test_config());
    let heap = FragmentedCHeap::new();

    handler.unbind_route(test_route(7));
    released(&heap).await;
}

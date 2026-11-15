#![cfg(all(target_os = "linux", target_env = "gnu"))]

use super::*;

async fn released_once() -> Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let memory = process_memory::ProcessMemory::sample().to_json();
        if memory["c_heap_releases"] == 1 {
            return memory;
        }
        assert!(Instant::now() < deadline, "no release ran: {memory}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn module_status(handler: &Handler) -> Value {
    response_of(handler.handle_status_value(&json!({"method": "status"})))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_health_probe_releases_freed_c_heap_pages_past_the_threshold() {
    let producer = Arc::new(ProducerState::default());
    let (handler, _store, _dir, _project) = handler_with_store(producer, default_test_config());
    let _ = CompositeComponent::health(&handler).await;
    assert_eq!(
        module_status(&handler)["process_memory"]["c_heap_releases"],
        0
    );

    let (kept, total) = process_memory::fragmented_c_heap();
    let held = process_memory::ProcessMemory::sample();
    let _ = CompositeComponent::health(&handler).await;
    released_once().await;

    let status = module_status(&handler);
    let reported = &status["process_memory"];
    assert_eq!(reported["c_heap_releases"], 1);
    assert!(reported["c_heap_released_bytes"].as_u64().unwrap() > total / 2);
    assert!(
        held.rss_anon_bytes.unwrap() - reported["rss_anon_bytes"].as_u64().unwrap() > total / 2,
        "{held:?} then {reported}"
    );
    assert!(reported["rss_file_bytes"].is_u64() && reported["threads"].is_u64());
    assert!(reported["c_heap"]["free_bytes"].is_u64());
    drop(kept);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_a_sessions_last_route_releases_freed_c_heap_pages_past_the_threshold() {
    let producer = Arc::new(ProducerState::default());
    let (handler, _store, _dir, _project) = handler_with_store(producer, default_test_config());
    let (kept, total) = process_memory::fragmented_c_heap();

    handler.unbind_route(test_route(7));
    let memory = released_once().await;
    assert!(memory["c_heap_released_bytes"].as_u64().unwrap() > total / 2);
    drop(kept);
}

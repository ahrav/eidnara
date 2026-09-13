use daemon::bench_internals::{self, CacheTtlProvenance, transform_cached};
use daemon::canonical_memory::{CanonicalMemoryRead, CanonicalMemorySnapshot};
use daemon::transform::{ProducerContext, TransformRequest};
use daemon::wire::IngressMessage;
use memory_store::MemoryStore;

pub fn request(session: &str, messages: &[IngressMessage], caveman: bool) -> TransformRequest {
    // Deserialize through serde so omitted fields use `TransformRequest` defaults.
    serde_json::from_value(serde_json::json!({
        "kind": "transform",
        "v": 2,
        "serializer_profile": "owned-llmrunner",
        "session_id": session,
        "render_config": "bench-config",
        "caveman_enabled": caveman,
        "messages": messages,
    }))
    .expect("bench transform request")
}

pub fn producer_ctx(dir: &str) -> ProducerContext<'_> {
    ProducerContext {
        project_memory: Some(CanonicalMemoryRead::Available(
            CanonicalMemorySnapshot::new(0, false, Vec::new()),
        )),
        project_path: "git:bench",
        note_project_path: "git:bench",
        project_directory: dir,
        history_budget_tokens: 60_000.0,
        user_profile_budget_tokens: 4_000.0,
        memory_enabled: true,
        inject_docs: true,
        temporal_awareness: true,
        now_ms: 1_700_000_000_000,
        execute_threshold_percentage: 65.0,
        compaction_enabled: true,
        smart_drops: false,
        cache_ttl: "5m".to_string(),
        cache_ttl_provenance: CacheTtlProvenance::Default,
        model_key: None,
        observed_last_response_at_ms: None,
        guidance_date: Some("Today's date: Thu Jan 01 2026".to_string()),
        historian_active: false,
        wrapup_active: false,
    }
}

pub fn fresh_store() -> (tempfile::TempDir, MemoryStore) {
    let dir = tempfile::tempdir().expect("bench store dir");
    let descriptor = daemon::store_descriptor_in(dir.path());
    let store = MemoryStore::open(&descriptor).expect("bench store");
    (dir, store)
}

/// Primes persistent state without retaining output-cache entries.
pub fn steady_state(
    messages: &[IngressMessage],
    caveman: bool,
) -> (tempfile::TempDir, MemoryStore, TransformRequest) {
    let (dir, store) = fresh_store();
    let req = request("bench-steady", messages, caveman);
    let ctx = producer_ctx(dir.path().to_str().expect("utf8 dir"));
    let cache = bench_internals::OutputCache::default();
    transform_cached(&store, &req, &ctx, &cache).expect("materializing pass");
    (dir, store, req)
}

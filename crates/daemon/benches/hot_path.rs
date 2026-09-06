//! Criterion benches for the daemon transform hot path.
//!
//! Claim under measurement: single-threaded, warm-process, warm-tokenizer
//! service time of one in-process stage call (or one full transform pass) at a
//! fixed corpus point. Arrival model: none (local operation microbenchmark).
//! Corpus is deterministic (seed in `support/corpus.rs`); every cell is
//! reported per-benchmark, never aggregated. Cross-change comparisons need
//! process-level replication: `docs/perf/daemon-hot-path.md` describes the
//! baseline workflow and its limits.
//!
//! Run: `cargo bench -p daemon --features bench-internals`

#[path = "support/corpus.rs"]
mod corpus;

use std::collections::HashSet;
use std::time::Duration;

use context_core::CoreState;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use daemon::bench_internals::{self, CacheTtlProvenance, MirroredClaimMemory, transform};
use daemon::transform::{ProducerContext, TransformRequest};
use daemon::wire::{IngressMessage, project_messages};
use memory_store::MemoryStore;
use std::hint::black_box;

use corpus::{CORPUS_SEED, ContentClass, Rng};

const MESSAGE_COUNTS: &[usize] = &[100, 1_400, 2_500];
/// Counts whose first HARD pass the store commits. The pass's `meta` field grows
/// by roughly 460 bytes per message and the store bounds one durable text field
/// at 512 KiB, so a 1_400-message first pass is rejected with `InputLimit`.
const E2E_MESSAGE_COUNTS: &[usize] = &[100, 1_000];
/// Steady-state groups use the largest count in `E2E_MESSAGE_COUNTS`.
const E2E_STEADY_COUNT: usize = 1_000;
const PAYLOAD_SIZES: &[usize] = &[256, 2_048, 4_096];
const TOKENIZER_CLASSES: &[ContentClass] = &[
    ContentClass::Prose,
    ContentClass::Code,
    ContentClass::JsonTool,
    ContentClass::Log,
];

fn warm_tokenizer() {
    // The vocab decode behind the OnceLock is a one-time ~hundreds-of-ms cost;
    // it belongs to process startup, not to any per-call estimand here.
    black_box(tokenizer::estimate_tokens("warm the vendored vocab"));
}

fn bench_tokenizer(c: &mut Criterion) {
    warm_tokenizer();
    let mut group = c.benchmark_group("tokenizer/estimate_tokens");
    for &class in TOKENIZER_CLASSES {
        for &bytes in PAYLOAD_SIZES {
            let mut rng = Rng::new(CORPUS_SEED ^ bytes as u64);
            let sample = corpus::text(class, bytes, &mut rng);
            group.throughput(Throughput::Bytes(sample.len() as u64));
            group.bench_with_input(
                BenchmarkId::new(class.label(), format!("{bytes}B")),
                &sample,
                |b, sample| b.iter(|| tokenizer::estimate_tokens(black_box(sample))),
            );
        }
    }
    group.finish();
}

fn bench_projection(c: &mut Criterion) {
    let mut group = c.benchmark_group("projection/full");
    for &count in MESSAGE_COUNTS {
        let messages = corpus::messages(ContentClass::Mixed, count, 2_048, CORPUS_SEED);
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{count}msgs_2KiB_mixed")),
            &messages,
            |b, messages| {
                b.iter_batched(
                    || (),
                    |()| project_messages(black_box(messages)).expect("projection"),
                    criterion::BatchSize::PerIteration,
                )
            },
        );
    }
    group.finish();
}

fn bench_tail_hygiene(c: &mut Criterion) {
    warm_tokenizer();
    let mut group = c.benchmark_group("tail_hygiene/measure");
    group.sample_size(20);
    let core = CoreState::empty();
    let protected: HashSet<String> = HashSet::new();
    for &count in MESSAGE_COUNTS {
        let messages = corpus::messages(ContentClass::Mixed, count, 2_048, CORPUS_SEED);
        let projection = project_messages(&messages).expect("projection");
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{count}msgs_2KiB_mixed")),
            &projection,
            |b, projection| {
                b.iter(|| {
                    bench_internals::measure_tail_hygiene(
                        black_box(projection),
                        &core,
                        None,
                        &[],
                        20,
                        &protected,
                    )
                })
            },
        );
    }
    group.finish();
}

fn bench_m0_trim_claims(c: &mut Criterion) {
    warm_tokenizer();
    let mut group = c.benchmark_group("m0/trim_claims_to_budget");
    for &count in &[8usize, 64, 256] {
        let mut rng = Rng::new(CORPUS_SEED ^ 0xC1A1);
        let claims: Vec<MirroredClaimMemory> = (0..count)
            .map(|index| MirroredClaimMemory {
                public_claim_id: format!("mcm_{index:032}"),
                revision_locator: format!("mcm_{index:032}/r1/deadbeef"),
                project_id: 1,
                category: "ARCHITECTURE_DECISIONS".to_string(),
                content: corpus::text(ContentClass::Prose, 300, &mut rng),
                importance: (rng.next() % 100) as i64,
                provenance_label: None,
            })
            .collect();
        // A category outside POSITIVE_MEMORY_CATEGORIES is filtered before any
        // tokenization, so the cell would time an empty eligible set.
        let retained = bench_internals::trim_claims_to_budget(&claims, 8_000.0);
        assert!(
            retained > 0,
            "m0 fixture filtered out entirely at {count} claims: check the claim category"
        );
        assert!(
            count < 256 || retained < count,
            "the {count}-claim cell must exceed the budget and trim, retained {retained}"
        );
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{count}claims_8k_budget")),
            &claims,
            |b, claims| {
                b.iter(|| bench_internals::trim_claims_to_budget(black_box(claims), 8_000.0))
            },
        );
    }
    group.finish();
}

fn request(session: &str, messages: &[IngressMessage], caveman: bool) -> TransformRequest {
    // The serde path is the production wire: absent fields take the same
    // defaults every harness sender gets.
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

fn producer_ctx(dir: &str) -> ProducerContext<'_> {
    ProducerContext {
        claim_lane: None,
        project_path: "git:bench",
        note_project_path: "git:bench",
        project_directory: dir,
        history_budget_tokens: 60_000.0,
        memory_budget_tokens: 8_000.0,
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

fn fresh_store() -> (tempfile::TempDir, MemoryStore) {
    let dir = tempfile::tempdir().expect("bench store dir");
    let descriptor = storage::StorageDescriptor {
        module_id: "eidnara-bench".to_string(),
        storage_namespace: "memory".to_string(),
        isolation: storage::Isolation::Module,
        backend: storage::StorageBackend::Sqlite {
            path: dir.path().join("store.db").to_string_lossy().into_owned(),
        },
    };
    let store = MemoryStore::open(&descriptor).expect("bench store");
    (dir, store)
}

fn bench_e2e_first_hard(c: &mut Criterion) {
    warm_tokenizer();
    let mut group = c.benchmark_group("e2e/first_hard");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(20));
    for &count in E2E_MESSAGE_COUNTS {
        let messages = corpus::messages(ContentClass::Mixed, count, 2_048, CORPUS_SEED);
        group.bench_function(
            BenchmarkId::from_parameter(format!("{count}msgs_2KiB_mixed")),
            |b| {
                b.iter_batched(
                    || {
                        // Clear the token cache so first_hard measures an all-miss pass.
                        bench_internals::clear_token_cache();
                        let (dir, store) = fresh_store();
                        let req = request("bench-hard", &messages, false);
                        (dir, store, req)
                    },
                    |(dir, store, req)| {
                        let ctx = producer_ctx(dir.path().to_str().expect("utf8 dir"));
                        let out = transform(&store, &req, &ctx).expect("hard pass");
                        // `out` carries message buffers; return `out` so Criterion drops it after timing.
                        // Reading a Copy field off it would drop the buffers inside the timed section.
                        (dir, store, req, out)
                    },
                    criterion::BatchSize::PerIteration,
                )
            },
        );
    }
    group.finish();
}

/// One materializing pass, then the measured loop repeats the same request:
/// the repeated pass is a stable (non-committing) pass, matching the
/// production defer cadence.
fn steady_state(
    messages: &[IngressMessage],
    caveman: bool,
) -> (tempfile::TempDir, MemoryStore, TransformRequest) {
    let (dir, store) = fresh_store();
    let req = request("bench-steady", messages, caveman);
    let ctx = producer_ctx(dir.path().to_str().expect("utf8 dir"));
    transform(&store, &req, &ctx).expect("materializing pass");
    (dir, store, req)
}

fn bench_e2e_steady(c: &mut Criterion) {
    warm_tokenizer();
    let mut group = c.benchmark_group("e2e/steady");
    group.sample_size(20);
    for &count in E2E_MESSAGE_COUNTS {
        let messages = corpus::messages(ContentClass::Mixed, count, 2_048, CORPUS_SEED);
        let (dir, store, req) = steady_state(&messages, false);
        let ctx = producer_ctx(dir.path().to_str().expect("utf8 dir"));
        group.bench_function(
            BenchmarkId::from_parameter(format!("{count}msgs_2KiB_mixed")),
            |b| {
                b.iter_batched(
                    || (),
                    |()| transform(&store, &req, &ctx).expect("steady pass"),
                    criterion::BatchSize::PerIteration,
                )
            },
        );
    }
    // Payload-class sensitivity at the production-shaped 1,400-message point.
    for &class in &[
        ContentClass::Prose,
        ContentClass::Code,
        ContentClass::JsonTool,
    ] {
        let messages = corpus::messages(class, E2E_STEADY_COUNT, 2_048, CORPUS_SEED);
        let (dir, store, req) = steady_state(&messages, false);
        let ctx = producer_ctx(dir.path().to_str().expect("utf8 dir"));
        group.bench_function(
            BenchmarkId::from_parameter(format!("{E2E_STEADY_COUNT}msgs_2KiB_{}", class.label())),
            |b| {
                b.iter_batched(
                    || (),
                    |()| transform(&store, &req, &ctx).expect("steady pass"),
                    criterion::BatchSize::PerIteration,
                )
            },
        );
    }
    group.finish();
}

fn bench_e2e_steady_output_cache(c: &mut Criterion) {
    warm_tokenizer();
    let mut group = c.benchmark_group("e2e/steady_output_cache");
    group.sample_size(20);
    let messages = corpus::messages(ContentClass::Mixed, E2E_STEADY_COUNT, 2_048, CORPUS_SEED);
    let (dir, store, req) = steady_state(&messages, false);
    let ctx = producer_ctx(dir.path().to_str().expect("utf8 dir"));
    let cache = bench_internals::OutputCache::default();
    // Prime the cache so the measured loop is the warm-cache steady pass.
    bench_internals::transform_cached(&store, &req, &ctx, &cache).expect("prime pass");
    group.bench_function(format!("{E2E_STEADY_COUNT}msgs_2KiB_mixed"), |b| {
        b.iter_batched(
            || (),
            |()| {
                bench_internals::transform_cached(&store, &req, &ctx, &cache)
                    .expect("cached steady pass")
            },
            criterion::BatchSize::PerIteration,
        )
    });
    group.finish();
}

fn bench_e2e_steady_caveman(c: &mut Criterion) {
    warm_tokenizer();
    let mut group = c.benchmark_group("e2e/steady_caveman");
    group.sample_size(20);
    let messages = corpus::messages(ContentClass::Mixed, E2E_STEADY_COUNT, 2_048, CORPUS_SEED);
    let (dir, store, req) = steady_state(&messages, true);
    let ctx = producer_ctx(dir.path().to_str().expect("utf8 dir"));
    group.bench_function(format!("{E2E_STEADY_COUNT}msgs_2KiB_mixed"), |b| {
        b.iter_batched(
            || (),
            |()| transform(&store, &req, &ctx).expect("caveman steady pass"),
            criterion::BatchSize::PerIteration,
        )
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_tokenizer,
    bench_projection,
    bench_tail_hygiene,
    bench_m0_trim_claims,
    bench_e2e_first_hard,
    bench_e2e_steady,
    bench_e2e_steady_output_cache,
    bench_e2e_steady_caveman,
);
criterion_main!(benches);

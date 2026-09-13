//! This example collects release-binary evidence for the served-output canonicalizer.

#[allow(dead_code)]
#[path = "../tests/support/alloc_recorder.rs"]
mod alloc_recorder;
#[allow(dead_code)]
#[path = "../benches/support/corpus.rs"]
mod corpus;
#[allow(dead_code)]
#[path = "../tests/support/served_output_fixtures.rs"]
mod served_output_fixtures;
#[allow(dead_code)]
#[path = "../benches/support/transform_fixture.rs"]
mod transform_fixture;

use std::hint::black_box;
use std::time::Instant;

use alloc_recorder::{BufferProvenance, Event, Ledger, record_window};
use corpus::{CORPUS_SEED, ContentClass};
use daemon::bench_internals::{self, transform_cached};
use daemon::transform::{ProducerContext, TransformRequest};
use daemon::wire::IngressMessage;
use memory_store::{MemoryStore, WireMessage};
use serde_json::{Value, json};
use served_output_fixtures::{
    Population, declaration_order_equals_canonical, populations, reference_bytes,
};

#[global_allocator]
static GLOBAL: alloc_recorder::RecordingAlloc = alloc_recorder::RecordingAlloc;

const TRANSFORM_MESSAGE_COUNTS: &[usize] = &[100, 1_000];
/// Transform passes cost milliseconds, so a fixed short warmup replaces the
/// tenth-of-samples rule used by the microsecond cells.
const TRANSFORM_WARMUP: usize = 3;

struct Options {
    out: Option<String>,
    label: String,
    commit: String,
    micro_samples: usize,
    transform_samples: usize,
}

fn parse_options() -> Options {
    let mut options = Options {
        out: None,
        label: "unlabeled".to_string(),
        commit: "unknown".to_string(),
        micro_samples: 200,
        transform_samples: 30,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .unwrap_or_else(|| panic!("{name} requires a value"))
        };
        match arg.as_str() {
            "--out" => options.out = Some(value("--out")),
            "--label" => options.label = value("--label"),
            "--commit" => options.commit = value("--commit"),
            "--micro-samples" => {
                options.micro_samples = value("--micro-samples").parse().expect("usize")
            }
            "--transform-samples" => {
                options.transform_samples = value("--transform-samples").parse().expect("usize")
            }
            other => panic!("unknown argument {other}"),
        }
    }
    options
}

fn process_cpu_ns() -> u128 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid, writable timespec and the clock id is a constant.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts) };
    assert_eq!(rc, 0, "clock_gettime(CLOCK_PROCESS_CPUTIME_ID)");
    ts.tv_sec as u128 * 1_000_000_000 + ts.tv_nsec as u128
}

/// x86 exposes `model name`; arm64 exposes only implementer/part codes.
fn cpu_model() -> String {
    let Ok(info) = std::fs::read_to_string("/proc/cpuinfo") else {
        return "unknown".to_string();
    };
    let field = |name: &str| {
        info.lines()
            .find(|line| line.starts_with(name))
            .and_then(|line| line.split_once(':'))
            .map(|(_, value)| value.trim().to_string())
    };
    field("model name")
        .or_else(|| {
            Some(format!(
                "implementer {} part {} variant {} revision {}",
                field("CPU implementer")?,
                field("CPU part")?,
                field("CPU variant")?,
                field("CPU revision")?
            ))
        })
        .unwrap_or_else(|| "unknown".to_string())
}

/// Compile-time feature resolution of this binary's own crate; the script
/// records the workspace feature graph separately.
fn resolved_features() -> Vec<&'static str> {
    let mut features = Vec::new();
    if cfg!(feature = "bench-internals") {
        features.push("bench-internals");
    }
    if cfg!(feature = "test-support") {
        features.push("test-support");
    }
    if cfg!(feature = "direct-host-fixture") {
        features.push("direct-host-fixture");
    }
    if cfg!(debug_assertions) {
        features.push("debug-assertions");
    }
    features
}

fn percentile(sorted: &[u128], fraction: f64) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = ((sorted.len() - 1) as f64 * fraction).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

/// Per-sample `setup` output and each result's drop stay outside both clocks.
fn time_cell<I, T>(
    warmup: usize,
    samples: usize,
    mut setup: impl FnMut() -> I,
    mut call: impl FnMut(I) -> T,
) -> Value {
    for _ in 0..warmup {
        black_box(call(setup()));
    }
    let mut elapsed = Vec::with_capacity(samples);
    let mut cpu_total = 0u128;
    let mut wall_total = 0u128;
    for _ in 0..samples {
        let input = setup();
        let cpu_start = process_cpu_ns();
        let start = Instant::now();
        let value = call(input);
        let wall = start.elapsed().as_nanos();
        cpu_total += process_cpu_ns() - cpu_start;
        wall_total += wall;
        elapsed.push(wall);
        drop(black_box(value));
    }
    let mut sorted = elapsed.clone();
    sorted.sort_unstable();
    let mean = if sorted.is_empty() {
        0
    } else {
        sorted.iter().sum::<u128>() / sorted.len() as u128
    };
    json!({
        "samples": samples,
        "warmup": warmup,
        "p50_ns": percentile(&sorted, 0.50),
        "p95_ns": percentile(&sorted, 0.95),
        "min_ns": sorted.first().copied().unwrap_or(0),
        "max_ns": sorted.last().copied().unwrap_or(0),
        "mean_ns": mean,
        "cpu_ns_total": cpu_total,
        "wall_ns_total": wall_total,
        "elapsed_ns": elapsed,
    })
}

fn ledger_json(ledger: &Ledger) -> Value {
    json!({
        "allocation_events": ledger.allocation_events,
        "realloc_events": ledger.realloc_events,
        "dealloc_events": ledger.dealloc_events,
        "requested_bytes": ledger.requested_bytes,
        "peak_live_bytes": ledger.peak_live_bytes,
        "live_bytes_at_close": ledger.live_bytes_at_close,
        "overflow": ledger.overflow,
    })
}

fn allocation_cell(population: Population) -> Value {
    let message = population.build();
    let reference = reference_bytes(&message);
    let output_bytes = reference.len();
    let class = if declaration_order_equals_canonical(&message, &reference) {
        "canonical-miss"
    } else {
        "unordered-miss"
    };
    let (bytes, canonicalizer) =
        record_window(|| daemon::served_json::canonical_served_bytes_for_test(&message));
    assert_eq!(bytes, reference, "{}", population.label());
    let ptr = bytes.as_ptr() as usize;
    let return_capacity = bytes.capacity();
    let chain = canonicalizer.growth_chain(ptr);
    let provenance = canonicalizer.buffer_provenance(ptr, bytes.len(), return_capacity);
    let output_sized_allocations = canonicalizer
        .allocations_of_size(output_bytes)
        .iter()
        .filter(|event| matches!(event, Event::Alloc { .. }))
        .count();
    let storage_outside_chain = canonicalizer
        .allocations_at_least_outside(output_bytes, ptr)
        .len();
    let logical_reorder_output_bytes = match provenance {
        BufferProvenance::FreshExactSizeAllocation => output_bytes,
        BufferProvenance::GrowthChain | BufferProvenance::Unattributed => 0,
    };
    drop(bytes);
    let (served, full_constructor) =
        record_window(|| daemon::transform::served_message_for_test(message));
    assert_eq!(served.canonical_bytes_for_test(), reference.as_slice());
    drop(served);
    json!({
        "population": population.label(),
        "class": class,
        "output_bytes": output_bytes,
        "canonicalizer": {
            "ledger": ledger_json(&canonicalizer),
            "return_len": output_bytes,
            "return_capacity": return_capacity,
            "return_capacity_over_len": return_capacity as f64 / output_bytes as f64,
            "return_provenance": format!("{provenance:?}"),
            "return_growth_chain_events": chain.len(),
            "output_sized_allocations": output_sized_allocations,
            "output_sized_storage_outside_chain": storage_outside_chain,
            "logical_reorder_output_bytes": logical_reorder_output_bytes,
        },
        "full_constructor": {
            "ledger": ledger_json(&full_constructor),
            "peak_over_output": full_constructor.peak_live_bytes as f64 / output_bytes as f64,
        },
    })
}

fn micro_timing_cells(samples: usize) -> Vec<Value> {
    let mut cells = Vec::new();
    for population in populations() {
        let message = population.build();
        cells.push(json!({
            "boundary": "canonicalizer",
            "population": population.label(),
            "timing": time_cell(
                samples / 10,
                samples,
                || &message,
                |message| daemon::served_json::canonical_served_bytes_for_test(black_box(message)),
            ),
        }));
        cells.push(json!({
            "boundary": "full_constructor",
            "population": population.label(),
            "timing": time_cell(
                samples / 10,
                samples,
                || message.clone(),
                |message| daemon::transform::served_message_for_test(black_box(message)),
            ),
        }));
    }
    cells
}

fn served_order_counts(messages: &[daemon::transform::ServedMessage]) -> (usize, usize) {
    let mut canonical = 0;
    let mut noncanonical = 0;
    for served in messages {
        let message: &WireMessage = served;
        if declaration_order_equals_canonical(message, served.canonical_bytes_for_test()) {
            canonical += 1;
        } else {
            noncanonical += 1;
        }
    }
    (canonical, noncanonical)
}

fn transform_pass(
    store: &MemoryStore,
    req: &TransformRequest,
    ctx: &ProducerContext<'_>,
    cache: &bench_internals::OutputCache,
) -> daemon::transform::TransformWithProjection {
    transform_cached(store, req, ctx, cache).expect("transform pass")
}

fn frequency_json(label: &str, result: &daemon::transform::TransformWithProjection) -> Value {
    let served = result.response.messages();
    let (canonical, noncanonical) = served_order_counts(served);
    let timings = result.response.timings.as_ref();
    json!({
        "workload": label,
        "served_messages": served.len(),
        "canonical_order": canonical,
        "noncanonical_order": noncanonical,
        "cache_hits": timings.map(|t| t.cache_hits),
        "cache_misses": timings.map(|t| t.cache_misses),
        "cache_dirty_skips": timings.map(|t| t.cache_dirty_skips),
    })
}

fn transform_cells(samples: usize) -> (Vec<Value>, Vec<Value>) {
    let mut timing = Vec::new();
    let mut frequencies = Vec::new();
    for &count in TRANSFORM_MESSAGE_COUNTS {
        let messages: Vec<IngressMessage> =
            corpus::messages(ContentClass::Mixed, count, 2_048, CORPUS_SEED);
        let (dir, store, req) = transform_fixture::steady_state(&messages, false);
        let ctx = transform_fixture::producer_ctx(dir.path().to_str().expect("utf8 dir"));
        let label = format!("{count}msgs_2KiB_mixed");

        let cold = transform_pass(&store, &req, &ctx, &bench_internals::OutputCache::default());
        frequencies.push(frequency_json(&format!("transform_cold/{label}"), &cold));
        drop(cold);
        timing.push(json!({
            "boundary": "transform_cold",
            "population": label,
            "timing": time_cell(
                TRANSFORM_WARMUP.min(samples),
                samples,
                bench_internals::OutputCache::default,
                |cache| transform_pass(&store, &req, &ctx, &cache),
            ),
        }));

        let cache = bench_internals::OutputCache::default();
        drop(transform_pass(&store, &req, &ctx, &cache));
        let warm = transform_pass(&store, &req, &ctx, &cache);
        frequencies.push(frequency_json(&format!("transform_warm/{label}"), &warm));
        drop(warm);
        timing.push(json!({
            "boundary": "transform_warm",
            "population": label,
            "timing": time_cell(
                TRANSFORM_WARMUP.min(samples),
                samples,
                || (),
                |()| transform_pass(&store, &req, &ctx, &cache),
            ),
        }));
    }
    (timing, frequencies)
}

fn main() {
    let options = parse_options();
    let started = Instant::now();
    let allocation: Vec<Value> = populations().map(allocation_cell).collect();
    let mut timing = micro_timing_cells(options.micro_samples);
    let (transform_timing, frequencies) = transform_cells(options.transform_samples);
    timing.extend(transform_timing);
    let document = json!({
        "kind": "canonical-output-evidence/v1",
        "label": options.label,
        "provenance": {
            "commit": options.commit,
            "package_version": env!("CARGO_PKG_VERSION"),
            "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "requested_features": ["bench-internals", "test-support"],
            "resolved_features": resolved_features(),
            "target_arch": std::env::consts::ARCH,
            "os": std::env::consts::OS,
            "cpu_model": cpu_model(),
            "available_parallelism": std::thread::available_parallelism().map(|n| n.get()).ok(),
            "allocator": "System behind a thread-owned recording wrapper; recording is disabled during timing cells",
            "pid": std::process::id(),
            "unix_time_s": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .ok(),
            "driver_wall_ns": started.elapsed().as_nanos(),
        },
        "timing_boundaries": {
            "canonicalizer": "canonical_served_bytes_for_test: serialization, span sorting, and reorder copy; excludes hashing, receipts, and Arc conversion",
            "full_constructor": "served_message_for_test: canonicalizer plus block receipts, SHA-256, identity formatting, and Arc conversion; the per-sample message clone runs before the clocks start",
            "transform_cold": "transform_cached with a fresh output cache per call; every served message is constructed",
            "transform_warm": "transform_cached with a primed output cache; positive hits reuse served messages",
        },
        "allocation": allocation,
        "timing": timing,
        "frequencies": frequencies,
        "completed_output_replay": {
            "status": "not-constructed",
            "reason": "host-level PreparedOutput page replay is a separate reuse path outside this in-process driver",
        },
        "host_latency": {
            "status": "unmeasured",
            "reason": "no real-host transform driver is retained; in-process transform timing is not user-visible latency",
        },
    });
    let text = serde_json::to_string_pretty(&document).expect("serialize evidence");
    match options.out {
        Some(path) => std::fs::write(&path, text).unwrap_or_else(|err| panic!("{path}: {err}")),
        None => println!("{text}"),
    }
}

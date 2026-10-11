//! Reference-host load measurements for the certified production lane (#825 D25 and D26).
//!
//! The test needs the certified production bundle at `EIDNARA_LOCAL_EMBEDDINGS_PRODUCTION_BUNDLE` and the locked
//! ONNX Runtime library at `EIDNARA_LOCAL_EMBEDDINGS_TEST_ORT_LIBRARY`, so it is `#[ignore]`d and a run without them
//! fails. Activation certifies both against the digests `release/production-inputs.lock.json` pins, so a path to any
//! other artifact fails before it is measured. Run it in release on the D21 host:
//! `cargo test --release -p host-runtime --test local_embeddings_reference_load -- --ignored --nocapture`.
//! `EIDNARA_LOCAL_EMBEDDINGS_SAMPLES` names a file for the raw microsecond samples, kept outside the repository.
//! The page cache is warm for every timed phase: activation reads the whole model and certification runs it.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use host_runtime::SecondaryComponent;
use host_runtime::local_embeddings::{
    DenseUnavailable, EmbeddingInputLimits, LaneInfo, LocalEmbeddingsComponent,
    LocalEmbeddingsConfig, LocalEmbeddingsLimits, LocalEmbeddingsStatus, PollOutcome,
    SubmitOutcome,
};

const PRODUCTION_INPUTS_LOCK_JSON: &str =
    include_str!("../../../release/production-inputs.lock.json");

/// The parent's declared open-loop query rate for the embedding gates.
const QUERY_RATE_PER_SECOND: f64 = 2.0;
/// Queries offered with the lane otherwise idle, and under continuous backfill.
const IDLE_QUERIES: usize = 600;
const BACKFILL_QUERIES: usize = 1000;
/// Backfill texts timed at steady state after warm-up.
const BACKFILL_TEXTS: usize = 200;
const BACKFILL_TOKENS: u32 = 76;
/// Backfill jobs kept in flight under load, so a background waiter is queued whenever a query arrives.
const BACKFILL_IN_FLIGHT: usize = 2;
const MAX_QUERY_TOKENS: u32 = 32;
const MIN_QUERY_TOKENS: u32 = 4;
const QUERY_DEADLINE: Duration = Duration::from_secs(5);
const SEED: u64 = 0x9e37_79b9_7f4a_7c15;
/// #825 acceptance budgets for the reference host.
const MIN_BACKFILL_TEXTS_PER_SECOND: f64 = 8.0;
const MAX_QUERY_P99_US: u64 = 60_000;

const WORDS: &[&str] = &[
    "context",
    "window",
    "summary",
    "memory",
    "session",
    "project",
    "search",
    "vector",
    "lexical",
    "kernel",
    "decision",
    "claim",
    "evidence",
    "review",
    "index",
    "token",
    "budget",
    "backfill",
    "query",
    "latency",
    "throughput",
    "release",
    "payload",
    "artifact",
    "runtime",
    "shutdown",
    "cancel",
    "deadline",
    "scheduler",
    "fairness",
    "grant",
    "waiter",
    "native",
    "inference",
    "model",
    "bundle",
    "manifest",
    "tokenizer",
    "certify",
    "corpus",
    "embedding",
    "dense",
    "fusion",
    "route",
    "admission",
    "eligible",
    "canonical",
    "projection",
    "occurrence",
    "rowid",
    "commit",
    "snapshot",
];

/// A fixed linear congruential stream keeps texts and arrivals reproducible.
struct Stream(u64);

impl Stream {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }

    fn uniform(&mut self) -> f64 {
        (self.next() as f64 + 0.5) / (1u64 << 53) as f64
    }

    fn word(&mut self) -> &'static str {
        WORDS[(self.next() % WORDS.len() as u64) as usize]
    }
}

fn locked_sha256(key: &str) -> String {
    let lock: serde_json::Value =
        serde_json::from_str(PRODUCTION_INPUTS_LOCK_JSON).expect("production inputs lock parses");
    lock["inputs"][key]["sha256"]
        .as_str()
        .unwrap_or_else(|| panic!("the lock pins inputs.{key}.sha256"))
        .to_owned()
}

fn required_path(name: &str) -> PathBuf {
    std::env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("{name} must be set"))
}

fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

/// A text of exactly `tokens` tokens under the lane's own counter, built from the fixed vocabulary.
fn text_of(
    component: &LocalEmbeddingsComponent,
    limits: EmbeddingInputLimits,
    tokens: u32,
    stream: &mut Stream,
) -> String {
    let count = |text: &str| {
        component
            .preflight_embedding(limits, text)
            .expect("vocabulary text is admissible")
            .tokens()
            .get()
    };
    loop {
        let mut text = stream.word().to_string();
        let mut counted = count(&text);
        while counted < tokens {
            text.push(' ');
            text.push_str(stream.word());
            counted = count(&text);
        }
        if counted == tokens {
            return text;
        }
    }
}

/// One query's outcome; times are microseconds since the run's origin.
struct Sample {
    phase: &'static str,
    scheduled_us: u64,
    sent_us: u64,
    done_us: u64,
    tokens: u32,
    /// `ok`; `busy` for a full queue or a wait that reached the deadline, which the lane reports alike; or `refused`.
    outcome: &'static str,
}

impl Sample {
    fn latency_us(&self) -> u64 {
        self.done_us - self.scheduled_us
    }

    fn service_us(&self) -> u64 {
        self.done_us - self.sent_us
    }
}

/// Order statistics of one phase's completed queries, latency measured from each scheduled arrival.
#[derive(Debug)]
#[expect(dead_code, reason = "the run report reads every field through Debug")]
struct PhaseSummary {
    offered: usize,
    completed: usize,
    busy: usize,
    p50_us: u64,
    p99_us: u64,
    /// The second-largest latency: with 600 or more samples, a distribution-free one-sided 95% upper bound on p99.
    p99_upper_us: u64,
    max_us: u64,
    /// The p99 of each half of the phase in arrival order, to show the lane did not drift.
    halves_p99_us: (u64, u64),
    /// The largest gap between a scheduled arrival and its send, which bounds the generator's own lag.
    max_send_lag_us: u64,
}

/// The `q` quantile by nearest rank; `None` for an empty slice.
fn percentile(sorted: &[u64], q: f64) -> Option<u64> {
    let rank = ((sorted.len() as f64 * q).ceil() as usize).checked_sub(1)?;
    sorted.get(rank).copied()
}

fn summarize(samples: &[Sample], phase: &str) -> Option<PhaseSummary> {
    let phase_samples: Vec<&Sample> = samples.iter().filter(|s| s.phase == phase).collect();
    let completed: Vec<&Sample> = phase_samples
        .iter()
        .copied()
        .filter(|s| s.outcome == "ok")
        .collect();
    let sorted = |of: &[&Sample]| {
        let mut latency: Vec<u64> = of.iter().map(|s| s.latency_us()).collect();
        latency.sort_unstable();
        latency
    };
    let latency = sorted(&completed);
    let half = completed.len() / 2;
    Some(PhaseSummary {
        offered: phase_samples.len(),
        completed: completed.len(),
        busy: phase_samples.iter().filter(|s| s.outcome == "busy").count(),
        p50_us: percentile(&latency, 0.5)?,
        p99_us: percentile(&latency, 0.99)?,
        p99_upper_us: latency[latency.len().saturating_sub(2)],
        max_us: *latency.last()?,
        halves_p99_us: (
            percentile(&sorted(&completed[..half]), 0.99)?,
            percentile(&sorted(&completed[half..]), 0.99)?,
        ),
        max_send_lag_us: phase_samples
            .iter()
            .map(|s| s.sent_us - s.scheduled_us)
            .max()?,
    })
}

/// The estimate upper-bounds a backfill query's queue wait when the fastest successful idle service for the same token
/// count is at most that query's own service time. For token counts with zero successful idle samples, the full latency
/// serves as the queue-wait bound.
fn max_queue_wait_us(samples: &[Sample]) -> Option<u64> {
    let mut fastest_service: BTreeMap<u32, u64> = BTreeMap::new();
    for sample in samples
        .iter()
        .filter(|s| s.phase == "idle" && s.outcome == "ok")
    {
        fastest_service
            .entry(sample.tokens)
            .and_modify(|fastest| *fastest = (*fastest).min(sample.service_us()))
            .or_insert(sample.service_us());
    }
    samples
        .iter()
        .filter(|s| s.phase == "backfill" && s.outcome == "ok")
        .map(|s| {
            let service = fastest_service.get(&s.tokens).copied().unwrap_or(0);
            s.latency_us().saturating_sub(service)
        })
        .max()
}

#[test]
fn the_queue_wait_never_understates_a_loaded_query_that_runs_faster_than_the_idle_median() {
    let completed = |phase, done_us| Sample {
        phase,
        scheduled_us: 0,
        sent_us: 0,
        done_us,
        tokens: 16,
        outcome: "ok",
    };
    let fastest_idle_service_us = 20_000;
    let mut samples: Vec<Sample> = [fastest_idle_service_us, 30_000, 40_000]
        .into_iter()
        .map(|service| completed("idle", service))
        .collect();
    let loaded_wait_us = 105_000;
    samples.push(completed(
        "backfill",
        loaded_wait_us + fastest_idle_service_us,
    ));
    let wait = max_queue_wait_us(&samples).expect("one loaded query completed");
    assert!(
        wait >= loaded_wait_us,
        "the proxy {wait}us understates the {loaded_wait_us}us queue wait"
    );
}

/// Embeds one 76-token text through the job table as backfill does, from submission to its vector.
async fn backfill_one(
    component: &LocalEmbeddingsComponent,
    limits: EmbeddingInputLimits,
    lane: &LaneInfo,
    item: &str,
    text: &str,
) -> Duration {
    let started = Instant::now();
    let admitted = component.preflight_embedding(limits, text).expect("admit");
    let job_id = match component.submit_admitted(&admitted, item).expect("submit") {
        SubmitOutcome::Queued { job_id } => job_id,
        other => panic!("backfill submission refused: {other:?}"),
    };
    loop {
        match component.poll_admitted(lane, &job_id, item, text) {
            PollOutcome::Page(page) => {
                assert_eq!(page.vectors.len(), 1);
                return started.elapsed();
            }
            PollOutcome::Pending { .. } => tokio::time::sleep(Duration::from_micros(200)).await,
            PollOutcome::Failed { code, message } => {
                panic!("backfill job failed: {code}: {message}")
            }
            PollOutcome::Restarted | PollOutcome::KeyMismatch | PollOutcome::BadCursor => {
                panic!("backfill job left the table without a vector")
            }
        }
    }
}

/// Offers `count` queries of 4 to 32 tokens on an open Poisson schedule: every arrival is drawn before the first is
/// sent, and each query runs in its own task from its scheduled time, so no arrival waits for an earlier answer.
async fn run_queries(
    component: &LocalEmbeddingsComponent,
    limits: EmbeddingInputLimits,
    dims: usize,
    origin: Instant,
    count: usize,
    phase: &'static str,
    stream: &mut Stream,
) -> Vec<Sample> {
    let mut at = 0.0f64;
    let schedule: Vec<(f64, String)> = (0..count)
        .map(|_| {
            at += -stream.uniform().ln() / QUERY_RATE_PER_SECOND;
            let tokens = MIN_QUERY_TOKENS
                + (stream.next() % u64::from(MAX_QUERY_TOKENS - MIN_QUERY_TOKENS + 1)) as u32;
            (at, text_of(component, limits, tokens, stream))
        })
        .collect();
    let start = Instant::now();
    let tasks: Vec<_> = schedule
        .into_iter()
        .map(|(offset, text)| {
            let component = component.clone();
            let due = start + Duration::from_secs_f64(offset);
            tokio::spawn(async move {
                tokio::time::sleep_until(due.into()).await;
                let sent = Instant::now();
                let admitted = component.preflight_embedding(limits, &text).unwrap();
                let tokens = admitted.tokens().get();
                let outcome = match component
                    .embed_query(&admitted, tokio::time::Instant::now() + QUERY_DEADLINE)
                    .await
                {
                    Ok(vector) => {
                        assert_eq!(vector.len(), dims);
                        "ok"
                    }
                    Err(DenseUnavailable::LaneBusy { .. }) => "busy",
                    Err(_) => "refused",
                };
                Sample {
                    phase,
                    scheduled_us: micros(due.duration_since(origin)),
                    sent_us: micros(sent.duration_since(origin)),
                    done_us: micros(origin.elapsed()),
                    tokens,
                    outcome,
                }
            })
        })
        .collect();
    let mut samples = Vec::with_capacity(count);
    for task in tasks {
        samples.push(task.await.unwrap());
    }
    samples
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires EIDNARA_LOCAL_EMBEDDINGS_PRODUCTION_BUNDLE and EIDNARA_LOCAL_EMBEDDINGS_TEST_ORT_LIBRARY; run in release on the D21 host with --ignored"]
async fn production_lane_meets_the_reference_query_and_backfill_budgets() {
    let bundle_dir = required_path("EIDNARA_LOCAL_EMBEDDINGS_PRODUCTION_BUNDLE");
    let ort = required_path("EIDNARA_LOCAL_EMBEDDINGS_TEST_ORT_LIBRARY");
    // Activation certifies the supplied bundle and library against the shipped digests, so a path to any other
    // artifact fails here before it is measured as the production lane.
    let manifest_sha256 = locked_sha256("bundle_manifest");
    let ort_sha256 = locked_sha256("ort_runtime");
    let lane_limits = LocalEmbeddingsLimits::shipped_lane();
    let max_waiting_queries = lane_limits.max_waiting_queries;
    let component = LocalEmbeddingsComponent::new(Some(LocalEmbeddingsConfig {
        bundle_dir,
        bundle_manifest_sha256: Some(manifest_sha256.clone()),
        ort_library: ort,
        ort_library_sha256: ort_sha256.clone(),
        limits: lane_limits,
    }));
    component.initialize().await.expect("initialize");
    SecondaryComponent::activate(&component)
        .await
        .expect("activate");
    let lane = match component.status() {
        LocalEmbeddingsStatus::Ready(lane) => lane,
        other => panic!("production bundle did not certify: {other:?}"),
    };
    let limits = EmbeddingInputLimits::of_lane(&lane);
    let mut stream = Stream(SEED);
    let origin = Instant::now();

    // The first query after activation is reported on its own: certification already ran the model, so no timed
    // phase starts colder than this one.
    let first_text = text_of(&component, limits, MAX_QUERY_TOKENS, &mut stream);
    let admitted = component.preflight_embedding(limits, &first_text).unwrap();
    let first_started = Instant::now();
    component
        .embed_query(&admitted, tokio::time::Instant::now() + QUERY_DEADLINE)
        .await
        .expect("first query");
    let first_query_us = micros(first_started.elapsed());

    // Warm-up runs before any timed phase so first-touch costs stay out of every measurement.
    for n in 0..8 {
        let text = text_of(&component, limits, BACKFILL_TOKENS, &mut stream);
        backfill_one(&component, limits, &lane, &format!("warm-{n}"), &text).await;
        let query = text_of(&component, limits, MAX_QUERY_TOKENS, &mut stream);
        let admitted = component.preflight_embedding(limits, &query).unwrap();
        component
            .embed_query(&admitted, tokio::time::Instant::now() + QUERY_DEADLINE)
            .await
            .expect("warm-up query");
    }

    // Backfill throughput: texts embedded back to back at steady state, with no concurrent query.
    let texts: Vec<String> = (0..BACKFILL_TEXTS)
        .map(|_| text_of(&component, limits, BACKFILL_TOKENS, &mut stream))
        .collect();
    let mut services: Vec<u64> = Vec::with_capacity(BACKFILL_TEXTS);
    let throughput_started = Instant::now();
    for (n, text) in texts.iter().enumerate() {
        let service =
            backfill_one(&component, limits, &lane, &format!("throughput-{n}"), text).await;
        services.push(micros(service));
    }
    let throughput = BACKFILL_TEXTS as f64 / throughput_started.elapsed().as_secs_f64();
    let max_text_service_us = *services.iter().max().unwrap();

    let mut samples = run_queries(
        &component,
        limits,
        lane.dims,
        origin,
        IDLE_QUERIES,
        "idle",
        &mut stream,
    )
    .await;

    // A fresh open-loop sequence at the same rate while backfill keeps two texts in flight, so a background text
    // always waits behind the one running and each query arrival competes with it.
    let feeding = Arc::new(AtomicBool::new(true));
    let embedded = Arc::new(AtomicUsize::new(0));
    let backfill_texts: Arc<Vec<String>> = Arc::new(
        (0..64)
            .map(|_| text_of(&component, limits, BACKFILL_TOKENS, &mut stream))
            .collect(),
    );
    let feeders: Vec<_> = (0..BACKFILL_IN_FLIGHT)
        .map(|feeder| {
            let component = component.clone();
            let lane = lane.clone();
            let feeding = Arc::clone(&feeding);
            let embedded = Arc::clone(&embedded);
            let texts = Arc::clone(&backfill_texts);
            tokio::spawn(async move {
                let mut n = 0usize;
                while feeding.load(Ordering::Relaxed) {
                    let text = &texts[(n * BACKFILL_IN_FLIGHT + feeder) % texts.len()];
                    let item = format!("saturate-{feeder}-{n}");
                    backfill_one(&component, limits, &lane, &item, text).await;
                    embedded.fetch_add(1, Ordering::Relaxed);
                    n += 1;
                }
            })
        })
        .collect();
    samples.extend(
        run_queries(
            &component,
            limits,
            lane.dims,
            origin,
            BACKFILL_QUERIES,
            "backfill",
            &mut stream,
        )
        .await,
    );
    feeding.store(false, Ordering::Relaxed);
    for feeder in feeders {
        feeder.await.unwrap();
    }
    let backfill_embedded = embedded.load(Ordering::Relaxed);

    // Raw samples are written before any gate is judged, so a failing run still leaves its evidence.
    let threads = host_runtime::local_embeddings::inference::inference_threads();
    let parallelism = std::thread::available_parallelism().map_or(0, std::num::NonZeroUsize::get);
    if let Some(path) = std::env::var_os("EIDNARA_LOCAL_EMBEDDINGS_SAMPLES") {
        let mut rows = vec![format!(
            "{{\"rate_per_second\":{QUERY_RATE_PER_SECOND},\"seed\":\"{SEED:#x}\",\"model\":\"{}\",\"manifest_sha256\":\"{manifest_sha256}\",\"ort_sha256\":\"{ort_sha256}\",\"available_parallelism\":{parallelism},\"threads\":{threads},\"max_waiting_queries\":{max_waiting_queries},\"cache\":\"warm\",\"first_query_us\":{first_query_us},\"throughput\":{throughput},\"backfill_embedded\":{backfill_embedded},\"backfill_service_us\":{services:?}}}",
            lane.model
        )];
        rows.extend(samples.iter().map(|s| {
            format!(
                "{{\"phase\":\"{}\",\"scheduled_us\":{},\"sent_us\":{},\"done_us\":{},\"tokens\":{},\"outcome\":\"{}\"}}",
                s.phase, s.scheduled_us, s.sent_us, s.done_us, s.tokens, s.outcome
            )
        }));
        std::fs::write(path, rows.join("\n")).unwrap();
    }

    let idle = summarize(&samples, "idle");
    let loaded = summarize(&samples, "backfill");
    let max_queue_wait_us = max_queue_wait_us(&samples);
    eprintln!(
        "parallelism {parallelism}, threads {threads}, waiters {max_waiting_queries}; first query after activation {first_query_us} us; \
         backfill {throughput:.2} texts/s over {BACKFILL_TEXTS} texts of {BACKFILL_TOKENS} tokens, max single-text service {max_text_service_us} us; \
         idle at {QUERY_RATE_PER_SECOND}/s {idle:?}; under backfill {loaded:?} with {backfill_embedded} texts embedded and max latency past the fastest idle service {max_queue_wait_us:?} us; \
         zero busy bounds the busy rate at 3/n by the rule of three"
    );

    let idle = idle.expect("idle queries completed");
    let loaded = loaded.expect("queries under backfill completed");
    assert!(
        throughput >= MIN_BACKFILL_TEXTS_PER_SECOND,
        "76-token backfill throughput {throughput:.2} texts/s is under {MIN_BACKFILL_TEXTS_PER_SECOND}"
    );
    assert_eq!(idle.completed, IDLE_QUERIES, "every idle query completes");
    assert!(
        idle.p99_us <= MAX_QUERY_P99_US,
        "query embedding p99 {}us exceeds {MAX_QUERY_P99_US}us",
        idle.p99_us
    );
    assert_eq!(loaded.busy, 0, "no query is busy under backfill");
    assert_eq!(
        loaded.completed, BACKFILL_QUERIES,
        "every query under backfill completes"
    );
    assert!(
        backfill_embedded >= BACKFILL_QUERIES / 8,
        "backfill kept the slot busy: {backfill_embedded} texts"
    );
    let max_queue_wait_us = max_queue_wait_us.expect("queries under backfill completed");
    assert!(
        max_queue_wait_us <= 2 * max_text_service_us,
        "queue wait {max_queue_wait_us}us exceeds twice the longest text service {max_text_service_us}us"
    );
}

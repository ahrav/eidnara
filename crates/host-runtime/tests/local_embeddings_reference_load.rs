//! Reference-host load measurements for the certified production lane (#825 D25 and D26).
//!
//! The test needs the certified production bundle at `EIDNARA_LOCAL_EMBEDDINGS_PRODUCTION_BUNDLE` and the locked
//! ONNX Runtime library at `EIDNARA_LOCAL_EMBEDDINGS_TEST_ORT_LIBRARY`, so it is `#[ignore]`d and fails, rather than
//! passes, when run without them. Run it in release on the D21 host:
//! `cargo test --release -p host-runtime --test local_embeddings_reference_load -- --ignored --nocapture`.
//! `EIDNARA_LOCAL_EMBEDDINGS_SAMPLES` names a file for the raw microsecond samples, kept outside the repository.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use host_runtime::SecondaryComponent;
use host_runtime::local_embeddings::{
    DenseUnavailable, EmbeddingInputLimits, LocalEmbeddingsComponent, LocalEmbeddingsConfig,
    LocalEmbeddingsLimits, LocalEmbeddingsStatus, PollOutcome, SubmitOutcome,
};
use sha2::{Digest, Sha256};

/// The parent's declared open-loop query rate for the embedding gates.
const QUERY_RATE_PER_SECOND: f64 = 2.0;
/// Queries offered with the lane otherwise idle, and under continuous backfill.
const IDLE_QUERIES: usize = 600;
const BACKFILL_QUERIES: usize = 1000;
/// Backfill texts timed at steady state after warm-up.
const BACKFILL_TEXTS: usize = 200;
const BACKFILL_TOKENS: u32 = 76;
const MAX_QUERY_TOKENS: u32 = 32;
const MIN_QUERY_TOKENS: u32 = 4;

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

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn required_path(name: &str) -> PathBuf {
    std::env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("{name} must be set"))
}

/// The lane limits the daemon serves with: one text per inference call.
fn serving_limits() -> LocalEmbeddingsLimits {
    LocalEmbeddingsLimits {
        max_batch_items: 1,
        max_waiting_queries: 4,
        ..LocalEmbeddingsLimits::default()
    }
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
        while count(&text) < tokens {
            text.push(' ');
            text.push_str(stream.word());
        }
        if count(&text) == tokens {
            return text;
        }
    }
}

struct Sample {
    phase: &'static str,
    scheduled_us: u64,
    sent_us: u64,
    done_us: u64,
    tokens: u32,
    outcome: &'static str,
}

fn percentile(sorted: &[u64], q: f64) -> u64 {
    sorted[((sorted.len() as f64 * q).ceil() as usize).saturating_sub(1)]
}

/// Embeds one 76-token text through the job table as backfill does, from submission to its vector.
async fn backfill_one(
    component: &LocalEmbeddingsComponent,
    limits: EmbeddingInputLimits,
    lane: &host_runtime::local_embeddings::LaneInfo,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires EIDNARA_LOCAL_EMBEDDINGS_PRODUCTION_BUNDLE and EIDNARA_LOCAL_EMBEDDINGS_TEST_ORT_LIBRARY; run in release on the D21 host with --ignored"]
async fn production_lane_meets_the_reference_query_and_backfill_budgets() {
    let bundle_dir = required_path("EIDNARA_LOCAL_EMBEDDINGS_PRODUCTION_BUNDLE");
    let ort = required_path("EIDNARA_LOCAL_EMBEDDINGS_TEST_ORT_LIBRARY");
    let ort_sha256 = sha256_hex(&std::fs::read(&ort).expect("ORT library is readable"));
    let component = LocalEmbeddingsComponent::new(Some(LocalEmbeddingsConfig {
        bundle_dir,
        bundle_manifest_sha256: None,
        ort_library: ort,
        ort_library_sha256: ort_sha256,
        limits: serving_limits(),
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
    let mut stream = Stream(0x9e37_79b9_7f4a_7c15);
    let origin = Instant::now();
    let micros = |at: Instant| u64::try_from(at.duration_since(origin).as_micros()).unwrap();
    let mut samples: Vec<Sample> = Vec::new();

    // Warm-up runs before any timed phase so model load and first-touch costs stay out of every measurement.
    for n in 0..8 {
        let text = text_of(&component, limits, BACKFILL_TOKENS, &mut stream);
        backfill_one(&component, limits, &lane, &format!("warm-{n}"), &text).await;
        let query = text_of(&component, limits, MAX_QUERY_TOKENS, &mut stream);
        let admitted = component.preflight_embedding(limits, &query).unwrap();
        component
            .embed_query(
                &admitted,
                tokio::time::Instant::now() + Duration::from_secs(5),
            )
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
        services.push(u64::try_from(service.as_micros()).unwrap());
    }
    let throughput = BACKFILL_TEXTS as f64 / throughput_started.elapsed().as_secs_f64();
    let max_text_service_us = *services.iter().max().unwrap();

    // Open-loop queries: arrivals follow a Poisson schedule regardless of earlier queries, each timed from its scheduled arrival.
    let run_queries = |count: usize, phase: &'static str, stream: &mut Stream| {
        let mut at = 0.0f64;
        let schedule: Vec<(f64, String)> = (0..count)
            .map(|_| {
                at += -stream.uniform().ln() / QUERY_RATE_PER_SECOND;
                let tokens = MIN_QUERY_TOKENS
                    + (stream.next() % u64::from(MAX_QUERY_TOKENS - MIN_QUERY_TOKENS + 1)) as u32;
                (at, text_of(&component, limits, tokens, stream))
            })
            .collect();
        let component = component.clone();
        async move {
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
                            .embed_query(
                                &admitted,
                                tokio::time::Instant::now() + Duration::from_secs(5),
                            )
                            .await
                        {
                            Ok(vector) => {
                                assert_eq!(vector.len(), 768);
                                "ok"
                            }
                            Err(DenseUnavailable::LaneBusy { .. }) => "busy",
                            Err(_) => "refused",
                        };
                        (due, sent, Instant::now(), tokens, outcome)
                    })
                })
                .collect();
            let mut out = Vec::with_capacity(tasks.len());
            for task in tasks {
                let (due, sent, done, tokens, outcome) = task.await.unwrap();
                out.push((phase, due, sent, done, tokens, outcome));
            }
            out
        }
    };

    let idle = run_queries(IDLE_QUERIES, "idle", &mut stream).await;

    // The same open-loop queries while backfill keeps the native slot busy with one text after another.
    let feeding = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let backfill_texts: Vec<String> = (0..64)
        .map(|_| text_of(&component, limits, BACKFILL_TOKENS, &mut stream))
        .collect();
    let feeder = {
        let component = component.clone();
        let lane = lane.clone();
        let feeding = std::sync::Arc::clone(&feeding);
        tokio::spawn(async move {
            let mut embedded = 0usize;
            while feeding.load(std::sync::atomic::Ordering::Relaxed) {
                let text = &backfill_texts[embedded % backfill_texts.len()];
                backfill_one(
                    &component,
                    limits,
                    &lane,
                    &format!("saturate-{embedded}"),
                    text,
                )
                .await;
                embedded += 1;
            }
            embedded
        })
    };
    let loaded = run_queries(BACKFILL_QUERIES, "backfill", &mut stream).await;
    feeding.store(false, std::sync::atomic::Ordering::Relaxed);
    let backfill_embedded = feeder.await.unwrap();

    for (phase, due, sent, done, tokens, outcome) in idle.iter().chain(&loaded) {
        samples.push(Sample {
            phase,
            scheduled_us: micros(*due),
            sent_us: micros(*sent),
            done_us: micros(*done),
            tokens: *tokens,
            outcome,
        });
    }
    let summarize = |phase: &str| {
        let phase_samples: Vec<&Sample> = samples.iter().filter(|s| s.phase == phase).collect();
        let completed = phase_samples.iter().filter(|s| s.outcome == "ok").count();
        let busy = phase_samples.iter().filter(|s| s.outcome == "busy").count();
        let mut latency: Vec<u64> = phase_samples
            .iter()
            .filter(|s| s.outcome == "ok")
            .map(|s| s.done_us - s.scheduled_us)
            .collect();
        latency.sort_unstable();
        (
            phase_samples.len(),
            completed,
            busy,
            percentile(&latency, 0.5),
            percentile(&latency, 0.99),
            *latency.last().unwrap(),
        )
    };
    let idle_summary = summarize("idle");
    let loaded_summary = summarize("backfill");
    // A query's queue wait under backfill is its latency past the idle median service of a query of its length.
    let mut idle_service: std::collections::BTreeMap<u32, Vec<u64>> =
        std::collections::BTreeMap::new();
    for sample in samples
        .iter()
        .filter(|s| s.phase == "idle" && s.outcome == "ok")
    {
        idle_service
            .entry(sample.tokens)
            .or_default()
            .push(sample.done_us - sample.sent_us);
    }
    let median_service = |tokens: u32| {
        let mut service = idle_service[&tokens].clone();
        service.sort_unstable();
        service[service.len() / 2]
    };
    let max_queue_wait_us = samples
        .iter()
        .filter(|s| s.phase == "backfill" && s.outcome == "ok")
        .map(|s| (s.done_us - s.scheduled_us).saturating_sub(median_service(s.tokens)))
        .max()
        .unwrap();
    eprintln!(
        "threads {}; backfill {throughput:.2} texts/s over {BACKFILL_TEXTS} texts of {BACKFILL_TOKENS} tokens, max single-text service {max_text_service_us} us; \
         idle queries at {QUERY_RATE_PER_SECOND}/s (offered, completed, busy, p50, p99, max us) {idle_summary:?}; \
         under backfill {loaded_summary:?} with {backfill_embedded} backfill texts embedded and max queue wait {max_queue_wait_us} us",
        host_runtime::local_embeddings::inference::inference_threads(),
    );
    if let Some(path) = std::env::var_os("EIDNARA_LOCAL_EMBEDDINGS_SAMPLES") {
        let mut rows = vec![format!(
            "{{\"rate_per_second\":{QUERY_RATE_PER_SECOND},\"seed\":\"0x9e3779b97f4a7c15\",\"threads\":{},\"throughput\":{throughput},\"backfill_service_us\":{services:?}}}",
            host_runtime::local_embeddings::inference::inference_threads()
        )];
        rows.extend(samples.iter().map(|s| {
            format!(
                "{{\"phase\":\"{}\",\"scheduled_us\":{},\"sent_us\":{},\"done_us\":{},\"tokens\":{},\"outcome\":\"{}\"}}",
                s.phase, s.scheduled_us, s.sent_us, s.done_us, s.tokens, s.outcome
            )
        }));
        std::fs::write(path, rows.join("\n")).unwrap();
    }

    assert!(
        throughput >= 8.0,
        "76-token backfill throughput {throughput:.2} texts/s is under 8"
    );
    assert_eq!(idle_summary.1, IDLE_QUERIES, "every idle query completes");
    assert!(
        idle_summary.4 <= 60_000,
        "query embedding p99 {}us exceeds 60ms",
        idle_summary.4
    );
    assert_eq!(loaded_summary.2, 0, "no query is busy under backfill");
    assert!(
        max_queue_wait_us <= 2 * max_text_service_us,
        "queue wait {max_queue_wait_us}us exceeds twice the longest text service {max_text_service_us}us"
    );
    assert_eq!(
        loaded_summary.1, BACKFILL_QUERIES,
        "every query under backfill completes"
    );
}

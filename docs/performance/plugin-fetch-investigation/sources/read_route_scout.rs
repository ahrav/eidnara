//! The binary benchmarks each `kernel.read` stage using the `kernel_routes` fixture.
//!
//! Usage: `read_route_scout <rows> <scope_count> [reps] [k]`
//!
//! Output lines:
//! - `ROUTE bytes=<n> rows=<n>`: the real route's encoded body.
//! - `TIME <stage> min_ms=.. median_ms=.. max_ms=.. reps=..`: warm timings.
//! - `PARITY <check> ok|MISMATCH`: byte parity of an alternative encoder.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::hint::black_box;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use daemon::dispatch::PreparedOutcome;
use daemon::kernel_route_fixtures::{
    DOMAIN, admission, commit_request, intent, project_scope_spec, read_request, route_identity,
    seed_domain, sha256_hex,
};
use daemon::kernel_routes::read::{MAX_READ_ROW_BYTES, MAX_READ_ROWS, NewestRows};
use daemon::kernel_routes::{KernelOutcome, KernelState};
use daemon::{Handler, dev_descriptor_at};
use host_runtime::{BindOutcome, CompositeComponent, HostInit, PrimaryComponent, RouteHandle};
use kernel::{
    DecisionPayload, DecisionRow, DecisionSpec, Dimension, EventKind, KernelStore, ScopeTermFilter,
    Sensitivity, SourceClass, Surface, TaintClass, VisibleRow,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const SESSION: &str = "session-bench";

struct Daemon {
    _data: tempfile::TempDir,
    handler: Handler,
    route: RouteHandle,
    project: PathBuf,
    runtime: tokio::runtime::Runtime,
    encoded: RefCell<Vec<u8>>,
}

impl Daemon {
    fn start() -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime");
        let data = tempfile::tempdir().expect("tempdir");
        let descriptor = dev_descriptor_at(data.path().to_str().unwrap());
        let handler = Handler::new();
        handler.disable_kernel_sampler_for_test();
        let project = data.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let route = RouteHandle {
            channel: 7,
            epoch: 1,
        };
        runtime.block_on(async {
            PrimaryComponent::initialize(
                &handler,
                HostInit {
                    host_capabilities: Vec::new(),
                    storage: Some(serde_json::to_value(&descriptor).unwrap()),
                },
            )
            .await
            .unwrap();
            PrimaryComponent::activate(&handler).await.unwrap();
            let started = Instant::now();
            while handler.kernel_state() != KernelState::Ready {
                assert!(
                    started.elapsed() < Duration::from_secs(20),
                    "kernel never opened"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let identity = route_identity(&project, "bench", SESSION);
            assert!(matches!(
                handler.bind(route, identity).await,
                BindOutcome::Accept
            ));
        });
        let daemon = Self {
            _data: data,
            handler,
            route,
            project,
            runtime,
            encoded: RefCell::new(Vec::with_capacity(1 << 20)),
        };
        seed_domain(&daemon.store());
        daemon
    }

    fn store(&self) -> Arc<KernelStore> {
        self.handler.kernel_store_for_test().unwrap()
    }

    fn call(&self, request: Value) -> usize {
        let outcome = self
            .runtime
            .block_on(self.handler.dispatch_value_for_test(self.route, request));
        match outcome {
            PreparedOutcome::Response(output) => {
                if let Some(body) = output.json_for_test() {
                    if body["state"]["kind"] != "available" {
                        panic!("kernel route did not answer available: {}", body["state"]);
                    }
                }
                let mut encoded = self.encoded.borrow_mut();
                encoded.clear();
                let measured = output.measure().expect("response measures");
                measured.write_to(&mut *encoded).expect("response encodes");
                encoded.len()
            }
            PreparedOutcome::Error { code, message } => {
                panic!("kernel route answered {code}: {message}")
            }
            PreparedOutcome::Streamed => panic!("kernel route streamed"),
        }
    }

    fn call_json(&self, request: Value) -> Value {
        self.call(request);
        serde_json::from_slice(&self.encoded.borrow()).expect("response is JSON")
    }

    fn project_scope_id(&self) -> String {
        let response = self.call_json(commit_request(
            &self.project,
            SESSION,
            "seed-scope",
            vec![json!({"op": "insert_decision", "spec": {
                "decision_id": "seed-decision-0",
                "object_id": "seed-decision-object-0",
                "domain_id": DOMAIN,
                "decision_kind": "memory",
                "payload": {
                    "summary": "decision 0 summarises a remembered fact about the project",
                    "rationale": "because the assistant observed it in turn 0",
                },
                "source_id": "memory-lineage",
                "source_revision": 1,
            }})],
            vec![],
        ));
        assert_eq!(response["receipt"]["replayed"], false);
        let read = self.call_json(read_request(
            &self.project,
            SESSION,
            "explicit_search",
            None,
        ));
        read["rows"][0]["scope_id"]
            .as_str()
            .expect("seed row carries the project scope")
            .to_string()
    }

    fn shutdown(self) {
        let Self {
            handler, runtime, ..
        } = self;
        runtime.block_on(handler.shutdown()).unwrap();
    }
}

fn filler(index: usize) -> String {
    let bytes: usize = std::env::var("SCOUT_PAYLOAD_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if bytes == 0 {
        return String::new();
    }
    let mut text = String::with_capacity(bytes + 16);
    let mut word = index;
    while text.len() < bytes {
        word = word.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        text.push_str(" filler");
        text.push_str(&format!("{:x}", (word >> 33) & 0xfff));
    }
    text
}

fn store_decision(index: usize, scope_id: &str) -> DecisionSpec {
    DecisionSpec {
        decision_id: format!("store-decision-{index}"),
        object_id: format!("store-decision-object-{index}"),
        domain_id: DOMAIN.to_string(),
        proposition_id: None,
        scope_id: Some(scope_id.to_string()),
        anchor_id: None,
        evidence_id: None,
        decision_kind: "architecture".to_string(),
        payload: DecisionPayload {
            summary: format!("decision {index} summarises a remembered fact about the project"),
            rationale: format!(
                "because the assistant observed it in turn {index}{}",
                filler(index)
            ),
        },
        source_kind: "repo".to_string(),
        source_id: format!("lineage-{}", index % 97),
        source_revision: 1,
        sensitivity: Sensitivity::Normal,
    }
}

fn seed_decisions(store: &KernelStore, count: usize, scope_ids: &[String]) {
    let hidden_every: usize = std::env::var("SCOUT_HIDDEN_EVERY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut index = 0;
    while index < count {
        let batch_end = (index + 256).min(count);
        let batch: Vec<usize> = (index..batch_end).collect();
        store
            .commit(intent(&format!("seed-decisions-{index}")), |envelope| {
                for &i in &batch {
                    let scope_id = &scope_ids[i % scope_ids.len()];
                    let spec = store_decision(i, scope_id);
                    let object_id = spec.object_id.clone();
                    envelope.insert_decision(spec)?;
                    if hidden_every > 0 && i % hidden_every == hidden_every - 1 {
                        continue;
                    }
                    envelope.record_admission(admission(
                        &object_id,
                        EventKind::Other,
                        None,
                        (SourceClass::ModelInference, TaintClass::AssistantInference),
                    ))?;
                }
                Ok(String::new())
            })
            .unwrap();
        index = batch_end;
    }
}

fn seed_foreign_scopes(store: &KernelStore, count: usize) -> Vec<String> {
    let scopes: Vec<(String, String)> = (0..count)
        .map(|i| {
            let digest = sha256_hex(format!("/other/project/{i}").as_bytes());
            (format!("project:{digest}"), digest)
        })
        .collect();
    store
        .commit(intent("seed-foreign-scopes"), |envelope| {
            for (scope_id, digest) in &scopes {
                envelope.insert_scope(project_scope_spec(scope_id, digest))?;
            }
            Ok(String::new())
        })
        .unwrap();
    scopes.into_iter().map(|(scope_id, _)| scope_id).collect()
}

struct Timer {
    stages: Vec<(&'static str, Vec<f64>)>,
}

impl Timer {
    fn new() -> Self {
        Self { stages: Vec::new() }
    }

    fn time<T>(&mut self, name: &'static str, f: impl FnOnce() -> T) -> T {
        let started = Instant::now();
        let value = f();
        let elapsed = started.elapsed().as_secs_f64() * 1e3;
        match self.stages.iter_mut().find(|(n, _)| *n == name) {
            Some((_, samples)) => samples.push(elapsed),
            None => self.stages.push((name, vec![elapsed])),
        }
        value
    }

    fn push(&mut self, name: &'static str, elapsed: f64) {
        match self.stages.iter_mut().find(|(n, _)| *n == name) {
            Some((_, samples)) => samples.push(elapsed),
            None => self.stages.push((name, vec![elapsed])),
        }
    }

    /// Drops the first sample of every stage (warm-up) and reports the rest.
    fn report(&self) {
        for (name, samples) in &self.stages {
            let mut warm: Vec<f64> = samples.iter().skip(1).copied().collect();
            if warm.is_empty() {
                continue;
            }
            warm.sort_by(f64::total_cmp);
            let median = warm[warm.len() / 2];
            println!(
                "TIME {name} min_ms={:.4} median_ms={median:.4} max_ms={:.4} reps={} first_ms={:.4}",
                warm[0],
                warm[warm.len() - 1],
                warm.len(),
                samples[0]
            );
        }
    }
}

fn row_json(row: &VisibleRow, decision: Option<&DecisionRow>, known_as_of: i64) -> Value {
    json!({
        "object": row.object,
        "visibility": row.visibility.as_str(),
        "labeled": row.labeled,
        "scope_id": row.scope_id,
        "token": {"object_id": row.object.object_id, "known_as_of": known_as_of},
        "decision": decision.map(|decision| json!({
            "decision_kind": decision.decision_kind,
            "payload": decision.payload,
        })),
    })
}

#[derive(Default)]
struct CountingWriter {
    len: usize,
}

impl Write for CountingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.len += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn measure<T: Serialize + ?Sized>(value: &T) -> usize {
    let mut writer = CountingWriter::default();
    serde_json::to_writer(&mut writer, value).expect("measure serializes");
    writer.len
}

/// Field order is the sorted key order `serde_json::Map` (a `BTreeMap`) emits.
#[derive(Serialize)]
struct TypedObject<'a> {
    created_commit_seq: i64,
    domain_id: &'a str,
    invalidated_commit_seq: Option<i64>,
    object_id: &'a str,
    object_kind: &'a str,
    sensitivity: Sensitivity,
    source_id: &'a str,
    source_kind: &'a str,
    source_revision: i64,
    superseded_by: Option<&'a str>,
}

#[derive(Serialize)]
struct TypedPayload<'a> {
    rationale: &'a str,
    summary: &'a str,
}

#[derive(Serialize)]
struct TypedDecision<'a> {
    decision_kind: &'a str,
    payload: TypedPayload<'a>,
}

#[derive(Serialize)]
struct TypedToken<'a> {
    known_as_of: i64,
    object_id: &'a str,
}

#[derive(Serialize)]
struct TypedRow<'a> {
    decision: Option<TypedDecision<'a>>,
    labeled: bool,
    object: TypedObject<'a>,
    scope_id: Option<&'a str>,
    token: TypedToken<'a>,
    visibility: &'static str,
}

impl<'a> TypedRow<'a> {
    fn new(row: &'a VisibleRow, decision: Option<&'a DecisionRow>, known_as_of: i64) -> Self {
        let object = &row.object;
        Self {
            decision: decision.map(|decision| TypedDecision {
                decision_kind: &decision.decision_kind,
                payload: TypedPayload {
                    rationale: &decision.payload.rationale,
                    summary: &decision.payload.summary,
                },
            }),
            labeled: row.labeled,
            object: TypedObject {
                created_commit_seq: object.created_commit_seq,
                domain_id: &object.domain_id,
                invalidated_commit_seq: object.invalidated_commit_seq,
                object_id: &object.object_id,
                object_kind: &object.object_kind,
                sensitivity: object.sensitivity,
                source_id: &object.source_id,
                source_kind: &object.source_kind,
                source_revision: object.source_revision,
                superseded_by: object.superseded_by.as_deref(),
            },
            scope_id: row.scope_id.as_deref(),
            token: TypedToken {
                known_as_of,
                object_id: &object.object_id,
            },
            visibility: row.visibility.as_str(),
        }
    }
}

#[derive(Serialize)]
struct TypedResponse<'a> {
    gated: bool,
    known_as_of: i64,
    rows: &'a [TypedRow<'a>],
    state: &'a KernelOutcome,
    tip: i64,
    truncated: bool,
}

/// Unique lowercase operands of at least two characters, split on
/// `/[^\p{L}\p{N}_]+/u` as `kernel-memory-search.ts` does.
fn query_terms(query: &str) -> Vec<String> {
    let lowered = query.to_lowercase();
    let mut terms: Vec<String> = Vec::new();
    for term in lowered.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
        if term.chars().count() >= 2 && !terms.iter().any(|t| t == term) {
            terms.push(term.to_string());
        }
    }
    terms
}

struct Hit<'a> {
    score: f64,
    row: &'a VisibleRow,
}

/// Score is the matched share of `terms` over lowercase `summary\nrationale`;
/// order is score desc, `created_commit_seq` desc, `object_id` asc.
fn rank<'a>(
    rows: &'a [VisibleRow],
    decisions: &HashMap<String, DecisionRow>,
    terms: &[String],
    k: usize,
    scratch: &mut String,
) -> Vec<Hit<'a>> {
    let mut hits: Vec<Hit<'a>> = Vec::new();
    for row in rows {
        let Some(decision) = decisions.get(&row.object.object_id) else {
            continue;
        };
        scratch.clear();
        for c in decision.payload.summary.chars() {
            scratch.extend(c.to_lowercase());
        }
        scratch.push('\n');
        for c in decision.payload.rationale.chars() {
            scratch.extend(c.to_lowercase());
        }
        let matched = terms
            .iter()
            .filter(|term| scratch.contains(term.as_str()))
            .count();
        if matched > 0 {
            hits.push(Hit {
                score: matched as f64 / terms.len() as f64,
                row,
            });
        }
    }
    hits.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| {
                right
                    .row
                    .object
                    .created_commit_seq
                    .cmp(&left.row.object.created_commit_seq)
            })
            .then_with(|| left.row.object.object_id.cmp(&right.row.object.object_id))
    });
    hits.truncate(k);
    hits
}


/// Borrowed payload view for the scan; escaped strings fall back to owned.
#[derive(Deserialize)]
struct PayloadView<'a> {
    #[serde(borrow)]
    summary: std::borrow::Cow<'a, str>,
    #[serde(borrow)]
    rationale: std::borrow::Cow<'a, str>,
}

struct Candidate {
    matched: usize,
    created_commit_seq: i64,
    object_id: String,
}

/// Lowercases `text` into `scratch`, taking the ASCII fast path when it applies.
fn lower_into(text: &str, scratch: &mut String) {
    if text.is_ascii() {
        scratch.push_str(text);
        let start = scratch.len() - text.len();
        scratch[start..].make_ascii_lowercase();
    } else {
        for c in text.chars() {
            scratch.extend(c.to_lowercase());
        }
    }
}

struct Pushdown {
    conn: rusqlite::Connection,
    scope_id: String,
}

impl Pushdown {
    fn open(data_root: &std::path::Path, scope_id: &str) -> Self {
        fn find(dir: &std::path::Path) -> Option<PathBuf> {
            for entry in fs::read_dir(dir).ok()? {
                let path = entry.ok()?.path();
                if path.is_dir() {
                    if let Some(found) = find(&path) {
                        return Some(found);
                    }
                } else if path.file_name().is_some_and(|name| name == "kernel.sqlite") {
                    return Some(path);
                }
            }
            None
        }
        let path = find(data_root).expect("kernel.sqlite under the data root");
        let conn = rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .expect("read-only kernel connection");
        Self {
            conn,
            scope_id: scope_id.to_string(),
        }
    }

    /// Scans live decisions in the project scope at `as_of`, scores each
    /// payload against `terms`, and returns the matches ordered like the
    /// client ranker: matched share desc, `created_commit_seq` desc,
    /// `object_id` asc.
    fn scan_rank(&self, as_of: i64, terms: &[String], scratch: &mut String) -> Vec<Candidate> {
        let domain_join = std::env::var("SCOUT_SCAN_DOMAIN_JOIN").is_ok();
        let sql = if domain_join {
            "SELECT d.object_id, d.created_commit_seq, d.decision_payload FROM decisions d
             JOIN object_registry o ON o.object_id=d.object_id AND o.domain_id=?3
             WHERE d.scope_id=?1 AND d.created_commit_seq<=?2
               AND (d.invalidated_commit_seq IS NULL OR ?2<d.invalidated_commit_seq)"
        } else {
            "SELECT object_id, created_commit_seq, decision_payload FROM decisions
             WHERE scope_id=?1 AND created_commit_seq<=?2
               AND (invalidated_commit_seq IS NULL OR ?2<invalidated_commit_seq)"
        };
        let mut statement = self.conn.prepare_cached(sql).unwrap();
        let mut rows = if domain_join {
            statement
                .query(rusqlite::params![self.scope_id, as_of, DOMAIN])
                .unwrap()
        } else {
            statement
                .query(rusqlite::params![self.scope_id, as_of])
                .unwrap()
        };
        let mut candidates = Vec::new();
        while let Some(row) = rows.next().unwrap() {
            let payload: &[u8] = row.get_ref(2).unwrap().as_blob().unwrap();
            let view: PayloadView<'_> = serde_json::from_slice(payload).unwrap();
            scratch.clear();
            lower_into(&view.summary, scratch);
            scratch.push('\n');
            lower_into(&view.rationale, scratch);
            let matched = terms
                .iter()
                .filter(|term| memchr::memmem::find(scratch.as_bytes(), term.as_bytes()).is_some())
                .count();
            if matched > 0 {
                candidates.push(Candidate {
                    matched,
                    created_commit_seq: row.get(1).unwrap(),
                    object_id: row.get(0).unwrap(),
                });
            }
        }
        candidates.sort_unstable_by(|left, right| {
            right
                .matched
                .cmp(&left.matched)
                .then_with(|| right.created_commit_seq.cmp(&left.created_commit_seq))
                .then_with(|| left.object_id.cmp(&right.object_id))
        });
        candidates
    }
}

/// Authorizes ranked candidates through the served-class query in batches of
/// at most 64 ids until `k` visible rows are found or candidates run out.
fn authorize<'a>(
    store: &KernelStore,
    filter: ScopeTermFilter<'_>,
    as_of: i64,
    candidates: &'a [Candidate],
    k: usize,
) -> (Vec<VisibleRow>, usize) {
    let mut visible_rows: Vec<VisibleRow> = Vec::with_capacity(k);
    let mut batches = 0usize;
    for batch in candidates.chunks(64) {
        batches += 1;
        let ids: Vec<String> = batch.iter().map(|c| c.object_id.clone()).collect();
        let visible = store
            .visible_as_of_in_scope(Surface::ExplicitSearch, as_of, Some(&ids), Some(filter))
            .unwrap();
        let by_id: HashMap<&str, VisibleRow> = visible
            .rows
            .into_iter()
            .map(|row| (row.object.object_id.clone().leak() as &str, row))
            .collect();
        for candidate in batch {
            if let Some(row) = by_id.get(candidate.object_id.as_str()) {
                visible_rows.push(row.clone());
                if visible_rows.len() == k {
                    return (visible_rows, batches);
                }
            }
        }
    }
    (visible_rows, batches)
}

fn main() {
    if std::env::var("SCOUT_LOWER").is_ok() {
        for sample in [
            "İstanbul", "STRAẞE", "ΣΊΣΥΦΟΣ", "ǅemal", "ＡＢＣ", "Ⅸ", "ﬀ", "ᾈ", "ΑΣ", "İ", "K", "ẞ",
            "ΌΣ ΟΣ", "ΠΡΟΣ",
        ] {
            let lowered = sample.to_lowercase();
            let hex: String = lowered.bytes().map(|b| format!("{b:02x}")).collect();
            println!("LOWER {sample:?} -> {lowered:?} {hex}");
        }
        return;
    }
    let args: Vec<String> = std::env::args().collect();
    let rows: usize = args.get(1).map_or(1000, |v| v.parse().unwrap());
    let scope_count: usize = args.get(2).map_or(1, |v| v.parse().unwrap());
    let reps: usize = args.get(3).map_or(21, |v| v.parse().unwrap());
    let k: usize = args.get(4).map_or(8, |v| v.parse().unwrap());
    let dump = std::env::var("SCOUT_DUMP").ok();

    let daemon = Daemon::start();
    let own_scope = daemon.project_scope_id();
    let digest = own_scope
        .strip_prefix("project:")
        .expect("scope id is project:<digest>")
        .to_string();
    let mut scopes = vec![own_scope];
    if scope_count > 1 {
        scopes.extend(seed_foreign_scopes(&daemon.store(), scope_count - 1));
    }
    seed_decisions(&daemon.store(), rows, &scopes);
    let store = daemon.store();
    let request = read_request(&daemon.project, SESSION, "explicit_search", None);
    let pushdown = Pushdown::open(daemon._data.path(), &scopes[0]);

    let route_bytes = daemon.call(request.clone());
    let reference: Vec<u8> = daemon.encoded.borrow().clone();
    let reference_value: Value = serde_json::from_slice(&reference).unwrap();
    let served = reference_value["rows"].as_array().unwrap().len();
    println!("ROUTE bytes={route_bytes} rows={served} fixture_rows={rows} scopes={scope_count}");
    if let Some(path) = &dump {
        fs::write(path, &reference).unwrap();
        println!("DUMP path={path}");
    }

    let mut timer = Timer::new();
    if std::env::var("SCOUT_CONCURRENCY").is_ok() {
        let terms = query_terms("remembered fact");
        let tip = store.tip().unwrap();
        let filter_digest = digest.clone();
        for threads in [1usize, 4, 8] {
            for _ in 0..reps {
                let name = match threads {
                    1 => "concurrency/route/1x10",
                    4 => "concurrency/route/4x10",
                    _ => "concurrency/route/8x10",
                };
                timer.time(name, || {
                    std::thread::scope(|scope| {
                        for _ in 0..threads {
                            let handler = &daemon.handler;
                            let runtime = &daemon.runtime;
                            let route = daemon.route;
                            let request = &request;
                            scope.spawn(move || {
                                let mut encoded = Vec::with_capacity(1 << 20);
                                for _ in 0..10 {
                                    let outcome = runtime.block_on(
                                        handler.dispatch_value_for_test(route, request.clone()),
                                    );
                                    let PreparedOutcome::Response(output) = outcome else {
                                        panic!("route did not answer");
                                    };
                                    encoded.clear();
                                    let measured = output.measure().unwrap();
                                    measured.write_to(&mut encoded).unwrap();
                                    black_box(encoded.len());
                                }
                            });
                        }
                    });
                });
                let name = match threads {
                    1 => "concurrency/pushdown/1x10",
                    4 => "concurrency/pushdown/4x10",
                    _ => "concurrency/pushdown/8x10",
                };
                timer.time(name, || {
                    std::thread::scope(|scope| {
                        for _ in 0..threads {
                            let store = &store;
                            let terms = &terms;
                            let data_root = daemon._data.path();
                            let scope_id = scopes[0].as_str();
                            let filter_digest = filter_digest.as_str();
                            scope.spawn(move || {
                                let pushdown = Pushdown::open(data_root, scope_id);
                                let filter = ScopeTermFilter {
                                    dimension: Dimension::Project,
                                    value: filter_digest,
                                };
                                let mut scratch = String::new();
                                for _ in 0..10 {
                                    let candidates = pushdown.scan_rank(tip, terms, &mut scratch);
                                    let (visible_rows, _) =
                                        authorize(store, filter, tip, &candidates, k);
                                    let ids: Vec<String> = visible_rows
                                        .iter()
                                        .map(|row| row.object.object_id.clone())
                                        .collect();
                                    let hydrated =
                                        store.decisions_for_objects_as_of(&ids, tip).unwrap();
                                    black_box(hydrated.len());
                                }
                            });
                        }
                    });
                });
            }
        }
        timer.report();
        daemon.shutdown();
        return;
    }
    if std::env::var("SCOUT_ROUTE_ONLY").is_ok() {
        for _ in 0..reps {
            timer.time("route/full", || black_box(daemon.call(request.clone())));
        }
        timer.report();
        daemon.shutdown();
        return;
    }
    let queries: Vec<(&str, Vec<String>)> = [
        ("common", "remembered fact"),
        ("selective", "turn 77"),
        ("absent", "zebra quokka"),
        ("long", "SQLite cache ordinary absent another last other word"),
    ]
    .into_iter()
    .map(|(name, query)| (name, query_terms(query)))
    .collect();
    let mut parity_typed = true;
    let mut ranked_ids: HashMap<&str, Vec<String>> = HashMap::new();

    for rep in 0..reps {
        timer.time("route/full", || black_box(daemon.call(request.clone())));

        let tip = timer.time("stage/tip", || store.tip().unwrap());
        let filter = ScopeTermFilter {
            dimension: Dimension::Project,
            value: &digest,
        };
        let visible = timer.time("stage/visible_as_of_in_scope", || {
            store
                .visible_as_of_in_scope(Surface::ExplicitSearch, tip, None, Some(filter))
                .unwrap()
        });
        let known_as_of = visible.known_as_of;
        let (kept, truncated) = timer.time("stage/newest_heap", || {
            let mut newest = NewestRows::new(MAX_READ_ROWS);
            for row in visible.rows {
                newest.push(row);
            }
            newest.finish()
        });
        let decision_ids: Vec<String> = kept
            .iter()
            .filter(|row| row.object.object_kind == "decision")
            .map(|row| row.object.object_id.clone())
            .collect();
        let sizes = timer.time("stage/payload_sizes", || {
            store
                .decision_payload_sizes_as_of(&decision_ids, known_as_of)
                .unwrap()
        });
        black_box(&sizes);
        let decisions: HashMap<String, DecisionRow> = timer.time("stage/decisions_hydrate", || {
            store
                .decisions_for_objects_as_of(&decision_ids, known_as_of)
                .unwrap()
                .into_iter()
                .map(|decision| (decision.object_id.clone(), decision))
                .collect()
        });

        let values: Vec<Value> = timer.time("json/row_json_trees", || {
            kept.iter()
                .map(|row| row_json(row, decisions.get(&row.object.object_id), known_as_of))
                .collect()
        });
        let row_bytes: usize = timer.time("json/row_measure", || {
            values.iter().map(|value| measure(value) + 1).sum()
        });
        assert!(row_bytes <= MAX_READ_ROW_BYTES);
        let body = timer.time("json/wrap_response", || {
            let mut body = match json!({
                "known_as_of": known_as_of,
                "tip": visible.tip,
                "gated": false,
                "truncated": truncated,
                "rows": values,
            }) {
                Value::Object(map) => map,
                _ => unreachable!(),
            };
            body.insert(
                "state".to_string(),
                serde_json::to_value(KernelOutcome::Available).unwrap(),
            );
            Value::Object(body)
        });
        let whole = timer.time("json/measure_whole", || measure(&body));
        let mut out = Vec::with_capacity(whole);
        timer.time("json/write_whole", || {
            serde_json::to_writer(&mut out, &body).unwrap();
        });
        assert_eq!(out.len(), whole);
        if rep == 0 {
            println!(
                "PARITY json_replica_bytes {}",
                if out == reference { "ok" } else { "MISMATCH" }
            );
        }
        timer.time("json/drop_trees", || drop(body));

        let state = KernelOutcome::Available;
        let typed_rows: Vec<TypedRow<'_>> = timer.time("typed/build_rows", || {
            kept.iter()
                .map(|row| TypedRow::new(row, decisions.get(&row.object.object_id), known_as_of))
                .collect()
        });
        let typed_row_bytes: usize = timer.time("typed/row_measure", || {
            typed_rows.iter().map(|row| measure(row) + 1).sum()
        });
        assert_eq!(typed_row_bytes, row_bytes);
        let response = TypedResponse {
            gated: false,
            known_as_of,
            rows: &typed_rows,
            state: &state,
            tip: visible.tip,
            truncated,
        };
        let typed_whole = timer.time("typed/measure_whole", || measure(&response));
        let mut typed_out = Vec::with_capacity(typed_whole);
        timer.time("typed/write_whole", || {
            serde_json::to_writer(&mut typed_out, &response).unwrap();
        });
        if typed_out != reference {
            parity_typed = false;
        }
        drop(typed_rows);

        let mut scratch = String::new();
        for (name, terms) in &queries {
            let hits = match *name {
                "common" => timer.time("rank/common", || {
                    rank(&kept, &decisions, terms, k, &mut scratch)
                }),
                "selective" => timer.time("rank/selective", || {
                    rank(&kept, &decisions, terms, k, &mut scratch)
                }),
                "absent" => timer.time("rank/absent", || {
                    rank(&kept, &decisions, terms, k, &mut scratch)
                }),
                _ => timer.time("rank/long", || {
                    rank(&kept, &decisions, terms, k, &mut scratch)
                }),
            };
            if rep == 0 {
                ranked_ids.insert(
                    name,
                    hits.iter()
                        .map(|hit| hit.row.object.object_id.clone())
                        .collect(),
                );
            }
            if *name == "common" {
                let top_rows: Vec<TypedRow<'_>> = timer.time("rank/encode_k_rows", || {
                    hits.iter()
                        .map(|hit| {
                            TypedRow::new(
                                hit.row,
                                decisions.get(&hit.row.object.object_id),
                                known_as_of,
                            )
                        })
                        .collect()
                });
                let top = TypedResponse {
                    gated: false,
                    known_as_of,
                    rows: &top_rows,
                    state: &state,
                    tip: visible.tip,
                    truncated,
                };
                let len = measure(&top);
                let mut top_out = Vec::with_capacity(len);
                serde_json::to_writer(&mut top_out, &top).unwrap();
                black_box(top_out.len());
            }
        }

        for c in [8usize, 64] {
            let ids: Vec<String> = decision_ids.iter().take(c).cloned().collect();
            let name = if c == 8 {
                "candidates/visible_named_8"
            } else {
                "candidates/visible_named_64"
            };
            let named = timer.time(name, || {
                store
                    .visible_as_of_in_scope(Surface::ExplicitSearch, tip, Some(&ids), Some(filter))
                    .unwrap()
            });
            black_box(named.rows.len());
            let name = if c == 8 {
                "candidates/hydrate_named_8"
            } else {
                "candidates/hydrate_named_64"
            };
            let hydrated = timer.time(name, || {
                store.decisions_for_objects_as_of(&ids, known_as_of).unwrap()
            });
            black_box(hydrated.len());
        }

        let mut ranked_pushdown: Vec<(&str, Vec<String>)> = Vec::new();
        for (name, terms) in &queries {
            let total_started = Instant::now();
            let candidates = timer.time(
                match *name {
                    "common" => "pushdown/common/scan_rank",
                    "selective" => "pushdown/selective/scan_rank",
                    "absent" => "pushdown/absent/scan_rank",
                    _ => "pushdown/long/scan_rank",
                },
                || pushdown.scan_rank(known_as_of, terms, &mut scratch),
            );
            let (visible_rows, batches) = timer.time(
                match *name {
                    "common" => "pushdown/common/authorize",
                    "selective" => "pushdown/selective/authorize",
                    "absent" => "pushdown/absent/authorize",
                    _ => "pushdown/long/authorize",
                },
                || authorize(&store, filter, known_as_of, &candidates, k),
            );
            black_box(batches);
            let ids: Vec<String> = visible_rows
                .iter()
                .map(|row| row.object.object_id.clone())
                .collect();
            let encoded_len = timer.time(
                match *name {
                    "common" => "pushdown/common/hydrate_encode",
                    "selective" => "pushdown/selective/hydrate_encode",
                    "absent" => "pushdown/absent/hydrate_encode",
                    _ => "pushdown/long/hydrate_encode",
                },
                || {
                    let hydrated: HashMap<String, DecisionRow> = store
                        .decisions_for_objects_as_of(&ids, known_as_of)
                        .unwrap()
                        .into_iter()
                        .map(|decision| (decision.object_id.clone(), decision))
                        .collect();
                    let top_rows: Vec<TypedRow<'_>> = visible_rows
                        .iter()
                        .map(|row| {
                            TypedRow::new(row, hydrated.get(&row.object.object_id), known_as_of)
                        })
                        .collect();
                    let top = TypedResponse {
                        gated: false,
                        known_as_of,
                        rows: &top_rows,
                        state: &state,
                        tip: visible.tip,
                        truncated: false,
                    };
                    let len = measure(&top);
                    let mut top_out = Vec::with_capacity(len);
                    serde_json::to_writer(&mut top_out, &top).unwrap();
                    top_out.len()
                },
            );
            black_box(encoded_len);
            let total = total_started.elapsed().as_secs_f64() * 1e3;
            timer.push(
                match *name {
                    "common" => "pushdown/common/total",
                    "selective" => "pushdown/selective/total",
                    "absent" => "pushdown/absent/total",
                    _ => "pushdown/long/total",
                },
                total,
            );
            if rep == 0 {
                ranked_pushdown.push((name, ids));
            }
        }
        if rep == 0 {
            for (name, ids) in &ranked_pushdown {
                let reference_ids = ranked_ids.get(name).cloned().unwrap_or_default();
                println!(
                    "PARITY pushdown_rank_{name} {}",
                    if *ids == reference_ids { "ok" } else { "MISMATCH" }
                );
            }
        }

        drop(decisions);
        drop(kept);
    }

    println!(
        "PARITY typed_bytes {}",
        if parity_typed { "ok" } else { "MISMATCH" }
    );
    for (name, ids) in &ranked_ids {
        println!("RANK {name} k={} ids={}", ids.len(), ids.join(","));
    }
    timer.report();
    daemon.shutdown();
}

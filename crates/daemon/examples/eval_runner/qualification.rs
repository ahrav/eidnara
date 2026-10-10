use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io::{self, BufRead, BufWriter, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use daemon::synthetic_history::SyntheticHistory;
use eval_core::{
    ACTIVE_WINDOW, BacklogSample, Case, CaseResult, DIRTY_SOURCE_SUFFIX, Environment,
    FAKE_MODEL_LATENCY_MS, Histogram, HostManifest, MESSAGE_TOKENS, OutageRun, OutageSchedule,
    Phase, Publication, QUALIFICATION_SEED, QualificationReport, RawRange, Repetition, Shape,
    StoreObservation, WitnessRun, audit_lineage, catalog, check_witness_runs,
};
use flate2::{Compression, write::DeflateEncoder};
use host_runtime::{CallError, Client, ClientRoute, RequestOptions, TargetKind};
use memory_store::MemoryStore;
use rusqlite::params;
use serde_json::{Value, json};

use super::campaign::{command, parse_flags_with_optional, sha256_hex};
use super::scale::commit_anchor;
use super::support::direct_host::{FixtureProcess, Launch, fixture_binary, wait_for_store};
use super::support::eval_surface::block_on;

pub const USAGE: &str = "qualification --out <report.json> --cases <all|name,...> \
     --operations <per repetition> --repetitions <n> --outage-scale <divisor> \
     --state-dir <dir on the measured filesystem> \
     [--witness-runs <json object of witness name to {test, exit_code}>]";

const RAW_CAP_BYTES: u64 = memory_store::MAX_SESSION_TRANSCRIPT_COMPRESSED_BYTES as u64;

const WORDS: [&str; 8] = [
    " fold", " cache", " segment", " window", " budget", " tier", " branch", " entry",
];

fn message_text(seed: u64, ordinal: u64) -> String {
    const PADDED_WORDS: [[u8; 8]; WORDS.len()] = {
        let mut padded = [[0; 8]; WORDS.len()];
        let mut word = 0;
        while word < WORDS.len() {
            let bytes = WORDS[word].as_bytes();
            let mut byte = 0;
            while byte < bytes.len() {
                assert!(bytes[byte] == b' ' || bytes[byte].is_ascii_lowercase());
                padded[word][byte] = bytes[byte];
                byte += 1;
            }
            word += 1;
        }
        padded
    };
    let mut state = (seed ^ ordinal.wrapping_mul(0x9E37_79B9_7F4A_7C15)) | 1;
    let mut bytes = [0; MESSAGE_TOKENS * 8];
    let mut len = 0;
    for _ in 0..MESSAGE_TOKENS {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let word = (state % WORDS.len() as u64) as usize;
        bytes[len..len + 8].copy_from_slice(&PADDED_WORDS[word]);
        len += WORDS[word].len();
    }
    String::from_utf8(bytes[..len].to_vec()).expect("concatenated words are UTF-8")
}

/// ASCII text long enough to fill one transform frame; requests slice it to fit.
fn filler() -> &'static str {
    static FILLER: OnceLock<String> = OnceLock::new();
    FILLER.get_or_init(|| {
        let unit = message_text(QUALIFICATION_SEED, 0);
        unit.repeat(daemon::MAX_TRANSFORM_FRAME_BYTES / unit.len() + 1)
    })
}

fn window_message(shape: Shape, ordinal: u64) -> Value {
    let text = match shape {
        Shape::Noise => " .,;".repeat(MESSAGE_TOKENS / 4),
        _ => message_text(QUALIFICATION_SEED, ordinal),
    };
    message(shape, ordinal, text)
}

fn message(shape: Shape, ordinal: u64, text: String) -> Value {
    let mid = format!("m{ordinal}");
    let role = match shape {
        Shape::SameRole => "user",
        Shape::SystemOnly => "system",
        _ if ordinal % 2 == 1 => "user",
        _ => "assistant",
    };
    let tool = (shape == Shape::ToolArc && role == "assistant")
        || (shape == Shape::Mixed && ordinal.is_multiple_of(16));
    let mut content = vec![json!({"kind": {"type": "text", "text": text}})];
    if tool {
        let id = format!("call-{ordinal}");
        content.push(json!({"kind": {
            "type": "tool_call", "id": id, "name": "read", "input": {"path": "src/lib.rs"},
        }}));
        content.push(json!({"kind": {
            "type": "tool_result", "id": id, "tool_name": "read",
            "output": {"kind": {"type": "text", "text": message_text(7, ordinal)}},
        }}));
    }
    json!({
        "mid": mid,
        "ordinal": ordinal,
        "ck": {"role": role, "content": content, "meta": {"harness_id": mid}},
    })
}

/// A session's folded frontier: the last folded ordinal and the segment sequence ending there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Anchor {
    end: u64,
    sequence: u64,
}

fn transform(session: &str, anchor: Anchor, revision: String, messages: Vec<Value>) -> Value {
    json!({
        "kind": "transform",
        "v": 3,
        "boundary": {"mid": format!("m{}", anchor.end), "sequence": anchor.sequence},
        "base_revision": revision,
        "session_id": session,
        "serializer_profile": "owned-llmrunner",
        "render_config": "qualification-config",
        "messages": messages,
    })
}

/// Headroom the largest-admitted request leaves under the transform frame cap.
const LARGEST_HEADROOM_BYTES: usize = 1024;

fn window_request(case: &Case, session: &str, anchor: Anchor, turn: u64) -> Value {
    let stride = if case.shape == Shape::Sparse { 3 } else { 1 };
    let window = if case.shape == Shape::LargestAdmitted {
        1
    } else {
        ACTIVE_WINDOW
    };
    let build = |text: Option<&str>| {
        let mut messages = vec![window_message(Shape::Mixed, anchor.end)];
        messages.extend((1..=window).map(|offset| {
            let ordinal = anchor.end + offset * stride;
            match text {
                Some(text) => message(case.shape, ordinal, text.to_owned()),
                None => window_message(case.shape, ordinal),
            }
        }));
        transform(session, anchor, format!("qualification-{turn}"), messages)
    };
    if case.shape != Shape::LargestAdmitted {
        return build(None);
    }
    let base = serde_json::to_vec(&build(Some(""))).map_or(0, |bytes| bytes.len());
    let room = daemon::MAX_TRANSFORM_FRAME_BYTES - base - LARGEST_HEADROOM_BYTES;
    build(Some(&filler()[..room]))
}

/// The seeding pass streams this file from disk, so no generator state stays resident.
fn write_generator(path: &Path, messages: u64) -> io::Result<()> {
    let mut out = BufWriter::with_capacity(1024 * 1024, fs::File::create(path)?);
    for ordinal in 1..=messages {
        out.write_all(br#"{"ck":{"content":[{"kind":{"text":""#)?;
        out.write_all(message_text(QUALIFICATION_SEED, ordinal).as_bytes())?;
        out.write_all(br#"","type":"text"}}"#)?;
        if ordinal.is_multiple_of(16) {
            write!(
                out,
                r#",{{"kind":{{"id":"call-{ordinal}","input":{{"path":"src/lib.rs"}},"name":"read","type":"tool_call"}}}},{{"kind":{{"id":"call-{ordinal}","output":{{"kind":{{"text":""#,
            )?;
            out.write_all(message_text(7, ordinal).as_bytes())?;
            out.write_all(br#"","type":"text"}},"tool_name":"read","type":"tool_result"}}"#)?;
        }
        let role = if ordinal % 2 == 1 {
            "user"
        } else {
            "assistant"
        };
        writeln!(
            out,
            r#"],"meta":{{"harness_id":"m{ordinal}"}},"role":"{role}"}},"mid":"m{ordinal}","ordinal":{ordinal}}}"#,
        )?;
    }
    out.flush()
}

fn transcript_line(line: &str) -> io::Result<String> {
    let message: Value = serde_json::from_str(line)?;
    let role = message["ck"]["role"].as_str().unwrap_or("?");
    let mut text = format!(
        "{}:",
        role.chars().next().unwrap_or('?').to_ascii_uppercase()
    );
    for part in message["ck"]["content"].as_array().into_iter().flatten() {
        if let Some(body) = part["kind"]["text"].as_str() {
            text.push_str(body);
        }
    }
    text.push('\n');
    Ok(text)
}

fn deflate(text: &str) -> io::Result<Vec<u8>> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(text.as_bytes())?;
    encoder.finish()
}

/// Each segment's deflated transcript, streamed from `generator` in sequence order.
fn transcripts(
    generator: &Path,
    segments: usize,
    span: usize,
) -> io::Result<impl Iterator<Item = io::Result<Vec<u8>>>> {
    let mut lines = io::BufReader::new(fs::File::open(generator)?).lines();
    Ok((0..segments).map(move |_| {
        let mut transcript = String::new();
        for _ in 0..span {
            let line = lines
                .next()
                .unwrap_or_else(|| Err(io::ErrorKind::UnexpectedEof.into()))?;
            transcript.push_str(&transcript_line(&line)?);
        }
        deflate(&transcript)
    }))
}

fn open_store(root: &Path) -> io::Result<MemoryStore> {
    let descriptor = daemon::managed_store_descriptor(root).map_err(io::Error::other)?;
    MemoryStore::open(&descriptor).map_err(io::Error::other)
}

/// Seeds `segments` folded rows and the deflated raw transcripts of the newest rows that fit
/// the store's per-session transcript cap, as its own oldest-first eviction would leave them.
fn seed(
    store: &MemoryStore,
    session: &str,
    segments: usize,
    generator: &Path,
) -> io::Result<Anchor> {
    let mut history = SyntheticHistory::mixed(segments);
    history.seed = QUALIFICATION_SEED;
    history.seed(store, session);
    let span = history.span;
    let mut kept: VecDeque<(i64, Vec<u8>)> = VecDeque::new();
    let mut bytes = 0;
    for (index, blob) in transcripts(generator, segments, span as usize)?.enumerate() {
        let blob = blob?;
        bytes += blob.len() as u64;
        kept.push_back((index as i64 + 1, blob));
        while bytes > RAW_CAP_BYTES {
            bytes -= kept
                .pop_front()
                .map_or(0, |(_, oldest)| oldest.len() as u64);
        }
    }
    store
        .with_fenced_conn_for_test(|tx| {
            let mut insert = tx.prepare_cached(
                "INSERT INTO chunk_transcripts
                   (session_id, history_segment_seq, start_ordinal, end_ordinal,
                    transcript_deflate, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0)",
            )?;
            for (sequence, blob) in &kept {
                let end = span * sequence;
                insert.execute(params![session, sequence, end - span + 1, end, blob])?;
            }
            Ok(())
        })
        .map_err(io::Error::other)?;
    let end = span * segments as i64;
    commit_anchor(store, session, end)?;
    Ok(Anchor {
        end: end as u64,
        sequence: segments as u64,
    })
}

fn observe(
    store: &MemoryStore,
    session: &str,
    after_sequence: u64,
) -> io::Result<(StoreObservation, Vec<Publication>)> {
    let ordinal = |value: i64| u64::try_from(value).unwrap_or(0);
    let (raw, publications) = store
        .with_fenced_conn_for_test(|tx| {
            let raw = tx
                .prepare(
                    "SELECT start_ordinal, end_ordinal, transcript_bytes FROM chunk_transcripts
                      WHERE session_id = ?1",
                )?
                .query_map(params![session], |r| {
                    Ok(RawRange {
                        start: ordinal(r.get(0)?),
                        end: ordinal(r.get(1)?),
                        bytes: ordinal(r.get(2)?),
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let publications = tx
                .prepare(
                    "SELECT sequence, start_message, end_message FROM history_segments
                      WHERE session_id = ?1 AND sequence > ?2",
                )?
                .query_map(params![session, after_sequence as i64], |r| {
                    Ok(Publication {
                        lineage: format!("segment-{}", r.get::<_, i64>(0)?),
                        start: ordinal(r.get(1)?),
                        end: ordinal(r.get(2)?),
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((raw, publications))
        })
        .map_err(io::Error::other)?;
    let covered = store
        .load(session)
        .map_err(io::Error::other)?
        .meta
        .coverage_ordinal
        .unwrap_or(0);
    Ok((StoreObservation { raw, covered }, publications))
}

/// Every request waits out the daemon's work, so no request is still running when its route
/// closes.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);

async fn request(client: &Client, route: ClientRoute, body: &Value) -> Result<Value, CallError> {
    let bytes = serde_json::to_vec(body).expect("request serializes");
    let options = RequestOptions {
        timeout: REQUEST_TIMEOUT,
        ..RequestOptions::default()
    };
    let response = client.request(route, bytes, options).await?;
    Ok(serde_json::from_slice(&response.body).unwrap_or(Value::Null))
}

fn launch(root: &Path, latency_ms: Option<u64>) -> FixtureProcess {
    let home = root.join("config-home");
    fs::create_dir_all(home.join("eidnara")).expect("config home");
    fs::write(
        home.join("eidnara/eidnara.jsonc"),
        r#"{ "history_summarizer": { "model": "fixture/summarizer", "context_limit_tokens": 128000 } }"#,
    )
    .expect("user tier");
    let mut launch = Launch::at(root.to_path_buf()).config_home(&home);
    if let Some(ms) = latency_ms {
        launch = launch.env("EIDNARA_FIXTURE_SUMMARIZER_LATENCY_MS", &ms.to_string());
    }
    launch.start()
}

async fn open(fixture: &FixtureProcess, client: &Client, session: &str) -> ClientRoute {
    let route = fixture
        .open_route(client, "context", TargetKind::ToolProvider, session)
        .await;
    wait_for_store(client, route, session).await;
    route
}

#[derive(Debug, Clone, Copy)]
struct ProcessCounters {
    read_bytes: u64,
    minor_faults: u64,
    peak_rss_bytes: u64,
}

fn counters(pid: u32) -> io::Result<ProcessCounters> {
    let missing = |what: &str| io::Error::other(format!("/proc/{pid} has no readable {what}"));
    let io_text = fs::read_to_string(format!("/proc/{pid}/io"))?;
    let read_bytes = io_text
        .lines()
        .find_map(|line| line.strip_prefix("rchar: "))
        .and_then(|value| value.trim().parse().ok())
        .ok_or_else(|| missing("rchar"))?;
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let minor_faults = stat
        .rsplit_once(") ")
        .and_then(|(_, rest)| rest.split(' ').nth(7))
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| missing("minflt"))?;
    let status = fs::read_to_string(format!("/proc/{pid}/status"))?;
    let peak_rss_kib: u64 = status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .and_then(|value| value.trim().trim_end_matches("kB").trim().parse().ok())
        .ok_or_else(|| missing("VmHWM"))?;
    Ok(ProcessCounters {
        read_bytes,
        minor_faults,
        peak_rss_bytes: peak_rss_kib * 1024,
    })
}

struct Interactive {
    cold_open_us: u64,
    observations: Vec<(u64, bool)>,
    start: ProcessCounters,
    end: ProcessCounters,
}

impl Interactive {
    /// Bytes read over the cold open, then over the interactive phase. The fixture launches
    /// inside the cold-open interval, so the cold share is every byte it read before `start`.
    fn read_phases(&self) -> (u64, u64) {
        (
            self.start.read_bytes,
            self.end.read_bytes.saturating_sub(self.start.read_bytes),
        )
    }
}

async fn interactive(
    fixture: &FixtureProcess,
    case: &Case,
    sessions: &[String],
    anchors: &[Anchor],
    operations: u64,
    cold: Instant,
) -> io::Result<Interactive> {
    let pid = fixture.pid();
    let client = fixture.client().await;
    let mut routes = Vec::with_capacity(sessions.len());
    for session in sessions {
        routes.push(open(fixture, &client, session).await);
    }
    // The cold-open measurements include each session's first window request.
    for (pick, session) in sessions.iter().enumerate() {
        let body = window_request(case, session, anchors[pick], 0);
        let response = request(&client, routes[pick], &body).await;
        if !response.as_ref().is_ok_and(|r| r["status"] == "ok") {
            return Err(io::Error::other(format!(
                "{session}: the cold pass failed: {response:?}"
            )));
        }
    }
    let cold_open_us = cold.elapsed().as_micros() as u64;
    let start = counters(pid)?;
    let mut observations = Vec::with_capacity(operations as usize);
    for turn in 0..operations {
        let pick = (turn % sessions.len() as u64) as usize;
        let body = window_request(case, &sessions[pick], anchors[pick], turn + 1);
        let started = Instant::now();
        let response = request(&client, routes[pick], &body).await;
        let micros = started.elapsed().as_micros() as u64;
        let ok = response.as_ref().is_ok_and(|r| r["status"] == "ok");
        observations.push((micros, !ok));
    }
    let end = counters(pid)?;
    let _ = client.close().await;
    Ok(Interactive {
        cold_open_us,
        observations,
        start,
        end,
    })
}

fn repetition(
    case: &Case,
    (index, position): (u32, u32),
    operations: u64,
    generator: &Path,
    parent: &Path,
) -> io::Result<Repetition> {
    let dir = tempfile::tempdir_in(parent)?;
    let root = dir.path().join("state");
    fs::create_dir_all(root.join("project"))?;
    let segments = (case.retained / 2) as usize;
    let sessions: Vec<String> = (0..case.sessions)
        .map(|n| format!("{}-{n}", case.name))
        .collect();
    let seeding = Instant::now();
    let mut anchors = Vec::new();
    let mut before = Vec::new();
    let seed_us;
    {
        let store = open_store(&root)?;
        for session in &sessions {
            anchors.push(seed(&store, session, segments, generator)?);
        }
        seed_us = seeding.elapsed().as_micros() as u64;
        for session in &sessions {
            before.push(observe(&store, session, segments as u64)?.0);
        }
    }
    let ingested_bytes = dir_bytes(&root);
    let cold = Instant::now();
    let fixture = launch(&root, None);
    let measured = block_on(interactive(
        &fixture, case, &sessions, &anchors, operations, cold,
    ));
    fixture.shutdown();
    let measured = measured?;
    let (cold_read_bytes, read_bytes) = measured.read_phases();
    let store = open_store(&root)?;
    let mut lineage_violations = Vec::new();
    for (session, before) in sessions.iter().zip(&before) {
        let (after, publications) = observe(&store, session, segments as u64)?;
        lineage_violations.extend(audit_lineage(before, &after, &publications, RAW_CAP_BYTES));
    }
    let latency_us = Histogram::of(&measured.observations);
    Ok(Repetition {
        index,
        position,
        seed_us,
        ingested_bytes,
        cold_open_us: measured.cold_open_us,
        cold_read_bytes,
        operations,
        failed_operations: latency_us.censored,
        latency_us,
        read_bytes,
        minor_faults: measured
            .end
            .minor_faults
            .saturating_sub(measured.start.minor_faults),
        peak_rss_bytes: measured.end.peak_rss_bytes,
        lineage_violations,
    })
}

fn dir_bytes(path: &Path) -> u64 {
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| match entry.metadata() {
            Ok(meta) if meta.is_dir() => dir_bytes(&entry.path()),
            Ok(meta) => meta.len(),
            Err(_) => 0,
        })
        .sum()
}

const OUTAGE_PASS_MESSAGES: u64 = 16_000;

/// One outage-run pass: the window from the current anchor through the newest arrived
/// message, with context pressure high enough to fold.
fn arrival_request(session: &str, anchor: Anchor, newest: u64, turn: u64) -> Value {
    let last = newest.min(anchor.end + OUTAGE_PASS_MESSAGES);
    let messages: Vec<Value> = (anchor.end..=last)
        .map(|ordinal| window_message(Shape::Mixed, ordinal))
        .collect();
    let mut request = transform(session, anchor, format!("outage-{turn}"), messages);
    request["usage"] =
        json!({"current_total_input_tokens": 120_000, "context_limit_tokens": 128_000});
    request
}

fn rendered_anchor(response: &Value) -> Option<Anchor> {
    let boundary = response.get("boundary")?;
    let end = boundary["mid"].as_str()?.strip_prefix('m')?.parse().ok()?;
    Some(Anchor {
        end,
        sequence: boundary["sequence"].as_u64()?,
    })
}

/// Calibration keeps one active window of unfolded messages ahead of the anchor. The run
/// clock, not the loop count, places each pass in its phase and sets the arrived volume.
fn outage(scale: u64, parent: &Path) -> io::Result<OutageRun> {
    let schedule = OutageSchedule::scaled(scale);
    let dir = tempfile::tempdir_in(parent)?;
    let root = dir.path().join("state");
    fs::create_dir_all(root.join("project"))?;
    let session = "outage";
    let segments = 5_000;
    let generator = dir.path().join("generator.jsonl");
    write_generator(&generator, 2 * segments as u64)?;
    let (seeded, before) = {
        let store = open_store(&root)?;
        let seeded = seed(&store, session, segments, &generator)?;
        (seeded, observe(&store, session, seeded.sequence)?.0)
    };
    let fixture = launch(&root, Some(FAKE_MODEL_LATENCY_MS));
    let measured = block_on(async {
        let client = fixture.client().await;
        let route = open(&fixture, &client, session).await;
        let tokens = MESSAGE_TOKENS as u64;
        let mut anchor = seeded;
        let mut turn = 0u64;
        let pass = async |anchor: &mut Anchor, newest: u64, turn: u64| {
            let body = arrival_request(session, *anchor, newest, turn);
            let Ok(response) = request(&client, route, &body).await else {
                return false;
            };
            if let Some(next) = rendered_anchor(&response)
                && next.end > anchor.end
            {
                *anchor = next;
            }
            response["status"] == "ok"
        };
        let mut failed_passes = 0u64;
        let mut newest = anchor.end;
        let calibration = paced(schedule.steady_seconds.max(10), async || {
            turn += 1;
            newest = newest.max(anchor.end + ACTIVE_WINDOW);
            failed_passes += u64::from(!pass(&mut anchor, newest, turn).await);
        })
        .await;
        let elapsed_ms = calibration.as_millis().max(1) as u64;
        let fold_rate = (anchor.end - seeded.end) * tokens * 1000 / elapsed_ms;
        let arrival_rate = eval_core::frozen_arrival(fold_rate);
        let arrival_base = newest;
        let mut samples = Vec::new();
        let mut current = None;
        let clock = Instant::now();
        loop {
            let at = clock.elapsed();
            let second = at.as_secs();
            let Some(phase) = schedule.phase_at(second) else {
                break;
            };
            if current != Some(phase) {
                if phase == Phase::Outage {
                    acknowledged(&fixture, 71, "outage-begin")?;
                } else if current == Some(Phase::Outage) {
                    acknowledged(&fixture, 72, "outage-end")?;
                }
                current = Some(phase);
            }
            let arrived = arrival_rate * at.as_millis() as u64 / 1000;
            newest = newest.max(arrival_base + arrived / tokens);
            turn += 1;
            let ok = pass(&mut anchor, newest, turn).await;
            failed_passes += u64::from(!ok && phase != Phase::Outage);
            samples.push(BacklogSample {
                second,
                elapsed_ms: clock.elapsed().as_millis() as u64,
                tokens: (newest - anchor.end) * tokens,
            });
            next_second(clock, second).await;
        }
        let _ = client.close().await;
        io::Result::Ok((fold_rate, samples, failed_passes))
    });
    fixture.shutdown();
    let (fold_rate, samples, failed_passes) = measured?;
    let store = open_store(&root)?;
    let (after, publications) = observe(&store, session, seeded.sequence)?;
    Ok(OutageRun::new(
        schedule,
        fold_rate,
        samples,
        failed_passes,
        audit_lineage(&before, &after, &publications, RAW_CAP_BYTES),
    ))
}

async fn paced(seconds: u64, mut pass: impl AsyncFnMut()) -> Duration {
    let clock = Instant::now();
    loop {
        let second = clock.elapsed().as_secs();
        if second >= seconds {
            return clock.elapsed();
        }
        pass().await;
        next_second(clock, second).await;
    }
}

/// Completes at the start of the run clock's next second, or immediately when the pass has
/// already reached that deadline.
async fn next_second(clock: Instant, second: u64) {
    let target = clock + Duration::from_secs(second + 1);
    tokio::time::sleep_until(tokio::time::Instant::from_std(target)).await;
}

fn acknowledged(fixture: &FixtureProcess, id: u64, command: &str) -> io::Result<()> {
    let ack = fixture.control(id, command);
    if ack["result"]["accepted"] == true {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "fixture refused {command}: {ack}"
        )))
    }
}

fn filesystem_name(path: &Path) -> io::Result<String> {
    let magic = rustix::fs::statfs(path)?.f_type as u64;
    Ok(match magic {
        0x5846_5342 => "xfs".into(),
        0x0102_1994 => "tmpfs".into(),
        0x8584_58f6 => "ramfs".into(),
        0xef53 => "ext4".into(),
        0x9123_683e => "btrfs".into(),
        other => format!("0x{other:x}"),
    })
}

/// `ssd` or `hdd` from the rotational flag of the block device behind `path`, or of the
/// whole disk when that device is a partition.
fn state_disk(path: &Path) -> io::Result<String> {
    let dev = rustix::fs::stat(path)?.st_dev;
    let base = format!(
        "/sys/dev/block/{}:{}",
        rustix::fs::major(dev),
        rustix::fs::minor(dev)
    );
    let flag = ["queue/rotational", "../queue/rotational"]
        .iter()
        .find_map(|tail| fs::read_to_string(format!("{base}/{tail}")).ok());
    Ok(match flag.as_deref().map(str::trim) {
        Some("0") => "ssd".into(),
        Some("1") => "hdd".into(),
        _ => "unknown".into(),
    })
}

/// A fixture launched on an empty root reports its build and model workers.
fn fixture_report(parent: &Path) -> io::Result<(bool, u32)> {
    let dir = tempfile::tempdir_in(parent)?;
    let root = dir.path().join("state");
    fs::create_dir_all(root.join("project"))?;
    let fixture = launch(&root, None);
    let readiness = fixture.readiness().clone();
    fixture.shutdown();
    let debug = readiness["debug_assertions"].as_bool();
    let workers = readiness["model_workers"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok());
    debug
        .zip(workers)
        .ok_or_else(|| io::Error::other("fixture readiness omits its build or model workers"))
}

fn environment(mut host: HostManifest, state_dir: &Path) -> io::Result<Environment> {
    let effective_cpus = std::thread::available_parallelism().map_or(0, |n| n.get() as u32);
    let cgroup = fs::read_to_string("/proc/self/cgroup")
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|l| l.strip_prefix("0::").map(str::to_owned))
        });
    let limit = cgroup
        .and_then(|path| fs::read_to_string(format!("/sys/fs/cgroup{path}/memory.max")).ok())
        .and_then(|value| value.trim().parse::<u64>().ok());
    let effective_memory_bytes = limit.unwrap_or(host.memory_bytes).min(host.memory_bytes);
    let constrained_by_cgroup =
        effective_cpus < host.core_count || effective_memory_bytes < host.memory_bytes;
    host.disk = state_disk(state_dir)?;
    let (fixture_debug, model_workers) = fixture_report(state_dir)?;
    let target_env = if cfg!(target_env = "gnu") {
        "gnu"
    } else {
        "other"
    };
    Ok(Environment {
        host,
        target: format!(
            "{}-unknown-{}-{target_env}",
            std::env::consts::ARCH,
            std::env::consts::OS
        ),
        effective_cpus,
        effective_memory_bytes,
        constrained_by_cgroup,
        state_filesystem: filesystem_name(state_dir)?,
        fixture_debug,
        fixture_sha256: sha256_hex(&fs::read(fixture_binary())?),
        model_workers,
    })
}

/// `HEAD`, marked `-dirty` when any tracked or untracked file differs from it.
fn source_commit() -> String {
    let head = command("git", &["rev-parse", "HEAD"]);
    if command("git", &["status", "--porcelain"]).is_empty() {
        head
    } else {
        format!("{head}{DIRTY_SOURCE_SUFFIX}")
    }
}

fn host_manifest() -> HostManifest {
    let cpuinfo = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let cpu_model = cpuinfo
        .lines()
        .find_map(|l| {
            l.strip_prefix("model name")
                .and_then(|r| r.split(':').nth(1))
        })
        .map_or("unknown".into(), |m| m.trim().to_owned());
    let core_count = cpuinfo
        .lines()
        .filter(|l| l.starts_with("processor"))
        .count() as u32;
    let memory_bytes = fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|t| {
            t.lines()
                .find_map(|l| l.strip_prefix("MemTotal:"))
                .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
        })
        .map_or(0, |kib| kib * 1024);
    let kernel = fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    let glibc = std::process::Command::new("ldd")
        .arg("--version")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .and_then(|text| text.lines().next().map(str::to_owned))
        .unwrap_or_else(|| "unknown".into());
    HostManifest {
        cpu_model,
        core_count,
        memory_bytes,
        kernel: kernel.trim().to_owned(),
        glibc,
        disk: "unknown".into(),
    }
}

pub struct Config {
    pub out: PathBuf,
    pub cases: Vec<Case>,
    pub operations: u64,
    pub repetitions: u32,
    pub outage_scale: u64,
    pub state_dir: PathBuf,
    /// The witness runs the operator recorded, carried into the report unchanged.
    pub witness_runs: BTreeMap<String, WitnessRun>,
}

/// Reads the optional `--witness-runs` file: a JSON object from each witness name to the test that witnessed it and that test's exit code.
fn witness_runs(path: Option<&str>) -> Result<BTreeMap<String, WitnessRun>, String> {
    let Some(path) = path else {
        return Ok(BTreeMap::new());
    };
    let read = || {
        let bytes = fs::read(path).map_err(|error| error.to_string())?;
        let runs: BTreeMap<String, WitnessRun> =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        check_witness_runs(&runs)?;
        Ok(runs)
    };
    read().map_err(|error: String| format!("--witness-runs {path}: {error}"))
}

pub fn config_from_args(args: impl IntoIterator<Item = String>) -> Result<Config, String> {
    let flags = [
        "out",
        "cases",
        "operations",
        "repetitions",
        "outage-scale",
        "state-dir",
    ];
    let values = parse_flags_with_optional(args, &flags, &["witness-runs"], USAGE)?;
    let number = |name: &str| {
        values[name]
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or(format!("--{name} must be a positive count"))
    };
    let all = catalog();
    let cases = if values["cases"] == "all" {
        all
    } else {
        let names: Vec<&str> = values["cases"].split(',').collect();
        let cases: Vec<Case> = all
            .into_iter()
            .filter(|c| names.contains(&c.name.as_str()))
            .collect();
        if cases.len() != names.len() {
            return Err("--cases names a case outside the catalog".into());
        }
        cases
    };
    Ok(Config {
        out: PathBuf::from(&values["out"]),
        cases,
        operations: number("operations")?,
        repetitions: u32::try_from(number("repetitions")?).map_err(|e| e.to_string())?,
        outage_scale: number("outage-scale")?,
        state_dir: PathBuf::from(&values["state-dir"]),
        witness_runs: witness_runs(values.get("witness-runs").map(String::as_str))?,
    })
}

fn guarded<T>(work: impl FnOnce() -> io::Result<T>) -> Result<T, String> {
    match catch_unwind(AssertUnwindSafe(work)) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(error.to_string()),
        Err(panic) => Err(panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_else(|| "panic".into())),
    }
}

/// Round-robin repetitions spread campaign drift across cases.
pub fn run(config: &Config) -> io::Result<QualificationReport> {
    fs::create_dir_all(&config.state_dir)?;
    let source_commit = source_commit();
    let tokenizer = format!("vocab-sha256:{}", sha256_hex(tokenizer::vocab_blob()));
    let environment = environment(host_manifest(), &config.state_dir)?;
    let generators = tempfile::tempdir_in(&config.state_dir)?;
    let mut results: Vec<(CaseResult, Option<PathBuf>)> = Vec::new();
    for case in &config.cases {
        let path = generators.path().join(format!("{}.jsonl", case.name));
        let mut result = CaseResult {
            case: case.clone(),
            repetitions: Vec::new(),
            failed_repetitions: Vec::new(),
        };
        let generator = match guarded(|| write_generator(&path, case.retained)) {
            Ok(()) => Some(path),
            Err(error) => {
                result
                    .failed_repetitions
                    .push(format!("generator: {error}"));
                None
            }
        };
        results.push((result, generator));
    }
    let cases = results.len();
    for index in 0..config.repetitions {
        for position in 0..cases {
            let (result, generator) = &mut results[(position + index as usize) % cases];
            let Some(generator) = generator else { continue };
            let case = &result.case;
            let slot = (index, position as u32);
            match guarded(|| {
                repetition(case, slot, config.operations, generator, &config.state_dir)
            }) {
                Ok(repetition) => result.repetitions.push(repetition),
                Err(error) => {
                    eprintln!("qualification: {} repetition {index}: {error}", case.name);
                    result
                        .failed_repetitions
                        .push(format!("repetition {index} position {position}: {error}"));
                }
            }
        }
        eprintln!("qualification: repetition {index} done");
    }
    let outage = guarded(|| outage(config.outage_scale, &config.state_dir));
    let report = QualificationReport::build(
        source_commit,
        tokenizer,
        environment,
        results.into_iter().map(|(result, _)| result).collect(),
        outage,
        config.witness_runs.clone(),
    );
    fs::write(&config.out, serde_json::to_vec_pretty(&report)?)?;
    Ok(report)
}

#[cfg(test)]
mod tests {

    use super::*;
    use eval_core::LineageViolation;

    /// SHA-256 of the first 64 generator messages under seed 702.
    const GENERATOR_64_SHA256: &str =
        "f90f928c23bcdd86c4b6f6862d80490e1d0ff6f5f9999b0f7e94f386dda2a3ea";

    fn args(extra: &[&str]) -> Vec<String> {
        [
            "--out",
            "report.json",
            "--cases",
            "all",
            "--operations",
            "10",
            "--repetitions",
            "5",
            "--outage-scale",
            "1",
            "--state-dir",
            "state",
        ]
        .iter()
        .chain(extra)
        .map(|arg| (*arg).to_owned())
        .collect()
    }

    #[test]
    fn witness_runs_are_read_from_the_named_file_and_validated() {
        assert!(config_from_args(args(&[])).unwrap().witness_runs.is_empty());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("witnesses.json");
        let all: BTreeMap<String, WitnessRun> = eval_core::WITNESSES
            .iter()
            .map(|name| {
                (
                    (*name).to_owned(),
                    WitnessRun {
                        test: format!("daemon --lib {name}"),
                        source_commit: "c".into(),
                        exit_code: 0,
                    },
                )
            })
            .collect();
        fs::write(&path, serde_json::to_vec(&all).unwrap()).unwrap();
        let file = path.to_str().unwrap();
        assert_eq!(
            config_from_args(args(&["--witness-runs", file]))
                .unwrap()
                .witness_runs,
            all
        );
        let mut unknown = all.clone();
        unknown.insert(
            "warm_soak".into(),
            WitnessRun {
                test: "t".into(),
                source_commit: "c".into(),
                exit_code: 0,
            },
        );
        let refusal = |extra: &[&str]| config_from_args(args(extra)).err().unwrap_or_default();
        fs::write(&path, serde_json::to_vec(&unknown).unwrap()).unwrap();
        assert!(refusal(&["--witness-runs", file]).contains("is outside"));
        fs::write(
            &path,
            br#"{"normal_fold": {"test": "t", "source_commit": "c"}}"#,
        )
        .unwrap();
        assert!(
            refusal(&["--witness-runs", file]).contains("exit_code"),
            "a run without its exit code is refused"
        );
        fs::write(
            &path,
            format!(
                r#"{{"normal_fold": {{"test": "t", "source_commit": "c{DIRTY_SOURCE_SUFFIX}", "exit_code": 0}}}}"#
            ),
        )
        .unwrap();
        assert!(
            refusal(&["--witness-runs", file]).contains("clean source commit"),
            "a run on a dirty tree is refused"
        );
        assert!(refusal(&["--witness-runs"]).contains("needs a value"));
        assert!(refusal(&["--witness-runs", "--out", "x"]).contains("needs a value"));
        assert!(refusal(&["--witness-runs", file, "--witness-runs", file]).contains("given twice"));
    }

    /// Seed 702 makes the streamed generator byte-identical across runs; the pin catches any change to the fixture text, roles, or tool cadence.
    #[test]
    fn the_seeded_generator_is_reproducible_and_pinned() {
        let dir = tempfile::tempdir().unwrap();
        let (first, second) = (dir.path().join("a.jsonl"), dir.path().join("b.jsonl"));
        write_generator(&first, 64).unwrap();
        write_generator(&second, 64).unwrap();
        let bytes = fs::read(&first).unwrap();
        assert_eq!(bytes, fs::read(&second).unwrap());
        assert_eq!(sha256_hex(&bytes), GENERATOR_64_SHA256);
    }

    #[test]
    fn seeded_generator_matches_window_messages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("messages.jsonl");
        write_generator(&path, 4096).unwrap();
        let lines: Vec<String> = io::BufReader::new(fs::File::open(&path).unwrap())
            .lines()
            .collect::<io::Result<_>>()
            .unwrap();
        assert_eq!(lines.len(), 4096);
        for (index, line) in lines.iter().enumerate() {
            assert_eq!(
                *line,
                serde_json::to_string(&window_message(Shape::Mixed, index as u64 + 1)).unwrap(),
                "ordinal {}",
                index + 1,
            );
        }
        write_generator(&path, 0).unwrap();
        assert!(fs::read(&path).unwrap().is_empty());
        assert_eq!(
            write_generator(&dir.path().join("missing/file"), 1)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound,
        );
        #[cfg(target_os = "linux")]
        assert!(write_generator(Path::new("/dev/full"), 64).is_err());
    }

    #[test]
    fn seeded_generator_text_matches_original_word_stream() {
        for seed in [0, 7, QUALIFICATION_SEED, u64::MAX] {
            for ordinal in (0..=256).chain([999, 1_000_000, u64::MAX]) {
                let mut state = (seed ^ ordinal.wrapping_mul(0x9E37_79B9_7F4A_7C15)) | 1;
                let mut expected = String::new();
                for _ in 0..MESSAGE_TOKENS {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    expected.push_str(WORDS[(state % WORDS.len() as u64) as usize]);
                }
                assert_eq!(message_text(seed, ordinal), expected, "{seed}:{ordinal}");
            }
        }
    }

    #[test]
    fn ordinary_messages_hold_exactly_the_measured_token_count() {
        for ordinal in [1, 2, 999, 1_000_000] {
            let text = message_text(QUALIFICATION_SEED, ordinal);
            assert_eq!(tokenizer::estimate_tokens(&text), MESSAGE_TOKENS);
            assert_eq!(text, message_text(QUALIFICATION_SEED, ordinal));
        }
        assert_ne!(
            message_text(QUALIFICATION_SEED, 1),
            message_text(QUALIFICATION_SEED, 2)
        );
    }

    #[test]
    fn the_largest_admitted_request_fills_the_transform_frame_cap() {
        let case = catalog()
            .into_iter()
            .find(|c| c.shape == Shape::LargestAdmitted)
            .unwrap();
        for end in [10, 99_999] {
            let anchor = Anchor { end, sequence: 5 };
            let bytes = serde_json::to_vec(&window_request(&case, "s", anchor, 7)).unwrap();
            assert!(bytes.len() <= daemon::MAX_TRANSFORM_FRAME_BYTES);
            assert!(bytes.len() + LARGEST_HEADROOM_BYTES >= daemon::MAX_TRANSFORM_FRAME_BYTES);
        }
    }

    #[test]
    fn an_outage_pass_fits_the_transform_frame_cap_for_any_backlog() {
        let anchor = Anchor {
            end: 99_999_000,
            sequence: 5_000,
        };
        for backlog in [ACTIVE_WINDOW, 17_856, 100_000] {
            let body = arrival_request("outage", anchor, anchor.end + backlog, 3);
            let carried = body["messages"].as_array().unwrap().len() as u64;
            assert_eq!(carried, backlog.min(OUTAGE_PASS_MESSAGES) + 1);
            let bytes = serde_json::to_vec(&body).unwrap();
            assert!(
                bytes.len() <= daemon::MAX_TRANSFORM_FRAME_BYTES,
                "backlog {backlog}: {} bytes",
                bytes.len()
            );
        }
    }

    #[test]
    fn a_smoke_campaign_writes_a_report_that_does_not_qualify() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            out: dir.path().join("report.json"),
            cases: catalog().into_iter().take(1).collect(),
            operations: 3,
            repetitions: 1,
            outage_scale: 60,
            state_dir: dir.path().join("state"),
            witness_runs: BTreeMap::new(),
        };
        let report = run(&config).unwrap();
        assert!(!report.qualified, "a smoke run is never qualification");
        let written: QualificationReport =
            serde_json::from_slice(&fs::read(&config.out).unwrap()).unwrap();
        assert_eq!(written, report);
        assert_eq!(
            report.environment.fixture_debug,
            report
                .host_shortfalls
                .contains(&eval_core::HostShortfall::DebugBuild)
        );
        assert_eq!(report.environment.model_workers, 8);
        assert_eq!(report.environment.fixture_sha256.len(), 64);
        assert!(report.tokenizer.starts_with("vocab-sha256:"));
        let result = &report.cases[0];
        assert_eq!(result.failed_repetitions, Vec::<String>::new());
        assert_eq!(result.repetitions.len(), 1);
        assert_eq!(result.operations(), 3);
        let repetition = &result.repetitions[0];
        assert!(repetition.peak_rss_bytes > 0);
        assert!(repetition.cold_open_us > 0 && repetition.seed_us > 0);
        assert_eq!(repetition.lineage_violations, Vec::new());
        assert!(
            report.gates["retained_10k"].contains(&eval_core::GateFailure::Repetitions { kept: 1 })
        );
        let outage = report.outage.expect("outage run");
        assert!(!outage.samples.is_empty());
        assert!(outage.samples.len() as u64 <= outage.schedule.total_seconds());
        assert!(outage.fold_tokens_per_second > 0, "calibration folded");
        assert_eq!(outage.lineage_violations, Vec::new());
    }

    #[test]
    fn every_read_of_the_launched_daemon_lands_in_the_cold_open_or_the_interactive_phase() {
        const SEGMENTS: usize = 4;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("state");
        fs::create_dir_all(root.join("project")).unwrap();
        let generator = dir.path().join("generator.jsonl");
        write_generator(&generator, 2 * SEGMENTS as u64).unwrap();
        let sessions = ["phases".to_string()];
        let anchors = [seed(
            &open_store(&root).unwrap(),
            &sessions[0],
            SEGMENTS,
            &generator,
        )
        .unwrap()];
        let case = catalog().into_iter().next().unwrap();
        let cold = Instant::now();
        let fixture = launch(&root, None);
        let measured = block_on(interactive(&fixture, &case, &sessions, &anchors, 2, cold));
        fixture.shutdown();
        let measured = measured.unwrap();
        assert_eq!(
            measured
                .observations
                .iter()
                .filter(|(_, failed)| *failed)
                .count(),
            0
        );
        let (cold_reads, interactive_reads) = measured.read_phases();
        assert!(interactive_reads > 0);
        assert_eq!(
            cold_reads + interactive_reads,
            measured.end.read_bytes,
            "{cold_reads} cold and {interactive_reads} interactive bytes"
        );
    }

    fn backend_counter(fixture: &FixtureProcess, name: &str) -> u64 {
        fixture.control(9, "counters")["result"][name]
            .as_u64()
            .expect("backend counter")
    }

    fn wait_for_counter(fixture: &FixtureProcess, name: &str, above: u64) -> u64 {
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let value = backend_counter(fixture, name);
            if value > above || Instant::now() > deadline {
                return value;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    #[test]
    fn the_outage_controls_fail_model_calls_and_restore_them() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("state");
        fs::create_dir_all(root.join("project")).unwrap();
        let generator = dir.path().join("generator.jsonl");
        write_generator(&generator, 100).unwrap();
        let anchor = {
            let store = open_store(&root).unwrap();
            seed(&store, "during", 50, &generator).unwrap();
            seed(&store, "after", 50, &generator).unwrap()
        };
        let fixture = launch(&root, None);
        block_on(async {
            let client = fixture.client().await;
            let fold = async |session: &str| {
                let route = open(&fixture, &client, session).await;
                let body = arrival_request(session, anchor, anchor.end + ACTIVE_WINDOW, 1);
                request(&client, route, &body).await.unwrap();
            };
            acknowledged(&fixture, 1, "outage-begin").unwrap();
            let failed = backend_counter(&fixture, "failed");
            fold("during").await;
            assert!(wait_for_counter(&fixture, "failed", failed) > failed);
            acknowledged(&fixture, 2, "outage-end").unwrap();
            let completed = backend_counter(&fixture, "completed");
            fold("after").await;
            assert!(wait_for_counter(&fixture, "completed", completed) > completed);
            let _ = client.close().await;
        });
        fixture.shutdown();
    }

    fn seeded_store(dir: &Path, segments: usize) -> (MemoryStore, StoreObservation, u64) {
        let root = dir.join("state");
        fs::create_dir_all(root.join("project")).unwrap();
        let generator = dir.join("generator.jsonl");
        write_generator(&generator, 2 * segments as u64).unwrap();
        let store = open_store(&root).unwrap();
        let anchor = seed(&store, "s", segments, &generator).unwrap();
        let (before, _) = observe(&store, "s", anchor.sequence).unwrap();
        (store, before, anchor.sequence)
    }

    fn audit_after(tamper: impl FnOnce(&MemoryStore)) -> Vec<LineageViolation> {
        let dir = tempfile::tempdir().unwrap();
        let (store, before, sequence) = seeded_store(dir.path(), 10);
        assert_eq!(before.raw.len(), 10);
        assert_eq!(before.covered, 20);
        tamper(&store);
        let (after, publications) = observe(&store, "s", sequence).unwrap();
        audit_lineage(&before, &after, &publications, RAW_CAP_BYTES)
    }

    fn sql(store: &MemoryStore, statement: &str) {
        store
            .with_fenced_conn_for_test(|tx| tx.execute(statement, []))
            .unwrap();
    }

    #[test]
    fn store_oracles_fire_on_tampered_history_and_stay_quiet_on_an_untouched_store() {
        assert_eq!(audit_after(|_| {}), Vec::new());
        assert_eq!(
            audit_after(|store| sql(
                store,
                "DELETE FROM chunk_transcripts WHERE session_id = 's' AND history_segment_seq = 3"
            )),
            vec![LineageViolation::RawHistoryLost { start: 5, end: 6 }]
        );
        assert_eq!(
            audit_after(|store| sql(
                store,
                "INSERT INTO history_segments
                   (session_id, sequence, start_message, end_message, title, content)
                 VALUES ('s', 11, 19, 20, 't', 'c')"
            )),
            vec![LineageViolation::DuplicatePublication {
                lineage: "segment-11".into(),
                start: 19,
                end: 20,
            }]
        );
        assert_eq!(
            audit_after(|store| sql(
                store,
                "INSERT INTO history_segments
                   (session_id, sequence, start_message, end_message, title, content)
                 VALUES ('s', 11, 21, 22, 't', 'c')"
            )),
            vec![LineageViolation::RawHistoryLost { start: 21, end: 22 }],
            "a publication whose transcript is gone while older ones remain lost raw history"
        );
        assert_eq!(
            audit_after(|store| {
                let loaded = store.load("s").unwrap();
                let mut meta = loaded.meta.clone();
                meta.coverage_ordinal = Some(40);
                store
                    .commit("s", loaded.row_version, &loaded.core, &meta)
                    .unwrap();
            }),
            vec![LineageViolation::FalseProgress {
                covered: 40,
                published: 20,
            }]
        );
    }

    #[test]
    fn seeded_transcripts_hold_the_streamed_raw_messages() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _, _) = seeded_store(dir.path(), 2);
        let blob: Vec<u8> = store
            .with_fenced_conn_for_test(|tx| {
                tx.query_row(
                    "SELECT transcript_deflate FROM chunk_transcripts
                      WHERE session_id = 's' AND history_segment_seq = 2",
                    [],
                    |r| r.get(0),
                )
            })
            .unwrap();
        let mut text = String::new();
        io::Read::read_to_string(&mut flate2::read::DeflateDecoder::new(&blob[..]), &mut text)
            .unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("U:") && lines[1].starts_with("A:"));
        assert!(lines[0].contains(&message_text(QUALIFICATION_SEED, 3)));
        assert!(lines[1].contains(&message_text(QUALIFICATION_SEED, 4)));
    }

    #[test]
    fn seeding_past_the_transcript_cap_keeps_only_the_newest_transcripts() {
        let dir = tempfile::tempdir().unwrap();
        let segments = 20_000;
        let (_store, before, _) = seeded_store(dir.path(), segments);
        let bytes: u64 = before.raw.iter().map(|r| r.bytes).sum();
        assert!(bytes <= RAW_CAP_BYTES);
        assert!(
            before.raw.len() < segments,
            "older transcripts were left out"
        );
        let newest = before.raw.iter().map(|r| r.end).max();
        assert_eq!(newest, Some(2 * segments as u64));
        let oldest = before.raw.iter().map(|r| r.start).min().unwrap();
        assert_eq!(
            before.raw.len() as u64,
            (2 * segments as u64 - oldest).div_ceil(2)
        );
    }

    #[test]
    fn calibration_drives_one_pass_per_second_like_the_measured_phases() {
        let mut passes = 0u64;
        let elapsed = block_on(paced(2, async || passes += 1));
        assert_eq!(passes, 2, "one pass starts in each second of the window");
        assert!(elapsed >= Duration::from_secs(2));
        let mut slow = 0u64;
        block_on(paced(2, async || {
            slow += 1;
            tokio::time::sleep(Duration::from_millis(1_200)).await;
        }));
        assert_eq!(
            slow, 2,
            "a pass that overruns its second starts the next at once"
        );
    }

    #[test]
    fn shapes_change_roles_tools_and_ordinals() {
        let roles = |shape| {
            (1..=8u64)
                .map(|o| {
                    window_message(shape, o)["ck"]["role"]
                        .as_str()
                        .unwrap()
                        .to_owned()
                })
                .collect::<Vec<_>>()
        };
        assert!(roles(Shape::SameRole).iter().all(|r| r == "user"));
        assert!(roles(Shape::SystemOnly).iter().all(|r| r == "system"));
        let tool_arc = window_message(Shape::ToolArc, 2);
        assert_eq!(tool_arc["ck"]["content"].as_array().unwrap().len(), 3);
        let case = catalog()
            .into_iter()
            .find(|c| c.shape == Shape::Sparse)
            .unwrap();
        let sparse = window_request(
            &case,
            "s",
            Anchor {
                end: 10,
                sequence: 5,
            },
            0,
        );
        assert_eq!(sparse["messages"][1]["ordinal"], 13);
        assert_eq!(sparse["messages"].as_array().unwrap().len(), 301);
    }
}

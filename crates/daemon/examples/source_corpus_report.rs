//! Prints source-corpus measurements over the frozen corpus sessions: per harness and content
//! kind, capture completeness, loss by named condition, tool-result byte-length quantiles
//! stratified by the harness `truncated` flag, the share of units and bytes above the 64 KiB unit
//! cap, per-pass batch fill against the 32-unit and 64 KiB batch bounds, publish time against
//! admitted bytes, and the R1 cell census.
//!
//! Every figure describes the frozen corpus population, computed exhaustively. The exit status is
//! non-zero only when `unclassified_loss` is: an expected unit neither admitted nor lost under a
//! named condition. Every other figure only prints.
//!
//! Usage: `cargo run -p daemon --example source_corpus_report --features test-support -- [runs]`
//! (`runs`, default 5, is how many independent replays time the publish batches).

#[allow(dead_code)]
#[path = "../src/source_corpus.rs"]
mod source_corpus;

#[allow(dead_code)]
#[path = "../tests/support/corpus_replay.rs"]
mod corpus_replay;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::process::ExitCode;

use daemon::harness_sources::{
    Harness, MAX_SOURCE_BATCH_BYTES, MAX_SOURCE_BATCH_UNITS, MAX_SOURCE_UNIT_BYTES, SessionIdentity,
};
use serde_json::Value;

use corpus_replay::{
    PROJECT, PassMeasure, Reconciliation, ReplayRecord, ReplayStore, harness_adapter,
    opencode_records, pi_records, reconcile, replay,
};
use source_corpus::{ExpectedUnit, LossCondition, corpus_dir};

/// Passes a replay runs at most; a pass offers every unit not yet settled.
const PASSES: usize = 8;

struct Session {
    harness: Harness,
    closure: &'static str,
    id: String,
    records: Vec<ReplayRecord>,
    fixture: Value,
}

fn fixture(path: &str) -> Value {
    let bytes = std::fs::read(corpus_dir().join(path)).expect("fixture reads");
    serde_json::from_slice(&bytes).expect("fixture is JSON")
}

fn sessions() -> Vec<Session> {
    let opencode = fixture("sessions/opencode-1.18.22/mix.json");
    let pi = fixture("sessions/pi-0.80.2/mix.json");
    let (opencode_id, opencode_records) = opencode_records(&opencode);
    let (pi_id, pi_records) = pi_records(&pi);
    vec![
        Session {
            harness: Harness::OpenCode,
            closure: "OpenCode 1.18.22",
            id: opencode_id,
            records: opencode_records,
            fixture: opencode,
        },
        Session {
            harness: Harness::Pi,
            closure: "Pi 0.80.2",
            id: pi_id,
            records: pi_records,
            fixture: pi,
        },
    ]
}

/// The session's own id: the adapters refuse a record of another session, and every run
/// publishes into a fresh kernel.
fn identity(session: &Session) -> SessionIdentity {
    SessionIdentity {
        project_id: PROJECT.to_owned(),
        harness: session.harness,
        session_id: session.id.clone(),
    }
}

fn kind(unit: &ExpectedUnit) -> &'static str {
    if unit.tool.is_some() { "tool" } else { "text" }
}

fn of_kind(records: &[ReplayRecord], wanted: &str) -> Vec<ReplayRecord> {
    records
        .iter()
        .map(|record| {
            let mut expected = record.expected.clone();
            expected.units.retain(|unit| kind(unit) == wanted);
            ReplayRecord {
                native: record.native.clone(),
                expected,
            }
        })
        .collect()
}

fn ratio(numerator: usize, denominator: usize) -> String {
    if denominator == 0 {
        "n/a (0/0)".to_owned()
    } else {
        format!(
            "{:.4} ({numerator}/{denominator})",
            numerator as f64 / denominator as f64
        )
    }
}

/// Nearest-rank quantile `q` of the sorted `values`, with its 1-based order-statistic rank.
fn quantile(values: &[usize], q: f64) -> String {
    if values.is_empty() {
        return "n/a".to_owned();
    }
    let rank = ((q * values.len() as f64).ceil() as usize).clamp(1, values.len());
    format!("{} (rank {rank} of {})", values[rank - 1], values.len())
}

fn byte_lengths(out: &mut String, label: &str, mut values: Vec<usize>) {
    values.sort_unstable();
    let max = values
        .last()
        .map_or("n/a".to_owned(), |max| max.to_string());
    let _ = writeln!(
        out,
        "  {label}: n={} p50={} p90={} p99={} max={max}",
        values.len(),
        quantile(&values, 0.50),
        quantile(&values, 0.90),
        quantile(&values, 0.99),
    );
    let _ = writeln!(out, "    distribution: {values:?}");
}

fn losses(out: &mut String, result: &Reconciliation) {
    let named: Vec<String> = LossCondition::ALL
        .iter()
        .map(|condition| {
            format!(
                "{}={}",
                condition.name(),
                result.losses.get(condition).copied().unwrap_or(0)
            )
        })
        .collect();
    let _ = writeln!(out, "    loss by condition: {}", named.join(" "));
    let _ = writeln!(out, "    unclassified_loss={}", result.unclassified.len());
}

fn passes(out: &mut String, measures: &[PassMeasure]) {
    for (index, pass) in measures.iter().enumerate() {
        let _ = writeln!(
            out,
            "    pass {}: offered {} units / {} bytes; admitted {} units (bound \
             {MAX_SOURCE_BATCH_UNITS}) and {} bytes (bound {MAX_SOURCE_BATCH_BYTES})",
            index + 1,
            pass.offered_units,
            pass.offered_bytes,
            pass.admitted_units,
            pass.admitted_bytes,
        );
    }
}

const CELLS: [&str; 17] = [
    "read",
    "bash",
    "edit",
    "write",
    "grep",
    "glob",
    "todowrite",
    "MCP tool",
    "subagent call",
    "aborted call",
    "text",
    "JSON text",
    "Pi image item",
    "OpenCode non-text part",
    "output over 64 KiB",
    "invalid UTF-8",
    "canary",
];

fn text_cells(text: &str, cells: &mut BTreeMap<&'static str, usize>) {
    *cells.entry("text").or_default() += 1;
    if text.contains("sk-ant-api03-") {
        *cells.entry("canary").or_default() += 1;
    }
    if text.contains('\u{FFFD}') {
        *cells.entry("invalid UTF-8").or_default() += 1;
    }
    if text.len() > MAX_SOURCE_UNIT_BYTES {
        *cells.entry("output over 64 KiB").or_default() += 1;
    }
    if serde_json::from_str::<Value>(text).is_ok_and(|value| value.is_object() || value.is_array())
    {
        *cells.entry("JSON text").or_default() += 1;
    }
}

/// The cells each closure cannot produce, as `packages/e2e-tests/src/corpus-sessions/census.ts`
/// declares them; a zero count outside this list is a missing witness.
fn cannot_produce(harness: Harness) -> &'static [&'static str] {
    match harness {
        Harness::OpenCode => &["Pi image item", "output over 64 KiB"],
        Harness::Pi => &[
            "glob",
            "todowrite",
            "MCP tool",
            "subagent call",
            "OpenCode non-text part",
        ],
    }
}

/// Witness counts per R1 cell, by the rules of `packages/e2e-tests/src/corpus-sessions/census.ts`.
fn census(session: &Session) -> BTreeMap<&'static str, usize> {
    let mut cells = BTreeMap::new();
    let tool_cell = |name: &str| -> Option<&'static str> {
        Some(match name {
            "read" => "read",
            "bash" => "bash",
            "edit" => "edit",
            "write" => "write",
            "grep" => "grep",
            "glob" if session.harness == Harness::OpenCode => "glob",
            "todowrite" if session.harness == Harness::OpenCode => "todowrite",
            "task" if session.harness == Harness::OpenCode => "subagent call",
            "corpus_probe" if session.harness == Harness::OpenCode => "MCP tool",
            _ => return None,
        })
    };
    match session.harness {
        Harness::OpenCode => {
            for message in session.fixture["messages"].as_array().into_iter().flatten() {
                for part in message["parts"].as_array().into_iter().flatten() {
                    let data = &part["data"];
                    match data["type"].as_str() {
                        Some("file") => *cells.entry("OpenCode non-text part").or_default() += 1,
                        Some("tool") => {
                            if let Some(cell) = data["tool"].as_str().and_then(tool_cell) {
                                *cells.entry(cell).or_default() += 1;
                            }
                            let state = &data["state"];
                            let output = state["output"]
                                .as_str()
                                .or_else(|| state["error"].as_str())
                                .unwrap_or_default();
                            if output.contains("User aborted the command") {
                                *cells.entry("aborted call").or_default() += 1;
                            }
                            text_cells(output, &mut cells);
                        }
                        _ => {}
                    }
                }
            }
        }
        Harness::Pi => {
            for entry in session.fixture["entries"].as_array().into_iter().flatten() {
                let message = &entry["message"];
                if message["role"].as_str() != Some("toolResult") {
                    continue;
                }
                if let Some(cell) = message["toolName"].as_str().and_then(tool_cell) {
                    *cells.entry(cell).or_default() += 1;
                }
                let items = message["content"].as_array().cloned().unwrap_or_default();
                if items.iter().any(|item| item["type"] == "image") {
                    *cells.entry("Pi image item").or_default() += 1;
                }
                let text: String = items
                    .iter()
                    .filter_map(|item| item["text"].as_str())
                    .collect();
                if message["isError"] == Value::Bool(true) && text.ends_with(" aborted") {
                    *cells.entry("aborted call").or_default() += 1;
                }
                text_cells(&text, &mut cells);
            }
        }
    }
    cells
}

fn host() -> String {
    let name = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|name| name.trim().to_owned())
        .unwrap_or_else(|_| "unknown".to_owned());
    let cpu = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|info| {
            info.lines()
                .find(|line| line.starts_with("model name"))
                .and_then(|line| line.split(':').nth(1))
                .map(|model| model.trim().to_owned())
        })
        .unwrap_or_else(|| "unknown".to_owned());
    let cpus = std::thread::available_parallelism().map_or(0, std::num::NonZero::get);
    format!("{name}; {cpu}; {cpus} CPUs")
}

/// Writes one session's section and returns its unclassified loss count.
fn session_section(out: &mut String, session: &Session) -> usize {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = ReplayStore::open(dir.path());
    let _ = writeln!(out, "## {}", session.closure);
    let outcome = replay(
        &store,
        &identity(session),
        &session.records,
        &harness_adapter,
        PASSES,
    );
    let all = reconcile(&session.records, &outcome);
    let _ = writeln!(
        out,
        "- all units: expected {}, admitted {}, unclassified_loss {}; oracle disagreements: \
         phantom {}, revision_mismatch {}, duplicate {}",
        all.expected,
        all.admitted,
        all.unclassified.len(),
        all.phantom.len(),
        all.revision_mismatch.len(),
        all.duplicate.len(),
    );
    for wanted in ["text", "tool"] {
        let result = reconcile(&of_kind(&session.records, wanted), &outcome);
        let _ = writeln!(out, "- content kind `{wanted}`:");
        let _ = writeln!(
            out,
            "    completeness (published + replayed over expected): {} (replayed {})",
            ratio(result.admitted, result.expected),
            result.replayed,
        );
        losses(out, &result);
    }
    let units: Vec<&ExpectedUnit> = session
        .records
        .iter()
        .flat_map(|record| &record.expected.units)
        .collect();
    let _ = writeln!(out, "- tool-result byte lengths (expected table):");
    for (label, truncated) in [
        ("truncated=true", Some(true)),
        ("truncated=false", Some(false)),
        ("truncated=unknown", None),
    ] {
        let values = units
            .iter()
            .filter(|unit| {
                unit.tool
                    .as_ref()
                    .is_some_and(|tool| tool.truncated == truncated)
            })
            .map(|unit| unit.byte_len)
            .collect();
        byte_lengths(out, label, values);
    }
    let over: Vec<&&ExpectedUnit> = units
        .iter()
        .filter(|unit| unit.byte_len > MAX_SOURCE_UNIT_BYTES)
        .collect();
    let total_bytes: usize = units.iter().map(|unit| unit.byte_len).sum();
    let over_bytes: usize = over.iter().map(|unit| unit.byte_len).sum();
    let _ = writeln!(
        out,
        "- above {MAX_SOURCE_UNIT_BYTES} bytes: units {}, bytes {}",
        ratio(over.len(), units.len()),
        ratio(over_bytes, total_bytes),
    );
    let _ = writeln!(
        out,
        "- offered and admitted per pass (bulk arrival: pass 1 offers the whole session):"
    );
    passes(out, &outcome.passes);
    let _ = writeln!(out, "- census (witness units per R1 cell):");
    let cells = census(session);
    for cell in CELLS {
        let count = cells.get(cell).copied().unwrap_or(0);
        let shown = if cannot_produce(session.harness).contains(&cell) {
            format!("{count} (the closure cannot produce this cell; see the corpus README)")
        } else {
            count.to_string()
        };
        let _ = writeln!(out, "    {cell}: {shown}");
    }
    let _ = writeln!(out);
    all.unclassified.len()
}

fn main() -> ExitCode {
    let runs: usize = std::env::args()
        .nth(1)
        .map_or(5, |runs| runs.parse().expect("runs is a positive integer"));
    assert!(runs > 0, "runs is a positive integer");
    let sessions = sessions();
    let mut out = String::new();
    let _ = writeln!(out, "# Source corpus measurement report\n");
    let _ = writeln!(
        out,
        "Estimand: every figure is a population fact about the frozen corpus \
         (crates/daemon/testdata/corpus/sessions), computed exhaustively; none estimates real \
         sessions. No figure is compared to a threshold. The replay offers every record, the \
         final assistant reply included, which live capture never offers; live reconciliation \
         (the corpus README) counts one unit fewer per session.\n"
    );
    let _ = writeln!(
        out,
        "Replay: a pass offers every unit not yet settled to the screened publisher as one \
         batch, in a fresh kernel per session; passes run until a pass defers nothing, at most \
         {PASSES}. Bounds: unit cap {MAX_SOURCE_UNIT_BYTES} bytes, batch \
         {MAX_SOURCE_BATCH_UNITS} units and {MAX_SOURCE_BATCH_BYTES} bytes.\n"
    );
    let unclassified: usize = sessions
        .iter()
        .map(|session| session_section(&mut out, session))
        .sum();
    let _ = writeln!(
        out,
        "## Publish time against admitted bytes\n\nHost: {}. Runs: {runs}, one after another \
         in one process; each run replays every session into its own fresh kernel, OpenCode \
         first, after the session sections above have replayed each session once in the same \
         process. Pairs are (admitted bytes, elapsed ms) per batch, measured around the screened \
         publisher. The daemon's `publish_ms_last` and `publish_ms_max` gauges also span the \
         receipt preview wait, and no daemon counter records admitted bytes per batch; these \
         pairs are the replay harness's measurement.",
        host()
    );
    for run in 1..=runs {
        for session in &sessions {
            let dir = tempfile::tempdir().expect("temp dir");
            let store = ReplayStore::open(dir.path());
            let outcome = replay(
                &store,
                &identity(session),
                &session.records,
                &harness_adapter,
                PASSES,
            );
            let pairs: Vec<String> = outcome
                .passes
                .iter()
                .map(|pass| format!("({}, {:.3})", pass.admitted_bytes, pass.elapsed_ms))
                .collect();
            let _ = writeln!(out, "- run {run}, {}: {}", session.closure, pairs.join(" "));
        }
    }
    let _ = writeln!(out, "\nunclassified_loss = {unclassified}");
    print!("{out}");
    if unclassified == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

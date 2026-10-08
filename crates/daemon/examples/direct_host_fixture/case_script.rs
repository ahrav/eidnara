//! Corpus case scripts for the direct host fixture.
//!
//! A selection names up to [`MAX_QUEUE`] entries: compression fidelity scenario IDs, or
//! [`FILLER`]. The fixture resolves each scenario ID through the digest-checked corpus, and every
//! later summarizer request consumes the front entry. A scenario entry binds its source's approved
//! example to the ordinals the request actually presents; a filler entry answers with compact
//! fixture-authored segments over the presented records, so a scenario can age behind newer rows
//! whose bodies stay small. A request that does not present the scenario source's messages, or that
//! arrives after the queue is empty, fails typed; neither returns the default scripted text.
//! Before any selection the fixture answers summarizer requests as it always has.

use std::collections::VecDeque;
use std::sync::LazyLock;

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::compression_fidelity_corpus::{CORPUS_BYTES, CORPUS_SHA256, Corpus, Part, corpus_from};

/// The most entries one selection may queue.
pub const MAX_QUEUE: usize = 8;

/// The queue entry that answers one request with compact fixture-authored segments.
pub const FILLER: &str = "filler";

/// Presented records per filler segment.
const FILLER_CHUNK: usize = 2;

/// Characters of a message's first text part that locate it in the presented transcript.
const PROBE_CHARS: usize = 48;

/// One presented record of the summarizer's input: its ordinal range and its text with alias
/// markers removed and whitespace collapsed.
pub type Record = (u64, u64, String);

static CORPUS: LazyLock<Result<(Corpus, Value), String>> = LazyLock::new(|| {
    let corpus = corpus_from(CORPUS_BYTES)?;
    let raw = serde_json::from_slice(CORPUS_BYTES).map_err(|error| error.to_string())?;
    Ok((corpus, raw))
});

/// Why a selection or a scripted answer was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptError {
    CorpusRejected,
    UnknownScenario,
    EmptyQueue,
    QueueTooLong,
}

impl ScriptError {
    pub fn code(self) -> &'static str {
        match self {
            Self::CorpusRejected => "corpus_rejected",
            Self::UnknownScenario => "unknown_scenario",
            Self::EmptyQueue => "empty_queue",
            Self::QueueTooLong => "queue_too_long",
        }
    }
}

/// How a summarizer request armed with a script failed. Both are typed backend failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerFailure {
    /// The request does not present the scenario source's messages as one contiguous run.
    Mismatch,
    /// Every queued scenario was already consumed.
    Exhausted,
}

impl AnswerFailure {
    pub fn provider_code(self) -> &'static str {
        match self {
            Self::Mismatch => "fixture_script_mismatch",
            Self::Exhausted => "fixture_script_exhausted",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Mismatch => "fixture script does not match the presented messages",
            Self::Exhausted => "fixture script queue is exhausted",
        }
    }
}

/// One consumed scenario and the ordinals its approved example was bound to.
#[derive(Debug, Clone, Serialize)]
pub struct Binding {
    pub scenario: String,
    pub source: String,
    pub start: u64,
    pub end: u64,
    /// The first presented ordinal left for a later firing, when the request presented more.
    pub unprocessed_from: Option<u64>,
    /// SHA-256 of the exact answer text.
    pub output_sha256: String,
}

/// The script state the `script-status` control reports.
#[derive(Debug, Clone, Serialize)]
pub struct ScriptStatus {
    pub corpus_sha256: &'static str,
    pub armed: bool,
    pub remaining: usize,
    pub bound: u64,
    /// Requests a filler entry answered.
    pub filled: u64,
    pub mismatched: u64,
    pub exhausted: u64,
    /// The current selection's successful bindings, oldest first.
    pub bindings: Vec<Binding>,
}

enum Queued {
    Scenario { scenario: String, source: String },
    Filler,
}

/// The selection and its consumption record. Every summarizer request after a selection consumes
/// one entry or fails, so an unrelated request cannot leave a scenario for a later one.
#[derive(Default)]
pub struct CaseScript {
    armed: bool,
    queue: VecDeque<Queued>,
    bound: u64,
    filled: u64,
    mismatched: u64,
    exhausted: u64,
    bindings: Vec<Binding>,
}

fn corpus() -> Result<&'static (Corpus, Value), ScriptError> {
    CORPUS.as_ref().map_err(|_| ScriptError::CorpusRejected)
}

/// The case and source a scenario ID names.
fn resolve(scenario: &str) -> Result<(&'static str, &'static str), ScriptError> {
    let (corpus, _) = corpus()?;
    corpus
        .cases
        .iter()
        .find_map(|case| {
            case.scenarios
                .iter()
                .find(|candidate| candidate.id == scenario)
                .map(|found| (case.id.as_str(), found.source.as_str()))
        })
        .ok_or(ScriptError::UnknownScenario)
}

/// The native records of the source a scenario names, as the corpus file spells them, so a
/// harness can seed them without parsing the corpus itself.
pub fn source_records(scenario: &str) -> Result<Value, ScriptError> {
    let (case, source) = resolve(scenario)?;
    let (_, raw) = corpus()?;
    let found = raw["cases"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|candidate| candidate["id"] == case)
        .flat_map(|candidate| candidate["sources"].as_array().into_iter().flatten())
        .find(|candidate| candidate["id"] == source)
        .ok_or(ScriptError::UnknownScenario)?;
    Ok(serde_json::json!({
        "corpus_sha256": CORPUS_SHA256,
        "case": case,
        "scenario": scenario,
        "source": source,
        "messages": found["messages"],
    }))
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The leading characters of a message's first text part, the part that renders as written.
fn probe(parts: &[Part]) -> Option<String> {
    parts.iter().find_map(|part| match part {
        Part::Text { text, .. } => Some(collapse(text).chars().take(PROBE_CHARS).collect()),
        Part::Tool { .. } => None,
    })
}

impl CaseScript {
    /// Replaces the queue with `scenarios`, resolved and bounded before any state changes, and
    /// clears the previous selection's bindings.
    pub fn select(&mut self, scenarios: &[String]) -> Result<(), ScriptError> {
        if scenarios.is_empty() {
            return Err(ScriptError::EmptyQueue);
        }
        if scenarios.len() > MAX_QUEUE {
            return Err(ScriptError::QueueTooLong);
        }
        let queue = scenarios
            .iter()
            .map(|scenario| {
                if scenario == FILLER {
                    return Ok(Queued::Filler);
                }
                resolve(scenario).map(|(_, source)| Queued::Scenario {
                    scenario: scenario.clone(),
                    source: source.to_owned(),
                })
            })
            .collect::<Result<VecDeque<_>, _>>()?;
        self.armed = true;
        self.queue = queue;
        self.bindings.clear();
        Ok(())
    }

    pub fn armed(&self) -> bool {
        self.armed
    }

    pub fn status(&self) -> ScriptStatus {
        ScriptStatus {
            corpus_sha256: CORPUS_SHA256,
            armed: self.armed,
            remaining: self.queue.len(),
            bound: self.bound,
            filled: self.filled,
            mismatched: self.mismatched,
            exhausted: self.exhausted,
            bindings: self.bindings.clone(),
        }
    }

    /// Consumes the front entry for one summarizer request presenting `records`.
    pub fn answer(&mut self, records: &[Record]) -> Result<String, AnswerFailure> {
        let Some(queued) = self.queue.pop_front() else {
            self.exhausted += 1;
            return Err(AnswerFailure::Exhausted);
        };
        let Queued::Scenario { scenario, source } = queued else {
            self.filled += 1;
            return Ok(filler(records));
        };
        match bind(&scenario, &source, records) {
            Some((text, binding)) => {
                self.bound += 1;
                self.bindings.push(binding);
                Ok(text)
            }
            None => {
                self.mismatched += 1;
                Err(AnswerFailure::Mismatch)
            }
        }
    }
}

/// The approved example of `queued`'s source with its range moved to the presented ordinals.
///
/// Every source message must appear, in order, in one contiguous run of presented records, and
/// every record of that run must carry one of them. Records before the run are covered by one
/// fixture-authored lead-in segment, since a summarizer answer covers its chunk from the start;
/// records after it are left to a later firing through `<unprocessed_from>`.
fn bind(scenario: &str, source_id: &str, records: &[Record]) -> Option<(String, Binding)> {
    let (corpus, _) = corpus().ok()?;
    let source = corpus
        .cases
        .iter()
        .find_map(|case| case.source(source_id))?;
    let mut hits = Vec::with_capacity(source.messages.len());
    let mut cursor = 0;
    for message in &source.messages {
        let probe = probe(&message.parts)?;
        let at = cursor
            + records[cursor..]
                .iter()
                .position(|(_, _, text)| text.contains(&probe))?;
        hits.push(at);
        cursor = at;
    }
    let (first, last) = (*hits.first()?, *hits.last()?);
    if !(first..=last).all(|index| hits.contains(&index)) {
        return None;
    }
    let (start, end) = (records[first].0, records[last].1);
    let count = source.messages.len();
    let mut text = source.approved_example.replacen(
        &format!("start=\"1\" end=\"{count}\""),
        &format!("start=\"{start}\" end=\"{end}\""),
        1,
    );
    if text == source.approved_example {
        return None;
    }
    let unprocessed_from = records.get(last + 1).map(|(next, _, _)| *next);
    let meta_at = text.find("<meta>")?;
    let meta_end = text.find("</meta>")? + "</meta>".len();
    let processed = format!("<messages_processed>{start}-{end}</messages_processed>");
    let meta = match unprocessed_from {
        Some(next) => {
            format!("<meta>{processed}<unprocessed_from>{next}</unprocessed_from></meta>")
        }
        None => format!("<meta>{processed}</meta>"),
    };
    text.replace_range(meta_at..meta_end, &meta);
    if first > 0 {
        let (lead_start, lead_end) = (records[0].0, records[first - 1].1);
        let lead = format!(
            "<history_segment start=\"{lead_start}\" end=\"{lead_end}\" title=\"Fixture lead-in {lead_start} to {lead_end}\" episode_type=\"infra\" importance=\"20\"><p1>Fixture-authored lead-in before the scripted case.</p1><p2>Fixture lead-in.</p2><p3>Lead-in.</p3><p4 /></history_segment>"
        );
        let at = text.find("<history_segments>")? + "<history_segments>".len();
        text.insert_str(at, &lead);
    }
    let binding = Binding {
        scenario: scenario.to_owned(),
        source: source_id.to_owned(),
        start,
        end,
        unprocessed_from,
        output_sha256: format!("{:x}", Sha256::digest(text.as_bytes())),
    };
    Some((text, binding))
}

/// Compact segments over every presented record, two records each, with bodies that name only
/// their range, so a newer filler row costs a few tokens at any tier.
fn filler(records: &[Record]) -> String {
    let mut segments = String::new();
    for group in records.chunks(FILLER_CHUNK) {
        let (start, end) = (group[0].0, group[group.len() - 1].1);
        segments.push_str(&format!(
            "<history_segment start=\"{start}\" end=\"{end}\" title=\"Fixture filler {start} to {end}\" episode_type=\"infra\" importance=\"30\"><p1>Fixture-authored filler for messages {start} to {end}.</p1><p2>Filler {start} to {end}.</p2><p3>Filler.</p3><p4 /></history_segment>"
        ));
    }
    let next = records.last().map_or(1, |(_, end, _)| end + 1);
    format!(
        "<output><history_segments>{segments}</history_segments><meta><unprocessed_from>{next}</unprocessed_from></meta></output>"
    )
}

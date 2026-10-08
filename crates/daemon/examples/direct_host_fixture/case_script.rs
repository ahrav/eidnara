//! Corpus case scripts for the direct host fixture.
//!
//! A selection names up to [`MAX_QUEUE`] entries: compression fidelity scenario IDs, a scenario
//! ID with the [`P1_ONLY`] suffix, [`FILLER`], or [`ECHO`] for the fixture's default answer,
//! whose bodies repeat the presented text; `filler:N` and `echo:N` answer with at most `N` rows. The
//! fixture resolves each scenario ID through the digest-checked corpus, and every later
//! summarizer request consumes the front entry when it is admitted. A scenario entry binds
//! its source's approved example to the ordinals the request actually presents; a filler entry
//! answers with compact fixture-authored segments over the presented records, so a scenario can
//! age behind newer rows whose bodies stay small. A request that does not present the scenario
//! source's messages, or that arrives after the queue is empty, fails typed; neither returns the
//! default scripted text. Before any selection the fixture answers summarizer requests as it
//! always has.
//!
//! The consumption counters cover the fixture's lifetime. `bindings` covers the current
//! selection and records a binding only once its answer is delivered, so an admitted request
//! that is cancelled or fails before answering consumes its entry without reporting a binding.

use std::collections::VecDeque;
use std::sync::LazyLock;

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::compression_fidelity_corpus::{
    CORPUS_BYTES, CORPUS_SHA256, Corpus, Part, Source, block_text, corpus_from,
};

/// The most entries one selection may queue.
pub const MAX_QUEUE: usize = 8;

/// The queue entry that answers one request with compact fixture-authored segments.
pub const FILLER: &str = "filler";

/// The queue entry that answers one request with the fixture's default segments, so newer rows
/// can carry bodies as large as the messages they cover.
const ECHO: &str = "echo";

/// The importance of every segment in the default answer.
pub const ECHO_IMPORTANCE: u8 = 50;

/// Presented records per segment in the default answer.
const SUMMARY_CHUNK: usize = 5;

/// The scenario-entry suffix that serves the approved example with its P2 and P3 bodies removed,
/// so the parser's fallback supplies them.
const P1_ONLY: &str = "@p1-only";

/// The importance of every filler and lead-in segment, so a witness can compute the curve over
/// its rows from their count alone.
pub const FIXTURE_IMPORTANCE: u8 = 30;

/// Characters of a native block that locate it in presented or delivered text.
const PROBE_CHARS: usize = 48;

/// Native blocks shorter than this are too generic to locate a leak; they yield no leak probe.
const MIN_LEAK_PROBE_CHARS: usize = 16;

/// Presented records per filler segment when the entry names no row count.
const FILLER_CHUNK: usize = 2;

/// The most rows one `filler:N` or `echo:N` entry may ask for.
const MAX_ROWS: usize = 64;

/// One presented record of the summarizer's input: its ordinal range and its text with alias
/// markers removed. One record can carry several consecutive messages of the same role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub start: u64,
    pub end: u64,
    pub text: String,
}

static CORPUS: LazyLock<Result<(Corpus, Value), String>> = LazyLock::new(|| {
    let corpus = corpus_from(CORPUS_BYTES)?;
    let raw = serde_json::from_slice(CORPUS_BYTES).map_err(|error| error.to_string())?;
    Ok((corpus, raw))
});

/// Why a selection or a source request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptError {
    CorpusRejected,
    UnknownScenario,
    EmptyQueue,
    QueueTooLong,
    BadRowCount,
}

impl ScriptError {
    pub fn code(self) -> &'static str {
        match self {
            Self::CorpusRejected => "corpus_rejected",
            Self::UnknownScenario => "unknown_scenario",
            Self::EmptyQueue => "empty_queue",
            Self::QueueTooLong => "queue_too_long",
            Self::BadRowCount => "bad_row_count",
        }
    }
}

/// How a summarizer request armed with a script failed. Both are typed backend failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerFailure {
    /// The request does not present the scenario source's messages as one contiguous run.
    Mismatch,
    /// Every queued entry was already consumed.
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

/// One delivered scenario answer and the ordinals its approved example was bound to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Binding {
    pub scenario: String,
    pub source: String,
    pub start: u64,
    pub end: u64,
    /// The first presented ordinal left for a later firing, when the request presented more.
    pub unprocessed_from: Option<u64>,
    /// SHA-256 of the exact answer text, so an evidence record can name the bytes it covers.
    pub output_sha256: String,
}

/// An admitted scripted answer. [`CaseScript::delivered`] records it once it is emitted.
#[derive(Debug)]
pub struct Answer {
    pub text: String,
    /// `None` for a filler answer.
    binding: Option<Binding>,
}

/// The script state the `script-status` control reports.
#[derive(Debug, Clone, Serialize)]
pub struct ScriptStatus {
    pub corpus_sha256: &'static str,
    pub armed: bool,
    pub remaining: usize,
    pub bound: u64,
    /// Delivered filler and echo answers.
    pub filled: u64,
    pub mismatched: u64,
    pub exhausted: u64,
    /// The importance of every filler and lead-in segment, and of every echo segment.
    pub filler_importance: u8,
    pub echo_importance: u8,
    /// The current selection's delivered bindings, oldest first.
    pub bindings: Vec<Binding>,
}

enum Queued {
    Scenario {
        scenario: String,
        source: &'static Source,
        p1_only: bool,
    },
    /// `rows` bounds the segment count, or `None` for one segment per two records.
    Filler { rows: Option<usize> },
    /// `rows` bounds the segment count, or `None` for one segment per five records.
    Echo { rows: Option<usize> },
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
    CORPUS.as_ref().map_err(|error| {
        eprintln!("direct host fixture: corpus rejected: {error}");
        ScriptError::CorpusRejected
    })
}

/// The case and source a scenario ID names.
fn resolve(scenario: &str) -> Result<(&'static str, &'static Source), ScriptError> {
    let (corpus, _) = corpus()?;
    let (case, source) = corpus
        .cases
        .iter()
        .find_map(|case| {
            case.scenarios
                .iter()
                .find(|candidate| candidate.id == scenario)
                .map(|found| (case, found.source.as_str()))
        })
        .ok_or(ScriptError::UnknownScenario)?;
    let source = case.source(source).ok_or(ScriptError::CorpusRejected)?;
    Ok((case.id.as_str(), source))
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn probe_of(text: &str) -> String {
    collapse(text).chars().take(PROBE_CHARS).collect()
}

/// The probe that locates a message in the presented transcript: the leading characters of its
/// first text part, the part the presenter renders as written.
fn message_probe(parts: &[Part]) -> Option<String> {
    parts
        .iter()
        .find_map(|part| match part {
            Part::Text { text, .. } => Some(probe_of(text)),
            Part::Tool { .. } => None,
        })
        .filter(|probe| !probe.is_empty())
}

/// The approved example's range pattern: one segment over the whole source, from ordinal 1.
fn range_pattern(source: &Source) -> String {
    format!("start=\"1\" end=\"{}\"", source.messages.len())
}

/// Whether `bind` can rewrite this source's approved example and locate every message.
fn scriptable(source: &Source) -> bool {
    let example = &source.approved_example;
    example.matches(&range_pattern(source)).count() == 1
        && example.matches("<meta>").count() == 1
        && example.contains("<history_segments>")
        && ["<p2>", "</p2>", "<p3>", "</p3>"]
            .iter()
            .all(|tag| example.matches(tag).count() == 1)
        && source
            .messages
            .iter()
            .all(|message| message_probe(&message.parts).is_some())
}

/// The native records of the source a scenario names, as the corpus file spells them, so a
/// harness can seed them without parsing the corpus itself. The records pass through as raw JSON
/// because the corpus schema reads them without a serializer. `leak_probes` are the leading
/// characters of every text block and settled tool output long enough to locate a raw leak, and
/// `memory_examples` are the case's eligible memory examples.
pub fn source_records(scenario: &str) -> Result<Value, ScriptError> {
    let (case, source) = resolve(scenario)?;
    let (_, raw) = corpus()?;
    let found = raw["cases"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|candidate| candidate["id"] == case)
        .flat_map(|candidate| candidate["sources"].as_array().into_iter().flatten())
        .find(|candidate| candidate["id"] == source.id.as_str())
        .ok_or(ScriptError::CorpusRejected)?;
    let leak_probes: Vec<String> = source
        .messages
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(block_text)
        .map(probe_of)
        .filter(|probe| probe.chars().count() >= MIN_LEAK_PROBE_CHARS)
        .collect();
    let (corpus, _) = corpus()?;
    let memory_examples: Vec<Value> = corpus
        .case(case)
        .into_iter()
        .flat_map(|case| &case.memory_examples)
        .map(|example| {
            serde_json::json!({
                "id": example.id,
                "category": example.category,
                "text": example.text,
            })
        })
        .collect();
    Ok(serde_json::json!({
        "corpus_sha256": CORPUS_SHA256,
        "case": case,
        "scenario": scenario,
        "source": source.id,
        "messages": found["messages"],
        "leak_probes": leak_probes,
        "memory_examples": memory_examples,
    }))
}

impl CaseScript {
    /// Replaces the queue with `entries`, resolved and bounded before any state changes, and
    /// clears the previous selection's bindings.
    pub fn select(&mut self, entries: &[String]) -> Result<(), ScriptError> {
        if entries.is_empty() {
            return Err(ScriptError::EmptyQueue);
        }
        if entries.len() > MAX_QUEUE {
            return Err(ScriptError::QueueTooLong);
        }
        let queue = entries
            .iter()
            .map(|entry| {
                if let Some(rows) = row_count(entry, FILLER) {
                    return rows.map(|rows| Queued::Filler { rows });
                }
                if let Some(rows) = row_count(entry, ECHO) {
                    return rows.map(|rows| Queued::Echo { rows });
                }
                let (scenario, p1_only) = match entry.strip_suffix(P1_ONLY) {
                    Some(scenario) => (scenario, true),
                    None => (entry.as_str(), false),
                };
                let (_, source) = resolve(scenario)?;
                if !scriptable(source) {
                    return Err(ScriptError::CorpusRejected);
                }
                Ok(Queued::Scenario {
                    scenario: entry.clone(),
                    source,
                    p1_only,
                })
            })
            .collect::<Result<VecDeque<_>, _>>()?;
        self.armed = true;
        self.queue = queue;
        self.bindings.clear();
        Ok(())
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
            filler_importance: FIXTURE_IMPORTANCE,
            echo_importance: ECHO_IMPORTANCE,
            bindings: self.bindings.clone(),
        }
    }

    /// Consumes the front entry for one admitted summarizer request presenting `records`.
    /// Returns `None` while no selection was ever made.
    pub fn answer(&mut self, records: &[Record]) -> Option<Result<Answer, AnswerFailure>> {
        if !self.armed {
            return None;
        }
        let Some(queued) = self.queue.pop_front() else {
            self.exhausted += 1;
            return Some(Err(AnswerFailure::Exhausted));
        };
        let answer = match queued {
            Queued::Filler { rows } => Ok(Answer {
                text: filler(records, rows),
                binding: None,
            }),
            Queued::Echo { rows } => Ok(Answer {
                text: echo(records, rows),
                binding: None,
            }),
            Queued::Scenario {
                scenario,
                source,
                p1_only,
            } => bind(&scenario, source, p1_only, records)
                .map(|(text, binding)| Answer {
                    text,
                    binding: Some(binding),
                })
                .ok_or(AnswerFailure::Mismatch),
        };
        if answer.is_err() {
            self.mismatched += 1;
        }
        Some(answer)
    }

    /// Records an admitted answer once the fixture emits it.
    pub fn delivered(&mut self, answer: &Answer) {
        match &answer.binding {
            Some(binding) => {
                self.bound += 1;
                self.bindings.push(binding.clone());
            }
            None => self.filled += 1,
        }
    }
}

/// The first and last record of the source's run, when its messages appear in order in one
/// contiguous run of presented records covering exactly the source's ordinals. Consecutive
/// messages may share one merged record; a record inside the run that carries none of them is a
/// mismatch.
fn locate(source: &Source, records: &[Record]) -> Option<(usize, usize)> {
    let mut hits = Vec::with_capacity(source.messages.len());
    let mut cursor = 0;
    for message in &source.messages {
        let probe = message_probe(&message.parts)?;
        let at = cursor
            + records[cursor..]
                .iter()
                .position(|record| collapse(&record.text).contains(&probe))?;
        hits.push(at);
        cursor = at;
    }
    let (first, last) = (*hits.first()?, *hits.last()?);
    let contiguous = hits.windows(2).all(|pair| pair[1] - pair[0] <= 1);
    // A record holds at most as many messages as its range spans, so a later message quoting an
    // earlier one cannot stand in for it.
    let within_ranges = hits.chunk_by(|a, b| a == b).all(|run| {
        let record = &records[run[0]];
        run.len() as u64 <= record.end - record.start + 1
    });
    let covered = records[last].end - records[first].start + 1;
    (contiguous && within_ranges && covered == source.messages.len() as u64)
        .then_some((first, last))
}

/// The approved example of `source` with its range moved to the presented ordinals.
///
/// Records before the source's run are covered by one fixture-authored lead-in segment, since a
/// summarizer answer covers its chunk from the start; records after it are left to a later
/// firing through `<unprocessed_from>`.
fn bind(
    scenario: &str,
    source: &Source,
    p1_only: bool,
    records: &[Record],
) -> Option<(String, Binding)> {
    let (first, last) = locate(source, records)?;
    let (start, end) = (records[first].start, records[last].end);
    let mut text = source.approved_example.replacen(
        &range_pattern(source),
        &format!("start=\"{start}\" end=\"{end}\""),
        1,
    );
    if p1_only {
        for tag in ["p2", "p3"] {
            let close_tag = format!("</{tag}>");
            let open = text.find(&format!("<{tag}>"))?;
            let close = open + text[open..].find(&close_tag)? + close_tag.len();
            text.replace_range(open..close, "");
        }
    }
    let unprocessed_from = records.get(last + 1).map(|record| record.start);
    let meta_at = text.find("<meta>")?;
    let meta_end = meta_at + text[meta_at..].find("</meta>")? + "</meta>".len();
    let processed = format!("<messages_processed>{start}-{end}</messages_processed>");
    let meta = match unprocessed_from {
        Some(next) => {
            format!("<meta>{processed}<unprocessed_from>{next}</unprocessed_from></meta>")
        }
        None => format!("<meta>{processed}</meta>"),
    };
    text.replace_range(meta_at..meta_end, &meta);
    if first > 0 {
        let (lead_start, lead_end) = (records[0].start, records[first - 1].end);
        let lead = format!(
            "<history_segment start=\"{lead_start}\" end=\"{lead_end}\" title=\"Fixture lead-in {lead_start} to {lead_end}\" episode_type=\"infra\" importance=\"{FIXTURE_IMPORTANCE}\"><p1>Fixture-authored lead-in before the scripted case.</p1><p2>Fixture lead-in.</p2><p3>Lead-in.</p3><p4 /></history_segment>"
        );
        let at = text.find("<history_segments>")? + "<history_segments>".len();
        text.insert_str(at, &lead);
    }
    let binding = Binding {
        scenario: scenario.to_owned(),
        source: source.id.clone(),
        start,
        end,
        unprocessed_from,
        output_sha256: format!("{:x}", Sha256::digest(text.as_bytes())),
    };
    Some((text, binding))
}

/// The row bound of a `kind` or `kind:N` queue entry: `None` when `entry` is another kind,
/// `Ok(None)` for the bare kind, and `Ok(Some(N))` for `N` in `1..=MAX_ROWS`.
fn row_count(entry: &str, kind: &str) -> Option<Result<Option<usize>, ScriptError>> {
    if entry == kind {
        return Some(Ok(None));
    }
    let rows = entry.strip_prefix(kind)?.strip_prefix(':')?;
    Some(match rows.parse::<usize>() {
        Ok(rows) if (1..=MAX_ROWS).contains(&rows) => Ok(Some(rows)),
        _ => Err(ScriptError::BadRowCount),
    })
}

/// `records` in contiguous groups: `rows` groups as evenly as the record count allows, or one
/// per record when fewer are presented, or groups of `chunk` without `rows`.
fn groups(records: &[Record], rows: Option<usize>, chunk: usize) -> Vec<&[Record]> {
    match rows {
        None => records.chunks(chunk).collect(),
        Some(rows) => {
            let count = rows.min(records.len());
            (0..count)
                .map(|k| &records[k * records.len() / count..(k + 1) * records.len() / count])
                .collect()
        }
    }
}

/// The summarizer answer the fixture stands in for a provider with: one `history_segment` per
/// run of five presented records, its text the records' own words, in the output document the
/// daemon's validator reads. A recording of this is what the campaign's structured arm replays.
pub fn scripted_summary(records: &[Record]) -> String {
    echo(records, None)
}

/// The default answer over `records` grouped by [`groups`].
fn echo(records: &[Record], rows: Option<usize>) -> String {
    let mut segments = String::new();
    for group in groups(records, rows, SUMMARY_CHUNK) {
        let (start, end) = (group[0].start, group[group.len() - 1].end);
        // Escaped as element content, so a message saying `<T> & B` leaves the document
        // well-formed; the validator unescapes it.
        let text = group
            .iter()
            .map(|record| record.text.as_str())
            .collect::<Vec<_>>()
            .join("; ")
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        segments.push_str(&format!(
            r#"<history_segment start="{start}" end="{end}" title="messages {start} to {end}" episode_type="feature" importance="{ECHO_IMPORTANCE}"><p1>{text}</p1><p2>{text}</p2><p3>messages {start} to {end}</p3><p4 /></history_segment>"#
        ));
    }
    output_document(&segments, records.last().map_or(1, |record| record.end + 1))
}

/// The summarizer's output document over `segments`, resuming at ordinal `next`.
fn output_document(segments: &str, next: u64) -> String {
    format!(
        "<output><history_segments>{segments}</history_segments><meta><unprocessed_from>{next}</unprocessed_from></meta></output>"
    )
}

/// Compact segments over every presented record, with bodies that name only their range, so a
/// newer filler row costs a few tokens at any tier. Without `rows` each segment covers two
/// records; `rows` splits the records into that many contiguous segments, as evenly as the
/// record count allows, or one per record when fewer are presented.
fn filler(records: &[Record], rows: Option<usize>) -> String {
    let mut segments = String::new();
    for group in groups(records, rows, FILLER_CHUNK) {
        let (start, end) = (group[0].start, group[group.len() - 1].end);
        segments.push_str(&format!(
            "<history_segment start=\"{start}\" end=\"{end}\" title=\"Fixture filler {start} to {end}\" episode_type=\"infra\" importance=\"{FIXTURE_IMPORTANCE}\"><p1>Fixture-authored filler for messages {start} to {end}.</p1><p2>Filler {start} to {end}.</p2><p3>Filler.</p3><p4 /></history_segment>"
        ));
    }
    output_document(&segments, records.last().map_or(1, |record| record.end + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(start: u64, end: u64, text: &str) -> Record {
        Record {
            start,
            end,
            text: text.to_owned(),
        }
    }

    /// One presented record per message from ordinal `first`, its text parts joined as the
    /// presenter joins parts.
    fn presented(source: &Source, first: u64) -> Vec<Record> {
        source
            .messages
            .iter()
            .zip(first..)
            .map(|(message, ordinal)| {
                let text = message
                    .parts
                    .iter()
                    .filter_map(block_text)
                    .collect::<Vec<_>>()
                    .join(" / ");
                record(ordinal, ordinal, &text)
            })
            .collect()
    }

    fn scenario_for(source: &Source) -> String {
        let (corpus, _) = corpus().unwrap();
        corpus
            .cases
            .iter()
            .flat_map(|case| &case.scenarios)
            .find(|scenario| scenario.source == source.id)
            .unwrap()
            .id
            .clone()
    }

    fn sources() -> impl Iterator<Item = &'static Source> {
        corpus()
            .unwrap()
            .0
            .cases
            .iter()
            .flat_map(|case| &case.sources)
    }

    fn armed(entries: &[&str]) -> CaseScript {
        let mut script = CaseScript::default();
        let entries: Vec<String> = entries.iter().map(|entry| (*entry).to_owned()).collect();
        script.select(&entries).unwrap();
        script
    }

    #[test]
    fn every_source_binds_behind_a_lead_in_and_before_a_trailing_record() {
        for source in sources() {
            assert!(scriptable(source), "{}", source.id);
            let count = source.messages.len() as u64;
            let mut records = vec![record(1, 1, "lead-in turn")];
            records.extend(presented(source, 2));
            records.push(record(count + 2, count + 2, "trailing turn"));
            let (text, binding) = bind("S", source, false, &records).unwrap();
            assert_eq!(
                (binding.start, binding.end, binding.unprocessed_from),
                (2, count + 1, Some(count + 2)),
                "{}",
                source.id
            );
            assert!(text.contains(&format!("start=\"2\" end=\"{}\"", count + 1)));
            assert!(text.contains("title=\"Fixture lead-in 1 to 1\""));
            assert!(text.contains(&format!(
                "<unprocessed_from>{}</unprocessed_from>",
                count + 2
            )));
            assert_eq!(
                binding.output_sha256,
                format!("{:x}", Sha256::digest(text.as_bytes()))
            );
        }
    }

    #[test]
    fn a_source_presented_at_its_own_ordinals_binds_without_a_lead_in() {
        for source in sources() {
            let (text, binding) = bind("S", source, false, &presented(source, 1)).unwrap();
            assert_eq!((binding.start, binding.unprocessed_from), (1, None));
            assert!(!text.contains("Fixture lead-in"), "{}", source.id);
            assert!(!text.contains("<unprocessed_from>"), "{}", source.id);
        }
    }

    #[test]
    fn consecutive_messages_merged_into_one_record_bind() {
        let source = sources().find(|source| source.id == "C4.V1").unwrap();
        let mut records = presented(source, 1);
        // C4.V1 messages 4 and 5 are consecutive assistant messages.
        let merged = format!("{} / {}", records[3].text, records[4].text);
        records.splice(3..5, [record(4, 5, &merged)]);
        let (_, binding) = bind("S", source, false, &records).unwrap();
        assert_eq!((binding.start, binding.end), (1, 7));
    }

    #[test]
    fn a_foreign_record_a_reordered_run_or_a_missing_message_is_a_mismatch() {
        for source in sources() {
            let records = presented(source, 1);
            let mut foreign = records.clone();
            foreign.insert(1, record(2, 2, "unrelated message"));
            for later in &mut foreign[2..] {
                later.start += 1;
                later.end += 1;
            }
            assert!(
                bind("S", source, false, &foreign).is_none(),
                "{}",
                source.id
            );
            let mut reordered = records.clone();
            reordered.swap(0, 1);
            assert!(
                bind("S", source, false, &reordered).is_none(),
                "{}",
                source.id
            );
            assert!(
                bind("S", source, false, &records[..records.len() - 1]).is_none(),
                "{}",
                source.id
            );
        }
    }

    #[test]
    fn a_counted_filler_entry_answers_with_that_many_segments() {
        let records: Vec<Record> = (1..=10).map(|k| record(k, k, "note")).collect();
        for (rows, segments) in [(1, 1), (3, 3), (4, 4), (6, 6), (7, 7), (10, 10), (64, 10)] {
            let mut script = armed(&[&format!("filler:{rows}")]);
            let answer = script.answer(&records).unwrap().unwrap();
            assert_eq!(
                answer.text.matches("<history_segment ").count(),
                segments,
                "{rows}"
            );
            assert!(
                answer
                    .text
                    .contains("<unprocessed_from>11</unprocessed_from>")
            );
        }
    }

    #[test]
    fn a_p1_only_entry_removes_the_case_p2_and_p3_bodies_and_keeps_the_lead_in() {
        let source = sources().find(|source| source.id == "C1.V1").unwrap();
        let mut records = vec![record(1, 1, "lead-in turn")];
        records.extend(presented(source, 2));
        let mut script = armed(&[&format!("{}{P1_ONLY}", scenario_for(source))]);
        let answer = script.answer(&records).unwrap().unwrap();
        let case_at = answer.text.find("start=\"2\"").unwrap();
        let (lead, case) = answer.text.split_at(case_at);
        assert!(lead.contains("<p2>Fixture lead-in.</p2>"));
        assert!(case.contains("<p1>") && case.contains("<p4 />"));
        assert!(!case.contains("<p2>") && !case.contains("<p3>"));
        let binding = answer.binding.as_ref().unwrap();
        assert_eq!(
            binding.output_sha256,
            format!("{:x}", Sha256::digest(answer.text.as_bytes()))
        );
    }

    #[test]
    fn plain_filler_keeps_two_records_per_segment_and_echo_repeats_the_text() {
        let records: Vec<Record> = (1..=5).map(|k| record(k, k, "note")).collect();
        let mut script = armed(&[FILLER, ECHO]);
        let filled = script.answer(&records).unwrap().unwrap().text;
        for range in [
            "start=\"1\" end=\"2\"",
            "start=\"3\" end=\"4\"",
            "start=\"5\" end=\"5\"",
        ] {
            assert!(filled.contains(range), "{range}");
        }
        assert!(filled.contains(&format!("importance=\"{FIXTURE_IMPORTANCE}\"")));
        let echoed = script.answer(&records).unwrap().unwrap().text;
        assert!(echoed.contains("<p1>note; note; note; note; note</p1>"));
        assert!(echoed.contains(&format!("importance=\"{ECHO_IMPORTANCE}\"")));
        let mut counted = armed(&["echo:5"]);
        let rows = counted.answer(&records).unwrap().unwrap().text;
        assert_eq!(rows.matches("<history_segment ").count(), 5);
        assert!(rows.contains("<p1>note</p1>"));
    }

    #[test]
    fn the_queue_consumes_in_order_and_records_bindings_only_when_delivered() {
        let source = sources().find(|source| source.id == "C1.V1").unwrap();
        let scenario = scenario_for(source);
        let mut unarmed = CaseScript::default();
        assert!(unarmed.answer(&presented(source, 1)).is_none());

        let mut script = armed(&[FILLER, &scenario, &scenario]);
        let filled = script.answer(&[record(1, 2, "x")]).unwrap().unwrap();
        assert!(filled.text.contains("title=\"Fixture filler 1 to 2\""));
        script.delivered(&filled);
        let bound = script.answer(&presented(source, 1)).unwrap().unwrap();
        assert!(script.status().bindings.is_empty());
        script.delivered(&bound);
        assert_eq!(
            script
                .answer(&[record(1, 1, "unrelated")])
                .unwrap()
                .unwrap_err(),
            AnswerFailure::Mismatch
        );
        assert_eq!(
            script.answer(&presented(source, 1)).unwrap().unwrap_err(),
            AnswerFailure::Exhausted
        );
        let status = script.status();
        assert_eq!(
            (
                status.remaining,
                status.bound,
                status.filled,
                status.mismatched,
                status.exhausted
            ),
            (0, 1, 1, 1, 1)
        );
        assert_eq!(status.bindings.len(), 1);
        assert_eq!(status.bindings[0].scenario, scenario);
    }

    #[test]
    fn a_refused_selection_leaves_the_previous_queue() {
        let mut script = armed(&[FILLER]);
        for (entries, error) in [
            (vec![], ScriptError::EmptyQueue),
            (
                vec![FILLER.to_owned(); MAX_QUEUE + 1],
                ScriptError::QueueTooLong,
            ),
            (
                vec![FILLER.to_owned(), "C9.S1".to_owned()],
                ScriptError::UnknownScenario,
            ),
            (vec!["filler:0".to_owned()], ScriptError::BadRowCount),
            (vec!["filler:65".to_owned()], ScriptError::BadRowCount),
            (vec!["echo:0".to_owned()], ScriptError::BadRowCount),
        ] {
            assert_eq!(script.select(&entries), Err(error));
            assert_eq!(script.status().remaining, 1);
        }
    }

    #[test]
    fn source_records_carry_long_native_probes() {
        let records = source_records("C6.S1").unwrap();
        assert_eq!(records["source"], "C6.V1");
        let probes = records["leak_probes"].as_array().unwrap();
        assert!(!probes.is_empty());
        assert!(probes.iter().all(|probe| {
            let length = probe.as_str().unwrap().chars().count();
            (MIN_LEAK_PROBE_CHARS..=PROBE_CHARS).contains(&length)
        }));
        assert_eq!(source_records("C6.V1"), Err(ScriptError::UnknownScenario));
        assert_eq!(records["memory_examples"], serde_json::json!([]));
        let c3 = source_records("C3.S6").unwrap();
        assert_eq!(c3["memory_examples"][0]["id"], "C3.M1");
        assert_eq!(c3["memory_examples"][0]["category"], "ARCHITECTURE");
    }
}

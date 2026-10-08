//! Corpus case scripts for the direct host fixture.
//!
//! A selection names up to [`MAX_QUEUE`] entries: compression fidelity scenario IDs, or
//! [`FILLER`]. The fixture resolves each scenario ID through the digest-checked corpus, and every
//! later summarizer request consumes the front entry when it is admitted. A scenario entry binds
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

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::ops::Range;
use std::sync::LazyLock;

use daemon::history_summarizer_chunk::{ALIAS_CLOSE, ALIAS_OPEN};
use memchr::memmem::Finder;
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

/// Characters of a native block that locate it in presented or delivered text.
const PROBE_CHARS: usize = 48;

/// Native blocks shorter than this are too generic to locate a leak; they yield no leak probe.
const MIN_LEAK_PROBE_CHARS: usize = 16;

/// Presented records per filler segment.
const FILLER_CHUNK: usize = 2;

/// One presented record of the summarizer's input: its ordinal range and its text with alias
/// markers removed. One record can carry several consecutive messages of the same role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub start: u64,
    pub end: u64,
    pub text: String,
}

/// The ordinal range of the presented line that starts `text`, `[a-b] R: part / part`, and the
/// offset of its parts. The search stays within the line.
fn presented_header(text: &str) -> Option<(u64, u64, usize)> {
    let rest = text.strip_prefix('[')?;
    let (range, after_range) = split_once_pair(rest, b']')?;
    let (start, end) = match memchr::memchr(b'-', range.as_bytes()) {
        Some(dash) => (range[..dash].parse().ok()?, range[dash + 1..].parse().ok()?),
        None => {
            let ordinal = range.parse().ok()?;
            (ordinal, ordinal)
        }
    };
    let (_, parts_at) = split_once_pair(&rest[after_range..], b':')?;
    Some((start, end, 1 + after_range + parts_at))
}

/// One presented line with alias markers stripped from its parts.
#[cfg(test)]
fn presented_line(line: &str) -> Option<(u64, u64, String)> {
    let (start, end, parts_at) = presented_header(line)?;
    let mut text = String::with_capacity(line.len() - parts_at);
    push_stripped_tokens(line, parts_at, &mut text);
    Some((start, end, text))
}

/// Splits `text` at its first `first` followed by a space, when that comes before any newline.
/// Returns the text before the delimiter and the offset after it.
fn split_once_pair(text: &str, first: u8) -> Option<(&str, usize)> {
    let bytes = text.as_bytes();
    let at = bytes
        .windows(2)
        .position(|window| window[0] == b'\n' || (window[0] == first && window[1] == b' '))?;
    (bytes[at] == first).then(|| (&text[..at], at + 2))
}

/// Whitespace follows `char::is_whitespace`, so a non-ASCII separator such as U+00A0 splits
/// tokens exactly as `str::split_whitespace` does.
#[inline(always)]
fn whitespace_len(text: &str, at: usize) -> usize {
    let byte = text.as_bytes()[at];
    if byte < 0x80 {
        usize::from(is_ascii_whitespace(byte))
    } else {
        multibyte_whitespace_len(text, at)
    }
}

#[cold]
#[inline(never)]
fn multibyte_whitespace_len(text: &str, at: usize) -> usize {
    match text[at..].chars().next() {
        Some(character) if character.is_whitespace() => character.len_utf8(),
        _ => 0,
    }
}

/// `is_ascii_whitespace` matches `char::is_whitespace` for ASCII bytes, including U+000B.
fn is_ascii_whitespace(byte: u8) -> bool {
    matches!(byte, b'\t' | b'\n' | 0x0B | 0x0C | b'\r' | b' ')
}

fn skip_whitespace(text: &str, mut at: usize) -> usize {
    while at < text.len() {
        let gap = whitespace_len(text, at);
        if gap == 0 {
            break;
        }
        at += gap;
    }
    at
}

/// Skips whitespace within the current line; a newline stays in place.
fn skip_line_whitespace(text: &str, mut at: usize) -> usize {
    while at < text.len() && text.as_bytes()[at] != b'\n' {
        let gap = whitespace_len(text, at);
        if gap == 0 {
            break;
        }
        at += gap;
    }
    at
}

fn token_end(text: &str, mut at: usize) -> usize {
    let bytes = text.as_bytes();
    while at < bytes.len() {
        if bytes[at] < 0x80 {
            if is_ascii_whitespace(bytes[at]) {
                break;
            }
            at += 1;
        } else {
            if whitespace_len(text, at) > 0 {
                break;
            }
            at += text[at..].chars().next().map_or(1, char::len_utf8);
        }
    }
    at
}

const LOW_BITS: u64 = 0x0101_0101_0101_0101;
const HIGH_BITS: u64 = LOW_BITS << 7;

/// `next_special` returns the index of the first byte at or after `from` that is below U+0020, has
/// its high bit set, or is a space right after another space. `next_special` scans past isolated
/// spaces and classifies eight bytes per word-parallel step.
fn next_special(bytes: &[u8], from: usize) -> usize {
    let mut carry = u64::from(from > 0 && bytes[from - 1] == b' ') << 7;
    let (words, tail) = bytes[from..].as_chunks::<8>();
    let (pairs, odd) = words.as_chunks::<2>();
    let mut at = from;
    for [first, second] in pairs {
        let (special, next) = classify(u64::from_le_bytes(*first), carry);
        if special != 0 {
            return at + (special.trailing_zeros() / 8) as usize;
        }
        let (special, next) = classify(u64::from_le_bytes(*second), next);
        if special != 0 {
            return at + 8 + (special.trailing_zeros() / 8) as usize;
        }
        carry = next;
        at += 16;
    }
    for word in odd {
        let (special, next) = classify(u64::from_le_bytes(*word), carry);
        if special != 0 {
            return at + (special.trailing_zeros() / 8) as usize;
        }
        carry = next;
        at += 8;
    }
    for &byte in tail {
        if !(0x20..0x80).contains(&byte) || (byte == b' ' && carry != 0) {
            return at;
        }
        carry = u64::from(byte == b' ');
        at += 1;
    }
    at
}

/// Flags the special bytes of one little-endian word in each byte's high bit. `carry` holds, in
/// bit 7, whether the byte before the word is a space; the returned carry does the same for the
/// word's last byte.
#[inline(always)]
fn classify(word: u64, carry: u64) -> (u64, u64) {
    let seven_bits = LOW_BITS * 0x7F;
    let below_space = !(((word & seven_bits) + LOW_BITS * 0x60) | word) & HIGH_BITS;
    let not_space = word ^ (LOW_BITS * 0x20);
    let space = !(((not_space & seven_bits) + seven_bits) | not_space) & HIGH_BITS;
    let pair = space & ((space << 8) | carry);
    ((word & HIGH_BITS) | below_space | pair, space >> 56)
}

/// The byte offset of the first `ALIAS_CLOSE` in `token`, a slice that starts on a character
/// boundary.
fn alias_close(token: &[u8]) -> Option<usize> {
    let mut encoded = [0; 4];
    let close = ALIAS_CLOSE.encode_utf8(&mut encoded).as_bytes();
    token
        .windows(close.len())
        .position(|window| window == close)
}

/// Appends the stripped tokens of the line that starts at `from` in `text` to `out`, and returns
/// the offset of the newline that ends the line, or `text.len()`.
fn push_stripped_tokens(text: &str, from: usize, out: &mut String) -> usize {
    let bytes = text.as_bytes();
    let mut scan = skip_line_whitespace(text, from);
    let mut run = scan;
    let mut token_start = true;
    while scan < bytes.len() {
        let at = next_special(bytes, scan);
        if at >= bytes.len() || bytes[at] == b'\n' {
            scan = at;
            break;
        }
        let opens_token = if at == run {
            token_start
        } else {
            bytes[at - 1] == b' '
        };
        if opens_token && text[at..].starts_with(ALIAS_OPEN) {
            let marker = at + ALIAS_OPEN.len_utf8();
            let end = token_end(text, marker);
            if let Some(close) = alias_close(&bytes[marker..end]) {
                out.push_str(&text[run..at]);
                scan = marker + close + ALIAS_CLOSE.len_utf8();
                let after = skip_line_whitespace(text, scan);
                token_start = after > scan;
                if token_start && !out.is_empty() && !out.ends_with(' ') {
                    out.push(' ');
                }
                scan = after;
                run = scan;
                continue;
            }
            scan = marker;
            token_start = false;
            continue;
        }
        let gap = whitespace_len(text, at);
        if gap > 0 {
            out.push_str(&text[run..at]);
            scan = skip_line_whitespace(text, at + gap);
            if !out.is_empty() && !out.ends_with(' ') {
                out.push(' ');
            }
            run = scan;
            token_start = true;
            continue;
        }
        scan = at + text[at..].chars().next().map_or(1, char::len_utf8);
        token_start = false;
    }
    out.push_str(&text[run..scan]);
    if out.ends_with(' ') {
        out.pop();
    }
    scan
}

/// The presented records of a summarizer prompt's first `<new_messages>` section. Returns `None`
/// when the section is missing, unclosed, or holds no record line.
pub fn presented_records(prompt: &str) -> Option<Vec<Record>> {
    static OPEN: LazyLock<Finder<'static>> = LazyLock::new(|| Finder::new(b"<new_messages>"));
    static CLOSE: LazyLock<Finder<'static>> = LazyLock::new(|| Finder::new(b"</new_messages>"));
    let body = &prompt[OPEN.find(prompt.as_bytes())? + OPEN.needle().len()..];
    let body = &body[..CLOSE.find(body.as_bytes())?];
    // The first parsed header starts a record. A later header starts a record when its start
    // equals the preceding record's `end + 1`. After the first record, a nonblank line that does
    // not start a record extends the preceding record's text.
    let bytes = body.as_bytes();
    let mut lines: Vec<Record> = Vec::with_capacity(body.len() / 128 + 1);
    let mut scratch = String::new();
    let mut pending: Option<(u64, u64)> = None;
    let mut at = 0;
    while at < bytes.len() {
        let next = pending.map(|(_, end)| end + 1);
        match presented_header(&body[at..]) {
            Some((start, end, parts_at)) if next.is_none_or(|next| start == next) => {
                if let Some((start, end)) = pending {
                    lines.push(Record {
                        start,
                        end,
                        text: scratch.clone(),
                    });
                }
                scratch.clear();
                at = push_stripped_tokens(body, at + parts_at, &mut scratch) + 1;
                pending = Some((start, end));
            }
            _ => {
                let line_end = memchr::memchr(b'\n', &bytes[at..]).map_or(bytes.len(), |n| at + n);
                let continuation = body[at..line_end].trim();
                if pending.is_some() && !continuation.is_empty() {
                    scratch.push(' ');
                    scratch.push_str(continuation);
                }
                at = line_end + 1;
            }
        }
    }
    if let Some((start, end)) = pending {
        lines.push(Record {
            start,
            end,
            text: scratch,
        });
    }
    (!lines.is_empty()).then_some(lines)
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
    /// Delivered filler answers.
    pub filled: u64,
    pub mismatched: u64,
    pub exhausted: u64,
    /// The current selection's delivered bindings, oldest first.
    pub bindings: Vec<Binding>,
}

enum Queued {
    Scenario {
        scenario: String,
        source: &'static Source,
        prepared: &'static Prepared,
    },
    Filler,
}

/// Case ID and source ID.
type SourceKey = (&'static str, &'static str);

/// What `bind` needs for each source, prepared once per process. `None` marks a source whose
/// approved example `bind` cannot rewrite or whose messages it cannot locate.
static PREPARED: LazyLock<HashMap<SourceKey, Option<Prepared>>> = LazyLock::new(|| {
    corpus()
        .map(|(corpus, _)| {
            corpus
                .cases
                .iter()
                .flat_map(|case| {
                    case.sources.iter().map(move |source| {
                        ((case.id.as_str(), source.id.as_str()), prepared(source))
                    })
                })
                .collect()
        })
        .unwrap_or_default()
});

struct Prepared {
    probes: Vec<Probe>,
    template: Template,
}

/// Where `bind` rewrites a source's approved example. The example holds, in order, one
/// `<history_segments>` opening tag, one range pattern, and one `<meta>` element.
struct Template {
    /// The byte offset right after `<history_segments>`, where a lead-in segment goes.
    segments: usize,
    /// The bytes of `start="1" end="N"`.
    range: Range<usize>,
    /// The bytes of `<meta>...</meta>`.
    meta: Range<usize>,
}

impl Template {
    fn new(source: &Source) -> Option<Self> {
        let example = source.approved_example.as_str();
        let pattern = range_pattern(source);
        let segments = example.find("<history_segments>")? + "<history_segments>".len();
        let range_at = example.find(&pattern)?;
        let range = range_at..range_at + pattern.len();
        let meta_at = example.find("<meta>")?;
        let meta = meta_at..meta_at + example[meta_at..].find("</meta>")? + "</meta>".len();
        let single =
            example.matches(&pattern).count() == 1 && example.matches("<meta>").count() == 1;
        let ordered = segments <= range.start && range.end <= meta.start;
        (single && ordered).then_some(Self {
            segments,
            range,
            meta,
        })
    }
}

/// Locates one source message in presented text with a ready-made finder.
struct Probe {
    /// The words of the collapsed probe, in order.
    tokens: Vec<String>,
    /// Finds candidate occurrences of the first word.
    first: Finder<'static>,
    /// The probe was cut right after a whole word, so a carrying text continues past that word.
    trailing_space: bool,
}

impl Probe {
    fn new(parts: &[Part]) -> Option<Self> {
        message_probe(parts).map(|text| Self::from_text(&text))
    }

    fn from_text(text: &str) -> Self {
        let tokens: Vec<String> = text.split_whitespace().map(str::to_owned).collect();
        Self {
            first: Finder::new(&tokens[0]).into_owned(),
            trailing_space: text.ends_with(' '),
            tokens,
        }
    }

    /// Whether `text` with its whitespace collapsed contains the probe.
    ///
    /// Collapsing joins the words of `text` with single spaces, so the probe occurs in the
    /// collapsed text exactly where its words occur in `text` separated by whitespace runs, with
    /// the first word possibly inside a longer word and the last word possibly cut short. A probe
    /// that ends in a space needs whitespace and a further word after its last word, since
    /// collapsing drops trailing whitespace.
    fn carried_by(&self, text: &str) -> bool {
        self.first
            .find_iter(text.as_bytes())
            .any(|at| self.matches_at(text, at))
    }

    fn matches_at(&self, text: &str, at: usize) -> bool {
        let bytes = text.as_bytes();
        let mut pos = at;
        for (index, token) in self.tokens.iter().enumerate() {
            if index > 0 {
                let after = skip_whitespace(text, pos);
                if after == pos {
                    return false;
                }
                pos = after;
            }
            if !bytes[pos..].starts_with(token.as_bytes()) {
                return false;
            }
            pos += token.len();
        }
        if !self.trailing_space {
            return true;
        }
        let after = skip_whitespace(text, pos);
        after > pos && after < text.len()
    }
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

fn probe_of(text: &str) -> String {
    text.split_whitespace()
        .enumerate()
        .flat_map(|(index, word)| (index > 0).then_some(' ').into_iter().chain(word.chars()))
        .take(PROBE_CHARS)
        .collect()
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

/// The template and the probes of every message, when `bind` can rewrite this source's approved
/// example and locate every message.
fn prepared(source: &Source) -> Option<Prepared> {
    let template = Template::new(source)?;
    let probes = source
        .messages
        .iter()
        .map(|message| Probe::new(&message.parts))
        .collect::<Option<Vec<_>>>()?;
    Some(Prepared { probes, template })
}

/// The native records of the source a scenario names, as the corpus file spells them, so a
/// harness can seed them without parsing the corpus itself. The records pass through as raw JSON
/// because the corpus schema reads them without a serializer. `leak_probes` are the leading
/// characters of every text block and settled tool output long enough to locate a raw leak.
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
    Ok(serde_json::json!({
        "corpus_sha256": CORPUS_SHA256,
        "case": case,
        "scenario": scenario,
        "source": source.id,
        "messages": found["messages"],
        "leak_probes": leak_probes,
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
                if entry == FILLER {
                    return Ok(Queued::Filler);
                }
                let (case, source) = resolve(entry)?;
                let prepared = PREPARED
                    .get(&(case, source.id.as_str()))
                    .and_then(Option::as_ref)
                    .ok_or(ScriptError::CorpusRejected)?;
                Ok(Queued::Scenario {
                    scenario: entry.clone(),
                    source,
                    prepared,
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
            Queued::Filler => Ok(Answer {
                text: filler(records),
                binding: None,
            }),
            Queued::Scenario {
                scenario,
                source,
                prepared,
            } => bind(&scenario, source, prepared, records)
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
fn locate(probes: &[Probe], records: &[Record]) -> Option<(usize, usize)> {
    let mut hits = Vec::with_capacity(probes.len());
    let mut cursor = 0;
    for probe in probes {
        let at = cursor
            + records[cursor..]
                .iter()
                .position(|record| probe.carried_by(&record.text))?;
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
    (contiguous && within_ranges && covered == probes.len() as u64).then_some((first, last))
}

/// The approved example of `source` with its range moved to the presented ordinals.
///
/// Records before the source's run are covered by one fixture-authored lead-in segment, since a
/// summarizer answer covers its chunk from the start; records after it are left to a later
/// firing through `<unprocessed_from>`.
fn bind(
    scenario: &str,
    source: &Source,
    prepared: &Prepared,
    records: &[Record],
) -> Option<(String, Binding)> {
    let (first, last) = locate(&prepared.probes, records)?;
    let (start, end) = (records[first].start, records[last].end);
    let unprocessed_from = records.get(last + 1).map(|record| record.start);
    let example = source.approved_example.as_str();
    let template = &prepared.template;
    let mut text = String::with_capacity(example.len() + 512);
    text.push_str(&example[..template.segments]);
    if first > 0 {
        let (lead_start, lead_end) = (records[0].start, records[first - 1].end);
        write!(
            text,
            "<history_segment start=\"{lead_start}\" end=\"{lead_end}\" title=\"Fixture lead-in {lead_start} to {lead_end}\" episode_type=\"infra\" importance=\"20\"><p1>Fixture-authored lead-in before the scripted case.</p1><p2>Fixture lead-in.</p2><p3>Lead-in.</p3><p4 /></history_segment>"
        )
        .expect("write to a String");
    }
    text.push_str(&example[template.segments..template.range.start]);
    write!(text, "start=\"{start}\" end=\"{end}\"").expect("write to a String");
    text.push_str(&example[template.range.end..template.meta.start]);
    write!(
        text,
        "<meta><messages_processed>{start}-{end}</messages_processed>"
    )
    .expect("write to a String");
    if let Some(next) = unprocessed_from {
        write!(text, "<unprocessed_from>{next}</unprocessed_from>").expect("write to a String");
    }
    text.push_str("</meta>");
    text.push_str(&example[template.meta.end..]);
    let binding = Binding {
        scenario: scenario.to_owned(),
        source: source.id.clone(),
        start,
        end,
        unprocessed_from,
        output_sha256: lower_hex(&Sha256::digest(text.as_bytes())),
    };
    Some((text, binding))
}

fn lower_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|byte| {
            [
                DIGITS[usize::from(byte >> 4)],
                DIGITS[usize::from(byte & 0x0F)],
            ]
        })
        .map(char::from)
        .collect()
}

/// The summarizer's output document over `segments`, resuming at ordinal `next`.
pub fn output_document(segments: &str, next: u64) -> String {
    format!(
        "<output><history_segments>{segments}</history_segments><meta><unprocessed_from>{next}</unprocessed_from></meta></output>"
    )
}

/// Compact segments over every presented record, two records each, with bodies that name only
/// their range, so a newer filler row costs a few tokens at any tier.
fn filler(records: &[Record]) -> String {
    let mut segments = String::with_capacity(records.len().div_ceil(FILLER_CHUNK) * 256);
    for group in records.chunks(FILLER_CHUNK) {
        let (start, end) = (group[0].start, group[group.len() - 1].end);
        push_filler_segment(&mut segments, start, end);
    }
    output_document(&segments, records.last().map_or(1, |record| record.end + 1))
}

/// The literal pieces of a filler segment; `start` and `end` alternate between them.
const FILLER_PIECES: [&str; 9] = [
    "<history_segment start=\"",
    "\" end=\"",
    "\" title=\"Fixture filler ",
    " to ",
    "\" episode_type=\"infra\" importance=\"30\"><p1>Fixture-authored filler for messages ",
    " to ",
    ".</p1><p2>Filler ",
    " to ",
    ".</p2><p3>Filler.</p3><p4 /></history_segment>",
];

fn push_filler_segment(out: &mut String, start: u64, end: u64) {
    for (index, piece) in FILLER_PIECES.iter().enumerate() {
        out.push_str(piece);
        if index + 1 < FILLER_PIECES.len() {
            push_decimal(out, if index % 2 == 0 { start } else { end });
        }
    }
}

/// Appends `value` in decimal without the formatting machinery.
fn push_decimal(out: &mut String, mut value: u64) {
    let mut digits = [0u8; 20];
    let mut at = digits.len();
    loop {
        at -= 1;
        digits[at] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    out.extend(digits[at..].iter().copied().map(char::from));
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

    fn ready(source: &Source) -> Prepared {
        prepared(source).unwrap_or_else(|| panic!("{}", source.id))
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
            let prepared = ready(source);
            let count = source.messages.len() as u64;
            let mut records = vec![record(1, 1, "lead-in turn")];
            records.extend(presented(source, 2));
            records.push(record(count + 2, count + 2, "trailing turn"));
            let (text, binding) = bind("S", source, &prepared, &records).unwrap();
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
            let (text, binding) = bind("S", source, &ready(source), &presented(source, 1)).unwrap();
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
        let (_, binding) = bind("S", source, &ready(source), &records).unwrap();
        assert_eq!((binding.start, binding.end), (1, 7));
    }

    #[test]
    fn a_probe_is_carried_through_collapsed_whitespace_only() {
        let carries = |text: &str, probe: &str| Probe::from_text(probe).carried_by(text);
        assert!(carries("x a b c y", "a b c"));
        assert!(carries("xa \n b\u{a0}cd", "a b cd"));
        assert!(carries("a b c", "a b "));
        assert!(carries("a b\t c", "a b "));
        assert!(!carries("a b \n", "a b "));
        assert!(!carries("a b", "a b "));
        assert!(!carries("ab c", "a b c"));
        assert!(!carries("a b c", "a b cd"));
    }

    struct Rng(u64);

    impl Rng {
        fn below(&mut self, bound: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % bound as u64) as usize
        }

        fn text(&mut self, pieces: &[&str], count: usize) -> String {
            (0..count)
                .map(|_| pieces[self.below(pieces.len())])
                .collect()
        }
    }

    const PIECES: &[&str] = &[
        "a", "b", "c", "ab", "bc", " ", " ", " ", "  ", "\t", "\n", "\u{a0}", "\u{2028}",
        "\u{3000}", "\u{200b}", "é", "/", "«s1»", "«", "»",
    ];

    #[test]
    fn carried_by_agrees_with_collapsing_the_text() {
        let collapse = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for _ in 0..20_000 {
            let text_pieces = 1 + rng.below(12);
            let text = rng.text(PIECES, text_pieces);
            let probe_pieces = 1 + rng.below(5);
            let mut probe = collapse(&rng.text(PIECES, probe_pieces));
            if rng.below(3) == 0 {
                probe.push(' ');
            }
            if probe.trim().is_empty() {
                continue;
            }
            assert_eq!(
                Probe::from_text(&probe).carried_by(&text),
                collapse(&text).contains(&probe),
                "text {text:?} probe {probe:?}"
            );
        }
    }

    #[test]
    fn presented_line_agrees_with_splitting_on_whitespace() {
        let reference = |parts: &str| -> String {
            parts
                .split_whitespace()
                .map(|token| match token.strip_prefix(ALIAS_OPEN) {
                    Some(marked) => marked
                        .split_once(ALIAS_CLOSE)
                        .map_or(token, |(_, rest)| rest),
                    None => token,
                })
                .filter(|token| !token.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        };
        let mut rng = Rng(0xD1B5_4A32_D192_ED03);
        for _ in 0..20_000 {
            let pieces = rng.below(24);
            let parts = rng.text(PIECES, pieces).replace('\n', "\t");
            let line = format!("[1] U: {parts}");
            assert_eq!(
                presented_line(&line).map(|(_, _, text)| text),
                Some(reference(&parts)),
                "{parts:?}"
            );
        }
    }

    #[test]
    fn a_source_presented_with_wider_whitespace_binds() {
        for source in sources() {
            let mut records = presented(source, 1);
            for record in &mut records {
                record.text = record.text.replace(' ', " \n\t ");
            }
            let (_, binding) = bind("S", source, &ready(source), &records).unwrap();
            assert_eq!(
                (binding.start, binding.end),
                (1, source.messages.len() as u64),
                "{}",
                source.id
            );
        }
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
            let prepared = ready(source);
            assert!(
                bind("S", source, &prepared, &foreign).is_none(),
                "{}",
                source.id
            );
            let mut reordered = records.clone();
            reordered.swap(0, 1);
            assert!(
                bind("S", source, &prepared, &reordered).is_none(),
                "{}",
                source.id
            );
            assert!(
                bind("S", source, &prepared, &records[..records.len() - 1]).is_none(),
                "{}",
                source.id
            );
        }
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
    }
}

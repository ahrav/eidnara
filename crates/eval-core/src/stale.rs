//! Stale preference as a real harness serves it: a coding session whose user
//! sets project values and later changes them, the provider request the
//! harness sends when the user asks for a value, the five M0 renderings of
//! that request's served history, the knowledge-update grade of a live
//! answer, and the paired exact McNemar test between two renderings. Every
//! function is pure and makes no model call.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use serde_json::Value;

use crate::statistics::Ratio;

pub const STALE_EXPORT_SCHEMA: &str = "eval-stale-preference-export/v1";

/// The D-8 override sentence, the only addition arm (b) makes.
pub const PRECEDENCE_SENTENCE: &str = "Later statements supersede earlier ones: where two statements in this history disagree, the later one is current.";

/// Whether `text` carries `phrase` as whole words: `slot4` is not carried by
/// a text saying `slot47`. An empty phrase is never carried.
pub fn carries(text: &str, phrase: &str) -> bool {
    locate(text, phrase).is_some()
}

/// The first whole-word occurrence of `phrase` in `text`.
pub fn locate(text: &str, phrase: &str) -> Option<ServedSpan> {
    if phrase.is_empty() {
        return None;
    }
    let in_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.match_indices(phrase)
        .find(|(at, _)| {
            let before = text[..*at].chars().next_back();
            let after = text[at + phrase.len()..].chars().next();
            !before.is_some_and(in_word) && !after.is_some_and(in_word)
        })
        .map(|(start, _)| ServedSpan {
            start,
            end: start + phrase.len(),
        })
}

/// A byte range of a served text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServedSpan {
    pub start: usize,
    pub end: usize,
}

/// Which of a pair's two values a served text carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StaleDelivery {
    Live,
    Stale,
    Both,
    Neither,
}

impl StaleDelivery {
    pub fn of(served: &str, stale: &str, live: &str) -> Self {
        match (carries(served, stale), carries(served, live)) {
            (true, true) => Self::Both,
            (true, false) => Self::Stale,
            (false, true) => Self::Live,
            (false, false) => Self::Neither,
        }
    }

    /// The stale value reached the model, with or without the live one.
    pub fn stale_delivered(self) -> bool {
        matches!(self, Self::Stale | Self::Both)
    }
}

/// A live answer under the LongMemEval knowledge-update rule: naming the old
/// value as history beside the new one as current is current.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grade {
    Current,
    Stale,
    Miss,
}

pub fn grade(answer: &str, stale: &str, live: &str) -> Grade {
    if carries(answer, live) {
        Grade::Current
    } else if carries(answer, stale) {
        Grade::Stale
    } else {
        Grade::Miss
    }
}

/// One text part of a provider request, replaced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartText {
    pub at: HistoryAt,
    pub text: String,
}

/// Arms (b) through (e) of one served request, each as the one text part it
/// replaces; arm (a) is the request as served. (b) puts the D-8 override
/// sentence in a `<memory-updates>` block at the head of the m1 delta, where
/// D-8 puts it; (c) appends one D-7 footer line to the stale segment's body,
/// stale prose kept; (d) replaces the stale value in that body with the D-7
/// marker; (e) removes it from that body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Arms {
    pub precedence_line: PartText,
    pub footer: PartText,
    pub anchored_replacement: PartText,
    pub omission_oracle: PartText,
}

const M0_OPEN: &str = "<session-history>";
const M0_CLOSE: &str = "</session-history>";
const M1_OPEN: &str = "<session-history-since>\n";
const M1_SEGMENTS_OPEN: &str = "<new-history_segments>";
const M1_SEGMENTS_CLOSE: &str = "</new-history_segments>";
/// The daemon's m1 text when the delta is empty (`memory_render::M1_PLACEHOLDER`).
pub const M1_PLACEHOLDER: &str = "(no new content since last materialization)";

/// One rendered segment of a served history block: its message range and
/// its body's byte range in the part (after the heading line).
#[derive(Debug, Clone, Copy)]
struct Segment {
    start: u64,
    end: u64,
    body: ServedSpan,
}

/// The segments rendered between `open` and `close` in `text`, each body
/// ending at the next heading or the block's close.
fn segments_in(text: &str, open: &str, close: &str) -> Vec<Segment> {
    let Some(from) = text.find(open).map(|at| at + open.len()) else {
        return Vec::new();
    };
    let to = text[from..].find(close).map_or(text.len(), |at| from + at);
    let block = &text[from..to];
    let headings: Vec<usize> = block
        .match_indices("## ")
        .map(|(at, _)| at)
        .filter(|at| *at == 0 || block[..*at].ends_with('\n'))
        .collect();
    headings
        .iter()
        .enumerate()
        .filter_map(|(index, at)| {
            let line_end = at + block[*at..].find('\n')?;
            let (start, end) = block[at + 3..line_end].split(' ').next()?.split_once('-')?;
            let next = headings.get(index + 1).copied().unwrap_or(block.len());
            let body_end = block[..next].trim_end_matches('\n').len().max(line_end + 1);
            Some(Segment {
                start: start.parse().ok()?,
                end: end.parse().ok()?,
                body: ServedSpan {
                    start: from + line_end + 1,
                    end: from + body_end,
                },
            })
        })
        .collect()
}

/// The served history of a request: the part holding `<session-history>`
/// (m0) and the m1 part (`<session-history-since>` or the empty-delta
/// placeholder), each with its rendered segments.
struct Served<'a> {
    m0: Option<(HistoryAt, &'a str, Vec<Segment>)>,
    m1: Option<(HistoryAt, &'a str, Vec<Segment>)>,
}

impl<'a> Served<'a> {
    fn of(parts: &[(HistoryAt, &'a str)]) -> Self {
        let m0 = parts
            .iter()
            .find(|(_, text)| text.contains(M0_OPEN))
            .map(|(at, text)| (*at, *text, segments_in(text, M0_OPEN, M0_CLOSE)));
        let m1 = parts
            .iter()
            .find(|(_, text)| text.starts_with(M1_OPEN) || text.trim() == M1_PLACEHOLDER)
            .map(|(at, text)| {
                (
                    *at,
                    *text,
                    segments_in(text, M1_SEGMENTS_OPEN, M1_SEGMENTS_CLOSE),
                )
            });
        Self { m0, m1 }
    }

    fn blocks(&self) -> impl Iterator<Item = (Block, HistoryAt, &'a str, &Vec<Segment>)> {
        [(Block::M0, &self.m0), (Block::M1, &self.m1)]
            .into_iter()
            .filter_map(|(block, part)| {
                part.as_ref()
                    .map(|(at, text, segments)| (block, *at, *text, segments))
            })
    }

    /// The served segment whose range holds `ordinal`, m0 first, then m1.
    fn segment_of(&self, ordinal: u64) -> Option<(Block, HistoryAt, &'a str, Segment)> {
        self.blocks().find_map(|(block, at, text, segments)| {
            segments
                .iter()
                .find(|s| (s.start..=s.end).contains(&ordinal))
                .map(|segment| (block, at, text, *segment))
        })
    }
}

/// Every whole-word occurrence of `phrase` in `text[span]`, as spans of `text`.
fn occurrences(text: &str, span: ServedSpan, phrase: &str) -> Vec<ServedSpan> {
    let mut found = Vec::new();
    let mut from = span.start;
    while let Some(hit) = locate(&text[from..span.end], phrase) {
        found.push(ServedSpan {
            start: from + hit.start,
            end: from + hit.end,
        });
        from += hit.end;
    }
    found
}

/// Every span of `text` in `spans` (in order, disjoint) replaced by `with`.
fn replaced(text: &str, spans: &[ServedSpan], with: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for span in spans {
        out.push_str(&text[at..span.start]);
        out.push_str(with);
        at = span.end;
    }
    out.push_str(&text[at..]);
    out
}

/// Arms (b) through (e) for a stale value served at `values` inside the body
/// of `segment` in the part `at`. `m1` is the request's m1 part, which (b)
/// rewrites.
fn arms(
    at: HistoryAt,
    text: &str,
    segment: Segment,
    values: &[ServedSpan],
    m1: (HistoryAt, &str),
    pair: &FactPair,
    live_ordinal: u64,
) -> Arms {
    let (key, live) = (&pair.key, &pair.live_value);
    let insert = |text: &str, offset: usize, added: &str| {
        format!("{}{added}{}", &text[..offset], &text[offset..])
    };
    let updates = format!("<memory-updates>\n{PRECEDENCE_SENTENCE}\n</memory-updates>");
    let (m1_at, delta) = m1;
    let precedence = match delta.starts_with(M1_OPEN) {
        true => insert(delta, M1_OPEN.len(), &format!("{updates}\n")),
        false => format!("{M1_OPEN}{updates}\n</session-history-since>"),
    };
    let part = |text: String| PartText { at, text };
    Arms {
        precedence_line: PartText {
            at: m1_at,
            text: precedence,
        },
        footer: part(insert(
            text,
            segment.body.end,
            &format!("\n[corrections: {key} = {live} @{live_ordinal}]"),
        )),
        anchored_replacement: part(replaced(
            text,
            values,
            &format!("[corrected @{live_ordinal}: {key} = {live}]"),
        )),
        omission_oracle: part(replaced(text, values, "")),
    }
}

/// Where a request serves a message: a segment of the m0 history, a segment
/// of the m1 delta, the raw tail under its ordinal marker, or nowhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Block {
    M0,
    M1,
    Raw,
    Absent,
}

pub const STALE_WORLD_SCHEMA: &str = "eval-stale-world/v1";
pub const STALE_CAPTURE_SCHEMA: &str = "eval-stale-capture/v1";

const COMPONENTS: [&str; 16] = [
    "billing service",
    "search indexer",
    "auth gateway",
    "payments worker",
    "metrics exporter",
    "image resizer",
    "session cache",
    "notification queue",
    "report builder",
    "flag service",
    "audit logger",
    "upload proxy",
    "ledger replica",
    "rate limiter",
    "geo resolver",
    "email relay",
];
const ATTRIBUTES: [&str; 8] = [
    "port",
    "timeout",
    "batch size",
    "worker count",
    "retry limit",
    "queue depth",
    "cache ttl",
    "connection limit",
];
/// The most subjects a world names: every component and attribute pair.
pub const MAX_SUBJECTS: usize = COMPONENTS.len() * ATTRIBUTES.len();
/// The assistant's answer to every user turn: an acknowledgement carrying no
/// value, so each value is stated once.
pub const ACKNOWLEDGEMENT: &str = "Noted.";

/// One harness turn: the user's prompt and the assistant's reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactTurn {
    pub user: String,
    pub assistant: String,
}

/// One subject stated, then restated with another value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactPair {
    pub task: String,
    pub subject: String,
    /// The subject as a D-7 claim key (`billing-service.port`).
    pub key: String,
    pub stale_value: String,
    pub live_value: String,
    pub stale_statement: String,
    pub restatement: String,
    pub stale_turn: u32,
    pub restating_turn: u32,
    pub question: String,
}

/// A coding session in which the user first sets every project value, later
/// sets every one again in a drawn order, then reports unrelated progress.
/// The phases guarantee the summarizer publishes a statement before it sees
/// its correction, and the progress ages the corrections out of the raw tail:
/// gate A needs the stale prose in an earlier segment and the correction in a
/// later one. A correction repeats the statement's form with no "instead" or
/// "change of plan", so only order says which value is current. Values are
/// distinct five-digit numbers, so a whole-word match finds one value and
/// nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactWorld {
    pub schema: String,
    pub root_seed: String,
    pub turns: Vec<FactTurn>,
    pub pairs: Vec<FactPair>,
}

/// The fact world of `subjects` subjects (at most [`MAX_SUBJECTS`]) under
/// `root_seed`. Each restatement targets a stated, not yet restated subject
/// drawn by `keyed_draw`, so the distance between a statement and its
/// correction varies.
pub fn fact_world(root_seed: u64, subjects: usize) -> FactWorld {
    assert!(subjects <= MAX_SUBJECTS, "at most {MAX_SUBJECTS} subjects");
    let draw = |kind, site: String, occurrence| {
        crate::stream::keyed_draw(
            root_seed,
            &crate::stream::ChoiceSite {
                kind,
                actor: "facts".to_string(),
                site,
                occurrence,
            },
        )
    };
    let mut used = std::collections::BTreeSet::new();
    let mut value = |site: String| {
        (0..)
            .map(|occurrence| {
                10_000
                    + draw(
                        crate::stream::ChoiceKind::TextWord,
                        site.clone(),
                        occurrence,
                    ) % 90_000
            })
            .find(|value| used.insert(*value))
            .expect("five-digit values outnumber statements")
            .to_string()
    };
    let subject = |index: usize| {
        let (component, attribute) = (
            COMPONENTS[index % COMPONENTS.len()],
            ATTRIBUTES[index / COMPONENTS.len()],
        );
        let slug = |words: &str| words.replace(' ', "-");
        (
            format!("{component} {attribute}"),
            format!("{}.{}", slug(component), slug(attribute)),
        )
    };
    let mut turns = Vec::with_capacity(subjects * 2);
    let mut pairs = Vec::with_capacity(subjects);
    for index in 0..subjects {
        let (name, key) = subject(index);
        let task = format!("stale-{index}");
        let stale_value = value(format!("stale:{task}"));
        let stale_statement = format!("Set the {name} to {stale_value}.");
        pairs.push(FactPair {
            task,
            question: format!("What is the {name} now? Reply with just the value."),
            subject: name,
            key,
            stale_value,
            live_value: String::new(),
            stale_statement: stale_statement.clone(),
            restatement: String::new(),
            stale_turn: index as u32,
            restating_turn: 0,
        });
        turns.push(FactTurn {
            user: stale_statement,
            assistant: ACKNOWLEDGEMENT.to_string(),
        });
    }
    let mut pending: Vec<usize> = (0..subjects).collect();
    while !pending.is_empty() {
        let turn = turns.len() as u32;
        let pick = draw(
            crate::stream::ChoiceKind::RestatementTarget,
            format!("turn:{turn}"),
            0,
        ) % pending.len() as u64;
        let pair = &mut pairs[pending.remove(pick as usize)];
        pair.live_value = value(format!("live:{}", pair.task));
        pair.restatement = format!("Set the {} to {}.", pair.subject, pair.live_value);
        pair.restating_turn = turn;
        turns.push(FactTurn {
            user: pair.restatement.clone(),
            assistant: ACKNOWLEDGEMENT.to_string(),
        });
    }
    for index in 0..subjects {
        turns.push(FactTurn {
            user: format!("Status note {index}: {}", FILLER[index % FILLER.len()]),
            assistant: ACKNOWLEDGEMENT.to_string(),
        });
    }
    FactWorld {
        schema: STALE_WORLD_SCHEMA.to_string(),
        root_seed: root_seed.to_string(),
        turns,
        pairs,
    }
}

/// Work the session reports after its last correction, naming no subject and
/// no value, so the corrections age out of the raw tail into segments.
const FILLER: [&str; 4] = [
    "the refactor of the request router moved the retry helpers into their own module and the unit tests still pass after the move.",
    "the flaky integration test turned out to be a missing await in the fixture teardown, and the rerun came back clean on the first try.",
    "the dependency upgrade went through after pinning the lockfile, and the release notes for the upgrade mention no behavior change.",
    "the dashboards now group errors by handler, which made the slow endpoint obvious; the follow-up is tracked in the backlog.",
];

/// One stored history segment's tiers, as the harness read them from the
/// daemon's store after the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SegmentTiers {
    pub start_message: i64,
    pub end_message: i64,
    pub p1: Option<String>,
    pub p2: Option<String>,
    pub p3: Option<String>,
    pub p4: Option<String>,
}

/// What a harness run produced: the world it lived, the harness that lived
/// it, the daemon's stored segments, and, per pair, the provider request the
/// harness sent on that pair's question turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaleCapture {
    pub schema: String,
    pub harness: String,
    /// The model that wrote the daemon's segments, or `fixture/scripted`.
    pub summarizer: String,
    pub world: FactWorld,
    pub segments: Vec<SegmentTiers>,
    pub requests: BTreeMap<String, Value>,
}

/// Where in a provider request the served history sits:
/// `messages[message].content[part].text`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryAt {
    pub message: usize,
    pub part: usize,
}

/// Every text part of a request's messages, with its position.
fn text_parts(request: &Value) -> Vec<(HistoryAt, &str)> {
    let messages = request["messages"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut parts = Vec::new();
    for (message, entry) in messages.iter().enumerate() {
        match &entry["content"] {
            Value::String(text) => parts.push((HistoryAt { message, part: 0 }, text.as_str())),
            Value::Array(content) => {
                for (part, block) in content.iter().enumerate() {
                    if let Some(text) = block["text"].as_str() {
                        parts.push((HistoryAt { message, part }, text));
                    }
                }
            }
            _ => {}
        }
    }
    parts
}

/// The request with `parts` replaced: an arm's request.
pub fn with_parts(request: &Value, parts: &[&PartText]) -> Value {
    let mut request = request.clone();
    for PartText { at, text } in parts.iter().copied() {
        match &mut request["messages"][at.message]["content"] {
            Value::String(old) => *old = text.clone(),
            content => content[at.part]["text"] = Value::String(text.clone()),
        }
    }
    request
}

/// Stored text as the renderer serves it: XML-escaped, with every body line
/// that starts a heading indented (`decay_render`'s escape and guard).
fn as_served(stored: &str) -> String {
    let escaped = stored
        .trim()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace("\n## ", "\n ## ");
    match escaped.starts_with("## ") {
        true => format!(" {escaped}"),
        false => escaped,
    }
}

/// The tier (`1..=4`) a served segment rendered at: the first stored tier
/// whose served form is the body; `0` when no stored segment matches.
fn rendered_tier(text: &str, segment: Segment, stored: &[SegmentTiers]) -> u8 {
    let body = text[segment.body.start..segment.body.end].trim();
    stored
        .iter()
        .find(|s| (s.start_message, s.end_message) == (segment.start as i64, segment.end as i64))
        .and_then(|s| {
            [&s.p1, &s.p2, &s.p3, &s.p4]
                .iter()
                .position(|tier| tier.as_deref().map(as_served).as_deref() == Some(body))
        })
        .map_or(0, |index| index as u8 + 1)
}

/// One stale-preference pair as the export carries it: the question, the
/// claim key its arms name, both values, the restating message's ordinal and
/// where the request serves it, the tier the stale segment rendered at, what
/// the served request delivered, whether the stale value is also served
/// outside the stale segment (a later segment's "from 34827 to 94225", the
/// raw tail, the hint), the served request itself (arm (a)), the part holding
/// the stale segment and the value's spans in its body, and arms (b) through
/// (e) as the parts they replace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StalePair {
    pub task: String,
    pub question: String,
    pub key: String,
    pub stale_value: String,
    pub live_value: String,
    pub restating_ordinal: u64,
    pub restatement: Block,
    /// `1..=4`, or `0` when the rendered body matched no stored tier.
    pub stale_tier: u8,
    pub stale_block: Block,
    pub delivery: StaleDelivery,
    pub stale_elsewhere: bool,
    pub history: HistoryAt,
    pub stale_spans: Vec<ServedSpan>,
    pub request: Value,
    pub arms: Arms,
}

/// A pair whose stale segment does not serve the stale value (decayed to a
/// tier without it, dropped by the summarizer, or not folded), so no
/// rendering can change it: what the whole request delivered, and the
/// request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unlocatable {
    pub delivery: StaleDelivery,
    pub request: Value,
}

/// The stale-preference export over one or more harness sessions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaleExport {
    pub schema: String,
    pub harness: String,
    pub summarizer: String,
    /// The world seeds, comma-separated decimals, one per session.
    pub root_seed: String,
    pub pairs: Vec<StalePair>,
    pub unlocatable: BTreeMap<String, Unlocatable>,
    /// Pairs whose request carried the stale value anywhere, located or not.
    pub stale_delivered: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureError {
    /// The capture's or its world's schema is not the one this build reads.
    Schema { found: String },
    /// The world is not `fact_world` of its own seed and size, so its
    /// ordinals, keys, and values cannot be trusted.
    WorldMismatch,
    /// A pair of the world has no captured request.
    MissingRequest { task: String },
    /// A request serves no m1 part, where arm (b) goes.
    NoM1 { task: String },
    /// Merged exports disagree on schema, harness, or summarizer, or repeat
    /// a world.
    Unmergeable { reason: &'static str },
}

debug_display!(CaptureError);

/// The export of one capture. A pair is located when the served segment that
/// covers its stale statement (in m0, then in the m1 delta) carries the stale
/// value in its body: where D-7 would anchor that segment's claim. That part
/// is the one arms (c) through (e) vary; (d) and (e) act on every whole-word
/// occurrence of the value in that body and nowhere else. The delivery is
/// over every text part of the request.
pub fn export_capture(capture: &StaleCapture) -> Result<StaleExport, CaptureError> {
    for found in [&capture.schema, &capture.world.schema] {
        if ![STALE_CAPTURE_SCHEMA, STALE_WORLD_SCHEMA].contains(&found.as_str()) {
            return Err(CaptureError::Schema {
                found: found.clone(),
            });
        }
    }
    let seed = capture
        .world
        .root_seed
        .parse::<u64>()
        .map_err(|_| CaptureError::WorldMismatch)?;
    if capture.world.pairs.len() > MAX_SUBJECTS
        || fact_world(seed, capture.world.pairs.len()) != capture.world
    {
        return Err(CaptureError::WorldMismatch);
    }
    let mut export = StaleExport {
        schema: STALE_EXPORT_SCHEMA.to_string(),
        harness: capture.harness.clone(),
        summarizer: capture.summarizer.clone(),
        root_seed: capture.world.root_seed.clone(),
        pairs: Vec::new(),
        unlocatable: BTreeMap::new(),
        stale_delivered: 0,
    };
    // Every turn is one user and one assistant message, numbered from one.
    let ordinal = |turn: u32| 2 * u64::from(turn) + 1;
    for pair in &capture.world.pairs {
        let task = || pair.task.clone();
        let request = capture
            .requests
            .get(&pair.task)
            .ok_or_else(|| CaptureError::MissingRequest { task: task() })?;
        let parts = text_parts(request);
        let joined = parts.iter().map(|(_, t)| *t).collect::<Vec<_>>().join("\n");
        let delivery = StaleDelivery::of(&joined, &pair.stale_value, &pair.live_value);
        export.stale_delivered += u32::from(delivery.stale_delivered());
        let served = Served::of(&parts);
        let m1 = served
            .m1
            .as_ref()
            .map(|(at, text, _)| (*at, *text))
            .ok_or_else(|| CaptureError::NoM1 { task: task() })?;
        let located = served
            .segment_of(ordinal(pair.stale_turn))
            .map(|(block, at, text, segment)| {
                let spans = occurrences(text, segment.body, &pair.stale_value);
                (block, at, text, segment, spans)
            })
            .filter(|(.., spans)| !spans.is_empty());
        let Some((stale_block, history, text, segment, stale_spans)) = located else {
            export.unlocatable.insert(
                task(),
                Unlocatable {
                    delivery,
                    request: request.clone(),
                },
            );
            continue;
        };
        let restating_ordinal = ordinal(pair.restating_turn);
        let marker = format!("§{restating_ordinal}§ ");
        let restatement = match served.segment_of(restating_ordinal) {
            Some((block, ..)) => block,
            None if parts.iter().any(|(_, t)| t.starts_with(&marker)) => Block::Raw,
            None => Block::Absent,
        };
        let stripped = replaced(text, &stale_spans, "");
        let stale_elsewhere = parts
            .iter()
            .map(|(at, t)| {
                if *at == history {
                    stripped.as_str()
                } else {
                    *t
                }
            })
            .any(|t| carries(t, &pair.stale_value));
        export.pairs.push(StalePair {
            task: task(),
            question: pair.question.clone(),
            key: pair.key.clone(),
            stale_value: pair.stale_value.clone(),
            live_value: pair.live_value.clone(),
            restating_ordinal,
            restatement,
            stale_tier: rendered_tier(text, segment, &capture.segments),
            stale_block,
            delivery,
            stale_elsewhere,
            history,
            arms: arms(
                history,
                text,
                segment,
                &stale_spans,
                m1,
                pair,
                restating_ordinal,
            ),
            stale_spans,
            request: request.clone(),
        });
    }
    Ok(export)
}

/// Merges exports of independent harness sessions: one schema, harness, and
/// summarizer, no world twice. Task ids gain the session's one-based
/// position (`world-2:stale-5`), so equal per-world ids cannot collide.
pub fn merge_exports(exports: Vec<StaleExport>) -> Result<StaleExport, CaptureError> {
    let mut exports = exports.into_iter();
    let first = exports.next().ok_or(CaptureError::Unmergeable {
        reason: "no exports",
    })?;
    let mut merged = StaleExport {
        pairs: Vec::new(),
        unlocatable: BTreeMap::new(),
        stale_delivered: 0,
        root_seed: String::new(),
        ..first.clone()
    };
    let mut seeds = std::collections::BTreeSet::new();
    for (index, export) in std::iter::once(first).chain(exports).enumerate() {
        if (&export.schema, &export.harness, &export.summarizer)
            != (&merged.schema, &merged.harness, &merged.summarizer)
        {
            return Err(CaptureError::Unmergeable {
                reason: "schema, harness, or summarizer differs",
            });
        }
        if !seeds.insert(export.root_seed.clone()) {
            return Err(CaptureError::Unmergeable {
                reason: "a world appears twice",
            });
        }
        let prefix = |task: &str| format!("world-{}:{task}", index + 1);
        merged
            .pairs
            .extend(export.pairs.into_iter().map(|mut pair| {
                pair.task = prefix(&pair.task);
                pair
            }));
        merged.unlocatable.extend(
            export
                .unlocatable
                .into_iter()
                .map(|(task, value)| (prefix(&task), value)),
        );
        merged.stale_delivered += export.stale_delivered;
        if index > 0 {
            merged.root_seed.push(',');
        }
        merged.root_seed.push_str(&export.root_seed);
    }
    Ok(merged)
}

/// Discordant counts of two arms over the pairs both answered, and the exact
/// two-sided binomial decision at the pre-registered alpha. The test is
/// two-sided: which arm is better is whichever of `first_only` and
/// `second_only` is smaller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McNemar {
    /// Pairs both arms answered.
    pub pairs: u32,
    /// Pairs where one arm or both have no outcome (a failed or censored
    /// call): reported, never scored.
    pub indeterminate: u32,
    /// Pairs where the event occurred on the first arm only.
    pub first_only: u32,
    /// Pairs where the event occurred on the second arm only.
    pub second_only: u32,
    /// `p <= alpha` for `p = min(1, 2 P(X <= min(b, c)))`, `X ~ Bin(b + c, 1/2)`.
    pub reject: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McNemarError {
    /// The two arms do not name the same pairs.
    UnpairedArms,
    /// Alpha must lie strictly between zero and one.
    AlphaOutOfRange,
    /// The exact tail left 128-bit range: roughly when `b + c` plus
    /// `log2` of alpha's denominator passes 127 (about 123 discordant pairs
    /// at alpha 1/20 split evenly).
    Overflow,
}

debug_display!(McNemarError);

/// Paired exact McNemar over a pre-registered event (for example "answered
/// stale") per pair on two arms, `None` where a call failed or was censored,
/// in exact integer arithmetic.
pub fn mcnemar(
    first: &BTreeMap<String, Option<bool>>,
    second: &BTreeMap<String, Option<bool>>,
    alpha: Ratio,
) -> Result<McNemar, McNemarError> {
    if !first.keys().eq(second.keys()) {
        return Err(McNemarError::UnpairedArms);
    }
    if alpha <= Ratio::ZERO || alpha >= Ratio::ONE {
        return Err(McNemarError::AlphaOutOfRange);
    }
    let answered: Vec<(bool, bool)> = first
        .iter()
        .filter_map(|(id, a)| Some(((*a)?, second[id]?)))
        .collect();
    let count = |cell: (bool, bool)| answered.iter().filter(|pair| **pair == cell).count() as u32;
    let (b, c) = (count((true, false)), count((false, true)));
    // ponytail: 128-bit tail, refused once `b + c + log2(denominator)` passes
    // 127 (about 123 discordant pairs at alpha 1/20); move to limbs if a
    // campaign ever gets there.
    let n = b + c;
    let tail = (0..=b.min(c)).try_fold(0u128, |sum, i| {
        let coefficient = crate::censoring::choose(u64::from(n), u64::from(i))
            .map_err(|_| McNemarError::Overflow)?;
        sum.checked_add(coefficient).ok_or(McNemarError::Overflow)
    })?;
    let (numerator, denominator) = alpha.parts();
    let lhs = tail.checked_mul(2 * denominator as u128);
    let rhs = 1u128
        .checked_shl(n)
        .and_then(|total| total.checked_mul(numerator as u128));
    let (Some(lhs), Some(rhs)) = (lhs, rhs) else {
        return Err(McNemarError::Overflow);
    };
    Ok(McNemar {
        pairs: answered.len() as u32,
        indeterminate: (first.len() - answered.len()) as u32,
        first_only: b,
        second_only: c,
        reject: lhs <= rhs,
    })
}

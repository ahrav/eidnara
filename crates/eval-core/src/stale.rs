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

/// Arms (b) through (e) of one served request, each as the text parts it
/// replaces; arm (a) is the request as served. (b) puts the D-8 override
/// sentence in a `<memory-updates>` block at the head of the m1 delta, where
/// D-8 puts it; (c) appends one D-7 footer line to the stale segment's body,
/// stale prose kept; (d) replaces the stale value with the D-7 marker; (e)
/// removes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Arms {
    pub precedence_line: Vec<PartText>,
    pub footer: Vec<PartText>,
    pub anchored_replacement: Vec<PartText>,
    pub omission_oracle: Vec<PartText>,
}

/// The D-7 claim-key grammar `[a-z0-9_-]+(\.[a-z0-9_-]+)+`, at most 64 bytes.
fn is_claim_key(key: &str) -> bool {
    let part = |p: &str| {
        !p.is_empty()
            && p.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    };
    key.len() <= 64 && key.contains('.') && key.split('.').all(part)
}

const M0_OPEN: &str = "<session-history>";
const M0_CLOSE: &str = "</session-history>";
const M1_OPEN: &str = "<session-history-since>\n";
/// The daemon's m1 text when the delta is empty (`memory_render::M1_PLACEHOLDER`).
pub const M1_PLACEHOLDER: &str = "(no new content since last materialization)";
/// What ends a rendered segment's body: the next segment's heading or the
/// close of the block it sits in.
const SEGMENT_ENDS: [&str; 3] = [
    "\n\n## ",
    "\n</session-history",
    "\n</new-history_segments>",
];

/// The served history of a request: the m0 part (`<session-history>`) and the
/// m1 part (`<session-history-since>` or the empty-delta placeholder).
#[derive(Debug, Clone, Copy)]
pub struct Served<'a> {
    pub m0: Option<(HistoryAt, &'a str)>,
    pub m1: Option<(HistoryAt, &'a str)>,
}

impl<'a> Served<'a> {
    fn of(parts: &[(HistoryAt, &'a str)]) -> Self {
        let find = |pick: &dyn Fn(&str) -> bool| parts.iter().copied().find(|(_, t)| pick(t));
        Self {
            m0: find(&|t| t.starts_with(M0_OPEN)),
            m1: find(&|t| t.starts_with(M1_OPEN) || t.trim() == M1_PLACEHOLDER),
        }
    }

    /// The part holding `phrase` and its span there: m0 first, then m1.
    fn locate(&self, phrase: &str) -> Option<(HistoryAt, &'a str, ServedSpan)> {
        [self.m0, self.m1]
            .into_iter()
            .flatten()
            .find_map(|(at, text)| locate(text, phrase).map(|span| (at, text, span)))
    }
}

/// Builds arms (b) through (e) for the stale value at `stale` in the part
/// `at` of `served`. `key` must satisfy the D-7 grammar and the span must lie
/// on character boundaries of that part.
pub fn arms(
    served: Served<'_>,
    at: HistoryAt,
    stale: ServedSpan,
    key: &str,
    live: &str,
    live_ordinal: u64,
) -> Arms {
    assert!(is_claim_key(key), "{key:?} is not a claim key");
    let text = [served.m0, served.m1]
        .into_iter()
        .flatten()
        .find(|(part, _)| *part == at)
        .map(|(_, text)| text)
        .expect("the stale statement's part is served history");
    let patch = |text: String| vec![PartText { at, text }];
    let segment_end = SEGMENT_ENDS
        .iter()
        .filter_map(|end| text[stale.end..].find(end).map(|found| stale.end + found))
        .min()
        .unwrap_or(text.len());
    let insert =
        |text: &str, at: usize, added: &str| format!("{}{added}{}", &text[..at], &text[at..]);
    let updates = format!("<memory-updates>\n{PRECEDENCE_SENTENCE}\n</memory-updates>");
    let precedence_line = match (served.m1, served.m0) {
        (Some((m1, delta)), _) if delta.starts_with(M1_OPEN) => vec![PartText {
            at: m1,
            text: insert(delta, M1_OPEN.len(), &format!("{updates}\n")),
        }],
        (Some((m1, _)), _) => vec![PartText {
            at: m1,
            text: format!("{M1_OPEN}{updates}\n</session-history-since>"),
        }],
        (None, Some((m0, history))) => {
            let close = history
                .find(M0_CLOSE)
                .map_or(history.len(), |c| c + M0_CLOSE.len());
            vec![PartText {
                at: m0,
                text: insert(
                    history,
                    close,
                    &format!("\n\n{M1_OPEN}{updates}\n</session-history-since>"),
                ),
            }]
        }
        (None, None) => unreachable!("the stale statement's part is served history"),
    };
    let at_span = |replacement: &str| {
        patch(format!(
            "{}{replacement}{}",
            &text[..stale.start],
            &text[stale.end..]
        ))
    };
    Arms {
        precedence_line,
        footer: patch(insert(
            text,
            segment_end,
            &format!("\n[corrections: {key} = {live} @{live_ordinal}]"),
        )),
        anchored_replacement: at_span(&format!("[corrected @{live_ordinal}: {key} = {live}]")),
        omission_oracle: at_span(""),
    }
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

/// A coding session in which the user sets project values and later changes
/// every one of them: two statements to each restatement, the evaluator's
/// correction regime, with the restatements left at the end closing the
/// session. Values are distinct five-digit numbers, so a whole-word match
/// finds one value and nothing else.
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
    let mut turns = Vec::new();
    let mut pairs: Vec<FactPair> = Vec::new();
    let mut unrestated: Vec<usize> = Vec::new();
    let mut next = 0;
    while next < subjects || !unrestated.is_empty() {
        let turn = turns.len() as u32;
        let restate = !unrestated.is_empty() && (next == subjects || turn % 3 == 2);
        let user = if restate {
            let pick = draw(
                crate::stream::ChoiceKind::RestatementTarget,
                format!("turn:{turn}"),
                0,
            ) % unrestated.len() as u64;
            let pair = &mut pairs[unrestated.remove(pick as usize)];
            pair.live_value = value(format!("live:{}", pair.task));
            pair.restatement = format!(
                "Change of plan: set the {} to {} instead.",
                pair.subject, pair.live_value
            );
            pair.restating_turn = turn;
            pair.restatement.clone()
        } else {
            let (name, key) = subject(next);
            let task = format!("stale-{next}");
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
                stale_turn: turn,
                restating_turn: 0,
            });
            unrestated.push(next);
            next += 1;
            stale_statement
        };
        turns.push(FactTurn {
            user,
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
pub fn with_parts(request: &Value, parts: &[PartText]) -> Value {
    let mut request = request.clone();
    for PartText { at, text } in parts {
        match &mut request["messages"][at.message]["content"] {
            Value::String(old) => *old = text.clone(),
            content => content[at.part]["text"] = Value::String(text.clone()),
        }
    }
    request
}

/// The tier (`1..=4`) a segment rendered at: the first tier whose stored text
/// is the rendered body. `None` when no stored segment matches the heading.
fn rendered_tier(history: &str, stale: ServedSpan, segments: &[SegmentTiers]) -> Option<u8> {
    let heading = history[..stale.start].rfind("## ")?;
    let (title, body) = history[heading + 3..].split_once('\n')?;
    let range = title.split(' ').next()?;
    let (start, end) = range.split_once('-')?;
    let (start, end) = (start.parse::<i64>().ok()?, end.parse::<i64>().ok()?);
    let body_end = SEGMENT_ENDS
        .iter()
        .filter_map(|end| body.find(end))
        .min()
        .unwrap_or(body.len());
    let body = body[..body_end].trim();
    let segment = segments
        .iter()
        .find(|s| s.start_message == start && s.end_message == end)?;
    [&segment.p1, &segment.p2, &segment.p3, &segment.p4]
        .iter()
        .position(|tier| tier.as_deref().map(str::trim) == Some(body))
        .map(|index| index as u8 + 1)
}

/// One stale-preference pair as the export carries it: the question, the
/// claim key its arms name, both values, the restating message's ordinal,
/// the tier the stale segment rendered at, what the served request delivered,
/// the served request itself (arm (a)), the part holding the stale value and
/// its span there, and arms (b) through (e) as the parts they replace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StalePair {
    pub task: String,
    pub question: String,
    pub key: String,
    pub stale_value: String,
    pub live_value: String,
    pub restating_ordinal: u64,
    /// `1..=4`, or `0` when the rendered body matched no stored tier.
    pub stale_tier: u8,
    pub delivery: StaleDelivery,
    pub history: HistoryAt,
    pub stale_span: ServedSpan,
    pub request: Value,
    pub arms: Arms,
}

/// A pair whose served history does not hold the stale value, so no
/// rendering can change it: what the whole request delivered, and the
/// request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unlocatable {
    pub delivery: StaleDelivery,
    pub request: Value,
}

/// The stale-preference export over one harness run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaleExport {
    pub schema: String,
    pub harness: String,
    pub summarizer: String,
    pub root_seed: String,
    pub pairs: Vec<StalePair>,
    pub unlocatable: BTreeMap<String, Unlocatable>,
    /// Pairs whose request carried the stale value anywhere, located or not.
    pub stale_delivered: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureError {
    Schema {
        found: String,
    },
    /// A pair of the world has no captured request.
    MissingRequest {
        task: String,
    },
}

debug_display!(CaptureError);

/// The export of one capture. A pair is located when the served history (the
/// m0 part, then the m1 part) carries the stale value, the one token of the
/// stale statement a summarizer that paraphrases keeps; that part is the one
/// arms (c) through (e) vary, and the value's span is what (d) and (e)
/// replace. The delivery is
/// over every text part of the request: the history, the raw tail, and the
/// search hint.
pub fn export_capture(capture: &StaleCapture) -> Result<StaleExport, CaptureError> {
    if capture.schema != STALE_CAPTURE_SCHEMA || capture.world.schema != STALE_WORLD_SCHEMA {
        return Err(CaptureError::Schema {
            found: capture.schema.clone(),
        });
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
    for pair in &capture.world.pairs {
        let request =
            capture
                .requests
                .get(&pair.task)
                .ok_or_else(|| CaptureError::MissingRequest {
                    task: pair.task.clone(),
                })?;
        let parts = text_parts(request);
        let served: String = parts
            .iter()
            .map(|(_, text)| *text)
            .collect::<Vec<_>>()
            .join("\n");
        let delivery = StaleDelivery::of(&served, &pair.stale_value, &pair.live_value);
        export.stale_delivered += u32::from(delivery.stale_delivered());
        let served_history = Served::of(&parts);
        let located = served_history.locate(&pair.stale_value);
        let Some((history, text, stale_span)) = located else {
            export.unlocatable.insert(
                pair.task.clone(),
                Unlocatable {
                    delivery,
                    request: request.clone(),
                },
            );
            continue;
        };
        // Every turn is one user and one assistant message, numbered from one.
        let restating_ordinal = 2 * u64::from(pair.restating_turn) + 1;
        export.pairs.push(StalePair {
            task: pair.task.clone(),
            question: pair.question.clone(),
            key: pair.key.clone(),
            stale_value: pair.stale_value.clone(),
            live_value: pair.live_value.clone(),
            restating_ordinal,
            stale_tier: rendered_tier(text, stale_span, &capture.segments).unwrap_or(0),
            delivery,
            history,
            stale_span,
            request: request.clone(),
            arms: arms(
                served_history,
                history,
                stale_span,
                &pair.key,
                &pair.live_value,
                restating_ordinal,
            ),
        });
    }
    Ok(export)
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

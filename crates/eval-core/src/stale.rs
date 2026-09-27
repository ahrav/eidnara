//! Stale preference: what a surface hands the model when a correction
//! coexists with the statement it supersedes, the five M0 renderings of one
//! served context, the knowledge-update grade of a live answer, and the
//! paired exact McNemar test between two renderings. Every function is pure
//! and makes no model call.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

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
pub fn locate(text: &str, phrase: &str) -> Option<TextSpan> {
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
        .map(|(start, _)| TextSpan {
            start,
            end: start + phrase.len(),
        })
}

/// A byte range of a served text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextSpan {
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

/// The five M0 renderings of one served context: (a) today's bytes, (b) the
/// override sentence after the stale statement, (c) one D-7 footer line after
/// it with the stale prose kept, (d) the stale statement replaced by the D-7
/// marker, and (e) the stale statement removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Arms {
    pub today: String,
    pub precedence_line: String,
    pub footer: String,
    pub anchored_replacement: String,
    pub omission_oracle: String,
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

/// Builds the five arms from arm (a)'s text and the stale statement's span in
/// it. Every arm differs from (a) only at that span. `key` must satisfy the
/// D-7 grammar and the span must lie on character boundaries of `served`.
pub fn arms(served: &str, stale: TextSpan, key: &str, live: &str, live_ordinal: u64) -> Arms {
    assert!(is_claim_key(key), "{key:?} is not a claim key");
    let (before, statement, after) = (
        &served[..stale.start],
        &served[stale.start..stale.end],
        &served[stale.end..],
    );
    let with = |replacement: &str| format!("{before}{replacement}{after}");
    Arms {
        today: served.to_string(),
        precedence_line: with(&format!("{statement}\n{PRECEDENCE_SENTENCE}")),
        footer: with(&format!(
            "{statement}\n[corrections: {key} = {live} @{live_ordinal}]"
        )),
        anchored_replacement: with(&format!("[corrected @{live_ordinal}: {key} = {live}]")),
        omission_oracle: with(""),
    }
}

/// One stale-preference pair as the campaign exports it: the task's question,
/// the claim key its arms name, arm (a)'s served text, the stale statement's
/// value and span, the live value and its span when served, the restating
/// message's ordinal, the stale segment's rendered tier, what the served text
/// delivered, and the five arms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StalePair {
    pub task: String,
    pub question: String,
    pub key: String,
    pub stale_value: String,
    pub stale_span: TextSpan,
    pub live_value: String,
    pub live_span: Option<TextSpan>,
    pub restating_ordinal: u64,
    /// `1..=4`, the tier the stale segment renders at in m0.
    pub stale_tier: u8,
    pub delivery: StaleDelivery,
    pub arms: Arms,
}

/// A campaign's stale-preference export. A pair whose stale statement the
/// served text does not carry has no arms and is listed under `unlocatable`
/// with its delivery, never dropped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaleExport {
    pub schema: String,
    pub generator_version: String,
    pub pairs: Vec<StalePair>,
    pub unlocatable: BTreeMap<String, StaleDelivery>,
    /// Pairs whose served text carried the stale value, located or not.
    pub stale_delivered: u32,
}

/// Discordant counts of two arms over the same pairs and the exact two-sided
/// binomial decision at the pre-registered alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McNemar {
    pub pairs: u32,
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
}

debug_display!(McNemarError);

/// Paired exact McNemar over an event (for example "answered stale") per
/// pair on two arms, in exact integer arithmetic.
pub fn mcnemar(
    first: &BTreeMap<String, bool>,
    second: &BTreeMap<String, bool>,
    alpha: Ratio,
) -> Result<McNemar, McNemarError> {
    if !first.keys().eq(second.keys()) {
        return Err(McNemarError::UnpairedArms);
    }
    if alpha <= Ratio::ZERO || alpha >= Ratio::ONE {
        return Err(McNemarError::AlphaOutOfRange);
    }
    let count = |a: bool, b: bool| {
        first
            .iter()
            .filter(|(id, x)| **x == a && second[*id] == b)
            .count() as u32
    };
    let (b, c) = (count(true, false), count(false, true));
    let n = (b + c) as usize;
    // Row n of Pascal's triangle: the tail below min(b, c) over 2^n.
    let mut row: Vec<Big> = vec![vec![1]];
    for _ in 0..n {
        let mut next = vec![vec![1]];
        for pair in row.windows(2) {
            next.push(add(&pair[0], &pair[1]));
        }
        next.push(vec![1]);
        row = next;
    }
    let sum = |items: &[Big]| items.iter().fold(vec![0], |acc, item| add(&acc, item));
    let tail = sum(&row[..=b.min(c) as usize]);
    let total = sum(&row);
    let (numerator, denominator) = alpha.parts();
    let lhs = mul_small(&tail, 2 * denominator as u64);
    let rhs = mul_small(&total, numerator as u64);
    Ok(McNemar {
        pairs: first.len() as u32,
        first_only: b,
        second_only: c,
        reject: compare(&lhs, &rhs) != Ordering::Greater,
    })
}

/// A non-negative integer as little-endian 64-bit limbs.
type Big = Vec<u64>;

fn add(a: &[u64], b: &[u64]) -> Big {
    let mut out = Vec::with_capacity(a.len().max(b.len()) + 1);
    let mut carry = 0u128;
    for i in 0..a.len().max(b.len()) {
        let sum = u128::from(*a.get(i).unwrap_or(&0)) + u128::from(*b.get(i).unwrap_or(&0)) + carry;
        out.push(sum as u64);
        carry = sum >> 64;
    }
    if carry > 0 {
        out.push(carry as u64);
    }
    out
}

fn mul_small(a: &[u64], m: u64) -> Big {
    let mut out = Vec::with_capacity(a.len() + 1);
    let mut carry = 0u128;
    for limb in a {
        let product = u128::from(*limb) * u128::from(m) + carry;
        out.push(product as u64);
        carry = product >> 64;
    }
    if carry > 0 {
        out.push(carry as u64);
    }
    out
}

fn compare(a: &[u64], b: &[u64]) -> Ordering {
    let trim = |x: &[u64]| x.len() - x.iter().rev().take_while(|l| **l == 0).count();
    let (la, lb) = (trim(a), trim(b));
    la.cmp(&lb)
        .then_with(|| a[..la].iter().rev().cmp(b[..lb].iter().rev()))
}

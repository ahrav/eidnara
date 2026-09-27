//! Stale preference: what a surface hands the model when a correction
//! coexists with the statement it supersedes, the five M0 renderings of one
//! served context, the knowledge-update grade of a live answer, and the
//! paired exact McNemar test between two renderings. Every function is pure
//! and makes no model call.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::generator::WorldConfig;
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

/// The five M0 renderings of one served context: (a) today's bytes, (b) the
/// override sentence on a line after the stale statement's line, (c) one D-7
/// footer line there with the stale prose kept, (d) the stale statement
/// replaced by the D-7 marker, and (e) the stale statement removed.
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
/// it. (d) and (e) differ from (a) only at that span; (b) and (c) add one line
/// after the line holding it, where D-7 appends a footer to the stale
/// segment's body. `key` must satisfy the D-7 grammar and the span must lie
/// on character boundaries of `served`.
pub fn arms(served: &str, stale: ServedSpan, key: &str, live: &str, live_ordinal: u64) -> Arms {
    assert!(is_claim_key(key), "{key:?} is not a claim key");
    let line_end = served[stale.end..]
        .find('\n')
        .map_or(served.len(), |at| stale.end + at);
    let after_line = |line: &str| format!("{}\n{line}{}", &served[..line_end], &served[line_end..]);
    let at_span = |replacement: &str| {
        format!(
            "{}{replacement}{}",
            &served[..stale.start],
            &served[stale.end..]
        )
    };
    Arms {
        today: served.to_string(),
        precedence_line: after_line(PRECEDENCE_SENTENCE),
        footer: after_line(&format!("[corrections: {key} = {live} @{live_ordinal}]")),
        anchored_replacement: at_span(&format!("[corrected @{live_ordinal}: {key} = {live}]")),
        omission_oracle: at_span(""),
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
    pub stale_span: ServedSpan,
    pub live_value: String,
    pub live_span: Option<ServedSpan>,
    pub restating_ordinal: u64,
    /// `1..=5` (`5` archived), the tier the stale segment renders at in m0.
    pub stale_tier: u8,
    pub delivery: StaleDelivery,
    pub arms: Arms,
}

/// A pair whose served text does not carry the stale value: what it
/// delivered and the text itself, empty when the surface served nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unlocatable {
    pub delivery: StaleDelivery,
    pub served: String,
}

/// What a pair is before its served text is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleQuestion {
    pub task: String,
    pub question: String,
    pub key: String,
    pub stale_value: String,
    pub live_value: String,
    pub restating_ordinal: u64,
}

/// A campaign's stale-preference export over one generated world. A pair
/// whose stale statement the served text does not carry has no arms and is
/// listed under `unlocatable`, never dropped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaleExport {
    pub schema: String,
    pub generator_version: String,
    /// The world's seed as a decimal, which canonical JSON cannot carry as a
    /// number, and its config: every pair comes from this one world.
    pub root_seed: String,
    pub config: WorldConfig,
    pub pairs: Vec<StalePair>,
    pub unlocatable: BTreeMap<String, Unlocatable>,
    /// Pairs whose served text carried the stale value, located or not.
    pub stale_delivered: u32,
}

impl StaleExport {
    pub fn new(root_seed: u64, config: WorldConfig) -> Self {
        Self {
            schema: STALE_EXPORT_SCHEMA.to_string(),
            generator_version: crate::generator::GENERATOR_VERSION.to_string(),
            root_seed: root_seed.to_string(),
            config,
            pairs: Vec::new(),
            unlocatable: BTreeMap::new(),
            stale_delivered: 0,
        }
    }

    /// Classifies one pair by its served text: exported with its arms when
    /// the text carries the stale value, unlocatable otherwise. `stale_tier`
    /// is read only for a located pair.
    pub fn record(&mut self, pair: StaleQuestion, served: &str, stale_tier: impl FnOnce() -> u8) {
        let delivery = StaleDelivery::of(served, &pair.stale_value, &pair.live_value);
        self.stale_delivered += u32::from(delivery.stale_delivered());
        let Some(stale_span) = locate(served, &pair.stale_value) else {
            self.unlocatable.insert(
                pair.task,
                Unlocatable {
                    delivery,
                    served: served.to_string(),
                },
            );
            return;
        };
        self.pairs.push(StalePair {
            arms: arms(
                served,
                stale_span,
                &pair.key,
                &pair.live_value,
                pair.restating_ordinal,
            ),
            live_span: locate(served, &pair.live_value),
            stale_tier: stale_tier(),
            stale_span,
            delivery,
            task: pair.task,
            question: pair.question,
            key: pair.key,
            stale_value: pair.stale_value,
            live_value: pair.live_value,
            restating_ordinal: pair.restating_ordinal,
        });
    }
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
    /// Pairs with no outcome on either arm (a failed or censored call):
    /// reported, never scored.
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
    /// The exact tail left 128-bit range (more than about 120 discordant
    /// pairs).
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
    // ponytail: 128-bit tail, refused past about 120 discordant pairs; move
    // to limbs if a campaign ever gets there.
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

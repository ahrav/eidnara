//! A deterministic lexical-overlap scorer: it ranks one descriptor text against
//! decision summaries by the share of each summary's terms the descriptor
//! contains.
//!
//! Both texts pass through the retrieval analyzer, `analyze_segments`, under
//! explicit bounds. A text's terms are the set of its atoms and parts, each
//! folded with `str::to_lowercase`, keeping terms of at least
//! [`MIN_TERM_SCALARS`] scalar values that hold an alphanumeric scalar, are not
//! all numeric, and are not in [`STOPWORDS`]. A summary scores
//! `|descriptor ∩ summary| / |summary|`, held as the integers `matched_terms`
//! and `decision_terms` and compared by cross-multiplication. A candidate needs
//! a score of at least [`CANDIDATE_SCORE_THRESHOLD`], at least
//! [`MIN_MATCHED_TERMS`] matched terms, and at least [`MIN_DECISION_TERMS`]
//! summary terms. At most [`CANDIDATES_PER_DESCRIPTOR`] candidates are kept, in
//! the total order score descending, `matched_terms` descending,
//! `decision_terms` ascending, decision object id bytes ascending.
//!
//! A descriptor over its bound is refused whole. A summary over its bound, or
//! with no term left after the floor, is reported unscorable and excluded; the
//! other summaries still score.
//!
//! [`matcher_digest`] versions these rules: its preimage names the matcher
//! epoch, the analyzer's contract epoch, the toolchain Unicode version, the
//! fold rule, the floor, the stopword list, the score formula, the thresholds,
//! K, and both bounds.
//!
//! The constant values are scaffolding under an assumed model limitation: they
//! reject one- and two-word coincidences on short summaries while a 64 KiB
//! descriptor still reaches a full score on a contained summary. The
//! out-of-tree measurement campaign (#1007) revises them under a new
//! [`CANDIDATE_MATCHER_EPOCH`].

use std::collections::BTreeSet;
use std::num::NonZeroUsize;
use std::sync::LazyLock;

use retrieval::lexical::{
    ANALYSIS_CONTRACT_EPOCH, LexicalBounds, LexicalRefusal, analyze_segments,
};
use sha2::{Digest, Sha256};

/// Changes whenever a matcher rule changes without changing another preimage field.
pub const CANDIDATE_MATCHER_EPOCH: &str = "contradiction-candidate-matcher.v1";
/// Terms shorter than this many scalar values are dropped.
pub const MIN_TERM_SCALARS: usize = 3;
/// A candidate's summary shares at least this many terms with the descriptor.
pub const MIN_MATCHED_TERMS: usize = 3;
/// A candidate's summary holds at least this many terms.
pub const MIN_DECISION_TERMS: usize = 4;
/// The least score a candidate reaches, as `(numerator, denominator)`.
pub const CANDIDATE_SCORE_THRESHOLD: (usize, usize) = (1, 2);
/// K: the most candidates one descriptor keeps.
pub const CANDIDATES_PER_DESCRIPTOR: usize = 4;
/// At least the 64 KiB publish unit bound, so a published descriptor is never refused. n atoms need at least 2n - 1 bytes, so an atom bound one past half the byte bound is never the binding refusal.
pub const DESCRIPTOR_BOUNDS: LexicalBounds = bounds(65_536);
pub const SUMMARY_BOUNDS: LexicalBounds = bounds(8_192);
/// The matcher's own copy of the user-hint gate's stopwords.
pub const STOPWORDS: [&str; 16] = [
    "and", "are", "but", "for", "from", "have", "into", "not", "that", "the", "this", "use", "was",
    "with", "you", "your",
];
const FOLD_RULE_ID: &str = "str-to-lowercase-per-term.v1";
const FLOOR_RULE_ID: &str = "min-scalars,alphanumeric,not-all-numeric,stopwords.v1";
const SCORE_FORMULA_ID: &str = "decision-side-containment.v1";

const fn bounds(max_input_bytes: usize) -> LexicalBounds {
    let (Some(max_input_bytes), Some(max_atoms)) = (
        NonZeroUsize::new(max_input_bytes),
        NonZeroUsize::new(max_input_bytes / 2 + 1),
    ) else {
        panic!("a bound is positive");
    };
    LexicalBounds {
        max_input_bytes,
        max_atoms,
    }
}

/// The folded, floored term set of `text` under `bounds`.
///
/// # Errors
///
/// The analyzer's refusal when `text` exceeds `bounds`.
pub fn term_set(text: &str, bounds: LexicalBounds) -> Result<BTreeSet<String>, LexicalRefusal> {
    let analysis = analyze_segments(&[text], bounds)?;
    Ok(analysis
        .atoms()
        .chain(analysis.parts())
        .map(str::to_lowercase)
        .filter(|term| kept(term))
        .collect())
}

fn kept(term: &str) -> bool {
    term.chars().count() >= MIN_TERM_SCALARS
        && term.chars().any(char::is_alphanumeric)
        && !term.chars().all(char::is_numeric)
        && !STOPWORDS.contains(&term)
}

/// One decision a descriptor is scored against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandidateInput<'a> {
    pub decision_object_id: &'a str,
    pub decision_id: &'a str,
    pub summary: &'a str,
}

/// One decision that passed every gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoredCandidate {
    pub decision_object_id: String,
    pub decision_id: String,
    pub matched_terms: usize,
    pub decision_terms: usize,
}

impl ScoredCandidate {
    /// The total order: score descending, `matched_terms` descending,
    /// `decision_terms` ascending, then the ids' bytes ascending.
    fn rank(&self, other: &Self) -> std::cmp::Ordering {
        let left = self.matched_terms * other.decision_terms;
        let right = other.matched_terms * self.decision_terms;
        right
            .cmp(&left)
            .then_with(|| other.matched_terms.cmp(&self.matched_terms))
            .then_with(|| self.decision_terms.cmp(&other.decision_terms))
            .then_with(|| self.decision_object_id.cmp(&other.decision_object_id))
            .then_with(|| self.decision_id.cmp(&other.decision_id))
    }
}

/// What scoring one summary found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryScore {
    /// The summary passed every gate.
    Candidate {
        matched_terms: usize,
        decision_terms: usize,
    },
    /// Analyzed; the overlap is below a gate, zero included.
    Below {
        matched_terms: usize,
        decision_terms: usize,
    },
    /// Over its bound or emptied by the floor.
    Unscorable,
}

/// One descriptor's analyzed terms, scored against one summary at a time so
/// only one summary's term set is resident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescriptorTerms(BTreeSet<String>);

impl DescriptorTerms {
    /// # Errors
    ///
    /// The analyzer's refusal when `descriptor` exceeds [`DESCRIPTOR_BOUNDS`]; the descriptor is refused whole.
    pub fn new(descriptor: &str) -> Result<Self, LexicalRefusal> {
        term_set(descriptor, DESCRIPTOR_BOUNDS).map(Self)
    }

    #[must_use]
    pub fn score(&self, summary: &str) -> SummaryScore {
        let Ok(terms) = term_set(summary, SUMMARY_BOUNDS) else {
            return SummaryScore::Unscorable;
        };
        if terms.is_empty() {
            return SummaryScore::Unscorable;
        }
        let matched_terms = terms.intersection(&self.0).count();
        let decision_terms = terms.len();
        let (numerator, denominator) = CANDIDATE_SCORE_THRESHOLD;
        if matched_terms >= MIN_MATCHED_TERMS
            && decision_terms >= MIN_DECISION_TERMS
            && matched_terms * denominator >= numerator * decision_terms
        {
            SummaryScore::Candidate {
                matched_terms,
                decision_terms,
            }
        } else {
            SummaryScore::Below {
                matched_terms,
                decision_terms,
            }
        }
    }
}

/// The best [`CANDIDATES_PER_DESCRIPTOR`] candidates seen so far, in rank order.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TopCandidates(Vec<ScoredCandidate>);

impl TopCandidates {
    /// Keeps `candidate` when it ranks within K; a later duplicate of a kept
    /// decision object id keeps the better-ranked entry.
    pub fn offer(&mut self, candidate: ScoredCandidate) {
        if let Some(index) = self
            .0
            .iter()
            .position(|kept| kept.decision_object_id == candidate.decision_object_id)
        {
            if candidate.rank(&self.0[index]).is_lt() {
                self.0.remove(index);
            } else {
                return;
            }
        }
        let at = self.0.partition_point(|kept| kept.rank(&candidate).is_lt());
        self.0.insert(at, candidate);
        self.0.truncate(CANDIDATES_PER_DESCRIPTOR);
    }

    #[must_use]
    pub fn into_vec(self) -> Vec<ScoredCandidate> {
        self.0
    }
}

/// One descriptor's result: the kept candidates in rank order, and the
/// decisions whose summaries were unscorable, in input order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescriptorScore {
    pub candidates: Vec<ScoredCandidate>,
    pub unscorable: Vec<String>,
}

/// Scores `descriptor` against every candidate.
///
/// # Errors
///
/// The analyzer's refusal when `descriptor` exceeds [`DESCRIPTOR_BOUNDS`].
pub fn score_descriptor(
    descriptor: &str,
    candidates: &[CandidateInput<'_>],
) -> Result<DescriptorScore, LexicalRefusal> {
    let terms = DescriptorTerms::new(descriptor)?;
    let mut top = TopCandidates::default();
    let mut unscorable = Vec::new();
    for candidate in candidates {
        match terms.score(candidate.summary) {
            SummaryScore::Candidate {
                matched_terms,
                decision_terms,
            } => top.offer(ScoredCandidate {
                decision_object_id: candidate.decision_object_id.to_string(),
                decision_id: candidate.decision_id.to_string(),
                matched_terms,
                decision_terms,
            }),
            SummaryScore::Below { .. } => {}
            SummaryScore::Unscorable => unscorable.push(candidate.decision_object_id.to_string()),
        }
    }
    Ok(DescriptorScore {
        candidates: top.into_vec(),
        unscorable,
    })
}

/// The matcher digest's preimage: `{len}:{field}\n` per field, names and values alternating.
#[must_use]
pub fn matcher_preimage() -> String {
    let (major, minor, update) = char::UNICODE_VERSION;
    let mut stopwords = String::new();
    for word in STOPWORDS {
        stopwords.push_str(&format!("{}:{word}\n", word.len()));
    }
    let stopwords = format!("{:x}", Sha256::digest(stopwords.as_bytes()));
    let (numerator, denominator) = CANDIDATE_SCORE_THRESHOLD;
    let fields = [
        ("matcher_epoch", CANDIDATE_MATCHER_EPOCH.to_string()),
        ("analysis_epoch", ANALYSIS_CONTRACT_EPOCH.to_string()),
        ("unicode", format!("{major}.{minor}.{update}")),
        ("fold", FOLD_RULE_ID.to_string()),
        ("floor", FLOOR_RULE_ID.to_string()),
        ("min_term_scalars", MIN_TERM_SCALARS.to_string()),
        ("stopwords", stopwords),
        ("score", SCORE_FORMULA_ID.to_string()),
        ("score_threshold", format!("{numerator}/{denominator}")),
        ("min_matched_terms", MIN_MATCHED_TERMS.to_string()),
        ("min_decision_terms", MIN_DECISION_TERMS.to_string()),
        (
            "candidates_per_descriptor",
            CANDIDATES_PER_DESCRIPTOR.to_string(),
        ),
        ("descriptor_bounds", bounds_field(DESCRIPTOR_BOUNDS)),
        ("summary_bounds", bounds_field(SUMMARY_BOUNDS)),
    ];
    let mut preimage = String::new();
    for (name, value) in fields {
        for field in [name, value.as_str()] {
            preimage.push_str(&format!("{}:{field}\n", field.len()));
        }
    }
    preimage
}

fn bounds_field(bounds: LexicalBounds) -> String {
    format!("{}/{}", bounds.max_input_bytes, bounds.max_atoms)
}

static MATCHER_DIGEST: LazyLock<String> =
    LazyLock::new(|| format!("{:x}", Sha256::digest(matcher_preimage().as_bytes())));

/// The lowercase hex SHA-256 of [`matcher_preimage`]; every candidate row stores it.
#[must_use]
pub fn matcher_digest() -> &'static str {
    &MATCHER_DIGEST
}

//! A deterministic lexical-overlap scorer: it ranks one descriptor text against
//! decision summaries by the share of each summary's terms the descriptor
//! contains.
//!
//! Both texts follow the retrieval analyzer's rules, `analyze_segments`, under
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
//! The gates count terms, and one identifier contributes its atom and each of
//! its parts, so a summary of one three-part identifier holds four terms.
//! `to_lowercase` is not full case folding: `İ` lowers to `i` and a combining
//! dot, and `ß` stays `ß`; both sides use the same rule, so only spellings
//! that lower to the same string match. NFC and NFD spellings are different
//! terms, and their scalar counts can differ at the floor. A decision object
//! id appears at most once in a result: the better-ranked entry wins, with
//! `decision_id` as the last key.
//!
//! [`term_set`] applies those rules to all-ASCII words itself and hands every
//! other word to the analyzer.
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

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::num::NonZeroUsize;
use std::sync::LazyLock;

use retrieval::lexical::{
    ANALYSIS_CONTRACT_EPOCH, Analysis, LexicalBounds, LexicalRefusal, analyze_segments,
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
/// At least the publish unit bound, so a published descriptor is never refused.
pub const DESCRIPTOR_BOUNDS: LexicalBounds = bounds(65_536);
const _: () = assert!(
    DESCRIPTOR_BOUNDS.max_input_bytes.get() >= crate::harness_sources::MAX_SOURCE_UNIT_BYTES
);
/// The bound one decision summary is analyzed under.
pub const SUMMARY_BOUNDS: LexicalBounds = bounds(8_192);
/// The matcher's own copy of the user-hint gate's stopwords.
pub const STOPWORDS: [&str; 16] = [
    "and", "are", "but", "for", "from", "have", "into", "not", "that", "the", "this", "use", "was",
    "with", "you", "your",
];
const TERM_SOURCE_ID: &str = "atoms-and-parts-as-a-set.v1";
const FOLD_RULE_ID: &str = "str-to-lowercase-per-term.v1";
const FLOOR_RULE_ID: &str = "min-scalars,alphanumeric,not-all-numeric,stopwords.v1";
const SCORE_FORMULA_ID: &str = "decision-side-containment.v1";
const ORDER_RULE_ID: &str =
    "score-desc,matched-desc,decision-terms-asc,object-id-asc,decision-id-asc,one-per-object.v1";

/// n atoms need at least 2n - 1 bytes, so an atom bound one past half the
/// byte bound is never the binding refusal.
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
    Ok(terms(&analysis)
        .into_iter()
        .map(|(_, term)| term.to_ascii_lowercase())
        .collect())
}

/// A kept term's [`hash`] and a spelling whose `to_ascii_lowercase` is the
/// folded term. An ASCII term keeps its analyzer bytes, and a non-ASCII term
/// holds its `str::to_lowercase`, which has no ASCII uppercase letter.
type Term<'a> = (u64, Cow<'a, str>);

/// The kept terms of `analysis` with repeats, atoms before parts.
fn terms(analysis: &Analysis) -> Vec<Term<'_>> {
    let mut terms = Vec::with_capacity(analysis.atoms().len() + analysis.parts().len());
    for term in analysis.atoms().chain(analysis.parts()) {
        match scan_ascii(term) {
            Scan::Dropped => {}
            Scan::Kept { hash } => terms.push((hash, Cow::Borrowed(term))),
            Scan::NotAscii => {
                let folded = term.to_lowercase();
                if kept(&folded) {
                    terms.push((hash(&folded), Cow::Owned(folded)));
                }
            }
        }
    }
    terms
}

fn folded_bytes(spelling: &str) -> impl Iterator<Item = u8> + '_ {
    spelling.bytes().map(|byte| byte.to_ascii_lowercase())
}

fn same(a: &Term<'_>, b: &Term<'_>) -> bool {
    a.0 == b.0 && a.1.eq_ignore_ascii_case(&b.1)
}

fn order(a: &Term<'_>, b: &Term<'_>) -> std::cmp::Ordering {
    a.0.cmp(&b.0)
        .then_with(|| folded_bytes(&a.1).cmp(folded_bytes(&b.1)))
}

/// The floor, applied to a folded term.
fn kept(term: &str) -> bool {
    term.chars().count() >= MIN_TERM_SCALARS
        && term.chars().any(char::is_alphanumeric)
        && !term.chars().all(char::is_numeric)
        && !STOPWORDS.contains(&term)
}

enum Scan {
    /// [`kept`] holds for the term's ASCII lowercase, whose [`hash`] is `hash`.
    Kept {
        hash: u64,
    },
    Dropped,
    NotAscii,
}

const ONES: u64 = 0x0101_0101_0101_0101;
const HIGH: u64 = ONES * 0x80;

/// The high bit of each byte of `word` in `low..=high`. A byte at or above
/// `0x80` can carry into its neighbour, so only an ASCII word gives exact bits.
const fn in_range(word: u64, low: u8, high: u8) -> u64 {
    word.wrapping_add(ONES * (0x80 - low as u64))
        & !word.wrapping_add(ONES * (0x7F - high as u64))
        & HIGH
}

const fn lowercase(word: u64) -> u64 {
    word | (in_range(word, b'A', b'Z') >> 2)
}

/// High bits marking the bytes seen so far that are not ASCII, that are ASCII
/// alphanumeric, and that are not ASCII digits.
#[derive(Default)]
struct Shape {
    not_ascii: u64,
    alphanumeric: u64,
    not_digit: u64,
}

impl Shape {
    /// `valid` holds the high bit of each byte of `word` that belongs to the term.
    fn add(&mut self, word: u64, valid: u64) {
        let digit = in_range(word, b'0', b'9');
        let letter = in_range(word, b'A', b'Z') | in_range(word, b'a', b'z');
        self.not_ascii |= word & valid;
        self.alphanumeric |= (letter | digit) & valid;
        self.not_digit |= !digit & valid;
    }
}

fn load(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().expect("eight bytes"))
}

/// A nonempty slice shorter than eight bytes as a little-endian word, zero
/// above its length. Overlapping loads write equal bytes to equal positions.
fn short_word(bytes: &[u8]) -> u64 {
    let length = bytes.len();
    let load4 = |at: usize| {
        u64::from(u32::from_le_bytes(
            bytes[at..at + 4].try_into().expect("four bytes"),
        ))
    };
    if length >= 4 {
        load4(0) | (load4(length - 4) << (8 * (length - 4)))
    } else {
        u64::from(bytes[0])
            | (u64::from(bytes[length / 2]) << (8 * (length / 2)))
            | (u64::from(bytes[length - 1]) << (8 * (length - 1)))
    }
}

/// Applies [`kept`] and [`hash`] to `term.to_ascii_lowercase()` in one pass
/// over eight-byte words, when `term` is ASCII. On ASCII input
/// `to_ascii_lowercase` and `str::to_lowercase` agree, so a kept ASCII term
/// needs no folded copy.
fn scan_ascii(term: &str) -> Scan {
    let bytes = term.as_bytes();
    let length = bytes.len();
    if length < MIN_TERM_SCALARS {
        return if term.is_ascii() {
            Scan::Dropped
        } else {
            Scan::NotAscii
        };
    }
    let mut shape = Shape::default();
    let mut hash = length as u64;
    let mut stopword = false;
    if length < 8 {
        let word = short_word(bytes);
        shape.add(word, HIGH >> (8 * (8 - length)));
        if shape.not_ascii != 0 {
            return Scan::NotAscii;
        }
        let word = lowercase(word);
        stopword = is_stopword(word | packed_length(length));
        hash = mix(hash, word);
    } else {
        let mut at = 0;
        while at + 8 <= length {
            let word = load(bytes, at);
            shape.add(word, HIGH);
            hash = mix(hash, lowercase(word));
            at += 8;
        }
        let rest = length - at;
        if rest > 0 {
            let word = load(bytes, length - 8) >> (8 * (8 - rest));
            shape.add(word, HIGH >> (8 * (8 - rest)));
            hash = mix(hash, lowercase(word));
        }
        if shape.not_ascii != 0 {
            return Scan::NotAscii;
        }
    }
    if shape.alphanumeric != 0 && shape.not_digit != 0 && !stopword {
        Scan::Kept { hash: finish(hash) }
    } else {
        Scan::Dropped
    }
}

/// The top byte holds the length, so words of at most seven bytes pack
/// distinctly.
const fn packed_length(length: usize) -> u64 {
    assert!(length < 8, "a packed word holds at most seven bytes");
    (length as u64) << 56
}

const PACKED_STOPWORDS: [u64; STOPWORDS.len()] = {
    let mut packed = [0; STOPWORDS.len()];
    let mut index = 0;
    while index < STOPWORDS.len() {
        let bytes = STOPWORDS[index].as_bytes();
        let mut word = packed_length(bytes.len());
        let mut at = 0;
        while at < bytes.len() {
            word |= (bytes[at] as u64) << (8 * at);
            at += 1;
        }
        packed[index] = word;
        index += 1;
    }
    packed
};

/// A multiplier whose product's top six bits send every packed stopword to
/// its own slot of [`STOPWORD_SLOTS`]; the build fails when the search finds
/// none.
const STOPWORD_MULTIPLIER: u64 = {
    let mut odd = 1u64;
    loop {
        assert!(odd < 1 << 16, "no stopword multiplier separates the list");
        let multiplier = odd.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let mut taken = 0u64;
        let mut index = 0;
        while index < PACKED_STOPWORDS.len() {
            let slot = PACKED_STOPWORDS[index].wrapping_mul(multiplier) >> 58;
            if taken & (1 << slot) != 0 {
                break;
            }
            taken |= 1 << slot;
            index += 1;
        }
        if index == PACKED_STOPWORDS.len() {
            break multiplier;
        }
        odd += 2;
    }
};

/// Each stopword at its slot. Every other slot holds 0, which no packed word
/// equals because its length byte is at least 1.
const STOPWORD_SLOTS: [u64; 64] = {
    let mut slots = [0; 64];
    let mut index = 0;
    while index < PACKED_STOPWORDS.len() {
        let packed = PACKED_STOPWORDS[index];
        slots[(packed.wrapping_mul(STOPWORD_MULTIPLIER) >> 58) as usize] = packed;
        index += 1;
    }
    slots
};

fn is_stopword(packed: u64) -> bool {
    STOPWORD_SLOTS[(packed.wrapping_mul(STOPWORD_MULTIPLIER) >> 58) as usize] == packed
}

const fn mix(hash: u64, word: u64) -> u64 {
    (hash.rotate_left(5) ^ word).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

const fn finish(hash: u64) -> u64 {
    hash ^ (hash >> 32)
}

/// Mixes the byte length, then each eight-byte little-endian word with the
/// tail zero padded. [`scan_ascii`] computes the same value from an ASCII
/// term's lowercase words, so every spelling of one folded term hashes alike.
fn hash(term: &str) -> u64 {
    let (words, rest) = term.as_bytes().as_chunks::<8>();
    let mut hash = words.iter().fold(term.len() as u64, |hash, word| {
        mix(hash, u64::from_le_bytes(*word))
    });
    if !rest.is_empty() {
        let mut word = [0u8; 8];
        word[..rest.len()].copy_from_slice(rest);
        hash = mix(hash, u64::from_le_bytes(word));
    }
    finish(hash)
}

fn bucket(hash: u64, shift: u32) -> usize {
    hash.checked_shr(shift).map_or(0, |bucket| bucket as usize)
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

/// Whether a summary of `decision_terms` terms, `matched_terms` of them in the descriptor, is a candidate.
pub(crate) fn passes_gates(matched_terms: usize, decision_terms: usize) -> bool {
    let (numerator, denominator) = CANDIDATE_SCORE_THRESHOLD;
    matched_terms >= MIN_MATCHED_TERMS
        && decision_terms >= MIN_DECISION_TERMS
        && matched_terms * denominator >= numerator * decision_terms
}

/// One descriptor's analyzed terms, scored against one summary at a time so
/// only one summary's term set is resident.
///
/// The distinct terms are sorted by term hash, then text, and `starts` splits
/// them into a power-of-two number of buckets by the hash's top `64 - shift`
/// bits. A lookup binary-searches one bucket, so colliding terms cost at most
/// a logarithmic search. `filter` sets the bit a term's low hash bits select,
/// at eight or more bits per term, so most absent terms stop at one bit test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescriptorTerms {
    terms: Vec<(u64, Box<str>)>,
    starts: Vec<u32>,
    shift: u32,
    filter: Vec<u64>,
}

impl DescriptorTerms {
    /// # Errors
    ///
    /// The analyzer's refusal when `descriptor` exceeds [`DESCRIPTOR_BOUNDS`]; the descriptor is refused whole.
    pub fn new(descriptor: &str) -> Result<Self, LexicalRefusal> {
        let analysis = analyze_segments(&[descriptor], DESCRIPTOR_BOUNDS)?;
        let mut distinct = terms(&analysis);
        distinct.sort_unstable_by(order);
        distinct.dedup_by(|a, b| same(a, b));
        let bits = distinct.len().next_power_of_two().trailing_zeros();
        let shift = u64::BITS - bits;
        let mut starts = vec![0u32; (1 << bits) + 1];
        for &(hash, _) in &distinct {
            starts[bucket(hash, shift) + 1] += 1;
        }
        for index in 1..starts.len() {
            starts[index] += starts[index - 1];
        }
        let mut filter = vec![0u64; (distinct.len() * 8).next_power_of_two().div_ceil(64)];
        let mask = filter.len() * 64 - 1;
        for &(hash, _) in &distinct {
            let bit = hash as usize & mask;
            filter[bit / 64] |= 1 << (bit % 64);
        }
        let terms = distinct
            .into_iter()
            .map(|(hash, term)| (hash, term.to_ascii_lowercase().into_boxed_str()))
            .collect();
        Ok(Self {
            terms,
            starts,
            shift,
            filter,
        })
    }

    fn contains(&self, hash: u64, term: &str) -> bool {
        let bit = hash as usize & (self.filter.len() * 64 - 1);
        if self.filter[bit / 64] & (1 << (bit % 64)) == 0 {
            return false;
        }
        let bucket = bucket(hash, self.shift);
        let within = self.starts[bucket] as usize..self.starts[bucket + 1] as usize;
        self.terms[within]
            .binary_search_by(|(kept, text)| {
                kept.cmp(&hash)
                    .then_with(|| text.bytes().cmp(folded_bytes(term)))
            })
            .is_ok()
    }

    /// `(matched, distinct)` for a summary of ASCII terms, or `None` when
    /// `scan_ascii` returns `Scan::NotAscii` or more than `LIMIT` kept terms
    /// are distinct under ASCII case folding. The table holds at most `LIMIT`
    /// entries in `SLOTS = 2 * LIMIT` slots, so a probe passes at most `LIMIT`
    /// occupied slots.
    fn count_ascii(&self, analysis: &Analysis) -> Option<(usize, usize)> {
        const LIMIT: usize = 64;
        const SLOTS: usize = 2 * LIMIT;
        const EMPTY: u8 = u8::MAX;
        let mut slots = [EMPTY; SLOTS];
        let mut kept = [(0u64, ""); LIMIT];
        let (mut matched, mut distinct) = (0, 0);
        for term in analysis.atoms().chain(analysis.parts()) {
            let hash = match scan_ascii(term) {
                Scan::Dropped => continue,
                Scan::Kept { hash } => hash,
                Scan::NotAscii => return None,
            };
            let mut slot = hash as usize % SLOTS;
            let fresh = loop {
                match slots[slot] {
                    EMPTY => break true,
                    seen => {
                        let (seen_hash, seen_term) = kept[usize::from(seen)];
                        if seen_hash == hash && seen_term.eq_ignore_ascii_case(term) {
                            break false;
                        }
                        slot = (slot + 1) % SLOTS;
                    }
                }
            };
            if fresh {
                if distinct == LIMIT {
                    return None;
                }
                slots[slot] = u8::try_from(distinct).expect("at most LIMIT terms");
                kept[distinct] = (hash, term);
                distinct += 1;
                matched += usize::from(self.contains(hash, term));
            }
        }
        Some((matched, distinct))
    }

    #[must_use]
    pub fn score(&self, summary: &str) -> SummaryScore {
        let Ok(analysis) = analyze_segments(&[summary], SUMMARY_BOUNDS) else {
            return SummaryScore::Unscorable;
        };
        let (matched_terms, decision_terms) = match self.count_ascii(&analysis) {
            Some(counts) => counts,
            None => {
                let mut terms = terms(&analysis);
                terms.sort_unstable_by(order);
                terms.dedup_by(|a, b| same(a, b));
                let matched = terms
                    .iter()
                    .filter(|(hash, term)| self.contains(*hash, term))
                    .count();
                (matched, terms.len())
            }
        };
        if decision_terms == 0 {
            return SummaryScore::Unscorable;
        }
        if passes_gates(matched_terms, decision_terms) {
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
    /// Scores one decision's summary against `terms` and keeps it when it passes
    /// every gate and ranks within K; the score is returned so the caller can
    /// count unscorable summaries.
    pub fn record(&mut self, terms: &DescriptorTerms, input: CandidateInput<'_>) -> SummaryScore {
        let score = terms.score(input.summary);
        if let SummaryScore::Candidate {
            matched_terms,
            decision_terms,
        } = score
        {
            self.offer(ScoredCandidate {
                decision_object_id: input.decision_object_id.to_string(),
                decision_id: input.decision_id.to_string(),
                matched_terms,
                decision_terms,
            });
        }
        score
    }

    /// A later duplicate of a kept decision object id keeps the better-ranked entry.
    pub(crate) fn offer(&mut self, candidate: ScoredCandidate) {
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
/// decisions whose summaries were unscorable, in id order.
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
        if top.record(&terms, *candidate) == SummaryScore::Unscorable {
            unscorable.push(candidate.decision_object_id.to_string());
        }
    }
    unscorable.sort_unstable();
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
        push_field(&mut stopwords, word);
    }
    let stopwords = format!("{:x}", Sha256::digest(stopwords.as_bytes()));
    let (numerator, denominator) = CANDIDATE_SCORE_THRESHOLD;
    let fields = [
        ("matcher_epoch", CANDIDATE_MATCHER_EPOCH.to_string()),
        ("analysis_epoch", ANALYSIS_CONTRACT_EPOCH.to_string()),
        ("unicode", format!("{major}.{minor}.{update}")),
        ("terms", TERM_SOURCE_ID.to_string()),
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
        ("order", ORDER_RULE_ID.to_string()),
        ("descriptor_bounds", bounds_field(DESCRIPTOR_BOUNDS)),
        ("summary_bounds", bounds_field(SUMMARY_BOUNDS)),
    ];
    let mut preimage = String::new();
    for (name, value) in fields {
        push_field(&mut preimage, name);
        push_field(&mut preimage, &value);
    }
    preimage
}

/// `{len}:{field}\n`, so `("a", "bc")` and `("ab", "c")` stay distinct.
fn push_field(out: &mut String, field: &str) {
    out.push_str(&format!("{}:{field}\n", field.len()));
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

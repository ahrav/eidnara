//! TerseTextCompression prose compression.
//!
//! Two artifacts define the byte-level output contract: the committed
//! differential fixture (`testdata/terse_text_compression-golden.json`) and the naive
//! reference model in `tests/terse_text_compression_reference.rs`, a direct port of the
//! TypeScript compressor the fixture was generated from. `compress` must
//! match both exactly; keep the transformation order and ASCII word-boundary
//! rules aligned with the reference.
//!
//! Pattern scanning uses prepared matchers over a lowercase byte shadow of the
//! working text: one packed multi-pattern searcher per dropped-phrase set and
//! for the auxiliaries, and one `memmem::Finder` per replacement pattern.
//! Sequential per-pattern pass order is semantically load-bearing: a pass can
//! create text that a later pattern matches ("due in order to the fact that"
//! shortens to "due to the fact that", which the next pattern shortens to
//! "because"), so the passes must not be merged into one automaton.

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind, packed};
use memchr::memmem::Finder;
use regex::Regex;
use std::borrow::Cow;
use std::sync::OnceLock;

/// Compression strength applied by [`compress`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerseTextCompressionLevel {
    Lite,
    Full,
    Ultra,
}

#[derive(Debug, Clone)]
struct PreservedRegion {
    placeholder: String,
    original: String,
    /// The original is input that spelled a placeholder; it is restored as
    /// itself and never expanded, since it names no minted region.
    literal: bool,
}

const FILLER_WORDS: &[&str] = &[
    "just",
    "really",
    "basically",
    "actually",
    "essentially",
    "simply",
    "clearly",
    "obviously",
    "quite",
    "very",
    "somewhat",
    "rather",
    "fairly",
    "sort of",
    "kind of",
    "a bit",
];

const HEDGING_PHRASES: &[&str] = &[
    "i think",
    "i believe",
    "i feel",
    "probably",
    "perhaps",
    "maybe",
    "it seems",
    "it appears",
    "arguably",
    "i suppose",
    "i guess",
];

const PLEASANTRIES: &[&str] = &["please", "thanks", "thank you", "kindly", "if possible"];

const AUXILIARIES: &[&str] = &[
    "was",
    "were",
    "is",
    "are",
    "am",
    "be",
    "been",
    "being",
    "has been",
    "had been",
    "have been",
    "will be",
    "would be",
    "could be",
    "should be",
    "might be",
    "may be",
];

const PHRASE_SHORTENINGS: &[(&str, &str)] = &[
    ("in order to", "to"),
    ("due to the fact that", "because"),
    ("at this point in time", "now"),
    ("at the moment", "now"),
    ("in the event that", "if"),
    ("for the purpose of", "for"),
    ("with regard to", "about"),
    ("in spite of the fact that", "though"),
    ("on the grounds that", "because"),
    ("for the reason that", "because"),
];

const ULTRA_CONNECTIVE_REPLACEMENTS: &[(&str, &str)] = &[
    ("and then", "→"),
    ("then after", "→"),
    ("afterwards", "→"),
    ("because of", "//"),
    ("therefore", "→"),
    ("because", "//"),
    ("however", "but"),
    ("furthermore", "+"),
    ("additionally", "+"),
    ("as well as", "+"),
    (" and ", " + "),
    (" or ", " | "),
];

const ULTRA_ABBREVIATIONS: &[(&str, &str)] = &[
    ("history_summarizer", "hist"),
    ("history_segment", "cmpt"),
    ("history_segments", "cmpts"),
    ("compressor", "cmp"),
    ("compression", "cmp"),
    ("context", "ctx"),
    ("message", "msg"),
    ("messages", "msgs"),
    ("session", "ses"),
    ("configuration", "cfg"),
    ("config", "cfg"),
    ("implementation", "impl"),
    ("implemented", "impl"),
    ("repository", "repo"),
    ("database", "db"),
    ("directory", "dir"),
];

fn is_ascii_word(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn previous_char(text: &str, offset: usize) -> Option<char> {
    text[..offset].chars().next_back()
}

fn next_char(text: &str, offset: usize) -> Option<char> {
    text[offset..].chars().next()
}

fn has_word_boundary_before(text: &str, offset: usize) -> bool {
    !previous_char(text, offset).is_some_and(is_ascii_word)
}

fn has_word_boundary_after(text: &str, offset: usize) -> bool {
    !next_char(text, offset).is_some_and(is_ascii_word)
}

fn ascii_eq_at(text: &str, offset: usize, needle: &str) -> bool {
    let Some(candidate) = text.get(offset..offset.saturating_add(needle.len())) else {
        return false;
    };
    candidate.len() == needle.len() && candidate.eq_ignore_ascii_case(needle)
}

/// Byte-level twin of `has_word_boundary_before`/`after`. All needles are
/// pure ASCII, so a match's edge offsets sit on char boundaries, and any
/// UTF-8 lead or continuation byte (>= 0x80) is non-word exactly like the
/// non-ASCII char it belongs to.
#[inline]
fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn phrase_automaton(patterns: &[&str]) -> AhoCorasick {
    AhoCorasickBuilder::new()
        .ascii_case_insensitive(true)
        .match_kind(MatchKind::LeftmostFirst)
        .build(patterns)
        .expect("static pattern set builds")
}

#[cfg(test)]
fn filler_automaton() -> &'static AhoCorasick {
    static A: OnceLock<AhoCorasick> = OnceLock::new();
    A.get_or_init(|| phrase_automaton(FILLER_WORDS))
}

#[cfg(test)]
fn hedging_automaton() -> &'static AhoCorasick {
    static A: OnceLock<AhoCorasick> = OnceLock::new();
    A.get_or_init(|| phrase_automaton(HEDGING_PHRASES))
}

#[cfg(test)]
fn pleasantries_automaton() -> &'static AhoCorasick {
    static A: OnceLock<AhoCorasick> = OnceLock::new();
    A.get_or_init(|| phrase_automaton(PLEASANTRIES))
}

/// Leftmost-first search for one dropped-phrase set over the lowercase shadow.
///
/// The patterns are lowercase ASCII, so an exact match on the shadow is a case-insensitive
/// match on the text at the same offsets. The packed (Teddy) searcher is used when the
/// crate can build one for the host; the DFA automaton over the same patterns otherwise.
struct PhraseSearcher {
    packed: Option<packed::Searcher>,
    automaton: AhoCorasick,
}

impl PhraseSearcher {
    fn new(patterns: &[&str]) -> Self {
        let packed = packed::Config::new()
            .match_kind(packed::MatchKind::LeftmostFirst)
            .builder()
            .extend(patterns)
            .build();
        PhraseSearcher {
            packed,
            automaton: phrase_automaton(patterns),
        }
    }

    fn find(&self, shadow: &[u8]) -> Option<(usize, usize)> {
        match &self.packed {
            Some(searcher) => searcher.find(shadow).map(|m| (m.start(), m.end())),
            None => self.automaton.find(shadow).map(|m| (m.start(), m.end())),
        }
    }
}

fn filler_searcher() -> &'static PhraseSearcher {
    static S: OnceLock<PhraseSearcher> = OnceLock::new();
    S.get_or_init(|| PhraseSearcher::new(FILLER_WORDS))
}

fn hedging_searcher() -> &'static PhraseSearcher {
    static S: OnceLock<PhraseSearcher> = OnceLock::new();
    S.get_or_init(|| PhraseSearcher::new(HEDGING_PHRASES))
}

fn pleasantries_searcher() -> &'static PhraseSearcher {
    static S: OnceLock<PhraseSearcher> = OnceLock::new();
    S.get_or_init(|| PhraseSearcher::new(PLEASANTRIES))
}

/// Start of the maximal Unicode-whitespace run ending at `end`, bounded below by `floor`.
fn whitespace_run_start(text: &str, end: usize, floor: usize) -> usize {
    let mut start = end;
    while start > floor {
        let ch = text[..start]
            .chars()
            .next_back()
            .expect("start > 0 within text");
        if ch.is_whitespace() {
            start -= ch.len_utf8();
        } else {
            break;
        }
    }
    start
}

/// `drop_phrases` over a shadowed buffer: the search runs on the shadow, the whitespace run
/// before each dropped phrase is read from the text, and both buffers are rewritten together.
fn drop_phrases_shadowed(buf: &mut ShadowedText, searcher: &PhraseSearcher) {
    let mut out: Option<(String, Vec<u8>)> = None;
    let mut pending = 0usize;
    let mut search = 0usize;
    while search < buf.shadow.len() {
        let Some((start, end)) = searcher.find(&buf.shadow[search..]) else {
            break;
        };
        let s = search + start;
        let e = search + end;
        let before_ok = s == 0 || !is_word_byte(buf.shadow[s - 1]);
        let after_ok = e >= buf.shadow.len() || !is_word_byte(buf.shadow[e]);
        if !(before_ok && after_ok) {
            search = s + 1;
            continue;
        }
        let run_start = whitespace_run_start(&buf.text, s, pending);
        let (out_text, out_shadow) = out.get_or_insert_with(|| {
            (
                String::with_capacity(buf.text.len()),
                Vec::with_capacity(buf.shadow.len()),
            )
        });
        out_text.push_str(&buf.text[pending..run_start]);
        out_shadow.extend_from_slice(&buf.shadow[pending..run_start]);
        pending = e;
        search = e;
    }
    if let Some((mut out_text, mut out_shadow)) = out {
        out_text.push_str(&buf.text[pending..]);
        out_shadow.extend_from_slice(&buf.shadow[pending..]);
        buf.text = out_text;
        buf.shadow = out_shadow;
    }
}

/// Drops every whole-word occurrence of the automaton's phrases, along with
/// the whitespace run immediately preceding each dropped phrase.
///
/// The automaton must be leftmost-first over a set in which no phrase is a
/// prefix of another (asserted by `pattern_set_invariants`): at most one
/// phrase can then match at a given start position, so the leftmost match is
/// the same one a position-by-position scan selects. A candidate that fails a
/// word-boundary check restarts the search one byte later, which keeps
/// overlapping later candidates reachable.
#[cfg(test)]
fn drop_phrases<'a>(text: &'a str, automaton: &AhoCorasick) -> Cow<'a, str> {
    let bytes = text.as_bytes();
    let mut out: Option<String> = None;
    // Bytes below `pending` are already emitted or dropped.
    let mut pending = 0usize;
    let mut search = 0usize;
    while search < bytes.len() {
        let Some(m) = automaton.find(&bytes[search..]) else {
            break;
        };
        let s = search + m.start();
        let e = search + m.end();
        let before_ok = s == 0 || !is_word_byte(bytes[s - 1]);
        let after_ok = e >= bytes.len() || !is_word_byte(bytes[e]);
        if !(before_ok && after_ok) {
            search = s + 1;
            continue;
        }
        // A dropped phrase absorbs the maximal Unicode-whitespace run
        // immediately before it, bounded by the previous drop point.
        let mut run_start = s;
        while run_start > pending {
            let ch = text[..run_start]
                .chars()
                .next_back()
                .expect("run_start > 0 within text");
            if ch.is_whitespace() {
                run_start -= ch.len_utf8();
            } else {
                break;
            }
        }
        let out_ref = out.get_or_insert_with(|| String::with_capacity(text.len()));
        out_ref.push_str(&text[pending..run_start]);
        pending = e;
        search = e;
    }
    match out {
        None => Cow::Borrowed(text),
        Some(mut o) => {
            o.push_str(&text[pending..]);
            Cow::Owned(o)
        }
    }
}

/// Working text plus a lowercase byte shadow kept in exact sync. ASCII
/// lowering is length-preserving, so shadow offsets equal text offsets, and
/// a case-insensitive needle search over the shadow is an exact byte search.
struct ShadowedText {
    text: String,
    shadow: Vec<u8>,
}

impl ShadowedText {
    fn new(text: String) -> Self {
        let shadow = text.bytes().map(|b| b.to_ascii_lowercase()).collect();
        Self { text, shadow }
    }
}

/// Replaces whole-word, ASCII-case-insensitive occurrences of the finder's
/// needle. With `uppercase_first`, a match whose first source byte is an
/// ASCII uppercase letter receives the replacement with its first letter
/// uppercased (the abbreviation-pass case rule). A pass with no verified
/// match leaves the buffers untouched.
fn replace_word_phrase(
    buf: &mut ShadowedText,
    finder: &Finder<'_>,
    needle_len: usize,
    replacement: &str,
    uppercase_first: bool,
) {
    let mut pos = 0usize;
    let mut pending = 0usize;
    let mut out: Option<(String, Vec<u8>)> = None;
    while pos < buf.shadow.len() {
        let Some(off) = finder.find(&buf.shadow[pos..]) else {
            break;
        };
        let s = pos + off;
        let e = s + needle_len;
        let before_ok = s == 0 || !is_word_byte(buf.shadow[s - 1]);
        let after_ok = e >= buf.shadow.len() || !is_word_byte(buf.shadow[e]);
        if !(before_ok && after_ok) {
            pos = s + 1;
            continue;
        }
        let (out_text, out_shadow) = out.get_or_insert_with(|| {
            (
                String::with_capacity(buf.text.len()),
                Vec::with_capacity(buf.text.len()),
            )
        });
        out_text.push_str(&buf.text[pending..s]);
        out_shadow.extend_from_slice(&buf.shadow[pending..s]);
        if uppercase_first && buf.text.as_bytes()[s].is_ascii_uppercase() {
            let mut cased = replacement.to_string();
            if let Some(first) = cased.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            out_text.push_str(&cased);
        } else {
            out_text.push_str(replacement);
        }
        out_shadow.extend(replacement.bytes().map(|b| b.to_ascii_lowercase()));
        pending = e;
        pos = e;
    }
    if let Some((mut out_text, mut out_shadow)) = out {
        out_text.push_str(&buf.text[pending..]);
        out_shadow.extend_from_slice(&buf.shadow[pending..]);
        buf.text = out_text;
        buf.shadow = out_shadow;
    }
}

/// Replaces case-sensitive literal occurrences of the finder's needle, with
/// no word-boundary requirement. The finder must scan the original text, not
/// the shadow, to preserve case sensitivity.
fn replace_literal_phrase(
    buf: &mut ShadowedText,
    finder: &Finder<'_>,
    needle_len: usize,
    replacement: &str,
) {
    let mut pos = 0usize;
    let mut pending = 0usize;
    let mut out: Option<(String, Vec<u8>)> = None;
    while pos < buf.text.len() {
        let Some(off) = finder.find(&buf.text.as_bytes()[pos..]) else {
            break;
        };
        let s = pos + off;
        let e = s + needle_len;
        let (out_text, out_shadow) = out.get_or_insert_with(|| {
            (
                String::with_capacity(buf.text.len()),
                Vec::with_capacity(buf.text.len()),
            )
        });
        out_text.push_str(&buf.text[pending..s]);
        out_shadow.extend_from_slice(&buf.shadow[pending..s]);
        out_text.push_str(replacement);
        out_shadow.extend(replacement.bytes().map(|b| b.to_ascii_lowercase()));
        pending = e;
        pos = e;
    }
    if let Some((mut out_text, mut out_shadow)) = out {
        out_text.push_str(&buf.text[pending..]);
        out_shadow.extend_from_slice(&buf.shadow[pending..]);
        buf.text = out_text;
        buf.shadow = out_shadow;
    }
}

/// Counts whole-word occurrences, stopping at `limit`. The only caller
/// compares the count against 3, so counting past the limit is wasted work.
fn count_word_occurrences(
    buf: &ShadowedText,
    finder: &Finder<'_>,
    needle_len: usize,
    limit: usize,
) -> usize {
    let mut pos = 0usize;
    let mut count = 0usize;
    while pos < buf.shadow.len() {
        let Some(off) = finder.find(&buf.shadow[pos..]) else {
            break;
        };
        let s = pos + off;
        let e = s + needle_len;
        let before_ok = s == 0 || !is_word_byte(buf.shadow[s - 1]);
        let after_ok = e >= buf.shadow.len() || !is_word_byte(buf.shadow[e]);
        if before_ok && after_ok {
            count += 1;
            if count >= limit {
                return count;
            }
            pos = e;
        } else {
            pos = s + 1;
        }
    }
    count
}

fn shortening_finders() -> &'static [Finder<'static>] {
    static F: OnceLock<Vec<Finder<'static>>> = OnceLock::new();
    F.get_or_init(|| {
        PHRASE_SHORTENINGS
            .iter()
            .map(|(phrase, _)| Finder::new(phrase.as_bytes()))
            .collect()
    })
}

fn connective_finders() -> &'static [Finder<'static>] {
    static F: OnceLock<Vec<Finder<'static>>> = OnceLock::new();
    F.get_or_init(|| {
        ULTRA_CONNECTIVE_REPLACEMENTS
            .iter()
            .map(|(phrase, _)| Finder::new(phrase.as_bytes()))
            .collect()
    })
}

fn abbreviation_finders() -> &'static [Finder<'static>] {
    static F: OnceLock<Vec<Finder<'static>>> = OnceLock::new();
    F.get_or_init(|| {
        ULTRA_ABBREVIATIONS
            .iter()
            .map(|(term, _)| Finder::new(term.as_bytes()))
            .collect()
    })
}

fn apply_phrase_shortenings(buf: &mut ShadowedText) {
    for (i, (phrase, replacement)) in PHRASE_SHORTENINGS.iter().enumerate() {
        replace_word_phrase(
            buf,
            &shortening_finders()[i],
            phrase.len(),
            replacement,
            false,
        );
    }
}

fn apply_ultra_connectives(buf: &mut ShadowedText) {
    for (i, (phrase, replacement)) in ULTRA_CONNECTIVE_REPLACEMENTS.iter().enumerate() {
        if phrase.starts_with(' ') && phrase.ends_with(' ') {
            replace_literal_phrase(buf, &connective_finders()[i], phrase.len(), replacement);
        } else {
            replace_word_phrase(
                buf,
                &connective_finders()[i],
                phrase.len(),
                replacement,
                false,
            );
        }
    }
}

fn apply_ultra_abbreviations(buf: &mut ShadowedText) {
    for (i, (term, abbreviation)) in ULTRA_ABBREVIATIONS.iter().enumerate() {
        if count_word_occurrences(buf, &abbreviation_finders()[i], term.len(), 3) < 3 {
            continue;
        }
        replace_word_phrase(
            buf,
            &abbreviation_finders()[i],
            term.len(),
            abbreviation,
            true,
        );
    }
}

fn protect_regex<'a>(
    text: &'a str,
    regex: &Regex,
    preserved: &mut Vec<PreservedRegion>,
) -> Cow<'a, str> {
    protect_regex_filtered(text, regex, preserved, false, |_, _, _| true)
}

/// Captures input that already spells a placeholder as a literal region.
fn protect_literal_placeholders<'a>(
    text: &'a str,
    preserved: &mut Vec<PreservedRegion>,
) -> Cow<'a, str> {
    static LITERAL_PLACEHOLDER: OnceLock<Regex> = OnceLock::new();
    protect_regex_filtered(
        text,
        LITERAL_PLACEHOLDER.get_or_init(|| Regex::new("\u{0}EIDNARA_PRES_[0-9]+\u{0}").unwrap()),
        preserved,
        true,
        |_, _, _| true,
    )
}

fn mint_placeholder(preserved: &mut Vec<PreservedRegion>, original: &str, literal: bool) -> String {
    let placeholder = format!("\u{0}EIDNARA_PRES_{}\u{0}", preserved.len());
    preserved.push(PreservedRegion {
        placeholder: placeholder.clone(),
        original: original.to_string(),
        literal,
    });
    placeholder
}

fn protect_regex_filtered<'a>(
    text: &'a str,
    regex: &Regex,
    preserved: &mut Vec<PreservedRegion>,
    literal: bool,
    accept: impl Fn(&str, usize, usize) -> bool,
) -> Cow<'a, str> {
    let mut output: Option<String> = None;
    let mut cursor = 0;
    for matched in regex.find_iter(text) {
        if !accept(text, matched.start(), matched.end()) {
            continue;
        }
        let output = output.get_or_insert_with(|| String::with_capacity(text.len()));
        output.push_str(&text[cursor..matched.start()]);
        output.push_str(&mint_placeholder(preserved, matched.as_str(), literal));
        cursor = matched.end();
    }
    match output {
        None => Cow::Borrowed(text),
        Some(mut output) => {
            output.push_str(&text[cursor..]);
            Cow::Owned(output)
        }
    }
}

fn protect_identifier_regions<'a>(
    text: &'a str,
    preserved: &mut Vec<PreservedRegion>,
) -> Cow<'a, str> {
    static IDENTIFIER: OnceLock<Regex> = OnceLock::new();
    let regex = IDENTIFIER.get_or_init(|| Regex::new(r"(?:msg|ses|toolu)_[A-Za-z0-9]+").unwrap());
    protect_regex_filtered(text, regex, preserved, false, |text, start, end| {
        has_word_boundary_before(text, start) && has_word_boundary_after(text, end)
    })
}

/// Protects each maximal run of 7 to 40 hex digits whose neighbours are not ASCII alphanumerics.
/// A longer run protects nothing: its 40-digit prefix borders a digit and its remainder borders
/// the prefix, which is what the regex `[0-9a-fA-F]{7,40}` with the same neighbour filter selects.
fn protect_hash_regions<'a>(text: &'a str, preserved: &mut Vec<PreservedRegion>) -> Cow<'a, str> {
    let bytes = text.as_bytes();
    let mut output: Option<String> = None;
    let mut pending = 0usize;
    // Every run of seven or more hex digits covers a probe position, so probing every seventh
    // byte finds each such run.
    let mut probe = 6usize;
    let mut scanned = 0usize;
    while probe < bytes.len() {
        if !bytes[probe].is_ascii_hexdigit() {
            probe += 7;
            continue;
        }
        let mut start = probe;
        while start > scanned && bytes[start - 1].is_ascii_hexdigit() {
            start -= 1;
        }
        let mut end = probe + 1;
        while end < bytes.len() && bytes[end].is_ascii_hexdigit() {
            end += 1;
        }
        scanned = end;
        probe = end + 6;
        let run = end - start;
        if !(7..=40).contains(&run)
            || (start > 0 && bytes[start - 1].is_ascii_alphanumeric())
            || (end < bytes.len() && bytes[end].is_ascii_alphanumeric())
        {
            continue;
        }
        let output = output.get_or_insert_with(|| String::with_capacity(text.len()));
        output.push_str(&text[pending..start]);
        output.push_str(&mint_placeholder(preserved, &text[start..end], false));
        pending = end;
    }
    match output {
        None => Cow::Borrowed(text),
        Some(mut output) => {
            output.push_str(&text[pending..]);
            Cow::Owned(output)
        }
    }
}

fn is_path_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-' | b'/')
}

/// `protect_regex` for the path pattern. A path match consists of path bytes and contains `/`,
/// so the regex runs only over each maximal run of path bytes that contains a `/`; leftmost-first
/// matching inside such a run equals matching over the whole text, since no match crosses a
/// non-path byte.
fn protect_paths<'a>(
    text: &'a str,
    regex: &Regex,
    preserved: &mut Vec<PreservedRegion>,
) -> Cow<'a, str> {
    let bytes = text.as_bytes();
    let mut output: Option<String> = None;
    let mut cursor = 0;
    let mut scanned = 0;
    for slash in memchr::memchr_iter(b'/', bytes) {
        if slash < scanned {
            continue;
        }
        let mut run_start = slash;
        while run_start > scanned && is_path_byte(bytes[run_start - 1]) {
            run_start -= 1;
        }
        let mut run_end = slash + 1;
        while run_end < bytes.len() && is_path_byte(bytes[run_end]) {
            run_end += 1;
        }
        scanned = run_end;
        for matched in regex.find_iter(&text[run_start..run_end]) {
            let start = run_start + matched.start();
            let output = output.get_or_insert_with(|| String::with_capacity(text.len()));
            output.push_str(&text[cursor..start]);
            output.push_str(&mint_placeholder(preserved, matched.as_str(), false));
            cursor = run_start + matched.end();
        }
    }
    match output {
        None => Cow::Borrowed(text),
        Some(mut output) => {
            output.push_str(&text[cursor..]);
            Cow::Owned(output)
        }
    }
}

fn protect_regions(text: &str) -> (String, Vec<PreservedRegion>) {
    let mut preserved = Vec::new();
    let mut working = Cow::Borrowed(text);

    fn advance<'a>(working: &mut Cow<'a, str>, pass: impl FnOnce(&str) -> Cow<'_, str>) {
        if let Cow::Owned(next) = pass(working) {
            *working = Cow::Owned(next);
        }
    }

    // Input that already spells a placeholder (`\0EIDNARA_PRES_<n>\0`) is
    // captured first, as its own literal region, so restoration cannot mistake
    // it for a region minted below.
    advance(&mut working, |text| {
        protect_literal_placeholders(text, &mut preserved)
    });

    static FENCED: OnceLock<Regex> = OnceLock::new();
    static INLINE: OnceLock<Regex> = OnceLock::new();
    static URL: OnceLock<Regex> = OnceLock::new();
    static TAG: OnceLock<Regex> = OnceLock::new();
    static PATH: OnceLock<Regex> = OnceLock::new();
    if working.contains('`') {
        advance(&mut working, |text| {
            protect_regex(
                text,
                FENCED.get_or_init(|| Regex::new(r"(?s)```.*?```").unwrap()),
                &mut preserved,
            )
        });
        advance(&mut working, |text| {
            protect_regex(
                text,
                INLINE.get_or_init(|| Regex::new(r"`[^`\n]+`").unwrap()),
                &mut preserved,
            )
        });
    }
    if working.contains("http") {
        advance(&mut working, |text| {
            protect_regex(
                text,
                URL.get_or_init(|| Regex::new(r"https?://\S+").unwrap()),
                &mut preserved,
            )
        });
    }
    if working.contains('§') {
        advance(&mut working, |text| {
            protect_regex(
                text,
                TAG.get_or_init(|| Regex::new(r"§[0-9]+§").unwrap()),
                &mut preserved,
            )
        });
    }
    advance(&mut working, |text| {
        protect_identifier_regions(text, &mut preserved)
    });
    if working.contains('/') {
        advance(&mut working, |text| {
            protect_paths(
                text,
                PATH.get_or_init(|| {
                    Regex::new(
                        r"(?:\.{1,2}/)?(?:[A-Za-z0-9_.-]+/)+[A-Za-z0-9_.-]+\.[A-Za-z0-9_]{1,6}",
                    )
                    .unwrap()
                }),
                &mut preserved,
            )
        });
    }
    advance(&mut working, |text| {
        protect_hash_regions(text, &mut preserved)
    });
    (working.into_owned(), preserved)
}

fn placeholder_marker_finder() -> &'static Finder<'static> {
    static F: OnceLock<Finder<'static>> = OnceLock::new();
    F.get_or_init(|| Finder::new(b"\0EIDNARA_PRES_"))
}

const PLACEHOLDER_MARKER_LEN: usize = "\u{0}EIDNARA_PRES_".len();

/// Restores preserved regions in one left-to-right scan with recursive
/// expansion of nested placeholders.
///
/// Region `i`'s captured original can only embed placeholders with index
/// less than `i`: later regions did not exist when `i` was captured, and
/// matches within one protect pass never overlap. `max_idx` enforces that
/// bound, so recursion strictly decreases and terminates. Text that merely
/// resembles a placeholder — an out-of-bound index, malformed digits, or any
/// byte mismatch against the canonical placeholder string — stays literal.
fn restore_regions(text: &str, preserved: &[PreservedRegion]) -> String {
    if preserved.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    restore_into(text, preserved, preserved.len(), &mut out);
    out
}

fn restore_into(text: &str, preserved: &[PreservedRegion], max_idx: usize, out: &mut String) {
    let bytes = text.as_bytes();
    let mut pos = 0usize;
    let mut pending = 0usize;
    while pos < bytes.len() {
        let Some(off) = placeholder_marker_finder().find(&bytes[pos..]) else {
            break;
        };
        let s = pos + off;
        let digits_start = s + PLACEHOLDER_MARKER_LEN;
        let mut j = digits_start;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j == digits_start || j >= bytes.len() || bytes[j] != 0 {
            pos = s + 1;
            continue;
        }
        let idx: usize = std::str::from_utf8(&bytes[digits_start..j])
            .expect("digits are ascii")
            .parse()
            .unwrap_or(usize::MAX);
        let end = j + 1;
        if idx < max_idx && preserved[idx].placeholder.as_bytes() == &bytes[s..end] {
            out.push_str(&text[pending..s]);
            if preserved[idx].literal {
                out.push_str(&preserved[idx].original);
            } else {
                restore_into(&preserved[idx].original, preserved, idx, out);
            }
            pending = end;
            pos = end;
        } else {
            pos = s + 1;
        }
    }
    out.push_str(&text[pending..]);
}

fn drop_articles(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut output = String::with_capacity(text.len());
    let mut pending = 0usize;
    let mut cursor = 0usize;
    let mut next_lower = memchr::memchr3(b'a', b't', b'A', bytes);
    let mut next_upper_t = memchr::memchr(b'T', bytes);
    loop {
        let candidate = match (next_lower, next_upper_t) {
            (Some(lower), Some(upper)) => lower.min(upper),
            (Some(lower), None) => lower,
            (None, Some(upper)) => upper,
            (None, None) => break,
        };
        if candidate < cursor {
            if next_lower == Some(candidate) {
                next_lower =
                    memchr::memchr3(b'a', b't', b'A', &bytes[cursor..]).map(|o| cursor + o);
            } else {
                next_upper_t = memchr::memchr(b'T', &bytes[cursor..]).map(|o| cursor + o);
            }
            continue;
        }
        cursor = candidate;
        let after = cursor + 1;
        if next_lower == Some(candidate) {
            next_lower = memchr::memchr3(b'a', b't', b'A', &bytes[after..]).map(|o| after + o);
        } else {
            next_upper_t = memchr::memchr(b'T', &bytes[after..]).map(|o| after + o);
        }
        if cursor > 0 && is_word_byte(bytes[cursor - 1]) {
            cursor = after;
            continue;
        }
        let word_len = if ascii_eq_at(text, cursor, "the") {
            3
        } else if ascii_eq_at(text, cursor, "an") {
            2
        } else if ascii_eq_at(text, cursor, "a") {
            1
        } else {
            0
        };
        if word_len > 0 {
            let word_end = cursor + word_len;
            if (word_end >= bytes.len() || !is_word_byte(bytes[word_end]))
                && word_end < bytes.len()
                && let Some(run_end) = whitespace_run_end(text, word_end)
            {
                output.push_str(&text[pending..cursor]);
                pending = run_end;
                cursor = run_end;
                continue;
            }
        }
        cursor = after;
    }
    output.push_str(&text[pending..]);
    collapse_ascii_spaces(&output)
}

/// End of the Unicode-whitespace run starting at `offset`, or `None` when no whitespace starts
/// there.
fn whitespace_run_end(text: &str, offset: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut end = offset;
    while end < bytes.len() {
        let byte = bytes[end];
        if byte < 0x80 {
            if !matches!(byte, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r') {
                break;
            }
            end += 1;
        } else {
            let ch = text[end..].chars().next().expect("char boundary");
            if !ch.is_whitespace() {
                break;
            }
            end += ch.len_utf8();
        }
    }
    (end > offset).then_some(end)
}

/// Indices into `sorted_auxiliaries` grouped by first byte, in that list's order.
fn auxiliary_buckets() -> &'static [Vec<usize>; 128] {
    static BUCKETS: OnceLock<[Vec<usize>; 128]> = OnceLock::new();
    BUCKETS.get_or_init(|| {
        let mut buckets: [Vec<usize>; 128] = std::array::from_fn(|_| Vec::new());
        for (index, aux) in sorted_auxiliaries().iter().enumerate() {
            buckets[aux.as_bytes()[0] as usize].push(index);
        }
        buckets
    })
}

fn collapse_ascii_spaces(text: &str) -> String {
    let bytes = text.as_bytes();
    let Some(first_run) = memchr::memmem::find(bytes, b"  ") else {
        return text.to_string();
    };
    let mut output = String::with_capacity(text.len());
    let mut pending = 0usize;
    let mut cursor = first_run;
    while cursor < bytes.len() {
        if bytes[cursor] == b' ' && cursor > 0 && bytes[cursor - 1] == b' ' {
            output.push_str(&text[pending..cursor]);
            pending = cursor + 1;
        }
        cursor += 1;
    }
    output.push_str(&text[pending..]);
    output
}

fn matches_participle(text: &str, offset: usize) -> bool {
    let mut end = offset;
    while end < text.len() {
        let ch = next_char(text, end).unwrap();
        if !is_ascii_word(ch) {
            break;
        }
        end += ch.len_utf8();
    }
    if end == offset || !has_word_boundary_after(text, end) {
        return false;
    }
    let token = &text[offset..end].to_ascii_lowercase();
    ["ed", "en", "ing", "ized", "ised"]
        .iter()
        .any(|suffix| token.ends_with(suffix))
}

/// `AUXILIARIES` ordered longest-first, so multi-word forms ("has been") win
/// over their embedded single words ("been") at the same position.
fn sorted_auxiliaries() -> &'static [&'static str] {
    static SORTED: OnceLock<Vec<&'static str>> = OnceLock::new();
    SORTED.get_or_init(|| {
        let mut auxiliaries = AUXILIARIES.to_vec();
        auxiliaries.sort_by_key(|aux| std::cmp::Reverse(aux.len()));
        auxiliaries
    })
}

#[cfg(test)]
fn drop_auxiliaries(text: &str) -> String {
    let shadow = text
        .bytes()
        .map(|byte| byte.to_ascii_lowercase())
        .collect::<Vec<u8>>();
    drop_auxiliaries_shadowed(text, &shadow)
}

fn auxiliary_searcher() -> &'static PhraseSearcher {
    static S: OnceLock<PhraseSearcher> = OnceLock::new();
    S.get_or_init(|| PhraseSearcher::new(sorted_auxiliaries()))
}

/// Every drop begins at a whitespace run followed by an auxiliary, so the scan jumps from one
/// auxiliary occurrence in the shadow to the next and applies the whole-word, longest-first,
/// participle rule there.
fn drop_auxiliaries_shadowed(text: &str, shadow: &[u8]) -> String {
    debug_assert_eq!(text.len(), shadow.len());
    let auxiliaries = sorted_auxiliaries();
    let buckets = auxiliary_buckets();
    let searcher = auxiliary_searcher();
    let bytes = text.as_bytes();
    let mut output = String::with_capacity(text.len());
    let mut pending = 0usize;
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        let Some((offset, _)) = searcher.find(&shadow[cursor..]) else {
            break;
        };
        let whitespace_end = cursor + offset;
        // The whitespace run before the auxiliary must begin at or after `cursor`: a run that an
        // earlier auxiliary absorbed as its trailing whitespace introduces nothing.
        if whitespace_end == cursor
            || !text[..whitespace_end]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace)
        {
            cursor = whitespace_end + 1;
            continue;
        }
        let first = bytes.get(whitespace_end).map_or(0, u8::to_ascii_lowercase);
        let candidates: &[usize] = if first < 0x80 {
            &buckets[first as usize]
        } else {
            &[]
        };
        let aux = candidates
            .iter()
            .map(|&index| auxiliaries[index])
            .find(|aux| {
                ascii_eq_at(text, whitespace_end, aux)
                    && has_word_boundary_before(text, whitespace_end)
                    && has_word_boundary_after(text, whitespace_end + aux.len())
            });
        let Some(aux) = aux else {
            cursor = whitespace_end + 1;
            continue;
        };
        let aux_end = whitespace_end + aux.len();
        let Some(run_end) = whitespace_run_end(text, aux_end) else {
            cursor = aux_end;
            continue;
        };
        if matches_participle(text, run_end) {
            output.push_str(&text[pending..whitespace_run_start(text, whitespace_end, pending)]);
            output.push(' ');
            pending = run_end;
        }
        cursor = run_end;
    }
    output.push_str(&text[pending..]);
    collapse_ascii_spaces(&output)
}

fn transform_preserving_user_lines(text: &str, transform: impl Fn(&str) -> String) -> String {
    if !text.starts_with("U: ") && !text.contains("\nU: ") {
        return transform(text);
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let mut output = Vec::with_capacity(lines.len());
    let mut buffer = Vec::new();
    for line in lines {
        if line.starts_with("U: ") {
            if !buffer.is_empty() {
                output.push(transform(&buffer.join("\n")));
                buffer.clear();
            }
            output.push(line.to_string());
        } else {
            buffer.push(line);
        }
    }
    if !buffer.is_empty() {
        output.push(transform(&buffer.join("\n")));
    }
    output.join("\n")
}

fn normalize_whitespace(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut output = String::with_capacity(text.len());
    let mut pending = 0usize;
    let mut cursor = 0usize;
    // Collapse runs of spaces and tabs to one space, and drop them before a newline or the end.
    while let Some(offset) = memchr::memchr2(b' ', b'\t', &bytes[cursor..]) {
        let run_start = cursor + offset;
        cursor = run_start + 1;
        while cursor < bytes.len() && matches!(bytes[cursor], b' ' | b'\t') {
            cursor += 1;
        }
        if run_start + 1 == cursor
            && bytes[run_start] == b' '
            && cursor < bytes.len()
            && bytes[cursor] != b'\n'
        {
            continue;
        }
        output.push_str(&text[pending..run_start]);
        if cursor < bytes.len() && bytes[cursor] != b'\n' {
            output.push(' ');
        }
        pending = cursor;
    }
    output.push_str(&text[pending..]);
    if !output.contains("\n\n\n") {
        return output;
    }
    // Cap every newline run at two, the fixpoint of "\n\n\n" -> "\n\n".
    let mut capped = String::with_capacity(output.len());
    let mut run = 0usize;
    for ch in output.chars() {
        if ch == '\n' {
            run += 1;
            if run > 2 {
                continue;
            }
        } else {
            run = 0;
        }
        capped.push(ch);
    }
    capped
}

/// Compresses prose while preserving fenced and inline code, `http` and `https` URLs, numbered `§` tags, recognized prefixed identifiers, hashes, paths ending in a short extension, and `U: ` lines.
///
/// Other shapes are compressed, so a bare XML-style tag or an extensionless path such as `/tmp/session` is shortened like prose.
/// Transformations run in fixed pass order. Output is trimmed; internal ASCII whitespace is
/// normalized; newline runs contain at most two newlines.
pub fn compress(text: &str, level: TerseTextCompressionLevel) -> String {
    if text.is_empty() {
        return text.to_string();
    }
    let (protected_text, preserved) = protect_regions(text);
    let transformed = transform_preserving_user_lines(&protected_text, |chunk| {
        let mut buf = ShadowedText::new(chunk.to_string());
        drop_phrases_shadowed(&mut buf, filler_searcher());
        drop_phrases_shadowed(&mut buf, hedging_searcher());
        drop_phrases_shadowed(&mut buf, pleasantries_searcher());
        apply_phrase_shortenings(&mut buf);
        if matches!(
            level,
            TerseTextCompressionLevel::Full | TerseTextCompressionLevel::Ultra
        ) {
            let working = drop_auxiliaries_shadowed(&buf.text, &buf.shadow);
            buf = ShadowedText::new(drop_articles(&working));
        }
        if level == TerseTextCompressionLevel::Ultra {
            apply_ultra_connectives(&mut buf);
            apply_ultra_abbreviations(&mut buf);
        }
        buf.text
    });
    normalize_whitespace(&restore_regions(&transformed, &preserved))
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    /// Input that spells a placeholder byte for byte is preserved as itself;
    /// restoration must not swap it for the region minted with that index.
    #[test]
    fn literal_placeholder_text_survives_compression_unchanged() {
        let literal = "\u{0}EIDNARA_PRES_0\u{0}";
        let text = format!("see https://example.com/x and then {literal} literally");
        let out = compress(&text, TerseTextCompressionLevel::Full);
        assert!(out.contains(literal), "{out:?}");
        assert!(out.contains("https://example.com/x"), "{out:?}");
        assert_eq!(out.matches("https://example.com/x").count(), 1, "{out:?}");
    }

    #[derive(Debug, Deserialize)]
    struct GoldenCase {
        text: String,
        lite: String,
        full: String,
        ultra: String,
    }

    #[test]
    fn differential_golden_matches_typescript_oracle() {
        let cases: Vec<GoldenCase> = serde_json::from_str(include_str!(
            "../testdata/terse_text_compression-golden.json"
        ))
        .expect("valid terse_text_compression golden");
        assert!(
            !cases.is_empty(),
            "terse_text_compression golden fixture must not be empty"
        );
        for case in cases {
            assert_eq!(
                compress(&case.text, TerseTextCompressionLevel::Lite),
                case.lite,
                "lite: {:?}",
                case.text
            );
            assert_eq!(
                compress(&case.text, TerseTextCompressionLevel::Full),
                case.full,
                "full: {:?}",
                case.text
            );
            assert_eq!(
                compress(&case.text, TerseTextCompressionLevel::Ultra),
                case.ultra,
                "ultra: {:?}",
                case.text
            );
        }
    }

    /// Character-by-character forms of the passes, kept as the specification the byte-oriented
    /// passes are checked against.
    mod pass_reference {
        use super::super::{
            PreservedRegion, Regex, ascii_eq_at, has_word_boundary_after, has_word_boundary_before,
            matches_participle, next_char, previous_char, sorted_auxiliaries,
        };
        use std::sync::OnceLock;

        pub(super) fn protect_regions(text: &str) -> (String, Vec<PreservedRegion>) {
            let mut preserved = Vec::new();
            let mut working = text.to_string();

            // Input that already spells a placeholder (`\0EIDNARA_PRES_<n>\0`) is
            // captured first, as its own literal region, so restoration cannot mistake
            // it for a region minted below.
            working = protect_literal_placeholders(&working, &mut preserved);

            static FENCED: OnceLock<Regex> = OnceLock::new();
            static INLINE: OnceLock<Regex> = OnceLock::new();
            static URL: OnceLock<Regex> = OnceLock::new();
            static TAG: OnceLock<Regex> = OnceLock::new();
            static PATH: OnceLock<Regex> = OnceLock::new();
            working = protect_regex(
                &working,
                FENCED.get_or_init(|| Regex::new(r"(?s)```.*?```").unwrap()),
                &mut preserved,
            );
            working = protect_regex(
                &working,
                INLINE.get_or_init(|| Regex::new(r"`[^`\n]+`").unwrap()),
                &mut preserved,
            );
            working = protect_regex(
                &working,
                URL.get_or_init(|| Regex::new(r"https?://\S+").unwrap()),
                &mut preserved,
            );
            working = protect_regex(
                &working,
                TAG.get_or_init(|| Regex::new(r"§[0-9]+§").unwrap()),
                &mut preserved,
            );
            working = protect_identifier_regions(&working, &mut preserved);
            working = protect_regex(
                &working,
                PATH.get_or_init(|| {
                    Regex::new(
                        r"(?:\.{1,2}/)?(?:[A-Za-z0-9_.-]+/)+[A-Za-z0-9_.-]+\.[A-Za-z0-9_]{1,6}",
                    )
                    .unwrap()
                }),
                &mut preserved,
            );
            working = protect_hash_regions(&working, &mut preserved);
            (working, preserved)
        }

        fn protect_regex(
            text: &str,
            regex: &Regex,
            preserved: &mut Vec<PreservedRegion>,
        ) -> String {
            protect_regex_filtered(text, regex, preserved, false, |_, _, _| true)
        }

        fn protect_literal_placeholders(
            text: &str,
            preserved: &mut Vec<PreservedRegion>,
        ) -> String {
            static LITERAL_PLACEHOLDER: OnceLock<Regex> = OnceLock::new();
            protect_regex_filtered(
                text,
                LITERAL_PLACEHOLDER
                    .get_or_init(|| Regex::new("\u{0}EIDNARA_PRES_[0-9]+\u{0}").unwrap()),
                preserved,
                true,
                |_, _, _| true,
            )
        }

        fn protect_regex_filtered(
            text: &str,
            regex: &Regex,
            preserved: &mut Vec<PreservedRegion>,
            literal: bool,
            accept: impl Fn(&str, usize, usize) -> bool,
        ) -> String {
            let mut output = String::with_capacity(text.len());
            let mut cursor = 0;
            for matched in regex.find_iter(text) {
                if !accept(text, matched.start(), matched.end()) {
                    continue;
                }
                output.push_str(&text[cursor..matched.start()]);
                let placeholder = format!("\u{0}EIDNARA_PRES_{}\u{0}", preserved.len());
                preserved.push(PreservedRegion {
                    placeholder: placeholder.clone(),
                    original: matched.as_str().to_string(),
                    literal,
                });
                output.push_str(&placeholder);
                cursor = matched.end();
            }
            output.push_str(&text[cursor..]);
            output
        }

        fn protect_identifier_regions(text: &str, preserved: &mut Vec<PreservedRegion>) -> String {
            static IDENTIFIER: OnceLock<Regex> = OnceLock::new();
            let regex =
                IDENTIFIER.get_or_init(|| Regex::new(r"(?:msg|ses|toolu)_[A-Za-z0-9]+").unwrap());
            protect_regex_filtered(text, regex, preserved, false, |text, start, end| {
                has_word_boundary_before(text, start) && has_word_boundary_after(text, end)
            })
        }

        fn protect_hash_regions(text: &str, preserved: &mut Vec<PreservedRegion>) -> String {
            static HASH: OnceLock<Regex> = OnceLock::new();
            let regex = HASH.get_or_init(|| Regex::new(r"[0-9a-fA-F]{7,40}").unwrap());
            protect_regex_filtered(text, regex, preserved, false, |text, start, end| {
                !previous_char(text, start).is_some_and(|ch| ch.is_ascii_alphanumeric())
                    && !next_char(text, end).is_some_and(|ch| ch.is_ascii_alphanumeric())
            })
        }

        pub(super) fn drop_articles(text: &str) -> String {
            let mut output = String::with_capacity(text.len());
            let mut cursor = 0;
            while cursor < text.len() {
                let Some(ch) = next_char(text, cursor) else {
                    break;
                };
                if (ch == 't' || ch == 'T' || ch == 'a' || ch == 'A')
                    && has_word_boundary_before(text, cursor)
                {
                    let word = if ascii_eq_at(text, cursor, "the") {
                        "the"
                    } else if ascii_eq_at(text, cursor, "an") {
                        "an"
                    } else if ascii_eq_at(text, cursor, "a") {
                        "a"
                    } else {
                        ""
                    };
                    if !word.is_empty() && has_word_boundary_after(text, cursor + word.len()) {
                        let mut end = cursor + word.len();
                        if end < text.len() && next_char(text, end).is_some_and(char::is_whitespace)
                        {
                            while end < text.len()
                                && next_char(text, end).is_some_and(char::is_whitespace)
                            {
                                end += next_char(text, end).unwrap().len_utf8();
                            }
                            cursor = end;
                            continue;
                        }
                    }
                }
                output.push(ch);
                cursor += ch.len_utf8();
            }
            collapse_ascii_spaces(&output)
        }

        pub(super) fn collapse_ascii_spaces(text: &str) -> String {
            let mut output = String::with_capacity(text.len());
            let mut previous_space = false;
            for ch in text.chars() {
                if ch == ' ' {
                    if previous_space {
                        continue;
                    }
                    previous_space = true;
                } else {
                    previous_space = false;
                }
                output.push(ch);
            }
            output
        }

        pub(super) fn drop_auxiliaries(text: &str) -> String {
            let auxiliaries = sorted_auxiliaries();

            let mut output = String::with_capacity(text.len());
            let mut cursor = 0;
            while cursor < text.len() {
                let Some(ch) = next_char(text, cursor) else {
                    break;
                };
                if ch.is_whitespace() {
                    let mut whitespace_end = cursor;
                    while whitespace_end < text.len()
                        && next_char(text, whitespace_end).is_some_and(char::is_whitespace)
                    {
                        whitespace_end += next_char(text, whitespace_end).unwrap().len_utf8();
                    }
                    let Some(aux) = auxiliaries.iter().find(|aux| {
                        ascii_eq_at(text, whitespace_end, aux)
                            && has_word_boundary_before(text, whitespace_end)
                            && has_word_boundary_after(text, whitespace_end + aux.len())
                    }) else {
                        output.push_str(&text[cursor..whitespace_end]);
                        cursor = whitespace_end;
                        continue;
                    };
                    let mut aux_end = whitespace_end + aux.len();
                    if aux_end >= text.len()
                        || !next_char(text, aux_end).is_some_and(char::is_whitespace)
                    {
                        output.push_str(&text[cursor..aux_end]);
                        cursor = aux_end;
                        continue;
                    }
                    while aux_end < text.len()
                        && next_char(text, aux_end).is_some_and(char::is_whitespace)
                    {
                        aux_end += next_char(text, aux_end).unwrap().len_utf8();
                    }
                    if matches_participle(text, aux_end) {
                        output.push(' ');
                        cursor = aux_end;
                        continue;
                    }
                    output.push_str(&text[cursor..aux_end]);
                    cursor = aux_end;
                    continue;
                }
                output.push(ch);
                cursor += ch.len_utf8();
            }
            collapse_ascii_spaces(&output)
        }

        pub(super) fn transform_preserving_user_lines(
            text: &str,
            transform: impl Fn(&str) -> String,
        ) -> String {
            let lines: Vec<&str> = text.split('\n').collect();
            let mut output = Vec::with_capacity(lines.len());
            let mut buffer = Vec::new();
            for line in lines {
                if line.starts_with("U: ") {
                    if !buffer.is_empty() {
                        output.push(transform(&buffer.join("\n")));
                        buffer.clear();
                    }
                    output.push(line.to_string());
                } else {
                    buffer.push(line);
                }
            }
            if !buffer.is_empty() {
                output.push(transform(&buffer.join("\n")));
            }
            output.join("\n")
        }

        pub(super) fn normalize_whitespace(text: &str) -> String {
            let mut lines = Vec::new();
            for line in text.split('\n') {
                let mut normalized = String::with_capacity(line.len());
                let mut previous_space = false;
                for ch in line.chars() {
                    if ch == ' ' || ch == '\t' {
                        if previous_space {
                            continue;
                        }
                        normalized.push(' ');
                        previous_space = true;
                    } else {
                        normalized.push(ch);
                        previous_space = false;
                    }
                }
                while normalized.ends_with([' ', '\t']) {
                    normalized.pop();
                }
                lines.push(normalized);
            }
            let joined = lines.join("\n");
            if !joined.contains("\n\n\n") {
                return joined;
            }
            // Cap every newline run at two, the fixpoint of "\n\n\n" -> "\n\n".
            let mut output = String::with_capacity(joined.len());
            let mut run = 0usize;
            for ch in joined.chars() {
                if ch == '\n' {
                    run += 1;
                    if run > 2 {
                        continue;
                    }
                } else {
                    run = 0;
                }
                output.push(ch);
            }
            output
        }
    }

    const PASS_VOCABULARY: &[&str] = &[
        "the",
        "The",
        "THE",
        "a",
        "A",
        "an",
        "An",
        "and",
        "or",
        "is",
        "was",
        "were",
        "has been",
        "have been",
        "will be",
        "could be",
        "Been",
        "being",
        "am",
        "be",
        "implemented",
        "configured",
        "running",
        "ized",
        "walked",
        "open",
        "config",
        "configuration",
        "message",
        "messages",
        "session",
        "history_segment",
        "context",
        "repository",
        "directory",
        "just",
        "really",
        "basically",
        "i think",
        "probably",
        "please",
        "thanks",
        "in order to",
        "at the moment",
        "and then",
        "because of",
        "however",
        "as well as",
        " ",
        "  ",
        "\t",
        "\n",
        "\n\n",
        "\n\n\n",
        "\u{a0}",
        "\u{2003}",
        "\u{0B}",
        "\r\n",
        "\u{85}",
        "deadbeef",
        "0123456",
        "abcdef0123456789abcdef0123456789abcdef01",
        "0123456789abcdef0123456789abcdef0123456789abcdef",
        "x1234567",
        "1234567x",
        "ab12",
        "`code`",
        "```fenced\nblock```",
        "```open",
        "https://x.y/z",
        "§12§",
        "msg_abc",
        "toolu_1",
        "ses_",
        "src/lib.rs",
        "./a/b.c",
        "../x.y",
        "a/b",
        "U: hello",
        "\nU: there",
        "\u{0}EIDNARA_PRES_0\u{0}",
        "日本語",
        "é",
        "İ",
        "ΣΑΣ",
        "-",
        "_",
        ".",
        ",",
        "!",
        "to",
        "the_",
        "_the",
        "thea",
        "a_b",
        "an.",
        "a\n",
    ];

    fn vocabulary_text() -> impl proptest::strategy::Strategy<Value = String> {
        use proptest::strategy::Strategy;
        proptest::collection::vec(
            (
                proptest::sample::select(PASS_VOCABULARY),
                proptest::bool::ANY,
            ),
            0..40,
        )
        .prop_map(|pieces| {
            let mut text = String::new();
            for (piece, space) in pieces {
                text.push_str(piece);
                if space {
                    text.push(' ');
                }
            }
            text
        })
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(2000))]

        #[test]
        fn byte_passes_match_the_character_passes(text in vocabulary_text()) {
            use proptest::prelude::*;
            prop_assert_eq!(drop_articles(&text), pass_reference::drop_articles(&text));
            prop_assert_eq!(drop_auxiliaries(&text), pass_reference::drop_auxiliaries(&text));
            prop_assert_eq!(collapse_ascii_spaces(&text), pass_reference::collapse_ascii_spaces(&text));
            prop_assert_eq!(normalize_whitespace(&text), pass_reference::normalize_whitespace(&text));
            let (protected, preserved) = protect_regions(&text);
            let (reference_protected, reference_preserved) = pass_reference::protect_regions(&text);
            prop_assert_eq!(protected, reference_protected);
            prop_assert_eq!(
                preserved.iter().map(|region| (&region.placeholder, &region.original, region.literal)).collect::<Vec<_>>(),
                reference_preserved.iter().map(|region| (&region.placeholder, &region.original, region.literal)).collect::<Vec<_>>()
            );
            let upper = |chunk: &str| chunk.to_ascii_uppercase();
            prop_assert_eq!(
                transform_preserving_user_lines(&text, upper),
                pass_reference::transform_preserving_user_lines(&text, upper)
            );
            for level in [
                TerseTextCompressionLevel::Lite,
                TerseTextCompressionLevel::Full,
                TerseTextCompressionLevel::Ultra,
            ] {
                prop_assert_eq!(compress(&text, level), reference_compress(&text, level));
            }
        }
    }

    /// `compress` over the character-by-character passes.
    fn reference_compress(text: &str, level: TerseTextCompressionLevel) -> String {
        if text.is_empty() {
            return text.to_string();
        }
        let (protected_text, preserved) = pass_reference::protect_regions(text);
        let transformed =
            pass_reference::transform_preserving_user_lines(&protected_text, |chunk| {
                let a = drop_phrases(chunk, filler_automaton());
                let b = drop_phrases(&a, hedging_automaton());
                let c = drop_phrases(&b, pleasantries_automaton());
                let mut buf = ShadowedText::new(c.into_owned());
                apply_phrase_shortenings(&mut buf);
                if matches!(
                    level,
                    TerseTextCompressionLevel::Full | TerseTextCompressionLevel::Ultra
                ) {
                    let working = pass_reference::drop_auxiliaries(&buf.text);
                    buf = ShadowedText::new(pass_reference::drop_articles(&working));
                }
                if level == TerseTextCompressionLevel::Ultra {
                    apply_ultra_connectives(&mut buf);
                    apply_ultra_abbreviations(&mut buf);
                }
                buf.text
            });
        pass_reference::normalize_whitespace(&restore_regions(&transformed, &preserved))
            .trim()
            .to_string()
    }

    /// `drop_phrases` relies on leftmost-first automaton semantics matching a
    /// position-by-position scan, which requires that no phrase in a set is a
    /// prefix of another and that every phrase is non-empty ASCII starting
    /// with a non-whitespace byte. Guards pattern-set edits that would break
    /// those assumptions silently.
    #[test]
    fn pattern_set_invariants() {
        for set in [FILLER_WORDS, HEDGING_PHRASES, PLEASANTRIES] {
            for (i, a) in set.iter().enumerate() {
                assert!(a.is_ascii(), "needle must be ascii: {a:?}");
                assert!(!a.is_empty(), "needle must be non-empty");
                assert!(
                    !a.as_bytes()[0].is_ascii_whitespace(),
                    "needle must start non-whitespace: {a:?}"
                );
                assert_eq!(
                    a.to_ascii_lowercase(),
                    *a,
                    "needle must be lowercase: {a:?}"
                );
                for (j, b) in set.iter().enumerate() {
                    if i != j {
                        assert!(
                            !b.starts_with(a),
                            "prefix pair breaks leftmost-first equivalence: {a:?} / {b:?}"
                        );
                    }
                }
            }
        }
        // Shadow scans require ASCII lowercase needles.
        for (needle, _) in PHRASE_SHORTENINGS
            .iter()
            .chain(ULTRA_CONNECTIVE_REPLACEMENTS)
            .chain(ULTRA_ABBREVIATIONS)
        {
            assert!(needle.is_ascii(), "needle must be ascii: {needle:?}");
            assert_eq!(
                needle.to_ascii_lowercase(),
                *needle,
                "needle must be lowercase: {needle:?}"
            );
        }
    }
}

//! Parity oracle: `tiktoken-rs::CoreBPE` driven by the `fancy-regex` pre-tokenizer.
//! `tests/reference_parity.rs` compares the live implementation against this one on random
//! inputs. Never edit this file to make a comparison pass; a change here is a change to the
//! invariant.
#![allow(missing_docs)]

use std::sync::OnceLock;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use fancy_regex::Regex;
use rustc_hash::FxHashMap;
use tiktoken_rs::{CoreBPE, Rank};

const CLAUDE_TIKTOKEN: &str = include_str!("../assets/claude.tiktoken");

macro_rules! ecmascript_whitespace {
    () => {
        r"\t\n\x0B\x0C\r \x{00A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}"
    };
}

pub const CLAUDE_PAT_STR: &str = concat!(
    r"'s|'t|'re|'ve|'m|'ll|'d| ?\p{L}+| ?\p{N}+| ?[^",
    ecmascript_whitespace!(),
    r"\p{L}\p{N}]+|[",
    ecmascript_whitespace!(),
    r"]+(?![^",
    ecmascript_whitespace!(),
    r"])|[",
    ecmascript_whitespace!(),
    r"]+",
);

pub const MAX_PIECE_BYTES: usize = 4096;

fn tokenizer() -> &'static CoreBPE {
    static TOKENIZER: OnceLock<CoreBPE> = OnceLock::new();
    TOKENIZER.get_or_init(|| {
        let mut encoder: FxHashMap<Vec<u8>, Rank> = FxHashMap::default();
        for line in CLAUDE_TIKTOKEN.lines() {
            if line.is_empty() {
                continue;
            }
            let (raw, rank_str) = line.split_once(' ').expect("vocab line");
            let bytes = STANDARD.decode(raw).expect("vocab token base64");
            let rank: Rank = rank_str.parse().expect("vocab rank");
            encoder.insert(bytes, rank);
        }
        CoreBPE::new(encoder, FxHashMap::default(), CLAUDE_PAT_STR).expect("claude BPE")
    })
}

pub fn piece_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(CLAUDE_PAT_STR).expect("claude pattern"))
}

fn char_chunks(piece: &str, max_bytes: usize) -> impl Iterator<Item = &str> {
    let mut rest = piece;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let mut end = rest.len().min(max_bytes);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        let (head, tail) = rest.split_at(end);
        rest = tail;
        Some(head)
    })
}

fn encode_bounded(bpe: &CoreBPE, text: &str) -> Vec<Rank> {
    if text.len() <= MAX_PIECE_BYTES {
        return bpe.encode_ordinary(text);
    }
    let mut out = Vec::new();
    let mut span_start = 0;
    for m in piece_regex().find_iter(text) {
        let m = m.expect("backtrack limit");
        if m.end() - m.start() <= MAX_PIECE_BYTES {
            continue;
        }
        out.extend(bpe.encode_ordinary(&text[span_start..m.start()]));
        for chunk in char_chunks(m.as_str(), MAX_PIECE_BYTES) {
            out.extend(bpe.encode_ordinary(chunk));
        }
        span_start = m.end();
    }
    out.extend(bpe.encode_ordinary(&text[span_start..]));
    out
}

pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    encode_bounded(tokenizer(), text).len()
}

pub fn encode_ordinary(text: &str) -> Vec<Rank> {
    encode_bounded(tokenizer(), text)
}

/// Pre-token piece byte ranges under the reference pattern, so tests can build inputs that
/// start and end on piece boundaries.
pub fn piece_spans(text: &str) -> Vec<(usize, usize)> {
    piece_regex()
        .find_iter(text)
        .map(|m| {
            let m = m.expect("backtrack limit");
            (m.start(), m.end())
        })
        .collect()
}

//! Pins `encode_ordinary` to the token ids `ai-tokenizer` produces for every fixture case.
//! `gen/gen-token-golden.ts` writes the fixture. Ids are compared, not counts, so an encoding
//! that is wrong but happens to produce the same number of tokens still fails.

use serde::Deserialize;
use tokenizer::{MAX_PIECE_BYTES, encode_ordinary, estimate_tokens};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GoldenCase {
    label: String,
    text: String,
    ids: Vec<u32>,
}

fn load_golden() -> Vec<GoldenCase> {
    let raw = include_str!("../testdata/token-golden.json");
    let cases: Vec<GoldenCase> = serde_json::from_str(raw).expect("token-golden.json is malformed");
    assert!(!cases.is_empty(), "golden corpus is empty");
    cases
}

#[test]
fn encode_ordinary_matches_ai_tokenizer_ids() {
    let cases = load_golden();
    let mut failures = Vec::new();
    for c in &cases {
        if !c.text.is_empty() {
            assert!(!c.ids.is_empty(), "case '{}' has text but no ids", c.label);
        }
        let got = encode_ordinary(&c.text);
        if got != c.ids {
            failures.push(format!(
                "case '{}': text={:?}\n  expected {:?}\n  got      {:?}",
                c.label, c.text, c.ids, got
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "token-ID mismatches vs ai-tokenizer (claude):\n{}",
        failures.join("\n")
    );
}

#[test]
fn estimate_tokens_matches_golden_counts() {
    for c in load_golden() {
        assert_eq!(
            estimate_tokens(&c.text),
            c.ids.len(),
            "estimate_tokens count mismatch for case '{}'",
            c.label
        );
    }
}

#[test]
fn empty_text_is_zero() {
    assert_eq!(estimate_tokens(""), 0);
}

#[test]
fn deterministic_across_calls() {
    let text = "Eidnara keeps a long session inside the context window.";
    let first = estimate_tokens(text);
    for _ in 0..1000 {
        assert_eq!(estimate_tokens(text), first);
    }
}

#[test]
fn deterministic_across_threads() {
    let cases = load_golden();
    let per_thread: Vec<Vec<(Vec<u32>, usize)>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                s.spawn(|| {
                    cases
                        .iter()
                        .map(|c| (encode_ordinary(&c.text), estimate_tokens(&c.text)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for (i, (ids, count)) in per_thread[0].iter().enumerate() {
        assert_eq!(ids, &cases[i].ids, "case '{}'", cases[i].label);
        assert_eq!(*count, ids.len(), "case '{}'", cases[i].label);
    }
    for other in &per_thread[1..] {
        assert_eq!(other, &per_thread[0]);
    }
}

/// Known divergence from `ai-tokenizer@1.0.6`, which drops the BOM here because its rank
/// lookup decodes byte slices with a BOM-stripping `TextDecoder` and scores `EF BB BF 0A` as
/// `"\n"`, yielding `[92, 203]`. The correct encoding keeps the BOM as its own token.
#[test]
fn bom_before_newline_is_preserved() {
    let bom = encode_ordinary("\u{feff}");
    let newline = encode_ordinary("\n");
    let x = encode_ordinary("x");
    assert_eq!(bom.len(), 1);
    assert_eq!(newline.len(), 1);
    assert_eq!(x.len(), 1);
    let got = encode_ordinary("x\u{feff}\n");
    assert_eq!(got, [x[0], bom[0], newline[0]]);
}

/// A letter run longer than the cap is chunked; the result must be consistent with counting
/// and equal to the concatenation of the chunk encodings. The over-long piece is ` ?\p{L}+`,
/// so it starts with the space before the run.
#[test]
fn over_long_piece_is_chunked_and_bounded() {
    let piece = format!(" {}", "a".repeat(MAX_PIECE_BYTES * 3 + 17));
    let text = format!("prefix{piece} suffix");
    let ids = encode_ordinary(&text);
    assert_eq!(ids.len(), estimate_tokens(&text));
    let mut expected = encode_ordinary("prefix");
    let mut start = 0;
    while start < piece.len() {
        let end = (start + MAX_PIECE_BYTES).min(piece.len());
        expected.extend(encode_ordinary(&piece[start..end]));
        start = end;
    }
    expected.extend(encode_ordinary(" suffix"));
    assert_eq!(ids, expected);
}

/// Multi-byte characters never get split mid-codepoint when a CJK run exceeds the cap.
#[test]
fn over_long_cjk_piece_keeps_char_boundaries() {
    let run = "你好世界".repeat(MAX_PIECE_BYTES / 4);
    let ids = encode_ordinary(&run);
    assert_eq!(ids.len(), estimate_tokens(&run));
    assert!(
        ids.len() >= run.chars().count() / 2,
        "implausibly few tokens: {}",
        ids.len()
    );
}

/// Text at or below the cap takes the unchunked path. A long document with no over-long piece
/// must encode identically whether or not the bounded path scans it. The sentence starts with
/// its space and ends at punctuation so its pieces are the same whether it is encoded alone or
/// inside the repeated text.
#[test]
fn long_text_without_over_long_piece_is_unaffected_by_bound() {
    let sentence = " The quick brown fox jumps over the lazy dog.";
    let repeats = MAX_PIECE_BYTES / sentence.len() * 4;
    let text = sentence.repeat(repeats);
    assert!(text.len() > MAX_PIECE_BYTES);
    let whole = encode_ordinary(&text);
    let mut concatenated = Vec::new();
    for _ in 0..repeats {
        concatenated.extend(encode_ordinary(sentence));
    }
    assert_eq!(whole, concatenated);
}

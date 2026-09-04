//! `PROPTEST_CASES` sets the property-test budget (guard.sh uses 50 000; default 2000). The
//! differential test reads `TOKENIZER_DIFF_CORPUS` (JSONL of `{text, ids}` written by
//! `gen/gen-diff-corpus.ts`) and is skipped when the variable is unset.

use std::io::BufRead as _;

use proptest::prelude::*;

use super::reference_impl as reference;
use super::{MAX_PIECE_BYTES, encode_ordinary, estimate_tokens};

/// Strings shaped like the benchmark arms plus arbitrary Unicode, biased toward the class
/// boundaries the pre-tokenizer distinguishes.
fn text_strategy() -> impl Strategy<Value = String> {
    let ws = r"[ \t\n\r\x0B\x0C\u{A0}\u{1680}\u{2000}-\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}\u{85}\u{200B}]";
    prop_oneof![
        4 => r"([A-Za-z]{1,12}('s|'t|'re|'ve|'m|'ll|'d)?[ ,.!?;:]{0,3}){0,40}",
        3 => r"([a-z_]{1,8}[ =+\-*/(){}\[\];:,.<>|&^!~%#@]{1,4}[0-9]{0,6}[ \t\n]{0,4}){0,40}",
        2 => r"[\u{4E00}-\u{9FA5}\u{3041}-\u{3096}\u{30A1}-\u{30FA}\u{AC00}-\u{D7A3}、。「」]{0,80}",
        2 => proptest::string::string_regex(&format!(r"({ws}{{1,6}}[A-Za-z0-9]{{0,5}}){{0,30}}")).unwrap(),
        2 => r"([0-9]{1,12}[.,:\-/xX]?){0,30}",
        2 => r"[\p{L}\p{N}\p{P}\p{S}\p{Z}\p{M}\p{C}]{0,120}",
        1 => any::<String>(),
        1 => r"(x| )[a-z]{3800,4400}( |[0-9]{1,3})?",
        1 => r" ?[\u{4E00}-\u{9FA5}]{1200,1500}[。 ]?",
        1 => r"( ?[A-Za-z]{1,6}|\u{FEFF}|\u{85}| ?[0-9]{1,4}|[ \t]{1,5}|[!-/]{1,4}|[\u{300}-\u{36F}]|[😀-🙏]|[\u{600}-\u{6FF}]{1,5}){0,60}",
    ]
}

fn is_ecmascript_whitespace_run(piece: &str) -> bool {
    piece.chars().all(|c| {
        matches!(
            c,
            '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
                ..='\u{200A}'
                    | '\u{2028}'
                    | '\u{2029}'
                    | '\u{202F}'
                    | '\u{205F}'
                    | '\u{3000}'
                    | '\u{FEFF}'
        )
    })
}

fn cases() -> ProptestConfig {
    ProptestConfig {
        cases: std::env::var("PROPTEST_CASES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(2000),
        max_shrink_iters: 2000,
        ..ProptestConfig::default()
    }
}

proptest! {
    #![proptest_config(cases())]

    #[test]
    fn ids_match_reference_impl(text in text_strategy()) {
        prop_assert_eq!(encode_ordinary(&text), reference::encode_ordinary(&text));
    }

    #[test]
    fn count_equals_encode_len(text in text_strategy()) {
        prop_assert_eq!(estimate_tokens(&text), encode_ordinary(&text).len());
        prop_assert_eq!(estimate_tokens(&text), reference::estimate_tokens(&text));
    }

    /// Splitting after a non-whitespace pre-token piece and encoding the halves separately
    /// yields the same ids as encoding the whole. No alternative of the pattern looks behind;
    /// the one lookahead (`\s+(?!\S)`) only reads past the end of a whitespace piece, so a
    /// boundary right after a whitespace piece is excluded.
    #[test]
    fn concat_at_piece_boundary(text in text_strategy(), pick in 0usize..1000) {
        let spans: Vec<(usize, usize)> = reference::piece_spans(&text)
            .into_iter()
            .filter(|&(s, e)| !is_ecmascript_whitespace_run(&text[s..e]))
            .collect();
        if spans.len() < 2 {
            return Ok(());
        }
        let k = spans[pick % (spans.len() - 1)].1;
        let (a, b) = text.split_at(k);
        let mut split = encode_ordinary(a);
        split.extend(encode_ordinary(b));
        prop_assert_eq!(split, encode_ordinary(&text));
    }
}

#[test]
fn encode_is_pure_across_threads() {
    let texts: Vec<String> = (0..2000u32)
        .map(|i| {
            format!(
                "{} {}{}{}",
                "word".repeat((i % 7) as usize),
                i,
                if i % 3 == 0 { "\u{3000}" } else { " " },
                "你好".repeat((i % 5) as usize)
            )
        })
        .collect();
    let expected: Vec<Vec<u32>> = texts.iter().map(|t| encode_ordinary(t)).collect();
    std::thread::scope(|s| {
        for _ in 0..8 {
            s.spawn(|| {
                for (t, e) in texts.iter().zip(&expected) {
                    assert_eq!(&encode_ordinary(t), e);
                    assert_eq!(estimate_tokens(t), e.len());
                }
            });
        }
    });
}

#[test]
fn differential_corpus_matches_ai_tokenizer() {
    let Ok(path) = std::env::var("TOKENIZER_DIFF_CORPUS") else {
        eprintln!("TOKENIZER_DIFF_CORPUS unset; differential test skipped");
        return;
    };
    #[derive(serde::Deserialize)]
    struct Case {
        text: String,
        ids: Vec<u32>,
    }
    let file = std::fs::File::open(&path).expect("open differential corpus");
    let mut n = 0usize;
    let mut long = 0usize;
    let mut failures = Vec::new();
    for line in std::io::BufReader::new(file).lines() {
        let line = line.unwrap();
        if line.trim().is_empty() {
            continue;
        }
        let c: Case = serde_json::from_str(&line).expect("corpus line");
        n += 1;
        if c.text.len() > MAX_PIECE_BYTES {
            long += 1;
        }
        let got = encode_ordinary(&c.text);
        if got != c.ids || estimate_tokens(&c.text) != c.ids.len() {
            failures.push(format!(
                "case {n}: {:?}... expected {} ids, got {}",
                c.text.chars().take(40).collect::<String>(),
                c.ids.len(),
                got.len()
            ));
        }
    }
    assert!(n >= 20_000, "differential corpus too small: {n}");
    assert!(long >= 2_000, "too few over-cap inputs: {long}");
    assert!(
        failures.is_empty(),
        "{} of {n} differential cases differ:\n{}",
        failures.len(),
        failures
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    eprintln!("differential corpus: {n} cases, {long} over MAX_PIECE_BYTES, all match");
}

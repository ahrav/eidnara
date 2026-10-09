//! `PROPTEST_CASES` sets the property-test budget (default 2000; CI for the perf changes ran
//! 50 000 per property).

use proptest::prelude::*;

use super::reference_impl as reference;
use super::{encode_ordinary, estimate_tokens};

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

/// The `proptest!` macro layers `PROPTEST_CASES` over this default. A zero budget runs no
/// cases and still reports success, so it is rejected.
fn cases() -> ProptestConfig {
    let config = ProptestConfig {
        cases: 2000,
        max_shrink_iters: 2000,
        ..ProptestConfig::default()
    };
    let effective = proptest::test_runner::contextualize_config(config.clone());
    assert!(
        effective.cases > 0,
        "PROPTEST_CASES must be a positive integer"
    );
    config
}

proptest! {
    #![proptest_config(cases())]

    #[test]
    fn ids_match_reference_impl(text in text_strategy()) {
        prop_assert_eq!(encode_ordinary(&text), reference::encode_ordinary(&text));
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

    /// A `"\n\n"` join of texts that each start outside the whitespace class after the first
    /// costs every earlier text with one more `"\n"`, one `"\n"` per separator, and the last
    /// text. Texts may end in whitespace. The reference agrees wherever every piece merges whole.
    #[test]
    fn paragraph_join_costs_each_text_with_its_newline(
        first in text_strategy(),
        rest in proptest::collection::vec(text_strategy(), 0..6),
    ) {
        let mut parts = vec![first.as_str()];
        parts.extend(rest.iter().map(String::as_str).filter(|text| crate::starts_outside_whitespace(text)));
        let joined = parts.join("\n\n");
        prop_assert_eq!(join_cost(&parts, estimate_tokens), estimate_tokens(&joined));
        let merged_whole = reference::piece_spans(&joined)
            .iter()
            .all(|&(start, end)| end - start <= crate::MAX_PIECE_BYTES);
        if merged_whole {
            prop_assert_eq!(join_cost(&parts, estimate_tokens), reference::estimate_tokens(&joined));
        }
    }

    #[test]
    fn paragraph_spans_count_as_their_text(
        parts in proptest::collection::vec(text_strategy(), 1..6),
        joins in proptest::collection::vec("(\n\n|\n\n\n| \n\n|\n \n\n|\n)", 0..6),
    ) {
        let mut text = String::new();
        for (index, part) in parts.iter().enumerate() {
            if index > 0 {
                text.push_str(joins.get(index - 1).map_or("\n\n", String::as_str));
            }
            text.push_str(part);
        }
        let spans: Vec<&str> = crate::paragraph_spans(&text).collect();
        let separators = spans.len() - 1;
        prop_assert_eq!(spans.iter().map(|span| span.len()).sum::<usize>() + separators, text.len());
        let summed = spans.iter().map(|span| estimate_tokens(span)).sum::<usize>()
            + separators * estimate_tokens("\n");
        prop_assert_eq!(summed, estimate_tokens(&text));
        let merged_whole = reference::piece_spans(&text)
            .iter()
            .all(|&(start, end)| end - start <= crate::MAX_PIECE_BYTES);
        if merged_whole {
            prop_assert_eq!(summed, reference::estimate_tokens(&text));
        }
    }
}

/// The join identity `starts_outside_whitespace` states, evaluated with `count`.
fn join_cost(parts: &[&str], count: impl Fn(&str) -> usize) -> usize {
    let (last, earlier) = parts.split_last().expect("one part");
    earlier
        .iter()
        .map(|part| count(&format!("{part}\n")) + count("\n"))
        .sum::<usize>()
        + count(last)
}

#[test]
fn paragraph_joins_hold_at_class_boundaries() {
    for opens in ["x", "#", "'s", "9", "\u{85}x", "\u{200B}", "東"] {
        assert!(crate::starts_outside_whitespace(opens), "{opens:?}");
    }
    for blank in ["", " x", "\nx", "\tx", "\u{A0}x", "\u{3000}x", "\u{FEFF}x"] {
        assert!(!crate::starts_outside_whitespace(blank), "{blank:?}");
    }
    let pairs = [
        ("x", "'s"),
        ("x", "\u{85}y"),
        ("\u{200B}", "x"),
        ("東", "9"),
        ("x", "\u{300}"),
        ("…", "x"),
        ("## 3-4 · ", "## 5-6 · t"),
        ("t\u{3000}", "#"),
        ("t\u{FEFF}", "x"),
        ("a \t ", "b"),
        ("  ", "x"),
    ];
    for (before, after) in pairs {
        let joined = format!("{before}\n\n{after}");
        assert_eq!(
            join_cost(&[before, after], estimate_tokens),
            estimate_tokens(&joined),
            "{joined:?}"
        );
        assert_eq!(
            estimate_tokens(&joined),
            reference::estimate_tokens(&joined),
            "{joined:?}"
        );
    }
    assert_eq!(estimate_tokens("\n"), 1);
}

#[test]
fn count_matches_reference_on_boundary_inputs() {
    let over_long = format!(" {}", "a".repeat(crate::MAX_PIECE_BYTES + 1));
    for text in [
        "",
        " ",
        "\n",
        "x\u{feff}\n",
        "hello world",
        over_long.as_str(),
    ] {
        assert_eq!(
            estimate_tokens(text),
            reference::estimate_tokens(text),
            "{text:?}"
        );
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

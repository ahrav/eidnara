//! The scorer's term pipeline must keep the documented rules exactly while it
//! takes faster paths; `reference` applies those rules directly (atoms and
//! parts, `str::to_lowercase` per term, the floor on scalar values, set
//! semantics, decision-side containment) and defines expected behavior.
//! Optimized output must match it for every generated text.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use daemon::contradiction_candidates::scorer::{
    CANDIDATES_PER_DESCRIPTOR, CandidateInput, DESCRIPTOR_BOUNDS, DescriptorTerms, SUMMARY_BOUNDS,
    SummaryScore, score_descriptor, term_set,
};
use proptest::prelude::*;

mod reference {
    use std::collections::BTreeSet;

    use daemon::contradiction_candidates::scorer::{
        CANDIDATE_SCORE_THRESHOLD, MIN_DECISION_TERMS, MIN_MATCHED_TERMS, MIN_TERM_SCALARS,
        STOPWORDS, SummaryScore,
    };
    use retrieval::lexical::{LexicalBounds, LexicalRefusal, analyze_segments};

    pub fn term_set(text: &str, bounds: LexicalBounds) -> Result<BTreeSet<String>, LexicalRefusal> {
        let analysis = analyze_segments(&[text], bounds)?;
        Ok(analysis
            .atoms()
            .chain(analysis.parts())
            .map(str::to_lowercase)
            .filter(|term| {
                term.chars().count() >= MIN_TERM_SCALARS
                    && term.chars().any(char::is_alphanumeric)
                    && !term.chars().all(char::is_numeric)
                    && !STOPWORDS.contains(&term.as_str())
            })
            .collect())
    }

    pub fn score(descriptor: &BTreeSet<String>, summary: &str) -> SummaryScore {
        let Ok(terms) = term_set(summary, super::SUMMARY_BOUNDS) else {
            return SummaryScore::Unscorable;
        };
        if terms.is_empty() {
            return SummaryScore::Unscorable;
        }
        let matched_terms = terms.intersection(descriptor).count();
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

/// `PIECES` exercises ASCII case mixes, stopwords, identifier splitting, digit
/// runs, eight-byte boundaries, Unicode lowercase conversions, combining
/// marks, private-use scalars, and numeric scalars.
const PIECES: &[&str] = &[
    "alpha",
    "Beta",
    "GAMMA",
    "delta",
    "the",
    "THE",
    "And",
    "YOUR",
    "config",
    "Config",
    "HTTPServer",
    "configLoader",
    "config_loader",
    "parseConfigFile",
    "IDs",
    "utf8Decoder",
    "x86_64",
    "v2",
    "2024",
    "a_1",
    "1_2",
    "go",
    "abcdefg",
    "abcdefgh",
    "abcdefghi",
    "ABCDEFGHIJKLMNOP",
    "transaction",
    "Transaction",
    "École",
    "e\u{301}cole",
    "\u{301}abc",
    "İSTANBUL",
    "istanbul",
    "İs",
    "ß",
    "STRASSE",
    "ΣΑΣ",
    "σας",
    "\u{212A}elvin",
    "Kelvin",
    "ǅungla",
    "ⅫⅫⅫ",
    "²³⁴",
    "日本語",
    "\u{E000}\u{E001}\u{E002}",
    "___",
    "naïve",
    "NAÏVE",
    "😀",
];

const SEPARATORS: &[&str] = &[
    " ", "  ", "\n", ", ", ". ", "::", "()", "-", "/", "\u{A0}", "",
];

fn text(max_pieces: usize) -> impl Strategy<Value = String> {
    prop::collection::vec(
        (
            prop::sample::select(PIECES),
            0..1_000u32,
            prop::sample::select(SEPARATORS),
        ),
        0..max_pieces,
    )
    .prop_map(|pieces| {
        let mut text = String::new();
        for (piece, number, separator) in pieces {
            if number % 7 == 0 {
                text.push_str(&format!("w{number}"));
            } else {
                text.push_str(piece);
            }
            text.push_str(separator);
        }
        text
    })
}

fn reference_terms(descriptor: &str) -> BTreeSet<String> {
    reference::term_set(descriptor, DESCRIPTOR_BOUNDS).unwrap()
}

/// Numbered words let the scorer tests exercise hundreds of distinct terms.
fn ascii_text(max_words: usize) -> impl Strategy<Value = String> {
    prop::collection::vec((0..300u32, 0..4u8), 0..max_words).prop_map(|words| {
        words
            .into_iter()
            .map(|(number, case)| match case {
                0 => format!("term{number}"),
                1 => format!("Word{number}"),
                2 => format!("LOADER{number}x"),
                _ => format!("camelCase{number}"),
            })
            .collect::<Vec<_>>()
            .join(" ")
    })
}

fn rank(a: &(usize, usize, String), b: &(usize, usize, String)) -> Ordering {
    (b.0 * a.1)
        .cmp(&(a.0 * b.1))
        .then_with(|| b.0.cmp(&a.0))
        .then_with(|| a.1.cmp(&b.1))
        .then_with(|| a.2.cmp(&b.2))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]
    #[test]
    fn term_sets_match_the_documented_rules(text in text(120)) {
        prop_assert_eq!(
            term_set(&text, SUMMARY_BOUNDS),
            reference::term_set(&text, SUMMARY_BOUNDS)
        );
        prop_assert_eq!(
            term_set(&text, DESCRIPTOR_BOUNDS),
            reference::term_set(&text, DESCRIPTOR_BOUNDS)
        );
    }

    #[test]
    fn summary_scores_match_the_documented_rules(
        descriptor in text(300),
        summaries in prop::collection::vec(text(200), 1..6),
    ) {
        let terms = DescriptorTerms::new(&descriptor).unwrap();
        let expected_terms = reference_terms(&descriptor);
        for summary in &summaries {
            prop_assert_eq!(
                terms.score(summary),
                reference::score(&expected_terms, summary),
                "{:?}", summary
            );
        }
    }

    #[test]
    fn summaries_drawn_from_the_descriptor_score_as_the_rules_say(
        descriptor in text(200),
        picks in prop::collection::vec((0..400usize, any::<bool>()), 1..120),
    ) {
        let words: Vec<&str> = descriptor
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|word| !word.is_empty())
            .collect();
        prop_assume!(!words.is_empty());
        let summary = picks
            .iter()
            .map(|&(index, upper)| {
                let word = words[index % words.len()];
                if upper { word.to_uppercase() } else { word.to_string() }
            })
            .collect::<Vec<_>>()
            .join(" ");
        let terms = DescriptorTerms::new(&descriptor).unwrap();
        prop_assert_eq!(
            terms.score(&summary),
            reference::score(&reference_terms(&descriptor), &summary)
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]
    #[test]
    fn ascii_summaries_with_many_distinct_terms_score_as_the_rules_say(
        descriptor in ascii_text(400),
        summary in ascii_text(160),
    ) {
        let terms = DescriptorTerms::new(&descriptor).unwrap();
        prop_assert_eq!(
            terms.score(&summary),
            reference::score(&reference_terms(&descriptor), &summary)
        );
    }

    #[test]
    fn score_descriptor_keeps_the_reference_top_k(
        descriptor in text(200),
        summaries in prop::collection::vec(text(40), 0..12),
    ) {
        let expected_terms = reference_terms(&descriptor);
        let ids: Vec<String> = (0..summaries.len()).map(|index| format!("d{index:02}")).collect();
        let inputs: Vec<CandidateInput<'_>> = summaries
            .iter()
            .zip(&ids)
            .map(|(summary, id)| CandidateInput {
                decision_object_id: id,
                decision_id: id,
                summary,
            })
            .collect();
        let mut expected = Vec::new();
        let mut expected_unscorable = Vec::new();
        for (summary, id) in summaries.iter().zip(&ids) {
            match reference::score(&expected_terms, summary) {
                SummaryScore::Candidate { matched_terms, decision_terms } => {
                    expected.push((matched_terms, decision_terms, id.clone()));
                }
                SummaryScore::Unscorable => expected_unscorable.push(id.clone()),
                SummaryScore::Below { .. } => {}
            }
        }
        expected.sort_by(rank);
        expected.truncate(CANDIDATES_PER_DESCRIPTOR);
        let scored = score_descriptor(&descriptor, &inputs).unwrap();
        let kept: Vec<(usize, usize, String)> = scored
            .candidates
            .into_iter()
            .map(|kept| (kept.matched_terms, kept.decision_terms, kept.decision_object_id))
            .collect();
        prop_assert_eq!(kept, expected);
        prop_assert_eq!(scored.unscorable, expected_unscorable);
    }
}

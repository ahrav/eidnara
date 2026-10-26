//! The contradiction-candidate scorer over hand-written fixtures. Every
//! expected term set below is written from the documented rules (atoms and
//! parts, per-term lowercase, at least three scalars, an alphanumeric scalar,
//! not all numeric, no stopword), never produced by the analyzer.

use std::collections::BTreeSet;

use daemon::contradiction_candidates::scorer::{
    CANDIDATE_MATCHER_EPOCH, CANDIDATES_PER_DESCRIPTOR, CandidateInput, DESCRIPTOR_BOUNDS,
    DescriptorTerms, SUMMARY_BOUNDS, ScoredCandidate, SummaryScore, matcher_digest,
    matcher_preimage, score_descriptor, term_set,
};
use retrieval::lexical::{ANALYSIS_CONTRACT_EPOCH, LexicalRefusal};

fn set(terms: &[&str]) -> BTreeSet<String> {
    terms.iter().map(|term| (*term).to_string()).collect()
}

fn terms(text: &str) -> BTreeSet<String> {
    term_set(text, SUMMARY_BOUNDS).unwrap()
}

fn candidate<'a>(id: &'a str, summary: &'a str) -> CandidateInput<'a> {
    CandidateInput {
        decision_object_id: id,
        decision_id: id,
        summary,
    }
}

#[test]
fn term_sets_follow_the_documented_fold_and_floor() {
    // `go` has two scalars and `the` is a stopword.
    assert_eq!(
        terms("Kernel writes go through the one writer transaction"),
        set(&[
            "kernel",
            "writes",
            "through",
            "one",
            "writer",
            "transaction"
        ])
    );
    // Parts give prose-to-identifier recall; the atom stays too.
    assert_eq!(
        terms("HTTPServer config_loader"),
        set(&[
            "httpserver",
            "http",
            "server",
            "config_loader",
            "config",
            "loader"
        ])
    );
    // Digit boundaries split parts; all-numeric and short parts drop.
    assert_eq!(
        terms("utf8Decoder 2024 v2"),
        set(&["utf8decoder", "utf", "decoder"])
    );
    // Private-use and underscore-only atoms carry no alphanumeric scalar.
    assert_eq!(terms("\u{E000}\u{E001}\u{E002} ___ cache"), set(&["cache"]));
    // A combining mark glues to its base; set semantics drop duplicates.
    assert_eq!(
        terms("e\u{301}cole cache cache CACHE"),
        set(&["e\u{301}cole", "cache"])
    );
    // Non-ASCII uppercase folds; diacritics are kept.
    assert_eq!(terms("ÉCOLE École"), set(&["école"]));
    // The floor counts scalars, not bytes, and runs after the fold: `éa` and
    // `日本` hold two scalars, `THE` lowers to a stopword, and `İs` lowers to
    // three scalars, `i`, a combining dot, and `s`.
    assert_eq!(terms("éa 日本 THE And İs"), set(&["i\u{307}s"]));
    // `İ` lowers to `i` plus a combining dot, so it is a different term from `i`.
    assert_eq!(
        terms("İSTANBUL istanbul"),
        set(&["i\u{307}stanbul", "istanbul"])
    );
}

#[test]
fn one_identifier_can_satisfy_the_term_gates() {
    // `parseConfigFile` holds four terms: the atom and its three parts.
    assert_eq!(
        terms("parseConfigFile"),
        set(&["parseconfigfile", "parse", "config", "file"])
    );
    assert_eq!(
        DescriptorTerms::new("call parseConfigFile here")
            .unwrap()
            .score("parseConfigFile"),
        SummaryScore::Candidate {
            matched_terms: 4,
            decision_terms: 4
        }
    );
}

#[test]
fn a_contained_summary_scores_full_and_disjoint_texts_score_nothing() {
    let summary = "Kernel writes go through one writer transaction";
    let scored = score_descriptor(
        "Notes: kernel writes go through one writer transaction today.",
        &[
            candidate("full", summary),
            candidate("disjoint", "Pancake batter needs eggs and flour"),
        ],
    )
    .unwrap();
    assert_eq!(
        scored.candidates,
        [ScoredCandidate {
            decision_object_id: "full".to_string(),
            decision_id: "full".to_string(),
            matched_terms: 6,
            decision_terms: 6,
        }]
    );
    assert!(scored.unscorable.is_empty());
    let descriptor = DescriptorTerms::new("Pancake batter needs eggs").unwrap();
    assert_eq!(
        descriptor.score(summary),
        SummaryScore::Below {
            matched_terms: 0,
            decision_terms: 6
        }
    );
}

#[test]
fn case_folds_identically_on_both_sides_and_diacritics_do_not_fold() {
    let summary = "Écoles cache every Kernel writer";
    let folded = DescriptorTerms::new("ÉCOLES CACHE EVERY KERNEL WRITER").unwrap();
    let same = DescriptorTerms::new(summary).unwrap();
    assert_eq!(folded.score(summary), same.score(summary));
    assert_eq!(
        folded.score(summary),
        SummaryScore::Candidate {
            matched_terms: 5,
            decision_terms: 5
        }
    );
    // A diacritic-only difference, and NFC against NFD, are different terms.
    let summary = "école école1 école2 école3";
    for descriptor in [
        "ecole ecole1 ecole2 ecole3",
        "e\u{301}cole e\u{301}cole1 e\u{301}cole2 e\u{301}cole3",
    ] {
        assert_eq!(
            DescriptorTerms::new(descriptor).unwrap().score(summary),
            SummaryScore::Below {
                matched_terms: 0,
                decision_terms: 4
            },
            "{descriptor:?}"
        );
    }
}

#[test]
fn identifiers_in_the_descriptor_match_prose_in_the_summary() {
    // Summary terms: http, server, config, loader, rules.
    assert_eq!(
        DescriptorTerms::new("fn HTTPServer::configLoader()")
            .unwrap()
            .score("HTTP server config loader rules"),
        SummaryScore::Candidate {
            matched_terms: 4,
            decision_terms: 5
        }
    );
}

#[test]
fn the_count_floors_block_short_or_thin_overlaps() {
    let descriptor = DescriptorTerms::new("alpha beta gamma delta epsilon").unwrap();
    // Three summary terms, all matched: under `MIN_DECISION_TERMS`.
    assert_eq!(
        descriptor.score("alpha beta gamma"),
        SummaryScore::Below {
            matched_terms: 3,
            decision_terms: 3
        }
    );
    // Two of four matched: half the terms, under `MIN_MATCHED_TERMS`.
    assert_eq!(
        descriptor.score("alpha beta zeta theta"),
        SummaryScore::Below {
            matched_terms: 2,
            decision_terms: 4
        }
    );
    // Three of seven matched: enough terms, under the 1/2 threshold.
    assert_eq!(
        descriptor.score("alpha beta gamma zeta theta iota kappa"),
        SummaryScore::Below {
            matched_terms: 3,
            decision_terms: 7
        }
    );
    // Three of six: exactly the threshold.
    assert_eq!(
        descriptor.score("alpha beta gamma zeta theta iota"),
        SummaryScore::Candidate {
            matched_terms: 3,
            decision_terms: 6
        }
    );
}

#[test]
fn an_oversize_descriptor_is_refused_whole_and_an_exact_fit_is_analyzed() {
    let bound = DESCRIPTOR_BOUNDS.max_input_bytes.get();
    assert_eq!(bound, 65_536);
    assert_eq!(DESCRIPTOR_BOUNDS.max_atoms.get(), 32_769);
    // The most atoms 65536 bytes hold: 32768 one-letter atoms and separators.
    let exact = "a ".repeat(bound / 2);
    assert_eq!(exact.len(), bound);
    assert!(DescriptorTerms::new(&exact).is_ok());
    let over = format!("{exact}a");
    assert_eq!(
        score_descriptor(&over, &[candidate("d", "alpha beta gamma delta")]),
        Err(LexicalRefusal::InputTooLong {
            bytes: bound + 1,
            bound
        })
    );
}

#[test]
fn an_unscorable_summary_is_reported_and_the_others_still_score() {
    let bound = SUMMARY_BOUNDS.max_input_bytes.get();
    let at_bound = format!("{} alpha beta gamma delta", "x".repeat(bound - 23));
    assert_eq!(at_bound.len(), bound);
    let over = format!("{at_bound}y");
    // The most atoms 8192 bytes hold is analyzed, not refused; one-letter atoms then fall to the floor.
    assert_eq!(SUMMARY_BOUNDS.max_atoms.get(), 4_097);
    assert_eq!(
        term_set(&"a ".repeat(bound / 2), SUMMARY_BOUNDS),
        Ok(BTreeSet::new())
    );
    assert!(term_set(&over, SUMMARY_BOUNDS).is_err());
    let scored = score_descriptor(
        "alpha beta gamma delta",
        &[
            candidate("over", &over),
            candidate("floor-emptied", "a an of to 12 345 the"),
            candidate("also-emptied", "x y z"),
            candidate("at-bound", &at_bound),
            candidate("plain", "alpha beta gamma delta"),
        ],
    )
    .unwrap();
    assert_eq!(scored.unscorable, ["also-emptied", "floor-emptied", "over"]);
    let ids: Vec<&str> = scored
        .candidates
        .iter()
        .map(|kept| kept.decision_object_id.as_str())
        .collect();
    assert_eq!(ids, ["plain", "at-bound"]);
}

#[test]
fn the_top_k_is_the_same_under_every_input_order() {
    let summaries = [
        ("tie-c", "alpha beta gamma delta"),
        ("tie-a", "alpha beta gamma delta"),
        ("tie-e", "alpha beta gamma delta"),
        ("tie-b", "alpha beta gamma delta"),
        ("tie-d", "alpha beta gamma delta"),
        ("more-terms", "alpha beta gamma delta omega"),
        ("wider", "alpha beta gamma delta epsilon zeta"),
        // Five matched of six is more matches at a lower score than the ties.
        ("lower-score", "alpha beta gamma delta epsilon omicron"),
        ("unscorable", "x y z"),
    ];
    let descriptor = "alpha beta gamma delta epsilon zeta";
    let ordered: Vec<CandidateInput<'_>> = summaries
        .iter()
        .map(|(id, summary)| candidate(id, summary))
        .collect();
    let expected = score_descriptor(descriptor, &ordered).unwrap();
    let ids: Vec<&str> = expected
        .candidates
        .iter()
        .map(|kept| kept.decision_object_id.as_str())
        .collect();
    // `wider` matches more terms at the same full score; the K+1 ties fall to id order.
    assert_eq!(ids, ["wider", "tie-a", "tie-b", "tie-c"]);
    assert_eq!(expected.candidates.len(), CANDIDATES_PER_DESCRIPTOR);
    assert_eq!(expected.unscorable, ["unscorable"]);
    let mut permutation = ordered.clone();
    for rotation in 0..permutation.len() {
        permutation.rotate_left(1);
        let mut reversed = permutation.clone();
        reversed.reverse();
        for order in [&permutation, &reversed] {
            assert_eq!(
                score_descriptor(descriptor, order).unwrap(),
                expected,
                "{rotation}"
            );
        }
    }
}

#[test]
fn a_decision_object_id_appears_once_and_the_better_entry_wins_in_any_order() {
    let inputs = [
        CandidateInput {
            decision_object_id: "same",
            decision_id: "weaker",
            summary: "alpha beta gamma delta omega",
        },
        CandidateInput {
            decision_object_id: "same",
            decision_id: "stronger",
            summary: "alpha beta gamma delta",
        },
    ];
    for order in [inputs, [inputs[1], inputs[0]]] {
        let scored = score_descriptor("alpha beta gamma delta", &order).unwrap();
        assert_eq!(
            scored.candidates,
            [ScoredCandidate {
                decision_object_id: "same".to_string(),
                decision_id: "stronger".to_string(),
                matched_terms: 4,
                decision_terms: 4,
            }]
        );
    }
}

#[test]
fn the_matcher_digest_pins_every_rule() {
    let (major, minor, update) = char::UNICODE_VERSION;
    let unicode = format!("{major}.{minor}.{update}");
    let expected: String = [
        "matcher_epoch",
        "contradiction-candidate-matcher.v1",
        "analysis_epoch",
        ANALYSIS_CONTRACT_EPOCH,
        "unicode",
        &unicode,
        "terms",
        "atoms-and-parts-as-a-set.v1",
        "fold",
        "str-to-lowercase-per-term.v1",
        "floor",
        "min-scalars,alphanumeric,not-all-numeric,stopwords.v1",
        "min_term_scalars",
        "3",
        "stopwords",
        "fc90dd770d6a61eb3ee45824f2414582ed5d5b56a0b6d04679a5314fc589324d",
        "score",
        "decision-side-containment.v1",
        "score_threshold",
        "1/2",
        "min_matched_terms",
        "3",
        "min_decision_terms",
        "4",
        "candidates_per_descriptor",
        "4",
        "order",
        "score-desc,matched-desc,decision-terms-asc,object-id-asc,decision-id-asc,one-per-object.v1",
        "descriptor_bounds",
        "65536/32769",
        "summary_bounds",
        "8192/4097",
    ]
    .iter()
    .map(|value| format!("{}:{value}\n", value.len()))
    .collect();
    assert_eq!(ANALYSIS_CONTRACT_EPOCH, "identifier-analysis.v1");
    assert_eq!(matcher_preimage(), expected);
    assert_eq!(
        CANDIDATE_MATCHER_EPOCH,
        "contradiction-candidate-matcher.v1"
    );
    assert_eq!(
        matcher_digest(),
        "63a46048304e06a0ff71bf491c9f83effd1c606d2cd393963695670c0a94b0bd",
        "a changed matcher rule changes the digest; update the epoch and the digest together"
    );
}

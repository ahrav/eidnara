//! Frozen-alias citations against hand-checked native oracles: every rendered part carries the alias, native message and block identity, and exact presented bytes the golden names; a fact set is accepted whole or rejected whole, with the rejection code the history still publishes alongside; truncation withdraws aliases the model could not see whole; and rebuilding from the same frozen messages, which is how a reattachment recovers the table, reproduces it byte for byte.

use std::sync::Arc;

use crate::history_summarizer_chunk::{
    HistorySummarizerBuiltChunk, alias_marker, build_history_summarizer_chunk, presented_input,
};
use crate::history_summarizer_citations::{ExtractionFailure, ExtractionOutcome};
use crate::history_summarizer_validate::{
    HistorySummarizerChunk, ValidateOptions, ValidatedChunk, validate_history_summarizer_output,
};
use crate::wire::{IngressMessage, project_messages};
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Deserialize)]
struct Golden {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    label: String,
    budget: usize,
    offset: u64,
    #[serde(rename = "eligibleEnd")]
    eligible_end: u64,
    ck: Vec<Arc<IngressMessage>>,
    #[serde(rename = "expectedText")]
    expected_text: String,
    #[serde(rename = "expectedAliases")]
    expected_aliases: Vec<ExpectedAlias>,
}

#[derive(Deserialize)]
struct ExpectedAlias {
    alias: String,
    #[serde(rename = "messageId")]
    message_id: String,
    ordinal: u64,
    #[serde(rename = "blockIds")]
    block_ids: Vec<String>,
    presented: String,
    transformed: bool,
}

fn golden() -> Golden {
    serde_json::from_str(include_str!(
        "../testdata/history_summarizer-citations-golden.json"
    ))
    .unwrap()
}

fn build(case: &Case) -> HistorySummarizerBuiltChunk {
    let projection = project_messages(&case.ck).unwrap();
    build_history_summarizer_chunk(
        &case.ck,
        &projection.blocks,
        case.offset,
        case.budget,
        case.eligible_end,
    )
}

/// One `<output>` with one segment covering `start..=end` and the given fact items.
fn output(start: u64, end: u64, facts: &[&str]) -> String {
    let items = facts
        .iter()
        .map(|fact| format!("* {fact}"))
        .collect::<Vec<_>>()
        .join("\n");
    let facts_block = if facts.is_empty() {
        String::new()
    } else {
        format!("<facts><PROJECT_RULES>\n{items}\n</PROJECT_RULES></facts>")
    };
    format!(
        r#"<output><history_segments><history_segment start="{start}" end="{end}" title="t" episode_type="feature" importance="50"><p1>full</p1><p2>short</p2><p3>t</p3><p4 /></history_segment></history_segments>{facts_block}<meta><unprocessed_from>{}</unprocessed_from></meta></output>"#,
        end + 1
    )
}

fn validate(text: &str, chunk: &HistorySummarizerChunk) -> ValidatedChunk {
    validate_history_summarizer_output(text, chunk, &[], ValidateOptions::default()).unwrap()
}

#[test]
fn rendered_aliases_match_the_hand_checked_native_oracles() {
    for case in &golden().cases {
        let built = build(case);
        assert_eq!(built.text, case.expected_text, "{}", case.label);
        assert_eq!(
            built.chunk.aliases.aliases.len(),
            case.expected_aliases.len(),
            "{}",
            case.label
        );
        let projection = project_messages(&case.ck).unwrap();
        for (frozen, expected) in built
            .chunk
            .aliases
            .aliases
            .iter()
            .zip(&case.expected_aliases)
        {
            assert_eq!(frozen.alias, expected.alias, "{}", case.label);
            assert_eq!(frozen.message_id, expected.message_id, "{}", case.label);
            assert_eq!(frozen.ordinal, expected.ordinal, "{}", case.label);
            assert_eq!(frozen.block_ids, expected.block_ids, "{}", case.label);
            assert_eq!(frozen.presented, expected.presented, "{}", case.label);
            assert_eq!(frozen.transformed, expected.transformed, "{}", case.label);
            // Block hashes are the native blocks' own serialized bytes, so a changed block is detectable without model text.
            for (block_id, hash) in frozen.block_ids.iter().zip(&frozen.block_hashes) {
                let block = projection
                    .blocks
                    .iter()
                    .find(|block| block.id == *block_id)
                    .unwrap();
                assert_eq!(
                    *hash,
                    format!("{:x}", Sha256::digest(block.bytes.as_bytes()))
                );
            }
            // The marker occurs once and is followed by exactly the presented bytes: native text cannot forge a marker.
            let marker = alias_marker(&frozen.alias);
            assert_eq!(built.text.matches(&marker).count(), 1, "{}", case.label);
            let at = built.text.find(&marker).unwrap();
            assert!(
                built.text[at + marker.len()..].starts_with(&frozen.presented),
                "{}",
                case.label
            );
        }
        assert_eq!(
            built.text.matches('\u{ab}').count(),
            case.expected_aliases.len(),
            "{}: only issued markers open a bracket",
            case.label
        );
        // A reattachment recovers the table by rebuilding from the same frozen messages, so the rebuild must be byte-identical, and the table must survive the chunk's own serialization.
        assert_eq!(build(case).chunk.aliases, built.chunk.aliases);
        let round_trip: HistorySummarizerChunk =
            serde_json::from_str(&serde_json::to_string(&built.chunk).unwrap()).unwrap();
        assert_eq!(round_trip.aliases, built.chunk.aliases);
    }
}

#[test]
fn the_prompt_notation_rule_ends_a_part_before_the_separator_the_renderer_emits() {
    // The renderer joins the parts of one line with ` / `, which is outside every part's presented bytes; the prompt's notation rule must draw the same boundary or a whole-part citation on a non-final part is an invalid span.
    let separator = " / ";
    let open = '\u{ab}';
    for case in &golden().cases {
        let built = build(case);
        for frozen in &built.chunk.aliases.aliases {
            assert!(
                !frozen.presented.ends_with(separator),
                "{}: {} presented ends with the separator",
                case.label,
                frozen.alias
            );
            let marker = alias_marker(&frozen.alias);
            let at = built.text.find(&marker).unwrap();
            let after = &built.text[at + marker.len() + frozen.presented.len()..];
            assert!(
                after.is_empty()
                    || after.starts_with('\n')
                    || after
                        .strip_prefix(separator)
                        .is_some_and(|next| next.starts_with(open)),
                "{}: {} is followed by {after:?}, not a line end or the separator and the next marker",
                case.label,
                frozen.alias
            );
        }
    }
    let notation = crate::history_summarizer_prompt::HISTORY_SUMMARIZER_SYSTEM_PROMPT
        .lines()
        .find(|line| line.contains("marks the start of one presented message part"))
        .expect("the prompt documents the marker notation");
    assert!(
        notation.contains("not including the ` / ` that precedes the next `\u{ab}` marker"),
        "the notation rule must exclude the inter-part separator: {notation}"
    );
}

#[test]
fn a_fact_set_is_accepted_whole_or_rejected_whole_while_history_publishes() {
    let cases = golden().cases;
    let built = build(&cases[1]);
    let chunk = &built.chunk;
    // s1 = "I will read it" (14 bytes), s3 = "first part / second part".
    let accepted = validate(
        &output(
            1,
            3,
            &[
                "[s1:0-14] Read before editing.",
                "[s3:0-10] [s1:2-6] Two citations.",
            ],
        ),
        chunk,
    );
    assert_eq!(
        accepted.extraction,
        ExtractionOutcome::Accepted { count: 2 }
    );
    assert_eq!(accepted.facts.len(), 2);
    assert_eq!(accepted.facts[1].citations.len(), 2);
    assert_eq!(accepted.facts[1].content, "Two citations.");
    assert_eq!(accepted.history_segments.len(), 1);
    // One invalid sibling rejects the whole set; the valid history still publishes.
    for (label, items, failure) in [
        (
            "unknown alias",
            vec!["[s1:0-14] ok", "[s9:0-3] unknown"],
            ExtractionFailure::UnknownAlias,
        ),
        (
            "empty range",
            vec!["[s1:0-14] ok", "[s1:5-5] empty"],
            ExtractionFailure::InvalidSpan,
        ),
        (
            "past the end",
            vec!["[s1:0-15] past"],
            ExtractionFailure::InvalidSpan,
        ),
        (
            "uncited sibling",
            vec!["[s1:0-14] ok", "no citation"],
            ExtractionFailure::MissingCitation,
        ),
        (
            "malformed",
            vec!["[s1:0-x] bad"],
            ExtractionFailure::MalformedCitation,
        ),
    ] {
        let validated = validate(&output(1, 3, &items), chunk);
        assert_eq!(
            validated.extraction,
            ExtractionOutcome::Rejected { failure },
            "{label}"
        );
        assert!(
            validated.facts.is_empty(),
            "{label}: no sibling is salvaged"
        );
        assert_eq!(
            validated.history_segments.len(),
            1,
            "{label}: history is untouched"
        );
    }
    // Intentional no-fact output and an absent facts block are not rejections.
    assert_eq!(
        validate(&output(1, 3, &[]), chunk).extraction,
        ExtractionOutcome::NotRequested
    );
    let empty_block = output(1, 3, &[]).replace(
        "<meta>",
        "<facts><PROJECT_RULES>\n</PROJECT_RULES></facts><meta>",
    );
    assert_eq!(
        validate(&empty_block, chunk).extraction,
        ExtractionOutcome::NoFacts
    );
    // Memory disabled: the same facts are not requested, whatever the model emitted.
    let disabled = validate_history_summarizer_output(
        &output(1, 3, &["[s1:0-14] ok"]),
        chunk,
        &[],
        ValidateOptions {
            memory_enabled: false,
            ..ValidateOptions::default()
        },
    )
    .unwrap();
    assert_eq!(disabled.extraction, ExtractionOutcome::NotRequested);
    assert!(disabled.facts.is_empty());
}

#[test]
fn multibyte_spans_must_fall_on_character_boundaries() {
    let cases = golden().cases;
    let built = build(&cases[3]);
    // "α🙂 中文 rule": α is bytes 0..2, 🙂 is 2..6, a space, 中文 is 7..13.
    let ok = validate(
        &output(1, 1, &["[s1:0-6] alpha and a face", "[s1:7-13] chinese"]),
        &built.chunk,
    );
    assert_eq!(ok.extraction, ExtractionOutcome::Accepted { count: 2 });
    for (label, item) in [
        ("inside α", "[s1:1-6] x"),
        ("inside 🙂", "[s1:0-4] x"),
        ("inside 中", "[s1:7-8] x"),
    ] {
        let validated = validate(&output(1, 1, &[item]), &built.chunk);
        assert_eq!(
            validated.extraction,
            ExtractionOutcome::Rejected {
                failure: ExtractionFailure::InvalidSpan
            },
            "{label}"
        );
    }
}

#[test]
fn citations_outside_the_finally_accepted_segment_are_rejected() {
    // Four messages; the model emits two segments and the second is provisional, so boundary healing discards it. A fact citing a part in the discarded segment is outside the accepted history.
    let ck: Vec<Arc<IngressMessage>> = serde_json::from_value(serde_json::json!([
        {"mid":"u1","ordinal":1,"ck":{"role":"user","content":[{"kind":{"type":"text","text":"one"}}]}},
        {"mid":"a2","ordinal":2,"ck":{"role":"assistant","content":[{"kind":{"type":"text","text":"two"}}]}},
        {"mid":"u3","ordinal":3,"ck":{"role":"user","content":[{"kind":{"type":"text","text":"three"}}]}},
        {"mid":"a4","ordinal":4,"ck":{"role":"assistant","content":[{"kind":{"type":"text","text":"four"}}]}}
    ]))
    .unwrap();
    let projection = project_messages(&ck).unwrap();
    let built = build_history_summarizer_chunk(&ck, &projection.blocks, 1, 10_000, 5);
    assert_eq!(built.chunk.aliases.aliases.len(), 4);
    let text = r#"<output><history_segments><history_segment start="1" end="2" title="a" episode_type="feature" importance="50"><p1>a</p1><p2>a</p2><p3>a</p3><p4 /></history_segment><history_segment start="3" end="4" title="b" episode_type="feature" importance="50"><p1>b</p1><p2>b</p2><p3>b</p3><p4 /></history_segment></history_segments><facts><PROJECT_RULES>
* [s1:0-3] cited inside the kept segment
* [s4:0-4] cited inside the discarded segment
</PROJECT_RULES></facts><meta><unprocessed_from>5</unprocessed_from></meta></output>"#;
    let validated = validate(text, &built.chunk);
    assert!(validated.discarded_last);
    assert_eq!(validated.history_segments.len(), 1);
    assert_eq!(
        validated.extraction,
        ExtractionOutcome::Rejected {
            failure: ExtractionFailure::OutsideAcceptedSegment
        }
    );
    assert!(validated.facts.is_empty());
    // The same fact set with only the kept citation is accepted against the healed boundary.
    let kept = text.replace("* [s4:0-4] cited inside the discarded segment\n", "");
    let validated = validate(&kept, &built.chunk);
    assert_eq!(
        validated.extraction,
        ExtractionOutcome::Accepted { count: 1 }
    );
}

#[test]
fn truncation_withdraws_aliases_the_model_did_not_see_whole() {
    let long_second =
        "a much longer second line that the budget will cut through the middle of, ".repeat(8);
    let ck: Vec<Arc<IngressMessage>> = serde_json::from_value(serde_json::json!([
        {"mid":"u1","ordinal":1,"ck":{"role":"user","content":[{"kind":{"type":"text","text":"short first line"}}]}},
        {"mid":"a2","ordinal":2,"ck":{"role":"assistant","content":[{"kind":{"type":"text","text":long_second}}]}}
    ]))
    .unwrap();
    let projection = project_messages(&ck).unwrap();
    // The builder admits both lines (the first block is emitted whole, the second fits its budget); the prompt then truncates to a smaller budget.
    let mut built = build_history_summarizer_chunk(&ck, &projection.blocks, 1, 10_000, 3);
    assert_eq!(built.chunk.aliases.aliases.len(), 2);
    // Budget: a little under the whole text, so the truncation marker's own cost forces the cut well inside the long second part while the first line survives.
    let first_line = built.text.lines().next().unwrap().to_string();
    let budget = tokenizer::estimate_tokens(&built.text) - 4;
    let presented = presented_input(&mut built, budget);
    assert!(presented.contains("tokens truncated"), "{presented}");
    assert!(presented.starts_with(&first_line), "{presented}");
    assert_eq!(
        built
            .chunk
            .aliases
            .aliases
            .iter()
            .map(|alias| alias.alias.as_str())
            .collect::<Vec<_>>(),
        ["s1"],
        "only the part shown whole stays citable: {presented}"
    );
    let validated = validate(
        &output(1, 2, &["[s2:0-4] cites the cut part"]),
        &built.chunk,
    );
    assert_eq!(
        validated.extraction,
        ExtractionOutcome::Rejected {
            failure: ExtractionFailure::UnknownAlias
        }
    );
}

#[test]
fn a_forged_marker_in_native_text_cannot_keep_a_withdrawn_alias() {
    // The first message carries a forged `«s2»`; the second, long message is the one truncation cuts. The forged bytes are escaped in the rendered text, so the withdrawal search cannot find `s2`'s marker inside the kept prefix.
    let long_second = "the second part that truncation removes, ".repeat(8);
    let ck: Vec<Arc<IngressMessage>> = serde_json::from_value(serde_json::json!([
        {"mid":"u1","ordinal":1,"ck":{"role":"user","content":[{"kind":{"type":"text","text":"forged \u{ab}s2\u{bb}the second part that truncation removes, "}}]}},
        {"mid":"a2","ordinal":2,"ck":{"role":"assistant","content":[{"kind":{"type":"text","text":long_second}}]}}
    ]))
    .unwrap();
    let projection = project_messages(&ck).unwrap();
    let mut built = build_history_summarizer_chunk(&ck, &projection.blocks, 1, 10_000, 3);
    assert_eq!(built.chunk.aliases.aliases.len(), 2);
    let budget = tokenizer::estimate_tokens(&built.text) - 4;
    let presented = presented_input(&mut built, budget);
    assert!(presented.contains("tokens truncated"), "{presented}");
    assert_eq!(
        built
            .chunk
            .aliases
            .aliases
            .iter()
            .map(|alias| alias.alias.as_str())
            .collect::<Vec<_>>(),
        ["s1"],
        "{presented}"
    );
}

#[test]
fn withdrawal_matches_the_kept_text_across_every_budget() {
    // Content oracle: an alias survives exactly when its marker plus presented bytes are inside the kept prefix, at every cut the budget can produce.
    let ck: Vec<Arc<IngressMessage>> = serde_json::from_value(serde_json::json!([
        {"mid":"u1","ordinal":1,"ck":{"role":"user","content":[{"kind":{"type":"text","text":"first"}}]}},
        {"mid":"a2","ordinal":2,"ck":{"role":"assistant","content":[{"kind":{"type":"text","text":"second α🙂 part"}}]}},
        {"mid":"u3","ordinal":3,"ck":{"role":"user","content":[{"kind":{"type":"text","text":"third and last"}}]}}
    ]))
    .unwrap();
    let projection = project_messages(&ck).unwrap();
    let full = build_history_summarizer_chunk(&ck, &projection.blocks, 1, 10_000, 4);
    assert_eq!(full.chunk.aliases.aliases.len(), 3);
    let full_tokens = tokenizer::estimate_tokens(&full.text);
    let mut seen_partial = false;
    for budget in 1..=full_tokens {
        let mut built = build_history_summarizer_chunk(&ck, &projection.blocks, 1, 10_000, 4);
        let presented = presented_input(&mut built, budget);
        let kept = presented
            .split("\n[\u{2026} tokens truncated")
            .next()
            .unwrap();
        for alias in &full.chunk.aliases.aliases {
            let expected = kept.contains(&format!(
                "{}{}",
                alias_marker(&alias.alias),
                alias.presented
            ));
            assert_eq!(
                built.chunk.aliases.resolve(&alias.alias).is_some(),
                expected,
                "budget {budget}, alias {}: {presented}",
                alias.alias
            );
        }
        let kept_count = built.chunk.aliases.aliases.len();
        seen_partial |= kept_count > 0 && kept_count < 3;
    }
    assert!(seen_partial, "some budget must keep a strict subset");
}

#[test]
fn an_oversized_first_block_at_the_same_budget_is_admitted_then_withdrawn() {
    // Production builds and presents under one budget. The only cut that budget can produce is a first block the builder admits whole because nothing precedes it; presenting it truncates, and its alias goes with the cut.
    let huge = "an enormous first message that no budget of this size can hold, ".repeat(40);
    let ck: Vec<Arc<IngressMessage>> = serde_json::from_value(serde_json::json!([
        {"mid":"u1","ordinal":1,"ck":{"role":"user","content":[{"kind":{"type":"text","text":huge}}]}},
        {"mid":"a2","ordinal":2,"ck":{"role":"assistant","content":[{"kind":{"type":"text","text":"never fits after that"}}]}}
    ]))
    .unwrap();
    let projection = project_messages(&ck).unwrap();
    let budget = 60;
    let mut built = build_history_summarizer_chunk(&ck, &projection.blocks, 1, budget, 3);
    assert_eq!(built.chunk.aliases.aliases.len(), 1, "{}", built.text);
    assert!(tokenizer::estimate_tokens(&built.text) > budget);
    let presented = presented_input(&mut built, budget);
    assert!(presented.contains("tokens truncated"));
    assert!(built.chunk.aliases.aliases.is_empty());
    let validated = validate(
        &output(1, 1, &["[s1:0-10] cites the cut part"]),
        &built.chunk,
    );
    assert_eq!(
        validated.extraction,
        ExtractionOutcome::Rejected {
            failure: ExtractionFailure::UnknownAlias
        }
    );
}

#[test]
fn appending_a_tail_does_not_change_the_frozen_range_aliases() {
    // A reattachment rebuilds the table from the frozen range while the transcript may have grown; the frozen range's aliases must not depend on what follows it.
    let cases = golden().cases;
    let case = &cases[1];
    let before = build(case).chunk.aliases;
    let mut ck = case.ck.clone();
    let tail: Vec<Arc<IngressMessage>> = serde_json::from_value(serde_json::json!([
        {"mid":"u9","ordinal":9,"ck":{"role":"user","content":[{"kind":{"type":"text","text":"a later message"}}]}}
    ]))
    .unwrap();
    ck.extend(tail);
    let projection = project_messages(&ck).unwrap();
    let after = build_history_summarizer_chunk(
        &ck,
        &projection.blocks,
        case.offset,
        case.budget,
        case.eligible_end,
    );
    assert_eq!(after.chunk.aliases, before);
}

#[test]
fn a_force_kept_final_segment_is_citable_and_an_unwrapped_block_is_still_a_fact_set() {
    let cases = golden().cases;
    let built = build(&cases[1]);
    let chunk = &built.chunk;
    // Wrapup keeps the final segment; a citation into it is a citation into persisted history, so a one-segment wrapup can admit facts.
    let kept = validate_history_summarizer_output(
        &output(
            1,
            3,
            &["[s3:0-10] cited inside the force-kept final segment"],
        ),
        chunk,
        &[],
        ValidateOptions {
            force_keep_last_history_segment: true,
            ..ValidateOptions::default()
        },
    )
    .unwrap();
    assert_eq!(kept.history_segments.len(), 1);
    assert_eq!(kept.extraction, ExtractionOutcome::Accepted { count: 1 });
    // Fact material outside a `<facts>` wrapper is still a proposed set: it is judged, not reported as extraction-free.
    let unwrapped = output(1, 3, &[]).replace(
        "<meta>",
        "<PROJECT_RULES>\n* uncited outside the wrapper\n</PROJECT_RULES><meta>",
    );
    assert_eq!(
        validate(&unwrapped, chunk).extraction,
        ExtractionOutcome::Rejected {
            failure: ExtractionFailure::MissingCitation
        }
    );
    // Material inside `<facts>` that is not a category block or a bullet is malformed, not an absent fact; so is a second `<facts>` block.
    for (label, text) in [
        (
            "prose beside the categories",
            output(1, 3, &["[s1:0-14] ok"]).replace("<facts>", "<facts>Here are the facts:"),
        ),
        (
            "a non-bullet line inside a category",
            output(1, 3, &["[s1:0-14] ok"])
                .replace("\n</PROJECT_RULES>", "\nnot a bullet\n</PROJECT_RULES>"),
        ),
        (
            "an unknown category",
            output(1, 3, &["[s1:0-14] ok"]).replace("</facts>", "<OTHER>\n* x\n</OTHER></facts>"),
        ),
        (
            "a second facts block",
            output(1, 3, &["[s1:0-14] ok"]).replace("<meta>", "<facts></facts><meta>"),
        ),
        // A category whose closing tag names a different category is unreadable: its items must not vanish into no-fact success.
        (
            "a mismatched category close tag as the only block",
            output(1, 3, &["[s1:0-14] ok"]).replace("</PROJECT_RULES>", "</ARCHITECTURE>"),
        ),
        // Nor may a well-formed sibling block be salvaged around it.
        (
            "a mismatched category beside a valid one",
            output(1, 3, &["[s1:0-14] ok"]).replace(
                "</facts>",
                "<CONSTRAINTS>\n* [s1:0-14] dropped\n</NAMING></facts>",
            ),
        ),
    ] {
        let validated = validate(&text, chunk);
        assert_eq!(
            validated.extraction,
            ExtractionOutcome::Rejected {
                failure: ExtractionFailure::MalformedFacts
            },
            "{label}"
        );
        assert_eq!(validated.history_segments.len(), 1, "{label}");
    }
    // An item that is only a citation is malformed, even beside a valid sibling.
    let citation_only = validate(&output(1, 3, &["[s1:0-14] ok", "[s1:0-14]"]), chunk);
    assert_eq!(
        citation_only.extraction,
        ExtractionOutcome::Rejected {
            failure: ExtractionFailure::MalformedCitation
        }
    );
    assert!(citation_only.facts.is_empty());
}

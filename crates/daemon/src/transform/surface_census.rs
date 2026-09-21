use std::any::type_name_of_val;
use std::cell::Cell;

use eval_core::{SURFACE1_HINT_BOUNDS, SURFACE1_STAGES, Surface1Stage};
use memory_store::{MemoryStore, StoredHistorySegment};
use serde_json::json;

use super::tests::{active_cc_req, comp, run, spine, store, wire_item};
use super::*;
use crate::memory_tool::{MemorySearchResult, MemorySearchSourceKind};

const SESSION: &str = "census";
const FILLER: &str = "filler sentence about nothing in particular";
const MATCHING_PROMPT: &str = "quasar nebula survey cadence decision";

fn segment(sequence: i64, text: &str) -> StoredHistorySegment {
    comp(sequence, sequence, sequence, &format!("m{sequence}"), text)
}

fn seeded(dir: &std::path::Path, texts: &[&str]) -> MemoryStore {
    let s = store(dir);
    let segments = texts
        .iter()
        .enumerate()
        .map(|(index, text)| segment(index as i64 + 1, text))
        .collect::<Vec<_>>();
    s.replace_history_segments(SESSION, &segments).unwrap();
    s
}

fn search(s: &MemoryStore, query: &str) -> Result<Vec<MemorySearchResult>, TransformError> {
    run_user_hint_lexical_search(
        s,
        SESSION,
        query,
        DEFAULT_AUTO_SEARCH_SCORE_THRESHOLD,
        &mut UserHintTrace::default(),
    )
}

fn sequences(results: &[MemorySearchResult]) -> Vec<i64> {
    results.iter().map(|result| result.id).collect()
}

fn hint_result(snippet: &str) -> MemorySearchResult {
    MemorySearchResult {
        source_kind: MemorySearchSourceKind::HistorySegmentBody,
        id: 1,
        snippet: snippet.to_string(),
        category: None,
        sequence: Some(1),
        title: Some("C1".to_string()),
        note_status: None,
        surface_condition: None,
    }
}

fn hint_queries_over_two_passes(request: &TransformRequest) -> usize {
    let dir = tempfile::tempdir().unwrap();
    let s = store(dir.path());
    USER_HINT_LEXICAL_QUERY_COUNT.with(|count| count.set(0));
    run(&s, request, &spine());
    run(&s, request, &spine());
    USER_HINT_LEXICAL_QUERY_COUNT.with(Cell::get)
}

fn prompt_request(prompt: &str) -> TransformRequest {
    let mut request = active_cc_req(SESSION, "cfg0", vec![wire_item("user", "m1", 1, &[prompt])]);
    request.auto_search_min_prompt_chars = DEFAULT_AUTO_SEARCH_MIN_PROMPT_CHARS;
    request
}

#[test]
fn auto_search_is_enabled_by_the_wire_default_and_drives_the_hint_query() {
    assert!(default_auto_search_enabled());
    let explicit = prompt_request("a request long enough to clear the prompt gate");
    let wire = serde_json::to_value(&explicit).unwrap();
    assert!(
        wire.get("auto_search_enabled").is_none(),
        "the default is omitted on the wire, so the reader's default decides"
    );
    let from_wire: TransformRequest = serde_json::from_value(wire).unwrap();
    assert!(from_wire.auto_search_enabled);
    assert!(
        hint_queries_over_two_passes(&from_wire) >= 1,
        "the default request reaches the hint query"
    );

    let mut disabled = explicit.clone();
    disabled.auto_search_enabled = false;
    assert_eq!(hint_queries_over_two_passes(&disabled), 0);
    let mut subagent = explicit;
    subagent.is_subagent = true;
    assert_eq!(hint_queries_over_two_passes(&subagent), 0);

    let minimal: TransformRequest = serde_json::from_value(json!({
        "kind": "transform",
        "v": 2,
        "serializer_profile": "claude-code-anthropic",
        "session_id": SESSION,
        "render_config": "cfg0",
        "messages": [],
    }))
    .unwrap();
    assert!(minimal.auto_search_enabled);
    assert_eq!(
        minimal.auto_search_score_threshold,
        DEFAULT_AUTO_SEARCH_SCORE_THRESHOLD
    );
    assert_eq!(
        minimal.auto_search_min_prompt_chars,
        DEFAULT_AUTO_SEARCH_MIN_PROMPT_CHARS
    );
    assert!(
        !minimal.serve_native,
        "attachment waits for the plugin's serve_native opt-in"
    );
}

#[test]
fn surface1_stages_are_anchored_to_production_symbols_in_pinned_order() {
    let rows: [(Surface1Stage, &str, String); 13] = [
        (
            Surface1Stage::TailEligibility,
            "daemon::transform::eligible_authored_user_tail",
            type_name_of_val(&eligible_authored_user_tail).to_string(),
        ),
        (
            Surface1Stage::Suppression,
            "daemon::transform::has_stacked_user_hint_augmentation",
            type_name_of_val(&has_stacked_user_hint_augmentation).to_string(),
        ),
        (
            Surface1Stage::LengthGate,
            "daemon::transform::default_auto_search_min_prompt_chars",
            type_name_of_val(&default_auto_search_min_prompt_chars).to_string(),
        ),
        (
            Surface1Stage::TokenGate,
            "USER_HINT_MIN_MATCHED_TOKENS=2",
            format!("USER_HINT_MIN_MATCHED_TOKENS={USER_HINT_MIN_MATCHED_TOKENS}"),
        ),
        (
            Surface1Stage::CandidateWindow,
            "memory_store::MemoryStore::load_history_segment_candidates",
            type_name_of_val(&MemoryStore::load_history_segment_candidates).to_string(),
        ),
        (
            Surface1Stage::MatchFilter,
            "daemon::transform::run_user_hint_lexical_search",
            type_name_of_val(&run_user_hint_lexical_search).to_string(),
        ),
        (
            Surface1Stage::Threshold,
            "daemon::transform::default_auto_search_score_threshold",
            type_name_of_val(&default_auto_search_score_threshold).to_string(),
        ),
        (
            Surface1Stage::Cap,
            "USER_HINT_RESULT_LIMIT=3",
            format!("USER_HINT_RESULT_LIMIT={USER_HINT_RESULT_LIMIT}"),
        ),
        (
            Surface1Stage::Render,
            "daemon::transform::render_user_hint",
            type_name_of_val(&render_user_hint).to_string(),
        ),
        (
            Surface1Stage::DecisionFreeze,
            "memory_store::MemoryStore::load_user_hints",
            type_name_of_val(&MemoryStore::load_user_hints).to_string(),
        ),
        (
            Surface1Stage::Deferral,
            "daemon::transform::user_hint_target_was_served",
            type_name_of_val(&user_hint_target_was_served).to_string(),
        ),
        (
            Surface1Stage::OverlayApply,
            "daemon::transform::user_hint_block_kind",
            type_name_of_val(&user_hint_block_kind).to_string(),
        ),
        (
            Surface1Stage::Attachment,
            "daemon::attach_native_messages_incremental",
            type_name_of_val(&crate::attach_native_messages_incremental).to_string(),
        ),
    ];
    let stages = rows.iter().map(|(stage, _, _)| *stage).collect::<Vec<_>>();
    assert_eq!(stages, SURFACE1_STAGES);
    for (stage, expected, observed) in &rows {
        assert_eq!(observed, expected, "{stage:?}");
    }
}

#[test]
fn suppression_and_the_length_gate_return_before_any_query() {
    assert_eq!(
        hint_queries_over_two_passes(&prompt_request("quasar nebula")),
        0,
        "short prompt"
    );
    assert_eq!(
        hint_queries_over_two_passes(&prompt_request(
            "<eidnara-search-hint>already augmented request text</eidnara-search-hint>"
        )),
        0,
        "stacked augmentation"
    );
    assert!(hint_queries_over_two_passes(&prompt_request(MATCHING_PROMPT)) >= 1);
}

#[test]
fn the_token_gate_returns_before_the_candidate_window_reads_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let s = seeded(dir.path(), &["quasar nebula pulsar", FILLER, FILLER]);
    let candidate_reads = |s: &MemoryStore| {
        s.statement_evictions()
            .keys()
            .filter(|sql| sql.contains("FROM history_segments"))
            .count()
    };
    s.start_statement_reuse_probe();
    assert!(search(&s, "quasar").unwrap().is_empty());
    assert_eq!(candidate_reads(&s), 0);
    assert_eq!(sequences(&search(&s, "quasar nebula").unwrap()), [1]);
    assert_eq!(candidate_reads(&s), 1, "two tokens reach the store read");
}

#[test]
fn the_threshold_empties_a_result_set_the_cap_would_otherwise_trim() {
    let dir = tempfile::tempdir().unwrap();
    let mut texts = vec!["quasar nebula orbit"; 5];
    texts.extend(std::iter::repeat_n(FILLER, 7));
    let s = seeded(dir.path(), &texts);
    assert!(
        search(&s, "quasar nebula pulsar magnetar")
            .unwrap()
            .is_empty(),
        "five candidates matching half the query score below the threshold"
    );
    assert_eq!(
        search(&s, "quasar nebula").unwrap().len(),
        SURFACE1_HINT_BOUNDS.results
    );
}

#[test]
fn hint_bound_constants_equal_the_evaluator_pins() {
    let bounds = SURFACE1_HINT_BOUNDS;
    assert_eq!(USER_HINT_CANDIDATE_LIMIT, bounds.candidates);
    assert_eq!(USER_HINT_TOKEN_CAP, bounds.query_tokens);
    assert_eq!(USER_HINT_RESULT_LIMIT, bounds.results);
    assert_eq!(USER_HINT_MIN_MATCHED_TOKENS, bounds.matched_tokens);
    assert_eq!(USER_HINT_FRAGMENT_CHAR_CAP, bounds.fragment_units);
    assert_eq!(USER_HINT_TOTAL_CHAR_CAP, bounds.total_units);
}

#[test]
fn the_candidate_window_holds_exactly_one_hundred_segments() {
    let mut texts = vec!["quasar nebula pulsar"];
    texts.extend(std::iter::repeat_n(
        FILLER,
        SURFACE1_HINT_BOUNDS.candidates - 1,
    ));
    {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            sequences(&search(&seeded(dir.path(), &texts), "quasar nebula").unwrap()),
            [1],
            "the oldest of 100 segments is inside the window"
        );
    }
    texts.push(FILLER);
    let dir = tempfile::tempdir().unwrap();
    assert!(
        search(&seeded(dir.path(), &texts), "quasar nebula")
            .unwrap()
            .is_empty(),
        "the oldest of 101 segments is outside the window"
    );
}

#[test]
fn the_query_keeps_twenty_four_tokens() {
    let query = (1..=25)
        .map(|n| format!("term{n:02}"))
        .collect::<Vec<_>>()
        .join(" ");
    let tokens = lexical_tokens(&query);
    assert_eq!(tokens.len(), SURFACE1_HINT_BOUNDS.query_tokens);
    assert!(!tokens.contains("term25"), "the 25th token is dropped");
    assert!(tokens.contains("term24"));
}

#[test]
fn three_results_survive_the_cap() {
    let mut texts = vec!["quasar nebula pulsar"; 5];
    texts.extend(std::iter::repeat_n(FILLER, 7));
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        search(&seeded(dir.path(), &texts), "quasar nebula")
            .unwrap()
            .len(),
        SURFACE1_HINT_BOUNDS.results
    );
}

#[test]
fn two_matched_tokens_gate_the_query_and_filter_candidates() {
    let dir = tempfile::tempdir().unwrap();
    let s = seeded(
        dir.path(),
        &[
            "quasar nebula pulsar",
            "quasar alone here",
            FILLER,
            FILLER,
            FILLER,
            FILLER,
        ],
    );
    assert!(
        search(&s, "quasar").unwrap().is_empty(),
        "one query token is gated"
    );
    assert_eq!(
        sequences(&search(&s, "quasar nebula").unwrap()),
        [1],
        "a candidate matching one token is filtered"
    );
}

#[test]
fn fragments_are_cut_at_eighty_units() {
    let long_snippet = (0..60)
        .map(|n| format!("quasarfield{n:02}"))
        .collect::<Vec<_>>()
        .join(" ");
    let rendered = render_user_hint(&[hint_result(&long_snippet)]).unwrap();
    let line = rendered
        .lines()
        .find(|line| line.starts_with("- "))
        .unwrap();
    assert_eq!(utf16_len(&line[2..]), SURFACE1_HINT_BOUNDS.fragment_units);
    assert!(line.ends_with('…'));
    let short = render_user_hint(&[hint_result("quasar nebula pulsar")]).unwrap();
    assert!(short.contains("- quasar nebula pulsar\n"));
}

#[test]
fn the_whole_hint_is_cut_at_eight_hundred_units() {
    let total = SURFACE1_HINT_BOUNDS.total_units;
    let oversized = format!(
        "<eidnara-search-hint>\n{}\n</eidnara-search-hint>",
        "x".repeat(2 * total)
    );
    let capped = truncate_hint_to_total_cap(&oversized, USER_HINT_TOTAL_CHAR_CAP);
    assert_eq!(utf16_len(&capped), total);
    assert!(capped.ends_with("…\n</eidnara-search-hint>"));
    let short = render_user_hint(&[hint_result("quasar nebula pulsar")]).unwrap();
    assert_eq!(
        truncate_hint_to_total_cap(&short, USER_HINT_TOTAL_CHAR_CAP),
        short
    );
}

//! The stale-preference harness contracts: the fact world, the request-level
//! arms over served m0 and m1 history, the export of a harness capture, and
//! the merge of independent sessions.

use std::collections::{BTreeMap, BTreeSet};

use eval_core::{
    ACKNOWLEDGEMENT, Block, CaptureError, FactPair, HistoryAt, M1_PLACEHOLDER, MAX_SUBJECTS,
    PRECEDENCE_SENTENCE, STALE_CAPTURE_SCHEMA, STALE_EXPORT_SCHEMA, STALE_WORLD_SCHEMA,
    SegmentTiers, StaleCapture, StaleDelivery, StaleExport, carries, export_capture, fact_world,
    merge_exports, with_parts,
};
use serde_json::{Value, json};

const SEED: u64 = 0x5EED_B000_0000_0002;

#[test]
fn the_fact_world_states_then_restates_every_subject_then_reports_progress() {
    let world = fact_world(SEED, 24);
    assert_eq!(
        world,
        fact_world(SEED, 24),
        "a pure function of seed and size"
    );
    assert_ne!(world, fact_world(SEED + 1, 24));
    assert_eq!(world.schema, STALE_WORLD_SCHEMA);
    assert_eq!((world.turns.len(), world.pairs.len()), (72, 24));
    let mut values = BTreeSet::new();
    let mut keys = BTreeSet::new();
    for pair in &world.pairs {
        assert!(pair.stale_turn < 24 && (24..48).contains(&pair.restating_turn));
        assert_eq!(
            world.turns[pair.stale_turn as usize].user,
            pair.stale_statement
        );
        assert_eq!(
            world.turns[pair.restating_turn as usize].user,
            pair.restatement
        );
        // Only order says which value is current.
        assert_eq!(
            pair.restatement,
            format!("Set the {} to {}.", pair.subject, pair.live_value)
        );
        assert!(carries(&pair.stale_statement, &pair.stale_value));
        assert!(!carries(&pair.restatement, &pair.stale_value));
        assert!(pair.question.contains(&pair.subject));
        assert!(
            !carries(&pair.question, &pair.stale_value)
                && !carries(&pair.question, &pair.live_value)
        );
        assert!(values.insert(pair.stale_value.clone()) && values.insert(pair.live_value.clone()));
        assert!(keys.insert(pair.key.clone()));
    }
    for value in &values {
        let stated = world
            .turns
            .iter()
            .filter(|turn| carries(&turn.user, value))
            .count();
        assert_eq!(stated, 1, "{value}");
    }
    for turn in &world.turns[48..] {
        assert!(turn.user.starts_with("Status note"));
        assert!(
            !world
                .pairs
                .iter()
                .any(|pair| turn.user.contains(&pair.subject))
        );
    }
    assert!(
        world
            .turns
            .iter()
            .all(|turn| turn.assistant == ACKNOWLEDGEMENT)
    );
    assert_eq!(fact_world(SEED, MAX_SUBJECTS).pairs.len(), MAX_SUBJECTS);
}

#[test]
#[should_panic(expected = "at most")]
fn a_world_names_no_more_subjects_than_it_has() {
    fact_world(SEED, MAX_SUBJECTS + 1);
}

/// The one pair of a one-subject world: stated on turn 0 (message 1),
/// restated on turn 1 (message 3).
fn pair() -> FactPair {
    fact_world(SEED, 1).pairs.remove(0)
}

fn m0(body: &str) -> String {
    format!(
        "<project-docs>\nAGENTS.md\n</project-docs>\n\n<session-history>\n{body}\n</session-history>"
    )
}

fn stale_body(pair: &FactPair) -> String {
    format!(
        "## 1-2 · Set {} {}\nSet the {} to {}; that {} stays until changed.",
        pair.subject, pair.stale_value, pair.subject, pair.stale_value, pair.stale_value
    )
}

fn m1_with(pair: &FactPair) -> String {
    format!(
        "<session-history-since>\n<new-history_segments>\n## 3-4 · Later\nSet the {} to {}.\n</new-history_segments>\n</session-history-since>",
        pair.subject, pair.live_value
    )
}

fn request(m0: &str, m1: &str, tail: &str) -> Value {
    json!({
        "system": "## Eidnara",
        "messages": [
            {"role": "user", "content": [{"type": "text", "text": m0}, {"type": "text", "text": m1}]},
            {"role": "assistant", "content": [{"type": "text", "text": tail}]},
            {"role": "user", "content": "§7§ What is it now?"},
        ],
    })
}

fn capture(request: Value, segments: Vec<SegmentTiers>) -> StaleCapture {
    let world = fact_world(SEED, 1);
    StaleCapture {
        schema: STALE_CAPTURE_SCHEMA.to_string(),
        harness: "opencode".to_string(),
        summarizer: "fixture/scripted".to_string(),
        requests: BTreeMap::from([(world.pairs[0].task.clone(), request)]),
        world,
        segments,
    }
}

fn row(start: i64, end: i64, p1: &str, p2: &str) -> SegmentTiers {
    SegmentTiers {
        start_message: start,
        end_message: end,
        p1: Some(p1.to_string()),
        p2: Some(p2.to_string()),
        p3: Some(format!("messages {start} to {end}")),
        p4: Some(String::new()),
    }
}

fn part(request: &Value, message: usize, part: usize) -> String {
    request["messages"][message]["content"][part]["text"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn arms_change_the_served_history_where_d7_and_d8_put_their_bytes() {
    let pair = pair();
    let body = stale_body(&pair);
    let (m0, m1) = (m0(&body), m1_with(&pair));
    let served = request(&m0, &m1, "§5§ Noted.");
    let stored = row(1, 2, body.split_once('\n').unwrap().1, "condensed");
    let export = export_capture(&capture(served.clone(), vec![stored])).unwrap();
    assert_eq!(export.schema, STALE_EXPORT_SCHEMA);
    let exported = &export.pairs[0];
    assert_eq!(exported.request, served, "arm (a) is the request as served");
    assert_eq!(
        exported.history,
        HistoryAt {
            message: 0,
            part: 0
        }
    );
    assert_eq!(
        (
            exported.restating_ordinal,
            exported.restatement,
            exported.stale_block,
            exported.stale_tier
        ),
        (3, Block::M1, Block::M0, 1)
    );
    assert_eq!(exported.delivery, StaleDelivery::Both);
    // The heading keeps the value after (d) and (e): D-7 rewrites the body's
    // anchor, never the title, so the export flags the leak.
    assert!(exported.stale_elsewhere);
    // Both body occurrences, never the heading or the project docs.
    assert_eq!(exported.stale_spans.len(), 2);
    let arm = |patch| with_parts(&served, &[patch]);
    // (b): the override sentence heads the m1 delta; m0 is untouched.
    let b = arm(&exported.arms.precedence_line);
    assert_eq!(part(&b, 0, 0), m0);
    assert_eq!(
        part(&b, 0, 1),
        m1.replacen(
            "<session-history-since>\n",
            &format!("<session-history-since>\n<memory-updates>\n{PRECEDENCE_SENTENCE}\n</memory-updates>\n"),
            1
        )
    );
    // (c): one footer line ends the stale segment's body, stale prose kept.
    let footer = format!("[corrections: {} = {} @3]", pair.key, pair.live_value);
    assert_eq!(
        part(&arm(&exported.arms.footer), 0, 0),
        m0.replacen(&body, &format!("{body}\n{footer}"), 1)
    );
    // (d) and (e): every occurrence in the stale segment's body, and no other.
    let marker = format!("[corrected @3: {} = {}]", pair.key, pair.live_value);
    let heading = body.split_once('\n').unwrap().0;
    let rest = body.split_once('\n').unwrap().1;
    assert_eq!(
        part(&arm(&exported.arms.anchored_replacement), 0, 0),
        m0.replacen(rest, &rest.replace(&pair.stale_value, &marker), 1)
    );
    let omitted = part(&arm(&exported.arms.omission_oracle), 0, 0);
    assert_eq!(
        omitted,
        m0.replacen(rest, &rest.replace(&pair.stale_value, ""), 1)
    );
    assert!(omitted.contains(heading), "the heading is left as served");
}

#[test]
fn an_empty_m1_delta_gets_a_memory_updates_block_and_a_missing_m1_is_refused() {
    let pair = pair();
    let served = request(&m0(&stale_body(&pair)), M1_PLACEHOLDER, "§3§ Set it.");
    let export = export_capture(&capture(served.clone(), vec![])).unwrap();
    let exported = &export.pairs[0];
    assert_eq!(exported.restatement, Block::Raw);
    assert_eq!(exported.stale_tier, 0, "no stored row matches");
    assert_eq!(
        part(
            &with_parts(&served, &[&exported.arms.precedence_line]),
            0,
            1
        ),
        format!(
            "<session-history-since>\n<memory-updates>\n{PRECEDENCE_SENTENCE}\n</memory-updates>\n</session-history-since>"
        )
    );
    let mut no_m1 = served.clone();
    no_m1["messages"][0]["content"] = json!([{"type": "text", "text": m0(&stale_body(&pair))}]);
    assert_eq!(
        export_capture(&capture(no_m1, vec![])),
        Err(CaptureError::NoM1 {
            task: pair.task.clone()
        })
    );
}

#[test]
fn a_stale_segment_in_m1_is_located_there_and_its_tier_reads_through_the_escape() {
    let pair = pair();
    // The stored p2 is what rendered; it carries an ampersand the renderer escapes.
    let p2 = format!(
        "Set the {} to {} & kept it.",
        pair.subject, pair.stale_value
    );
    let m1 = format!(
        "<session-history-since>\n<new-history_segments>\n## 1-2 · Setup\n{}\n</new-history_segments>\n</session-history-since>",
        p2.replace('&', "&amp;")
    );
    let served = request("<session-history></session-history>", &m1, "§3§ Set it.");
    let export = export_capture(&capture(served.clone(), vec![row(1, 2, "verbose", &p2)])).unwrap();
    let exported = &export.pairs[0];
    assert_eq!(
        (exported.history, exported.stale_block, exported.stale_tier),
        (
            HistoryAt {
                message: 0,
                part: 1
            },
            Block::M1,
            2
        )
    );
    let footer = part(&with_parts(&served, &[&exported.arms.footer]), 0, 1);
    assert!(footer.contains(&format!(
        "&amp; kept it.\n[corrections: {} = {} @3]\n</new-history_segments>",
        pair.key, pair.live_value
    )));
}

#[test]
fn a_stale_segment_without_the_value_is_unlocatable_even_if_a_later_segment_names_it() {
    let pair = pair();
    let body = format!(
        "## 1-2 · Setup\nConfigured the {}.\n\n## 3-4 · Later\nUpdated it from {} to {}.",
        pair.subject, pair.stale_value, pair.live_value
    );
    let served = request(&m0(&body), M1_PLACEHOLDER, "§5§ Noted.");
    let export = export_capture(&capture(served.clone(), vec![])).unwrap();
    assert!(export.pairs.is_empty());
    assert_eq!(export.unlocatable[&pair.task].delivery, StaleDelivery::Both);
    assert_eq!(export.unlocatable[&pair.task].request, served);
    assert_eq!(export.stale_delivered, 1);
    // The stale value served again elsewhere is flagged, not replaced.
    let body = format!(
        "{}\n\n## 3-4 · Later\nUpdated it from {} to {}.",
        stale_body(&pair),
        pair.stale_value,
        pair.live_value
    );
    let export = export_capture(&capture(
        request(&m0(&body), M1_PLACEHOLDER, "§5§ Noted."),
        vec![],
    ))
    .unwrap();
    assert!(export.pairs[0].stale_elsewhere);
    assert_eq!(export.pairs[0].restatement, Block::M0);
}

#[test]
fn an_export_refuses_a_foreign_schema_a_forged_world_and_a_missing_request() {
    let pair = pair();
    let served = request(&m0(&stale_body(&pair)), M1_PLACEHOLDER, "§3§ Set it.");
    let mut foreign = capture(served.clone(), vec![]);
    foreign.world.schema = "eval-stale-world/v0".to_string();
    assert_eq!(
        export_capture(&foreign),
        Err(CaptureError::Schema {
            found: "eval-stale-world/v0".to_string()
        })
    );
    let mut forged = capture(served.clone(), vec![]);
    forged.world.pairs[0].restating_turn = 0;
    assert_eq!(export_capture(&forged), Err(CaptureError::WorldMismatch));
    let mut missing = capture(served, vec![]);
    missing.requests.clear();
    assert_eq!(
        export_capture(&missing),
        Err(CaptureError::MissingRequest { task: pair.task })
    );
}

#[test]
fn a_merge_namespaces_every_session_and_refuses_a_mismatch_or_a_repeat() {
    let pair = pair();
    let served = request(&m0(&stale_body(&pair)), M1_PLACEHOLDER, "§3§ Set it.");
    let one = export_capture(&capture(served.clone(), vec![])).unwrap();
    let mut two = one.clone();
    two.root_seed = "7".to_string();
    let merged = merge_exports(vec![one.clone(), two.clone()]).unwrap();
    let tasks: Vec<&str> = merged.pairs.iter().map(|p| p.task.as_str()).collect();
    assert_eq!(tasks, ["world-1:stale-0", "world-2:stale-0"]);
    assert_eq!(
        (merged.stale_delivered, merged.root_seed.as_str()),
        (2, format!("{SEED},7").as_str())
    );
    let refuse = |exports: Vec<StaleExport>| merge_exports(exports).unwrap_err();
    assert!(matches!(
        refuse(vec![one.clone(), one.clone()]),
        CaptureError::Unmergeable { .. }
    ));
    let mut other = two;
    other.summarizer = "other".to_string();
    assert!(matches!(
        refuse(vec![one, other]),
        CaptureError::Unmergeable { .. }
    ));
    assert!(matches!(refuse(vec![]), CaptureError::Unmergeable { .. }));
}

#[test]
fn string_content_parts_are_read_and_replaced() {
    let pair = pair();
    let served = json!({"messages": [
        {"role": "user", "content": m0(&stale_body(&pair))},
        {"role": "user", "content": M1_PLACEHOLDER},
    ]});
    let export = export_capture(&capture(served.clone(), vec![])).unwrap();
    let exported = &export.pairs[0];
    assert_eq!(
        exported.history,
        HistoryAt {
            message: 0,
            part: 0
        }
    );
    let replaced = with_parts(&served, &[&exported.arms.omission_oracle]);
    assert!(!carries(
        replaced["messages"][0]["content"]
            .as_str()
            .unwrap()
            .split_once('\n')
            .unwrap()
            .1
            .split_once("## 1-2")
            .unwrap()
            .1
            .split_once('\n')
            .unwrap()
            .1,
        &pair.stale_value
    ));
}

#[test]
fn the_positive_control_writes_the_live_value_as_the_stale_one_wherever_it_is_served() {
    let pair = pair();
    let served = request(
        &m0(&stale_body(&pair)),
        &m1_with(&pair),
        &format!("§3§ Set the {} to {}.", pair.subject, pair.live_value),
    );
    let exported = export_capture(&capture(served.clone(), vec![]))
        .unwrap()
        .pairs
        .remove(0);
    let at: Vec<HistoryAt> = exported
        .arms
        .positive_control
        .iter()
        .map(|p| p.at)
        .collect();
    assert_eq!(
        at,
        [
            HistoryAt {
                message: 0,
                part: 1
            },
            HistoryAt {
                message: 1,
                part: 0
            }
        ],
        "the m1 delta and the raw tail carry the live value; m0 does not"
    );
    let parts: Vec<_> = exported.arms.positive_control.iter().collect();
    let control = with_parts(&served, &parts);
    let everything = serde_json::to_string(&control["messages"]).unwrap();
    assert!(!carries(&everything, &pair.live_value));
    assert!(carries(&part(&control, 0, 1), &pair.stale_value));
}

#[test]
fn a_merge_refuses_an_input_that_is_itself_a_merge_and_a_swapped_schema() {
    let pair = pair();
    let served = request(&m0(&stale_body(&pair)), M1_PLACEHOLDER, "§3§ Set it.");
    let one = export_capture(&capture(served.clone(), vec![])).unwrap();
    let mut two = one.clone();
    two.root_seed = "7".to_string();
    // One export is no merge: its output would carry prefixed ids under an
    // unjoined seed, and a later merge would prefix them again.
    assert!(matches!(
        merge_exports(vec![one.clone()]),
        Err(CaptureError::Unmergeable { .. })
    ));
    // Two inputs agreeing on a schema this build does not read are refused
    // as such, not merged under it.
    let mut foreign = one.clone();
    foreign.schema = "eval-stale-preference-export/v0".to_string();
    let mut foreign_two = two.clone();
    foreign_two.schema = foreign.schema.clone();
    assert_eq!(
        merge_exports(vec![foreign.clone(), foreign_two]).unwrap_err(),
        CaptureError::Schema {
            found: foreign.schema
        }
    );
    let merged = merge_exports(vec![one.clone(), two]).unwrap();
    assert!(matches!(
        merge_exports(vec![merged]),
        Err(CaptureError::Unmergeable { .. })
    ));
    let mut swapped = capture(served, vec![]);
    swapped.schema = STALE_WORLD_SCHEMA.to_string();
    assert!(matches!(
        export_capture(&swapped),
        Err(CaptureError::Schema { .. })
    ));
}

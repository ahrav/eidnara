//! The stale-preference harness contracts: the fact world, the request-level
//! arms over served m0 and m1 history, and the export of a harness capture.

use std::collections::{BTreeMap, BTreeSet};

use eval_core::{
    ACKNOWLEDGEMENT, CaptureError, FactWorld, HistoryAt, M1_PLACEHOLDER, MAX_SUBJECTS,
    PRECEDENCE_SENTENCE, STALE_CAPTURE_SCHEMA, STALE_EXPORT_SCHEMA, STALE_WORLD_SCHEMA,
    SegmentTiers, StaleCapture, StaleDelivery, carries, export_capture, fact_world, with_parts,
};
use serde_json::{Value, json};

const SEED: u64 = 0x5EED_B000_0000_0002;

#[test]
fn the_fact_world_restates_every_subject_once_with_a_value_of_its_own() {
    let world = fact_world(SEED, 24);
    assert_eq!(
        world,
        fact_world(SEED, 24),
        "a pure function of seed and size"
    );
    assert_ne!(world, fact_world(SEED + 1, 24));
    assert_eq!(world.schema, STALE_WORLD_SCHEMA);
    assert_eq!((world.turns.len(), world.pairs.len()), (48, 24));
    let mut values = BTreeSet::new();
    let mut keys = BTreeSet::new();
    for pair in &world.pairs {
        assert!(pair.stale_turn < pair.restating_turn, "{pair:?}");
        assert_eq!(
            world.turns[pair.stale_turn as usize].user,
            pair.stale_statement
        );
        assert_eq!(
            world.turns[pair.restating_turn as usize].user,
            pair.restatement
        );
        assert!(carries(&pair.stale_statement, &pair.stale_value));
        assert!(carries(&pair.restatement, &pair.live_value));
        assert!(!carries(&pair.restatement, &pair.stale_value));
        assert!(pair.question.contains(&pair.subject));
        assert!(
            !carries(&pair.question, &pair.stale_value)
                && !carries(&pair.question, &pair.live_value)
        );
        assert!(values.insert(pair.stale_value.clone()) && values.insert(pair.live_value.clone()));
        assert!(keys.insert(pair.key.clone()));
    }
    // Every value is stated once: acknowledgements carry none.
    for value in &values {
        let stated = world
            .turns
            .iter()
            .filter(|turn| carries(&turn.user, value))
            .count();
        assert_eq!(stated, 1, "{value}");
    }
    assert!(
        world
            .turns
            .iter()
            .all(|turn| turn.assistant == ACKNOWLEDGEMENT)
    );
    // Two statements to each restatement while subjects remain, as the
    // evaluator's correction regime; the rest close the session.
    let restatements: Vec<bool> = world
        .turns
        .iter()
        .map(|turn| turn.user.starts_with("Change of plan"))
        .collect();
    assert_eq!(&restatements[..6], [false, false, true, false, false, true]);
    assert_eq!(fact_world(SEED, MAX_SUBJECTS).pairs.len(), MAX_SUBJECTS);
}

#[test]
#[should_panic(expected = "at most")]
fn a_world_names_no_more_subjects_than_it_has() {
    fact_world(SEED, MAX_SUBJECTS + 1);
}

const M0: &str = "<session-history>\n## 1-5 · messages 1 to 5\nSet the billing service port to 34827.; Noted.; Set the search indexer port to 67339.\n\n## 6-10 · messages 6 to 10\nNoted.; Set the auth gateway port to 91549.\n</session-history>";
const M1: &str = "<session-history-since>\n<new-history_segments>\n## 11-15 · messages 11 to 15\nChange of plan: set the billing service port to 94225 instead.; Noted.\n</new-history_segments>\n</session-history-since>";

fn request(m0: &str, m1: &str) -> Value {
    json!({
        "system": "## Eidnara",
        "messages": [
            {"role": "user", "content": [{"type": "text", "text": m0}, {"type": "text", "text": m1}]},
            {"role": "assistant", "content": [{"type": "text", "text": "§16§ Noted."}]},
            {"role": "user", "content": [{"type": "text", "text": "§17§ What is the billing service port now? Reply with just the value."}]},
        ],
    })
}

fn segments() -> Vec<SegmentTiers> {
    let row = |start, end, p1: &str, p3: &str| SegmentTiers {
        start_message: start,
        end_message: end,
        p1: Some(p1.to_string()),
        p2: Some(p1.to_string()),
        p3: Some(p3.to_string()),
        p4: Some(String::new()),
    };
    vec![
        row(
            1,
            5,
            "Set the billing service port to 34827.; Noted.; Set the search indexer port to 67339.",
            "messages 1 to 5",
        ),
        row(
            6,
            10,
            "Noted.; Set the auth gateway port to 91549.",
            "messages 6 to 10",
        ),
        row(
            11,
            15,
            "Change of plan: set the billing service port to 94225 instead.; Noted.",
            "messages 11 to 15",
        ),
    ]
}

/// A world of one subject, stated on turn 0 and restated on turn 2 (message
/// 5), whose question turn served `request`.
fn capture(request: Value) -> StaleCapture {
    let world: FactWorld = serde_json::from_value(json!({
        "schema": STALE_WORLD_SCHEMA,
        "root_seed": "1",
        "turns": [],
        "pairs": [{
            "task": "stale-0", "subject": "billing service port", "key": "billing-service.port",
            "stale_value": "34827", "live_value": "94225",
            "stale_statement": "Set the billing service port to 34827.",
            "restatement": "Change of plan: set the billing service port to 94225 instead.",
            "stale_turn": 0, "restating_turn": 2,
            "question": "What is the billing service port now? Reply with just the value.",
        }],
    }))
    .unwrap();
    StaleCapture {
        schema: STALE_CAPTURE_SCHEMA.to_string(),
        harness: "opencode".to_string(),
        world,
        segments: segments(),
        requests: BTreeMap::from([("stale-0".to_string(), request)]),
    }
}

fn history_text(request: &Value) -> (String, String) {
    let part = |i: usize| {
        request["messages"][0]["content"][i]["text"]
            .as_str()
            .unwrap()
            .to_string()
    };
    (part(0), part(1))
}

#[test]
fn arms_change_the_served_history_where_d7_and_d8_put_their_bytes() {
    let served = request(M0, M1);
    let export = export_capture(&capture(served.clone())).unwrap();
    assert_eq!(export.schema, STALE_EXPORT_SCHEMA);
    assert_eq!(export.stale_delivered, 1);
    let pair = &export.pairs[0];
    assert_eq!(pair.request, served, "arm (a) is the request as served");
    assert_eq!(
        pair.history,
        HistoryAt {
            message: 0,
            part: 0
        }
    );
    assert_eq!((pair.restating_ordinal, pair.stale_tier), (5, 1));
    assert_eq!(pair.delivery, StaleDelivery::Both);
    let stale = "Set the billing service port to 34827.";
    let arm = |parts| history_text(&with_parts(&served, parts));
    // (b): the override sentence heads the m1 delta; m0 is untouched.
    let (m0, m1) = arm(&pair.arms.precedence_line);
    assert_eq!(m0, M0);
    assert_eq!(
        m1,
        M1.replacen(
            "<session-history-since>\n",
            &format!("<session-history-since>\n<memory-updates>\n{PRECEDENCE_SENTENCE}\n</memory-updates>\n"),
            1
        )
    );
    // (c): one footer line ends the stale segment's body, stale prose kept.
    let (m0, m1) = arm(&pair.arms.footer);
    assert_eq!(m1, M1);
    assert_eq!(
        m0,
        M0.replacen(
            "67339.\n\n## 6-10",
            "67339.\n[corrections: billing-service.port = 94225 @5]\n\n## 6-10",
            1
        )
    );
    // (d) and (e): at the stale statement's span only.
    let (m0, _) = arm(&pair.arms.anchored_replacement);
    assert_eq!(
        m0,
        M0.replacen(stale, "[corrected @5: billing-service.port = 94225]", 1)
    );
    let (m0, _) = arm(&pair.arms.omission_oracle);
    assert_eq!(m0, M0.replacen(stale, "", 1));
    assert!(!carries(&m0, "34827"));
}

#[test]
fn an_empty_m1_delta_gets_the_memory_updates_block_of_its_own() {
    let served = request(M0, M1_PLACEHOLDER);
    let export = export_capture(&capture(served.clone())).unwrap();
    let (_, m1) = history_text(&with_parts(&served, &export.pairs[0].arms.precedence_line));
    assert_eq!(
        m1,
        format!(
            "<session-history-since>\n<memory-updates>\n{PRECEDENCE_SENTENCE}\n</memory-updates>\n</session-history-since>"
        )
    );
}

#[test]
fn a_stale_statement_in_the_m1_delta_is_located_there_at_tier_one() {
    let m1 = "<session-history-since>\n<new-history_segments>\n## 1-5 · messages 1 to 5\nSet the billing service port to 34827.; Noted.; Set the search indexer port to 67339.\n</new-history_segments>\n</session-history-since>";
    let served = request("<session-history></session-history>", m1);
    let export = export_capture(&capture(served.clone())).unwrap();
    let pair = &export.pairs[0];
    assert_eq!(
        (pair.history, pair.stale_tier),
        (
            HistoryAt {
                message: 0,
                part: 1
            },
            1
        )
    );
    let (_, m1_footer) = history_text(&with_parts(&served, &pair.arms.footer));
    assert!(m1_footer.contains(
        "67339.\n[corrections: billing-service.port = 94225 @5]\n</new-history_segments>"
    ));
}

#[test]
fn a_stale_statement_outside_the_served_history_is_unlocatable_and_kept() {
    // A segment demoted to its title renders no prose; the raw tail still
    // carries the statement, and the delivery counts it.
    let m0 = "<session-history>\n## 1-5 · messages 1 to 5\nmessages 1 to 5\n</session-history>";
    let mut served = request(m0, M1_PLACEHOLDER);
    served["messages"][2]["content"][0]["text"] =
        json!("§3§ Set the billing service port to 34827.");
    let export = export_capture(&capture(served.clone())).unwrap();
    assert!(export.pairs.is_empty());
    assert_eq!(export.unlocatable["stale-0"].delivery, StaleDelivery::Stale);
    assert_eq!(export.unlocatable["stale-0"].request, served);
    assert_eq!(export.stale_delivered, 1);
}

#[test]
fn an_export_refuses_a_foreign_schema_and_a_missing_request() {
    let mut foreign = capture(request(M0, M1));
    foreign.schema = "eval-stale-capture/v0".to_string();
    assert!(matches!(
        export_capture(&foreign),
        Err(CaptureError::Schema { .. })
    ));
    let mut missing = capture(request(M0, M1));
    missing.requests.clear();
    assert_eq!(
        export_capture(&missing),
        Err(CaptureError::MissingRequest {
            task: "stale-0".to_string()
        })
    );
}

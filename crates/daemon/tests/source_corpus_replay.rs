//! The source-corpus replay oracle: every frozen corpus session and every synthetic session below
//! replays through the harness source adapters and the screened publisher against a real kernel,
//! and every unit the independent readers expect is admitted or lost under exactly one named
//! condition.
//!
//! A replay pass offers every unit not yet settled as one batch. The frozen sessions replay until a
//! pass defers nothing, at most [`SETTLING_PASSES`] passes; a synthetic case states its own pass
//! count.

#[allow(dead_code)]
#[path = "../src/source_corpus.rs"]
mod source_corpus;

#[allow(dead_code)]
#[path = "support/corpus_replay.rs"]
mod corpus_replay;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};

use daemon::harness_sources::{
    Harness, MAX_SOURCE_BATCH_UNITS, MAX_SOURCE_UNIT_BYTES, SessionIdentity,
};
use kernel::source_identity::OccurrenceClass;
use proptest::prelude::*;
use serde_json::{Value, json};

use corpus_replay::{
    PROJECT, ReplayRecord, ReplayStore, Settled, harness_adapter, opencode_records, pi_records,
    reconcile, replay,
};
use source_corpus::{LossCondition, NonUnitReason, corpus_dir};

/// Passes a frozen session replays at most; each corpus session settles in fewer.
const SETTLING_PASSES: usize = 8;

struct Shared {
    store: ReplayStore,
    _dir: tempfile::TempDir,
}

/// One kernel for the whole binary; each case publishes under its own session id.
static STORE: LazyLock<Arc<Shared>> = LazyLock::new(|| {
    let dir = tempfile::tempdir().unwrap();
    Arc::new(Shared {
        store: ReplayStore::open(dir.path()),
        _dir: dir,
    })
});

static SESSIONS: AtomicUsize = AtomicUsize::new(0);

fn fresh_session(prefix: &str) -> String {
    format!("{prefix}{:04}", SESSIONS.fetch_add(1, Ordering::Relaxed))
}

fn identity(harness: Harness, session_id: &str) -> SessionIdentity {
    SessionIdentity {
        project_id: PROJECT.to_owned(),
        harness,
        session_id: session_id.to_owned(),
    }
}

fn fixture(path: &std::path::Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// Every frozen session fixture under `sessions/`, by closure directory.
fn frozen_sessions() -> Vec<(String, Value)> {
    let mut sessions: Vec<(String, Value)> = std::fs::read_dir(corpus_dir().join("sessions"))
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let closure = path.file_name().unwrap().to_str().unwrap().to_owned();
            (closure, fixture(&path.join("mix.json")))
        })
        .collect();
    sessions.sort_by(|a, b| a.0.cmp(&b.0));
    sessions
}

fn assert_partitioned(records: &[ReplayRecord], outcome: &corpus_replay::Replay) {
    let result = reconcile(records, outcome);
    assert_eq!(result.unclassified, Vec::<String>::new());
    assert_eq!(result.phantom, Vec::<String>::new());
    assert_eq!(result.revision_mismatch, Vec::<String>::new());
    assert_eq!(result.duplicate, Vec::<String>::new());
    assert_eq!(result.admitted + result.lost(), result.expected);
}

#[test]
fn every_frozen_session_reconciles_with_no_unclassified_loss() {
    let sessions = frozen_sessions();
    assert_eq!(
        sessions
            .iter()
            .map(|(closure, _)| closure.as_str())
            .collect::<Vec<_>>(),
        ["opencode-1.18.22", "pi-0.80.2"]
    );
    for (closure, fixture) in &sessions {
        let (harness, (session, records)) = if closure.starts_with("opencode-") {
            (Harness::OpenCode, opencode_records(fixture))
        } else {
            (Harness::Pi, pi_records(fixture))
        };
        let outcome = replay(
            &STORE.store,
            &identity(harness, &session),
            &records,
            &harness_adapter,
            SETTLING_PASSES,
        );
        assert!(outcome.passes.len() < SETTLING_PASSES, "{closure} settles");
        assert_partitioned(&records, &outcome);
        let result = reconcile(&records, &outcome);
        assert_eq!(
            result.losses.get(&LossCondition::Deferred),
            None,
            "{closure}"
        );
        assert_eq!(
            result.losses.get(&LossCondition::SecretBearing),
            Some(&1),
            "{closure}"
        );
        let with_reason = |reason| {
            records
                .iter()
                .filter(|record| {
                    record
                        .expected
                        .non_units
                        .iter()
                        .any(|non_unit| non_unit.reason == reason)
                })
                .map(|record| record.expected.id.as_str())
                .collect::<Vec<_>>()
        };
        match harness {
            Harness::OpenCode => {
                assert_eq!(result.losses.get(&LossCondition::UnitTooLarge), None);
                assert!(!with_reason(NonUnitReason::NonTextPart).is_empty());
            }
            Harness::Pi => {
                assert_eq!(result.losses.get(&LossCondition::UnitTooLarge), Some(&1));
                // The image-bearing toolResult is a reader non-unit: its refusal loses no unit.
                let image = with_reason(NonUnitReason::NonTextItem);
                assert_eq!(image.len(), 1);
                assert!(outcome.refused_records.contains(image[0]));
            }
        }
    }
}

/// A part of a synthetic OpenCode message.
#[derive(Debug, Clone)]
enum Part {
    Text(usize),
    Tool(usize),
    /// A tool part still `pending`: the reader names it a non-unit and the adapter yields none.
    PendingTool,
    Image,
}

/// A frozen-format OpenCode session: one user message, then one completed assistant message
/// holding `parts` in order.
fn synthetic_opencode(session: &str, parts: &[Part]) -> Value {
    let order: Vec<usize> = (0..parts.len()).collect();
    synthetic_opencode_ordered(session, parts, &order)
}

/// As [`synthetic_opencode`], with part `index` created at `2_000 + order[index]` and the parts
/// stored in reverse: the stored order always differs from the creation order, and the id order
/// differs whenever `order` is not the identity.
fn synthetic_opencode_ordered(session: &str, parts: &[Part], order: &[usize]) -> Value {
    let part = |index: usize, data: Value| json!({ "id": format!("prt_{session}_{index:03}"), "time_created": 2_000 + order[index], "data": data });
    let mut assistant: Vec<Value> = parts
        .iter()
        .enumerate()
        .map(|(index, kind)| {
            part(
                index,
                match kind {
                    Part::Text(bytes) => json!({ "type": "text", "text": "t".repeat(*bytes) }),
                    Part::Tool(bytes) => json!({
                        "type": "tool", "tool": "bash", "callID": format!("call_{session}_{index}"),
                        "state": { "status": "completed", "input": {}, "output": "o".repeat(*bytes),
                                   "metadata": { "exit": 0 },
                                   "time": { "start": 2_000 + index, "end": 2_500 + index } },
                    }),
                    Part::PendingTool => json!({
                        "type": "tool", "tool": "bash", "callID": format!("call_{session}_{index}"),
                        "state": { "status": "pending", "input": {} },
                    }),
                    Part::Image => json!({ "type": "file", "mime": "image/png", "url": "data:," }),
                },
            )
        })
        .collect();
    assistant.reverse();
    json!({
        "session_id": session,
        "messages": [
            { "id": format!("msg_{session}_a"), "time_created": 1_000,
              "data": { "role": "user", "time": { "created": 1_000 } },
              "parts": [{ "id": format!("prt_{session}_user"), "time_created": 1_000,
                          "data": { "type": "text", "text": "start" } }] },
            { "id": format!("msg_{session}_b"), "time_created": 1_500,
              "data": { "role": "assistant", "time": { "created": 1_500, "completed": 3_000 } },
              "parts": assistant },
        ],
    })
}

fn ae1_parts() -> Vec<Part> {
    let mut parts: Vec<Part> = (0..11).map(|_| Part::Text(12)).collect();
    parts.extend((0..7).map(|_| Part::Tool(40)));
    parts.push(Part::Image);
    parts
}

#[test]
fn ae1_replays_nineteen_units_and_one_classified_skip() {
    let session = fresh_session("ae1");
    let (_, records) = opencode_records(&synthetic_opencode(&session, &ae1_parts()));
    let outcome = replay(
        &STORE.store,
        &identity(Harness::OpenCode, &session),
        &records,
        &harness_adapter,
        SETTLING_PASSES,
    );
    let result = reconcile(&records, &outcome);
    assert_eq!((result.expected, result.admitted), (19, 19));
    assert_partitioned(&records, &outcome);
    let skips: Vec<(NonUnitReason, &str)> = records
        .iter()
        .flat_map(|record| &record.expected.non_units)
        .map(|non_unit| (non_unit.reason, non_unit.kind.as_str()))
        .collect();
    assert_eq!(skips, [(NonUnitReason::NonTextPart, "file")]);
}

#[test]
fn a_mutated_adapter_fails_the_replay_naming_the_part() {
    let session = fresh_session("drop");
    let (_, records) = opencode_records(&synthetic_opencode(&session, &ae1_parts()));
    let drop_last_text = |identity: &SessionIdentity, native: &Value| {
        let mut units = harness_adapter(identity, native)?;
        if let Some(last) = units
            .iter()
            .rposition(|unit| unit.class == OccurrenceClass::Messages)
        {
            units.remove(last);
        }
        Ok(units)
    };
    let outcome = replay(
        &STORE.store,
        &identity(Harness::OpenCode, &session),
        &records,
        &drop_last_text,
        SETTLING_PASSES,
    );
    let result = reconcile(&records, &outcome);
    assert_eq!(result.unclassified.len(), 2, "{:?}", result.unclassified);
    assert!(
        result
            .unclassified
            .iter()
            .any(|key| key.contains(&format!("msg_{session}_b")) && key.contains("part 10")),
        "{:?}",
        result.unclassified
    );

    let session = fresh_session("swap");
    let (_, records) = opencode_records(&synthetic_opencode(&session, &ae1_parts()));
    let created_for_completed = |identity: &SessionIdentity, native: &Value| {
        let mut native = native.clone();
        if let Some(time) = native.pointer_mut("/info/time")
            && let Some(created) = time.get("created").cloned()
        {
            time["completed"] = created;
        }
        harness_adapter(identity, &native)
    };
    let outcome = replay(
        &STORE.store,
        &identity(Harness::OpenCode, &session),
        &records,
        &created_for_completed,
        SETTLING_PASSES,
    );
    let result = reconcile(&records, &outcome);
    assert_eq!(
        result.revision_mismatch.len(),
        11,
        "{:?}",
        result.revision_mismatch
    );
    assert!(result.revision_mismatch[0].contains("part 0"));
}

#[test]
fn an_oversized_unit_and_a_33_unit_batch_reconcile_to_named_losses() {
    let session = fresh_session("cap");
    let parts = [
        Part::Text(12),
        Part::Tool(MAX_SOURCE_UNIT_BYTES + 1),
        Part::Text(12),
    ];
    let (_, records) = opencode_records(&synthetic_opencode(&session, &parts));
    let outcome = replay(
        &STORE.store,
        &identity(Harness::OpenCode, &session),
        &records,
        &harness_adapter,
        SETTLING_PASSES,
    );
    assert_partitioned(&records, &outcome);
    let result = reconcile(&records, &outcome);
    assert_eq!(result.losses.get(&LossCondition::UnitTooLarge), Some(&1));
    assert_eq!(MAX_SOURCE_UNIT_BYTES + 1, 65_537);

    let session = fresh_session("batch");
    let parts: Vec<Part> = (0..MAX_SOURCE_BATCH_UNITS).map(|_| Part::Text(8)).collect();
    let (_, records) = opencode_records(&synthetic_opencode(&session, &parts));
    let outcome = replay(
        &STORE.store,
        &identity(Harness::OpenCode, &session),
        &records,
        &harness_adapter,
        1,
    );
    assert_partitioned(&records, &outcome);
    let result = reconcile(&records, &outcome);
    assert_eq!(result.expected, 33);
    assert_eq!(result.admitted, 32);
    assert_eq!(result.losses.get(&LossCondition::Deferred), Some(&1));
}

#[test]
fn a_pending_tool_part_is_a_non_unit_and_loses_no_unit() {
    let session = fresh_session("pending");
    let parts = [Part::Text(12), Part::PendingTool, Part::Tool(40)];
    let (_, records) = opencode_records(&synthetic_opencode(&session, &parts));
    let outcome = replay(
        &STORE.store,
        &identity(Harness::OpenCode, &session),
        &records,
        &harness_adapter,
        SETTLING_PASSES,
    );
    assert_partitioned(&records, &outcome);
    let result = reconcile(&records, &outcome);
    // The user text, the assistant text, and the settled tool part; the pending part is none.
    assert_eq!((result.expected, result.admitted), (3, 3));
    let skips: Vec<NonUnitReason> = records
        .iter()
        .flat_map(|record| &record.expected.non_units)
        .map(|non_unit| non_unit.reason)
        .collect();
    assert_eq!(skips, [NonUnitReason::UnsettledTool]);
}

fn part_strategy() -> impl Strategy<Value = Part> {
    let size = prop_oneof![
        4 => 1usize..64,
        1 => (MAX_SOURCE_UNIT_BYTES - 2)..(MAX_SOURCE_UNIT_BYTES + 3),
    ];
    prop_oneof![
        5 => size.clone().prop_map(Part::Text),
        3 => size.prop_map(Part::Tool),
        1 => Just(Part::PendingTool),
        1 => Just(Part::Image),
    ]
}

/// Runs one property case: `parts` created in `order`, replayed for `passes` passes.
fn identity_case(parts: &[Part], order: &[usize], passes: usize) -> Result<(), TestCaseError> {
    let session = fresh_session("prop");
    let (_, records) = opencode_records(&synthetic_opencode_ordered(&session, parts, order));
    let outcome = replay(
        &STORE.store,
        &identity(Harness::OpenCode, &session),
        &records,
        &harness_adapter,
        passes,
    );
    let result = reconcile(&records, &outcome);
    prop_assert_eq!(&result.unclassified, &Vec::<String>::new());
    prop_assert_eq!(&result.phantom, &Vec::<String>::new());
    prop_assert_eq!(&result.revision_mismatch, &Vec::<String>::new());
    prop_assert_eq!(&result.duplicate, &Vec::<String>::new());
    prop_assert_eq!(result.admitted + result.lost(), result.expected);
    let admitted = outcome
        .units
        .iter()
        .filter(|unit| matches!(unit.settled, Settled::Admitted { .. }))
        .count();
    prop_assert_eq!(admitted, result.admitted);
    // Size decides `unit_too_large` exactly: every unit past the cap, and no other.
    for unit in &outcome.units {
        let too_large = unit.settled == Settled::Lost(LossCondition::UnitTooLarge);
        prop_assert_eq!(
            too_large,
            unit.bytes > MAX_SOURCE_UNIT_BYTES,
            "{}",
            unit.key
        );
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]
    #[test]
    fn admitted_plus_named_loss_equals_expected_for_any_order_split_and_size(
        (parts, order) in proptest::collection::vec(part_strategy(), 1..40).prop_flat_map(|parts| {
            let order = Just((0..parts.len()).collect::<Vec<usize>>()).prop_shuffle();
            (Just(parts), order)
        }),
        passes in 1usize..4,
    ) {
        identity_case(&parts, &order, passes)?;
    }
}

/// The case the property once shrank to, kept as a named example.
#[test]
fn a_single_text_part_replays_in_one_pass() {
    identity_case(&[Part::Text(1)], &[0], 1).unwrap();
    let session = fresh_session("single");
    let (_, records) = opencode_records(&synthetic_opencode(&session, &[Part::Text(1)]));
    let outcome = replay(
        &STORE.store,
        &identity(Harness::OpenCode, &session),
        &records,
        &harness_adapter,
        1,
    );
    assert_eq!(outcome.passes.len(), 1);
    assert_eq!(reconcile(&records, &outcome).admitted, 2);
}

#[test]
fn a_seeded_dropped_unit_fails_the_identity_naming_the_unit() {
    let session = fresh_session("seeded");
    let parts = [Part::Text(5), Part::Tool(9), Part::Text(7)];
    let (_, records) = opencode_records(&synthetic_opencode(&session, &parts));
    let drop_tool = |identity: &SessionIdentity, native: &Value| {
        let units = harness_adapter(identity, native)?;
        Ok(units
            .into_iter()
            .filter(|unit| unit.class != OccurrenceClass::RawToolSpans)
            .collect())
    };
    let outcome = replay(
        &STORE.store,
        &identity(Harness::OpenCode, &session),
        &records,
        &drop_tool,
        SETTLING_PASSES,
    );
    let result = reconcile(&records, &outcome);
    assert_ne!(result.admitted + result.lost(), result.expected);
    assert_eq!(
        result.unclassified,
        [format!(
            "tool:call_{session}_1:0 (record msg_{session}_b, part 1)"
        )]
    );
}

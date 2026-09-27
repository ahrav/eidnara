//! Stale preference: the coexistence restatement the generator emits, the
//! stale-preference task role and its negative control, the delivery count,
//! the knowledge-update grader, and exact McNemar.

mod support;

use std::collections::{BTreeMap, BTreeSet};

use context_core::canonical_json::protocol_digest;
use eval_core::{
    Destination, EventId, EventLog, GENERATOR_VERSION, Grade, MAX_VALID_TIME_MS, McNemarError,
    Mode, PairError, PairSet, PairSetInput, Payload, Query, RANDOM_SCHEMA_VERSION, Ratio,
    RenderConfig, ReplayRefusal, Sensitivity, ServedClass, ServedSpan, SessionSpec, StaleDelivery,
    Task, TaskRole, Visibility, WorldConfig, WorldError, carries, compile_pair_set, generate_all,
    grade, locate, mcnemar, render, serialize_spec, text_decision,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use support::{WORLD_EPOCH_MS as EPOCH_MS, WORLD_SEED as SEED, world_config};

fn one_session(messages: u32, correction_every: u32, restatement_every: u32) -> WorldConfig {
    WorldConfig {
        sessions: vec![SessionSpec {
            messages,
            tool_span_every: 0,
            correction_every,
            invalidation_every: 0,
            restatement_every,
        }],
        repositories: vec![],
        epoch_ms: EPOCH_MS,
        tick_ms: 1_000,
        max_events_per_log: 64,
        planted: Vec::new(),
    }
}

fn coexistence() -> EventLog {
    generate_all(SEED, &one_session(12, 0, 3), Mode::Generate)
        .unwrap()
        .log
}

#[test]
fn a_restatement_is_a_new_message_at_a_later_time_naming_its_target_slot() {
    let config = one_session(12, 0, 3);
    assert_eq!(config.declared_events(), 12 + 4);
    let log = coexistence();
    assert_eq!(log.events.len(), 16);
    let restatements: Vec<_> = log
        .events
        .iter()
        .filter_map(|e| match &e.payload {
            Payload::Restatement {
                target,
                message_id,
                text,
            } => Some((e, target, message_id, text)),
            _ => None,
        })
        .collect();
    assert_eq!(restatements.len(), 4, "slots 2, 5, 8, and 11");
    let mut targets = BTreeSet::new();
    for (event, target, message_id, text) in restatements {
        let original = log.events.iter().find(|e| e.id == *target).unwrap();
        let Payload::Message {
            message_id: restated_id,
            text: stale,
            ..
        } = &original.payload
        else {
            panic!("a restatement targets a message")
        };
        assert_ne!(message_id, restated_id, "a lineage of its own");
        assert!(event.valid_time_ms > original.valid_time_ms);
        // The same slot word, another drawn word: the two decisions differ in
        // a whole word and neither carries the other.
        let (stale, live) = (text_decision(stale), text_decision(text));
        assert_eq!(
            stale.split(' ').skip(1).collect::<Vec<_>>(),
            live.split(' ').skip(1).collect::<Vec<_>>()
        );
        assert!(
            !carries(live, stale) && !carries(stale, live),
            "{stale} / {live}"
        );
        assert!(targets.insert(target.clone()), "a message is restated once");
    }
    // Both texts are served: the restatement renders as its own message.
    let rendering = render(
        &log,
        &RenderConfig {
            project_id: "p".to_string(),
            repository_id: "r".to_string(),
            object_format: "sha1".to_string(),
        },
    )
    .unwrap();
    assert_eq!(rendering.messages.len(), 16);
}

#[test]
fn a_tape_recorded_under_the_previous_generator_refuses() {
    assert_eq!(GENERATOR_VERSION, "eval-generator/v4");
    // A config version 3 could have written: no restatements, and the field
    // is not serialized.
    let config = world_config();
    assert!(
        !serde_json::to_string(&config)
            .unwrap()
            .contains("restatement_every"),
        "a config without restatements keeps its bytes"
    );
    let identity = |version: &str| {
        protocol_digest(
            "eval-tape/v1",
            &json!({
                "root_seed": SEED.to_string(),
                "config": config,
                "generator_version": version,
                "random_schema_version": RANDOM_SCHEMA_VERSION,
            }),
        )
        .unwrap()
    };
    let mut tape = generate_all(SEED, &config, Mode::Generate).unwrap().tape;
    assert_eq!(tape.identity, identity(GENERATOR_VERSION), "the recipe");
    tape.identity = identity("eval-generator/v3");
    assert_eq!(
        generate_all(SEED, &config, Mode::ReplayTape(tape.clone())).unwrap_err(),
        WorldError::Replay(ReplayRefusal::TapeMismatch {
            expected: identity(GENERATOR_VERSION),
            found: tape.identity,
        })
    );
}

/// The rendering of a world with corrections and no restatement, pinned at
/// the bytes `eval-generator/v3` and its renderer produced: the digest is
/// this test's computation run in a worktree at base `be542f0c`.
#[test]
fn a_correction_renders_the_bytes_it_rendered_before_restatements() {
    let world = generate_all(SEED, &world_config(), Mode::Generate).unwrap();
    let rendering = render(
        &world.log,
        &RenderConfig {
            project_id: "proj-eval".to_string(),
            repository_id: "repo-0".to_string(),
            object_format: "sha1".to_string(),
        },
    )
    .unwrap();
    let messages: Vec<_> = rendering
        .messages
        .iter()
        .map(|m| json!({"event": m.event_id, "message": m.message}))
        .collect();
    let digest = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&messages).unwrap())
    );
    assert_eq!(
        digest,
        "29f1ebe42a0cc82fdaf0fe7e12279b7cf4245913532765bc807b875066eb0f24"
    );
}

fn query() -> Query {
    Query {
        valid_time_ms: MAX_VALID_TIME_MS,
        observation_time_ms: MAX_VALID_TIME_MS,
        scope: BTreeSet::from(["session-0".to_string()]),
        destination: Destination::Local,
        served: Some(ServedClass {
            sensitivity: Sensitivity::Normal,
            visibility: Visibility::Labeled,
            auto_inject: Visibility::Hidden,
            auto_search: Visibility::Hidden,
        }),
        registry_sensitivity: Sensitivity::Normal,
        max_events_per_log: 64,
    }
}

fn task(name: &str, role: TaskRole, id: &EventId) -> Task {
    Task {
        id: name.to_string(),
        role,
        query: query(),
        evidence: BTreeSet::from([id.clone()]),
    }
}

/// A falsifier (the first message nothing supersedes), a positive control
/// (the last message), and one stale-preference task per `evidence` id.
fn compile(aged: &EventLog, evidence: &[EventId]) -> Result<PairSet, PairError> {
    let superseded: BTreeSet<&EventId> = aged
        .events
        .iter()
        .filter_map(|e| e.payload.supersedes().map(|(_, target)| target))
        .collect();
    let messages: Vec<&EventId> = aged
        .events
        .iter()
        .filter(|e| matches!(e.payload, Payload::Message { .. }))
        .map(|e| &e.id)
        .collect();
    let early = messages
        .iter()
        .find(|id| !superseded.contains(*id))
        .unwrap();
    let mut tasks = vec![
        task("early", TaskRole::Falsification, early),
        task(
            "last",
            TaskRole::PositiveControl,
            messages[messages.len() - 1],
        ),
    ];
    for (index, id) in evidence.iter().enumerate() {
        tasks.push(task(
            &format!("stale-{index}"),
            TaskRole::StalePreference,
            id,
        ));
    }
    let natural_fresh = generate_all(SEED ^ 0x77, &one_session(3, 0, 0), Mode::Generate)
        .unwrap()
        .log;
    compile_pair_set(PairSetInput {
        surface: eval_core::EvaluatedSurface::Surface1,
        declared_bound: None,
        aged,
        natural_fresh: &natural_fresh,
        fixture: &serialize_spec(),
        tasks: &tasks,
    })
}

fn ids_where(log: &EventLog, pick: impl Fn(&Payload) -> bool) -> Vec<EventId> {
    log.events
        .iter()
        .filter(|e| pick(&e.payload))
        .map(|e| e.id.clone())
        .collect()
}

#[test]
fn stale_preference_compiles_over_coexisting_restatements() {
    let aged = coexistence();
    let restatements = ids_where(&aged, |p| matches!(p, Payload::Restatement { .. }));
    let set = compile(&aged, &restatements).unwrap();
    set.validate(&serialize_spec()).unwrap();
    let truth = eval_core::reduce(&aged, &serialize_spec(), &query()).unwrap();
    for (pair, restatement) in set.pairs[2..].iter().zip(&restatements) {
        assert_eq!(pair.task.role, TaskRole::StalePreference);
        let Payload::Restatement { target, .. } = &aged
            .events
            .iter()
            .find(|e| e.id == *restatement)
            .unwrap()
            .payload
        else {
            unreachable!()
        };
        assert!(
            truth.required.contains(restatement),
            "the restatement is required"
        );
        assert!(
            !truth.required.contains(target),
            "its predecessor is superseded"
        );
        assert!(truth.units[target].facts.state.unwrap().superseded);
    }
}

#[test]
fn stale_preference_is_refused_over_a_same_message_id_correction() {
    // Today's correction replaces its target inside one lineage, so nothing
    // stale coexists with it: the inverted falsifier check fails.
    let aged = generate_all(SEED, &one_session(12, 3, 0), Mode::Generate)
        .unwrap()
        .log;
    let corrections = ids_where(&aged, |p| matches!(p, Payload::Correction { .. }));
    assert!(!corrections.is_empty());
    assert_eq!(
        compile(&aged, &corrections[..1]),
        Err(PairError::NoCoexistingRestatement {
            task: "stale-0".to_string(),
            id: corrections[0].clone(),
        })
    );
    // A plain message restates nothing either.
    let coexisting = coexistence();
    let plain = ids_where(&coexisting, |p| matches!(p, Payload::Message { .. }));
    let last = plain.last().unwrap().clone();
    assert_eq!(
        compile(&coexisting, std::slice::from_ref(&last))
            .unwrap_err()
            .kind(),
        "NoCoexistingRestatement"
    );
    // A restatement whose predecessor the history no longer holds supersedes
    // nothing.
    let restatement =
        ids_where(&coexisting, |p| matches!(p, Payload::Restatement { .. }))[0].clone();
    let Payload::Restatement { target, .. } = &coexisting
        .events
        .iter()
        .find(|e| e.id == restatement)
        .unwrap()
        .payload
    else {
        unreachable!()
    };
    let orphaned = coexisting.without(target);
    assert_eq!(
        compile(&orphaned, std::slice::from_ref(&restatement)),
        Err(PairError::NoCoexistingRestatement {
            task: "stale-0".to_string(),
            id: restatement,
        })
    );
}

#[test]
fn stale_preference_is_refused_when_the_predecessor_is_also_corrected_in_place() {
    // Corrections on every fifth slot and restatements on every second: some
    // message is both corrected in place and restated, so its served text is
    // no longer its own and the pair would grade against the wrong stale
    // value.
    let aged = generate_all(SEED, &one_session(24, 5, 2), Mode::Generate)
        .unwrap()
        .log;
    let doubled = aged
        .events
        .iter()
        .find(|e| match &e.payload {
            Payload::Restatement { target, .. } => aged.events.iter().any(
                |c| matches!(&c.payload, Payload::Correction { target: t, .. } if t == target),
            ),
            _ => false,
        })
        .expect("a restated message that is also corrected");
    assert_eq!(
        compile(&aged, std::slice::from_ref(&doubled.id)),
        Err(PairError::NoCoexistingRestatement {
            task: "stale-0".to_string(),
            id: doubled.id.clone(),
        })
    );
}

#[test]
fn delivery_records_which_value_the_served_text_carries() {
    let (stale, live) = ("cursor for slot3", "digest for slot3");
    let table = [
        ("- cursor for slot3 in w", StaleDelivery::Stale),
        ("- digest for slot3 in w", StaleDelivery::Live),
        (
            "- cursor for slot3 in w\n- digest for slot3 in w",
            StaleDelivery::Both,
        ),
        ("- cursor for slot37 in w", StaleDelivery::Neither),
    ];
    for (served, expected) in table {
        assert_eq!(StaleDelivery::of(served, stale, live), expected, "{served}");
    }
    assert!(StaleDelivery::Both.stale_delivered() && StaleDelivery::Stale.stale_delivered());
    assert!(!StaleDelivery::Live.stale_delivered() && !StaleDelivery::Neither.stale_delivered());
}

#[test]
fn the_grader_applies_the_knowledge_update_rule_without_a_model() {
    let (stale, live) = ("cursor for slot3", "digest for slot3");
    let table = [
        ("The current decision is digest for slot3.", Grade::Current),
        ("It was cursor for slot3.", Grade::Stale),
        (
            "Earlier it was cursor for slot3; now it is digest for slot3.",
            Grade::Current,
        ),
        ("I cannot tell.", Grade::Miss),
        ("cursor for slot30 and digest for slot31", Grade::Miss),
        ("xcursor for slot3", Grade::Miss),
    ];
    for (answer, expected) in table {
        assert_eq!(grade(answer, stale, live), expected, "{answer}");
    }
    assert_eq!(
        locate("a slot47 slot4.", "slot4"),
        Some(ServedSpan { start: 9, end: 14 })
    );
    assert!(!carries("anything", ""));
}

type Outcomes = BTreeMap<String, Option<bool>>;

fn table(first_only: u32, second_only: u32, both: u32, neither: u32) -> [Outcomes; 2] {
    let mut first = BTreeMap::new();
    let mut second = BTreeMap::new();
    let cells = [
        (first_only, true, false),
        (second_only, false, true),
        (both, true, true),
        (neither, false, false),
    ];
    for (count, a, b) in cells {
        for _ in 0..count {
            let id = format!("pair-{:04}", first.len());
            first.insert(id.clone(), Some(a));
            second.insert(id, Some(b));
        }
    }
    [first, second]
}

fn alpha() -> Ratio {
    Ratio::from_decimal("0.05").unwrap()
}

fn decide(b: u32, c: u32, alpha: Ratio) -> Result<bool, McNemarError> {
    let [first, second] = table(b, c, 0, 0);
    mcnemar(&first, &second, alpha).map(|result| result.reject)
}

/// Two-sided exact p-values by hand: `2 * sum_{i <= min} C(n, i) / 2^n`.
#[test]
fn exact_mcnemar_matches_hand_computed_tables() {
    // (b, c, concordant, reject): 0/0 has p = 1; 6/0 has p = 2/64 = 0.03125;
    // 5/0 has p = 2/32 = 0.0625; 15/5 in 80 pairs has p = 43400/1048576 =
    // 0.0414; 14/5 in 80 pairs has p = 33328/524288 = 0.0636; 40/40 is p = 1.
    let cases = [
        (0, 0, 80, false),
        (6, 0, 72, true),
        (5, 0, 73, false),
        (15, 5, 60, true),
        (14, 5, 61, false),
        (5, 15, 60, true),
        (40, 40, 0, false),
    ];
    for (b, c, concordant, reject) in cases {
        let [first, second] = table(b, c, concordant / 2, concordant - concordant / 2);
        let result = mcnemar(&first, &second, alpha()).unwrap();
        assert_eq!(
            (
                result.pairs,
                result.indeterminate,
                result.first_only,
                result.second_only,
                result.reject
            ),
            (b + c + concordant, 0, b, c, reject),
            "{b}/{c}"
        );
    }
    // p equal to alpha rejects: 6/0 has p = 1/32 exactly.
    assert!(decide(6, 0, Ratio::from_decimal("0.03125").unwrap()).unwrap());
    assert!(!decide(6, 0, Ratio::from_decimal("0.03124").unwrap()).unwrap());
    // Either side of alpha at n = 100 and n = 120: 61/39 has p ~ 0.0352 and
    // 60/40 p ~ 0.0569; 72/48 has p ~ 0.0353 and 71/49 p ~ 0.0548.
    assert!(decide(61, 39, alpha()).unwrap());
    assert!(!decide(60, 40, alpha()).unwrap());
    assert!(decide(72, 48, alpha()).unwrap());
    assert!(!decide(71, 49, alpha()).unwrap());
    // Past about 120 discordant pairs the exact tail leaves 128-bit range:
    // refused, typed.
    assert_eq!(decide(100, 28, alpha()), Err(McNemarError::Overflow));
    assert_eq!(decide(62, 62, alpha()), Err(McNemarError::Overflow));
}

#[test]
fn a_failed_call_is_counted_indeterminate_and_never_scored() {
    let [mut first, mut second] = table(6, 0, 0, 0);
    first.insert("failed-on-first".to_string(), None);
    second.insert("failed-on-first".to_string(), Some(false));
    first.insert("failed-on-second".to_string(), Some(true));
    second.insert("failed-on-second".to_string(), None);
    let result = mcnemar(&first, &second, alpha()).unwrap();
    assert_eq!(
        (
            result.pairs,
            result.indeterminate,
            result.first_only,
            result.reject
        ),
        (6, 2, 6, true)
    );
}

#[test]
fn mcnemar_refuses_unpaired_arms_and_an_alpha_outside_the_open_unit_interval() {
    let [first, mut second] = table(1, 1, 0, 0);
    for bad in [Ratio::ZERO, Ratio::ONE] {
        assert_eq!(
            mcnemar(&first, &second, bad),
            Err(McNemarError::AlphaOutOfRange)
        );
    }
    second.insert("extra".to_string(), Some(true));
    assert_eq!(
        mcnemar(&first, &second, alpha()),
        Err(McNemarError::UnpairedArms)
    );
}

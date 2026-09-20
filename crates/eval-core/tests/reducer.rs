//! The eligibility spec pin, spec drift refusals, and bitemporal reduction of
//! generated worlds.

mod support;

use std::collections::{BTreeMap, BTreeSet};

use context_core::canonical_json::protocol_digest;
use eval_core::{
    ArtifactEligibility, Destination, ELIGIBILITY_SPEC_DIGEST, ELIGIBILITY_SPEC_PROTOCOL,
    EligibilitySpec, Event, EventId, FactTuple, LogError, MAX_VALID_TIME_MS, Mode, PREDICATES,
    Payload, Predicate, PredicateKind, Query, ReduceError, Sensitivity, ServedClass, SpecError,
    StateFacts, Surface, Truth, Verdict, Visibility, World, check_spec, generate_all, judge,
    judge_surface, judge_with, reduce, serialize_spec, spec,
};
use serde_json::{Value, json};
use support::{WORLD_EPOCH_MS as EPOCH_MS, WORLD_SEED as SEED, world_config as config};

/// The kernel's short-circuit order, written down here rather than derived
/// from any enum or table in the crate.
const JUDGE_ORDER: [(&str, Verdict); 8] = [
    ("state_absent", Verdict::Retracted),
    ("superseded", Verdict::Superseded),
    ("invalidated", Verdict::Retracted),
    ("revision_differs", Verdict::Stale),
    ("out_of_scope", Verdict::WrongScope),
    ("sensitivity_denies_destination", Verdict::ProviderSensitive),
    ("unserved_or_hidden", Verdict::Hidden),
    ("artifact_denied", Verdict::ProviderSensitive),
];

const END: i64 = MAX_VALID_TIME_MS;

fn labeled() -> ServedClass {
    ServedClass {
        sensitivity: Sensitivity::Normal,
        visibility: Visibility::Labeled,
        auto_inject: Visibility::Hidden,
        auto_search: Visibility::Hidden,
    }
}

const OK_STATE: StateFacts = StateFacts {
    superseded: false,
    invalidated: false,
    revision_matches: true,
    in_scope: true,
    registry_sensitivity: Sensitivity::Normal,
};

fn live(served: Option<ServedClass>) -> FactTuple {
    FactTuple {
        state: Some(OK_STATE),
        served,
        artifact: None,
        destination: Destination::Local,
    }
}

fn state(facts: &mut FactTuple) -> &mut StateFacts {
    facts.state.as_mut().unwrap()
}

fn world() -> World {
    generate_all(SEED, &config(), Mode::Generate).unwrap()
}

fn query(valid_time_ms: i64, observation_time_ms: i64) -> Query {
    Query {
        valid_time_ms,
        observation_time_ms,
        scope: BTreeSet::from(["session-0".to_string(), "repository-0".to_string()]),
        destination: Destination::Local,
        served: Some(labeled()),
        registry_sensitivity: Sensitivity::Normal,
        max_events_per_log: config().max_events_per_log,
    }
}

fn fixture() -> Value {
    serialize_spec()
}

#[test]
fn the_in_code_table_digests_to_the_pinned_constant_in_judge_order() {
    let value = serialize_spec();
    assert_eq!(
        protocol_digest(ELIGIBILITY_SPEC_PROTOCOL, &value).unwrap(),
        ELIGIBILITY_SPEC_DIGEST
    );
    let checked = check_spec(&value).unwrap();
    let spec = spec();
    assert_eq!(checked, spec);
    assert_eq!(spec.version, ELIGIBILITY_SPEC_PROTOCOL);
    let order: Vec<(String, Verdict)> = spec
        .predicates
        .iter()
        .map(|p| {
            let name = serde_json::to_value(p.name).unwrap();
            (name.as_str().unwrap().to_string(), p.verdict)
        })
        .collect();
    let wanted: Vec<(String, Verdict)> = JUDGE_ORDER
        .iter()
        .map(|(n, v)| (n.to_string(), *v))
        .collect();
    assert_eq!(
        order, wanted,
        "fixture order is judge order, not enum order"
    );
    assert_eq!(spec.predicates, PREDICATES);
    assert_eq!(spec.verdicts, Verdict::ALL);
    assert_eq!(spec.vectors.len(), 22);
    let verdicts: BTreeSet<Verdict> = spec.vectors.iter().map(|v| v.verdict).collect();
    assert_eq!(verdicts.len(), 7, "every verdict has a vector");
    assert!(
        spec.vectors
            .iter()
            .any(|v| v.facts.state.is_some_and(|s| s.superseded && s.invalidated))
    );
    assert!(
        spec.vectors
            .iter()
            .any(|v| v.facts.destination == Destination::Remote)
    );
    for vector in &spec.vectors {
        assert_eq!(judge(&vector.facts), vector.verdict, "{vector:?}");
    }
    let round_trip: EligibilitySpec = serde_json::from_value(value).unwrap();
    assert_eq!(round_trip, spec);
}

#[test]
fn every_predicate_and_the_surface_fold_are_reachable_from_facts() {
    let base = live(Some(labeled()));
    assert_eq!(judge(&base), Verdict::Ok);
    type Mutation = fn(&mut FactTuple);
    let cases: [(Mutation, Verdict); 8] = [
        (|f| f.state = None, Verdict::Retracted),
        (
            |f| {
                state(f).superseded = true;
                state(f).invalidated = true;
            },
            Verdict::Superseded,
        ),
        (|f| state(f).invalidated = true, Verdict::Retracted),
        (|f| state(f).revision_matches = false, Verdict::Stale),
        (|f| state(f).in_scope = false, Verdict::WrongScope),
        (
            |f| f.served.as_mut().unwrap().sensitivity = Sensitivity::Secret,
            Verdict::ProviderSensitive,
        ),
        (|f| f.served = None, Verdict::Hidden),
        (
            |f| f.artifact = Some(ArtifactEligibility::Denied),
            Verdict::ProviderSensitive,
        ),
    ];
    for (mutate, verdict) in cases {
        let mut facts = base;
        mutate(&mut facts);
        assert_eq!(judge(&facts), verdict);
    }

    // Sensitivity is the served class when served, else the registry class.
    let mut sensitive = base;
    sensitive.served.as_mut().unwrap().sensitivity = Sensitivity::Sensitive;
    assert_eq!(judge(&sensitive), Verdict::Ok);
    sensitive.destination = Destination::Remote;
    assert_eq!(judge(&sensitive), Verdict::ProviderSensitive);
    let mut unserved_secret = live(None);
    state(&mut unserved_secret).registry_sensitivity = Sensitivity::Secret;
    assert_eq!(
        judge(&unserved_secret),
        Verdict::ProviderSensitive,
        "before Hidden"
    );

    // The fold: explicit search sees the label, the automatic surfaces do not.
    assert!(judge_surface(&base, Surface::ExplicitSearch).permits());
    assert!(!judge_surface(&base, Surface::AutoInject).permits());
    assert!(!judge_surface(&base, Surface::AutoSearch).permits());
    assert_eq!(
        judge_surface(&live(None), Surface::ExplicitSearch).visibility,
        Visibility::Hidden
    );
    let mut automatic = base;
    automatic.served = Some(ServedClass {
        auto_inject: Visibility::Visible,
        auto_search: Visibility::Visible,
        visibility: Visibility::Visible,
        ..labeled()
    });
    assert!(
        Surface::ALL
            .iter()
            .all(|s| judge_surface(&automatic, *s).permits())
    );
    let mut split = automatic;
    split.served.as_mut().unwrap().auto_search = Visibility::Hidden;
    assert!(judge_surface(&split, Surface::AutoInject).permits());
    assert!(!judge_surface(&split, Surface::AutoSearch).permits());
}

#[test]
fn reordered_predicates_an_added_verdict_or_a_changed_cell_are_spec_drift() {
    let base = fixture();
    let drift = |mutate: fn(&mut Value)| {
        let mut fixture = base.clone();
        mutate(&mut fixture);
        let found = protocol_digest(ELIGIBILITY_SPEC_PROTOCOL, &fixture).unwrap();
        assert_ne!(found, ELIGIBILITY_SPEC_DIGEST);
        assert_eq!(
            check_spec(&fixture).unwrap_err(),
            SpecError::SpecDrift {
                expected: ELIGIBILITY_SPEC_DIGEST.to_string(),
                found,
            }
        );
        fixture
    };
    let swapped = drift(|f| f["predicates"].as_array_mut().unwrap().swap(5, 6));
    drift(|f| {
        f["verdicts"]
            .as_array_mut()
            .unwrap()
            .push(json!("quarantined"))
    });
    drift(|f| f["vectors"][0]["facts"]["state"]["in_scope"] = json!(false));
    drift(|f| f["vectors"][0]["verdict"] = json!("hidden"));

    // A fixture canonical JSON cannot encode is refused by name.
    let mut fractional = base.clone();
    fractional["vectors"][0]["facts"]["state"]["in_scope"] = json!(0.5);
    assert!(matches!(
        check_spec(&fractional),
        Err(SpecError::NotCanonical(_))
    ));

    // Whitespace is not drift: the digest is over the parsed value.
    let text = serde_json::to_string_pretty(&base).unwrap();
    let spaced = text.replace('\n', "\n\n  ").replace(": ", ":    ");
    let reparsed: Value = serde_json::from_str(&spaced).unwrap();
    check_spec(&reparsed).unwrap();

    // The world reducer refuses a drifted or uncanonical fixture and yields no truth.
    let world = world();
    assert!(matches!(
        reduce(&world.log, &swapped, &query(END, END)),
        Err(ReduceError::Spec(SpecError::SpecDrift { .. }))
    ));
    assert!(matches!(
        reduce(&world.log, &fractional, &query(END, END)),
        Err(ReduceError::Spec(SpecError::NotCanonical(_)))
    ));
    reduce(&world.log, &reparsed, &query(END, END)).unwrap();

    // Every adjacent transposition of the order is visible on some fact tuple
    // except the first pair, which no realizable tuple can separate: a
    // superseded object has state.
    let realizable: Vec<FactTuple> = spec().vectors.iter().map(|v| v.facts).collect();
    for i in 0..PREDICATES.len() - 1 {
        let mut predicates: Vec<Predicate> = PREDICATES.to_vec();
        predicates.swap(i, i + 1);
        let disagrees = realizable
            .iter()
            .any(|facts| judge_with(&predicates, facts) != judge(facts));
        let separable = i != 0;
        assert_eq!(disagrees, separable, "transposition {i}");
    }
    assert_eq!(PREDICATES[0].name, PredicateKind::StateAbsent);
    assert_eq!(PREDICATES[1].name, PredicateKind::Superseded);
}

fn revisions_of(world: &World, target: &EventId) -> usize {
    world
        .log
        .events
        .iter()
        .filter(|r| match &r.payload {
            Payload::Correction { target: t, .. } | Payload::Invalidation { target: t } => {
                t == target
            }
            _ => false,
        })
        .count()
}

/// The facts an independent scan gives `event` at a cut.
fn expected_facts(log: &[Event], event: &Event, q: &Query) -> FactTuple {
    let in_cut = |e: &Event| {
        e.valid_time_ms <= q.valid_time_ms && e.observation_time_ms <= q.observation_time_ms
    };
    let targeted = |correction: bool| {
        log.iter().filter(|e| in_cut(e)).any(|e| match &e.payload {
            Payload::Correction { target, .. } => correction && *target == event.id,
            Payload::Invalidation { target } => !correction && *target == event.id,
            _ => false,
        })
    };
    let superseded = targeted(true);
    FactTuple {
        state: in_cut(event).then_some(StateFacts {
            superseded,
            invalidated: superseded || targeted(false),
            revision_matches: true,
            in_scope: q.scope.contains(&event.entity_id),
            registry_sensitivity: q.registry_sensitivity,
        }),
        served: q.served,
        artifact: None,
        destination: q.destination,
    }
}

#[test]
fn reduction_agrees_with_an_independent_scan_at_every_cut() {
    let world = world();
    let fixture = fixture();
    let valid_times: BTreeSet<i64> = world.log.events.iter().map(|e| e.valid_time_ms).collect();
    let observation_times: BTreeSet<i64> = world
        .log
        .events
        .iter()
        .map(|e| e.observation_time_ms)
        .collect();
    let mut cuts = 0;
    let mut verdicts_seen = BTreeSet::new();
    for &valid in valid_times.iter().chain([&(EPOCH_MS - 1), &END]) {
        for &observation in observation_times.iter().chain([&END]) {
            let q = query(valid, observation);
            let truth = reduce(&world.log, &fixture, &q).unwrap();
            let mut expected_required = BTreeSet::new();
            for event in &world.log.events {
                if matches!(event.payload, Payload::Invalidation { .. }) {
                    assert!(!truth.units.contains_key(&event.id));
                    continue;
                }
                let facts = expected_facts(&world.log.events, event, &q);
                let unit = &truth.units[&event.id];
                assert_eq!(
                    unit.facts, facts,
                    "{:?} at ({valid}, {observation})",
                    event.id
                );
                let verdict = judge(&facts);
                assert_eq!(unit.verdict, verdict);
                verdicts_seen.insert(verdict);
                if verdict == Verdict::Ok {
                    expected_required.insert(event.id.clone());
                }
            }
            assert_eq!(truth.required, expected_required);
            cuts += 1;
        }
    }
    assert!(cuts > 100, "{cuts}");
    assert_eq!(
        verdicts_seen,
        BTreeSet::from([
            Verdict::Ok,
            Verdict::Retracted,
            Verdict::Superseded,
            Verdict::WrongScope
        ]),
        "units are judged at their own revision, so stale is out of reach"
    );
    let before_everything = reduce(&world.log, &fixture, &query(EPOCH_MS - 1, END)).unwrap();
    assert!(before_everything.required.is_empty());
    assert!(
        before_everything
            .units
            .values()
            .all(|u| u.verdict == Verdict::Retracted)
    );
}

#[test]
fn a_correction_takes_over_when_true_and_known_and_an_invalidation_only_retracts() {
    let world = world();
    let fixture = fixture();
    // A correction strictly after its otherwise unrevised target, published
    // later than it happened, so both cuts have room to separate.
    let (correction, target) = world
        .log
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::Correction { target, .. } => {
                let target_event = world.log.events.iter().find(|t| t.id == *target).unwrap();
                (revisions_of(&world, target) == 1
                    && target_event.valid_time_ms < e.valid_time_ms
                    && e.observation_time_ms > e.valid_time_ms
                    && target_event.observation_time_ms < e.observation_time_ms)
                    .then_some((e, target))
            }
            _ => None,
        })
        .expect("the fixture has a separable correction");
    assert_eq!(correction.id, EventId("session:session-0:11".to_string()));
    assert_eq!(*target, EventId("session:session-0:1".to_string()));

    let before = reduce(
        &world.log,
        &fixture,
        &query(correction.valid_time_ms - 1, END),
    )
    .unwrap();
    assert_eq!(before.units[target].verdict, Verdict::Ok);
    assert!(before.required.contains(target));
    assert!(before.units[&correction.id].facts.state.is_none());
    assert_eq!(before.units[&correction.id].verdict, Verdict::Retracted);

    let at = reduce(&world.log, &fixture, &query(correction.valid_time_ms, END)).unwrap();
    let target_state = at.units[target].facts.state.unwrap();
    assert!(target_state.superseded && target_state.invalidated);
    assert_eq!(at.units[target].verdict, Verdict::Superseded);
    assert!(!at.required.contains(target));
    assert_eq!(at.units[&correction.id].verdict, Verdict::Ok);
    assert!(at.required.contains(&correction.id));

    // Knowledge lags truth: at the same valid time but observed before the
    // correction was published, the target is still the truth.
    let unknown = reduce(
        &world.log,
        &fixture,
        &query(correction.valid_time_ms, correction.observation_time_ms - 1),
    )
    .unwrap();
    assert_eq!(unknown.units[target].verdict, Verdict::Ok);
    assert!(unknown.required.contains(target));
    assert!(unknown.units[&correction.id].facts.state.is_none());

    // Invalidation retracts without a successor, in a world without corrections.
    let mut invalidating = config();
    invalidating.sessions.truncate(1);
    invalidating.sessions[0].correction_every = 0;
    invalidating.sessions[0].invalidation_every = 2;
    let retracting = generate_all(SEED, &invalidating, Mode::Generate).unwrap();
    let (invalidation, retracted) = retracting
        .log
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::Invalidation { target } => Some((e, target)),
            _ => None,
        })
        .unwrap();
    let after = reduce(
        &retracting.log,
        &fixture,
        &query(invalidation.valid_time_ms, END),
    )
    .unwrap();
    let retracted_state = after.units[retracted].facts.state.unwrap();
    assert!(retracted_state.invalidated && !retracted_state.superseded);
    assert_eq!(after.units[retracted].verdict, Verdict::Retracted);
    assert!(
        !after.units.contains_key(&invalidation.id),
        "invalidations are not units"
    );

    // A target the log does not contain changes nothing.
    let mut orphaned = world.log.clone();
    let position = orphaned
        .events
        .iter()
        .position(|e| e.id == *target)
        .unwrap();
    orphaned = orphaned.without(&orphaned.events[position].id.clone());
    let orphan_truth = reduce(&orphaned, &fixture, &query(END, END)).unwrap();
    assert!(!orphan_truth.units.contains_key(target));
    assert_eq!(orphan_truth.units[&correction.id].verdict, Verdict::Ok);
    let complete = reduce(&world.log, &fixture, &query(END, END)).unwrap();
    for (id, unit) in &orphan_truth.units {
        assert_eq!(unit, &complete.units[id]);
    }
}

#[test]
fn admission_and_destination_travel_with_the_query_and_refusals_are_typed() {
    let world = world();
    let fixture = fixture();
    let end = reduce(&world.log, &fixture, &query(END, END)).unwrap();
    let out_of_scope: Vec<&EventId> = end
        .units
        .iter()
        .filter(|(_, unit)| unit.verdict == Verdict::WrongScope)
        .map(|(id, _)| id)
        .collect();
    assert!(!out_of_scope.is_empty());
    assert!(out_of_scope.iter().all(|id| id.0.contains("session-1")));

    let mut unadmitted = query(END, END);
    unadmitted.served = None;
    let hidden = reduce(&world.log, &fixture, &unadmitted).unwrap();
    assert!(hidden.required.is_empty());
    assert!(hidden.units.values().any(|u| u.verdict == Verdict::Hidden));

    let mut remote = query(END, END);
    remote.destination = Destination::Remote;
    remote.served = Some(ServedClass {
        sensitivity: Sensitivity::Sensitive,
        ..labeled()
    });
    let refused = reduce(&world.log, &fixture, &remote).unwrap();
    assert!(refused.required.is_empty());
    assert!(
        refused
            .units
            .values()
            .any(|u| u.verdict == Verdict::ProviderSensitive)
    );

    // The truth is a pure function of the log, fixture, and query.
    assert_eq!(reduce(&world.log, &fixture, &query(END, END)).unwrap(), end);

    // Refusals name their cause and yield no truth.
    let mut invalid = world.log.clone();
    invalid.events.swap(0, 1);
    assert_eq!(
        reduce(&invalid, &fixture, &query(END, END)).unwrap_err(),
        ReduceError::Log(LogError::NotLinearized { position: 1 })
    );
    let mut wrong_schema = world.log.clone();
    wrong_schema.schema = "eval-events/v0".to_string();
    assert!(matches!(
        reduce(&wrong_schema, &fixture, &query(END, END)).unwrap_err(),
        ReduceError::Log(LogError::SchemaMismatch {
            field: "schema",
            ..
        })
    ));
    let mut tight = query(END, END);
    tight.max_events_per_log = world.log.events.len() as u32 - 1;
    assert!(matches!(
        reduce(&world.log, &fixture, &tight).unwrap_err(),
        ReduceError::Log(LogError::EventBound { .. })
    ));
    for (mutate, field) in [
        (
            (|q: &mut Query| q.valid_time_ms = -1) as fn(&mut Query),
            "valid_time_ms",
        ),
        (|q| q.valid_time_ms = MAX_VALID_TIME_MS + 1, "valid_time_ms"),
        (|q| q.observation_time_ms = -1, "observation_time_ms"),
        (|q| q.max_events_per_log = 0, "max_events_per_log"),
    ] {
        let mut q = query(END, END);
        mutate(&mut q);
        assert_eq!(
            reduce(&world.log, &fixture, &q).unwrap_err(),
            ReduceError::InvalidQuery(field)
        );
    }
}

#[test]
fn truth_and_queries_round_trip_through_serde_in_canonical_form() {
    let world = world();
    let truth = reduce(&world.log, &fixture(), &query(END, END)).unwrap();
    let value = serde_json::to_value(&truth).unwrap();
    assert_eq!(value["query"]["valid_time_ms"], json!(END.to_string()));
    assert_eq!(
        value["query"]["observation_time_ms"],
        json!(END.to_string())
    );
    let unit = value["units"].as_object().unwrap().values().next().unwrap();
    assert_eq!(
        unit["facts"]["state"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        [
            "in_scope",
            "invalidated",
            "registry_sensitivity",
            "revision_matches",
            "superseded"
        ]
    );
    let round_trip: Truth = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(round_trip, truth);
    let mut unknown = value;
    unknown["units"]
        .as_object_mut()
        .unwrap()
        .values_mut()
        .next()
        .unwrap()["hostname"] = json!("x");
    assert!(serde_json::from_value::<Truth>(unknown).is_err());
    let mut fractional = serde_json::to_value(query(END, END)).unwrap();
    fractional["valid_time_ms"] = json!(END);
    assert!(serde_json::from_value::<Query>(fractional).is_err());
    let surfaces: Vec<Value> = Surface::ALL
        .iter()
        .map(|s| serde_json::to_value(s).unwrap())
        .collect();
    assert_eq!(
        surfaces,
        [
            json!("auto_inject"),
            json!("auto_search"),
            json!("explicit_search")
        ]
    );
    let _ = BTreeMap::<Surface, ()>::new();
}

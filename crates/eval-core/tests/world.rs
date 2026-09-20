//! World generation, typed choice replay, event-log validation, and the
//! step-drive contract.

mod support;

use std::collections::{BTreeMap, BTreeSet};

use context_core::canonical_json::protocol_digest;
use eval_core::{
    CANDIDATES_DIGEST_PROTOCOL, CausalEdge, ChoiceKind, ChoiceSite, EVENT_SCHEMA_VERSION, Event,
    EventId, EventLog, GENERATOR_VERSION, Generator, LINEARIZATION_RULE_VERSION, LogError,
    MAX_REVISION_LEAD_MS, MAX_VALID_TIME_MS, Mode, Payload, RANDOM_SCHEMA_VERSION, ReplayRefusal,
    RepositorySpec, RunIdentity, SessionSpec, Step, StreamLabel, Tape, World, WorldConfig,
    WorldError, eval_run_id, generate_all, keyed_draw, tape_identity,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use support::{WORLD_EPOCH_MS as EPOCH_MS, WORLD_SEED as SEED, world_config as config};

fn world() -> World {
    generate_all(SEED, &config(), Mode::Generate).unwrap()
}

fn max() -> usize {
    config().max_events_per_log as usize
}

/// The text a reader sees plus same-stream revision targets; `cites` is the
/// one deliberately cross-stream field and is compared separately.
fn semantic_digest(event: &Event) -> String {
    let value = match &event.payload {
        Payload::Message { text, .. } => json!({"text": text}),
        Payload::Correction { text, target } => json!({"text": text, "target": target}),
        Payload::Invalidation { target } => json!({"target": target}),
        Payload::ToolSpan { output, .. } => json!({"output": output}),
        Payload::Commit { message, .. } => json!({"message": message}),
        Payload::Rename { .. } => serde_json::to_value(&event.payload).unwrap(),
    };
    protocol_digest("test-semantic/v1", &value).unwrap()
}

type Mutation<T> = Box<dyn Fn(&mut T)>;

fn tape_digest(tape: &Tape) -> String {
    protocol_digest("test-tape/v1", &serde_json::to_value(tape).unwrap()).unwrap()
}

fn position(log: &EventLog, id: &EventId) -> usize {
    log.events.iter().position(|e| e.id == *id).unwrap()
}

/// The per-slot event count, restated from the config rule independently of
/// the generator's own `events_at`.
fn expected_slot_events(config: &WorldConfig, entity: &str, k: u32) -> usize {
    let fires = |every: u32| every > 0 && (k + 1).is_multiple_of(every);
    if let Some(index) = entity.strip_prefix("session-") {
        let spec = &config.sessions[index.parse::<usize>().unwrap()];
        1 + usize::from(fires(spec.tool_span_every))
            + usize::from(k > 0 && fires(spec.correction_every))
            + usize::from(k > 0 && fires(spec.invalidation_every))
    } else {
        let index = entity.strip_prefix("repository-").unwrap();
        let spec = &config.repositories[index.parse::<usize>().unwrap()];
        1 + usize::from(fires(spec.rename_every))
    }
}

#[test]
fn generation_is_a_pure_function_of_seed_and_config() {
    let base = world();
    assert_eq!(base.log.events.len() as u64, config().declared_events());
    assert_eq!(base.log.events.len(), 34, "the fixture's declared count");
    let again = world();
    assert_eq!(base, again);
    assert_eq!(base.log.digest(), again.log.digest());

    let other_seed = generate_all(SEED + 1, &config(), Mode::Generate).unwrap();
    assert_ne!(base.log.digest(), other_seed.log.digest());
    assert_ne!(tape_digest(&base.tape), tape_digest(&other_seed.tape));

    let mut other_config = config();
    other_config.tick_ms = 2_000;
    let other = generate_all(SEED, &other_config, Mode::Generate).unwrap();
    assert_ne!(base.log.digest(), other.log.digest());

    assert_eq!(base.log.schema, EVENT_SCHEMA_VERSION);
    assert_eq!(
        base.log.linearization_rule_version,
        LINEARIZATION_RULE_VERSION
    );
    assert_eq!(base.tape.identity, tape_identity(SEED, &config()));

    // The generator constants are run-identity components: each one, the seed,
    // and the config change `eval_run_id`.
    let identity = RunIdentity {
        root_seed: SEED,
        config: serde_json::to_value(config()).unwrap(),
        random_schema_version: RANDOM_SCHEMA_VERSION.to_string(),
        generator_version: GENERATOR_VERSION.to_string(),
        linearization_rule_version: LINEARIZATION_RULE_VERSION.to_string(),
        ..support::identity()
    };
    let run_id = eval_run_id(&identity).unwrap();
    let mutations: [Mutation<RunIdentity>; 5] = [
        Box::new(|i| i.root_seed += 1),
        Box::new(|i| i.config = json!({"tick_ms": "2000"})),
        Box::new(|i| i.random_schema_version.push('x')),
        Box::new(|i| i.generator_version.push('x')),
        Box::new(|i| i.linearization_rule_version.push('x')),
    ];
    for mutate in mutations {
        let mut changed = identity.clone();
        mutate(&mut changed);
        assert_ne!(eval_run_id(&changed).unwrap(), run_id);
    }
}

#[test]
fn keyed_draws_are_pinned_and_distinct_per_kind_actor_site_and_occurrence() {
    let site = |kind, actor: &str, slot: &str, occurrence| ChoiceSite {
        kind,
        actor: actor.to_string(),
        site: slot.to_string(),
        occurrence,
    };
    let base = site(ChoiceKind::TimeGap, "session-0", "slot:1", 0);
    // Frozen so a key-shape or protocol change is reviewed.
    assert_eq!(keyed_draw(SEED, &base), 10_975_382_604_116_890_793);
    let variants = [
        site(ChoiceKind::ObservationLag, "session-0", "slot:1", 0),
        site(ChoiceKind::Cites, "session-0", "slot:1", 0),
        site(ChoiceKind::TimeGap, "session-1", "slot:1", 0),
        site(ChoiceKind::TimeGap, "session-0", "slot:2", 0),
        site(ChoiceKind::TimeGap, "session-0", "slot:1", 1),
    ];
    let mut draws = BTreeSet::from([keyed_draw(SEED, &base)]);
    for variant in &variants {
        assert!(draws.insert(keyed_draw(SEED, variant)), "{variant:?}");
    }
    assert!(draws.insert(keyed_draw(SEED + 1, &base)));
    assert_eq!(ChoiceKind::TimeGap.axis(), ChoiceKind::Cites.axis());
}

#[test]
fn every_choice_kind_is_recorded_and_the_tape_replays_the_same_world() {
    let base = world();
    let kinds: BTreeSet<ChoiceKind> = base.tape.entries.iter().map(|c| c.site.kind).collect();
    let all = BTreeSet::from([
        ChoiceKind::TimeGap,
        ChoiceKind::ObservationLag,
        ChoiceKind::TextWord,
        ChoiceKind::Cites,
        ChoiceKind::CorrectionTarget,
        ChoiceKind::InvalidationTarget,
        ChoiceKind::RenameTarget,
    ]);
    assert_eq!(kinds, all, "every choice kind's refusal path is reachable");
    assert!(
        base.tape
            .entries
            .iter()
            .any(|c| c.site.kind == ChoiceKind::TextWord && c.site.occurrence == 2),
        "a slot with three text draws exercises the occurrence counter"
    );

    let replayed = generate_all(SEED, &config(), Mode::ReplayTape(base.tape.clone())).unwrap();
    assert_eq!(replayed, base);
    let mut generator =
        Generator::new(SEED, &config(), Mode::ReplayTape(base.tape.clone())).unwrap();
    while let Step::Emitted(_) = generator.step().unwrap() {}
    assert_eq!(generator.finish().unwrap(), base);

    // Recorded candidate digests cover the candidate values: every `Cites`
    // entry digests the commits emitted before the citing message, and every
    // revision target digests the session's earlier messages.
    let commits: Vec<&Event> = base
        .log
        .events
        .iter()
        .filter(|e| matches!(e.payload, Payload::Commit { .. }))
        .collect();
    let mut checked = 0;
    for choice in &base.tape.entries {
        let k: u32 = choice
            .site
            .site
            .strip_prefix("slot:")
            .unwrap()
            .parse()
            .unwrap();
        let candidates: Vec<EventId> = match choice.site.kind {
            ChoiceKind::Cites => {
                let message = base
                    .log
                    .events
                    .iter()
                    .find(|e| {
                        e.entity_id == choice.site.actor
                            && matches!(&e.payload, Payload::Message { message_id, .. }
                                if *message_id == format!("{}-m{k}", choice.site.actor))
                    })
                    .unwrap();
                commits
                    .iter()
                    .filter(|c| c.valid_time_ms <= message.valid_time_ms)
                    .map(|c| c.id.clone())
                    .collect()
            }
            ChoiceKind::CorrectionTarget | ChoiceKind::InvalidationTarget => base
                .log
                .events
                .iter()
                .filter(|e| e.entity_id == choice.site.actor)
                .filter(|e| {
                    matches!(&e.payload, Payload::Message { message_id, .. }
                    if message_id.rsplit_once("-m").unwrap().1.parse::<u32>().unwrap() < k)
                })
                .map(|e| e.id.clone())
                .collect(),
            _ => continue,
        };
        let digest = protocol_digest(
            CANDIDATES_DIGEST_PROTOCOL,
            &serde_json::to_value(&candidates).unwrap(),
        )
        .unwrap();
        assert_eq!(choice.candidates_digest, digest, "{choice:?}");
        assert!((choice.selected_index as usize) < candidates.len());
        checked += 1;
    }
    assert!(checked >= 10, "{checked}");
}

#[test]
fn replay_refuses_a_missing_changed_or_out_of_range_choice_and_emits_no_log() {
    let base = world();
    let entries = base.tape.entries.len();
    let probes = [0, entries / 2, entries - 1];

    for &i in &probes {
        let mut short = base.tape.clone();
        short.entries.truncate(i);
        let refusal = generate_all(SEED, &config(), Mode::ReplayTape(short)).unwrap_err();
        assert_eq!(
            refusal,
            WorldError::Replay(ReplayRefusal::MissingChoice { entry: i })
        );

        let mut changed = base.tape.clone();
        changed.entries[i].candidates_digest = "00".repeat(32);
        let refusal = generate_all(SEED, &config(), Mode::ReplayTape(changed)).unwrap_err();
        assert!(
            matches!(
                refusal,
                WorldError::Replay(ReplayRefusal::ChangedCandidates { entry, .. }) if entry == i
            ),
            "{refusal}"
        );

        let mut out_of_range = base.tape.clone();
        out_of_range.entries[i].selected_index = u32::MAX;
        let refusal = generate_all(SEED, &config(), Mode::ReplayTape(out_of_range)).unwrap_err();
        assert!(
            matches!(
                refusal,
                WorldError::Replay(ReplayRefusal::IndexOutOfRange { entry, selected_index: u32::MAX, .. })
                    if entry == i
            ),
            "{refusal}"
        );

        let mut moved = base.tape.clone();
        moved.entries[i].site.actor = "nobody".to_string();
        let refusal = generate_all(SEED, &config(), Mode::ReplayTape(moved)).unwrap_err();
        assert!(
            matches!(
                refusal,
                WorldError::Replay(ReplayRefusal::SiteMismatch { entry, .. }) if entry == i
            ),
            "{refusal}"
        );
    }

    // The exact index boundary on a choice whose candidate count is known:
    // the first message's citation sees exactly one commit.
    let (i, cites) = base
        .tape
        .entries
        .iter()
        .enumerate()
        .find(|(_, c)| c.site.kind == ChoiceKind::Cites)
        .unwrap();
    assert_eq!(cites.selected_index, 0);
    let mut boundary = base.tape.clone();
    boundary.entries[i].selected_index = 1;
    assert_eq!(
        generate_all(SEED, &config(), Mode::ReplayTape(boundary)).unwrap_err(),
        WorldError::Replay(ReplayRefusal::IndexOutOfRange {
            entry: i,
            selected_index: 1,
            candidates: 1,
        })
    );

    // A tape is bound to the seed and config it was recorded under.
    for (seed, config) in [
        (SEED + 1, config()),
        (SEED, {
            let mut c = config();
            c.tick_ms = 7_777;
            c
        }),
    ] {
        assert_eq!(
            generate_all(seed, &config, Mode::ReplayTape(base.tape.clone())).unwrap_err(),
            WorldError::Replay(ReplayRefusal::TapeMismatch {
                expected: tape_identity(seed, &config),
                found: base.tape.identity.clone(),
            })
        );
    }

    // Entries past the generator's last choice are ignored, not refused: the
    // replay succeeds and the returned tape holds only the consumed entries.
    let mut trailing = base.tape.clone();
    trailing.entries.push(base.tape.entries[0].clone());
    let replayed = generate_all(SEED, &config(), Mode::ReplayTape(trailing)).unwrap();
    assert_eq!(replayed, base);
    assert_eq!(replayed.tape.entries.len(), entries);

    // A refusal mid-drive poisons the generator: no partial log, no finish.
    let mut short = base.tape.clone();
    short.entries.truncate(entries / 2);
    let mut generator = Generator::new(SEED, &config(), Mode::ReplayTape(short)).unwrap();
    let first_error = loop {
        match generator.step() {
            Ok(Step::Emitted(_)) => continue,
            Ok(Step::Done) => panic!("a short tape must refuse"),
            Err(error) => break error,
        }
    };
    assert!(matches!(
        first_error,
        WorldError::Replay(ReplayRefusal::MissingChoice { .. })
    ));
    assert_eq!(generator.step().unwrap_err(), first_error);
    assert_eq!(generator.log().unwrap_err(), first_error);
    assert_eq!(generator.finish().unwrap_err(), first_error);
}

/// Every event of `base` outside `removed_entity` keeps its semantic digest
/// and both times in `variant`.
fn assert_other_streams_unchanged(base: &EventLog, variant: &EventLog, removed_entity: &str) {
    let variant_events: BTreeMap<&EventId, &Event> =
        variant.events.iter().map(|e| (&e.id, e)).collect();
    let mut compared = 0;
    for event in base.events.iter().filter(|e| e.entity_id != removed_entity) {
        let other = variant_events
            .get(&event.id)
            .unwrap_or_else(|| panic!("{:?} survives the removal", event.id));
        assert_eq!(
            semantic_digest(event),
            semantic_digest(other),
            "{:?}",
            event.id
        );
        assert_eq!(event.valid_time_ms, other.valid_time_ms);
        assert_eq!(event.observation_time_ms, other.observation_time_ms);
        compared += 1;
    }
    assert!(compared > 0);
}

#[test]
fn removing_an_unrelated_event_changes_no_later_text_or_rename_digest() {
    let base = world();

    let mut shorter_session = config();
    shorter_session.sessions[1].messages -= 1;
    let variant = generate_all(SEED, &shorter_session, Mode::Generate).unwrap();
    assert_eq!(
        variant.log.events.len() as u64,
        shorter_session.declared_events()
    );
    assert_other_streams_unchanged(&base.log, &variant.log, "session-1");
    let unchanged_cites = base
        .log
        .events
        .iter()
        .filter(|e| e.entity_id != "session-1")
        .all(|e| {
            variant
                .log
                .events
                .iter()
                .any(|v| v.id == e.id && v.payload == e.payload)
        });
    assert!(
        unchanged_cites,
        "a session removal leaves other payloads whole"
    );

    // Removing a commit keeps every text, target, rename digest, and time on the
    // other streams. Citations draw over the shared commit list, so changing the
    // repositories may re-point them: the one deliberate cross-stream coupling.
    let mut shorter_repository = config();
    shorter_repository.repositories[0].commits -= 1;
    let variant = generate_all(SEED, &shorter_repository, Mode::Generate).unwrap();
    assert_other_streams_unchanged(&base.log, &variant.log, "repository-0");
    let mut more_repositories = config();
    more_repositories.repositories.push(RepositorySpec {
        commits: 3,
        rename_every: 0,
    });
    let variant = generate_all(SEED, &more_repositories, Mode::Generate).unwrap();
    assert_other_streams_unchanged(&base.log, &variant.log, "repository-1");
    let mut repointed = 0;
    for event in &base.log.events {
        let other = variant
            .log
            .events
            .iter()
            .find(|v| v.id == event.id)
            .unwrap();
        if other.payload == event.payload {
            continue;
        }
        match (&event.payload, &other.payload) {
            (
                Payload::Message {
                    cites: a, text: t, ..
                },
                Payload::Message {
                    cites: b, text: u, ..
                },
            ) => {
                assert_ne!(a, b);
                assert_eq!(t, u);
                repointed += 1;
            }
            _ => panic!("only citations may differ: {:?}", event.id),
        }
    }
    assert!(repointed > 0, "the fixture exercises the citation coupling");

    // Later events on the surviving streams exist, so the check is not vacuous.
    let removed_time = base
        .log
        .events
        .iter()
        .filter(|e| e.entity_id == "session-1")
        .map(|e| e.valid_time_ms)
        .max()
        .unwrap();
    let later_streams: BTreeSet<&str> = base
        .log
        .events
        .iter()
        .filter(|e| e.entity_id != "session-1" && e.valid_time_ms >= removed_time)
        .map(|e| e.entity_id.as_str())
        .collect();
    assert!(later_streams.len() >= 2, "{later_streams:?}");

    // Negative control: one sequential stream shared by every entity.
    let coupled = |sessions: [u32; 2]| -> EventLog {
        let mut counter = 0u64;
        let mut draw = || {
            let mut hasher = Sha256::new();
            hasher.update(SEED.to_le_bytes());
            hasher.update(counter.to_le_bytes());
            counter += 1;
            hasher.finalize()[0] as usize
        };
        let mut events = Vec::new();
        for k in 0..sessions.iter().max().copied().unwrap() {
            for (entity, messages) in sessions.iter().enumerate() {
                if k >= *messages {
                    continue;
                }
                let entity_id = format!("session-{entity}");
                let word = ["a", "b", "c", "d", "e", "f", "g"][draw() % 7];
                events.push(Event {
                    id: EventId::derive(StreamLabel::Session, &entity_id, k),
                    stream: StreamLabel::Session,
                    entity_id: entity_id.clone(),
                    local_seq: k,
                    valid_time_ms: EPOCH_MS + i64::from(k) * 1_000,
                    observation_time_ms: EPOCH_MS + i64::from(k) * 1_000,
                    causal_depth: 0,
                    payload: Payload::Message {
                        message_id: format!("{entity_id}-m{k}"),
                        role: "user".to_string(),
                        text: word.to_string(),
                        cites: None,
                    },
                });
            }
        }
        EventLog {
            schema: EVENT_SCHEMA_VERSION.to_string(),
            linearization_rule_version: LINEARIZATION_RULE_VERSION.to_string(),
            events,
            causal_edges: Vec::new(),
        }
    };
    let coupled_base = coupled([8, 6]);
    let coupled_variant = coupled([8, 5]);
    let changed = coupled_base
        .events
        .iter()
        .filter(|e| e.entity_id == "session-0")
        .any(|event| {
            let other = coupled_variant
                .events
                .iter()
                .find(|e| e.id == event.id)
                .unwrap();
            semantic_digest(event) != semantic_digest(other)
        });
    assert!(changed, "a shared stream re-rolls the other entity's text");
}

fn payload_kind(event: &Event) -> &'static str {
    match event.payload {
        Payload::Message { .. } => "message",
        Payload::ToolSpan { .. } => "tool_span",
        Payload::Commit { .. } => "commit",
        Payload::Rename { .. } => "rename",
        Payload::Correction { .. } => "correction",
        Payload::Invalidation { .. } => "invalidation",
    }
}

#[test]
fn events_are_self_contained_and_any_single_deletion_leaves_a_valid_log() {
    let base = world().log;
    base.validate(max()).unwrap();
    let kinds: BTreeSet<&str> = base.events.iter().map(payload_kind).collect();
    assert_eq!(kinds.len(), 6, "{kinds:?}");
    assert!(
        base.events.iter().any(|e| matches!(
            &e.payload,
            Payload::Rename {
                previous: Some(_),
                ..
            }
        )),
        "a rename chain"
    );
    assert!(
        base.events
            .iter()
            .any(|e| matches!(&e.payload, Payload::Message { cites: Some(_), .. })),
        "a cross-stream citation"
    );
    for event in &base.events {
        if let Payload::Commit { oid, .. } | Payload::Rename { oid, .. } = &event.payload {
            assert!(oid.len() == 40 && oid.bytes().all(|b| b.is_ascii_hexdigit()));
        }
    }
    let oids: BTreeSet<&str> = base
        .events
        .iter()
        .filter_map(|e| match &e.payload {
            Payload::Commit { oid, .. } => Some(oid.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(oids.len(), 8, "one oid per commit");

    for event in &base.events {
        let reduced = base.without(&event.id);
        reduced
            .validate(max())
            .unwrap_or_else(|error| panic!("deleting {:?}: {error}", event.id));
        assert_eq!(reduced.events.len(), base.events.len() - 1);
        for survivor in &reduced.events {
            let original = base.events.iter().find(|e| e.id == survivor.id).unwrap();
            assert_eq!(survivor, original, "no repair of surviving payloads");
        }
        assert!(
            reduced
                .causal_edges
                .iter()
                .all(|edge| edge.from != event.id && edge.to != event.id)
        );
    }

    // A deletion that leaves edges behind is refused.
    let target = base.causal_edges[0].to.clone();
    let mut dangling = base.clone();
    dangling.events.retain(|e| e.id != target);
    assert!(matches!(
        dangling.validate(max()),
        Err(LogError::DanglingEdge { .. })
    ));

    // A positional id is refused whether or not anything was deleted.
    let mut positional = base.clone();
    positional.events[3].id = EventId("3".to_string());
    assert_eq!(
        positional.validate(max()),
        Err(LogError::IdNotDerived {
            id: EventId("3".to_string()),
        })
    );

    // Two events sharing one id are refused even when their keys differ.
    let mut duplicate = base.clone();
    let mut twin = duplicate.events[0].clone();
    twin.valid_time_ms += 1;
    twin.causal_depth = 0;
    let at = duplicate
        .events
        .iter()
        .position(|e| e.key() > twin.key())
        .unwrap();
    duplicate.events.insert(at, twin.clone());
    assert_eq!(
        duplicate.validate(max()),
        Err(LogError::DuplicateId { id: twin.id })
    );

    for (field, found) in [
        ("schema", "eval-events/v0"),
        ("linearization_rule_version", "x"),
    ] {
        let mut wrong = base.clone();
        if field == "schema" {
            wrong.schema = found.to_string();
        } else {
            wrong.linearization_rule_version = found.to_string();
        }
        assert_eq!(
            wrong.validate(max()),
            Err(LogError::SchemaMismatch {
                field,
                found: found.to_string(),
            })
        );
    }
}

/// Whole-DAG longest path from the edges alone.
fn recomputed_depths(log: &EventLog) -> BTreeMap<EventId, u32> {
    let mut depths: BTreeMap<EventId, u32> = BTreeMap::new();
    for event in &log.events {
        let depth = log
            .causal_edges
            .iter()
            .filter(|edge| edge.to == event.id)
            .map(|edge| depths[&edge.from] + 1)
            .max()
            .unwrap_or(0);
        depths.insert(event.id.clone(), depth);
    }
    depths
}

#[test]
fn the_log_is_linearized_by_the_versioned_key_and_equality_checks_the_edges() {
    let base = world().log;

    let depths = recomputed_depths(&base);
    for event in &base.events {
        assert_eq!(event.causal_depth, depths[&event.id], "{:?}", event.id);
    }
    let mut recomputed = base.events.clone();
    recomputed.sort_by(|a, b| {
        (
            a.valid_time_ms,
            depths[&a.id],
            a.stream.label(),
            a.entity_id.as_str(),
            a.local_seq,
        )
            .cmp(&(
                b.valid_time_ms,
                depths[&b.id],
                b.stream.label(),
                b.entity_id.as_str(),
                b.local_seq,
            ))
    });
    assert_eq!(
        recomputed, base.events,
        "stored order equals the recomputed order"
    );
    assert!(StreamLabel::Repository.label() < StreamLabel::Session.label());
    assert_eq!(
        serde_json::to_value(StreamLabel::Repository).unwrap(),
        json!("repository")
    );
    assert_eq!(
        serde_json::to_value(StreamLabel::Session).unwrap(),
        json!("session")
    );

    let keys: BTreeSet<_> = base.events.iter().map(Event::key).collect();
    assert_eq!(keys.len(), base.events.len(), "the order is strict");

    for edge in &base.causal_edges {
        assert!(
            position(&base, &edge.from) < position(&base, &edge.to),
            "{edge:?}"
        );
    }

    let by_time: BTreeMap<i64, BTreeSet<StreamLabel>> =
        base.events.iter().fold(BTreeMap::new(), |mut acc, e| {
            acc.entry(e.valid_time_ms).or_default().insert(e.stream);
            acc
        });
    assert!(
        by_time.values().any(|streams| streams.len() == 2),
        "same-millisecond events on two streams"
    );
    let stream_of = |id: &EventId| base.events[position(&base, id)].stream;
    assert!(
        base.causal_edges
            .iter()
            .any(|edge| stream_of(&edge.from) != stream_of(&edge.to)),
        "a cross-stream causal edge"
    );

    let mut fewer_edges = base.clone();
    fewer_edges.causal_edges.pop();
    assert_eq!(fewer_edges.events, base.events);
    assert_ne!(fewer_edges, base);
    assert_ne!(fewer_edges.digest(), base.digest());

    let mut swapped = base.clone();
    swapped.events.swap(0, 1);
    assert_eq!(
        swapped.validate(max()),
        Err(LogError::NotLinearized { position: 1 })
    );

    let mut reversed = base.clone();
    let CausalEdge { from, to } = reversed.causal_edges[0].clone();
    reversed.causal_edges[0] = CausalEdge { from: to, to: from };
    reversed.causal_edges.sort();
    assert!(matches!(
        reversed.validate(max()),
        Err(LogError::EdgeAgainstOrder { .. })
    ));

    let mut shallow = base.clone();
    let deep = shallow
        .events
        .iter()
        .position(|e| e.causal_depth > 0)
        .unwrap();
    shallow.events[deep].causal_depth = 0;
    assert!(matches!(
        shallow.validate(max()),
        Err(LogError::EdgeAgainstDepth { .. } | LogError::NotLinearized { .. })
    ));
}

#[test]
fn logs_tapes_and_times_round_trip_through_serde_in_canonical_form() {
    let base = world();
    let log_text = serde_json::to_string(&base.log).unwrap();
    let log: EventLog = serde_json::from_str(&log_text).unwrap();
    assert_eq!(log, base.log);
    let tape_text = serde_json::to_string(&base.tape).unwrap();
    let tape: Tape = serde_json::from_str(&tape_text).unwrap();
    assert_eq!(tape, base.tape);
    let choice = serde_json::to_value(&base.tape.entries[0]).unwrap();
    assert!(
        choice.get("kind").is_some(),
        "the site is flattened: {choice}"
    );

    let mut unknown = serde_json::to_value(&base.log).unwrap();
    unknown["events"][0]["hostname"] = json!("box");
    assert!(serde_json::from_value::<EventLog>(unknown).is_err());
    let mut unknown = serde_json::to_value(&base.tape).unwrap();
    unknown["entries"][0]["pid"] = json!(7);
    assert!(serde_json::from_value::<Tape>(unknown).is_err());

    let mut event = serde_json::to_value(&base.log.events[0]).unwrap();
    assert_eq!(event["valid_time_ms"], json!(EPOCH_MS.to_string()));
    for bad in [
        json!(EPOCH_MS),
        json!("01000"),
        json!("+1000"),
        json!(" 1000"),
        json!(""),
        json!("1e3"),
    ] {
        event["valid_time_ms"] = bad.clone();
        assert!(
            serde_json::from_value::<Event>(event.clone()).is_err(),
            "{bad}"
        );
    }
    event["valid_time_ms"] = json!("-1");
    let negative: Event = serde_json::from_value(event).unwrap();
    assert_eq!(
        negative.valid_time_ms, -1,
        "the domain is the validator's job"
    );
}

#[test]
fn time_domain_and_observation_lead_are_enforced() {
    let base = world().log;
    for event in &base.events {
        assert!((0..=MAX_VALID_TIME_MS).contains(&event.valid_time_ms));
        assert!(
            event.observation_time_ms >= event.valid_time_ms,
            "generated worlds never lead"
        );
    }

    let mut ahead = base.clone();
    ahead.events[0].observation_time_ms = ahead.events[0].valid_time_ms - MAX_REVISION_LEAD_MS - 1;
    assert_eq!(
        ahead.validate(max()),
        Err(LogError::RevisionAhead {
            id: ahead.events[0].id.clone(),
        })
    );
    let mut at_bound = base.clone();
    at_bound.events[0].observation_time_ms =
        at_bound.events[0].valid_time_ms - MAX_REVISION_LEAD_MS;
    at_bound.validate(max()).unwrap();

    let mut late = base.clone();
    let last = late.events.len() - 1;
    late.events[last].valid_time_ms = MAX_VALID_TIME_MS + 1;
    late.events[last].observation_time_ms = i64::MAX;
    assert_eq!(
        late.validate(max()),
        Err(LogError::TimeOutOfDomain {
            id: late.events[last].id.clone(),
        })
    );
    late.events[last].valid_time_ms = MAX_VALID_TIME_MS;
    late.validate(max())
        .unwrap_or_else(|e| assert!(matches!(e, LogError::NotLinearized { .. }), "{e}"));

    let mut negative = base.clone();
    negative.events[0].observation_time_ms = -1;
    assert_eq!(
        negative.validate(max()),
        Err(LogError::TimeOutOfDomain {
            id: negative.events[0].id.clone(),
        })
    );

    let mut overflow = config();
    overflow.epoch_ms = MAX_VALID_TIME_MS;
    assert!(matches!(
        generate_all(SEED, &overflow, Mode::Generate).unwrap_err(),
        WorldError::TimeOverflow { .. }
    ));
}

#[test]
fn the_event_bound_is_refused_before_generation_and_by_the_validator() {
    let declared = config().declared_events();

    let mut tight = config();
    tight.max_events_per_log = declared as u32 - 1;
    assert_eq!(
        generate_all(SEED, &tight, Mode::Generate).unwrap_err(),
        WorldError::EventBound {
            events: declared,
            max: declared as u32 - 1,
        }
    );
    assert!(matches!(
        Generator::new(SEED, &tight, Mode::Generate),
        Err(WorldError::EventBound { .. })
    ));

    // The slot-count gate refuses in O(entities) before the exact count runs.
    let mut huge = config();
    huge.sessions[0].messages = u32::MAX;
    let started = std::time::Instant::now();
    assert!(matches!(
        huge.validate(),
        Err(WorldError::EventBound { events, max: 64 }) if events >= u64::from(u32::MAX)
    ));
    assert!(started.elapsed().as_secs() < 5);

    let mut exact = config();
    exact.max_events_per_log = declared as u32;
    let world = generate_all(SEED, &exact, Mode::Generate).unwrap();
    assert_eq!(world.log.events.len() as u64, declared);
    world.log.validate(declared as usize).unwrap();
    assert_eq!(
        world.log.validate(declared as usize - 1),
        Err(LogError::EventBound {
            events: declared as usize,
            max: declared as usize - 1,
        })
    );

    let cases: [(Mutation<WorldConfig>, &str); 6] = [
        (Box::new(|c| c.max_events_per_log = 0), "max_events_per_log"),
        (Box::new(|c| c.tick_ms = 0), "tick_ms"),
        (Box::new(|c| c.epoch_ms = -1), "epoch_ms"),
        (Box::new(|c| c.epoch_ms = MAX_VALID_TIME_MS + 1), "epoch_ms"),
        (Box::new(|c| c.sessions[0].messages = 0), "entities"),
        (
            Box::new(|c| {
                c.sessions.clear();
                c.repositories.clear();
            }),
            "entities",
        ),
    ];
    for (mutate, field) in cases {
        let mut config = config();
        mutate(&mut config);
        assert_eq!(config.validate(), Err(WorldError::InvalidField(field)));
        assert_eq!(
            generate_all(SEED, &config, Mode::Generate).unwrap_err(),
            WorldError::InvalidField(field)
        );
    }

    let mut missing_bound = serde_json::to_value(config()).unwrap();
    missing_bound
        .as_object_mut()
        .unwrap()
        .remove("max_events_per_log");
    assert!(serde_json::from_value::<WorldConfig>(missing_bound).is_err());
    let mut unknown = serde_json::to_value(config()).unwrap();
    unknown["horizon_days"] = json!(365);
    assert!(serde_json::from_value::<WorldConfig>(unknown).is_err());
    let mut fractional = serde_json::to_value(config()).unwrap();
    fractional["tick_ms"] = json!("1000.0");
    assert!(serde_json::from_value::<WorldConfig>(fractional).is_err());
    let round_trip: WorldConfig =
        serde_json::from_value(serde_json::to_value(config()).unwrap()).unwrap();
    assert_eq!(round_trip, config());
}

/// The drive-time bound check can only fire if `declared_events` drifts from
/// the emitters; this sweep keeps the two in agreement.
#[test]
fn declared_events_equals_the_emitted_count_across_spec_shapes() {
    let mut worlds = 0;
    for messages in 1..5u32 {
        for tool in 0..3u32 {
            for correction in 0..4u32 {
                for invalidation in [0u32, 1, 2] {
                    for (commits, rename) in [(1u32, 0u32), (3, 1), (4, 2)] {
                        let config = WorldConfig {
                            sessions: vec![SessionSpec {
                                messages,
                                tool_span_every: tool,
                                correction_every: correction,
                                invalidation_every: invalidation,
                            }],
                            repositories: vec![RepositorySpec {
                                commits,
                                rename_every: rename,
                            }],
                            epoch_ms: EPOCH_MS,
                            tick_ms: 250,
                            max_events_per_log: 64,
                        };
                        let world = generate_all(SEED, &config, Mode::Generate).unwrap();
                        assert_eq!(world.log.events.len() as u64, config.declared_events());
                        worlds += 1;
                    }
                }
            }
        }
    }
    assert_eq!(worlds, 4 * 3 * 4 * 3 * 3);
}

#[test]
fn generate_all_equals_the_step_fold_and_every_partial_log_validates() {
    let all = world();
    let mut generator = Generator::new(SEED, &config(), Mode::Generate).unwrap();
    let mut batches: Vec<Event> = Vec::new();
    let mut slots: BTreeMap<String, u32> = BTreeMap::new();
    let slot_count = config()
        .sessions
        .iter()
        .map(|s| s.messages)
        .chain(config().repositories.iter().map(|r| r.commits))
        .sum::<u32>() as usize;
    let mut steps = 0;
    let mut multi_event_batch: Option<(String, u32, usize)> = None;
    while let Step::Emitted(events) = generator.step().unwrap() {
        steps += 1;
        assert!(
            steps <= slot_count,
            "the drive terminates within the slot count"
        );
        // One mutation: one entity, one valid time, exactly the slot's declared events.
        let entity = events[0].entity_id.clone();
        assert!(events.iter().all(|e| e.entity_id == entity));
        assert!(
            events
                .iter()
                .all(|e| e.valid_time_ms == events[0].valid_time_ms)
        );
        let k = slots.entry(entity.clone()).or_default();
        assert_eq!(
            events.len(),
            expected_slot_events(&config(), &entity, *k),
            "{entity} slot {k}"
        );
        if events.len() > 1 {
            multi_event_batch.get_or_insert((entity.clone(), *k, events.len()));
        }
        *k += 1;
        batches.extend(events);
        let partial = generator.log().unwrap();
        partial.validate(max()).unwrap();
        assert_eq!(partial.events.len(), batches.len());
        for event in &batches {
            assert!(partial.events.contains(event));
        }
    }
    assert_eq!(steps, slot_count);
    assert!(matches!(generator.step().unwrap(), Step::Done));
    assert!(matches!(generator.step().unwrap(), Step::Done));
    let folded = generator.finish().unwrap();
    assert_eq!(folded, all);
    assert_eq!(folded.log.digest(), all.log.digest());

    // The ordered fold: the emission-ordered batches sorted by the key are the log.
    batches.sort_by(|a, b| a.key().cmp(&b.key()));
    assert_eq!(batches, all.log.events);

    // `finish` drives the remaining slots itself.
    let mut early = Generator::new(SEED, &config(), Mode::Generate).unwrap();
    assert!(matches!(early.step().unwrap(), Step::Emitted(_)));
    assert_eq!(early.finish().unwrap(), all);

    // Negative control: a batch cut before quiescence fails the per-slot count.
    let (entity, k, size) = multi_event_batch.expect("a slot with several events");
    assert_ne!(size - 1, expected_slot_events(&config(), &entity, k));
}

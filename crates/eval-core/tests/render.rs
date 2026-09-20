//! The renderer's fixture shape, the identity rule's flip matrix, accounting,
//! and the coverage registry, all without a store.

mod support;

use std::collections::{BTreeMap, BTreeSet};

use eval_core::{
    AccountingError, Coverage, CoverageError, EVENT_SCHEMA_VERSION, EncodingRefusal, Event,
    EventId, EventLog, LINEARIZATION_RULE_VERSION, MARKERS, Mode, Occurrence, OccurrenceClass,
    Payload, RenderConfig, RenderError, Rendering, Span, StreamLabel, check_accounting, encode,
    generate_all, git_identity, render,
};
use serde_json::json;
use support::{WORLD_EPOCH_MS as EPOCH_MS, WORLD_SEED as SEED, world_config};

fn config() -> RenderConfig {
    RenderConfig {
        project_id: "proj-eval".to_string(),
        repository_id: "repo-0".to_string(),
        object_format: "sha1".to_string(),
    }
}

fn message_event(entity: &str, seq: u32, valid: i64, role: &str, text: &str) -> Event {
    Event {
        id: EventId::derive(StreamLabel::Session, entity, seq),
        stream: StreamLabel::Session,
        entity_id: entity.to_string(),
        local_seq: seq,
        valid_time_ms: valid,
        observation_time_ms: valid + 1_000,
        causal_depth: 0,
        payload: Payload::Message {
            message_id: format!("{entity}-m{seq}"),
            role: role.to_string(),
            text: text.to_string(),
            cites: None,
        },
    }
}

fn log(events: Vec<Event>) -> EventLog {
    EventLog {
        schema: EVENT_SCHEMA_VERSION.to_string(),
        linearization_rule_version: LINEARIZATION_RULE_VERSION.to_string(),
        events,
        causal_edges: Vec::new(),
    }
}

fn expected_ids(rendering: &Rendering) -> BTreeSet<String> {
    rendering
        .messages
        .iter()
        .flat_map(|m| m.expected.iter().map(|u| u.identity.occurrence_id.clone()))
        .collect()
}

#[test]
fn rendered_messages_have_the_shape_the_adapter_reads_with_valid_time_as_revision() {
    let user = message_event(
        "session-0",
        0,
        EPOCH_MS,
        "user",
        "keep the barrier explicit",
    );
    let mut assistant = message_event("session-0", 1, EPOCH_MS + 5_000, "assistant", "done");
    let span = Event {
        id: EventId::derive(StreamLabel::Session, "session-0", 2),
        local_seq: 2,
        causal_depth: 1,
        payload: Payload::ToolSpan {
            message_id: "session-0-m1".to_string(),
            call_id: "session-0-m1-call0".to_string(),
            output: "ok".to_string(),
        },
        ..assistant.clone()
    };
    assistant.causal_depth = 0;
    let rendering = render(&log(vec![user, assistant, span]), &config()).unwrap();
    assert_eq!(rendering.messages.len(), 2);
    assert!(rendering.commits.is_empty());
    assert!(rendering.excluded_by_rule.is_empty());

    let user = &rendering.messages[0];
    assert_eq!(
        user.message,
        json!({
            "info": {"id": "session-0-m0", "sessionID": "session-0", "role": "user", "time": {"created": EPOCH_MS}},
            "parts": [{"type": "text", "text": "keep the barrier explicit"}],
        })
    );
    assert_eq!(user.observation_time_ms, EPOCH_MS + 1_000);
    assert_eq!(user.expected.len(), 1);
    assert_eq!(user.expected[0].revision, EPOCH_MS.to_string());
    let identity = [
        ("project_id", "proj-eval"),
        ("harness", "opencode"),
        ("session_id", "session-0"),
        ("message_id", "session-0-m0"),
        ("block_index", "0"),
    ];
    let independent = encode(&Occurrence {
        class: "messages",
        identity: &identity,
        revision: &EPOCH_MS.to_string(),
        representation: "text",
        span: None,
    })
    .unwrap();
    assert_eq!(user.expected[0].identity, independent);
    assert_eq!(
        user.expected[0].event_id,
        EventId::derive(StreamLabel::Session, "session-0", 0)
    );

    let assistant = &rendering.messages[1];
    let completed = EPOCH_MS + 5_000;
    assert_eq!(
        assistant.message["info"]["time"],
        json!({"created": completed - 1, "completed": completed}),
        "created sits before completed so the adapter's precedence is exercised"
    );
    assert_eq!(
        assistant.message["parts"][0],
        json!({"type": "text", "text": "done"})
    );
    assert_eq!(
        assistant.message["parts"][1],
        json!({
            "type": "tool", "callID": "session-0-m1-call0", "tool": "bash",
            "state": {"status": "completed", "input": {}, "output": "ok",
                      "time": {"start": completed, "end": completed}},
        })
    );
    assert_eq!(assistant.expected.len(), 2);
    assert_eq!(assistant.expected[0].revision, completed.to_string());
    assert_eq!(assistant.expected[1].revision, completed.to_string());
    assert_eq!(
        assistant.expected[1].event_id,
        EventId::derive(StreamLabel::Session, "session-0", 2)
    );
    let tool_identity = [
        ("project_id", "proj-eval"),
        ("harness", "opencode"),
        ("session_id", "session-0"),
        ("parent_message_id", "session-0-m1"),
        ("tool_call_id", "session-0-m1-call0"),
        ("result_revision", &completed.to_string()),
        ("block_index", "0"),
    ];
    let tool = encode(&Occurrence {
        class: "raw_tool_spans",
        identity: &tool_identity,
        revision: &completed.to_string(),
        representation: "tool_output",
        span: None,
    })
    .unwrap();
    assert_eq!(assistant.expected[1].identity, tool);
}

#[test]
fn every_payload_kind_renders_to_units_a_commit_or_a_named_exclusion() {
    let mut config_with_everything = world_config();
    config_with_everything.sessions[0].invalidation_every = 2;
    let world = generate_all(SEED, &config_with_everything, Mode::Generate).unwrap();
    let rendering = render(&world.log, &config()).unwrap();
    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    for event in &world.log.events {
        let kind = match event.payload {
            Payload::Message { .. } => "message",
            Payload::ToolSpan { .. } => "tool_span",
            Payload::Commit { .. } => "commit",
            Payload::Rename { .. } => "rename",
            Payload::Correction { .. } => "correction",
            Payload::Invalidation { .. } => "invalidation",
        };
        *kinds.entry(kind).or_default() += 1;
    }
    assert!(
        kinds.values().all(|count| *count > 0) && kinds.len() == 6,
        "{kinds:?}"
    );
    assert_eq!(
        rendering.messages.len(),
        kinds["message"] + kinds["correction"]
    );
    assert_eq!(rendering.commits.len(), kinds["commit"]);
    let units: usize = rendering.messages.iter().map(|m| m.expected.len()).sum();
    assert_eq!(
        units,
        kinds["message"] + kinds["correction"] + kinds["tool_span"]
    );
    assert_eq!(
        rendering.excluded_by_rule,
        BTreeMap::from([
            ("rename_is_commit_metadata".to_string(), kinds["rename"]),
            (
                "invalidation_has_no_adapter".to_string(),
                kinds["invalidation"]
            ),
        ])
    );
    assert_eq!(
        expected_ids(&rendering).len(),
        units,
        "every unit has its own identity"
    );

    // A correction renders the target's message id at its own valid time: the
    // same lineage, a later revision.
    let (correction, target) = world
        .log
        .events
        .iter()
        .find_map(|e| match &e.payload {
            Payload::Correction { target, .. } => Some((e, target)),
            _ => None,
        })
        .unwrap();
    let rendered_target = rendering
        .messages
        .iter()
        .find(|m| m.event_id == *target)
        .unwrap();
    let rendered_correction = rendering
        .messages
        .iter()
        .find(|m| m.event_id == correction.id)
        .unwrap();
    assert_eq!(
        rendered_correction.message["info"]["id"],
        rendered_target.message["info"]["id"]
    );
    assert_eq!(
        rendered_correction.expected[0].identity.lineage_id,
        rendered_target.expected[0].identity.lineage_id
    );
    assert_ne!(
        rendered_correction.expected[0].identity.occurrence_id,
        rendered_target.expected[0].identity.occurrence_id
    );
    assert!(rendered_correction.expected[0].revision > rendered_target.expected[0].revision);
    for message in &rendering.messages {
        let created = message.message["info"]["time"]["created"].as_i64().unwrap();
        assert!(
            created <= message.observation_time_ms,
            "generated fixtures never lead"
        );
    }
    for commit in &rendering.commits {
        let event = world
            .log
            .events
            .iter()
            .find(|e| e.id == commit.event_id)
            .unwrap();
        assert_eq!(commit.valid_time_ms, event.valid_time_ms);
        assert_eq!(commit.observation_time_ms, event.observation_time_ms);
    }

    // Tool spans attach only to their own session's message.
    let other_session = message_event("session-1", 0, EPOCH_MS, "user", "elsewhere");
    let mut foreign_span = message_event("session-1", 1, EPOCH_MS, "user", "x");
    foreign_span.payload = Payload::ToolSpan {
        message_id: "session-0-m0".to_string(),
        call_id: "stray".to_string(),
        output: "stray".to_string(),
    };
    let home = message_event("session-0", 0, EPOCH_MS, "user", "home");
    let rendering = render(&log(vec![home, other_session, foreign_span]), &config()).unwrap();
    assert!(
        rendering.messages.iter().all(|m| m.expected.len() == 1),
        "{rendering:?}"
    );
}

#[test]
fn render_refuses_bad_targets_roles_times_and_unencodable_identities_by_event() {
    let commit = Event {
        id: EventId::derive(StreamLabel::Repository, "repo-0", 0),
        stream: StreamLabel::Repository,
        entity_id: "repo-0".to_string(),
        local_seq: 0,
        valid_time_ms: EPOCH_MS,
        observation_time_ms: EPOCH_MS,
        causal_depth: 0,
        payload: Payload::Commit {
            oid: "a".repeat(40),
            message: "init".to_string(),
        },
    };
    let mut correction = message_event("session-0", 0, EPOCH_MS + 1, "user", "fix");
    correction.payload = Payload::Correction {
        target: commit.id.clone(),
        text: "fix".to_string(),
    };
    assert_eq!(
        render(&log(vec![commit.clone(), correction.clone()]), &config()).unwrap_err(),
        RenderError::CorrectionTargetIsNotAMessage(commit.id.clone())
    );
    let missing = EventId("session:session-9:9".to_string());
    let mut orphan = correction.clone();
    orphan.payload = Payload::Correction {
        target: missing.clone(),
        text: "fix".to_string(),
    };
    assert_eq!(
        render(&log(vec![orphan]), &config()).unwrap_err(),
        RenderError::CorrectionTargetMissing(missing)
    );

    // A correction stays in its target's session and advances its valid time;
    // otherwise it would mint a fresh lineage or the target's own occurrence.
    let target = message_event("session-0", 0, EPOCH_MS, "user", "original");
    let mut foreign = message_event("session-1", 0, EPOCH_MS + 1, "user", "fix");
    foreign.payload = Payload::Correction {
        target: target.id.clone(),
        text: "fix".to_string(),
    };
    assert_eq!(
        render(&log(vec![target.clone(), foreign]), &config()).unwrap_err(),
        RenderError::CorrectionTargetInOtherSession(target.id.clone())
    );
    let mut same_time = message_event("session-0", 1, EPOCH_MS, "user", "fix");
    same_time.payload = Payload::Correction {
        target: target.id.clone(),
        text: "fix".to_string(),
    };
    assert_eq!(
        render(&log(vec![target.clone(), same_time]), &config()).unwrap_err(),
        RenderError::CorrectionDoesNotAdvance(target.id.clone())
    );
    let mut backdated = message_event("session-0", 1, EPOCH_MS - 1, "user", "fix");
    backdated.payload = Payload::Correction {
        target: target.id.clone(),
        text: "fix".to_string(),
    };
    assert_eq!(
        render(&log(vec![target.clone(), backdated]), &config()).unwrap_err(),
        RenderError::CorrectionDoesNotAdvance(target.id.clone())
    );
    let mut advancing = message_event("session-0", 1, EPOCH_MS + 1, "user", "fix");
    advancing.payload = Payload::Correction {
        target: target.id.clone(),
        text: "fix".to_string(),
    };
    let rendered = render(&log(vec![target.clone(), advancing]), &config()).unwrap();
    assert_eq!(
        rendered.messages[1].expected[0].identity.lineage_id,
        rendered.messages[0].expected[0].identity.lineage_id
    );

    let system = message_event("session-0", 0, EPOCH_MS, "system", "x");
    assert_eq!(
        render(&log(vec![system.clone()]), &config()).unwrap_err(),
        RenderError::UnknownRole {
            event_id: system.id.clone(),
            role: "system".to_string(),
        }
    );
    let at_zero = message_event("session-0", 0, 0, "assistant", "x");
    assert_eq!(
        render(&log(vec![at_zero.clone()]), &config()).unwrap_err(),
        RenderError::NoEarlierCreated(at_zero.id.clone())
    );
    let at_one = message_event("session-0", 0, 1, "assistant", "x");
    assert_eq!(
        render(&log(vec![at_one]), &config()).unwrap().messages[0].message["info"]["time"],
        json!({"created": 0, "completed": 1})
    );

    let mut bad = config();
    bad.project_id = "tab\tin\tproject".to_string();
    let user = message_event("session-0", 0, EPOCH_MS, "user", "x");
    assert_eq!(
        render(&log(vec![user.clone()]), &bad).unwrap_err(),
        RenderError::Encoding {
            event_id: user.id.clone(),
            refusal: EncodingRefusal::MalformedIdentityValue,
        }
    );
    assert_eq!(
        git_identity(&config(), "zz").unwrap_err(),
        EncodingRefusal::MalformedOid
    );
    bad.repository_id = "tab\tin\trepo".to_string();
    assert_eq!(
        git_identity(&bad, "zz").unwrap_err(),
        EncodingRefusal::MalformedIdentityValue,
        "identity values are checked before the oid format"
    );
}

/// Identity derives from class, native identity, revision, representation,
/// and span: each identity field and the revision change the occurrence, the
/// representation and span change both ids, the payload changes neither.
#[test]
fn the_identity_flip_matrix_names_what_enters_each_id() {
    let base_identity = [
        ("project_id", "proj-a"),
        ("harness", "opencode"),
        ("session_id", "sess-01"),
        ("message_id", "msg-001"),
        ("block_index", "0"),
    ];
    let base = encode(&Occurrence {
        class: "messages",
        identity: &base_identity,
        revision: "1700000000000",
        representation: "text",
        span: None,
    })
    .unwrap();
    for (index, (name, _)) in base_identity.iter().enumerate() {
        let mut flipped = base_identity;
        flipped[index].1 = match *name {
            "harness" => "pi",
            "block_index" => "1",
            _ => "other",
        };
        let other = encode(&Occurrence {
            class: "messages",
            identity: &flipped,
            revision: "1700000000000",
            representation: "text",
            span: None,
        })
        .unwrap();
        assert_ne!(other.occurrence_id, base.occurrence_id, "{name}");
        assert_ne!(other.lineage_id, base.lineage_id, "{name}");
    }
    let later = encode(&Occurrence {
        class: "messages",
        identity: &base_identity,
        revision: "1700000000001",
        representation: "text",
        span: None,
    })
    .unwrap();
    assert_ne!(later.occurrence_id, base.occurrence_id);
    assert_eq!(
        later.lineage_id, base.lineage_id,
        "revision is not in the lineage"
    );
    let tool_identity = [
        ("project_id", "proj-a"),
        ("harness", "opencode"),
        ("session_id", "sess-01"),
        ("parent_message_id", "msg-001"),
        ("tool_call_id", "call-1"),
        ("result_revision", "5"),
        ("block_index", "0"),
    ];
    let whole = |representation: &str, span: Option<Span>| {
        encode(&Occurrence {
            class: "raw_tool_spans",
            identity: &tool_identity,
            revision: "5",
            representation,
            span,
        })
        .unwrap()
    };
    let output = whole("tool_output", None);
    let error = whole("tool_error", None);
    assert_ne!(output.occurrence_id, error.occurrence_id);
    assert_ne!(
        output.lineage_id, error.lineage_id,
        "representation is in the lineage"
    );
    let spanned = whole("tool_output", Some(Span { start: 0, end: 13 }));
    assert_ne!(spanned.occurrence_id, output.occurrence_id);
    assert_ne!(
        spanned.lineage_id, output.lineage_id,
        "span is in the lineage"
    );
    assert_ne!(output.occurrence_id, output.lineage_id);
    // Payload bytes and role are not inputs: the same tuple is one identity.
    assert_eq!(whole("tool_output", None), output);
    assert_eq!(
        OccurrenceClass::from_code("messages"),
        Some(OccurrenceClass::Messages)
    );
    assert_eq!(OccurrenceClass::from_code("mail"), None);
    let refusals: Vec<EncodingRefusal> = [
        Occurrence {
            class: "mail",
            identity: &base_identity,
            revision: "1",
            representation: "text",
            span: None,
        },
        Occurrence {
            class: "messages",
            identity: &base_identity[..4],
            revision: "1",
            representation: "text",
            span: None,
        },
        Occurrence {
            class: "messages",
            identity: &base_identity,
            revision: "01",
            representation: "text",
            span: None,
        },
        Occurrence {
            class: "messages",
            identity: &base_identity,
            revision: "",
            representation: "text",
            span: None,
        },
        Occurrence {
            class: "messages",
            identity: &base_identity,
            revision: "1",
            representation: "summary",
            span: None,
        },
    ]
    .iter()
    .map(|occurrence| encode(occurrence).unwrap_err())
    .collect();
    assert_eq!(
        refusals,
        [
            EncodingRefusal::UnknownClass,
            EncodingRefusal::MissingIdentityField,
            EncodingRefusal::MalformedRevision,
            EncodingRefusal::MissingRevision,
            EncodingRefusal::UnknownRepresentation,
        ]
    );
}

#[test]
fn accounting_names_every_way_a_unit_can_go_missing() {
    let set = |ids: &[&str]| -> BTreeSet<String> { ids.iter().map(|s| s.to_string()).collect() };
    check_accounting(&set(&[]), &set(&[]), &set(&[])).unwrap();
    check_accounting(&set(&["a", "b"]), &set(&["a"]), &set(&["b"])).unwrap();
    assert_eq!(
        check_accounting(&set(&["a", "b"]), &set(&["a"]), &set(&[])),
        Err(AccountingError::Missing(set(&["b"])))
    );
    assert_eq!(
        check_accounting(&set(&["a"]), &set(&["a", "z"]), &set(&[])),
        Err(AccountingError::Unexpected(set(&["z"])))
    );
    assert_eq!(
        check_accounting(&set(&["a"]), &set(&["a"]), &set(&["a"])),
        Err(AccountingError::PublishedAndRefused(set(&["a"])))
    );
}

#[test]
fn the_marker_registry_is_unique_and_incomplete_until_every_marker_fires() {
    let names: BTreeSet<&str> = MARKERS.iter().map(|m| m.name).collect();
    assert_eq!(names.len(), MARKERS.len());
    assert!(MARKERS.iter().all(|m| {
        m.name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    }));
    let mut coverage = Coverage::default();
    assert_eq!(
        coverage.record("wm_unregistered"),
        Err(CoverageError::Unregistered("wm_unregistered".to_string()))
    );
    for marker in &MARKERS[..MARKERS.len() - 1] {
        coverage.record(marker.name).unwrap();
    }
    assert_eq!(
        coverage.complete(),
        Err(CoverageError::Incomplete {
            missing: BTreeSet::from([MARKERS[MARKERS.len() - 1].name]),
        })
    );
    coverage.record(MARKERS[MARKERS.len() - 1].name).unwrap();
    coverage.complete().unwrap();
    assert_eq!(coverage.fired().len(), MARKERS.len());
}

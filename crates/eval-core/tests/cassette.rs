use std::collections::{BTreeMap, BTreeSet};

use context_core::canonical_json::ContractError;
use context_core::redaction::RedactionErrorKind;
use eval_core::{
    BACKEND_COVERED_FIELDS, BackendRecord, Boundary, CASSETTE_GENERATOR_VERSION, CASSETTE_SCHEMA,
    COVERED_FIELDS_VERSION, Cassette, CassetteError, CassetteFile, CassetteMiss, Location, Lookup,
    MissClass, OPENCODE_COVERED_FIELDS, OPENCODE_HEADER_ALLOWLIST, OpenCodeRequest,
    canonical_decimal_f64, request_digest,
};
use serde_json::{Value, json};

const NAMESPACE: &str = "eval-run:fresh:0";
const OTHER_NAMESPACE: &str = "eval-run:aged:0";
const AWS_CANARY: &str = "aws_access_key_id = AKIAQ7R3XM2ZT5WN6PBC";
const ANTHROPIC_CANARY: &str = "sk-ant-api03-Zk9Qw2Lm7Pv4Rt8Zw1Yc6Nb3Hd5Kf0JgAbCdEfGh";

/// Frozen so an encoding change to the covered projection is reviewed.
const FIXTURE_REQUEST_DIGEST: &str =
    "f07cbe5d14c0ad37783d4c63df7dd3ea8b9fca4d4d3b5810cfb0dbc3b7b56fb5";

type Mutation = (&'static str, Box<dyn Fn(&mut OpenCodeRequest)>);

/// A representative OpenCode 1.18.31 provider request: credential and
/// connection headers present, `cache_control` markers on two blocks, and a
/// tool result inside a user message.
fn opencode_request() -> OpenCodeRequest {
    let headers: BTreeMap<String, String> = [
        ("accept", "*/*"),
        ("anthropic-version", "2023-06-01"),
        ("authorization", "Bearer canary-authorization-token"),
        ("content-length", "66656"),
        ("content-type", "application/json"),
        ("host", "127.0.0.1:42047"),
        (
            "user-agent",
            "opencode/1.18.31 ai-sdk/provider-utils/4.0.46 runtime/bun/1.3.14",
        ),
        ("x-api-key", "test-key-not-real"),
        ("x-session-id", "ses_f3ebdec64ffe6gpQ1gJdeF4nsF"),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value.to_string()))
    .collect();
    OpenCodeRequest {
        path: "/messages".to_string(),
        headers,
        body: json!({
            "model": "mock-sonnet",
            "max_tokens": 8192,
            "stream": true,
            "tool_choice": {"type": "auto"},
            "system": [{"type": "text", "text": "You are opencode.", "cache_control": {"type": "ephemeral"}}],
            "tools": [{"name": "glob", "description": "Find files", "input_schema": {"type": "object"}}],
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "Use the tool once."}]},
                {"role": "assistant", "content": [{"type": "tool_use", "id": "toolu_1", "name": "glob", "input": {"pattern": "*.md"}, "cache_control": {"type": "ephemeral"}}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "No files found"}]}
            ]
        }),
    }
}

fn frames() -> Value {
    json!({
        "status": 200,
        "content_type": "text/event-stream",
        "frames": [
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_01\",\"usage\":{\"input_tokens\":100,\"output_tokens\":0}}}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":20}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
        ],
        "aborted": false
    })
}

fn title_request() -> OpenCodeRequest {
    let mut title = opencode_request();
    title.body["system"][0]["text"] = json!("Write a title.");
    title
}

fn recorded(requests: &[OpenCodeRequest]) -> Value {
    let mut cassette = Cassette::recording(NAMESPACE, Value::Null).unwrap();
    for request in requests {
        cassette
            .record(
                NAMESPACE,
                Boundary::Opencode,
                request.covered().unwrap(),
                frames(),
            )
            .unwrap();
    }
    serde_json::to_value(cassette.to_file().unwrap()).unwrap()
}

/// Re-signs a hand-edited file so only the edit under test is visible.
fn resigned(mut file: Value) -> Value {
    let parsed: CassetteFile = serde_json::from_value(file.clone()).unwrap();
    file["provenance"]["input_sha256"] = json!(parsed.input_sha256().unwrap());
    file
}

fn hit(cassette: &mut Cassette, request: &OpenCodeRequest) -> Value {
    match cassette
        .lookup(NAMESPACE, Boundary::Opencode, &request.covered().unwrap())
        .unwrap()
    {
        Lookup::Hit(entry) => entry.response.clone(),
        Lookup::Miss(miss) => panic!("miss: {miss:?}"),
    }
}

fn miss_of(cassette: &mut Cassette, request: &OpenCodeRequest) -> CassetteMiss {
    match cassette
        .lookup(NAMESPACE, Boundary::Opencode, &request.covered().unwrap())
        .unwrap()
    {
        Lookup::Miss(miss) => miss,
        Lookup::Hit(entry) => panic!("hit: {}", entry.request_digest),
    }
}

#[test]
fn covered_field_lists_are_sorted_pinned_and_closed_over_the_observed_body() {
    for fields in [&OPENCODE_COVERED_FIELDS[..], &BACKEND_COVERED_FIELDS[..]] {
        let mut sorted = fields.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, fields);
    }
    for header in OPENCODE_HEADER_ALLOWLIST {
        assert!(OPENCODE_COVERED_FIELDS.contains(&format!("headers.{header}").as_str()));
    }
    let raw: BTreeSet<String> = opencode_request()
        .body
        .as_object()
        .unwrap()
        .keys()
        .map(|key| format!("body.{key}"))
        .collect();
    let covered: BTreeSet<String> = OPENCODE_COVERED_FIELDS
        .iter()
        .filter(|field| field.starts_with("body."))
        .map(|field| field.to_string())
        .collect();
    let observed_but_optional: BTreeSet<String> = ["body.temperature".to_string()].into();
    assert_eq!(
        raw.union(&observed_but_optional)
            .cloned()
            .collect::<BTreeSet<_>>(),
        covered,
        "every observed body field is covered and nothing else is"
    );
    let projection = opencode_request().covered().unwrap();
    assert_eq!(request_digest(&projection).unwrap(), FIXTURE_REQUEST_DIGEST);
    assert_eq!(
        projection["headers"],
        json!({"anthropic-version": "2023-06-01"})
    );
    let mut widened = opencode_request();
    widened.body["metadata"] = json!({"user_id": "u1"});
    assert_eq!(
        widened.covered().err(),
        Some(CassetteError::UnknownRequestField("metadata".to_string()))
    );
}

#[test]
fn a_recorded_request_replays_and_no_credential_header_enters_the_file() {
    let file = recorded(&[opencode_request()]);
    let text = serde_json::to_string(&file).unwrap();
    for absent in [
        "authorization",
        "canary-authorization-token",
        "x-api-key",
        "test-key-not-real",
        "x-session-id",
        "user-agent",
    ] {
        assert!(!text.contains(absent), "{absent} persisted");
    }
    assert_eq!(file["schema"], json!(CASSETTE_SCHEMA));
    assert_eq!(
        file["covered_fields_version"],
        json!(COVERED_FIELDS_VERSION)
    );
    assert_eq!(
        file["provenance"]["generator_version"],
        json!(CASSETTE_GENERATOR_VERSION)
    );
    let mut cassette = Cassette::replay(&file, NAMESPACE).unwrap();
    assert_eq!(hit(&mut cassette, &opencode_request()), frames());
    assert_eq!((cassette.misses(), cassette.unconsumed()), (0, 0));
    // A request past the recording is a miss whose nearest entry is the last one.
    let miss = miss_of(&mut cassette, &opencode_request());
    assert_eq!(miss.turn, 1);
    assert_eq!(miss.class, MissClass::ModelRequestChanged);
    assert_eq!(
        miss.nearest_recorded,
        Some(FIXTURE_REQUEST_DIGEST.to_string())
    );
}

#[test]
fn one_byte_in_any_covered_field_misses_at_the_turn_and_terminates_the_cassette() {
    let file = recorded(&[opencode_request(), opencode_request()]);
    let mutations: Vec<Mutation> = vec![
        ("path", Box::new(|r| r.path.push('x'))),
        (
            "body.model",
            Box::new(|r| r.body["model"] = json!("mock-sonnex")),
        ),
        (
            "body.max_tokens",
            Box::new(|r| r.body["max_tokens"] = json!(8193)),
        ),
        ("body.stream", Box::new(|r| r.body["stream"] = json!(false))),
        (
            "body.tool_choice",
            Box::new(|r| r.body["tool_choice"]["type"] = json!("any")),
        ),
        (
            "body.system",
            Box::new(|r| r.body["system"][0]["text"] = json!("You are opencodf.")),
        ),
        (
            "body.tools",
            Box::new(|r| r.body["tools"][0]["name"] = json!("glib")),
        ),
        (
            "body.messages",
            Box::new(|r| r.body["messages"][0]["content"][0]["text"] = json!("Use the tool oncf.")),
        ),
        (
            "body.temperature",
            Box::new(|r| r.body["temperature"] = json!(0.7)),
        ),
        (
            "headers.anthropic-version",
            Box::new(|r| {
                r.headers
                    .insert("anthropic-version".to_string(), "2023-06-02".to_string());
            }),
        ),
        (
            "headers.anthropic-beta",
            Box::new(|r| {
                r.headers
                    .insert("anthropic-beta".to_string(), "x".to_string());
            }),
        ),
    ];
    let mut covered: BTreeSet<&str> = OPENCODE_COVERED_FIELDS.into_iter().collect();
    for (field, mutate) in mutations {
        assert!(covered.remove(field), "{field} mutated twice");
        let mut cassette = Cassette::replay(&file, NAMESPACE).unwrap();
        hit(&mut cassette, &opencode_request());
        let mut changed = opencode_request();
        mutate(&mut changed);
        let miss = miss_of(&mut cassette, &changed);
        assert_eq!(miss.turn, 1, "{field}");
        assert_eq!(miss.class, MissClass::ModelRequestChanged, "{field}");
        assert_eq!(
            miss.nearest_recorded,
            Some(FIXTURE_REQUEST_DIGEST.to_string())
        );
        assert_ne!(miss.request_digest, FIXTURE_REQUEST_DIGEST);
        // The run stops here: the unchanged request is refused with the same terminal.
        assert_eq!(miss_of(&mut cassette, &opencode_request()), miss);
        assert_eq!(cassette.terminal(), Some(&miss));
        assert_eq!(cassette.misses(), 1);
    }
    assert!(covered.is_empty(), "uncovered: {covered:?}");
}

#[test]
fn a_fractional_temperature_digests_exactly() {
    let mut warm = opencode_request();
    warm.body["temperature"] = json!(0.7);
    let file = recorded(&[warm.clone()]);
    assert_eq!(
        file["cases"][0]["request"]["body"]["temperature"],
        json!("0.7")
    );
    let mut same = opencode_request();
    same.body["temperature"] = json!(0.70);
    let mut cassette = Cassette::replay(&file, NAMESPACE).unwrap();
    hit(&mut cassette, &same);
    let mut hotter = opencode_request();
    hotter.body["temperature"] = json!(0.8);
    assert_ne!(
        request_digest(&hotter.covered().unwrap()).unwrap(),
        request_digest(&warm.covered().unwrap()).unwrap()
    );
}

#[test]
fn equal_digests_replay_in_recorded_order_and_distinct_ones_in_any_order() {
    let title = title_request();
    let file = recorded(&[opencode_request(), title.clone(), opencode_request()]);
    let mut cassette = Cassette::replay(&file, NAMESPACE).unwrap();
    assert_eq!(cassette.unconsumed(), 3);
    hit(&mut cassette, &title);
    hit(&mut cassette, &opencode_request());
    hit(&mut cassette, &opencode_request());
    assert_eq!(cassette.unconsumed(), 0);
    let miss = miss_of(&mut cassette, &opencode_request());
    assert_eq!(miss.turn, 3);
    assert_eq!(
        miss.nearest_recorded,
        Some(FIXTURE_REQUEST_DIGEST.to_string())
    );
}

#[test]
fn the_nearest_entry_is_the_first_unconsumed_one_of_the_same_boundary() {
    let title = title_request();
    let title_digest = request_digest(&title.covered().unwrap()).unwrap();
    let file = recorded(&[opencode_request(), title.clone(), opencode_request()]);
    let mut cassette = Cassette::replay(&file, NAMESPACE).unwrap();
    hit(&mut cassette, &opencode_request());
    let mut unknown = opencode_request();
    unknown.body["model"] = json!("other");
    let miss = miss_of(&mut cassette, &unknown);
    assert_eq!(miss.nearest_recorded, Some(title_digest));
    assert_eq!(cassette.unconsumed(), 2, "a miss consumes nothing");

    // A backend lookup never answers from OpenCode entries, and names none as nearest.
    let mut fresh = Cassette::replay(&file, NAMESPACE).unwrap();
    let Lookup::Miss(miss) = fresh
        .lookup(
            NAMESPACE,
            Boundary::Backend,
            &opencode_request().covered().unwrap(),
        )
        .unwrap()
    else {
        panic!("cross-boundary hit");
    };
    assert_eq!(miss.nearest_recorded, None);
}

#[test]
fn only_a_tool_result_change_is_tool_result_drift() {
    let file = recorded(&[opencode_request()]);
    let mut cassette = Cassette::replay(&file, NAMESPACE).unwrap();
    let mut drifted = opencode_request();
    drifted.body["messages"][2]["content"][0]["content"] = json!("One file found");
    let miss = miss_of(&mut cassette, &drifted);
    assert_eq!(miss.turn, 0);
    assert_eq!(miss.class, MissClass::ToolResultDrift);
}

#[test]
fn volatile_and_uncovered_changes_replay() {
    let file = recorded(&[opencode_request()]);
    let mut moved = opencode_request();
    moved.body["system"][0]
        .as_object_mut()
        .unwrap()
        .remove("cache_control");
    moved.body["messages"][2]["content"][0]["cache_control"] = json!({"type": "ephemeral"});
    moved
        .headers
        .insert("x-api-key".to_string(), "sk-other".to_string());
    moved
        .headers
        .insert("user-agent".to_string(), "opencode/9".to_string());
    moved.headers.remove("x-session-id");
    moved.headers.remove("authorization");
    let mut cassette = Cassette::replay(&file, NAMESPACE).unwrap();
    hit(&mut cassette, &moved);
}

#[test]
fn only_a_terminated_nonce_is_normalized() {
    let digest_of = |text: &str| {
        let mut request = opencode_request();
        request.body["system"][0]["text"] = json!(text);
        request_digest(&request.covered().unwrap()).unwrap()
    };
    let same = [
        ("a cch=1111; b", "a cch=2222; b"),
        ("cch=x-1_2;", "cch=Y9;"),
        ("cch=1; and cch=2;", "cch=3; and cch=4;"),
    ];
    for (left, right) in same {
        assert_eq!(digest_of(left), digest_of(right), "{left} vs {right}");
    }
    let different = [
        ("a cch=1111 b", "a cch=2222 b"),
        (
            "see https://e.com/?cch=1&q=one",
            "see https://e.com/?cch=1&q=two",
        ),
        ("cch=; x", "cch=; y"),
        ("a cch=1;", "a cch=1; tail"),
    ];
    for (left, right) in different {
        assert_ne!(digest_of(left), digest_of(right), "{left} vs {right}");
    }
    assert_eq!(
        opencode_request().covered().unwrap()["body"]["system"][0]["text"],
        json!("You are opencode.")
    );
}

#[test]
fn namespaces_bind_the_cassette_and_equal_digests_elsewhere_refuse() {
    let file = recorded(&[opencode_request()]);
    assert_eq!(
        Cassette::replay(&file, OTHER_NAMESPACE).err(),
        Some(CassetteError::NamespaceMismatch {
            recorded: NAMESPACE.to_string(),
            expected: OTHER_NAMESPACE.to_string(),
        })
    );
    let mut cassette = Cassette::replay(&file, NAMESPACE).unwrap();
    let covered = opencode_request().covered().unwrap();
    assert_eq!(
        cassette
            .lookup(OTHER_NAMESPACE, Boundary::Opencode, &covered)
            .err(),
        Some(CassetteError::WrongNamespace {
            recorded: NAMESPACE.to_string(),
            offered: OTHER_NAMESPACE.to_string(),
        })
    );
    assert_eq!(
        cassette
            .record(NAMESPACE, Boundary::Opencode, json!({}), json!({}))
            .err(),
        Some(CassetteError::RecordOnReplay)
    );
    let mut recording = Cassette::recording(NAMESPACE, Value::Null).unwrap();
    assert!(matches!(
        recording.record(OTHER_NAMESPACE, Boundary::Opencode, json!({}), json!({})),
        Err(CassetteError::WrongNamespace { .. })
    ));
    assert_eq!(
        recording
            .lookup(NAMESPACE, Boundary::Opencode, &covered)
            .err(),
        Some(CassetteError::LookupOnRecord)
    );
}

#[test]
fn provenance_schema_and_version_pins_are_recomputed_on_read() {
    let file = recorded(&[opencode_request()]);
    let mut regenerated = file.clone();
    regenerated["cases"][0]["response"]["frames"][2] =
        json!("event: message_stop\ndata: {\"type\":\"message_stop\"} \n\n");
    assert!(matches!(
        Cassette::replay(&regenerated, NAMESPACE),
        Err(CassetteError::ProvenanceMismatch { .. })
    ));
    let mut declared = file.clone();
    declared["declarations"] = json!({"opencode": {"suppression": false}});
    assert!(matches!(
        Cassette::replay(&declared, NAMESPACE),
        Err(CassetteError::ProvenanceMismatch { .. })
    ));
    assert_eq!(
        Cassette::replay(&resigned(declared), NAMESPACE)
            .unwrap()
            .declarations(),
        &json!({"opencode": {"suppression": false}})
    );
    let mut rebound = file.clone();
    rebound["cases"][0]["request"]["body"]["model"] = json!("other");
    assert_eq!(
        Cassette::replay(&resigned(rebound), NAMESPACE).err(),
        Some(CassetteError::EntryDigestMismatch { index: 0 })
    );
    let mut schema = file.clone();
    schema["schema"] = json!("eval-cassette/v2");
    assert!(matches!(
        Cassette::replay(&schema, NAMESPACE),
        Err(CassetteError::SchemaMismatch { .. })
    ));
    let mut generator = file.clone();
    generator["provenance"]["generator_version"] = json!("eval-cassette-ts-v1");
    assert!(matches!(
        Cassette::replay(&generator, NAMESPACE),
        Err(CassetteError::GeneratorVersionMismatch { .. })
    ));
    let mut covered = file.clone();
    covered["covered_fields_version"] = json!("eval-cassette-covered/v0");
    assert!(matches!(
        Cassette::replay(&covered, NAMESPACE),
        Err(CassetteError::CoveredFieldsMismatch { .. })
    ));
    let mut extra = file;
    extra["extra"] = json!(1);
    assert!(matches!(
        Cassette::replay(&extra, NAMESPACE),
        Err(CassetteError::Shape(_))
    ));
}

#[test]
fn planted_secrets_and_unscannable_frames_are_refused_and_the_cassette_never_persists() {
    let refusal = |request: OpenCodeRequest, response: Value| {
        let mut cassette = Cassette::recording(NAMESPACE, Value::Null).unwrap();
        cassette
            .record(
                NAMESPACE,
                Boundary::Opencode,
                opencode_request().covered().unwrap(),
                frames(),
            )
            .unwrap();
        let error = cassette
            .record(
                NAMESPACE,
                Boundary::Opencode,
                request.covered().unwrap(),
                response,
            )
            .unwrap_err();
        assert_eq!(cassette.cases().len(), 1, "a refused entry never exists");
        // One admitted entry does not make a partial cassette persistable.
        assert_eq!(cassette.to_file().err(), Some(error.clone()));
        error
    };
    let mut body_token = opencode_request();
    body_token.body["messages"][0]["content"][0]["text"] =
        json!(format!("my key is {ANTHROPIC_CANARY}"));
    assert_eq!(
        refusal(body_token, frames()),
        CassetteError::RedactionRefused(Location::Request, RedactionErrorKind::SecretDetected)
    );
    let mut tool_output = opencode_request();
    tool_output.body["messages"][2]["content"][0]["content"] = json!(AWS_CANARY);
    assert_eq!(
        refusal(tool_output, frames()),
        CassetteError::RedactionRefused(Location::Request, RedactionErrorKind::SecretDetected)
    );
    let mut response = frames();
    response["frames"][0] = json!(format!("data: {{\"text\":\"{AWS_CANARY}\"}}\n\n"));
    assert_eq!(
        refusal(opencode_request(), response),
        CassetteError::RedactionRefused(Location::Response, RedactionErrorKind::SecretDetected)
    );
    let mut oversized = opencode_request();
    oversized.body["messages"][0]["content"][0]["text"] =
        json!("x".repeat(context_core::redaction::MAX_REDACTABLE_BYTES + 1));
    assert_eq!(
        refusal(oversized, frames()),
        CassetteError::RedactionRefused(Location::Request, RedactionErrorKind::InputLimit)
    );
    let text = serde_json::to_string(&recorded(&[opencode_request()])).unwrap();
    assert!(!text.contains("AKIA") && !text.contains("sk-ant-"));
}

#[test]
fn a_malformed_body_is_refused_rather_than_digested_as_empty() {
    let headers = BTreeMap::new();
    assert_eq!(
        OpenCodeRequest::from_raw("/messages", headers.clone(), "{not json").err(),
        Some(CassetteError::MalformedBody)
    );
    assert_eq!(
        OpenCodeRequest::from_raw("/messages", headers.clone(), "[]").err(),
        Some(CassetteError::MalformedBody)
    );
    let parsed = OpenCodeRequest::from_raw("/messages", headers, "{\"model\":\"m\"}").unwrap();
    assert_eq!(parsed.body, json!({"model": "m"}));
}

#[test]
fn backend_records_cover_the_pinned_fields_with_exact_temperatures() {
    let record = |temperature: f64| BackendRecord {
        prompt: "p".to_string(),
        system: None,
        provider: "anthropic".to_string(),
        model: "m".to_string(),
        max_output_tokens: 64,
        temperature: Some(canonical_decimal_f64(temperature).unwrap()),
        harness: "opencode".to_string(),
    };
    let fields: BTreeSet<String> = record(0.7)
        .covered()
        .unwrap()
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    let pinned: BTreeSet<String> = BACKEND_COVERED_FIELDS
        .iter()
        .map(|f| f.to_string())
        .collect();
    assert_eq!(fields, pinned);
    let digest = |temperature| request_digest(&record(temperature).covered().unwrap()).unwrap();
    assert_eq!(digest(0.7), digest(0.70));
    assert_ne!(digest(0.7), digest(0.8));
    assert_eq!(canonical_decimal_f64(1.0).unwrap(), "1");
    assert_eq!(canonical_decimal_f64(0.0000001).unwrap(), "0.0000001");
    for rejected in [f64::NAN, f64::INFINITY, -0.0, -1.0] {
        assert!(matches!(
            canonical_decimal_f64(rejected),
            Err(CassetteError::TemperatureNotDecimal(_))
        ));
    }
    let mut hand_written = record(0.7);
    hand_written.temperature = Some("0.70".to_string());
    assert!(matches!(
        hand_written.covered(),
        Err(CassetteError::TemperatureNotDecimal(_))
    ));
    let mut native = record(0.7);
    native.temperature = None;
    assert_eq!(native.covered().unwrap()["temperature"], Value::Null);
    assert_ne!(
        request_digest(&native.covered().unwrap()).unwrap(),
        digest(0.7)
    );
}

/// Stands in for request-derived content in an error payload.
const CANARY: &str = "CANARY-request-content";

fn request_shaped_errors() -> Vec<CassetteError> {
    vec![
        CassetteError::Shape(format!("invalid type: {CANARY}")),
        CassetteError::MalformedBody,
        CassetteError::UnknownRequestField(CANARY.to_string()),
        CassetteError::TemperatureNotDecimal(CANARY.to_string()),
        CassetteError::NotCanonical(ContractError::NotCanonical(format!(
            "number {CANARY} is not a safe integer"
        ))),
    ]
}

/// Each error paired with the oracle-owned values its detail must show.
fn oracle_owned_errors() -> Vec<(CassetteError, Vec<&'static str>)> {
    vec![
        (
            CassetteError::SchemaMismatch {
                found: "eval-cassette/v9".to_string(),
            },
            vec!["eval-cassette/v9"],
        ),
        (
            CassetteError::GeneratorVersionMismatch {
                found: "eval-cassette-ts-v1".to_string(),
            },
            vec!["eval-cassette-ts-v1"],
        ),
        (
            CassetteError::CoveredFieldsMismatch {
                found: "eval-cassette-covered/v0".to_string(),
            },
            vec!["eval-cassette-covered/v0"],
        ),
        (
            CassetteError::NamespaceMismatch {
                recorded: NAMESPACE.to_string(),
                expected: OTHER_NAMESPACE.to_string(),
            },
            vec![NAMESPACE, OTHER_NAMESPACE],
        ),
        (
            CassetteError::WrongNamespace {
                recorded: NAMESPACE.to_string(),
                offered: OTHER_NAMESPACE.to_string(),
            },
            vec![NAMESPACE, OTHER_NAMESPACE],
        ),
        (
            CassetteError::ProvenanceMismatch {
                recorded: "aa".repeat(32),
                computed: FIXTURE_REQUEST_DIGEST.to_string(),
            },
            vec![FIXTURE_REQUEST_DIGEST],
        ),
        (CassetteError::EntryDigestMismatch { index: 7 }, vec!["7"]),
        (
            CassetteError::RedactionRefused(Location::Response, RedactionErrorKind::InputLimit),
            vec!["Response", "InputLimit"],
        ),
        (
            CassetteError::ScannerUnavailable(RedactionErrorKind::Construction),
            vec!["Construction"],
        ),
        (CassetteError::RecordOnReplay, vec![]),
        (CassetteError::LookupOnRecord, vec![]),
    ]
}

#[test]
fn every_error_names_its_wire_kind() {
    let kinds: BTreeSet<&str> = request_shaped_errors()
        .iter()
        .chain(oracle_owned_errors().iter().map(|(error, _)| error))
        .map(CassetteError::kind)
        .collect();
    assert_eq!(kinds.len(), 16, "kinds are distinct");
    assert!(kinds.contains("RedactionRefused"));
    assert!(kinds.contains("NotCanonical"));
}

#[test]
fn no_wire_detail_carries_request_content() {
    for error in request_shaped_errors() {
        assert_eq!(error.detail(), "", "{}: detail must be empty", error.kind());
        assert!(
            error.to_string().contains(CANARY) || error == CassetteError::MalformedBody,
            "{}: the fixture carries the canary through Display",
            error.kind()
        );
    }
    for (error, visible) in oracle_owned_errors() {
        let detail = error.detail();
        assert!(!detail.contains(CANARY), "{}: {detail}", error.kind());
        for value in visible {
            assert!(
                detail.contains(value),
                "{}: detail {detail:?} lacks {value:?}",
                error.kind()
            );
        }
    }
}

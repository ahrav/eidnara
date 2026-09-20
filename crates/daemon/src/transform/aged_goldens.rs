use context_core::canonical_json::protocol_digest;
use serde::Deserialize;
use serde_json::{Value, json};

use super::tests::{active_cc_req, comp, run, spine, store, tail_bytes, wire_item};
use super::*;

const GOLDEN: &str = include_str!("../../testdata/aged-transform-golden.json");
const INPUT_PROTOCOL: &str = "eidnara-aged-transform-golden-v1";
const WIRE_PROTOCOL: &str = "eidnara-aged-transform-wire-v1";
const TAIL: &str = "tail";
const HINT_OPEN: &str = "<eidnara-search-hint>";

#[derive(Debug, Deserialize)]
struct Golden {
    schema: u32,
    provenance: Provenance,
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
struct Provenance {
    generator: String,
    generator_version: String,
    reachability: eval_core::Reachability,
    construction: eval_core::Construction,
    input_sha256: String,
}

#[derive(Debug, Deserialize)]
struct Case {
    id: String,
    input: Input,
    expected: Expected,
}

#[derive(Debug, Deserialize)]
struct Input {
    prompt: String,
    history: Vec<HistoryMessage>,
    segments: Vec<Segment>,
    aged_hint_expected: bool,
}

#[derive(Debug, Deserialize)]
struct HistoryMessage {
    id: String,
    text: String,
}

#[derive(Debug, Deserialize)]
struct Segment {
    sequence: i64,
    start: i64,
    end: i64,
    end_id: String,
    p1: String,
}

#[derive(Debug, Deserialize, PartialEq)]
struct Expected {
    fresh: WorldOutput,
    aged: WorldOutput,
}

#[derive(Debug, Deserialize, PartialEq)]
struct WorldOutput {
    tail: String,
    wire_digest: String,
}

fn raw() -> Value {
    serde_json::from_str(GOLDEN).unwrap()
}

fn golden() -> Golden {
    serde_json::from_value(raw()).unwrap()
}

fn input_digest(golden_value: &Value) -> String {
    let inputs = golden_value["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| json!({"id": case["id"], "input": case["input"]}))
        .collect::<Vec<_>>();
    protocol_digest(INPUT_PROTOCOL, &Value::Array(inputs)).unwrap()
}

fn world_output(response: &TransformResponse) -> WorldOutput {
    let wire = response
        .messages()
        .iter()
        .map(|message| serde_json::to_value(&**message).unwrap())
        .collect::<Vec<_>>();
    WorldOutput {
        tail: tail_bytes(response, TAIL).to_string(),
        wire_digest: protocol_digest(WIRE_PROTOCOL, &Value::Array(wire)).unwrap(),
    }
}

fn request(session: &str, messages: Vec<IngressMessage>) -> TransformRequest {
    let mut request = active_cc_req(session, "cfg0", messages);
    request.auto_search_min_prompt_chars = DEFAULT_AUTO_SEARCH_MIN_PROMPT_CHARS;
    request
}

fn run_fresh(case: &Case) -> WorldOutput {
    let dir = tempfile::tempdir().unwrap();
    let s = store(dir.path());
    let response = run(
        &s,
        &request(
            "fresh",
            vec![wire_item("user", TAIL, 1, &[&case.input.prompt])],
        ),
        &spine(),
    );
    world_output(&response)
}

fn run_aged(case: &Case, auto_search_enabled: bool) -> WorldOutput {
    let dir = tempfile::tempdir().unwrap();
    let s = store(dir.path());
    let mut messages = case
        .input
        .history
        .iter()
        .enumerate()
        .map(|(index, message)| wire_item("user", &message.id, index as u64 + 1, &[&message.text]))
        .collect::<Vec<_>>();
    messages.push(wire_item(
        "user",
        TAIL,
        case.input.history.len() as u64 + 1,
        &[&case.input.prompt],
    ));
    let segments = case
        .input
        .segments
        .iter()
        .map(|segment| {
            comp(
                segment.sequence,
                segment.start,
                segment.end,
                &segment.end_id,
                &segment.p1,
            )
        })
        .collect::<Vec<_>>();
    s.replace_history_segments("aged", &segments).unwrap();
    let mut request = request("aged", messages);
    request.auto_search_enabled = auto_search_enabled;
    world_output(&run(&s, &request, &spine()))
}

#[test]
fn aged_golden_provenance_covers_every_case_input() {
    let raw = raw();
    let golden = golden();
    assert_eq!(golden.schema, 1);
    assert_eq!(golden.provenance.generator, "hand-built");
    assert_eq!(
        golden.provenance.generator_version,
        "aged-transform-golden-v1"
    );
    assert_eq!(
        golden.provenance.reachability,
        eval_core::Reachability::DefaultProduction
    );
    assert_eq!(
        golden.provenance.construction,
        eval_core::Construction::HandBuilt
    );
    assert_eq!(golden.provenance.input_sha256, input_digest(&raw));

    let mut perturbed = raw.clone();
    let prompt = &mut perturbed["cases"][0]["input"]["prompt"];
    *prompt = Value::String(format!("{}x", prompt.as_str().unwrap()));
    assert_ne!(input_digest(&perturbed), golden.provenance.input_sha256);
}

#[test]
fn aged_history_changes_the_tail_only_when_it_matches_the_prompt() {
    let golden = golden();
    let hinted = golden
        .cases
        .iter()
        .filter(|case| case.input.aged_hint_expected)
        .count();
    assert!(
        hinted >= 1 && hinted < golden.cases.len(),
        "both arms are represented"
    );
    for case in &golden.cases {
        let fresh = run_fresh(case);
        let aged = run_aged(case, true);
        assert_eq!(fresh, case.expected.fresh, "{} fresh", case.id);
        assert_eq!(aged, case.expected.aged, "{} aged", case.id);
        assert!(!fresh.tail.contains(HINT_OPEN), "{}", case.id);
        assert_eq!(
            aged.tail.contains(HINT_OPEN),
            case.input.aged_hint_expected,
            "{}",
            case.id
        );
        assert_ne!(fresh.wire_digest, aged.wire_digest, "{}", case.id);
    }
}

/// The attachment is dropped through the production switch, not by editing the recorded output.
#[test]
fn a_dropped_attachment_fails_the_aged_golden() {
    for case in golden()
        .cases
        .iter()
        .filter(|case| case.input.aged_hint_expected)
    {
        let dropped = run_aged(case, false);
        assert!(!dropped.tail.contains(HINT_OPEN), "{}", case.id);
        assert_ne!(dropped, case.expected.aged, "{}", case.id);
        assert_ne!(
            dropped.wire_digest, case.expected.aged.wire_digest,
            "{}",
            case.id
        );
        assert_eq!(
            run_fresh(case),
            case.expected.fresh,
            "{} fresh arm holds",
            case.id
        );
    }
}

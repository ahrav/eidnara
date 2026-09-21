//! Surface 1 through the direct-host fixture: seed a session's history
//! segments, drive one native-serving transform pass, and map the host's
//! recorded auto-search outcome onto the evaluator's thirteen stages.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use daemon::transform::{UserHintPass, UserHintSkip};
use eval_core::{Ledger, Observation, RenderedMessage, Surface1Stage};
use host_runtime::TargetKind;
use memory_store::{MemoryStore, StoredHistorySegment};
use serde_json::{Value, json};

use super::direct_host::{BUDGET, FixtureProcess, request_json, wait_for_store};

pub const EPOCH_MS: i64 = 1_700_000_000_000;

pub type SurfaceLedger = Ledger<Surface1Stage>;

pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

/// One session's rendered messages in valid-time order; every message is a
/// native OpenCode message with the identity the renderer expects for it.
pub struct World {
    pub session: String,
    pub messages: Vec<RenderedMessage>,
}

pub fn mid(message: &RenderedMessage) -> &str {
    message.message["info"]["id"].as_str().unwrap()
}

pub fn text(message: &RenderedMessage) -> &str {
    message.message["parts"][0]["text"].as_str().unwrap()
}

/// The identity the renderer expects for the message's text unit.
pub fn expected_id(message: &RenderedMessage) -> String {
    message.expected[0].identity.occurrence_id.clone()
}

/// A history segment covering one message, with `phrase` as its summary;
/// `end_message_id` is the message's native identity as the summarizer
/// writes it.
pub fn segment(sequence: i64, message: &RenderedMessage, phrase: &str) -> StoredHistorySegment {
    StoredHistorySegment {
        sequence,
        start_message: sequence,
        end_message: sequence,
        start_message_id: format!("{}#0", mid(message)),
        end_message_id: format!("{}#0", mid(message)),
        title: format!("C{sequence}"),
        content: phrase.to_string(),
        p1: Some(phrase.to_string()),
        importance: 50,
        created_at: 0,
        ..Default::default()
    }
}

pub fn seed_store(root: &Path, session: &str, segments: &[StoredHistorySegment]) {
    let descriptor = daemon::managed_store_descriptor(root).unwrap();
    let store = MemoryStore::open(&descriptor).unwrap();
    store.replace_history_segments(session, segments).unwrap();
}

/// Segment sequence to the evaluator-expected occurrence id of the message it
/// ends on, through the segment's stored native identity.
pub fn identities(world: &World, segments: &[StoredHistorySegment]) -> BTreeMap<i64, String> {
    segments
        .iter()
        .map(|segment| {
            let (end_mid, _) = daemon::wire::split_block_id(&segment.end_message_id)
                .expect("a seeded segment ends on a block id");
            let message = world
                .messages
                .iter()
                .find(|message| mid(message) == end_mid)
                .expect("a segment ends on a rendered message");
            (segment.sequence, expected_id(message))
        })
        .collect()
}

/// The message as the OpenCode plugin sends it to the daemon: a text part
/// is a text block; a completed tool part is its call and its result, the
/// result's output as text.
pub fn ingress(message: &RenderedMessage, ordinal: u64) -> Value {
    let mut content = Vec::new();
    for part in message.message["parts"].as_array().unwrap() {
        match part["type"].as_str() {
            Some("text") => content.push(json!({"kind": {"type": "text", "text": part["text"]}})),
            Some("tool") => {
                let call_id = &part["callID"];
                let tool = &part["tool"];
                content.push(json!({"kind": {
                    "type": "tool_call", "id": call_id, "name": tool,
                    "input": part["state"]["input"],
                }}));
                content.push(json!({"kind": {
                    "type": "tool_result", "id": call_id, "tool_name": tool,
                    "output": {"kind": {"type": "text", "text": part["state"]["output"]}},
                }}));
            }
            other => panic!("a rendered message carries text and tool parts, not {other:?}"),
        }
    }
    json!({
        "mid": mid(message),
        "ordinal": ordinal,
        "ck": {
            "role": message.message["info"]["role"],
            "content": content,
            "meta": {"harness_id": mid(message)}
        }
    })
}

pub fn tail(session: &str, prompt: &str, ordinal: u64) -> (Value, Value) {
    let mid = format!("tail-{ordinal}");
    (
        json!({
            "mid": mid,
            "ordinal": ordinal,
            "ck": {
                "role": "user",
                "content": [{"kind": {"type": "text", "text": prompt}}],
                "meta": {"harness_id": mid}
            }
        }),
        json!({
            "info": {"id": mid, "sessionID": session, "role": "user", "time": {"created": EPOCH_MS + 10_000_000}},
            "parts": [{"type": "text", "text": prompt}]
        }),
    )
}

pub struct Pass {
    pub response: Value,
    pub native: Vec<Value>,
    /// The host's recorded pass, as the fixture's control socket returns it;
    /// `None` when auto-search did not run.
    pub outcome: Option<UserHintPass>,
}

pub struct Knobs {
    pub threshold: f64,
    pub min_prompt_chars: usize,
    pub native_tail: bool,
    /// Context pressure the harness reports with the request, as
    /// `(current_total_input_tokens, context_limit_tokens)`; without it the
    /// boundary protects the whole history and the summarizer never fires.
    pub usage: Option<(u64, u64)>,
}

impl Default for Knobs {
    fn default() -> Self {
        Self {
            threshold: 0.6,
            min_prompt_chars: 20,
            native_tail: true,
            usage: None,
        }
    }
}

/// The transform request one harness turn sends: the world's first `upto`
/// messages as ingress and native arrays, `tail` as a new user message after
/// them when given, and the pass's knobs.
pub fn transform_request(world: &World, upto: usize, tail: Option<&str>, knobs: &Knobs) -> Value {
    let mut messages: Vec<Value> = world.messages[..upto]
        .iter()
        .enumerate()
        .map(|(index, message)| ingress(message, index as u64 + 1))
        .collect();
    let mut native: Vec<Value> = world.messages[..upto]
        .iter()
        .map(|m| m.message.clone())
        .collect();
    if let Some(prompt) = tail {
        let (tail_ingress, tail_native) = self::tail(&world.session, prompt, upto as u64 + 1);
        messages.push(tail_ingress);
        if knobs.native_tail {
            native.push(tail_native);
        }
    }
    let mut request = json!({
            "kind": "transform",
            "base_revision": "surface-base-1",
            "v": 2,
            "session_id": world.session,
            "serializer_profile": "opencode-aisdk",
            "render_config": "surface-config",
            "full_array_fingerprint": format!("surface-fingerprint-{upto}-{}", tail.is_some()),
            "serve_native": true,
            "native_messages": native,
            "auto_search_enabled": true,
            "auto_search_score_threshold": knobs.threshold,
            "auto_search_min_prompt_chars": knobs.min_prompt_chars,
            "messages": messages,
    });
    if let Some((current_total_input_tokens, context_limit_tokens)) = knobs.usage {
        request["usage"] = json!({
            "current_total_input_tokens": current_total_input_tokens,
            "context_limit_tokens": context_limit_tokens,
        });
    }
    request
}

/// Drives one native-serving transform pass through the fixture and reads the
/// host's recorded auto-search outcome back over the control socket.
pub async fn pass(fixture: &FixtureProcess, world: &World, prompt: &str, knobs: &Knobs) -> Pass {
    let client = fixture.client().await;
    let route = fixture
        .open_route(&client, "context", TargetKind::ToolProvider, &world.session)
        .await;
    wait_for_store(&client, route, &world.session).await;
    let request = transform_request(world, world.messages.len(), Some(prompt), knobs);
    let native = request["native_messages"].as_array().unwrap().clone();
    let response = request_json(&client, route, request).await;
    assert_eq!(response["status"], "ok", "{response}");
    assert!(
        response.get("user_hint").is_none(),
        "the outcome stays off the wire"
    );
    let control = fixture.control(41, "user-hint-outcome");
    assert_eq!(control["ok"], true, "{control}");
    let outcome = control["result"]["outcome"]
        .as_object()
        .map(|_| serde_json::from_value(control["result"]["outcome"].clone()).unwrap());
    if let Some(UserHintPass::Decided(decided)) = &outcome {
        assert_eq!(
            decided.block_id,
            format!("tail-{}#0", world.messages.len() + 1),
            "the recorded pass is this request's tail"
        );
    }
    client.close_route(route).await.expect("route closes");
    Pass {
        response,
        native,
        outcome,
    }
}

/// Whether a history_summarizer firing or reattach is still running inside
/// the fixture.
pub fn summarizer_live(fixture: &FixtureProcess) -> bool {
    let control = fixture.control(43, "history-summarizer-live");
    assert_eq!(control["ok"], true, "{control}");
    control["result"]["live"]
        .as_bool()
        .unwrap_or_else(|| panic!("the control answers with a boolean: {control}"))
}

/// Waits until no firing is live; a firing spawned behind a pass finishes
/// before the next mutation, so every turn starts from quiescence. Blocks
/// the calling thread on the control socket, so `lifecycle` is driven from
/// `block_on` on the caller's thread and never spawned onto a worker.
pub fn drain(fixture: &FixtureProcess) {
    let deadline = std::time::Instant::now() + BUDGET;
    while summarizer_live(fixture) {
        assert!(
            std::time::Instant::now() < deadline,
            "the summarizer firing did not settle within the budget"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Lives the world through the fixture one harness turn at a time, as the
/// harness would send it: turn `n` carries the first `n` messages, with the
/// context pressure `usage(n)` reports for that turn, and the store moves
/// through every turn in one incarnation. Each turn is mutate, then drain to
/// quiescence. Returns each turn's transform response in order.
pub async fn lifecycle(
    fixture: &FixtureProcess,
    world: &World,
    usage: impl Fn(usize) -> Option<(u64, u64)>,
) -> Vec<Value> {
    let client = fixture.client().await;
    let route = fixture
        .open_route(&client, "context", TargetKind::ToolProvider, &world.session)
        .await;
    wait_for_store(&client, route, &world.session).await;
    let mut turns = Vec::with_capacity(world.messages.len());
    for upto in 1..=world.messages.len() {
        let knobs = Knobs {
            usage: usage(upto),
            ..Knobs::default()
        };
        let request = transform_request(world, upto, None, &knobs);
        let response = request_json(&client, route, request).await;
        assert_eq!(response["status"], "ok", "turn {upto}: {response}");
        drain(fixture);
        turns.push(response);
    }
    client.close_route(route).await.expect("route closes");
    turns
}

pub fn ids(sequences: &[i64], identities: &BTreeMap<i64, String>) -> BTreeSet<String> {
    sequences
        .iter()
        .map(|sequence| identities[sequence].clone())
        .collect()
}

/// Maps the host's outcome onto the thirteen stages. A gate that passed kept
/// every segment; a refusing gate kept none and the search stages never ran,
/// while the empty decision was still frozen, deferred nowhere, and applied
/// and attached nowhere.
pub fn observe(
    ledger: &mut SurfaceLedger,
    pass: Option<&UserHintPass>,
    identities: &BTreeMap<i64, String>,
) {
    observe_rendered(ledger, pass, identities, |_| true);
}

/// `observe`, with `rendered` deciding whether a selected segment's served
/// fragment carries the occurrence it stands for: a segment the cap kept but
/// the fragment truncated past the evidence reaches render with the evidence
/// absent, and every stage after it.
pub fn observe_rendered(
    ledger: &mut SurfaceLedger,
    pass: Option<&UserHintPass>,
    identities: &BTreeMap<i64, String>,
    rendered: impl Fn(i64) -> bool,
) {
    let universe: BTreeSet<String> = identities.values().cloned().collect();
    let record = |ledger: &mut SurfaceLedger, stage: Surface1Stage, kept: BTreeSet<String>| {
        ledger.observe(Observation::new(stage, 0, None, kept).unwrap());
    };
    let outcome = match pass {
        Some(UserHintPass::Decided(outcome)) => outcome,
        // An earlier pass's decision still stands and may be served; this pass
        // says nothing about it.
        Some(UserHintPass::Skipped {
            reason: UserHintSkip::AlreadyDecided | UserHintSkip::BehindFrontier,
        }) => {
            ledger.observe(Observation::unjoinable(
                Surface1Stage::TailEligibility,
                0,
                None,
            ));
            return;
        }
        Some(UserHintPass::Skipped { .. }) => {
            record(ledger, Surface1Stage::TailEligibility, BTreeSet::new());
            return;
        }
        // Auto-search did not run, so no stage was reached.
        None => return,
    };
    let trace = &outcome.trace;
    record(ledger, Surface1Stage::TailEligibility, universe.clone());
    let gates = [
        (Surface1Stage::Suppression, trace.suppression),
        (Surface1Stage::LengthGate, trace.length),
        (Surface1Stage::TokenGate, trace.tokens),
    ];
    let mut open = true;
    for (stage, passed) in gates {
        if open {
            let kept = if passed {
                universe.clone()
            } else {
                BTreeSet::new()
            };
            record(ledger, stage, kept);
        }
        open &= passed;
    }
    let selected = ids(&trace.selected, identities);
    if open {
        record(
            ledger,
            Surface1Stage::CandidateWindow,
            ids(&trace.window, identities),
        );
        let matched = ids(&trace.matched, identities);
        record(ledger, Surface1Stage::MatchFilter, matched.clone());
        let thresholded = if trace.threshold {
            matched
        } else {
            BTreeSet::new()
        };
        record(ledger, Surface1Stage::Threshold, thresholded);
        record(ledger, Surface1Stage::Cap, selected.clone());
    }
    let served: BTreeSet<String> = trace
        .selected
        .iter()
        .filter(|sequence| rendered(**sequence))
        .map(|sequence| identities[sequence].clone())
        .collect();
    let kept = |flag: bool| {
        if flag && !outcome.hint_text.is_empty() {
            served.clone()
        } else {
            BTreeSet::new()
        }
    };
    if open {
        record(ledger, Surface1Stage::Render, kept(true));
    }
    record(ledger, Surface1Stage::DecisionFreeze, kept(true));
    record(ledger, Surface1Stage::Deferral, kept(!outcome.deferred));
    record(ledger, Surface1Stage::OverlayApply, kept(outcome.applied));
    record(ledger, Surface1Stage::Attachment, kept(outcome.attached));
}

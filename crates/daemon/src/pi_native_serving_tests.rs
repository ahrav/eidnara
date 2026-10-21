//! Native serving of the `pi` serializer profile through the typed transform handler against a
//! real store.

use super::fold_authority_handler_tests::{assert_native_state, native_config};
use super::*;

const SYNTHETIC_ID_PREFIX: &str = "eidnara:synthetic:";

fn pi_rows(messages: &[IngressMessage]) -> Vec<Value> {
    messages
        .iter()
        .map(|message| {
            let text = message_text(message);
            let pi_message = if message.ck.role == "assistant" {
                json!({
                    "role": "assistant",
                    "content": [{ "type": "text", "text": text }],
                    "usage": {},
                    "stopReason": "stop",
                    "timestamp": 1_000 + message.ordinal,
                })
            } else {
                json!({ "role": "user", "content": text, "timestamp": 1_000 + message.ordinal })
            };
            json!({ "id": message.mid, "message": pi_message })
        })
        .collect()
}

fn opencode_messages(messages: &[IngressMessage]) -> Vec<Value> {
    messages
        .iter()
        .map(|message| {
            json!({
                "info": { "id": message.mid, "sessionID": "ses", "role": message.ck.role },
                "parts": [{ "type": "text", "text": message_text(message) }],
            })
        })
        .collect()
}

fn message_text(message: &IngressMessage) -> String {
    match message.ck.content()[0].kind() {
        BlockKind::Text { text } => text.clone(),
        other => unreachable!("{other:?}"),
    }
}

fn ingress(decoded: codec::DecodedHarnessMessages) -> Vec<IngressMessage> {
    decoded
        .messages
        .into_iter()
        .map(|mut message| {
            message.ck.meta.created_at_ms = Some(1_000 + message.ordinal as i64);
            message
        })
        .collect()
}

fn alternating_transcript() -> Vec<IngressMessage> {
    big_messages()
        .into_iter()
        .map(|message| {
            if message.ordinal % 2 == 0 {
                wire_with_role(
                    &message.mid,
                    message.ordinal,
                    "assistant",
                    &message_text(&message),
                )
            } else {
                message
            }
        })
        .collect()
}

fn native_request(profile: &str, messages: Vec<IngressMessage>, native: Vec<Value>) -> Value {
    let mut request = request_with_usage(messages, 48_000, 50_000);
    request["serializer_profile"] = json!(profile);
    request["native_messages"] = json!(native);
    request
}

fn texts(profile: &str, applied: &Value) -> Vec<Vec<String>> {
    applied
        .as_array()
        .unwrap()
        .iter()
        .map(|value| {
            let parts = match profile {
                "pi" => &value["message"]["content"],
                _ => &value["parts"],
            };
            match parts {
                Value::String(text) => vec![text.clone()],
                Value::Array(parts) => parts
                    .iter()
                    .filter_map(|part| part["text"].as_str().map(str::to_owned))
                    .collect(),
                other => panic!("unexpected content {other}"),
            }
        })
        .collect()
}

fn assert_no_session_state(handler: &Handler, store: &MemoryStore) {
    assert!(!store.has_cache_state("ses").unwrap());
    assert!(
        !handler
            .transform_snapshots
            .lock()
            .unwrap()
            .entries
            .contains_key("ses")
    );
    assert!(
        !handler
            .transform_session_roots
            .lock()
            .unwrap()
            .contains_key("ses")
    );
}

fn roles(profile: &str, applied: &Value) -> Vec<String> {
    applied
        .as_array()
        .unwrap()
        .iter()
        .map(|value| {
            let role = match profile {
                "pi" => &value["message"]["role"],
                _ => &value["info"]["role"],
            };
            role.as_str().unwrap().to_owned()
        })
        .collect()
}

fn pi_ids(applied: &Value) -> Vec<String> {
    applied
        .as_array()
        .unwrap()
        .iter()
        .map(|row| codec::pi::row_id(row).unwrap().to_owned())
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn a_pi_window_folds_to_the_m0_m1_and_tail_an_opencode_window_gets() {
    let transcript = alternating_transcript();
    let pi_native = pi_rows(&transcript);
    let opencode_native = opencode_messages(&transcript);
    let pi_ck = ingress(
        codec::pi::decode_pi_rows(&pi_native.iter().cloned().map(Arc::new).collect::<Vec<_>>())
            .unwrap(),
    );
    let opencode_ck = ingress(codec::decode_opencode(&opencode_native));
    assert_eq!(
        pi_ck, opencode_ck,
        "equivalent transcripts decode to one CK window"
    );

    let mut applied = BTreeMap::new();
    let mut replayed = BTreeMap::new();
    for (profile, ck, native) in [
        ("pi", pi_ck, pi_native.clone()),
        ("opencode-aisdk", opencode_ck, opencode_native),
    ] {
        let producer = Arc::new(ProducerState::default());
        let (handler, _store, _dir, _project) =
            handler_with_store(Arc::clone(&producer), default_test_config());
        let request = native_request(profile, ck, native);
        let response = call_transform_request(&handler, request.clone()).await;
        assert_eq!(response["status"], "ok", "{profile}: {response}");
        assert_eq!(response["action"], "HARD", "{profile}: {response}");
        assert_eq!(producer.starts.load(Ordering::SeqCst), 1, "{profile}");
        applied.insert(profile, response["native_messages"].clone());
        let mut again = request;
        again["base_revision"] = json!("test-base-again");
        let response = call_transform_request(&handler, again).await;
        assert_eq!(response["status"], "ok", "{profile}: {response}");
        assert!(
            response["timings"]["native_cache_encoded_messages"]
                .as_u64()
                .is_some_and(|encoded| encoded > 0),
            "the second pass encodes its output again: {response}"
        );
        replayed.insert(profile, response["native_messages"].clone());
    }

    for outputs in [&applied, &replayed] {
        let pi = texts("pi", &outputs["pi"]);
        let opencode = texts("opencode-aisdk", &outputs["opencode-aisdk"]);
        assert!(pi[0][0].contains("autonomous summary"), "{:?}", pi[0]);
        assert_eq!(pi, opencode, "m0, m1, and tail are equal across profiles");
        assert_eq!(
            roles("pi", &outputs["pi"]),
            roles("opencode-aisdk", &outputs["opencode-aisdk"]),
        );
    }

    let ids = pi_ids(&applied["pi"]);
    let tail_start = ids
        .iter()
        .position(|id| !codec::pi::is_reserved_id(id))
        .expect("the tail survives");
    assert!(tail_start >= 1, "m0 precedes the tail: {ids:?}");
    let rows = applied["pi"].as_array().unwrap();
    for row in &rows[tail_start..] {
        let input = pi_native
            .iter()
            .find(|input| input["id"] == row["id"])
            .expect("a tail row comes from the window");
        assert_eq!(row, input);
    }

    let later: BTreeMap<String, String> = replayed["pi"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["message"].to_string(),
                row["id"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let first_m0 = &rows[0];
    assert_eq!(
        later
            .get(&first_m0["message"].to_string())
            .map(String::as_str),
        first_m0["id"].as_str(),
        "the second pass renders m0 from the same history under the same reserved id"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn identical_pi_passes_re_encode_identical_rows() {
    let producer = Arc::new(ProducerState::default());
    let (handler, _store, _dir, _project) =
        handler_with_store(Arc::clone(&producer), default_test_config());
    let messages = (1..=6u64)
        .map(|ordinal| {
            ck(
                &format!("{ordinal:08x}"),
                ordinal,
                &format!("turn {ordinal}"),
            )
        })
        .collect::<Vec<_>>();
    let mut request = native_request("pi", messages.clone(), pi_rows(&messages));
    request["usage"] = json!(ModuleUsage {
        current_total_input_tokens: 1_000,
        context_limit_tokens: 200_000,
        ..ModuleUsage::default()
    });
    let first = call_transform_request(&handler, request.clone()).await;
    assert_eq!(first["status"], "ok", "{first}");
    request["base_revision"] = json!("test-base-second");
    let second = call_transform_request(&handler, request).await;
    assert_eq!(second["status"], "ok", "{second}");
    assert_eq!(second["native_messages"], first["native_messages"]);
    assert!(keeps_from(&second, "previous") > 0, "{second}");
}

#[tokio::test(flavor = "current_thread")]
async fn native_serving_admits_pi_and_still_refuses_another_profile_with_no_effect() {
    let producer = Arc::new(ProducerState::default());
    let (handler, store, _dir, _project) = handler_with_store(producer, default_test_config());
    let messages = vec![ck("1a2b3c4d", 1, "hello")];

    for profile in ["claude-code-anthropic", "owned-llmrunner"] {
        let outcome = call_transform_outcome(
            &handler,
            native_request(profile, messages.clone(), pi_rows(&messages)),
        )
        .await;
        assert_eq!(
            error_frame(outcome),
            (
                "invalid_params".to_string(),
                format!(
                    "serializer_profile {profile} carries its window in messages, not native_messages"
                )
            )
        );
        assert_no_session_state(&handler, &store);
    }

    let rows = pi_rows(&messages);
    let response = call_transform_request(
        &handler,
        native_request("pi", messages.clone(), rows.clone()),
    )
    .await;
    assert_eq!(response["status"], "ok", "{response}");
    let applied = response["native_messages"].as_array().unwrap();
    assert_eq!(applied.last(), rows.last(), "{response}");
    assert!(
        applied[..applied.len() - 1]
            .iter()
            .all(|row| codec::pi::is_reserved_id(codec::pi::row_id(row).unwrap())),
        "{response}"
    );
}

/// One row of every closed Pi role, led by the summary row Pi's own compaction writes, with the
/// tool call and its result paired.
fn every_closed_role_rows() -> Vec<Value> {
    vec![
        json!({ "id": "eidnara:compactionSummary:c0ffee00", "message": {
            "role": "compactionSummary", "summary": "earlier", "tokensBefore": 10, "timestamp": 1
        } }),
        json!({ "id": "eidnara:branchSummary:c0ffee01", "message": {
            "role": "branchSummary", "summary": "branch", "fromId": "1a2b3c4d", "timestamp": 2
        } }),
        json!({ "id": "1a2b3c4d", "message": {
            "role": "user", "content": [{ "type": "text", "text": "run it" }], "timestamp": 3
        } }),
        json!({ "id": "2b3c4d5e", "message": {
            "role": "assistant",
            "content": [{
                "type": "toolCall", "id": "call-1", "name": "bash", "arguments": { "command": "ls" }
            }],
            "usage": {}, "stopReason": "toolUse", "timestamp": 4
        } }),
        json!({ "id": "3c4d5e6f", "message": {
            "role": "toolResult", "toolCallId": "call-1", "toolName": "bash",
            "content": [{ "type": "text", "text": "a b" }], "isError": false, "timestamp": 5
        } }),
        json!({ "id": "4d5e6f70", "message": {
            "role": "bashExecution", "command": "pwd", "output": "/w", "exitCode": 0,
            "cancelled": false, "truncated": false, "timestamp": 6
        } }),
        json!({ "id": "eidnara:custom:5e6f7081", "message": {
            "role": "custom", "customType": "note", "content": "custom", "display": true,
            "timestamp": 7
        } }),
    ]
}

#[tokio::test(flavor = "current_thread")]
async fn every_closed_role_serves_through_the_handler_and_replays_its_row() {
    let producer = Arc::new(ProducerState::default());
    let (handler, _store, _dir, _project) = handler_with_store(producer, default_test_config());
    let rows = every_closed_role_rows();
    let shared: Vec<Arc<Value>> = rows.iter().cloned().map(Arc::new).collect();
    let ck = ingress(codec::pi::decode_pi_rows(&shared).unwrap());
    let mut request = native_request("pi", ck, rows.clone());
    request["usage"] = json!(ModuleUsage {
        current_total_input_tokens: 1_000,
        context_limit_tokens: 200_000,
        ..ModuleUsage::default()
    });
    let response = call_transform_request(&handler, request).await;
    assert_eq!(response["status"], "ok", "{response}");
    let replayed: Vec<&Value> = response["native_messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| rows.iter().any(|input| input["id"] == row["id"]))
        .collect();
    assert_eq!(replayed, rows.iter().collect::<Vec<_>>(), "{response}");
}

#[tokio::test(flavor = "current_thread")]
async fn a_first_pi_window_with_an_unknown_role_leaves_no_session_state() {
    let producer = Arc::new(ProducerState::default());
    let (handler, store, _dir, _project) = handler_with_store(producer, default_test_config());
    let messages = vec![ck("1a2b3c4d", 1, "hello")];
    let mut rows = pi_rows(&messages);
    rows[0]["message"]["role"] = json!("systemNotice");
    let outcome = call_transform_outcome(&handler, native_request("pi", messages, rows)).await;
    assert_eq!(error_code(outcome), "invalid_params");
    assert_no_session_state(&handler, &store);
}

#[tokio::test(flavor = "current_thread")]
async fn a_pi_window_with_an_unknown_role_declines_with_no_state_change() {
    let producer = Arc::new(ProducerState::default());
    let (handler, store, _dir, _project) = handler_with_store(producer, default_test_config());
    let messages = vec![ck("1a2b3c4d", 1, "hello"), ck("2b3c4d5e", 2, "again")];
    let first = call_transform_request(
        &handler,
        native_request("pi", messages.clone(), pi_rows(&messages)),
    )
    .await;
    assert_eq!(first["status"], "ok", "{first}");
    let before = store.load("ses").unwrap();

    let mut unknown = pi_rows(&messages);
    unknown[1]["message"]["role"] = json!("systemNotice");
    let mut reserved = pi_rows(&messages);
    reserved[0]["id"] = json!("eidnara:user:1a2b3c4d");
    let mut malformed = pi_rows(&messages);
    malformed[1]["message"] = json!("assistant");
    for (window, rows, expected) in [
        (messages.clone(), unknown, "\"systemNotice\""),
        (messages.clone(), reserved, "must carry a host entry id"),
        (
            messages.clone(),
            malformed,
            "is not an {id, message: {role}} row",
        ),
    ] {
        let outcome = call_transform_outcome(&handler, native_request("pi", window, rows)).await;
        let (code, message) = error_frame(outcome);
        assert_eq!(code, "invalid_params");
        assert!(message.contains(expected), "{message}");
    }

    let after = store.load("ses").unwrap();
    assert_eq!(after.row_version, before.row_version);
    assert!(after.meta == before.meta);
}

#[tokio::test(flavor = "current_thread")]
async fn tagging_applies_to_a_pi_window() {
    assert!(tagging_surface_active(Some(SerializerProfile::Pi), true));
    assert!(!tagging_surface_active(Some(SerializerProfile::Pi), false));

    let producer = Arc::new(ProducerState::default());
    let (handler, _store, _dir, _project) = handler_with_store(producer, default_test_config());
    let messages = (1..=25u64)
        .map(|ordinal| {
            ck(
                &format!("{ordinal:08x}"),
                ordinal,
                &format!("output {ordinal}"),
            )
        })
        .collect::<Vec<_>>();
    let mut request = native_request("pi", messages.clone(), pi_rows(&messages));
    request["usage"] = json!(ModuleUsage {
        current_total_input_tokens: 45_000,
        context_limit_tokens: 50_000,
        ..ModuleUsage::default()
    });
    request["tool_present"] = json!(true);

    let transition = call_transform_request(&handler, request.clone()).await;
    assert_eq!(transition["surface_state"], "transition", "{transition}");
    let tagged = call_transform_request(&handler, request).await;
    assert_eq!(tagged["surface_state"], "active", "{tagged}");
    let rows = tagged["native_messages"].as_array().unwrap();
    let first_host = rows
        .iter()
        .find(|row| row["id"] == "00000001")
        .expect("the first host message is served");
    assert_eq!(first_host["message"]["content"], "§1§ output 1");
    assert_eq!(first_host["message"]["role"], "user");
}

#[test]
fn a_user_hint_in_string_content_counts_as_carried() {
    let outcome = transform::UserHintOutcome {
        block_id: "00000001#0".to_string(),
        hint_text: "remembered hint".to_string(),
        trace: transform::UserHintTrace::default(),
        deferred: false,
        applied: true,
        attached: false,
    };
    let carried = [Arc::new(json!({
        "id": "00000001",
        "message": { "role": "user", "content": "question\n\nremembered hint", "timestamp": 1 }
    }))];
    assert!(native_carries_user_hint(
        Some(SerializerProfile::Pi),
        &carried,
        &outcome
    ));
    let absent = [Arc::new(json!({
        "id": "00000001",
        "message": { "role": "user", "content": "question", "timestamp": 1 }
    }))];
    assert!(!native_carries_user_hint(
        Some(SerializerProfile::Pi),
        &absent,
        &outcome
    ));
}

/// A native binding that keeps the summarizer chain and withdraws fold authority through
/// `compaction.enabled`, so a native pass has a chain it could fire.
fn native_config_with_a_live_chain() -> DaemonConfig {
    let config = DaemonConfig {
        compaction_enabled: false,
        ..default_test_config()
    };
    assert!(!config.model_chain.is_empty() && !config.eidnara_folds());
    config
}

/// A handler over a real store and an open kernel whose route binds `config` for the session's
/// project, with the project's scope object in place for canonical memory rows.
async fn pi_handler(
    producer: &Arc<ProducerState>,
    config: DaemonConfig,
) -> (Handler, Arc<MemoryStore>, tempfile::TempDir, SessionBinding) {
    let (handler, store, dir, project) =
        handler_with_store_and_kernel(Arc::clone(producer), default_test_config()).await;
    let mut bound = binding(project.to_str().unwrap(), "ses");
    bound.config = config;
    let kernel = handler.kernel.kernel_store().unwrap();
    kernel
        .commit(kernel_route_fixtures::intent("project-scope"), |envelope| {
            let mut domains = HashSet::new();
            let mut ready = false;
            let mut refused = None;
            kernel_routes::commit::ensure_scope(
                envelope,
                &bound.kernel_project,
                &mut ready,
                &mut domains,
                &mut refused,
            )?;
            Ok(String::new())
        })
        .unwrap();
    handler.bind_route(test_route(7), bound.clone());
    (handler, store, dir, bound)
}

/// A Pi pass over `rows` at the stated pressure. The test client names every attempt with a fresh
/// base revision and offers its applied output as `previous`.
fn pi_request(rows: &[Value], current_total_input_tokens: u64, context_limit_tokens: u64) -> Value {
    let shared: Vec<Arc<Value>> = rows.iter().cloned().map(Arc::new).collect();
    let ck = ingress(codec::pi::decode_pi_rows(&shared).unwrap());
    let mut request = native_request("pi", ck, rows.to_vec());
    request["usage"] = json!(ModuleUsage {
        current_total_input_tokens,
        context_limit_tokens,
        ..ModuleUsage::default()
    });
    request.as_object_mut().unwrap().remove("base_revision");
    request
}

/// Serves one pass, applies its recipe against the submitted rows and the retained output, and
/// checks that every row an `input` keep serves is the submitted `Arc` itself. Returns the
/// response with `native_messages` set to the applied rows.
async fn serve(handler: &Handler, request: Value) -> Value {
    let request = handler.test_client_prepare_request(request);
    let outcome = handler
        .handle_transform_for_test(test_route(7), request.clone())
        .await;
    let PreparedOutcome::Response(bytes) = &outcome else {
        panic!("unexpected handler outcome: {outcome:?}");
    };
    let raw: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(raw["status"], "ok", "{raw}");
    let base = |values: &[Value]| -> (Vec<Arc<Value>>, Vec<usize>) {
        let values: Vec<Arc<Value>> = values.iter().cloned().map(Arc::new).collect();
        let lengths = values
            .iter()
            .map(|value| edit_recipe::canonical_len(value).unwrap())
            .collect();
        (values, lengths)
    };
    let (input, input_lengths) = base(request["native_messages"].as_array().unwrap());
    let input_revision =
        edit_recipe::Revision::parse(request["base_revision"].as_str().unwrap()).unwrap();
    let retained = handler
        .test_client
        .lock()
        .unwrap()
        .get("ses")
        .and_then(|client| client.applied_native.clone());
    let previous = retained.as_ref().map(|(revision, values)| {
        let (values, lengths) = base(values);
        (
            edit_recipe::Revision::parse(revision).unwrap(),
            values,
            lengths,
        )
    });
    let applied = edit_recipe::Recipe::from_json(&raw)
        .unwrap()
        .apply(
            edit_recipe::SourceBase {
                revision: &input_revision,
                values: &input,
                lengths: &input_lengths,
            },
            previous
                .as_ref()
                .map(|(revision, values, lengths)| edit_recipe::SourceBase {
                    revision,
                    values,
                    lengths,
                }),
        )
        .unwrap();
    let mut served = 0;
    for operation in raw["operations"].as_array().unwrap() {
        let count = match operation["op"].as_str().unwrap() {
            "insert" => operation["values"].as_array().unwrap().len(),
            _ => operation["count"].as_u64().unwrap() as usize,
        };
        if operation["op"] == "keep" && operation["source"] == "input" {
            let start = operation["start"].as_u64().unwrap() as usize;
            for offset in 0..count {
                assert!(
                    Arc::ptr_eq(&applied.values[served + offset], &input[start + offset]),
                    "served row {} is the submitted row {}",
                    served + offset,
                    start + offset
                );
            }
        }
        served += count;
    }
    assert_eq!(served, applied.values.len());
    let response = tool_body(handler.test_client_apply(&request, outcome));
    let applied: Vec<Value> = applied
        .values
        .iter()
        .map(|value| (**value).clone())
        .collect();
    assert_eq!(response["native_messages"], json!(applied));
    response
}

/// The texts of the two synthetic rows that lead an additive Pi output: m0, then m1.
fn synthetic_texts(applied: &Value) -> Vec<String> {
    let lead = json!(applied.as_array().unwrap()[..2]);
    assert!(
        pi_ids(&lead)
            .iter()
            .all(|id| id.starts_with(SYNTHETIC_ID_PREFIX)),
        "{lead}"
    );
    texts("pi", &lead)
        .into_iter()
        .map(|text| text.concat())
        .collect()
}

/// Asserts the additive shape: two synthetic user rows, then exactly the submitted rows in order.
fn assert_additive_shape(response: &Value, rows: &[Value], context: &str) {
    let applied = response["native_messages"].as_array().unwrap();
    assert_eq!(applied.len(), rows.len() + 2, "{context}: {response}");
    let ids = pi_ids(&response["native_messages"]);
    assert!(
        ids[..2]
            .iter()
            .all(|id| id.starts_with(SYNTHETIC_ID_PREFIX)),
        "{context}: {ids:?}"
    );
    assert_eq!(
        roles("pi", &response["native_messages"])[..2],
        ["user", "user"],
        "{context}"
    );
    assert_eq!(&applied[2..], rows, "{context}");
}

#[tokio::test(flavor = "current_thread")]
async fn a_native_pi_pass_serves_m0_m1_then_every_row_and_starts_no_summarizer() {
    let rows = every_closed_role_rows();
    let mut pressured = false;
    for (arm, config, input_tokens, limit_tokens) in [
        ("empty chain", native_config(), 1_000, 200_000),
        (
            "live chain",
            native_config_with_a_live_chain(),
            1_000,
            200_000,
        ),
        (
            "live chain at 96%",
            native_config_with_a_live_chain(),
            48_000,
            50_000,
        ),
    ] {
        let producer = Arc::new(ProducerState::default());
        let (handler, store, _dir, _bound) = pi_handler(&producer, config).await;

        let response = serve(&handler, pi_request(&rows, input_tokens, limit_tokens)).await;

        assert_eq!(response["boundary"], Value::Null, "{arm}: {response}");
        assert_eq!(response["action"], "HARD", "{arm}: {response}");
        assert_eq!(response["materialize_reason"], "first_render", "{arm}");
        let operations = response["operations"].as_array().unwrap();
        assert_eq!(operations.len(), 2, "{arm}: {response}");
        assert_eq!(operations[0]["op"], "insert", "{arm}");
        assert_eq!(
            operations[0]["values"].as_array().unwrap().len(),
            2,
            "{arm}"
        );
        assert_eq!(
            operations[1],
            json!({ "op": "keep", "source": "input", "start": 0, "count": rows.len() }),
            "{arm}"
        );
        assert_additive_shape(&response, &rows, arm);
        assert_eq!(
            pi_ids(&response["native_messages"])[2],
            "eidnara:compactionSummary:c0ffee00",
            "{arm}: the host's own summary leads the served window"
        );
        let diagnostics = &response["history_summarizer"];
        assert_eq!(diagnostics["fired"], false, "{arm}: {diagnostics}");
        assert_eq!(diagnostics["no_fire"], "native_authority", "{arm}");
        assert_eq!(response["committed"], true, "{arm}: {response}");
        assert_native_state(&store, "ses", arm);
        let meta = store.load("ses").unwrap().meta;
        let usage = meta.last_usage.expect("the pass records its usage");
        assert_eq!(usage.current_total_input_tokens, input_tokens, "{arm}");
        assert_eq!(usage.context_limit_tokens, limit_tokens, "{arm}");
        pressured |= usage.current_total_input_tokens * 100 >= usage.context_limit_tokens * 95;
        assert!(meta.newest_live_block_id.is_some(), "{arm}");
        assert_eq!(producer.starts.load(Ordering::SeqCst), 0, "{arm}");
        assert_eq!(producer.connects.load(Ordering::SeqCst), 0, "{arm}");
    }
    assert!(pressured, "one arm runs at or above 95 percent pressure");
}

#[tokio::test(flavor = "current_thread")]
async fn the_same_pi_windows_differ_by_binding_alone() {
    let closed_roles = every_closed_role_rows();
    let long = pi_rows(&alternating_transcript());
    let mut outcomes = BTreeMap::new();
    for (authority, config) in [
        ("eidnara", default_test_config()),
        ("native", native_config_with_a_live_chain()),
    ] {
        for (window, rows) in [("closed roles", &closed_roles), ("long", &long)] {
            let producer = Arc::new(ProducerState::default());
            let (handler, store, _dir, _bound) = pi_handler(&producer, config.clone()).await;
            let response = serve(&handler, pi_request(rows, 48_000, 50_000)).await;
            assert_eq!(
                store.load("ses").unwrap().meta.eidnara_folds,
                Some(authority == "eidnara"),
                "{authority} {window}"
            );
            outcomes.insert(
                (authority, window),
                (response, producer.starts.load(Ordering::SeqCst)),
            );
        }
    }

    // The closed-role window is too short to fold: both authorities serve it whole, and only the
    // Eidnara binding evaluates the fold trigger.
    let (eidnara, eidnara_starts) = &outcomes[&("eidnara", "closed roles")];
    let (native, native_starts) = &outcomes[&("native", "closed roles")];
    assert_eq!(eidnara["native_messages"], native["native_messages"]);
    assert_additive_shape(native, &closed_roles, "native closed roles");
    assert_eq!(
        eidnara["history_summarizer"]["no_fire"], "trigger_false",
        "{eidnara}"
    );
    assert!(
        eidnara["timings"]["trigger_tokenized_blocks"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert_eq!(
        native["history_summarizer"]["no_fire"], "native_authority",
        "{native}"
    );
    assert_eq!(native["timings"]["trigger_tokenized_blocks"], 0, "{native}");
    assert_eq!((*eidnara_starts, *native_starts), (0, 0));

    // The long window at 96 percent folds under Eidnara, as the folding Pi case above records, and
    // is served whole under native authority with no summarizer start.
    let (eidnara, eidnara_starts) = &outcomes[&("eidnara", "long")];
    let (native, native_starts) = &outcomes[&("native", "long")];
    assert_eq!(*eidnara_starts, 1, "{eidnara}");
    assert!(
        synthetic_texts(&eidnara["native_messages"])[0].contains("autonomous summary"),
        "{eidnara}"
    );
    assert!(eidnara["native_messages"].as_array().unwrap().len() < long.len() + 2);
    assert_eq!(*native_starts, 0, "{native}");
    assert_eq!(
        native["history_summarizer"]["no_fire"], "native_authority",
        "{native}"
    );
    assert_additive_shape(native, &long, "native long window");
}

#[tokio::test(flavor = "current_thread")]
async fn a_repeated_native_pi_window_reconstructs_identical_rows() {
    let rows = every_closed_role_rows();
    let producer = Arc::new(ProducerState::default());
    let (handler, store, _dir, _bound) =
        pi_handler(&producer, native_config_with_a_live_chain()).await;
    let first = serve(&handler, pi_request(&rows, 1_000, 200_000)).await;
    assert_eq!(first["committed"], true, "{first}");
    assert_native_state(&store, "ses", "first pass");

    let mut revisions = vec![first["output_revision"].clone()];
    for (arm, input_tokens) in [("unchanged", 1_000), ("again", 1_000), ("new usage", 2_000)] {
        let before = store.load("ses").unwrap();
        let repeat = serve(&handler, pi_request(&rows, input_tokens, 200_000)).await;
        let after = store.load("ses").unwrap();

        assert_eq!(
            serde_json::to_string(&repeat["native_messages"]).unwrap(),
            serde_json::to_string(&first["native_messages"]).unwrap(),
            "{arm}"
        );
        assert!(
            !revisions.contains(&repeat["output_revision"]),
            "{arm}: a fresh output revision: {repeat}"
        );
        revisions.push(repeat["output_revision"].clone());
        assert_eq!(
            repeat["previous_output_revision"],
            revisions[revisions.len() - 2],
            "{arm}"
        );
        assert_eq!(
            keeps_from(&repeat, "previous"),
            1,
            "{arm}: the repeat keeps the retained output: {repeat}"
        );
        assert_eq!(keeps_from(&repeat, "input"), 0, "{arm}: {repeat}");
        let metadata_changed = after.meta != before.meta || after.row_version != before.row_version;
        assert_eq!(
            repeat["committed"] == true,
            metadata_changed,
            "{arm}: {repeat}"
        );
        if input_tokens == 1_000 {
            assert_eq!(repeat["committed"], false, "{arm}: {repeat}");
        }
        assert_native_state(&store, "ses", arm);
    }
    assert_eq!(producer.starts.load(Ordering::SeqCst), 0);
    assert_eq!(producer.connects.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn a_canonical_memory_write_rerenders_native_pi_m0_as_hard() {
    let rows = every_closed_role_rows();
    let producer = Arc::new(ProducerState::default());
    let (handler, store, _dir, bound) =
        pi_handler(&producer, native_config_with_a_live_chain()).await;
    let first = serve(&handler, pi_request(&rows, 1_000, 200_000)).await;
    assert_eq!(first["materialize_reason"], "first_render", "{first}");
    assert!(
        !synthetic_texts(&first["native_messages"])[0].contains("<project-memory>"),
        "{first}"
    );

    let kernel = handler.kernel.kernel_store().unwrap();
    kernel_route_fixtures::commit_verified_memory(
        &kernel,
        "native-rule",
        "mem-native",
        &bound.kernel_project.scope_id(),
        "PROJECT_RULES",
        "Serve the window additively.",
    );
    let line = "mem-native: Serve the window additively.";

    let second = serve(&handler, pi_request(&rows, 1_000, 200_000)).await;

    assert_eq!(second["action"], "HARD", "{second}");
    assert_eq!(
        second["materialize_reason"], "project_memory_epoch",
        "{second}"
    );
    assert_additive_shape(&second, &rows, "after the memory write");
    let synthetic = synthetic_texts(&second["native_messages"]);
    assert_eq!(
        synthetic[0].matches("<project-memory>").count(),
        1,
        "{synthetic:?}"
    );
    assert_eq!(synthetic[0].matches(line).count(), 1, "{synthetic:?}");
    let served = serde_json::to_string(&second["native_messages"]).unwrap();
    assert_eq!(
        served.matches(line).count(),
        1,
        "one copy per request: {served}"
    );
    let meta = store.load("ses").unwrap().meta;
    assert!(!meta.project_memory_epoch_pending);
    assert_native_state(&store, "ses", "memory HARD");

    let third = serve(&handler, pi_request(&rows, 1_000, 200_000)).await;
    assert_ne!(third["action"], "HARD", "{third}");
    assert_eq!(
        third["native_messages"], second["native_messages"],
        "{third}"
    );
    assert_eq!(producer.starts.load(Ordering::SeqCst), 0);
    assert_eq!(producer.connects.load(Ordering::SeqCst), 0);
}

//! Native serving of the `pi` serializer profile through the typed transform handler against a
//! real store.

use super::*;

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
    request["serve_native"] = json!(true);
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
    for (profile, ck, native) in [
        ("pi", pi_ck, pi_native.clone()),
        ("opencode-aisdk", opencode_ck, opencode_native),
    ] {
        let producer = Arc::new(ProducerState::default());
        let (handler, _store, _dir, _project) =
            handler_with_store(Arc::clone(&producer), default_test_config());
        let response = call_transform_request(&handler, native_request(profile, ck, native)).await;
        assert_eq!(response["status"], "ok", "{profile}: {response}");
        assert_eq!(response["action"], "HARD", "{profile}: {response}");
        assert_eq!(producer.starts.load(Ordering::SeqCst), 1, "{profile}");
        applied.insert(profile, response["native_messages"].clone());
    }

    let pi = texts("pi", &applied["pi"]);
    let opencode = texts("opencode-aisdk", &applied["opencode-aisdk"]);
    assert!(pi[0][0].contains("autonomous summary"), "{:?}", pi[0]);
    assert_eq!(pi, opencode, "m0, m1, and tail are equal across profiles");

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
        assert_eq!(error_code(outcome), "serve_native_unsupported_profile");
        assert!(
            store.load("ses").unwrap().meta == Default::default(),
            "{profile}"
        );
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

    let mut rows = pi_rows(&messages);
    rows[1]["message"]["role"] = json!("systemNotice");
    let outcome =
        call_transform_outcome(&handler, native_request("pi", messages.clone(), rows)).await;
    let (code, message) = error_frame(outcome);
    assert_eq!(code, "invalid_params");
    assert!(message.contains("\"systemNotice\""), "{message}");

    let mut reserved = pi_rows(&messages);
    reserved[0]["id"] = json!("eidnara:user:1a2b3c4d");
    let outcome = call_transform_outcome(&handler, native_request("pi", messages, reserved)).await;
    assert_eq!(error_code(outcome), "invalid_params");

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

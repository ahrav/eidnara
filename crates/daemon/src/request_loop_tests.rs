//! Within one turn, the harness adds an assistant tool call and its result to the window for
//! each request.

use super::*;
use crate::history_summarizer_archive::WINDOW_CAP_BLOCKS;

/// Each assistant message holds this many text parts, so tens of requests reach the window cap.
const TEXT_PARTS: usize = 38;

fn user_row() -> Value {
    json!({
        "id": "user-row", "type": "message", "parentId": null,
        "message": { "role": "user", "content": "Read the configuration file.", "timestamp": 1 },
    })
}

fn call_row(index: usize, parent: &str) -> Value {
    let mut content: Vec<Value> = (0..TEXT_PARTS)
        .map(|part| json!({ "type": "text", "text": format!("Step {index}.{part} of the plan.") }))
        .collect();
    content.push(json!({
        "type": "toolCall", "id": format!("call-{index}"), "name": "read",
        "arguments": { "path": format!("config-{index}.toml") },
    }));
    json!({
        "id": format!("call-row-{index}"), "type": "message", "parentId": parent,
        "message": {
            "role": "assistant", "content": content,
            "api": "anthropic-messages", "provider": "anthropic", "model": "m",
            "usage": {}, "stopReason": "toolUse", "timestamp": 2 + 2 * index,
        },
    })
}

fn result_row(index: usize, text: &str) -> Value {
    json!({
        "id": format!("result-row-{index}"), "type": "message",
        "parentId": format!("call-row-{index}"),
        "message": {
            "role": "toolResult", "toolCallId": format!("call-{index}"), "toolName": "read",
            "content": [{ "type": "text", "text": text }],
            "isError": true, "timestamp": 3 + 2 * index,
        },
    })
}

/// Under `docs/host-wire-protocol.md` §7.10.3, a `null` boundary selects all rows; a message
/// boundary selects the suffix starting at the named row.
fn window<'a>(rows: &'a [Value], boundary: &Value) -> &'a [Value] {
    let Some(mid) = boundary["mid"].as_str() else {
        return rows;
    };
    let head = rows
        .iter()
        .position(|row| row["id"] == mid)
        .expect("the boundary names a row the consumer holds");
    &rows[head..]
}

async fn pass(handler: &Handler, store: &MemoryStore, rows: &[Value], boundary: &Value) -> Value {
    let request = json!({
        "kind": "transform",
        "boundary": boundary,
        "serializer_profile": "pi",
        "session_id": "ses",
        "render_config": "cfg0",
        "mid_turn": true,
        "tool_present": true,
        "usage": ModuleUsage {
            current_total_input_tokens: 20_000,
            context_limit_tokens: 200_000,
            ..ModuleUsage::default()
        },
        "native_messages": window(rows, boundary),
    });
    let response = call_transform_request(handler, request).await;
    assert_eq!(response["status"], "ok", "{response}");
    let deadline = std::time::Instant::now() + TEST_WAIT_BUDGET;
    while store.load("ses").unwrap().meta.history_summarizer.state != HistorySummarizerPhase::Idle {
        assert!(
            std::time::Instant::now() < deadline,
            "the summarizer run did not settle"
        );
        tokio::time::sleep(TEST_WAIT_POLL).await;
    }
    response
}

fn identity_row(store: &MemoryStore, mid: &str) -> Option<(String, i64)> {
    store
        .with_conn_for_test(|conn| {
            use rusqlite::OptionalExtension;
            conn.query_row(
                "SELECT identities, scan_version FROM block_identities
                  WHERE session_id = 'ses' AND mid = ?1",
                [mid],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
        })
        .unwrap()
}

fn newest_tag_number(store: &MemoryStore) -> i64 {
    store
        .with_conn_for_test(|conn| {
            conn.query_row(
                "SELECT coalesce(max(tag_number), 0) FROM tags WHERE session_id = 'ses'",
                [],
                |row| row.get(0),
            )
        })
        .unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn a_request_loop_projects_a_capped_window_and_stores_each_message_once() {
    const REQUESTS: usize = 80;
    let producer = Arc::new(ProducerState::default());
    let (handler, store, _dir, _project) =
        handler_with_store(Arc::clone(&producer), default_test_config());

    let mut rows = vec![user_row()];
    let mut boundary = Value::Null;
    let mut projected_messages = 0;
    let mut minted_tags = 0;
    let mut widest_window = 0;
    let mut cap_folds = 0;
    let mut first_stored: BTreeMap<String, (String, i64)> = BTreeMap::new();
    let mut blocks_per_request = 0;
    for index in 0..REQUESTS {
        let parent = rows.last().unwrap()["id"].as_str().unwrap().to_string();
        rows.push(call_row(index, &parent));
        rows.push(result_row(
            index,
            "Validation failed: the path is required.",
        ));
        let response = pass(&handler, &store, &rows, &boundary).await;
        let timings = &response["timings"];
        let window_blocks = timings["ledger"]["window_blocks"].as_u64().unwrap() as usize;
        if index == 0 {
            blocks_per_request = window_blocks - 1;
            assert_eq!(blocks_per_request, TEXT_PARTS + 2);
        }
        projected_messages += timings["projection_projected_messages"].as_u64().unwrap() as usize;
        minted_tags += timings["tag_mint_new"].as_i64().unwrap();
        widest_window = widest_window.max(window_blocks);
        if response["action"] == "HARD" && window_blocks >= WINDOW_CAP_BLOCKS {
            assert_eq!(response["materialize_reason"], "coverage_fold");
            cap_folds += 1;
        }
        boundary = response["boundary"].clone();
        for row in window(&rows, &boundary) {
            let mid = row["id"].as_str().unwrap();
            if !first_stored.contains_key(mid)
                && let Some(stored) = identity_row(&store, mid)
            {
                first_stored.insert(mid.to_string(), stored);
            }
        }
    }

    assert!(REQUESTS * blocks_per_request > 3 * WINDOW_CAP_BLOCKS);
    assert!(cap_folds >= 2, "{cap_folds}");
    let widest_allowed = WINDOW_CAP_BLOCKS + 2 * blocks_per_request;
    assert!(widest_window <= widest_allowed, "{widest_window}");
    let messages_per_window = 2 * widest_allowed.div_ceil(blocks_per_request);
    assert!(
        projected_messages <= REQUESTS * messages_per_window,
        "{projected_messages}"
    );

    assert!(minted_tags >= REQUESTS as i64);
    assert_eq!(newest_tag_number(&store), minted_tags);
    let last_window: Vec<String> = window(&rows, &boundary)
        .iter()
        .map(|row| row["id"].as_str().unwrap().to_string())
        .collect();
    assert!(last_window.len() > 4);
    let unchanged = |store: &MemoryStore, mids: &[String]| {
        for mid in mids {
            assert_eq!(
                identity_row(store, mid).as_ref(),
                first_stored.get(mid),
                "{mid}"
            );
        }
    };
    unchanged(&store, &last_window[1..]);

    for _ in 0..3 {
        let response = pass(&handler, &store, &rows, &boundary).await;
        assert_eq!(response["timings"]["tag_mint_new"], 0);
        assert_eq!(response["boundary"], boundary);
    }
    assert_eq!(newest_tag_number(&store), minted_tags);
    unchanged(&store, &last_window[1..]);

    let edited = REQUESTS - 2;
    let edited_mid = format!("result-row-{edited}");
    assert!(last_window[1..].contains(&edited_mid));
    let at = rows
        .iter()
        .position(|row| row["id"] == edited_mid.as_str())
        .unwrap();
    rows[at] = result_row(edited, "The file holds twelve keys.");
    let response = pass(&handler, &store, &rows, &boundary).await;
    let (identities, written) = identity_row(&store, &edited_mid).unwrap();
    let (first_identities, first_written) = &first_stored[&edited_mid];
    assert_ne!(&identities, first_identities);
    assert!(written > *first_written);
    let served = serde_json::to_string(&response["native_messages"]).unwrap();
    assert!(served.contains("The file holds twelve keys."), "{response}");
    unchanged(&store, &[format!("result-row-{}", REQUESTS - 1)]);
}

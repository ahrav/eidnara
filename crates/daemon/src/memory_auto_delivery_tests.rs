//! Tests for automatic delivery of captured user statements. The capture helper models the
//! host's native extractor through handler calls.

use super::*;

const FIRST: u16 = 21;
const SECOND: u16 = 22;
const THIRD: u16 = 23;
const SWITCHED_OFF: u16 = 24;
const OTHER_PROJECT: u16 = 25;

const THURSDAY: &str = "Release branches are cut from main every Thursday.";
const TUESDAY: &str = "We moved the release cut from Thursday to Tuesday.";
const PNPM: &str = "Use pnpm for installs in this repository.";
const ASSISTANT_NOTE: &str = "The integration suite takes about four minutes on CI.";

async fn delivery_handler(
    config: DaemonConfig,
) -> (Handler, Arc<MemoryStore>, tempfile::TempDir, PathBuf) {
    let (handler, store, dir, project) =
        handler_with_store_and_kernel(Arc::new(ProducerState::default()), config.clone()).await;
    for (channel, session) in [(FIRST, "s1"), (SECOND, "s2"), (THIRD, "s3")] {
        bind(&handler, channel, &project, session, config.clone());
    }
    (handler, store, dir, project)
}

fn bind(handler: &Handler, channel: u16, project: &Path, session: &str, config: DaemonConfig) {
    handler.bind_route(
        test_route(channel),
        SessionBinding {
            config,
            ..binding_with_harness(project.to_str().unwrap(), "pi", session)
        },
    );
}

fn capture_config() -> DaemonConfig {
    DaemonConfig {
        memory_auto_capture: true,
        ..default_test_config()
    }
}

struct Extracted {
    quote: String,
    replaces: Vec<String>,
}

fn fact(quote: &str) -> Extracted {
    Extracted {
        quote: quote.to_string(),
        replaces: Vec::new(),
    }
}

async fn capture(
    handler: &Handler,
    channel: u16,
    session: &str,
    messages: &[(&str, &str, &str)],
    extract: impl Fn(&str, &[Value]) -> Vec<Extracted>,
) {
    let request = json!({
        "method": "memory.capture",
        "session_id": session,
        "model": "test/model",
        "messages": messages
            .iter()
            .map(|(id, role, text)| json!({"id": id, "role": role, "text": text}))
            .collect::<Vec<_>>(),
    });
    assert_eq!(
        body(
            handler
                .handle_memory_capture(test_route(channel), &request)
                .await
        )["state"],
        "accepted"
    );
    let next = json!({"method": "memory.capture.next", "session_id": session, "model": "custom-native/model"});
    let mut unseen: Vec<&str> = messages.iter().map(|(_, _, text)| *text).collect();
    while !unseen.is_empty() {
        let work = body(
            handler
                .handle_native_capture_next(test_route(channel), &next)
                .await,
        );
        assert_eq!(work["state"], "work", "{work}");
        let prompt: Value = serde_json::from_str(work["prompt"].as_str().unwrap()).unwrap();
        let existing = prompt["existing_memories"].as_array().unwrap().clone();
        let decisions: Vec<Value> = prompt["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|message| {
                let text = message["text"].as_str().unwrap();
                let ours = unseen.iter().position(|unseen| *unseen == text);
                let memories: Vec<Value> = ours
                    .map(|index| extract(unseen.remove(index), &existing))
                    .unwrap_or_default()
                    .into_iter()
                    .map(|memory| {
                        let mut value = json!({
                            "category": "PROJECT_RULES",
                            "content": format!("A project fact: {}", memory.quote),
                            "quote": memory.quote,
                        });
                        if let Some((first, rest)) = memory.replaces.split_first() {
                            value["replaces"] = json!(first);
                            if !rest.is_empty() {
                                value["also_replaces"] = json!(rest);
                            }
                        }
                        value
                    })
                    .collect();
                json!({"message_id": message["id"], "memories": memories})
            })
            .collect();
        let submission = json!({
            "method": "memory.capture.submit",
            "session_id": session,
            "lease": work["lease"],
            "model": work["model"],
            "output": json!({"version": 1, "decisions": decisions}).to_string(),
        });
        assert_eq!(
            body(
                handler
                    .handle_native_capture_submit(test_route(channel), &submission)
                    .await
            )["state"],
            "processed"
        );
    }
}

fn body(outcome: PreparedOutcome) -> Value {
    match outcome {
        PreparedOutcome::Error { code, message } => panic!("{code}: {message}"),
        other => tool_body(other),
    }
}

fn ids_containing(existing: &[Value], needle: &str) -> Vec<String> {
    existing
        .iter()
        .filter(|memory| memory["content"].as_str().unwrap().contains(needle))
        .map(|memory| memory["id"].as_str().unwrap().to_string())
        .collect()
}

async fn serve(handler: &Handler, channel: u16, session: &str) -> Value {
    let messages = vec![
        ck("00000001", 1, "Please tidy the README headings."),
        wire_with_role(
            "00000002",
            2,
            "assistant",
            "Done; the headings are consistent.",
        ),
    ];
    let mut request = request_with_usage(messages, 1_000, 200_000);
    request["session_id"] = json!(session);
    call_transform_request_on_channel(handler, channel, request).await
}

fn project_memory_block(response: &Value) -> Option<String> {
    let served = serde_json::to_string(response).unwrap();
    let start = served.find("<project-memory>")?;
    let end = served[start..].find("</project-memory>")? + start;
    Some(served[start..end].replace("\\n", "\n"))
}

async fn explicit_search(
    handler: &Handler,
    (channel, session): (u16, &str),
    project: &Path,
) -> Vec<(String, String)> {
    let request = kernel_route_fixtures::read_request(project, session, "explicit_search", None);
    let body = body(
        handler
            .dispatch_value_for_test(test_route(channel), request)
            .await,
    );
    assert_eq!(body["state"]["kind"], "available", "{body}");
    body["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["object"]["domain_id"] == "memory")
        .map(|row| {
            (
                row["decision"]["payload"]["summary"]
                    .as_str()
                    .unwrap()
                    .to_string(),
                row["visibility"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn stated(quote: &str) -> String {
    format!("User stated: {quote}")
}

#[tokio::test]
async fn a_user_statement_reaches_the_next_session_and_a_correction_replaces_it() {
    let (handler, _store, _dir, project) = delivery_handler(capture_config()).await;
    let before = serve(&handler, SECOND, "s2").await;
    assert_eq!(project_memory_block(&before), None, "{before}");

    capture(
        &handler,
        FIRST,
        "s1",
        &[(
            "m1",
            "user",
            &format!("{THURSDAY} Can you look at the flaky upload test?"),
        )],
        |_, _| vec![fact(THURSDAY)],
    )
    .await;

    let own = serve(&handler, FIRST, "s1").await;
    assert_eq!(
        project_memory_block(&own),
        None,
        "the capturing session keeps its statement in its conversation: {own}"
    );
    let delivered = serve(&handler, THIRD, "s3").await;
    let block = project_memory_block(&delivered).expect("a later session carries the block");
    assert!(block.contains(&stated(THURSDAY)), "{block}");

    capture(
        &handler,
        SECOND,
        "s2",
        &[("m2", "user", TUESDAY)],
        |_, existing| {
            vec![Extracted {
                quote: TUESDAY.to_string(),
                replaces: ids_containing(existing, "Thursday"),
            }]
        },
    )
    .await;

    let refreshed = serve(&handler, THIRD, "s3").await;
    let block = project_memory_block(&refreshed).expect("the correction is delivered");
    assert!(block.contains(&stated(TUESDAY)), "{block}");
    assert!(
        !block.contains(&stated(THURSDAY)),
        "the replaced value is never delivered as current: {block}"
    );
    assert_eq!(
        explicit_search(&handler, (THIRD, "s3"), &project).await,
        [(stated(TUESDAY), "visible".to_string())],
        "explicit search serves the correction record, which names both values"
    );
}

#[tokio::test]
async fn a_correction_retires_every_named_restatement_of_the_old_value() {
    let echo = "Noted: release branches are cut from main every Thursday.";
    let (handler, _store, _dir, project) = delivery_handler(capture_config()).await;
    capture(
        &handler,
        FIRST,
        "s1",
        &[("m1", "user", THURSDAY), ("m2", "assistant", echo)],
        |text, _| vec![fact(if text == echo { echo } else { THURSDAY })],
    )
    .await;
    assert_eq!(
        explicit_search(&handler, (FIRST, "s1"), &project)
            .await
            .len(),
        2
    );

    capture(
        &handler,
        SECOND,
        "s2",
        &[("m3", "user", TUESDAY)],
        |_, existing| {
            vec![Extracted {
                quote: TUESDAY.to_string(),
                replaces: ids_containing(existing, "every Thursday."),
            }]
        },
    )
    .await;
    assert_eq!(
        explicit_search(&handler, (SECOND, "s2"), &project).await,
        [(stated(TUESDAY), "visible".to_string())],
        "the user's earlier statement and the assistant's echo of it are both retired"
    );
}

#[tokio::test]
async fn assistant_reports_and_sentence_fragments_stay_on_explicit_search() {
    let negated = "Never use pnpm for installs in this repository. Use npm.";
    let fragment = "use pnpm for installs in this repository.";
    let (handler, _store, _dir, project) = delivery_handler(capture_config()).await;
    capture(
        &handler,
        FIRST,
        "s1",
        &[("m1", "assistant", ASSISTANT_NOTE), ("m2", "user", negated)],
        |text, _| {
            vec![fact(if text == ASSISTANT_NOTE {
                ASSISTANT_NOTE
            } else {
                fragment
            })]
        },
    )
    .await;

    let served = serve(&handler, SECOND, "s2").await;
    assert_eq!(project_memory_block(&served), None, "{served}");
    let mut rows = explicit_search(&handler, (SECOND, "s2"), &project).await;
    rows.sort();
    assert_eq!(
        rows,
        [
            (
                format!("Assistant reported: {ASSISTANT_NOTE}"),
                "labeled".to_string()
            ),
            (stated(fragment), "labeled".to_string()),
        ],
        "both stay retrievable, labeled as inference"
    );
}

#[tokio::test]
async fn switching_auto_inject_off_delivers_no_block_and_search_still_finds_the_statement() {
    let (handler, _store, _dir, project) = delivery_handler(capture_config()).await;
    bind(
        &handler,
        SWITCHED_OFF,
        &project,
        "s-off",
        DaemonConfig {
            memory_auto_inject: false,
            ..capture_config()
        },
    );
    capture(&handler, FIRST, "s1", &[("m1", "user", PNPM)], |_, _| {
        vec![fact(PNPM)]
    })
    .await;

    let on = serve(&handler, SECOND, "s2").await;
    assert!(
        project_memory_block(&on).is_some_and(|block| block.contains(&stated(PNPM))),
        "the session with injection on carries the block: {on}"
    );
    let off = serve(&handler, SWITCHED_OFF, "s-off").await;
    assert_eq!(project_memory_block(&off), None, "{off}");
    assert_eq!(
        explicit_search(&handler, (SWITCHED_OFF, "s-off"), &project).await,
        [(stated(PNPM), "visible".to_string())]
    );
}

#[tokio::test]
async fn another_projects_statement_is_not_delivered() {
    let (handler, _store, dir, project) = delivery_handler(capture_config()).await;
    let other = dir.path().join("other-project");
    std::fs::create_dir_all(&other).unwrap();
    bind(&handler, OTHER_PROJECT, &other, "s-other", capture_config());
    capture(
        &handler,
        OTHER_PROJECT,
        "s-other",
        &[("m1", "user", PNPM)],
        |_, _| vec![fact(PNPM)],
    )
    .await;
    capture(
        &handler,
        FIRST,
        "s1",
        &[("m2", "user", THURSDAY)],
        |_, _| vec![fact(THURSDAY)],
    )
    .await;

    let block = project_memory_block(&serve(&handler, SECOND, "s2").await)
        .expect("this project's own statement is delivered");
    assert!(block.contains(&stated(THURSDAY)), "{block}");
    assert!(!block.contains(PNPM), "{block}");
    assert_eq!(
        explicit_search(&handler, (SECOND, "s2"), &project).await,
        [(stated(THURSDAY), "visible".to_string())]
    );
}

#[tokio::test]
async fn a_user_correction_retires_a_memory_the_host_memory_tool_wrote() {
    let (handler, _store, _dir, project) = delivery_handler(capture_config()).await;
    let written = body(
        handler
            .dispatch_value_for_test(
                test_route(FIRST),
                kernel_route_fixtures::commit_request(
                    &project,
                    "s1",
                    "agent-write",
                    vec![json!({"op": "insert_decision", "spec": {
                        "decision_id": "dec-agent",
                        "object_id": "mem-agent",
                        "domain_id": "memory",
                        "decision_kind": "PROJECT_RULES",
                        "payload": {"summary": "Releases are cut on Thursdays.", "rationale": ""},
                        "source_id": "eidnara_memory",
                        "source_revision": 1,
                    }})],
                    Vec::new(),
                ),
            )
            .await,
    );
    assert_eq!(written["state"]["kind"], "available", "{written}");
    capture(
        &handler,
        SECOND,
        "s2",
        &[("m1", "user", TUESDAY)],
        |_, existing| {
            let replaces = ids_containing(existing, "Thursdays");
            assert_eq!(
                replaces,
                ["mem-agent"],
                "the agent's memory is offered as replaceable"
            );
            assert!(existing[0]["can_replace"].as_bool().unwrap());
            vec![Extracted {
                quote: TUESDAY.to_string(),
                replaces,
            }]
        },
    )
    .await;
    assert_eq!(
        explicit_search(&handler, (SECOND, "s2"), &project).await,
        [(stated(TUESDAY), "visible".to_string())]
    );
}

#[tokio::test]
async fn the_host_memory_tool_cannot_retire_a_user_statement() {
    let (handler, _store, _dir, project) = delivery_handler(capture_config()).await;
    capture(
        &handler,
        FIRST,
        "s1",
        &[("m1", "user", PNPM), ("m2", "assistant", ASSISTANT_NOTE)],
        |text, _| vec![fact(if text == PNPM { PNPM } else { ASSISTANT_NOTE })],
    )
    .await;
    let kernel = handler.kernel.kernel_store().unwrap();
    let read = kernel_routes::read::read_visible(
        &kernel,
        &kernel_routes::ProjectBinding::new(&project),
        kernel::Surface::ExplicitSearch,
        None,
        kernel_routes::read::RowSelection::DomainDecisions("memory"),
    )
    .unwrap();
    let id_of = |summary: &str| {
        read.decisions
            .values()
            .find(|decision| decision.payload.summary == summary)
            .unwrap()
            .object_id
            .clone()
    };
    let (user, assistant) = (
        id_of(&stated(PNPM)),
        id_of(&format!("Assistant reported: {ASSISTANT_NOTE}")),
    );
    let retire = |key: &str, object_id: &str| {
        kernel_route_fixtures::commit_request(
            &project,
            "s1",
            key,
            vec![json!({"op": "retire_decision", "object_id": object_id})],
            Vec::new(),
        )
    };

    let refused = body(
        handler
            .dispatch_value_for_test(test_route(FIRST), retire("retire-user", &user))
            .await,
    );
    assert_eq!(refused["state"]["kind"], "invalid", "{refused}");
    assert_eq!(refused["state"]["reason"], "admission_policy", "{refused}");
    let revise = kernel_route_fixtures::commit_request(
        &project,
        "s1",
        "revise-user",
        vec![
            json!({"op": "supersede_decision", "replaced_object_id": user, "spec": {
                "decision_id": "dec-revised",
                "object_id": "mem-revised",
                "domain_id": "memory",
                "decision_kind": "PROJECT_RULES",
                "payload": {"summary": "Use npm for installs.", "rationale": ""},
                "source_id": "memory_capture",
                "source_revision": 2,
            }}),
        ],
        Vec::new(),
    );
    let refused = body(
        handler
            .dispatch_value_for_test(test_route(FIRST), revise)
            .await,
    );
    assert_eq!(refused["state"]["reason"], "admission_policy", "{refused}");
    let allowed = body(
        handler
            .dispatch_value_for_test(test_route(FIRST), retire("retire-assistant", &assistant))
            .await,
    );
    assert_eq!(
        allowed["state"]["kind"], "available",
        "the tool may retire model inference: {allowed}"
    );
    assert_eq!(
        explicit_search(&handler, (FIRST, "s1"), &project).await,
        [(stated(PNPM), "visible".to_string())]
    );
}

#[tokio::test]
async fn a_long_session_carries_one_bounded_block_that_changes_only_with_other_sessions() {
    const LONG: u16 = 30;
    const SUBJECTS: [&str; 12] = [
        "The billing worker",
        "The search indexer",
        "The upload API",
        "The admin console",
        "The nightly export",
        "The auth gateway",
        "The metrics pipeline",
        "The mobile client",
        "The docs site",
        "The release CLI",
        "The webhook relay",
        "The cache warmer",
    ];
    const RULES: [&str; 5] = [
        "reads its settings from the config directory at startup",
        "logs in JSON with a request id on every line",
        "is owned by the platform team, which is paged after two failed retries",
        "targets Node 20 and pins every dependency to an exact version",
        "runs its integration tests against the shared staging database",
    ];
    let config = capture_config();
    let budget = config.memory_budget_tokens;
    let (handler, _store, _dir, project) = delivery_handler(config.clone()).await;
    for (index, subject) in SUBJECTS.iter().enumerate() {
        let channel = 40 + index as u16;
        let session = format!("earlier-{index}");
        bind(&handler, channel, &project, &session, config.clone());
        let statements: Vec<(String, String)> = RULES
            .iter()
            .enumerate()
            .map(|(at, rule)| (format!("e{index}-{at}"), format!("{subject} {rule}.")))
            .collect();
        let messages: Vec<(&str, &str, &str)> = statements
            .iter()
            .map(|(id, text)| (id.as_str(), "user", text.as_str()))
            .collect();
        capture(&handler, channel, &session, &messages, |text, _| {
            vec![fact(text)]
        })
        .await;
    }
    bind(&handler, LONG, &project, "long", config.clone());

    let mut conversation = Vec::new();
    let mut rows = Vec::new();
    for turn in 1..=40u64 {
        let ask = if turn % 8 == 0 {
            format!("From now on the release notes for sprint {turn} go in docs/notes.md.")
        } else {
            format!(
                "Next step {turn}: rename the helper in the upload handler, keep its behaviour, and update the two call sites and their tests."
            )
        };
        conversation.push(ck(&format!("{:08x}", turn * 2 - 1), turn * 2 - 1, &ask));
        conversation.push(wire_with_role(
            &format!("{:08x}", turn * 2),
            turn * 2,
            "assistant",
            &format!(
                "Step {turn} is done. I renamed the helper, updated both call sites, and the handler tests pass locally; nothing else in the module referenced the old name, so the change stays contained to the upload path."
            ),
        ));
        if turn % 8 == 0 {
            capture(
                &handler,
                LONG,
                "long",
                &[(&format!("own-{turn}"), "user", &ask)],
                |text, _| vec![fact(text)],
            )
            .await;
        }
        if turn == 20 {
            capture(
                &handler,
                FIRST,
                "s1",
                &[("concurrent", "user", PNPM)],
                |_, _| vec![fact(PNPM)],
            )
            .await;
        }
        let mut request = request_with_usage(conversation.clone(), 6_000 + turn * 400, 200_000);
        request["session_id"] = json!("long");
        let response = call_transform_request_on_channel(&handler, LONG, request).await;
        let block = project_memory_block(&response).expect("every pass carries the block");
        rows.push((
            turn,
            crate::token_cache::cached_estimate_tokens(&block),
            block,
            response["action"].as_str().unwrap_or_default().to_string(),
            response["materialize_reason"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        ));
    }

    eprintln!("turn  block_tokens  action  materialize_reason");
    for (turn, tokens, _, action, reason) in &rows {
        eprintln!("{turn:>4}  {tokens:>12}  {action:<6}  {reason}");
    }
    let first = &rows[0].2;
    assert_eq!(first.matches("User stated: ").count(), 60, "{first}");
    for (turn, tokens, block, ..) in &rows {
        assert!((*tokens as f64) <= budget, "turn {turn}: {tokens} tokens");
        assert!(
            !block.contains("docs/notes.md"),
            "turn {turn}: the session's own statements stay in its conversation"
        );
        assert_eq!(
            block.contains(PNPM),
            *turn >= 20,
            "turn {turn}: another session's statement arrives once it is published"
        );
        let expected = if *turn < 20 { &rows[0].2 } else { &rows[19].2 };
        assert_eq!(block, expected, "turn {turn}");
    }
    let rematerialized: Vec<u64> = rows
        .iter()
        .filter(|(.., reason)| reason == "project_memory_epoch")
        .map(|(turn, ..)| *turn)
        .collect();
    assert_eq!(
        rematerialized,
        [20],
        "only another session's publication rewrites the cached block"
    );
}

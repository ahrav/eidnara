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

/// Queues `messages` as capture sources and returns their job ids.
async fn enqueue(
    handler: &Handler,
    channel: u16,
    session: &str,
    messages: &[(&str, &str, &str)],
) -> Vec<String> {
    let request = json!({
        "method": "memory.capture",
        "session_id": session,
        "model": "test/model",
        "messages": messages
            .iter()
            .map(|(id, role, text)| json!({"id": id, "role": role, "text": text}))
            .collect::<Vec<_>>(),
    });
    let accepted = body(
        handler
            .handle_memory_capture(test_route(channel), &request)
            .await,
    );
    assert_eq!(accepted["state"], "accepted", "{accepted}");
    accepted["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|job| job.as_str().unwrap().to_string())
        .collect()
}

async fn capture(
    handler: &Handler,
    channel: u16,
    session: &str,
    messages: &[(&str, &str, &str)],
    extract: impl Fn(&str, &[Value]) -> Vec<Extracted>,
) {
    enqueue(handler, channel, session, messages).await;
    drain(handler, channel, session, messages, extract).await;
}

/// Runs the native drain until every one of `messages` has been extracted.
async fn drain(
    handler: &Handler,
    channel: u16,
    session: &str,
    messages: &[(&str, &str, &str)],
    extract: impl Fn(&str, &[Value]) -> Vec<Extracted>,
) {
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
    assert_eq!(
        live_statement_records(&handler, &project),
        [stated(TUESDAY)],
        "the replaced statement's approval is retired with it"
    );
}

fn live_statement_records(handler: &Handler, project: &Path) -> Vec<String> {
    let read = kernel_routes::read::read_visible(
        &handler.kernel.kernel_store().unwrap(),
        &kernel_routes::ProjectBinding::new(project),
        kernel::Surface::ExplicitSearch,
        None,
        kernel_routes::read::RowSelection::DomainDecisions("user-statements"),
    )
    .unwrap();
    read.decisions
        .into_values()
        .map(|decision| decision.payload.summary)
        .collect()
}

#[tokio::test]
async fn only_a_whole_user_statement_replaces_a_delivered_statement() {
    let (handler, _store, _dir, project) = delivery_handler(capture_config()).await;
    capture(
        &handler,
        FIRST,
        "s1",
        &[("m1", "user", THURSDAY)],
        |_, _| vec![fact(THURSDAY)],
    )
    .await;
    let fragment = "the release cut from Thursday to Tuesday.";
    capture(
        &handler,
        SECOND,
        "s2",
        &[("m2", "user", TUESDAY)],
        |_, existing| {
            vec![Extracted {
                quote: fragment.to_string(),
                replaces: ids_containing(existing, "Thursday"),
            }]
        },
    )
    .await;
    assert_eq!(
        explicit_search(&handler, (SECOND, "s2"), &project).await,
        [(stated(THURSDAY), "visible".to_string())],
        "a fragment cannot retire the user's statement"
    );
    assert_eq!(
        live_statement_records(&handler, &project),
        [stated(THURSDAY)]
    );
    let next = json!({"method": "memory.capture.next", "session_id": "s2", "model": "custom-native/model"});
    assert_eq!(
        body(
            handler
                .handle_native_capture_next(test_route(SECOND), &next)
                .await
        )["state"],
        "pending",
        "the refused source backs off as a failed extraction instead of re-planning at once"
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
    capture(
        &handler,
        FIRST,
        "s1",
        &[("m2", "user", THURSDAY)],
        |_, _| vec![fact(THURSDAY)],
    )
    .await;
    let on = serve(&handler, SECOND, "s2").await;
    assert_eq!(on["materialize_reason"], "project_memory_epoch", "{on}");
    let off = serve(&handler, SWITCHED_OFF, "s-off").await;
    assert_ne!(
        off["materialize_reason"], "project_memory_epoch",
        "a publication rewrites no m0 that carries no memory: {off}"
    );
    assert_eq!(project_memory_block(&off), None, "{off}");
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
    bind(
        &handler,
        OTHER_PROJECT + 1,
        &other,
        "s-other-2",
        capture_config(),
    );
    let theirs = project_memory_block(&serve(&handler, OTHER_PROJECT + 1, "s-other-2").await)
        .expect("the other project's statement is delivered in its own project");
    assert!(theirs.contains(&stated(PNPM)), "{theirs}");
    assert!(!theirs.contains(THURSDAY), "{theirs}");
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
    let approval = user.replacen("mem_", "stm_", 1);
    assert_eq!(live_statement_records(&handler, &project), [stated(PNPM)]);
    let refused = body(
        handler
            .dispatch_value_for_test(test_route(FIRST), retire("retire-approval", &approval))
            .await,
    );
    assert_eq!(
        refused["state"]["reason"], "admission_policy",
        "the statement record is explicit user text: {refused}"
    );
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
    let rejected = body(
        handler
            .dispatch_value_for_test(
                test_route(FIRST),
                kernel_route_fixtures::commit_request(
                    &project,
                    "s1",
                    "reject-user",
                    vec![
                        json!({"op": "disposition", "object_id": user, "event": "explicit_reject"}),
                    ],
                    Vec::new(),
                ),
            )
            .await,
    );
    assert_eq!(rejected["state"]["kind"], "available", "{rejected}");
    let refused = body(
        handler
            .dispatch_value_for_test(test_route(FIRST), retire("retire-rejected", &user))
            .await,
    );
    assert_eq!(
        refused["state"]["reason"], "admission_policy",
        "a disposition that drops the approval leaves the statement protected: {refused}"
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

fn numbered(count: usize, subject: &str) -> Vec<(String, String)> {
    (0..count)
        .map(|index| {
            (
                format!("{subject}-{index}"),
                format!("The {subject} service number {index} listens on its own port."),
            )
        })
        .collect()
}

async fn capture_statements(
    handler: &Handler,
    channel: u16,
    session: &str,
    statements: &[(String, String)],
) {
    for batch in statements.chunks(16) {
        let messages: Vec<(&str, &str, &str)> = batch
            .iter()
            .map(|(id, text)| (id.as_str(), "user", text.as_str()))
            .collect();
        capture(handler, channel, session, &messages, |text, _| {
            vec![fact(text)]
        })
        .await;
    }
}

fn capture_counter(handler: &Handler, name: &str) -> u64 {
    handler.source_capture.report(false, 0)[name]
        .as_u64()
        .unwrap()
}

/// The Kernel bounds the authority-bearing statements of one lineage at 64.
/// Each user message is its own lineage, so three projects sharing one data
/// home hold 70 statements each, every one approved and searchable, and the
/// block delivers the newest that fit `memory.injection_budget_tokens`.
#[tokio::test]
async fn hundreds_of_statements_across_projects_are_all_approved_and_delivered_within_the_budget() {
    const PER_PROJECT: usize = 70;
    const SMALL_BUDGET: f64 = 500.0;
    let config = capture_config();
    let (handler, store, dir, project) = delivery_handler(config.clone()).await;
    let mut projects = vec![("billing", project.clone())];
    for subject in ["search", "upload"] {
        let root = dir.path().join(format!("{subject}-project"));
        std::fs::create_dir_all(&root).unwrap();
        projects.push((subject, root));
    }
    for (index, (subject, root)) in projects.iter().enumerate() {
        let channel = 40 + index as u16;
        let session = format!("{subject}-writer");
        bind(&handler, channel, root, &session, config.clone());
        capture_statements(&handler, channel, &session, &numbered(PER_PROJECT, subject)).await;
    }
    assert_eq!(capture_counter(&handler, "decisions_approval_refused"), 0);
    for (index, (subject, root)) in projects.iter().enumerate() {
        let status = store
            .memory_capture_status(kernel_routes::ProjectBinding::new(root).digest())
            .unwrap();
        assert_eq!(
            (status.pending, status.failed, status.completed),
            (0, 0, PER_PROJECT as i64),
            "{subject}: every source committed"
        );
        let (reader, session) = (50 + index as u16, format!("{subject}-reader"));
        bind(&handler, reader, root, &session, config.clone());
        let rows = explicit_search(&handler, (reader, &session), root).await;
        assert_eq!(rows.len(), PER_PROJECT, "{subject}");
        assert!(
            rows.iter().all(|(_, visibility)| visibility == "visible"),
            "{subject}: every statement is approved"
        );
        assert_eq!(live_statement_records(&handler, root).len(), PER_PROJECT);

        let block = project_memory_block(&serve(&handler, reader, &session).await)
            .expect("a later session carries the block");
        assert!(
            crate::token_cache::cached_estimate_tokens(&block) as f64
                <= config.memory_budget_tokens,
            "{subject}: {block}"
        );
        let delivered = block.matches("User stated: ").count();
        assert!(
            delivered > 64,
            "{subject}: the default budget delivers more statements than one lineage holds, got {delivered}"
        );
        for index in 0..PER_PROJECT {
            assert_eq!(
                block.contains(&format!("The {subject} service number {index} listens")),
                index >= PER_PROJECT - delivered,
                "{subject}: the block holds the newest {delivered} statements, checked at {index}"
            );
        }
        assert!(!block.contains(projects[(index + 1) % 3].0), "{block}");

        let (tight, session) = (60 + index as u16, format!("{subject}-tight"));
        bind(
            &handler,
            tight,
            root,
            &session,
            DaemonConfig {
                memory_budget_tokens: SMALL_BUDGET,
                ..config.clone()
            },
        );
        let block = project_memory_block(&serve(&handler, tight, &session).await)
            .expect("a smaller budget still carries a block");
        assert!(
            crate::token_cache::cached_estimate_tokens(&block) as f64 <= SMALL_BUDGET,
            "{subject}: {block}"
        );
        let newest = format!("The {subject} service number {} listens", PER_PROJECT - 1);
        let oldest = format!("The {subject} service number 0 listens");
        assert!(block.contains(&newest), "{block}");
        assert!(
            !block.contains(&oldest),
            "the budget keeps the newest statements: {block}"
        );
    }
}

/// Fills the statement lineage of capture job `job` with 64 accepted records,
/// the Kernel's bound for one lineage, so the job's own statement is refused.
fn fill_statement_lineage(handler: &Handler, project: &Path, job: &str) {
    let kernel = handler.kernel.kernel_store().unwrap();
    let binding = kernel_routes::ProjectBinding::new(project);
    let source_id = format!("memory_capture_statement:{}", &job[..32]);
    kernel
        .commit_before(
            Instant::now() + Duration::from_secs(5),
            kernel::CommitIntent {
                producer: "test".into(),
                operation_key: binding.operation_key("test", "fill-lineage").unwrap(),
                request_digest: "0".repeat(64),
                actor: "test".into(),
                cause: "fill one statement lineage".into(),
            },
            |envelope| {
                let (mut domains, mut ready, mut refusal) = (HashSet::new(), false, None);
                kernel_routes::commit::ensure_domain(envelope, "user-statements", &mut domains)?;
                kernel_routes::commit::ensure_scope(
                    envelope,
                    &binding,
                    &mut ready,
                    &mut domains,
                    &mut refusal,
                )?;
                for index in 0..64 {
                    let id = format!("stm_fill{index}");
                    envelope.insert_decision(kernel::DecisionSpec {
                        decision_id: format!("dst_fill{index}"),
                        object_id: id.clone(),
                        domain_id: "user-statements".into(),
                        proposition_id: None,
                        scope_id: Some(binding.scope_id()),
                        anchor_id: None,
                        evidence_id: None,
                        decision_kind: "adr_accepted".into(),
                        payload: kernel::DecisionPayload {
                            summary: format!("Earlier statement {index}."),
                            rationale: "{}".into(),
                        },
                        source_kind: "user".into(),
                        source_id: source_id.clone(),
                        source_revision: 1,
                        sensitivity: kernel::Sensitivity::Normal,
                    })?;
                    kernel_routes::commit::accept_statement(envelope, &id)?;
                }
                Ok("64".into())
            },
        )
        .unwrap();
}

#[tokio::test]
async fn a_statement_whose_approval_the_kernel_refuses_is_stored_for_explicit_search_and_counted() {
    let (handler, store, _dir, project) = delivery_handler(capture_config()).await;
    let messages = [("m1", "user", THURSDAY), ("m2", "user", PNPM)];
    let jobs = enqueue(&handler, FIRST, "s1", &messages).await;
    fill_statement_lineage(&handler, &project, &jobs[0]);
    drain(&handler, FIRST, "s1", &messages, |text, _| vec![fact(text)]).await;

    let mut rows = explicit_search(&handler, (SECOND, "s2"), &project).await;
    rows.sort();
    assert_eq!(
        rows,
        [
            (stated(THURSDAY), "labeled".to_string()),
            (stated(PNPM), "visible".to_string()),
        ],
        "the refused statement is stored for explicit search; the other is approved"
    );
    assert_eq!(capture_counter(&handler, "decisions_approval_refused"), 1);
    let status = store
        .memory_capture_status(kernel_routes::ProjectBinding::new(&project).digest())
        .unwrap();
    assert_eq!(
        (status.pending, status.failed, status.completed),
        (0, 0, 2),
        "the refused approval leaves no source waiting"
    );
    let block = project_memory_block(&serve(&handler, SECOND, "s2").await)
        .expect("the approved statement is delivered");
    assert!(block.contains(&stated(PNPM)), "{block}");
    assert!(
        !block.contains(&stated(THURSDAY)),
        "a statement without its approval stays off the automatic surface: {block}"
    );
}

/// Forty frozen plans the publication step refuses are ripe when a new source
/// arrives. The next drain call issues the new source: refused plans wait in
/// their own window of the drain.
#[tokio::test]
async fn a_later_source_is_issued_in_the_next_drain_while_refused_plans_wait() {
    let (handler, store, _dir, project) = delivery_handler(capture_config()).await;
    let digest = kernel_routes::ProjectBinding::new(&project)
        .digest()
        .to_owned();
    let refused = numbered(40, "billing");
    for batch in refused.chunks(20) {
        let messages: Vec<(&str, &str, &str)> = batch
            .iter()
            .map(|(id, text)| (id.as_str(), "user", text.as_str()))
            .collect();
        for job in enqueue(&handler, FIRST, "s1", &messages).await {
            assert!(
                store
                    .begin_memory_capture_attempt(&digest, &job, 0)
                    .unwrap()
            );
            let frozen = store
                .prepare_memory_capture(&digest, &job, r#"{"version":0}"#)
                .unwrap();
            assert!(frozen.is_some(), "the plan is frozen");
        }
    }
    enqueue(
        &handler,
        SECOND,
        "s2",
        &[("later", "assistant", ASSISTANT_NOTE)],
    )
    .await;

    let next = json!({"method": "memory.capture.next", "session_id": "s2", "model": "custom-native/model"});
    let work = body(
        handler
            .handle_native_capture_next(test_route(SECOND), &next)
            .await,
    );
    assert_eq!(work["state"], "work", "{work}");
    let prompt: Value = serde_json::from_str(work["prompt"].as_str().unwrap()).unwrap();
    assert_eq!(
        prompt["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|message| message["text"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [ASSISTANT_NOTE],
        "the batch holds the new source and none of the refused plans"
    );
    let status = store.memory_capture_status(&digest).unwrap();
    assert_eq!(
        (status.pending, status.prepared, status.failed),
        (41, 40, 32),
        "one drain call attempted one window of refused plans and kept them queued"
    );
}

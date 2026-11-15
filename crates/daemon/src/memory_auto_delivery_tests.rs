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

#[derive(Default)]
struct Extracted {
    quote: String,
    replaces: Vec<String>,
    duplicate_of: Option<String>,
    contests: Vec<String>,
    /// The memory category; `PROJECT_RULES` when absent.
    category: Option<&'static str>,
}

fn fact(quote: &str) -> Extracted {
    Extracted {
        quote: quote.to_string(),
        ..Extracted::default()
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
                            "category": memory.category.unwrap_or("PROJECT_RULES"),
                            "content": format!("A project fact: {}", memory.quote),
                            "quote": memory.quote,
                        });
                        if let Some((first, rest)) = memory.replaces.split_first() {
                            value["replaces"] = json!(first);
                            if !rest.is_empty() {
                                value["also_replaces"] = json!(rest);
                            }
                        }
                        if let Some(target) = memory.duplicate_of {
                            value["duplicate_of"] = json!(target);
                        }
                        if !memory.contests.is_empty() {
                            value["contests"] = json!(memory.contests);
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
    Some(
        served[start..end]
            .replace("\\n", "\n")
            .replace("\\\"", "\""),
    )
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
                ..Extracted::default()
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

/// A question is no statement, so it cannot retire what the user stated. The
/// extractor names the statement as replaced anyway; the question is stored
/// for explicit search, the statement stays live, and the block shows the
/// statement as contested by the question, with both dates.
#[tokio::test]
async fn a_user_quotation_that_is_no_statement_contests_the_statement_it_names() {
    let (handler, store, _dir, project) = delivery_handler(capture_config()).await;
    capture(
        &handler,
        FIRST,
        "s1",
        &[("m1", "user", THURSDAY)],
        |_, _| vec![fact(THURSDAY)],
    )
    .await;
    let question = "Did we move the release cut from Thursday to Tuesday?";
    capture(
        &handler,
        SECOND,
        "s2",
        &[("m2", "user", question)],
        |_, existing| {
            vec![Extracted {
                quote: question.to_string(),
                replaces: ids_containing(existing, "Thursday"),
                ..Extracted::default()
            }]
        },
    )
    .await;
    let mut rows = explicit_search(&handler, (SECOND, "s2"), &project).await;
    rows.sort();
    assert_eq!(
        rows,
        [
            (stated(question), "labeled".to_string()),
            (stated(THURSDAY), "visible".to_string()),
        ],
        "the question is stored; the statement it named stays live"
    );
    assert_eq!(
        live_statement_records(&handler, &project),
        [stated(THURSDAY)]
    );
    let status = store
        .memory_capture_status(kernel_routes::ProjectBinding::new(&project).digest())
        .unwrap();
    assert_eq!(
        (status.pending, status.failed),
        (0, 0),
        "the source completes instead of failing extraction"
    );
    let block = project_memory_block(&serve(&handler, THIRD, "s3").await)
        .expect("the contested statement is delivered");
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let contested = format!(
        "{} (stated {today}; contested {today} by mem_",
        stated(THURSDAY)
    );
    assert!(block.contains(&contested), "{block}");
    assert!(
        block.contains(&format!(": \"{}\")", stated(question))),
        "{block}"
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
                ..Extracted::default()
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

/// The extractor quotes "use pnpm ..." out of "Never use pnpm ...". The
/// stored statement is the user's whole sentence, negation included. The
/// assistant's message is quoted whole and still gets no statement record.
#[tokio::test]
async fn an_assistant_report_stays_on_explicit_search_and_a_cut_quotation_stores_the_whole_sentence()
 {
    let negated = "Never use pnpm for installs in this repository. Use npm.";
    let fragment = "use pnpm for installs in this repository.";
    let whole = "Never use pnpm for installs in this repository.";
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

    let mut rows = explicit_search(&handler, (SECOND, "s2"), &project).await;
    rows.sort();
    assert_eq!(
        rows,
        [
            (
                format!("Assistant reported: {ASSISTANT_NOTE}"),
                "labeled".to_string()
            ),
            (stated(whole), "visible".to_string()),
        ],
        "the assistant's report stays labeled; the user's sentence is stored whole"
    );
    assert_eq!(live_statement_records(&handler, &project), [stated(whole)]);
    let block = project_memory_block(&serve(&handler, SECOND, "s2").await)
        .expect("the user's statement is delivered");
    assert!(block.contains(&stated(whole)), "{block}");
    assert!(!block.contains(ASSISTANT_NOTE), "{block}");
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
                ..Extracted::default()
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

const CITATION_COUNTERS: [&str; 5] = [
    "decisions_cited",
    "decisions_source_capture_off",
    "decisions_source_id_unjoinable",
    "decisions_publish_budget_expired",
    "decisions_descriptor_not_found",
];
const ANCHOR_COUNTERS: [&str; 2] = ["decisions_anchored", "decisions_head_unavailable"];

/// The project's rows on explicit search, its live statement records, and
/// the approval-refused, citation, and anchor counts.
type Published = (Vec<(String, String)>, Vec<String>, u64, u64, u64);

async fn published(handler: &Handler, project: &Path) -> Published {
    let mut rows = explicit_search(handler, (SECOND, "s2"), project).await;
    rows.sort();
    let sum = |names: &[&str]| {
        names
            .iter()
            .map(|name| capture_counter(handler, name))
            .sum()
    };
    let mut records = live_statement_records(handler, project);
    records.retain(|record| record.starts_with("User stated: "));
    records.sort();
    (
        rows,
        records,
        capture_counter(handler, "decisions_approval_refused"),
        sum(&CITATION_COUNTERS),
        sum(&ANCHOR_COUNTERS),
    )
}

/// One message states two facts and its lineage is full, so the approving
/// commit is refused and the plan commits labeled. The completion write of
/// that job fails once: the plan stays queued, and the next drain replays the
/// labeled receipt and completes the source with the same rows and counts.
#[tokio::test]
async fn a_statement_whose_approval_the_kernel_refuses_is_stored_for_explicit_search_and_counted() {
    let (handler, store, _dir, project) = delivery_handler(capture_config()).await;
    let both = format!("{THURSDAY} {TUESDAY}");
    let messages = [("m1", "user", both.as_str()), ("m2", "user", PNPM)];
    let jobs = enqueue(&handler, FIRST, "s1", &messages).await;
    fill_statement_lineage(&handler, &project, &jobs[0]);
    store
        .execute_tag_sql_for_test(&format!(
            "CREATE TRIGGER hold_plans BEFORE UPDATE ON memory_capture_jobs
             WHEN NEW.commit_seq IS NOT NULL AND NEW.job_id='{}'
             BEGIN SELECT RAISE(ABORT,'held'); END",
            jobs[0]
        ))
        .unwrap();
    drain(&handler, FIRST, "s1", &messages, |text, _| {
        if text == PNPM {
            vec![fact(PNPM)]
        } else {
            vec![fact(THURSDAY), fact(TUESDAY)]
        }
    })
    .await;

    let committed = published(&handler, &project).await;
    assert_eq!(
        committed.0,
        [
            (stated(THURSDAY), "labeled".to_string()),
            (stated(PNPM), "visible".to_string()),
            (stated(TUESDAY), "labeled".to_string()),
        ],
        "the refused message's statements are stored for explicit search; the other is approved"
    );
    assert_eq!(
        committed.1,
        [stated(PNPM)],
        "a labeled statement has no statement record"
    );
    assert_eq!(
        committed.2, 2,
        "each statement of the refused plan is counted once"
    );
    assert_eq!(
        (committed.3, committed.4),
        (3, 3),
        "the refused attempt adds no citation or anchor count: one of each per committed decision"
    );
    let digest = kernel_routes::ProjectBinding::new(&project)
        .digest()
        .to_owned();
    let held = store.memory_capture_status(&digest).unwrap();
    assert_eq!(
        (held.pending, held.prepared, held.completed),
        (1, 1, 1),
        "the labeled commit is durable and its completion write failed"
    );
    store
        .execute_tag_sql_for_test(
            "DROP TRIGGER hold_plans; UPDATE memory_capture_jobs SET retry_at_ms=0",
        )
        .unwrap();
    let next = json!({"method": "memory.capture.next", "session_id": "s1", "model": "custom-native/model"});
    let replayed = body(
        handler
            .handle_native_capture_next(test_route(FIRST), &next)
            .await,
    );
    assert_eq!(
        replayed["state"], "ready",
        "the frozen plan replays its labeled receipt and asks the model for nothing: {replayed}"
    );
    assert_eq!(
        published(&handler, &project).await,
        committed,
        "a replayed receipt writes no row, no statement record, and no count"
    );
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
    let mut plans = Vec::new();
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
            plans.push(job);
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
    let before = now_ms();
    let work = body(
        handler
            .handle_native_capture_next(test_route(SECOND), &next)
            .await,
    );
    let after = now_ms();
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
    // A plan that does not decode is refused as frozen: one dispatch, the
    // wait that follows the first dispatch, and the plan kept.
    for job in &plans[..32] {
        let (attempts, retry_at, error, abandoned, _, frozen) = queue_row(&store, job);
        assert_eq!(attempts, 2, "the refused commit is the second dispatch");
        assert_eq!(error.as_deref(), Some("invalid_preparation"));
        assert!(
            (before + 2_000..=after + 2_000).contains(&retry_at),
            "the wait after the first dispatch is two seconds: {retry_at} in {before}..{after}"
        );
        assert_eq!(abandoned, None);
        assert_eq!(frozen.as_deref(), Some(r#"{"version":0}"#));
    }
    for job in &plans[32..] {
        let (attempts, retry_at, error, ..) = queue_row(&store, job);
        assert_eq!(
            (attempts, retry_at, error),
            (1, 0, None),
            "a plan beyond the window is as it was"
        );
    }

    // The refusal on the ninth dispatch abandons the source.
    let last = plans[0].as_str();
    store
        .execute_tag_sql_for_test(&format!(
            "UPDATE memory_capture_jobs SET retry_at_ms=CASE WHEN job_id='{last}' THEN 0 ELSE {} END, attempts=CASE WHEN job_id='{last}' THEN 8 ELSE attempts END WHERE prepared_json IS NOT NULL",
            i64::MAX
        ))
        .unwrap();
    handler.native_capture.lock().unwrap().expire_all_for_test();
    body(
        handler
            .handle_native_capture_next(test_route(SECOND), &next)
            .await,
    );
    let (attempts, _, error, abandoned, text, frozen) = queue_row(&store, last);
    assert_eq!(attempts, 9);
    assert!(
        abandoned.is_some(),
        "the ninth dispatch abandons the source"
    );
    assert_eq!(error.as_deref(), Some("invalid_preparation"));
    assert_eq!((text.as_str(), frozen), ("", None));
    let status = store.memory_capture_status(&digest).unwrap();
    assert_eq!((status.pending, status.prepared), (40, 39));
}

/// Sets short publication waits, so a test that holds the gate or the Kernel
/// writer finishes in well under a second.
fn short_waits(handler: &Handler) {
    waits(
        handler,
        Duration::from_millis(300),
        Duration::from_millis(200),
    );
}

fn waits(handler: &Handler, plan_commit_budget: Duration, commit_wait: Duration) {
    handler.native_capture.lock().unwrap().set_waits_for_test(
        crate::memory_capture::CaptureWaits {
            plan_commit_budget,
            commit_wait,
        },
    );
}

fn production_waits(handler: &Handler) {
    handler
        .native_capture
        .lock()
        .unwrap()
        .set_waits_for_test(crate::memory_capture::CaptureWaits::default());
}

/// A plan that decodes and that the Kernel accepts once it has the writer.
fn statement_plan(quote: &str) -> String {
    json!({"version": 1, "model": "test/model", "memories": [{
        "category": "PROJECT_RULES",
        "content": stated(quote),
        "quote": quote,
    }]})
    .to_string()
}

/// Holds the Kernel writer on its own thread until the returned sender is
/// sent to or dropped.
fn hold_writer(
    kernel: &Arc<kernel::KernelStore>,
) -> (std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>) {
    let (release, released) = std::sync::mpsc::channel::<()>();
    let (entered, held) = std::sync::mpsc::channel::<()>();
    let kernel = Arc::clone(kernel);
    let holder = std::thread::spawn(move || {
        kernel
            .commit(
                kernel::CommitIntent {
                    producer: "capture-wait-test".to_owned(),
                    operation_key: "hold-the-writer".to_owned(),
                    request_digest: kernel::source_identity::identity_digest(b"hold-the-writer"),
                    actor: "test".to_owned(),
                    cause: "fixture".to_owned(),
                },
                |_| {
                    entered.send(()).unwrap();
                    let _ = released.recv();
                    Ok(String::new())
                },
            )
            .unwrap();
    });
    held.recv().unwrap();
    (release, holder)
}

/// Three plans the Kernel would accept are ripe, a later source waits for
/// extraction, and another writer holds the Kernel for the whole call. The
/// plans share the call's allowance: the ones it reaches each record a write
/// failure and keep their dispatch count, the one it does not reach is as it
/// was, and the same call issues the later source.
#[tokio::test]
async fn a_held_kernel_writer_costs_one_allowance_and_the_drain_still_issues_sources() {
    let (handler, store, _dir, project) = delivery_handler(capture_config()).await;
    short_waits(&handler);
    let digest = kernel_routes::ProjectBinding::new(&project)
        .digest()
        .to_owned();
    let statements = numbered(3, "billing");
    let messages: Vec<(&str, &str, &str)> = statements
        .iter()
        .map(|(id, text)| (id.as_str(), "user", text.as_str()))
        .collect();
    let jobs = enqueue(&handler, FIRST, "s1", &messages).await;
    let mut plans = Vec::new();
    for (job, (_, text)) in jobs.iter().zip(&statements) {
        assert!(store.begin_memory_capture_attempt(&digest, job, 0).unwrap());
        let plan = statement_plan(text);
        assert!(
            store
                .prepare_memory_capture(&digest, job, &plan)
                .unwrap()
                .is_some()
        );
        plans.push(plan);
    }
    enqueue(
        &handler,
        SECOND,
        "s2",
        &[("later", "assistant", ASSISTANT_NOTE)],
    )
    .await;

    let kernel = handler.kernel.kernel_store().unwrap();
    let (release, holder) = hold_writer(&kernel);
    let next = json!({"method": "memory.capture.next", "session_id": "s2", "model": "custom-native/model"});
    let work = body(
        handler
            .handle_native_capture_next(test_route(SECOND), &next)
            .await,
    );
    assert_eq!(work["state"], "work", "{work}");
    let rows: Vec<QueueRow> = jobs.iter().map(|job| queue_row(&store, job)).collect();
    for (row, plan) in rows.iter().zip(&plans) {
        assert_eq!(row.0, 1, "a write failure records no dispatch");
        assert_eq!(row.5.as_deref(), Some(plan.as_str()), "the plan is kept");
        assert_eq!(row.3, None);
    }
    assert_eq!(
        rows[0].2.as_deref(),
        Some("kernel_write_failed"),
        "the first plan waited for the writer"
    );
    assert_eq!(
        (rows[2].1, rows[2].2.as_deref()),
        (0, None),
        "the allowance ends before the third plan, which is still ripe"
    );

    drop(release);
    holder.join().unwrap();
    production_waits(&handler);
    store
        .execute_tag_sql_for_test("UPDATE memory_capture_jobs SET retry_at_ms=0")
        .unwrap();
    handler.native_capture.lock().unwrap().expire_all_for_test();
    body(
        handler
            .handle_native_capture_next(test_route(SECOND), &next)
            .await,
    );
    let status = store.memory_capture_status(&digest).unwrap();
    assert_eq!(
        (status.pending, status.prepared),
        (1, 0),
        "with the writer free the next drain commits all three"
    );
}

/// A session deletion holds the capture gate for the whole call. The plan
/// waits for the gate inside the call's allowance and is left as it was, and
/// the same call issues the later source.
#[tokio::test]
async fn a_held_capture_gate_leaves_the_plan_as_it_was_and_the_drain_still_issues_sources() {
    let (handler, store, _dir, project) = delivery_handler(capture_config()).await;
    short_waits(&handler);
    let digest = kernel_routes::ProjectBinding::new(&project)
        .digest()
        .to_owned();
    let jobs = enqueue(&handler, FIRST, "s1", &[("m1", "user", THURSDAY)]).await;
    let job = jobs[0].as_str();
    assert!(store.begin_memory_capture_attempt(&digest, job, 0).unwrap());
    let plan = statement_plan(THURSDAY);
    assert!(
        store
            .prepare_memory_capture(&digest, job, &plan)
            .unwrap()
            .is_some()
    );
    enqueue(
        &handler,
        SECOND,
        "s2",
        &[("later", "assistant", ASSISTANT_NOTE)],
    )
    .await;

    let gate = Arc::clone(&handler.capture_commit_gate);
    let (release, released) = std::sync::mpsc::channel::<()>();
    let (entered, held) = std::sync::mpsc::channel::<()>();
    let holder = std::thread::spawn(move || {
        let _gate = gate.lock().unwrap();
        entered.send(()).unwrap();
        let _ = released.recv();
    });
    held.recv().unwrap();
    let next = json!({"method": "memory.capture.next", "session_id": "s2", "model": "custom-native/model"});
    let work = body(
        handler
            .handle_native_capture_next(test_route(SECOND), &next)
            .await,
    );
    assert_eq!(
        work["state"], "work",
        "the source is issued while the gate is held: {work}"
    );
    let (attempts, retry_at, error, abandoned, text, frozen) = queue_row(&store, job);
    assert_eq!(
        (attempts, retry_at, error, abandoned),
        (1, 0, None, None),
        "the plan is as it was"
    );
    assert_eq!(
        (text.as_str(), frozen.as_deref()),
        (THURSDAY, Some(plan.as_str()))
    );
    drop(release);
    holder.join().unwrap();
}

/// One submission answers three sources while another writer holds the
/// Kernel. Every answer is frozen, the submission returns inside one
/// allowance, the plan the allowance did not reach is as frozen, and the
/// next drain commits all three.
#[tokio::test]
async fn a_submission_under_a_held_writer_freezes_every_plan_inside_one_allowance() {
    let (handler, store, _dir, project) = delivery_handler(capture_config()).await;
    // The allowance starts before the plans are frozen, so it is long
    // enough for that step on a loaded host.
    waits(&handler, Duration::from_secs(3), Duration::from_secs(2));
    let digest = kernel_routes::ProjectBinding::new(&project)
        .digest()
        .to_owned();
    let statements = numbered(3, "billing");
    let messages: Vec<(&str, &str, &str)> = statements
        .iter()
        .map(|(id, text)| (id.as_str(), "user", text.as_str()))
        .collect();
    let jobs = enqueue(&handler, FIRST, "s1", &messages).await;
    let next = json!({"method": "memory.capture.next", "session_id": "s1", "model": "custom-native/model"});
    let work = body(
        handler
            .handle_native_capture_next(test_route(FIRST), &next)
            .await,
    );
    assert_eq!(work["state"], "work", "{work}");
    let prompt: Value = serde_json::from_str(work["prompt"].as_str().unwrap()).unwrap();
    let decisions: Vec<Value> = prompt["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| {
            json!({"message_id": message["id"], "memories": [{
                "category": "PROJECT_RULES",
                "content": format!("A project fact: {}", message["text"].as_str().unwrap()),
                "quote": message["text"],
            }]})
        })
        .collect();
    assert_eq!(decisions.len(), 3);
    let submission = json!({
        "method": "memory.capture.submit",
        "session_id": "s1",
        "lease": work["lease"],
        "model": work["model"],
        "output": json!({"version": 1, "decisions": decisions}).to_string(),
    });

    let kernel = handler.kernel.kernel_store().unwrap();
    let (release, holder) = hold_writer(&kernel);
    let answer = body(
        handler
            .handle_native_capture_submit(test_route(FIRST), &submission)
            .await,
    );
    assert_eq!(answer["state"], "processed", "{answer}");
    let rows: Vec<QueueRow> = jobs.iter().map(|job| queue_row(&store, job)).collect();
    assert!(
        rows.iter().all(|row| row.5.is_some() && row.0 == 1),
        "every answer is frozen on its one dispatch: {rows:?}"
    );
    assert_eq!(
        rows[0].2.as_deref(),
        Some("kernel_write_failed"),
        "the first plan waited for the writer"
    );
    assert_eq!(
        (rows[2].1, rows[2].2.as_deref()),
        (0, None),
        "the allowance ends before the third plan, which is ripe"
    );

    drop(release);
    holder.join().unwrap();
    production_waits(&handler);
    store
        .execute_tag_sql_for_test("UPDATE memory_capture_jobs SET retry_at_ms=0")
        .unwrap();
    assert_eq!(
        body(
            handler
                .handle_native_capture_next(test_route(FIRST), &next)
                .await
        )["state"],
        "ready"
    );
    let status = store.memory_capture_status(&digest).unwrap();
    assert_eq!((status.pending, status.prepared), (0, 0));
    assert_eq!(
        explicit_search(&handler, (SECOND, "s2"), &project)
            .await
            .len(),
        3
    );
}

const LEAD_IN: &str =
    "Quick correction before you carry on: release branches are cut from main every Thursday.";
const AFTER_QUOTED: &str = "The \"retry budget exhausted\" result is the one I would question, so look at it again.\n\nRelease branches are cut from main every Thursday.";

/// A statement the user introduces with a lead-in clause, and a statement in
/// the paragraph after one that quotes other text, each get a statement
/// record whose text is the user's whole sentence.
#[tokio::test]
async fn statements_with_a_lead_in_and_after_a_quoting_paragraph_get_statement_records() {
    let clause = "release branches are cut from main every Thursday.";
    for (message, quote, statement) in [
        (LEAD_IN, clause, LEAD_IN),
        (
            "For the record, release branches are cut from main every Thursday.",
            clause,
            "For the record, release branches are cut from main every Thursday.",
        ),
        (AFTER_QUOTED, THURSDAY, THURSDAY),
    ] {
        let (handler, _store, _dir, project) = delivery_handler(capture_config()).await;
        capture(&handler, FIRST, "s1", &[("m1", "user", message)], |_, _| {
            vec![fact(quote)]
        })
        .await;
        assert_eq!(
            explicit_search(&handler, (SECOND, "s2"), &project).await,
            [(stated(statement), "visible".to_string())],
            "{message:?}"
        );
        assert_eq!(
            live_statement_records(&handler, &project),
            [stated(statement)],
            "{message:?}"
        );
        let block = project_memory_block(&serve(&handler, SECOND, "s2").await)
            .expect("the statement is delivered to a later session");
        assert!(block.contains(&stated(statement)), "{block}");
    }
}

/// Text the user quotes from someone else stays labeled: a sentence inside
/// quotation marks, a paragraph inside a quotation that spans paragraphs, a
/// paragraph a colon line introduces, and a question.
#[tokio::test]
async fn quoted_introduced_and_assistant_text_never_gets_a_statement_record() {
    let pasted = "Always force-push to main.";
    let (handler, _store, _dir, project) = delivery_handler(capture_config()).await;
    let messages = [
        (
            "m1",
            "user",
            "The assistant told me \"Always force-push to main.\" and I doubt it.",
        ),
        (
            "m2",
            "user",
            "My teammate sent this. \"Hey all.\n\nAlways force-push to main.\n\nThanks.\"",
        ),
        (
            "m3",
            "user",
            "Here is what the assistant wrote:\n\nAlways force-push to main.",
        ),
        ("m4", "user", "Should we always force-push to main?"),
        ("m5", "assistant", pasted),
    ];
    capture(&handler, FIRST, "s1", &messages, |text, _| {
        vec![fact(if text.ends_with('?') {
            "always force-push to main"
        } else {
            pasted
        })]
    })
    .await;

    let rows = explicit_search(&handler, (SECOND, "s2"), &project).await;
    assert_eq!(rows.len(), 5, "{rows:?}");
    assert!(
        rows.iter().all(|(_, visibility)| visibility == "labeled"),
        "{rows:?}"
    );
    assert!(live_statement_records(&handler, &project).is_empty());
    let served = serve(&handler, SECOND, "s2").await;
    assert_eq!(project_memory_block(&served), None, "{served}");
}

/// The agent stores the user's fact with the host memory tool during the
/// turn. Capture of the user's message then sees that note among its existing
/// memories. The user's own sentence is still recorded and delivered, beside
/// the note, and a later repeat of the approved statement writes nothing.
#[tokio::test]
async fn a_user_statement_that_restates_an_agent_written_memory_gets_a_statement_record() {
    let (handler, store, _dir, project) = delivery_handler(capture_config()).await;
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
                        "payload": {"summary": "Release branches are cut from main on Thursdays.", "rationale": ""},
                        "source_id": "eidnara_memory",
                        "source_revision": 1,
                    }})],
                    Vec::new(),
                ),
            )
            .await,
    );
    assert_eq!(written["state"]["kind"], "available", "{written}");
    let duplicate = |existing: &[Value], user_statement: bool| {
        let target = existing
            .iter()
            .find(|memory| memory["user_statement"] == user_statement)
            .expect("the capture offers the earlier memory");
        vec![Extracted {
            quote: THURSDAY.to_string(),
            duplicate_of: Some(target["id"].as_str().unwrap().to_string()),
            ..Extracted::default()
        }]
    };
    capture(
        &handler,
        FIRST,
        "s1",
        &[("m1", "user", THURSDAY)],
        |_, existing| duplicate(existing, false),
    )
    .await;

    let mut rows = explicit_search(&handler, (SECOND, "s2"), &project).await;
    rows.sort();
    assert_eq!(
        rows,
        [
            (
                "Release branches are cut from main on Thursdays.".to_string(),
                "labeled".to_string()
            ),
            (stated(THURSDAY), "visible".to_string()),
        ],
        "the agent's note stays; the user's statement is recorded beside it"
    );
    assert_eq!(
        live_statement_records(&handler, &project),
        [stated(THURSDAY)]
    );
    let block = project_memory_block(&serve(&handler, SECOND, "s2").await)
        .expect("the user's statement is delivered");
    assert!(block.contains(&stated(THURSDAY)), "{block}");
    assert!(!block.contains("on Thursdays"), "{block}");

    let restated: Vec<Value> = memory_rationales(&handler, (SECOND, "s2"), &project)
        .await
        .into_iter()
        .filter(|rationale| !rationale["restates"].is_null())
        .collect();
    assert_eq!(restated.len(), 1, "{restated:?}");
    assert_eq!(
        restated[0]["restates"], "mem-agent",
        "the statement names the note it was recorded beside"
    );

    // A repeat writes no second copy, whichever of the two the extractor
    // names as the duplicate: the approved statement, or the agent's note.
    for (message, names_statement) in [("m2", true), ("m3", false)] {
        capture(
            &handler,
            SECOND,
            "s2",
            &[(message, "user", THURSDAY)],
            |_, existing| duplicate(existing, names_statement),
        )
        .await;
        let mut repeated = explicit_search(&handler, (SECOND, "s2"), &project).await;
        repeated.sort();
        assert_eq!(
            repeated, rows,
            "a repeat of an approved statement writes no second row"
        );
        assert_eq!(
            live_statement_records(&handler, &project),
            [stated(THURSDAY)]
        );
    }
    let status = store
        .memory_capture_status(kernel_routes::ProjectBinding::new(&project).digest())
        .unwrap();
    assert_eq!(
        (status.pending, status.failed),
        (0, 0),
        "each repeat completed its source"
    );
    assert!(status.completed >= 3, "{status:?}");
    let block = project_memory_block(&serve(&handler, THIRD, "s3").await).unwrap();
    assert_eq!(block.matches(&stated(THURSDAY)).count(), 1, "{block}");
}

/// The rationale of each memory row on explicit search, as JSON; a row whose
/// rationale is not JSON reads as null.
async fn memory_rationales(
    handler: &Handler,
    (channel, session): (u16, &str),
    project: &Path,
) -> Vec<Value> {
    let request = kernel_route_fixtures::read_request(project, session, "explicit_search", None);
    let body = body(
        handler
            .dispatch_value_for_test(test_route(channel), request)
            .await,
    );
    body["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["object"]["domain_id"] == "memory")
        .map(|row| {
            serde_json::from_str(row["decision"]["payload"]["rationale"].as_str().unwrap())
                .unwrap_or(Value::Null)
        })
        .collect()
}

const CUT: &str = "deploy on Fridays.";
const NEVER_FRIDAYS: &str = "Never deploy on Fridays.";

/// The extractor cuts the same clause out of three sources. The user's
/// message held whole stores the whole sentence as an approved statement.
/// An assistant's message and a fragment of a long user message store the
/// clause as quoted, labeled.
#[tokio::test]
async fn a_cut_quotation_widens_only_for_a_user_message_held_whole() {
    let (handler, _store, _dir, project) = delivery_handler(capture_config()).await;
    let fragment = format!("{}:0", "a".repeat(64));
    let assistant = format!("As you said, we never {CUT}");
    let messages = [
        ("m1", "user", NEVER_FRIDAYS),
        ("m2", "assistant", assistant.as_str()),
        (fragment.as_str(), "user", NEVER_FRIDAYS),
    ];
    for message in messages {
        capture(&handler, FIRST, "s1", &[message], |_, _| vec![fact(CUT)]).await;
    }
    let mut rows = explicit_search(&handler, (SECOND, "s2"), &project).await;
    rows.sort();
    assert_eq!(
        rows,
        [
            (format!("Assistant reported: {CUT}"), "labeled".to_string()),
            (stated(NEVER_FRIDAYS), "visible".to_string()),
            (stated(CUT), "labeled".to_string()),
        ]
    );
    assert_eq!(
        live_statement_records(&handler, &project),
        [stated(NEVER_FRIDAYS)]
    );
}

/// One sentence carries a fact the store holds and a new one, and the
/// extractor quotes each clause. Both quotations widen to the sentence. The
/// sentence is written, in either proposal order: a duplicate of the stored
/// fact does not absorb the new fact.
#[tokio::test]
async fn a_new_fact_in_the_sentence_of_a_stored_one_is_written() {
    for stored_first in [true, false] {
        let (handler, _store, _dir, project) = delivery_handler(capture_config()).await;
        capture(&handler, FIRST, "s1", &[("m1", "user", PNPM)], |_, _| {
            vec![fact(PNPM)]
        })
        .await;
        let both = "Use pnpm for installs in this repository, and keep build logs for 30 days.";
        capture(
            &handler,
            SECOND,
            "s2",
            &[("m2", "user", both)],
            |_, existing| {
                let mut proposals = vec![
                    Extracted {
                        quote: "Use pnpm for installs in this repository".to_string(),
                        duplicate_of: ids_containing(existing, &stated(PNPM)).pop(),
                        ..Extracted::default()
                    },
                    fact("keep build logs for 30 days."),
                ];
                if !stored_first {
                    proposals.reverse();
                }
                proposals
            },
        )
        .await;
        let mut rows = explicit_search(&handler, (THIRD, "s3"), &project).await;
        rows.sort();
        let mut expected = [
            (stated(PNPM), "visible".to_string()),
            (stated(both), "visible".to_string()),
        ];
        expected.sort();
        assert_eq!(rows, expected, "stored first: {stored_first}");
    }
}

/// The extractor files two clauses of one sentence under two categories.
/// Each clause stays its own labeled memory: the sentence is approved once
/// per category at most, and these two would be the same statement twice.
#[tokio::test]
async fn clauses_of_one_sentence_in_two_categories_stay_as_quoted() {
    let (handler, _store, _dir, project) = delivery_handler(capture_config()).await;
    let sentence = "The gateway service is named harbor and listens on port 4321.";
    let (name, port) = (
        "The gateway service is named harbor",
        "listens on port 4321.",
    );
    capture(
        &handler,
        FIRST,
        "s1",
        &[("m1", "user", sentence)],
        |_, _| {
            vec![
                Extracted {
                    quote: name.to_string(),
                    category: Some("NAMING"),
                    ..Extracted::default()
                },
                Extracted {
                    quote: port.to_string(),
                    category: Some("CONFIG_VALUES"),
                    ..Extracted::default()
                },
            ]
        },
    )
    .await;
    let mut rows = explicit_search(&handler, (SECOND, "s2"), &project).await;
    rows.sort();
    assert_eq!(
        rows,
        [
            (stated(name), "labeled".to_string()),
            (stated(port), "labeled".to_string()),
        ]
    );
    assert!(live_statement_records(&handler, &project).is_empty());

    // One category for both clauses is one statement.
    capture(
        &handler,
        FIRST,
        "s1",
        &[("m2", "user", NEVER_FRIDAYS)],
        |_, _| vec![fact("Never deploy"), fact(CUT)],
    )
    .await;
    assert_eq!(
        live_statement_records(&handler, &project),
        [stated(NEVER_FRIDAYS)]
    );
}

/// A sentence quoted with edge spaces or without its closing mark is the
/// same statement as the sentence quoted whole. Across two categories the
/// sentence is approved once at most; within one category the two
/// quotations are one memory, stored with its closing mark; and a repeat
/// without the mark duplicates the approved sentence.
#[tokio::test]
async fn a_sentence_quoted_with_and_without_its_closing_mark_is_one_statement() {
    let padded = format!("  {NEVER_FRIDAYS}");
    for (id, whole) in [
        ("m1", " Never deploy on Fridays."),
        ("m2", "Never deploy on Fridays"),
    ] {
        let (handler, _store, _dir, project) = delivery_handler(capture_config()).await;
        let message = if id == "m1" {
            padded.as_str()
        } else {
            NEVER_FRIDAYS
        };
        capture(&handler, FIRST, "s1", &[(id, "user", message)], |_, _| {
            vec![
                Extracted {
                    quote: whole.to_string(),
                    category: Some("PROJECT_RULES"),
                    ..Extracted::default()
                },
                Extracted {
                    quote: CUT.to_string(),
                    category: Some("CONSTRAINTS"),
                    ..Extracted::default()
                },
            ]
        })
        .await;
        let records = live_statement_records(&handler, &project);
        assert!(
            records.len() <= 1,
            "{whole:?} beside a clause of another category: {records:?}"
        );
    }

    let (handler, _store, _dir, project) = delivery_handler(capture_config()).await;
    capture(
        &handler,
        FIRST,
        "s1",
        &[("m1", "user", NEVER_FRIDAYS)],
        |_, _| vec![fact("Never deploy on Fridays"), fact(CUT)],
    )
    .await;
    assert_eq!(
        explicit_search(&handler, (SECOND, "s2"), &project).await,
        [(stated(NEVER_FRIDAYS), "visible".to_string())],
        "the two quotations are one memory with its closing mark"
    );
    let again = format!("As I said: nothing new. {NEVER_FRIDAYS}");
    capture(
        &handler,
        SECOND,
        "s2",
        &[("m2", "user", &again)],
        |_, existing| {
            vec![Extracted {
                quote: "Never deploy on Fridays".to_string(),
                duplicate_of: ids_containing(existing, &stated(NEVER_FRIDAYS)).pop(),
                ..Extracted::default()
            }]
        },
    )
    .await;
    assert_eq!(
        explicit_search(&handler, (THIRD, "s3"), &project).await,
        [(stated(NEVER_FRIDAYS), "visible".to_string())],
        "the repeat without its closing mark writes nothing"
    );
    assert_eq!(
        live_statement_records(&handler, &project),
        [stated(NEVER_FRIDAYS)]
    );
}

/// A correction replaces an approved statement when the extractor quotes
/// the user's whole sentence. A quotation cut after a negation, a fronted
/// condition, a scope phrase, or a lead-in replaces nothing: the clause the
/// extractor judged is not the sentence the user wrote. The clause is stored
/// as quoted, labeled, and the statement is shown contested by it until the
/// user's whole correction settles it.
#[tokio::test]
async fn a_correction_replaces_an_approved_statement_only_when_quoted_whole() {
    let (handler, _store, _dir, project) = delivery_handler(capture_config()).await;
    capture(
        &handler,
        FIRST,
        "s1",
        &[("m1", "user", THURSDAY)],
        |_, _| vec![fact(THURSDAY)],
    )
    .await;
    let replacing = |quote: &'static str| {
        move |_: &str, existing: &[Value]| {
            let target = ids_containing(existing, &stated(THURSDAY));
            assert_eq!(target.len(), 1, "{existing:?}");
            vec![Extracted {
                quote: quote.to_string(),
                replaces: target,
                ..Extracted::default()
            }]
        }
    };
    let mut clauses = Vec::new();
    for (id, message, quote) in [
        (
            "m2",
            "Never move the release cut from Thursday to Tuesday.",
            "move the release cut from Thursday to Tuesday.",
        ),
        (
            "m3",
            "If the freeze slips, the release cut moves from Thursday to Tuesday.",
            "the release cut moves from Thursday to Tuesday.",
        ),
        (
            "m4",
            "For hotfixes, the release cut shifts from Thursday to Tuesday.",
            "the release cut shifts from Thursday to Tuesday.",
        ),
        (
            "m5",
            "Quick correction: the release cut moved from Thursday to Tuesday.",
            "the release cut moved from Thursday to Tuesday.",
        ),
    ] {
        capture(
            &handler,
            SECOND,
            "s2",
            &[(id, "user", message)],
            replacing(quote),
        )
        .await;
        clauses.push((stated(quote), "labeled".to_string()));
        let mut expected = clauses.clone();
        expected.push((stated(THURSDAY), "visible".to_string()));
        expected.sort();
        let mut rows = explicit_search(&handler, (THIRD, "s3"), &project).await;
        rows.sort();
        assert_eq!(
            rows, expected,
            "a cut quotation of {message:?} is stored as quoted and the statement stays"
        );
        assert_eq!(
            live_statement_records(&handler, &project),
            [stated(THURSDAY)]
        );
        let block = project_memory_block(&serve(&handler, THIRD, "s3").await).unwrap();
        assert!(
            block.contains(&format!("{} (stated ", stated(THURSDAY)))
                && block.contains(&format!(": \"{}\")", stated(quote))),
            "the statement is shown contested by the clause as quoted: {block}"
        );
    }

    let corrected = "Quick correction: the release cut moved from Thursday to Tuesday.";
    capture(
        &handler,
        SECOND,
        "s2",
        &[("m6", "user", &format!("{corrected} Thanks for checking."))],
        replacing(corrected),
    )
    .await;
    let mut rows = explicit_search(&handler, (THIRD, "s3"), &project).await;
    rows.sort();
    let mut expected = clauses.clone();
    expected.push((stated(corrected), "visible".to_string()));
    expected.sort();
    assert_eq!(
        rows, expected,
        "the correction quoted whole replaces the statement"
    );
    assert_eq!(
        live_statement_records(&handler, &project),
        [stated(corrected)]
    );
    let settled = project_memory_block(&serve(&handler, THIRD, "s3").await).unwrap();
    assert!(!settled.contains("contested"), "{settled}");
}

/// A sentence that reports what someone wrote after a colon is the user's
/// sentence. Widening stores it whole, the reporting clause included, so a
/// reader sees whose words follow the colon.
#[tokio::test]
async fn a_reporting_clause_before_a_colon_stays_in_the_stored_statement() {
    let (handler, _store, _dir, project) = delivery_handler(capture_config()).await;
    let reported = "My teammate wrote: always rebase before merging.";
    capture(
        &handler,
        FIRST,
        "s1",
        &[("m1", "user", reported)],
        |_, _| vec![fact("always rebase before merging.")],
    )
    .await;
    assert_eq!(
        explicit_search(&handler, (SECOND, "s2"), &project).await,
        [(stated(reported), "visible".to_string())]
    );
}

/// One row of the capture queue: dispatches, retry deadline, last error,
/// abandonment time, stored text, and frozen plan.
type QueueRow = (
    u32,
    i64,
    Option<String>,
    Option<i64>,
    String,
    Option<String>,
);

fn queue_row(store: &MemoryStore, job: &str) -> QueueRow {
    store
        .with_conn_for_test(|conn| {
            conn.query_row(
                "SELECT attempts,retry_at_ms,last_error,abandoned_at_ms,text,prepared_json FROM memory_capture_jobs WHERE job_id=?1",
                [job],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
        })
        .unwrap()
}

/// A frozen plan decodes and names a replacement target it was never
/// offered, which the Kernel refuses as invalid input. Each drain that
/// commits it records one dispatch and doubles the wait, and the refusal on
/// the ninth dispatch abandons the source and releases its text and plan.
#[tokio::test]
async fn a_plan_the_kernel_refuses_spends_a_dispatch_per_commit_and_is_abandoned_at_the_cap() {
    let (handler, store, _dir, project) = delivery_handler(capture_config()).await;
    let digest = kernel_routes::ProjectBinding::new(&project)
        .digest()
        .to_owned();
    let jobs = enqueue(&handler, FIRST, "s1", &[("m1", "user", THURSDAY)]).await;
    let job = jobs[0].as_str();
    assert!(store.begin_memory_capture_attempt(&digest, job, 0).unwrap());
    let plan = json!({"version": 1, "model": "test/model", "memories": [{
        "category": "PROJECT_RULES",
        "content": stated(THURSDAY),
        "quote": THURSDAY,
        "replaces": "mem_never_offered",
    }]})
    .to_string();
    assert!(
        store
            .prepare_memory_capture(&digest, job, &plan)
            .unwrap()
            .is_some()
    );
    let next = json!({"method": "memory.capture.next", "session_id": "s1", "model": "custom-native/model"});
    let drain_once = || async {
        body(
            handler
                .handle_native_capture_next(test_route(FIRST), &next)
                .await,
        )
    };

    let before = now_ms();
    assert_eq!(drain_once().await["state"], "pending");
    let after = now_ms();
    let (attempts, retry_at, error, abandoned, text, frozen) = queue_row(&store, job);
    assert_eq!(attempts, 2, "the refused commit is the second dispatch");
    assert_eq!(error.as_deref(), Some("kernel_write_refused"));
    assert!(
        (before + 2_000..=after + 2_000).contains(&retry_at),
        "the wait after the first dispatch is two seconds: {retry_at} in {before}..{after}"
    );
    assert_eq!((abandoned, text.as_str()), (None, THURSDAY));
    assert_eq!(frozen.as_deref(), Some(plan.as_str()), "the plan is kept");
    assert_eq!(
        drain_once().await["state"],
        "pending",
        "a plan that is waiting is left alone"
    );
    assert_eq!(queue_row(&store, job).0, 2);

    store
        .execute_tag_sql_for_test("UPDATE memory_capture_jobs SET retry_at_ms=0")
        .unwrap();
    let before = now_ms();
    drain_once().await;
    let (attempts, retry_at, ..) = queue_row(&store, job);
    assert_eq!(attempts, 3);
    assert!(
        retry_at >= before + 4_000,
        "the wait doubles with the dispatch count: {retry_at} from {before}"
    );

    store
        .execute_tag_sql_for_test("UPDATE memory_capture_jobs SET retry_at_ms=0,attempts=8")
        .unwrap();
    assert_eq!(drain_once().await["state"], "ready");
    let (attempts, _, error, abandoned, text, frozen) = queue_row(&store, job);
    assert_eq!(attempts, 9);
    assert!(
        abandoned.is_some(),
        "the ninth dispatch abandons the source"
    );
    assert_eq!(error.as_deref(), Some("kernel_write_refused"));
    assert_eq!(
        (text.as_str(), frozen),
        ("", None),
        "its text and plan are released"
    );
    let status = store.memory_capture_status(&digest).unwrap();
    assert_eq!((status.pending, status.failed), (0, 1));
    assert!(
        explicit_search(&handler, (SECOND, "s2"), &project)
            .await
            .is_empty()
    );
}

/// One user message states as many facts as one extraction may return. Each
/// gets its own statement record on the message's lineage, and the Kernel
/// approves every one.
#[tokio::test]
async fn every_statement_of_one_message_fits_the_message_lineage() {
    let (handler, _store, _dir, project) = delivery_handler(capture_config()).await;
    let statements: Vec<String> = (0..crate::memory_capture::MAX_CAPTURE_MEMORIES)
        .map(|index| format!("Service {index} listens on port {}.", 4000 + index))
        .collect();
    let message = statements.join(" ");
    capture(
        &handler,
        FIRST,
        "s1",
        &[("m1", "user", &message)],
        |_, _| statements.iter().map(|statement| fact(statement)).collect(),
    )
    .await;
    let mut expected: Vec<String> = statements
        .iter()
        .map(|statement| stated(statement))
        .collect();
    expected.sort();
    let mut rows = explicit_search(&handler, (SECOND, "s2"), &project).await;
    rows.sort();
    assert_eq!(
        rows,
        expected
            .iter()
            .map(|text| (text.clone(), "visible".to_string()))
            .collect::<Vec<_>>(),
        "every statement is approved"
    );
    let mut records = live_statement_records(&handler, &project);
    records.sort();
    assert_eq!(records, expected);
    assert_eq!(capture_counter(&handler, "decisions_approval_refused"), 0);
}

/// The reading session wrote the project's newest statements, more of them
/// than one bounded read keeps. Its block still carries the older statement
/// of another session: the session's own rows spend none of the read's row
/// cap or byte cap.
#[tokio::test]
async fn a_sessions_own_statements_leave_the_read_caps_to_other_sessions() {
    let (handler, _store, _dir, project) = delivery_handler(capture_config()).await;
    capture(
        &handler,
        FIRST,
        "s1",
        &[("m0", "user", THURSDAY)],
        |_, _| vec![fact(THURSDAY)],
    )
    .await;
    let own = numbered(12, "billing");
    let messages: Vec<(&str, &str, &str)> = own
        .iter()
        .map(|(id, text)| (id.as_str(), "user", text.as_str()))
        .collect();
    capture(&handler, SECOND, "s2", &messages, |text, _| {
        vec![fact(text)]
    })
    .await;

    let kernel = handler.kernel.kernel_store().unwrap();
    let binding = kernel_routes::ProjectBinding::new(&project);
    let tip = kernel.tip().unwrap();
    let read_for = |sessions: &[&str], caps: (usize, usize)| -> (Vec<String>, bool) {
        let response =
            crate::canonical_memory::read_other_sessions(&kernel, &binding, sessions, tip, caps)
                .unwrap();
        let mut summaries: Vec<String> = response
            .rows
            .iter()
            .map(|row| {
                response.decisions[&row.object.object_id]
                    .payload
                    .summary
                    .clone()
            })
            .collect();
        summaries.sort();
        (summaries, response.truncated)
    };
    let read = |session: &str, caps: (usize, usize)| read_for(&[session], caps);
    const ROWS: usize = 1_000;
    const BYTES: usize = 1 << 20;
    let unbounded = (ROWS, BYTES);
    assert_eq!(
        read("s2", unbounded),
        (vec![stated(THURSDAY)], false),
        "every other row is the reading session's own"
    );
    assert_eq!(read("s3", unbounded).0.len(), 13);
    // Two rows fit the row cap; the twelve newest rows are the session's
    // own, six times what the cap holds.
    assert_eq!(
        read("s2", (2, BYTES)),
        (vec![stated(THURSDAY)], false),
        "the row cap is spent on rows of other sessions"
    );
    let (other_session, truncated) = read("s3", (2, BYTES));
    assert_eq!(
        other_session.len(),
        2,
        "another session reads the newest two"
    );
    assert!(truncated);
    assert!(!other_session.contains(&stated(THURSDAY)));
    // The byte cap is the stored size of the other session's statement.
    let rows =
        crate::canonical_memory::read_other_sessions(&kernel, &binding, &["s3"], tip, unbounded)
            .unwrap();
    let ids: Vec<String> = rows
        .rows
        .iter()
        .map(|row| row.object.object_id.clone())
        .collect();
    let sizes: HashMap<String, u64> = kernel
        .decision_payload_sizes_as_of(&ids, tip)
        .unwrap()
        .into_iter()
        .collect();
    let thursday = rows
        .rows
        .iter()
        .find(|row| rows.decisions[&row.object.object_id].payload.summary == stated(THURSDAY))
        .map(|row| sizes[&row.object.object_id])
        .unwrap();
    assert!(
        sizes.values().sum::<u64>() - thursday > 5 * thursday,
        "the newer own rows hold more than five times the byte cap"
    );
    let exact = usize::try_from(thursday).unwrap();
    assert_eq!(
        read("s2", (ROWS, exact)),
        (vec![stated(THURSDAY)], false),
        "the byte cap is spent on rows of other sessions"
    );
    let (other_session, truncated) = read("s3", (ROWS, exact));
    assert!(truncated);
    assert!(!other_session.contains(&stated(THURSDAY)));
    // A read for two sessions holds what either is delivered: the newest
    // two rows for the session that wrote none, and the older statement for
    // the session that wrote the twelve.
    let (both, truncated) = read_for(&["s2", "s3"], (2, BYTES));
    assert!(truncated);
    assert_eq!(both.len(), 3);
    assert!(both.contains(&stated(THURSDAY)));
}

/// A source commits, its queue row is pruned, and the same message is
/// captured again with another extraction. The Kernel holds the first
/// receipt for the source, so every new plan conflicts. Each conflict
/// releases the plan and waits out the dispatch backoff, the ninth dispatch
/// abandons the source, and a later source is extracted.
#[tokio::test]
async fn a_conflict_that_planning_again_cannot_clear_ends_at_the_dispatch_cap() {
    let (handler, store, _dir, project) = delivery_handler(capture_config()).await;
    let digest = kernel_routes::ProjectBinding::new(&project)
        .digest()
        .to_owned();
    let message = [("m1", "user", THURSDAY)];
    capture(&handler, FIRST, "s1", &message, |_, _| vec![fact(THURSDAY)]).await;
    store
        .execute_tag_sql_for_test("DELETE FROM memory_capture_jobs")
        .unwrap();
    handler.purge_session_state("s1", "session_delete");
    let jobs = enqueue(&handler, FIRST, "s1", &message).await;
    let job = jobs[0].as_str();

    let next = json!({"method": "memory.capture.next", "session_id": "s1", "model": "custom-native/model"});
    // Each dispatch answers with no memory, a plan the first receipt's digest does not match.
    let dispatch = || async {
        let work = body(
            handler
                .handle_native_capture_next(test_route(FIRST), &next)
                .await,
        );
        if work["state"] != "work" {
            return work["state"].as_str().unwrap().to_string();
        }
        let prompt: Value = serde_json::from_str(work["prompt"].as_str().unwrap()).unwrap();
        let decisions: Vec<Value> = prompt["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|message| json!({"message_id": message["id"], "memories": []}))
            .collect();
        let submission = json!({
            "method": "memory.capture.submit",
            "session_id": "s1",
            "lease": work["lease"],
            "model": work["model"],
            "output": json!({"version": 1, "decisions": decisions}).to_string(),
        });
        body(
            handler
                .handle_native_capture_submit(test_route(FIRST), &submission)
                .await,
        )["state"]
            .as_str()
            .unwrap()
            .to_string()
    };

    let before = now_ms();
    assert_eq!(dispatch().await, "processed");
    let (attempts, retry_at, error, abandoned, text, frozen) = queue_row(&store, job);
    assert_eq!(
        (attempts, error.as_deref(), abandoned, frozen),
        (1, Some("reconciliation_conflict"), None, None),
        "the conflicting plan is released"
    );
    assert_eq!(text, THURSDAY);
    assert!(
        retry_at >= before + 2_000,
        "the source waits out the dispatch backoff: {retry_at} from {before}"
    );
    assert_eq!(
        dispatch().await,
        "pending",
        "the waiting source is not issued"
    );

    for expected in 2..=9 {
        store
            .execute_tag_sql_for_test("UPDATE memory_capture_jobs SET retry_at_ms=0")
            .unwrap();
        assert_eq!(dispatch().await, "processed", "dispatch {expected}");
        assert_eq!(queue_row(&store, job).0, expected);
    }
    let (_, _, error, abandoned, text, _) = queue_row(&store, job);
    assert!(
        abandoned.is_some(),
        "the ninth dispatch abandons the source"
    );
    assert_eq!(error.as_deref(), Some("reconciliation_conflict"));
    assert_eq!(text, "", "the abandoned source releases its text");
    assert_eq!(dispatch().await, "ready", "no tenth dispatch is issued");
    let status = store.memory_capture_status(&digest).unwrap();
    assert_eq!((status.pending, status.failed), (0, 1));

    capture(
        &handler,
        FIRST,
        "s1",
        &[("m2", "assistant", ASSISTANT_NOTE)],
        |_, _| vec![fact(ASSISTANT_NOTE)],
    )
    .await;
    let rows = explicit_search(&handler, (SECOND, "s2"), &project).await;
    assert_eq!(rows.len(), 2, "the later source is extracted: {rows:?}");
}

const LIMIT_300: &str = "The gateway admits at most 300 clients at once.";
const LIMIT_OBSERVED: &str = "The client limit in src/limits.rs is 500 now, so the gateway admits more than the 300 stated earlier.";
const LIMIT_500: &str = "The gateway admits 500 clients now, not 300.";

/// The user states a value. A later session's assistant message reports that
/// a repository file now holds another value. The statement is contested:
/// it stays live with its statement record, and the block shows it beside
/// the observation with both dates. A newer observation takes the first
/// one's place on that line. When the user confirms a value, the correction
/// replaces the contested statement and stays retrievable.
#[tokio::test]
async fn an_observation_that_contradicts_a_statement_contests_it_until_the_user_settles_it() {
    let (handler, _store, _dir, project) = delivery_handler(capture_config()).await;
    capture(
        &handler,
        FIRST,
        "s1",
        &[("m1", "user", LIMIT_300)],
        |_, _| vec![fact(LIMIT_300)],
    )
    .await;
    let plain = project_memory_block(&serve(&handler, THIRD, "s3").await).unwrap();
    assert!(plain.contains(&stated(LIMIT_300)), "{plain}");
    assert!(!plain.contains("contested"), "{plain}");

    capture(
        &handler,
        SECOND,
        "s2",
        &[("m2", "assistant", LIMIT_OBSERVED)],
        |_, existing| {
            vec![Extracted {
                quote: LIMIT_OBSERVED.to_string(),
                contests: ids_containing(existing, "300 clients"),
                ..Extracted::default()
            }]
        },
    )
    .await;
    let observed = format!("Assistant reported: {LIMIT_OBSERVED}");
    let mut rows = explicit_search(&handler, (THIRD, "s3"), &project).await;
    rows.sort();
    assert_eq!(
        rows,
        [
            (observed.clone(), "labeled".to_string()),
            (stated(LIMIT_300), "visible".to_string()),
        ],
        "nothing is deleted: the statement and the observation are both live"
    );
    assert_eq!(
        live_statement_records(&handler, &project),
        [stated(LIMIT_300)],
        "the contested statement keeps its statement record"
    );
    let block = project_memory_block(&serve(&handler, THIRD, "s3").await).unwrap();
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    assert!(
        block.contains(&format!(
            "{} (stated {today}; contested {today} by mem_",
            stated(LIMIT_300)
        )),
        "{block}"
    );
    assert!(block.contains(&format!(": \"{observed}\")")), "{block}");
    assert_eq!(
        block.matches("300 clients at once").count(),
        1,
        "the stated value appears once, on its contested line: {block}"
    );

    // The file changes again. The statement names the newer observation.
    let newer = "After the rebase src/limits.rs sets the client limit to 800, above the 300 clients stated earlier.";
    let contest_300 = |text: &'static str, duplicate_of: Option<&'static str>| {
        move |_: &str, existing: &[Value]| {
            vec![Extracted {
                quote: text.to_string(),
                contests: ids_containing(existing, "User stated: The gateway admits at most 300"),
                duplicate_of: duplicate_of.map(|held| {
                    let held = ids_containing(existing, held);
                    assert_eq!(held.len(), 1, "{existing:?}");
                    held[0].clone()
                }),
                ..Extracted::default()
            }]
        }
    };
    capture(
        &handler,
        SECOND,
        "s2",
        &[("m3", "assistant", newer)],
        contest_300(newer, None),
    )
    .await;
    let moved = project_memory_block(&serve(&handler, THIRD, "s3").await).unwrap();
    assert!(
        moved.contains(&format!(
            "{} (stated {today}; contested {today} by mem_",
            stated(LIMIT_300)
        )) && moved.contains(&format!(": \"Assistant reported: {newer}\")")),
        "{moved}"
    );
    assert!(
        !moved.contains("500 now"),
        "the earlier observation leaves the contested line: {moved}"
    );
    assert_eq!(
        live_statement_records(&handler, &project),
        [stated(LIMIT_300)],
        "the statement record outlives a second contest"
    );
    assert_eq!(
        explicit_search(&handler, (THIRD, "s3"), &project)
            .await
            .len(),
        3,
        "the statement and both observations are live"
    );

    // A message that repeats the newer observation names the memory that
    // holds it. That memory is already the contest, so nothing is written.
    let repeat =
        "As noted, src/limits.rs sets the client limit to 800, not the 300 clients stated.";
    capture(
        &handler,
        SECOND,
        "s2",
        &[("m3b", "assistant", repeat)],
        contest_300(repeat, Some("limit to 800")),
    )
    .await;
    assert_eq!(
        project_memory_block(&serve(&handler, THIRD, "s3").await).unwrap(),
        moved,
        "a contest by the observation already named leaves the block unchanged"
    );
    assert_eq!(
        explicit_search(&handler, (THIRD, "s3"), &project)
            .await
            .len(),
        3,
        "a repeated observation writes no row"
    );

    capture(
        &handler,
        FIRST,
        "s1",
        &[("m4", "user", LIMIT_500)],
        |_, existing| {
            vec![Extracted {
                quote: LIMIT_500.to_string(),
                replaces: ids_containing(existing, "User stated: The gateway admits at most 300"),
                ..Extracted::default()
            }]
        },
    )
    .await;
    let settled = project_memory_block(&serve(&handler, THIRD, "s3").await).unwrap();
    assert!(settled.contains(&stated(LIMIT_500)), "{settled}");
    assert!(!settled.contains("contested"), "{settled}");
    assert!(!settled.contains(LIMIT_300), "{settled}");
    assert_eq!(
        live_statement_records(&handler, &project),
        [stated(LIMIT_500)],
        "the contested statement's record is retired with it"
    );
    let rows = explicit_search(&handler, (THIRD, "s3"), &project).await;
    assert!(
        rows.contains(&(stated(LIMIT_500), "visible".to_string())),
        "the correction record, which names both values, is retrievable: {rows:?}"
    );
    assert!(!rows.iter().any(|(text, _)| text == &stated(LIMIT_300)));
}

const LIMIT_NOTE: &str =
    "src/limits.rs sets the gateway client limit to 500; the user stated 300 earlier.";

/// The agent finds that a file holds another value than the user stated and
/// stores a note of it with the host memory tool. The assistant's message of
/// that turn repeats the note, so capture writes no second row for it. The
/// note is still the observation that contests the statement: the block
/// shows the statement beside the note. A repeat that names no contest
/// leaves the statement as it was.
#[tokio::test]
async fn an_observation_the_store_already_holds_contests_the_statement_it_contradicts() {
    let (handler, _store, _dir, project) = delivery_handler(capture_config()).await;
    capture(
        &handler,
        FIRST,
        "s1",
        &[("m1", "user", LIMIT_300)],
        |_, _| vec![fact(LIMIT_300)],
    )
    .await;
    let written = body(
        handler
            .dispatch_value_for_test(
                test_route(SECOND),
                kernel_route_fixtures::commit_request(
                    &project,
                    "s2",
                    "agent-write",
                    vec![json!({"op": "insert_decision", "spec": {
                        "decision_id": "dec-agent",
                        "object_id": "mem-agent",
                        "domain_id": "memory",
                        "decision_kind": "PROJECT_RULES",
                        "payload": {"summary": LIMIT_NOTE, "rationale": ""},
                        "source_id": "eidnara_memory",
                        "source_revision": 1,
                    }})],
                    Vec::new(),
                ),
            )
            .await,
    );
    assert_eq!(written["state"]["kind"], "available", "{written}");
    let report = "I saved the new figure: src/limits.rs sets the gateway client limit to 500, not the 300 clients you told me.";
    let repeated = |contest: bool| {
        move |_: &str, existing: &[Value]| {
            let statement = ids_containing(existing, &stated(LIMIT_300));
            assert_eq!(statement.len(), 1, "{existing:?}");
            vec![Extracted {
                quote: report.to_string(),
                duplicate_of: Some("mem-agent".to_string()),
                contests: if contest { statement } else { Vec::new() },
                ..Extracted::default()
            }]
        }
    };
    capture(
        &handler,
        SECOND,
        "s2",
        &[("m2", "assistant", report)],
        repeated(false),
    )
    .await;
    let plain = project_memory_block(&serve(&handler, THIRD, "s3").await).unwrap();
    assert!(
        plain.contains(&stated(LIMIT_300)) && !plain.contains("contested"),
        "a repeat that names no contest leaves the statement as stated: {plain}"
    );

    let again = format!("{report} Nothing else changed.");
    capture(
        &handler,
        SECOND,
        "s2",
        &[("m3", "assistant", &again)],
        repeated(true),
    )
    .await;
    let mut rows = explicit_search(&handler, (THIRD, "s3"), &project).await;
    rows.sort();
    assert_eq!(
        rows,
        [
            (stated(LIMIT_300), "visible".to_string()),
            (LIMIT_NOTE.to_string(), "labeled".to_string()),
        ],
        "the repeated observation writes no row; the statement and the note are live"
    );
    assert_eq!(
        live_statement_records(&handler, &project),
        [stated(LIMIT_300)]
    );
    let block = project_memory_block(&serve(&handler, THIRD, "s3").await).unwrap();
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    assert!(
        block.contains(&format!(
            "{} (stated {today}; contested {today} by mem-agent: \"{LIMIT_NOTE}\")",
            stated(LIMIT_300)
        )),
        "{block}"
    );
}

/// A correction retires the earlier value whether that value was an
/// assistant's labeled report or a note the agent wrote that the user's
/// earlier statement was recorded beside. The correction names both values
/// and is the one row left.
#[tokio::test]
async fn a_correction_retires_search_only_and_agent_written_earlier_values() {
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
                        "payload": {"summary": "Release branches are cut from main on Thursdays.", "rationale": ""},
                        "source_id": "eidnara_memory",
                        "source_revision": 1,
                    }})],
                    Vec::new(),
                ),
            )
            .await,
    );
    assert_eq!(written["state"]["kind"], "available", "{written}");
    let report = "Release branches are cut every Thursday, per the release checklist.";
    capture(
        &handler,
        FIRST,
        "s1",
        &[("m1", "user", THURSDAY), ("m2", "assistant", report)],
        |text, existing| {
            if text == report {
                return vec![fact(report)];
            }
            vec![Extracted {
                quote: THURSDAY.to_string(),
                duplicate_of: Some("mem-agent".to_string()),
                ..Extracted::default()
            }]
            .into_iter()
            .filter(|_| ids_containing(existing, "on Thursdays") == ["mem-agent"])
            .collect()
        },
    )
    .await;
    assert_eq!(
        explicit_search(&handler, (SECOND, "s2"), &project)
            .await
            .len(),
        3,
        "the agent's note, the user's statement, and the assistant's report"
    );

    capture(
        &handler,
        SECOND,
        "s2",
        &[("m3", "user", TUESDAY)],
        |_, existing| {
            let statement = ids_containing(existing, &stated(THURSDAY));
            let report = ids_containing(existing, "Assistant reported");
            assert_eq!((statement.len(), report.len()), (1, 1), "{existing:?}");
            vec![Extracted {
                quote: TUESDAY.to_string(),
                replaces: statement.into_iter().chain(report).collect(),
                ..Extracted::default()
            }]
        },
    )
    .await;
    assert_eq!(
        explicit_search(&handler, (THIRD, "s3"), &project).await,
        [(stated(TUESDAY), "visible".to_string())],
        "the statement, the report, and the note it restated are retired; the correction record stays"
    );
    let block = project_memory_block(&serve(&handler, THIRD, "s3").await).unwrap();
    assert!(block.contains(&stated(TUESDAY)), "{block}");
    assert!(!block.contains("every Thursday"), "{block}");
}

/// A project holds eighty long assistant reports that share most of their
/// words with a long user message, and one short statement. The message
/// corrects the statement in one sentence. The statement is among the
/// memories the extractor is offered, so the correction can replace it.
#[tokio::test]
async fn a_correction_is_offered_the_short_statement_it_corrects_among_many_long_memories() {
    let (handler, _store, _dir, _project) = delivery_handler(capture_config()).await;
    let statement = "The staging cluster alias is harbor-east.";
    capture(
        &handler,
        FIRST,
        "s1",
        &[("m0", "user", statement)],
        |_, _| vec![fact(statement)],
    )
    .await;
    let reports: Vec<(String, String)> = (0..80)
        .map(|index| {
            (
                format!("r{index}"),
                format!(
                    "Run {index}: the upload handler retries each failed request three times, the retry budget resets when a request succeeds, the subscriber test blocks during shutdown, and the worker pool drains before the listener closes."
                ),
            )
        })
        .collect();
    for batch in reports.chunks(16) {
        let messages: Vec<(&str, &str, &str)> = batch
            .iter()
            .map(|(id, text)| (id.as_str(), "assistant", text.as_str()))
            .collect();
        capture(&handler, FIRST, "s1", &messages, |text, _| vec![fact(text)]).await;
    }
    let correction = "The staging cluster alias changed: it is harbor-west now, not harbor-east.";
    let message = format!(
        "The subscriber test that blocks during shutdown is the one I would question: the upload handler retries each failed request, the retry budget resets when a request succeeds, and the worker pool drains before the listener closes, so look at the drain order again.\n\n{correction}"
    );
    let offered = std::cell::Cell::new(false);
    capture(
        &handler,
        SECOND,
        "s2",
        &[("m1", "user", &message)],
        |_, existing| {
            let target = ids_containing(existing, statement);
            offered.set(target.len() == 1);
            vec![Extracted {
                quote: correction.to_string(),
                replaces: target,
                ..Extracted::default()
            }]
        },
    )
    .await;
    assert!(offered.get(), "the statement is among the offered memories");
    let block = project_memory_block(&serve(&handler, THIRD, "s3").await).unwrap();
    assert!(block.contains(&stated(correction)), "{block}");
    assert!(!block.contains(&stated(statement)), "{block}");

    // An assistant's long report that names the alias is offered the user's
    // statement too, so it can contest it.
    let report = "The upload handler retries each failed request three times and the retry budget resets when a request succeeds; while checking the drain order I saw that deploy/staging.toml sets the staging cluster alias to harbor-north.";
    let offered = std::cell::Cell::new(false);
    capture(
        &handler,
        SECOND,
        "s2",
        &[("m2", "assistant", report)],
        |_, existing| {
            let target = ids_containing(existing, &stated(correction));
            offered.set(target.len() == 1);
            vec![Extracted {
                quote: report.to_string(),
                contests: target,
                ..Extracted::default()
            }]
        },
    )
    .await;
    assert!(
        offered.get(),
        "the statement is offered to the assistant's observation"
    );
    let block = project_memory_block(&serve(&handler, THIRD, "s3").await).unwrap();
    assert!(
        block.contains(&format!("{} (stated ", stated(correction))) && block.contains("contested"),
        "{block}"
    );
}

/// A project holds one user statement about a limit and eighty assistant
/// reports, each naming one handler of the same upload path. A long assistant
/// message lists forty of those handlers and reports that the limit's file
/// holds another value. Forty reports each share a rare name with the
/// message, and every report shares more of its common words than the
/// statement does, so one ranking over all memories fills with reports. The
/// statement is still among the memories the extractor is offered, so the
/// observation can contest it.
#[tokio::test]
async fn an_observation_is_offered_the_statement_it_bears_on_among_many_reports_of_its_topic() {
    let (handler, _store, _dir, _project) = delivery_handler(capture_config()).await;
    let statement = "A queued upload is held 30 seconds before the gateway gives up on it.";
    capture(
        &handler,
        FIRST,
        "s1",
        &[("m0", "user", statement)],
        |_, _| vec![fact(statement)],
    )
    .await;
    let reports: Vec<(String, String)> = (0..80)
        .map(|index| {
            (
                format!("r{index}"),
                format!(
                    "Checked handler{index:02}: gateway forwards each queued upload through stage{index:02}."
                ),
            )
        })
        .collect();
    for batch in reports.chunks(16) {
        let messages: Vec<(&str, &str, &str)> = batch
            .iter()
            .map(|(id, text)| (id.as_str(), "assistant", text.as_str()))
            .collect();
        capture(&handler, FIRST, "s1", &messages, |text, _| vec![fact(text)]).await;
    }
    let handlers: Vec<String> = (0..40).map(|index| format!("handler{index:02}")).collect();
    let report = format!(
        "gateway/backlog.rs now sets UPLOAD_BACKLOG_MS to 45000, so a queued upload stays in the backlog 45000 ms. Checked again: gateway forwards each queued upload through {}.",
        handlers.join(", ")
    );
    let offered = std::cell::Cell::new(false);
    capture(
        &handler,
        SECOND,
        "s2",
        &[("m1", "assistant", &report)],
        |_, existing| {
            let target = ids_containing(existing, &stated(statement));
            offered.set(target.len() == 1);
            vec![Extracted {
                quote: report.clone(),
                contests: target,
                ..Extracted::default()
            }]
        },
    )
    .await;
    assert!(
        offered.get(),
        "the statement is offered among eighty reports that share more of the message's words"
    );
    let block = project_memory_block(&serve(&handler, THIRD, "s3").await).unwrap();
    assert!(
        block.contains(&format!("{} (stated ", stated(statement)))
            && block.contains("; contested ")
            && block.contains("UPLOAD_BACKLOG_MS to 45000"),
        "{block}"
    );
    assert!(
        block.contains("handler00") && !block.contains("handler39"),
        "the contesting text is cut to its excerpt: {block}"
    );
    assert!(block.contains("\u{2026}\")"), "{block}");
}

/// The memory rows on explicit search as `(id, summary, created commit,
/// rationale)`.
fn memory_rows(handler: &Handler, project: &Path) -> Vec<(String, String, i64, Value)> {
    let kernel = handler.kernel.kernel_store().unwrap();
    let read = kernel_routes::read::read_visible(
        &kernel,
        &kernel_routes::ProjectBinding::new(project),
        kernel::Surface::ExplicitSearch,
        None,
        kernel_routes::read::RowSelection::DomainDecisions("memory"),
    )
    .unwrap();
    read.rows
        .iter()
        .map(|row| {
            let decision = &read.decisions[&row.object.object_id];
            (
                row.object.object_id.clone(),
                decision.payload.summary.clone(),
                row.object.created_commit_seq,
                serde_json::from_str(&decision.payload.rationale).unwrap_or(Value::Null),
            )
        })
        .collect()
}

/// One assistant message makes two observations that both contradict a
/// statement. The publication commits: the statement is contested once, by
/// the first observation, and both observations are stored. The record
/// keeps the commit that first recorded the statement. The session whose
/// message recorded the contest sees the statement without the contest
/// text, and once the observation is retired no session sees it.
#[tokio::test]
async fn a_statement_is_contested_once_per_message_and_shown_by_live_observations_of_other_sessions()
 {
    let (handler, store, _dir, project) = delivery_handler(capture_config()).await;
    capture(
        &handler,
        FIRST,
        "s1",
        &[("m1", "user", LIMIT_300)],
        |_, _| vec![fact(LIMIT_300)],
    )
    .await;
    let stated_seq = memory_rows(&handler, &project)[0].2;
    let first = "The client limit in src/limits.rs is 500 now, above the 300 clients stated.";
    let second = "The deploy config caps clients at 500 too, not the 300 clients stated.";
    let both = format!("{first} {second}");
    capture(
        &handler,
        SECOND,
        "s2",
        &[("m2", "assistant", &both)],
        |_, existing| {
            let statement = ids_containing(existing, &stated(LIMIT_300));
            [first, second]
                .into_iter()
                .map(|quote| Extracted {
                    quote: quote.to_string(),
                    contests: statement.clone(),
                    ..Extracted::default()
                })
                .collect()
        },
    )
    .await;
    let status = store
        .memory_capture_status(kernel_routes::ProjectBinding::new(&project).digest())
        .unwrap();
    assert_eq!(
        (status.pending, status.failed),
        (0, 0),
        "the publication committed"
    );
    let rows = memory_rows(&handler, &project);
    assert_eq!(
        rows.len(),
        3,
        "the statement and both observations: {rows:?}"
    );
    let statement = rows
        .iter()
        .find(|row| row.1 == stated(LIMIT_300))
        .expect("the statement is live");
    let observation = rows
        .iter()
        .find(|row| row.1 == format!("Assistant reported: {first}"))
        .expect("the first observation is stored");
    assert_eq!(statement.3["contested_by"]["memory"], observation.0);
    assert_eq!(
        statement.3["contested_by"]["stated_seq"], stated_seq,
        "the record keeps the commit that first recorded the statement"
    );
    assert!(statement.2 > stated_seq, "the contest is a later commit");
    assert_eq!(statement.3["contested_by"]["session"], "s2");

    let other = project_memory_block(&serve(&handler, THIRD, "s3").await).unwrap();
    assert!(
        other.contains(&format!(
            "by {}: \"Assistant reported: {first}\")",
            observation.0
        )),
        "{other}"
    );
    let own = project_memory_block(&serve(&handler, SECOND, "s2").await).unwrap();
    assert!(
        own.contains(&stated(LIMIT_300)) && !own.contains("contested") && !own.contains("500"),
        "the contesting session's block holds the statement without its own assistant text: {own}"
    );

    let retired = body(
        handler
            .dispatch_value_for_test(
                test_route(FIRST),
                kernel_route_fixtures::commit_request(
                    &project,
                    "s1",
                    "retire-observation",
                    vec![json!({"op": "retire_decision", "object_id": observation.0})],
                    Vec::new(),
                ),
            )
            .await,
    );
    assert_eq!(retired["state"]["kind"], "available", "{retired}");
    let after = project_memory_block(&serve(&handler, THIRD, "s3").await).unwrap();
    assert!(
        after.contains(&stated(LIMIT_300)) && !after.contains("contested"),
        "a retired observation contests nothing: {after}"
    );
}

/// A statement an observation contests, and the observation that contests
/// it: the handler, the project, and the two memory ids.
async fn contested_limit() -> (Handler, tempfile::TempDir, PathBuf, String, String) {
    let (handler, _store, dir, project) = delivery_handler(capture_config()).await;
    capture(
        &handler,
        FIRST,
        "s1",
        &[("m1", "user", LIMIT_300)],
        |_, _| vec![fact(LIMIT_300)],
    )
    .await;
    capture(
        &handler,
        SECOND,
        "s2",
        &[("m2", "assistant", LIMIT_OBSERVED)],
        |_, existing| {
            vec![Extracted {
                quote: LIMIT_OBSERVED.to_string(),
                contests: ids_containing(existing, "300 clients"),
                ..Extracted::default()
            }]
        },
    )
    .await;
    let rows = memory_rows(&handler, &project);
    let id = |needle: &str| {
        rows.iter()
            .find(|(_, summary, ..)| summary.contains(needle))
            .map(|(id, ..)| id.clone())
            .unwrap()
    };
    let (statement, observation) = (id("at most 300"), id("500 now"));
    let block = project_memory_block(&serve(&handler, THIRD, "s3").await).unwrap();
    assert!(
        block.contains("contested") && block.contains("500 now"),
        "{block}"
    );
    (handler, dir, project, statement, observation)
}

/// The contested statement's record names the observation and holds none of
/// its text, and a reader shows the observation's text while the Kernel
/// serves that memory. When a disposition hides the observation, the block
/// shows the statement alone, and the statement's stored row holds no word
/// of the observation.
#[tokio::test]
async fn a_contest_shows_the_observation_only_while_the_kernel_serves_it() {
    let (handler, _dir, project, statement, observation) = contested_limit().await;
    let stored = memory_rows(&handler, &project);
    let (_, summary, _, rationale) = stored
        .iter()
        .find(|(id, ..)| *id == statement)
        .expect("the contested statement");
    assert_eq!(*summary, stated(LIMIT_300));
    assert_eq!(rationale["contested_by"]["memory"], observation);
    assert!(
        !rationale.to_string().contains("500"),
        "the statement's row holds none of the observation's text: {rationale}"
    );

    let rejected = body(
        handler
            .dispatch_value_for_test(
                test_route(FIRST),
                kernel_route_fixtures::commit_request(
                    &project,
                    "s1",
                    "reject-observation",
                    vec![json!({"op": "disposition", "object_id": observation, "event": "explicit_reject"})],
                    Vec::new(),
                ),
            )
            .await,
    );
    assert_eq!(rejected["state"]["kind"], "available", "{rejected}");
    let block = project_memory_block(&serve(&handler, THIRD, "s3").await).unwrap();
    assert!(block.contains(&stated(LIMIT_300)), "{block}");
    assert!(
        !block.contains("contested") && !block.contains("500"),
        "a hidden observation contests nothing a reader is shown: {block}"
    );
    assert_eq!(
        live_statement_records(&handler, &project),
        [stated(LIMIT_300)],
        "the statement keeps its approval"
    );
}

/// The user states the contested fact again. The extractor names the
/// statement as the memory that already holds it, so nothing new is written,
/// and the statement is shown without the contest: the user's own later
/// statement settles it. The observation stays stored.
#[tokio::test]
async fn the_users_restatement_of_a_contested_statement_settles_the_contest() {
    let (handler, _dir, project, ..) = contested_limit().await;
    let again = format!("To confirm what we agreed. {LIMIT_300}");
    capture(
        &handler,
        SECOND,
        "s2",
        &[("m3", "user", &again)],
        |_, existing| {
            let held = ids_containing(existing, &stated(LIMIT_300));
            assert_eq!(held.len(), 1, "{existing:?}");
            assert_eq!(
                existing
                    .iter()
                    .find(|memory| memory["id"] == held[0].as_str())
                    .map(|memory| memory["contested"].clone()),
                Some(json!(true)),
                "the extractor is told the statement is contested"
            );
            vec![Extracted {
                quote: LIMIT_300.to_string(),
                duplicate_of: held.into_iter().next(),
                ..Extracted::default()
            }]
        },
    )
    .await;
    let block = project_memory_block(&serve(&handler, THIRD, "s3").await).unwrap();
    assert!(
        block.contains(&stated(LIMIT_300)) && !block.contains("contested"),
        "{block}"
    );
    let mut rows = explicit_search(&handler, (THIRD, "s3"), &project).await;
    rows.sort();
    assert_eq!(
        rows,
        [
            (
                format!("Assistant reported: {LIMIT_OBSERVED}"),
                "labeled".to_string()
            ),
            (stated(LIMIT_300), "visible".to_string()),
        ],
        "the statement and the observation are both stored, once each"
    );
    assert_eq!(
        live_statement_records(&handler, &project),
        [stated(LIMIT_300)]
    );
    assert!(
        memory_rows(&handler, &project)
            .iter()
            .all(|(.., rationale)| rationale.get("contested_by").is_none()),
        "no row names a contest"
    );
}

/// One capture call holds a question about a statement and the user's
/// correction of it. The question would contest the statement the
/// correction replaces; the batch settles that before either plan freezes,
/// so both sources commit in the one pass. Two assistant reports of one
/// batch that contest one statement leave one record on it.
#[tokio::test]
async fn one_batch_settles_its_contests_across_sources_before_any_plan_commits() {
    let (handler, store, _dir, project) = delivery_handler(capture_config()).await;
    let digest = kernel_routes::ProjectBinding::new(&project)
        .digest()
        .to_owned();
    capture(
        &handler,
        FIRST,
        "s1",
        &[("m1", "user", THURSDAY)],
        |_, _| vec![fact(THURSDAY)],
    )
    .await;
    let question = "Did we move the release cut from Thursday to Tuesday?";
    capture(
        &handler,
        SECOND,
        "s2",
        &[("m2", "user", question), ("m3", "user", TUESDAY)],
        |text, existing| {
            vec![Extracted {
                quote: text.to_string(),
                replaces: ids_containing(existing, &stated(THURSDAY)),
                ..Extracted::default()
            }]
        },
    )
    .await;
    let status = store.memory_capture_status(&digest).unwrap();
    assert_eq!(
        (status.pending, status.failed),
        (0, 0),
        "both sources commit in the pass that extracted them"
    );
    let mut rows = explicit_search(&handler, (THIRD, "s3"), &project).await;
    rows.sort();
    assert_eq!(
        rows,
        [
            (stated(question), "labeled".to_string()),
            (stated(TUESDAY), "visible".to_string()),
        ]
    );
    let block = project_memory_block(&serve(&handler, THIRD, "s3").await).unwrap();
    assert!(
        block.contains(&stated(TUESDAY))
            && !block.contains("every Thursday")
            && !block.contains("contested"),
        "{block}"
    );

    let reports = [
        "deploy.toml now cuts the release on Wednesday, so the Tuesday cut no longer holds.",
        "The CI calendar also shows a Wednesday release cut, so Tuesday no longer holds.",
    ];
    capture(
        &handler,
        SECOND,
        "s2",
        &[
            ("m4", "assistant", reports[0]),
            ("m5", "assistant", reports[1]),
        ],
        |text, existing| {
            vec![Extracted {
                quote: text.to_string(),
                contests: ids_containing(existing, &stated(TUESDAY)),
                ..Extracted::default()
            }]
        },
    )
    .await;
    let status = store.memory_capture_status(&digest).unwrap();
    assert_eq!((status.pending, status.failed), (0, 0));
    assert_eq!(memory_rows(&handler, &project).len(), 4);
    let block = project_memory_block(&serve(&handler, THIRD, "s3").await).unwrap();
    assert!(
        block.contains(&format!(": \"Assistant reported: {}\")", reports[0])),
        "the first report of the batch is the one record on the statement: {block}"
    );
}

/// A correction replaces a statement that was recorded beside an agent's
/// note, and a second memory of the same message names that note. The note
/// is retired with the statement after every memory has checked its
/// targets, so the source commits. A note that holds the statement's fact
/// among others of its own is kept.
#[tokio::test]
async fn a_restated_note_is_retired_after_the_plans_targets_are_checked_and_only_when_it_states_the_fact()
 {
    for (note, retired) in [
        (
            "Release branches are cut from main every Thursday morning.",
            true,
        ),
        (
            "Release branches are cut from main every Thursday, the on-call rotation changes on Mondays, and the staging database is refreshed from production each Sunday night.",
            false,
        ),
    ] {
        let (handler, store, _dir, project) = delivery_handler(capture_config()).await;
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
                            "payload": {"summary": note, "rationale": ""},
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
            FIRST,
            "s1",
            &[("m1", "user", THURSDAY)],
            |_, _| {
                vec![Extracted {
                    quote: THURSDAY.to_string(),
                    duplicate_of: Some("mem-agent".to_string()),
                    ..Extracted::default()
                }]
            },
        )
        .await;
        let both = format!("{TUESDAY} Keep the changelog current.");
        capture(
            &handler,
            SECOND,
            "s2",
            &[("m2", "user", &both)],
            |_, existing| {
                vec![
                    Extracted {
                        quote: TUESDAY.to_string(),
                        replaces: ids_containing(existing, &stated(THURSDAY)),
                        ..Extracted::default()
                    },
                    Extracted {
                        quote: "Keep the changelog current.".to_string(),
                        contests: vec!["mem-agent".to_string()],
                        ..Extracted::default()
                    },
                ]
            },
        )
        .await;
        let status = store
            .memory_capture_status(kernel_routes::ProjectBinding::new(&project).digest())
            .unwrap();
        assert_eq!((status.pending, status.failed), (0, 0), "{note}");
        let rows = explicit_search(&handler, (THIRD, "s3"), &project).await;
        assert!(
            rows.contains(&(stated(TUESDAY), "visible".to_string()))
                && !rows.iter().any(|(summary, _)| *summary == stated(THURSDAY)),
            "the correction replaces the statement: {rows:?}"
        );
        assert_eq!(
            rows.iter().any(|(summary, _)| summary == note),
            !retired,
            "{note}: {rows:?}"
        );
    }
}

/// The user states a fact the agent's note holds, and an assistant report
/// repeats it. A correction replaces the report and names the statement as
/// a further target. The statement is retired through that further target,
/// and the note it was recorded beside is retired with it.
#[tokio::test]
async fn a_statement_retired_as_a_further_target_takes_its_restated_note_with_it() {
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
                        "payload": {"summary": "Release branches are cut from main every Thursday morning.", "rationale": ""},
                        "source_id": "eidnara_memory",
                        "source_revision": 1,
                    }})],
                    Vec::new(),
                ),
            )
            .await,
    );
    assert_eq!(written["state"]["kind"], "available", "{written}");
    let report = "Release branches are cut from main every Thursday, per the checklist.";
    capture(
        &handler,
        FIRST,
        "s1",
        &[("m1", "user", THURSDAY), ("m2", "assistant", report)],
        |text, _| {
            if text == report {
                return vec![fact(report)];
            }
            vec![Extracted {
                quote: THURSDAY.to_string(),
                duplicate_of: Some("mem-agent".to_string()),
                ..Extracted::default()
            }]
        },
    )
    .await;
    assert_eq!(memory_rows(&handler, &project).len(), 3);
    capture(
        &handler,
        SECOND,
        "s2",
        &[("m3", "user", TUESDAY)],
        |_, existing| {
            let report = ids_containing(existing, "Assistant reported");
            let statement = ids_containing(existing, &stated(THURSDAY));
            assert_eq!((report.len(), statement.len()), (1, 1), "{existing:?}");
            vec![Extracted {
                quote: TUESDAY.to_string(),
                replaces: report.into_iter().chain(statement).collect(),
                ..Extracted::default()
            }]
        },
    )
    .await;
    assert_eq!(
        explicit_search(&handler, (THIRD, "s3"), &project).await,
        [(stated(TUESDAY), "visible".to_string())],
        "the report, the statement, and the note the statement restated are retired"
    );
}

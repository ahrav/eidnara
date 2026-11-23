//! Tests for the record of a project file the user named and the agent used, and for the
//! `NAMED_FILES` element a later session's `<project-memory>` block holds.

use super::*;

const NOTES: &str = "docs/release-notes.md";
const BODY: &str = "Tag the release before publishing the crate.";

fn call(mid: &str, ordinal: u64, name: &str, input: Value) -> IngressMessage {
    IngressMessage {
        mid: mid.to_string(),
        ordinal,
        ck: WireMessage::from_parts(
            "assistant",
            vec![WireBlock::bare(BlockKind::ToolCall {
                id: format!("call-{mid}"),
                name: name.to_string(),
                input,
                provider_executed: false,
            })],
            None,
            ProviderExtras::new(),
            HarnessMeta {
                harness_id: Some(mid.to_string()),
                ..Default::default()
            },
        ),
    }
}

/// The result the tool answered the call of message `mid` with.
fn answer(mid: &str, ordinal: u64, name: &str, output: OutputKind) -> IngressMessage {
    IngressMessage {
        mid: format!("{mid}-result"),
        ordinal,
        ck: WireMessage::from_parts(
            "tool",
            vec![WireBlock::bare(BlockKind::ToolResult {
                id: format!("call-{mid}"),
                tool_name: name.to_string(),
                output: ToolOutput::bare(output),
                provider_executed: false,
            })],
            None,
            ProviderExtras::new(),
            HarnessMeta::default(),
        ),
    }
}

/// A call of `name` and the text the tool answered it with.
fn answered(mid: &str, ordinal: u64, name: &str, input: Value) -> [IngressMessage; 2] {
    let done = OutputKind::Text {
        text: "done".into(),
    };
    [
        call(mid, ordinal, name, input),
        answer(mid, ordinal, name, done),
    ]
}

fn read(mid: &str, ordinal: u64, path: &str) -> [IngressMessage; 2] {
    answered(mid, ordinal, "read", json!({ "path": path }))
}

fn shell(mid: &str, ordinal: u64, command: &str) -> [IngressMessage; 2] {
    answered(mid, ordinal, "bash", json!({ "command": command }))
}

fn session_binding(project: &Path, session: &str) -> SessionBinding {
    SessionBinding {
        config: DaemonConfig {
            memory_auto_capture: true,
            ..default_test_config()
        },
        ..binding_with_harness(project.to_str().unwrap(), "pi", session)
    }
}

/// A Pi window of `session`, whose working directory is the project root.
fn window(session: &str, messages: &[IngressMessage]) -> TransformRequest {
    let mut request = capture_transform_request(messages);
    request.session_id = session.to_string();
    request.working_directory = Some(String::new());
    request
}

async fn project_with(files: &[(&str, &str)]) -> (Handler, tempfile::TempDir, PathBuf) {
    let (handler, _store, dir, project) =
        handler_with_store_and_kernel(Arc::new(ProducerState::default()), default_test_config())
            .await;
    for (path, body) in files {
        let path = project.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    (handler, dir, project)
}

fn write(handler: &Handler, record: named_files::NamedFileRecord) -> usize {
    record.write(
        &handler.kernel,
        &handler.capture_commit_gate,
        &handler.named_files,
    )
}

/// Shows the daemon one admitted pass of the binding's session and writes its record. Returns
/// how many files the record holds.
fn pass(handler: &Handler, binding: &SessionBinding, request: &TransformRequest) -> usize {
    handler
        .observe_named_files(binding, request)
        .and_then(|observed| {
            handler.named_files.keep(
                &binding.kernel_project,
                &binding.session,
                &observed,
                now_ms(),
            )
        })
        .map_or(0, |record| write(handler, record))
}

/// Plays one turn of `session`: the pass that shows the user's message and the pass that
/// shows the turn's calls. Returns how many files the turn recorded.
fn turn(
    handler: &Handler,
    project: &Path,
    session: &str,
    said: &str,
    calls: impl IntoIterator<Item = IngressMessage>,
) -> usize {
    let binding = session_binding(project, session);
    let mut messages = vec![ck(&format!("{session}-u1"), 1, said)];
    assert_eq!(
        pass(handler, &binding, &window(session, &messages)),
        0,
        "a message no call follows shows no use"
    );
    messages.extend(calls);
    pass(handler, &binding, &window(session, &messages))
}

/// Waits for the tracked jobs the handler started.
async fn settle(handler: &Handler) {
    handler.tasks.close();
    tokio::time::timeout(Duration::from_secs(10), handler.tasks.wait())
        .await
        .unwrap();
    handler.tasks.reopen();
}

/// The stored records of the project, each as its path and its rationale, read from the Kernel
/// by the domain's name.
fn stored_records(handler: &Handler, project: &Path) -> Vec<(String, Value)> {
    let store = handler.kernel.kernel_store().unwrap();
    let binding = session_binding(project, "reader");
    let read = kernel_routes::read::read_visible_within(
        &store,
        &binding.kernel_project,
        kernel::Surface::ExplicitSearch,
        Some(store.tip().unwrap()),
        kernel_routes::read::RowSelection::DomainDecisions("named-files"),
        (1_000, 1 << 20),
    )
    .unwrap();
    let mut records: Vec<(String, Value)> = read
        .rows
        .iter()
        .map(|row| {
            let decision = &read.decisions[&row.object.object_id];
            assert_eq!(row.object.domain_id, "named-files");
            assert_eq!(row.object.source_id, "named_file_use");
            assert_eq!(decision.decision_kind, "named_file_use");
            assert!(decision.object_id.starts_with("nfu_"), "{decision:?}");
            (
                decision.payload.summary.clone(),
                serde_json::from_str(&decision.payload.rationale).unwrap(),
            )
        })
        .collect();
    records.sort_by(|left, right| left.0.cmp(&right.0));
    records
}

fn stored_paths(handler: &Handler, project: &Path) -> Vec<String> {
    stored_records(handler, project)
        .into_iter()
        .map(|(path, _)| path)
        .collect()
}

fn delivered_lines(handler: &Handler, project: &Path, session: &str) -> Vec<String> {
    let binding = session_binding(project, session);
    canonical_memory::read_project_memory(
        &handler.kernel,
        &binding.kernel_project,
        session,
        binding.config.memory_budget_tokens,
    )
    .rows()
    .iter()
    .filter(|row| row.category == memory_render::NAMED_FILES_SECTION)
    .map(|row| row.content.clone())
    .collect()
}

/// The paths the lines of [`delivered_lines`] hold, in order.
fn delivered_paths(handler: &Handler, project: &Path, session: &str) -> Vec<String> {
    delivered_lines(handler, project, session)
        .iter()
        .map(|line| line.split(' ').next().unwrap().to_owned())
        .collect()
}

/// The user names a document and the agent reads it: the stored record holds the path, the
/// session, and the time of the pass, the next session's block holds the path and the number
/// of sessions that used it on a line of its own, and neither holds the document's text. Once
/// the file is removed the block holds no line for it.
#[tokio::test]
async fn a_named_file_the_agent_read_is_delivered_by_path_while_it_exists() {
    let (handler, _dir, project) = project_with(&[(NOTES, BODY)]).await;
    let said = format!("The release steps are written down in {NOTES}. Follow them.");
    let before = now_ms();
    let shown = OutputKind::Text { text: BODY.into() };
    let calls = [
        call("a1", 2, "read", json!({ "path": NOTES })),
        answer("a1", 3, "read", shown),
    ];
    assert_eq!(turn(&handler, &project, "s1", &said, calls), 1);

    let records = stored_records(&handler, &project);
    assert_eq!(records.len(), 1, "{records:?}");
    let (path, rationale) = &records[0];
    assert_eq!(path, "docs/release-notes.md");
    let fields = rationale.as_object().unwrap();
    let mut keys: Vec<&str> = fields.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["session", "source", "used_ms"]);
    assert_eq!(fields["source"], "named_file_use");
    assert_eq!(fields["session"], "s1");
    assert!(
        (before..=now_ms()).contains(&fields["used_ms"].as_i64().unwrap()),
        "{rationale}"
    );
    assert!(!rationale.to_string().contains("Tag the release"));

    let binding = session_binding(&project, "s2");
    let read = canonical_memory::read_project_memory(
        &handler.kernel,
        &binding.kernel_project,
        "s2",
        binding.config.memory_budget_tokens,
    );
    let block = memory_render::render_delivered_memory_block(read.rows());
    let lines: Vec<&str> = block.lines().collect();
    assert!(lines[1].starts_with("<NAMED_FILES note=\""), "{block}");
    assert!(
        lines[2]
            .starts_with("docs/release-notes.md (the agent used it in 1 earlier session, last on "),
        "the line is the path and its use, with no id: {block}"
    );
    assert_eq!(lines[3], "</NAMED_FILES>", "{block}");
    assert!(!block.contains("Tag the release"), "{block}");
    assert!(!block.contains("nfu_"), "{block}");
    assert!(
        delivered_lines(&handler, &project, "s1").is_empty(),
        "the session that used the file holds the use in its own conversation"
    );

    std::fs::remove_file(project.join(NOTES)).unwrap();
    assert!(delivered_lines(&handler, &project, "s2").is_empty());
}

/// A file the agent passes to a shell command is recorded when the command line holds its
/// path as a word, with shell punctuation around it, and a file the user names with Pi's `@`
/// reference is recorded under its path.
#[tokio::test]
async fn a_named_script_the_agent_ran_and_a_pi_file_reference_are_recorded() {
    let (handler, _dir, project) =
        project_with(&[("tools/check-links.sh", "#!/bin/sh\n"), (NOTES, BODY)]).await;
    let said = "Run `sh tools/check-links.sh` before you finish and fix what it reports.";
    let ran = shell("a1", 2, "sh tools/check-links.sh; echo rc=$?");
    assert_eq!(turn(&handler, &project, "s1", said, ran), 1);
    assert_eq!(
        delivered_paths(&handler, &project, "reader"),
        ["tools/check-links.sh"]
    );

    let said = format!("Summarize @{NOTES} for me.");
    let calls = read("a1", 2, &format!("@{NOTES}"));
    assert_eq!(turn(&handler, &project, "s2", &said, calls), 1);
    assert_eq!(
        stored_paths(&handler, &project),
        [NOTES, "tools/check-links.sh"]
    );
}

/// A pass records a file only when the user's message names it, a call of Pi's `read` or
/// `bash` after the message uses it, the call's result is no error, and the path names a
/// regular file: a file the agent found by itself, a file the user named that no call
/// touched, a file only an edit call names, a read that failed, and a path that names no
/// file are each left out.
#[tokio::test]
async fn a_pass_records_only_named_files_a_read_or_a_command_used() {
    let (handler, _dir, project) = project_with(&[
        (NOTES, BODY),
        ("src/parser.rs", "fn parse() {}\n"),
        ("README.md", "# Demo\n"),
    ])
    .await;

    // Found by the agent, named by nobody.
    let unnamed = read("a1", 2, "README.md");
    assert_eq!(
        turn(&handler, &project, "s1", "Please add a changelog.", unnamed),
        0
    );

    // Named and never opened.
    let said = format!("Keep {NOTES} in mind for later.");
    assert_eq!(turn(&handler, &project, "s2", &said, []), 0);

    // Named, and named by an edit call alone.
    let edit = answered(
        "a1",
        2,
        "edit",
        json!({ "path": "src/parser.rs", "oldText": "{}", "newText": "{ todo!() }" }),
    );
    let said = "Fix the bug in src/parser.rs.";
    assert_eq!(turn(&handler, &project, "s3", said, edit), 0);

    // Named, and read by a call the tool answered with an error.
    let failed = OutputKind::ErrorText {
        text: "permission denied".into(),
    };
    let calls = [
        call("a1", 2, "read", json!({ "path": NOTES })),
        answer("a1", 3, "read", failed),
    ];
    let said = format!("Read {NOTES} first.");
    assert_eq!(turn(&handler, &project, "s4", &said, calls), 0);

    // Named and read, and absent from the project.
    let said = "Show me docs/handover.md.";
    let calls = shell("a1", 2, "cat docs/handover.md");
    assert_eq!(turn(&handler, &project, "s5", said, calls), 0);

    assert!(stored_records(&handler, &project).is_empty());
    assert!(delivered_lines(&handler, &project, "s9").is_empty());
}

/// A session records a path once. Later passes of the turn and a later turn that names the
/// file again add nothing, and a later turn records the new file it names.
#[tokio::test]
async fn a_session_records_each_path_once_over_its_turns() {
    let (handler, _dir, project) =
        project_with(&[(NOTES, BODY), ("docs/plan.md", "Plan.\n")]).await;
    let binding = session_binding(&project, "s1");
    let said = format!("Read {NOTES} first.");
    let mut messages = vec![ck("s1-u1", 1, &said)];
    messages.extend(read("a1", 2, NOTES));
    assert_eq!(pass(&handler, &binding, &window("s1", &messages)), 1);
    messages.extend(read("a2", 3, NOTES));
    assert_eq!(pass(&handler, &binding, &window("s1", &messages)), 0);

    let again = format!("Now compare {NOTES} with docs/plan.md.");
    messages.push(ck("s1-u2", 4, &again));
    assert_eq!(pass(&handler, &binding, &window("s1", &messages)), 0);
    messages.extend(read("a3", 5, NOTES));
    assert_eq!(pass(&handler, &binding, &window("s1", &messages)), 0);
    messages.extend(read("a4", 6, "docs/plan.md"));
    assert_eq!(pass(&handler, &binding, &window("s1", &messages)), 1);
    assert_eq!(stored_paths(&handler, &project), ["docs/plan.md", NOTES]);
    assert_eq!(
        stored_records(&handler, &project)
            .iter()
            .map(|(_, rationale)| rationale["session"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>(),
        ["s1", "s1"]
    );
}

/// A file is used by a call that follows the last message the user typed, when that message
/// names the file. The rows before that message show nothing: a fork copies its parent's rows
/// into a session of another id.
#[tokio::test]
async fn a_call_uses_a_file_the_last_typed_message_names() {
    let (handler, _dir, project) =
        project_with(&[(NOTES, BODY), ("docs/plan.md", "Plan.\n")]).await;
    let names_plan = "Now look at docs/plan.md.";

    // The read precedes the message that names the file.
    let binding = session_binding(&project, "s1");
    let mut messages = vec![ck("s1-u1", 1, "Have a look around.")];
    messages.extend(read("a1", 2, "docs/plan.md"));
    messages.push(ck("s1-u2", 3, names_plan));
    messages.push(wire_with_role("a2", 4, "assistant", "It is a plan."));
    assert_eq!(pass(&handler, &binding, &window("s1", &messages)), 0);
    messages.extend(read("a3", 5, "docs/plan.md"));
    assert_eq!(pass(&handler, &binding, &window("s1", &messages)), 1);

    // The message that names the file is followed by a message that names none, and the
    // read follows that one.
    let binding = session_binding(&project, "s2");
    let said = format!("Read {NOTES} first.");
    let mut messages = vec![ck("s2-u1", 1, &said), ck("s2-u2", 2, "Go on.")];
    messages.extend(read("a1", 3, NOTES));
    assert_eq!(pass(&handler, &binding, &window("s2", &messages)), 0);

    // A file the turn reads and then edits is used: the record says the file was read.
    let binding = session_binding(&project, "s3");
    let mut messages = vec![ck("s3-u1", 1, names_plan)];
    messages.extend(read("a1", 2, "docs/plan.md"));
    messages.extend(answered(
        "a2",
        3,
        "edit",
        json!({ "path": "docs/plan.md", "oldText": "Plan", "newText": "Plans" }),
    ));
    assert_eq!(pass(&handler, &binding, &window("s3", &messages)), 1);

    // A fork's first pass shows its parent's last turn before the message typed in the
    // fork, and records none of the parent's uses under the fork's id.
    let fork = session_binding(&project, "s4");
    let mut messages = vec![ck("s0-u1", 1, &said)];
    messages.extend(read("a1", 2, NOTES));
    messages.push(ck("s4-u1", 3, "Thanks."));
    assert_eq!(pass(&handler, &fork, &window("s4", &messages)), 0);
    assert_eq!(
        stored_paths(&handler, &project),
        ["docs/plan.md", "docs/plan.md"]
    );
}

/// A session records files while its working directory is the project root, where a
/// relative path names the file of that path. A request that carries no working directory,
/// or the directory of a package, records nothing.
#[tokio::test]
async fn a_session_records_files_at_the_project_root() {
    let (handler, _dir, project) = project_with(&[
        ("README.md", "# Root\n"),
        ("packages/app/README.md", "# App\n"),
    ])
    .await;
    let binding = session_binding(&project, "s1");
    let mut messages = vec![ck("s1-u1", 1, "Follow README.md.")];
    messages.extend(read("a1", 2, "README.md"));
    let mut request = window("s1", &messages);
    for elsewhere in [None, Some("packages/app"), Some("../elsewhere"), Some(".")] {
        request.working_directory = elsewhere.map(str::to_owned);
        assert_eq!(pass(&handler, &binding, &request), 0, "{elsewhere:?}");
    }
    request.working_directory = Some(String::new());
    assert_eq!(pass(&handler, &binding, &request), 1);
    assert_eq!(stored_paths(&handler, &project), ["README.md"]);
}

/// A session's recorded paths are held for each project a route binds it to: one session id
/// under two project roots that hold the same relative file records the file in each project.
#[tokio::test]
async fn one_session_id_records_a_path_in_each_project_it_is_bound_to() {
    let (handler, dir, project) = project_with(&[(NOTES, BODY)]).await;
    let other = dir.path().join("other-project");
    std::fs::create_dir_all(other.join("docs")).unwrap();
    std::fs::write(other.join(NOTES), "Other notes.\n").unwrap();
    let said = format!("Read {NOTES} first.");
    let mut messages = vec![ck("shared-u1", 1, &said)];
    messages.extend(read("a1", 2, NOTES));
    for root in [&project, &other] {
        let binding = session_binding(root, "shared");
        assert_eq!(pass(&handler, &binding, &window("shared", &messages)), 1);
        assert_eq!(pass(&handler, &binding, &window("shared", &messages)), 0);
        assert_eq!(stored_paths(&handler, root), [NOTES]);
    }
}

/// A path outside the project, through a symbolic link to a file or to a directory, or into
/// the project's Git store is no file of the project. A recorded path whose directory a link
/// later replaces is delivered no longer.
#[tokio::test]
async fn a_path_outside_the_project_and_a_link_are_left_out() {
    let (handler, dir, project) = project_with(&[(NOTES, BODY)]).await;
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("outside.md"), "Elsewhere.\n").unwrap();
    std::fs::write(outside.join("release-notes.md"), "Elsewhere.\n").unwrap();
    std::os::unix::fs::symlink(outside.join("outside.md"), project.join("linked.md")).unwrap();
    std::os::unix::fs::symlink(&outside, project.join("linked-dir")).unwrap();
    std::fs::create_dir_all(project.join(".git")).unwrap();
    std::fs::write(project.join(".git/config"), "[core]\n").unwrap();

    let elsewhere = outside.join("outside.md");
    let said = format!(
        "See {}, linked.md, linked-dir/outside.md, ../outside/outside.md, docs/../linked.md \
         and .git/config.",
        elsewhere.display()
    );
    let calls = [
        read("a1", 2, elsewhere.to_str().unwrap()),
        read("a2", 3, "linked.md"),
        read("a3", 4, "linked-dir/outside.md"),
        shell("a4", 5, "cat ../outside/outside.md .git/config"),
        read("a5", 6, "docs/../linked.md"),
    ]
    .into_iter()
    .flatten();
    assert_eq!(turn(&handler, &project, "s2", &said, calls), 0);
    assert!(stored_records(&handler, &project).is_empty());

    let said = format!("Read {NOTES} first.");
    assert_eq!(
        turn(&handler, &project, "s3", &said, read("a1", 2, NOTES)),
        1
    );
    assert_eq!(delivered_paths(&handler, &project, "reader"), [NOTES]);
    std::fs::remove_dir_all(project.join("docs")).unwrap();
    std::os::unix::fs::symlink(&outside, project.join("docs")).unwrap();
    assert!(project.join(NOTES).is_file());
    assert!(delivered_lines(&handler, &project, "reader").is_empty());
}

/// A file more sessions used comes first, then the file used last, then the lesser path. The
/// element holds at most eight lines, looks for the files of the first 32 paths, and lists a
/// recorded path only when it is one the recorder writes.
#[tokio::test]
async fn the_element_orders_files_by_sessions_then_last_use_then_path() {
    let names: Vec<String> = (0..12)
        .map(|index| format!("docs/guide-{index:02}.md"))
        .collect();
    let files: Vec<(&str, &str)> = names
        .iter()
        .map(|name| (name.as_str(), "Guide.\n"))
        .collect();
    let (handler, _dir, project) = project_with(&files).await;
    let store = handler.kernel.kernel_store().unwrap();
    let binding = session_binding(&project, "reader");
    let record = |session: &str, name: &str, used_ms: i64| {
        named_files::record(
            &store,
            &binding.kernel_project,
            session,
            name,
            used_ms,
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
    };
    let day = 86_400_000;
    // One session each, used on days 1 to 12 in an order that differs from the paths' order;
    // guides 3 and 4 share a time.
    for (index, used_day) in [5, 9, 1, 7, 7, 12, 2, 11, 3, 10, 4, 8]
        .into_iter()
        .enumerate()
    {
        record(&format!("s{index}"), &names[index], used_day * day);
    }
    // A second session used guide 2, the one used longest ago.
    record("again", &names[2], 6 * day);

    assert_eq!(
        delivered_paths(&handler, &project, "reader"),
        [2, 5, 7, 9, 1, 11, 3, 4].map(|index: usize| names[index].clone())
    );
    let lines = delivered_lines(&handler, &project, "reader");
    assert_eq!(
        lines[0],
        "docs/guide-02.md (the agent used it in 2 earlier sessions, last on 1970-01-07)"
    );
    assert_eq!(
        lines[1],
        "docs/guide-05.md (the agent used it in 1 earlier session, last on 1970-01-13)"
    );
    // The reading session's own records are left out of its block.
    assert_eq!(
        delivered_paths(&handler, &project, "s5")[..2],
        [names[2].clone(), names[7].clone()]
    );

    // Two newer records of paths no recorder writes, one with a space and one of 257 bytes:
    // they take two of the 32 places that are looked for and are never listed, so the eight
    // lines are the ones before.
    std::fs::write(project.join("docs/with space.md"), "Odd.\n").unwrap();
    record("odd", "docs/with space.md", 39 * day);
    let long = format!("docs/{}.md", "a".repeat(249));
    assert_eq!(long.len(), 257);
    std::fs::write(project.join(&long), "Long.\n").unwrap();
    record("odd", &long, 38 * day);
    assert_eq!(
        delivered_paths(&handler, &project, "reader"),
        [2, 5, 7, 9, 1, 11, 3, 4].map(|index: usize| names[index].clone())
    );
    // Newer records of 28 absent files: guide 2 is the first path, the absent files and the
    // two odd paths take the next 30, guide 5 is the 32nd and the last that is looked for,
    // and guide 7 is the 33rd.
    for index in 0..28 {
        record(
            "ghost",
            &format!("docs/ghost-{index:02}.md"),
            40 * day + index,
        );
    }
    assert_eq!(
        delivered_paths(&handler, &project, "reader"),
        [names[2].clone(), names[5].clone()]
    );
}

/// The records come from primary Pi sessions whose configuration permits memory capture, and
/// from messages the user typed: a subagent's pass, a session with capture off, a session of
/// another harness, and rows of the user role that the user did not type record nothing.
#[tokio::test]
async fn a_record_needs_a_primary_pi_session_with_capture_and_a_typed_message() {
    let (handler, _dir, project) =
        project_with(&[(NOTES, BODY), ("docs/plan.md", "Plan.\n")]).await;
    let said = format!("Read {NOTES} first.");
    let play = |binding: &SessionBinding, subagent: bool| {
        let session = binding.session.clone();
        let mut messages = vec![ck(&format!("{session}-u1"), 1, &said)];
        messages.extend(read("a1", 2, NOTES));
        let mut request = window(&session, &messages);
        request.is_subagent = subagent;
        pass(&handler, binding, &request)
    };

    assert_eq!(play(&session_binding(&project, "subagent"), true), 0);
    let mut capture_off = session_binding(&project, "capture-off");
    capture_off.config.memory_auto_capture = false;
    assert_eq!(play(&capture_off, false), 0);
    let mut other_harness = session_binding(&project, "other-harness");
    other_harness.harness = "opencode".to_string();
    assert_eq!(play(&other_harness, false), 0);
    assert!(stored_records(&handler, &project).is_empty());

    // A summary row and a synthetic row of the user role name a second file after the typed
    // message: the typed message stays the one whose files are read.
    let binding = session_binding(&project, "typed");
    let mut summary = ck("typed-x1", 2, "Earlier the user asked about docs/plan.md.");
    summary.ck.meta.summary = true;
    let mut synthetic = ck("typed-x2", 3, "Reminder: docs/plan.md is open.");
    synthetic.ck.meta.synthetic = true;
    let mut messages = vec![ck("typed-u1", 1, &said), summary, synthetic];
    messages.extend(read("a1", 4, NOTES));
    messages.extend(read("a2", 5, "docs/plan.md"));
    assert_eq!(pass(&handler, &binding, &window("typed", &messages)), 1);
    assert_eq!(stored_paths(&handler, &project), [NOTES]);

    // A row of the user role that Pi's own row gives another role is no typed message.
    let binding = session_binding(&project, "custom");
    let mut messages = vec![ck("custom-u1", 1, &said)];
    messages.extend(read("a1", 2, NOTES));
    let mut request = window("custom", &messages);
    request.native_messages.as_mut().unwrap()[0] = Arc::new(
        json!({"id": "custom-u1", "message": {"role": "custom", "content": said.as_str()}}),
    );
    assert_eq!(pass(&handler, &binding, &request), 0);
    assert_eq!(play(&session_binding(&project, "control"), false), 1);
}

/// A pass observed before a session deletion writes no record after it: the record is
/// refused when it is taken, and a record already taken writes nothing, whether the memo
/// forgot the session or the `session.delete` route ran. A write that starts while the
/// deletion's gate is held, and that the deletion precedes, writes nothing.
#[tokio::test]
async fn a_pass_observed_before_a_deletion_writes_no_record() {
    let (handler, _dir, project) = project_with(&[(NOTES, BODY)]).await;
    handler.bind_route(test_route(WRITER), session_binding(&project, "writer"));
    let said = format!("Read {NOTES} first.");
    let observe = |session: &str| {
        let binding = session_binding(&project, session);
        let mut messages = vec![ck(&format!("{session}-u1"), 1, &said)];
        messages.extend(read("a1", 2, NOTES));
        let observed = handler
            .observe_named_files(&binding, &window(session, &messages))
            .expect("the window shows a use");
        (binding, observed)
    };
    let keep = |binding: &SessionBinding, observed: &named_files::Observed| {
        handler.named_files.keep(
            &binding.kernel_project,
            &binding.session,
            observed,
            now_ms(),
        )
    };

    // Deleted between the observation and the record.
    let (binding, observed) = observe("s1");
    handler.named_files.delete_session("s1");
    assert!(keep(&binding, &observed).is_none());

    // Deleted over the route between the record and its write.
    let (binding, observed) = observe("writer");
    let record = keep(&binding, &observed).expect("the file exists");
    let deleted = handler
        .dispatch_value_for_test(
            test_route(WRITER),
            json!({"method": "session.delete", "session_id": "writer"}),
        )
        .await;
    assert!(
        matches!(deleted, PreparedOutcome::Response(_)),
        "{deleted:?}"
    );
    assert_eq!(write(&handler, record), 0);

    // Deleted while the write waits for the gate the deletion holds.
    let (binding, observed) = observe("s2");
    let record = keep(&binding, &observed).expect("the file exists");
    let gate = handler.capture_commit_gate.lock().unwrap();
    let written = std::thread::scope(|scope| {
        let writing = scope.spawn(|| write(&handler, record));
        handler.named_files.delete_session("s2");
        drop(gate);
        writing.join().unwrap()
    });
    assert_eq!(written, 0);
    assert!(stored_records(&handler, &project).is_empty());

    // A pass observed after the deletion records as any pass does.
    let (binding, observed) = observe("s2");
    let record = keep(&binding, &observed).expect("the file exists");
    assert_eq!(write(&handler, record), 1);
}

/// Two records taken of one observation before either is written: the first write records
/// the path, and the second finds the path recorded under the gate and writes nothing. A
/// record taken while the session had a place writes nothing once the places are taken.
#[tokio::test]
async fn a_write_rechecks_the_sessions_paths_and_places_under_the_gate() {
    let (handler, _dir, project) = project_with(&[(NOTES, BODY)]).await;
    let binding = session_binding(&project, "s1");
    let said = format!("Read {NOTES} first.");
    let mut messages = vec![ck("s1-u1", 1, &said)];
    messages.extend(read("a1", 2, NOTES));
    let observed = handler
        .observe_named_files(&binding, &window("s1", &messages))
        .expect("the window shows a use");
    let keep = |session: &str| {
        handler
            .named_files
            .keep(&binding.kernel_project, session, &observed, now_ms())
            .expect("the file exists")
    };
    let (first, second) = (keep("s1"), keep("s1"));
    assert_eq!(write(&handler, first), 1);
    assert_eq!(write(&handler, second), 0);
    assert_eq!(stored_paths(&handler, &project), [NOTES]);

    let late = keep("s2");
    handler
        .named_files
        .fill_session_for_test(&binding.kernel_project, "s2");
    assert_eq!(write(&handler, late), 0);
    assert_eq!(stored_paths(&handler, &project), [NOTES]);
}

/// A session's second write of a path replays the first: the domain holds one row for the
/// session and the path, with the first write's time.
#[tokio::test]
async fn a_second_write_of_a_path_leaves_the_first_record() {
    let (handler, _dir, project) = project_with(&[(NOTES, BODY)]).await;
    let store = handler.kernel.kernel_store().unwrap();
    let binding = session_binding(&project, "s1");
    for used_ms in [1_000, 2_000] {
        named_files::record(
            &store,
            &binding.kernel_project,
            "s1",
            NOTES,
            used_ms,
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
    }
    let records = stored_records(&handler, &project);
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].1["used_ms"], 1_000);
}

/// A caller of `kernel.commit` changes no named-file record and takes none of the recorder's
/// ids: an insert into the domain, a decision, an observation, or a successor under a
/// record's id prefixes or the domain's object id, a replacement, a retirement, and a
/// disposition of a record are each refused as invalid input, and the record stays as the
/// daemon wrote it. Artifact ingest refuses an artifact under one of the ids, in the domain,
/// or with the recorder's source kind, and stages nothing for it.
#[tokio::test]
async fn a_kernel_caller_changes_no_named_file_record() {
    let (handler, _dir, project) = project_with(&[(NOTES, BODY)]).await;
    handler.bind_route(test_route(WRITER), session_binding(&project, "writer"));
    let said = format!("Read {NOTES} first.");
    assert_eq!(
        turn(&handler, &project, "s1", &said, read("a1", 2, NOTES)),
        1
    );
    let before = stored_records(&handler, &project);
    let store = handler.kernel.kernel_store().unwrap();
    let binding = session_binding(&project, "reader");
    let record = kernel_routes::read::read_visible_within(
        &store,
        &binding.kernel_project,
        kernel::Surface::ExplicitSearch,
        Some(store.tip().unwrap()),
        kernel_routes::read::RowSelection::DomainDecisions("named-files"),
        (10, 1 << 20),
    )
    .unwrap()
    .rows[0]
        .object
        .object_id
        .clone();
    assert!(record.starts_with("nfu_"), "{record}");
    let spec = |object_id: &str, decision_id: &str, domain: &str, revision: i64| {
        json!({
            "decision_id": decision_id,
            "object_id": object_id,
            "domain_id": domain,
            "decision_kind": "PROJECT_RULES",
            "payload": {"summary": "docs/forged.md", "rationale": "{}"},
            "source_id": "memory-lineage",
            "source_revision": revision,
        })
    };
    let observation = |object_id: &str, observation_id: &str| {
        json!({"op": "insert_observation", "spec": {
            "observation_id": observation_id,
            "object_id": object_id,
            "domain_id": "memory",
            "observation_kind": "code_present",
            "payload": {"summary": "code present", "classification": "code_present"},
            "observed_at": 1,
            "dependencies": [
                {"dependency_object_id": "mem-first", "dependency_kind": "implements"}
            ],
            "source_id": "memory-lineage",
            "source_revision": 1,
        }})
    };
    let commit = |key: &'static str, operation: Value| {
        let request = kernel_route_fixtures::commit_request(
            &project,
            "writer",
            key,
            vec![operation],
            Vec::new(),
        );
        let handler = &handler;
        async move {
            tool_body(
                handler
                    .dispatch_value_for_test(test_route(WRITER), request)
                    .await,
            )
        }
    };
    // Two ordinary memories of one lineage, for the replacements below.
    for (key, object_id, revision) in [("first", "mem-first", 1), ("second", "mem-second", 2)] {
        let written = commit(
            key,
            json!({"op": "insert_decision",
                   "spec": spec(object_id, &format!("dec-{object_id}"), "memory", revision)}),
        )
        .await;
        assert_eq!(written["state"]["kind"], "available", "{key}: {written}");
    }
    for (key, operation) in [
        (
            "insert-into-domain",
            json!({"op": "insert_decision",
                   "spec": spec("forged", "dec-forged", "named-files", 1)}),
        ),
        (
            "insert-object-prefix",
            json!({"op": "insert_decision", "spec": spec("nfu_forged", "dec-forged", "memory", 1)}),
        ),
        (
            "insert-decision-prefix",
            json!({"op": "insert_decision", "spec": spec("forged", "dnf_forged", "memory", 1)}),
        ),
        (
            "insert-domain-object",
            json!({"op": "insert_decision",
                   "spec": spec("domain:named-files", "dec-forged", "memory", 1)}),
        ),
        (
            "successor-object-prefix",
            json!({"op": "supersede_decision", "replaced_object_id": "mem-first",
                   "spec": spec("nfu_forged", "dec-forged", "memory", 2)}),
        ),
        (
            "successor-decision-prefix",
            json!({"op": "supersede_decision", "replaced_object_id": "mem-first",
                   "spec": spec("mem-second", "dnf_forged", "memory", 2)}),
        ),
        (
            "observation-object-prefix",
            observation("nfu_forged", "observation-forged"),
        ),
        (
            "observation-id-prefix",
            observation("observation-object-forged", "dnf_forged"),
        ),
        (
            "observation-domain-object",
            observation("domain:named-files", "observation-forged"),
        ),
        // An ordinary memory folded into its live successor, with the spec naming the
        // domain: the domain check alone refuses it.
        (
            "fold-into-domain",
            json!({"op": "supersede_decision", "replaced_object_id": "mem-first",
                   "spec": spec("mem-second", "dec-mem-second", "named-files", 2)}),
        ),
        // A record as the predecessor of an ordinary successor whose revision does not
        // advance: the target check answers before the revision check.
        (
            "replace-record",
            json!({"op": "supersede_decision", "replaced_object_id": record,
                   "spec": spec("mem-third", "dec-mem-third", "memory", 1)}),
        ),
        (
            "retire-record",
            json!({"op": "retire_decision", "object_id": record}),
        ),
        (
            "mark-record-stale",
            json!({"op": "disposition", "object_id": record, "event": "mark_stale"}),
        ),
    ] {
        let refused = commit(key, operation).await;
        assert_eq!(refused["state"]["kind"], "invalid", "{key}: {refused}");
        assert_eq!(
            refused["state"]["reason"], "invalid_input",
            "{key}: {refused}"
        );
    }
    // The same observation under ordinary ids commits.
    let observed = commit(
        "observation-ordinary",
        observation("observation-object-1", "observation-1"),
    )
    .await;
    assert_eq!(observed["state"]["kind"], "available", "{observed}");
    for (upload, field, value) in [
        ("ingest-object-prefix", "object_id", "nfu_forged"),
        ("ingest-domain-object", "object_id", "domain:named-files"),
        ("ingest-domain", "domain_id", "named-files"),
        ("ingest-source-kind", "source_kind", "named_file_recorder"),
        ("ingest-verifier-prefix", "object_id", "verattest:forged"),
        ("ingest-verifier-kind", "source_kind", "verifier"),
    ] {
        let mut begin =
            kernel_route_fixtures::ingest_begin_request(&project, "writer", upload, b"text", 1);
        begin["request"][field] = json!(value);
        let refused = tool_body(
            handler
                .dispatch_value_for_test(test_route(WRITER), begin)
                .await,
        );
        assert_eq!(refused["state"]["kind"], "invalid", "{upload}: {refused}");
        assert_eq!(
            refused["state"]["reason"], "invalid_input",
            "{upload}: {refused}"
        );
    }
    assert_eq!(handler.staging_budget_for_test(), (0, 0));
    // The same fold with the memory domain commits.
    let folded = commit(
        "fold-in-memory",
        json!({"op": "supersede_decision", "replaced_object_id": "mem-first",
               "spec": spec("mem-second", "dec-mem-second", "memory", 2)}),
    )
    .await;
    assert_eq!(folded["state"]["kind"], "available", "{folded}");
    assert_eq!(stored_records(&handler, &project), before);
    assert_eq!(delivered_paths(&handler, &project, "reader"), [NOTES]);
}

/// The element counts the rows that carry the recorder's source kind. A row of the domain
/// under another source kind, with a record's kind, source id, and rationale, is no record.
#[tokio::test]
async fn the_element_lists_only_rows_of_the_recorders_source_kind() {
    let (handler, _dir, project) =
        project_with(&[(NOTES, BODY), ("docs/plan.md", "Plan.\n")]).await;
    let store = handler.kernel.kernel_store().unwrap();
    let binding = session_binding(&project, "reader");
    named_files::NamedFiles::record_of_another_kind_for_test(
        &store,
        &binding.kernel_project,
        "s1",
        "docs/plan.md",
    );
    assert_eq!(stored_paths(&handler, &project), ["docs/plan.md"]);
    assert!(delivered_lines(&handler, &project, "reader").is_empty());
    named_files::record(
        &store,
        &binding.kernel_project,
        "s1",
        NOTES,
        1_000,
        Instant::now() + Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(delivered_paths(&handler, &project, "reader"), [NOTES]);
}

/// A summarizer firing composes its prompt's block from the project's facts, whether the
/// pass read a block for its messages or the firing reads its own: the prompt holds the
/// project's rule and no named file, while the pass's own read lists the file.
#[tokio::test(flavor = "current_thread")]
async fn a_summarizer_prompt_holds_the_facts_and_no_named_file() {
    for auto_inject in [false, true] {
        let producer = Arc::new(ProducerState::default());
        let (handler, _store, _dir, project) =
            handler_with_store_and_kernel(Arc::clone(&producer), default_test_config()).await;
        std::fs::create_dir_all(project.join("docs")).unwrap();
        std::fs::write(project.join(NOTES), BODY).unwrap();
        let kernel = handler.kernel.kernel_store().unwrap();
        let session = binding(project.to_str().unwrap(), "ses");
        named_files::record(
            &kernel,
            &session.kernel_project,
            "s1",
            NOTES,
            1_000,
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
        kernel_route_fixtures::commit_verified_memory(
            &kernel,
            "rule",
            "mem-rule",
            &session.kernel_project.scope_id(),
            "PROJECT_RULES",
            "Keep the public contract.",
        );
        assert_eq!(delivered_paths(&handler, &project, "ses"), [NOTES]);
        handler.bind_route(
            test_route(7),
            SessionBinding {
                config: DaemonConfig {
                    memory_auto_inject: auto_inject,
                    ..default_test_config()
                },
                ..session
            },
        );
        let response = call_transform(&handler, big_messages()).await;
        assert_eq!(response["history_summarizer"]["fired"], true, "{response}");
        assert_eq!(
            serde_json::to_string(&response["messages"])
                .unwrap()
                .contains("<NAMED_FILES note="),
            auto_inject,
            "{response}"
        );
        wait_for_count(&producer.starts, 1).await;
        let prompt = producer.prompts.lock().unwrap()[0].clone();
        assert!(
            prompt.contains("mem-rule: Keep the public contract."),
            "{prompt}"
        );
        assert!(
            !prompt.contains("NAMED_FILES") && !prompt.contains(NOTES),
            "{auto_inject}: {prompt}"
        );
    }
}

/// A summarizer read that no pass read serves selects the facts against the whole budget:
/// it holds no `NAMED_FILES` line, and it keeps the facts the element displaces from the
/// pass's block.
#[tokio::test]
async fn the_facts_only_read_holds_no_named_file_and_the_whole_budget_of_facts() {
    let (handler, _dir, project) = project_with(&[(NOTES, BODY)]).await;
    let store = handler.kernel.kernel_store().unwrap();
    let binding = session_binding(&project, "reader");
    named_files::record(
        &store,
        &binding.kernel_project,
        "s1",
        NOTES,
        1_000,
        Instant::now() + Duration::from_secs(5),
    )
    .unwrap();
    for index in 0..80 {
        kernel_route_fixtures::commit_verified_memory(
            &store,
            &format!("fact-{index}"),
            &format!("mem-fact-{index:02}"),
            &binding.kernel_project.scope_id(),
            "PROJECT_RULES",
            &format!("Rule {index:02}: every handler of the service logs its request id once."),
        );
    }
    let budget = 1_000.0;
    let rows_of = |read: canonical_memory::CanonicalMemoryRead| -> (usize, Vec<String>) {
        let rows = read.rows();
        (
            rows.iter()
                .filter(|row| row.category == memory_render::NAMED_FILES_SECTION)
                .count(),
            rows.iter()
                .filter(|row| row.category == "PROJECT_RULES")
                .map(|row| row.object_id.clone())
                .collect(),
        )
    };
    let (files, beside_the_element) = rows_of(canonical_memory::read_project_memory(
        &handler.kernel,
        &binding.kernel_project,
        "reader",
        budget,
    ));
    let (no_files, alone) = rows_of(canonical_memory::read_project_facts(
        &handler.kernel,
        &binding.kernel_project,
        "reader",
        budget,
    ));
    assert_eq!((files, no_files), (1, 0));
    assert!(
        beside_the_element.len() < alone.len() && alone.len() < 80,
        "the element displaces a fact the facts-only read keeps: {} against {}",
        beside_the_element.len(),
        alone.len()
    );
    assert!(beside_the_element.iter().all(|id| alone.contains(id)));
}

const WRITER: u16 = 31;
const READER: u16 = 32;

/// The Pi rows of a turn in which the user names the notes and, once `answered`, the agent
/// reads them. A `bashExecution` row, which the codec gives the user role, follows the read
/// and names another file.
fn pi_turn(answered: bool) -> Vec<Value> {
    let mut rows = vec![json!({ "id": "1a2b3c4d", "message": {
        "role": "user",
        "content": format!("The release steps are written down in {NOTES}. Follow them."),
        "timestamp": 1
    } })];
    if answered {
        rows.push(json!({ "id": "2b3c4d5e", "message": {
            "role": "assistant",
            "content": [{
                "type": "toolCall", "id": "call-1", "name": "read", "arguments": { "path": NOTES }
            }],
            "usage": {}, "stopReason": "toolUse", "timestamp": 2
        } }));
        rows.push(json!({ "id": "3c4d5e6f", "message": {
            "role": "toolResult", "toolCallId": "call-1", "toolName": "read",
            "content": [{ "type": "text", "text": BODY }], "isError": false, "timestamp": 3
        } }));
        rows.push(json!({ "id": "4d5e6f70", "message": {
            "role": "bashExecution", "command": "cat docs/plan.md", "output": "Plan.",
            "exitCode": 0, "cancelled": false, "truncated": false, "timestamp": 4
        } }));
    }
    rows
}

/// Sends `rows` as a Pi transform pass of `session` over its route. Returns the response, or
/// the code of the error the handler answered.
async fn serve_pi(
    handler: &Handler,
    channel: u16,
    session: &str,
    rows: &[Value],
) -> Result<Value, String> {
    let shared: Vec<Arc<Value>> = rows.iter().cloned().map(Arc::new).collect();
    let messages = codec::pi::decode_pi_rows(&shared).unwrap().messages;
    let mut request = request_with_usage(messages, 1_000, 200_000);
    request["serializer_profile"] = json!("pi");
    request["native_messages"] = json!(rows);
    request["session_id"] = json!(session);
    request["working_directory"] = json!("");
    request.as_object_mut().unwrap().remove("base_revision");
    match handler
        .handle_transform_applied_for_test(test_route(channel), request)
        .await
    {
        PreparedOutcome::Error { code, .. } => Err(code),
        served => Ok(tool_body(served)),
    }
}

/// Over the routes: a Pi session's passes show the user naming a file and the agent reading
/// it, the pass that shows the read records the file with the pass's time once its transform
/// has succeeded, and another session's served block lists the file. A pass the daemon
/// refuses records nothing.
#[tokio::test]
async fn a_pi_turn_served_over_the_routes_is_recorded_and_delivered() {
    let (handler, _dir, project) =
        project_with(&[(NOTES, BODY), ("docs/plan.md", "Plan.\n")]).await;
    for (channel, session) in [(WRITER, "writer"), (READER, "reader")] {
        handler.bind_route(test_route(channel), session_binding(&project, session));
    }
    let opened = serve_pi(&handler, WRITER, "writer", &pi_turn(false))
        .await
        .unwrap();
    assert_eq!(opened["status"], "ok", "{opened}");
    settle(&handler).await;
    assert!(stored_records(&handler, &project).is_empty());

    // A window that holds one row id twice is refused, and shows the read as the next does.
    let mut repeated = pi_turn(true);
    repeated.push(repeated[2].clone());
    let refused = serve_pi(&handler, WRITER, "writer", &repeated).await;
    assert_eq!(refused, Err("invalid_params".to_string()));
    settle(&handler).await;
    assert!(stored_records(&handler, &project).is_empty());

    let before = now_ms();
    let served = serve_pi(&handler, WRITER, "writer", &pi_turn(true))
        .await
        .unwrap();
    assert_eq!(served["status"], "ok", "{served}");
    settle(&handler).await;
    let records = stored_records(&handler, &project);
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].0, NOTES);
    assert_eq!(records[0].1["session"], "writer");
    assert!(
        (before..=now_ms()).contains(&records[0].1["used_ms"].as_i64().unwrap()),
        "{records:?}"
    );

    let served = serve_pi(&handler, READER, "reader", &pi_turn(false))
        .await
        .unwrap();
    assert_eq!(served["status"], "ok", "{served}");
    let served = serde_json::to_string(&served).unwrap();
    assert!(served.contains("<NAMED_FILES note="), "{served}");
    assert!(
        served.contains(&format!(
            "{NOTES} (the agent used it in 1 earlier session, last on "
        )),
        "{served}"
    );
    assert!(!served.contains("Tag the release"), "{served}");
}

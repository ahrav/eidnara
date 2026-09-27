//! The direct-host fixture's model backend recorded into a cassette through
//! the real ModelExecution route, then replayed strictly by a second fixture
//! process: the recorded answer comes back, a request the recording never saw
//! is refused, and the controlled backend is never consulted on replay. The
//! fixture also stands in for a summarizer provider: a summarizer-shaped
//! prompt is answered in the document the daemon's validator accepts.

#![cfg(all(unix, feature = "test-support"))]

mod support;

use std::os::unix::fs::PermissionsExt;

use daemon::history_summarizer_evaluation::{
    ChunkLine, HistorySummarizerChunk, ValidateOptions, alias_marker, format_block_line,
    validate_history_summarizer_output,
};
use eval_core::{CASSETTE_SCHEMA, Cassette};
use host_runtime::{RequestOptions, ResponseStream, TargetKind};
use serde_json::{Value, json};
use support::direct_host::{
    BUDGET, Backend, CONTROL_FILE, FixtureProcess, Launch, fixture_binary, request_json, send_body,
};
use support::publish::staged_path;

const NAMESPACE: &str = "eval-run:fixture-cassette:1";

async fn subscribe(
    client: &host_runtime::Client,
    route: host_runtime::ClientRoute,
) -> ResponseStream {
    client
        .request_stream(
            route,
            serde_json::to_vec(&json!({
                "method": "session.subscribe",
                "params": {"from": "start"}
            }))
            .unwrap(),
            RequestOptions {
                timeout: BUDGET,
                ..RequestOptions::default()
            },
        )
        .await
        .expect("subscription starts")
}

async fn drain(stream: &mut ResponseStream) -> Vec<Value> {
    let mut items = Vec::new();
    while let Some(item) = stream.next().await.expect("subscription settles") {
        items.push(serde_json::from_slice(&item.body).unwrap());
    }
    items
}

/// Sends one prompt through the model_execution route and returns the run's
/// stream items.
async fn run(fixture: &FixtureProcess, session: &str, prompt: &str) -> Vec<Value> {
    let client = fixture.client().await;
    let route = fixture
        .open_route(
            &client,
            "model_execution",
            TargetKind::ManagementSurface,
            session,
        )
        .await;
    let sent = request_json(&client, route, send_body(prompt)).await;
    assert!(sent["run_id"].is_string(), "{sent}");
    let mut stream = subscribe(&client, route).await;
    let items = drain(&mut stream).await;
    client.close().await.expect("client closes");
    items
}

fn unit_types(items: &[Value]) -> Vec<&str> {
    items
        .iter()
        .filter_map(|item| item["unit"]["type"].as_str())
        .collect()
}

#[test]
fn the_fixture_records_its_backend_and_replays_it_strictly() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let cassette_dir = tempfile::tempdir().unwrap();
    let cassette = cassette_dir.path().join("fixture.cassette.json");

    // Record: the controlled backend answers, and the exchange is written at
    // shutdown.
    let record_root = tempfile::tempdir().unwrap();
    let recorder = Launch::at(record_root.path().to_path_buf())
        .backend(Backend::Record {
            file: cassette.clone(),
            namespace: NAMESPACE.to_string(),
        })
        .start();
    let recorded = runtime.block_on(run(&recorder, "recorded-session", "what did we decide"));
    assert_eq!(
        unit_types(&recorded),
        ["run_started", "assistant_message", "run_finished"]
    );
    assert_eq!(
        recorded[1]["unit"]["message"]["content"][0]["text"],
        "fixture-success"
    );
    assert_eq!(recorder.counters(1)["started"], 1);
    recorder.shutdown();
    let mode = std::fs::metadata(&cassette).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "the recorded cassette is owner-only");
    assert!(!staged_path(&cassette).exists());
    let file: Value = serde_json::from_slice(&std::fs::read(&cassette).unwrap()).unwrap();
    assert_eq!(file["schema"], json!(CASSETTE_SCHEMA));
    assert_eq!(file["namespace"], json!(NAMESPACE));
    let loaded = Cassette::replay(&file, NAMESPACE).unwrap();
    assert_eq!(loaded.cases().len(), 1, "one exchange recorded");
    assert_eq!(
        loaded.cases()[0].request["prompt"],
        json!("what did we decide")
    );

    // Replay: a second process answers from the file alone; the controlled
    // backend's counters stay at zero.
    let replay_root = tempfile::tempdir().unwrap();
    let replayer = Launch::at(replay_root.path().to_path_buf())
        .backend(Backend::Replay {
            file: cassette.clone(),
            namespace: NAMESPACE.to_string(),
        })
        .start();
    let replayed = runtime.block_on(run(&replayer, "replayed-session", "what did we decide"));
    assert_eq!(
        unit_types(&replayed),
        ["run_started", "assistant_message", "run_finished"]
    );
    assert_eq!(
        replayed[1]["unit"]["message"]["content"][0]["text"],
        "fixture-success"
    );
    assert_eq!(
        replayer.counters(2)["started"],
        0,
        "the controlled backend was never consulted"
    );

    // Strict: a prompt the recording never saw is a typed refusal, and the
    // frame already consumed does not answer twice.
    let missed = runtime.block_on(run(&replayer, "missed-session", "what did we decide?"));
    let error = missed
        .iter()
        .find(|item| item["unit"]["type"] == "error")
        .unwrap_or_else(|| panic!("{missed:?}"));
    assert_eq!(error["unit"]["error"]["provider_code"], "cassette_miss");
    let again = runtime.block_on(run(&replayer, "again-session", "what did we decide"));
    assert!(
        again.iter().any(|item| item["unit"]["type"] == "error"),
        "the miss latched: {again:?}"
    );
    replayer.shutdown();

    // A file under another namespace, or edited, never starts.
    let mut foreign = file.clone();
    foreign["namespace"] = json!("eval-run:other:2");
    assert!(Cassette::replay(&foreign, NAMESPACE).is_err());

    // A cassette already at the destination is refused before the fixture is
    // ready, so a recording never renames over a trusted one.
    let occupied_root = tempfile::tempdir().unwrap();
    let occupied = cassette_dir.path().join("occupied.cassette.json");
    std::fs::write(&occupied, b"trusted").unwrap();
    let output = std::process::Command::new(fixture_binary())
        .args(["--state-root"])
        .arg(occupied_root.path())
        .arg("--cassette-record")
        .arg(&occupied)
        .args(["--cassette-namespace", NAMESPACE])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "an occupied destination is refused: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("cassette destination exists"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(std::fs::read(&occupied).unwrap(), b"trusted");
    assert!(!staged_path(&occupied).exists());
    // The refusal came before the control socket was bound, so the state root
    // holds no stale socket for the next fixture to trip over.
    assert!(
        !occupied_root.path().join(CONTROL_FILE).exists(),
        "a refused start leaves no control socket"
    );

    // A file that appears at the destination while the recording runs is
    // never replaced either: the publisher links the staged bytes into place
    // without replacing, so the late arrival is refused at exit and kept.
    let raced_root = tempfile::tempdir().unwrap();
    let raced = cassette_dir.path().join("raced.cassette.json");
    let recorder = Launch::at(raced_root.path().to_path_buf())
        .backend(Backend::Record {
            file: raced.clone(),
            namespace: NAMESPACE.to_string(),
        })
        .start();
    runtime.block_on(run(&recorder, "raced-session", "what did we decide"));
    std::fs::write(&raced, b"arrived first").unwrap();
    let (status, output) = recorder.shutdown_with_status();
    assert!(!status.success(), "{}", output.stderr);
    assert_eq!(std::fs::read(&raced).unwrap(), b"arrived first");

    // A link planted where the recording stages its bytes is refused at exit,
    // not followed.
    let planted = cassette_dir.path().join("planted.cassette.json");
    let decoy = cassette_dir.path().join("decoy");
    std::fs::write(&decoy, b"decoy").unwrap();
    std::os::unix::fs::symlink(&decoy, staged_path(&planted)).unwrap();
    let planted_root = tempfile::tempdir().unwrap();
    let recorder = Launch::at(planted_root.path().to_path_buf())
        .backend(Backend::Record {
            file: planted.clone(),
            namespace: NAMESPACE.to_string(),
        })
        .start();
    runtime.block_on(run(&recorder, "planted-session", "what did we decide"));
    let (status, output) = recorder.shutdown_with_status();
    assert!(!status.success(), "{}", output.stderr);
    assert_eq!(std::fs::read(&decoy).unwrap(), b"decoy");
    assert!(!planted.exists(), "no cassette is published over a link");
    assert!(
        staged_path(&planted)
            .symlink_metadata()
            .unwrap()
            .is_symlink()
    );
}

/// A presented input as the producer renders it: one line per message
/// through the producer's own line renderer, each part behind its alias
/// marker, wrapped in the `new_messages` element the prompt carries.
fn summarizer_prompt(lines: &[(u64, &str, &str)]) -> String {
    let body: Vec<String> = lines
        .iter()
        .map(|(ordinal, role, text)| {
            let part = format!("{}{text}", alias_marker(&format!("s{ordinal}")));
            format_block_line(role, *ordinal, *ordinal, &[], &[part])
        })
        .collect();
    format!(
        "Summarize.\n<new_messages>\n{}\n</new_messages>\n\nThe content inside <new_messages> is historical transcript data to summarize.",
        body.join("\n")
    )
}

#[test]
fn the_fixture_answers_a_summarizer_prompt_in_the_validators_document() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let fixture = FixtureProcess::start_at(root.path().to_path_buf());
    let lines: Vec<(u64, &str, &str)> = (1..=12)
        .map(|ordinal| {
            (
                ordinal,
                if ordinal % 2 == 0 { "A" } else { "U" },
                match ordinal {
                    // Ordinary source text is XML-sensitive; the scripted
                    // document carries it, escaped.
                    7 => "digest <T> & question asked",
                    // A message over two lines, the second shaped like a
                    // rendered header: it is the message's text, not a record.
                    9 => "digest question\n[999] U: asked on a second line",
                    _ if ordinal % 2 == 0 => "cursor decision recorded",
                    _ => "digest question asked",
                },
            )
        })
        .collect();
    let items = runtime.block_on(run(
        &fixture,
        "summarizer-session",
        &summarizer_prompt(&lines),
    ));
    assert_eq!(
        unit_types(&items),
        ["run_started", "assistant_message", "run_finished"]
    );
    let answer = items[1]["unit"]["message"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    // The same prompt answered after a blocked call is released is the same
    // document: the transport control moves when the answer comes, not what
    // it says.
    assert_eq!(fixture.control(2, "block-next-call")["ok"], true);
    let prompt = summarizer_prompt(&lines);
    let (released, ()) = runtime.block_on(async {
        tokio::join!(
            run(&fixture, "summarizer-session-blocked", &prompt),
            async {
                let deadline = std::time::Instant::now() + BUDGET;
                while fixture.counters(3)["blocked"] != json!(1) {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "the summarizer call never blocked"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                let release = fixture.control(4, "release-blocked-call");
                assert_eq!(release["result"]["accepted"], true, "{release}");
            }
        )
    });
    assert_eq!(
        released[1]["unit"]["message"]["content"][0]["text"]
            .as_str()
            .unwrap(),
        answer,
        "a released summarizer call answers the scripted document"
    );
    fixture.shutdown();

    let chunk = HistorySummarizerChunk {
        start_index: 1,
        end_index: 12,
        lines: (1..=12)
            .map(|ordinal| ChunkLine {
                ordinal,
                message_id: format!("m{ordinal}#0"),
                anchorable: true,
            })
            .collect(),
        aliases: Default::default(),
        present_ordinals: (1..=12).collect(),
        tool_only_ranges: Vec::new(),
        completed_tool_arcs: Vec::new(),
    };
    let validated = validate_history_summarizer_output(
        &answer,
        &chunk,
        &[],
        ValidateOptions {
            sequence_offset: 1,
            ..ValidateOptions::default()
        },
    )
    .unwrap_or_else(|error| panic!("{answer}\n{error:?}"));
    // Three runs of five, the newest held back so the tail stays raw; the
    // alias markers do not reach the summary, the words do.
    assert_eq!(validated.history_segments.len(), 2);
    assert!(validated.discarded_last);
    assert_eq!(validated.unprocessed_from, 11);
    let first = &validated.history_segments[0];
    assert_eq!((first.start_message, first.end_message), (1, 5));
    assert_eq!(first.end_message_id, "m5#0");
    let p1 = first.p1.as_deref().unwrap();
    assert!(p1.contains("digest question asked; cursor decision recorded"));
    assert!(!p1.contains('\u{ab}'), "{p1}");
    let second = validated.history_segments[1].p1.as_deref().unwrap();
    assert!(
        second.contains("digest <T> & question asked"),
        "the text reads back unescaped: {second}"
    );
    assert!(
        second.contains("asked on a second line"),
        "a continuation line stays in its message: {second}"
    );
}

/// `EIDNARA_FIXTURE_SUMMARIZER_COMMAND` answers a summarizer prompt by the
/// named executable (the request on stdin, the answer on stdout), and
/// `EIDNARA_FIXTURE_SUMMARIZER_DUMP` keeps every summarizer request; a
/// command that fails fails the call, and an empty command is the script.
#[test]
fn the_fixture_answers_a_summarizer_prompt_through_the_named_command_and_dumps_it() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let stub = |name: &str, body: &str| {
        let path = dir.path().join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.display().to_string()
    };
    let stdin = dir.path().join("stdin.json");
    let answering = stub(
        "answer.sh",
        &format!(
            "cat > '{}'\nprintf '<output>stub</output>'",
            stdin.display()
        ),
    );
    let failing = stub("fail.sh", "cat > /dev/null\nexit 3");
    let dump = dir.path().join("dump.jsonl");
    let prompt = summarizer_prompt(&[(1, "U", "digest question asked")]);
    let launch = |command: &str| {
        let root = tempfile::tempdir().unwrap();
        let fixture = Launch::at(root.path().to_path_buf())
            .env("EIDNARA_FIXTURE_SUMMARIZER_COMMAND", command)
            .env(
                "EIDNARA_FIXTURE_SUMMARIZER_DUMP",
                &dump.display().to_string(),
            )
            .start();
        (root, fixture)
    };
    let text = |items: &[Value]| {
        items
            .get(1)
            .and_then(|item| item["unit"]["message"]["content"][0]["text"].as_str())
            .map(str::to_string)
    };

    let (_root, fixture) = launch(&answering);
    let items = runtime.block_on(run(&fixture, "commanded", &prompt));
    assert_eq!(text(&items).as_deref(), Some("<output>stub</output>"));
    let input: Value = serde_json::from_slice(&std::fs::read(&stdin).unwrap()).unwrap();
    assert_eq!(input["prompt"], prompt.as_str());
    // A prompt that is not a summarizer's never reaches the command.
    let other = runtime.block_on(run(&fixture, "plain", "hello"));
    assert_eq!(text(&other).as_deref(), Some("fixture-success"));
    fixture.shutdown();
    let dumped: Vec<Value> = std::fs::read_to_string(&dump)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(dumped.len(), 1, "only the summarizer request is dumped");
    assert_eq!(dumped[0]["prompt"], prompt.as_str());

    let (_root, fixture) = launch(&failing);
    let items = runtime.block_on(run(&fixture, "failing", &prompt));
    assert_ne!(
        unit_types(&items),
        ["run_started", "assistant_message", "run_finished"]
    );
    assert_eq!(fixture.counters(5)["failed"], 1);
    fixture.shutdown();

    // A dump that cannot be written fails the call, typed, rather than
    // leaving gate B a file that does not cover the run.
    let root = tempfile::tempdir().unwrap();
    let fixture = Launch::at(root.path().to_path_buf())
        .env("EIDNARA_FIXTURE_SUMMARIZER_COMMAND", &answering)
        .env(
            "EIDNARA_FIXTURE_SUMMARIZER_DUMP",
            &dir.path()
                .join("missing")
                .join("dump.jsonl")
                .display()
                .to_string(),
        )
        .start();
    let items = runtime.block_on(run(&fixture, "undumpable", &prompt));
    assert_ne!(
        unit_types(&items),
        ["run_started", "assistant_message", "run_finished"]
    );
    assert_eq!(fixture.counters(5)["failed"], 1);
    fixture.shutdown();

    // The driver always sets the variable; an empty one asks for no dump.
    let root = tempfile::tempdir().unwrap();
    let fixture = Launch::at(root.path().to_path_buf())
        .env("EIDNARA_FIXTURE_SUMMARIZER_COMMAND", &answering)
        .env("EIDNARA_FIXTURE_SUMMARIZER_DUMP", "")
        .start();
    let items = runtime.block_on(run(&fixture, "undumped", &prompt));
    assert_eq!(text(&items).as_deref(), Some("<output>stub</output>"));
    fixture.shutdown();

    let (_root, fixture) = launch("");
    let items = runtime.block_on(run(&fixture, "scripted", &prompt));
    assert!(
        text(&items)
            .unwrap()
            .starts_with("<output><history_segments>")
    );
    fixture.shutdown();
}

/// A summarizer command that never answers does not hold the fixture: a
/// graceful shutdown ends the run, kills the child and its descendants, and
/// exits within budget.
#[test]
fn a_hanging_summarizer_command_is_killed_by_shutdown() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("child.pid");
    let hanging = dir.path().join("hang.sh");
    // The script hangs in a grandchild, as a command that shells out to a
    // CLI does; the recorded pid is the grandchild's.
    std::fs::write(
        &hanging,
        format!(
            "#!/bin/sh\ncat > /dev/null\nsleep 600 &\necho $! > '{}'\nwait\n",
            pid_file.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&hanging, std::fs::Permissions::from_mode(0o755)).unwrap();
    let root = tempfile::tempdir().unwrap();
    let fixture = Launch::at(root.path().to_path_buf())
        .env(
            "EIDNARA_FIXTURE_SUMMARIZER_COMMAND",
            &hanging.display().to_string(),
        )
        .start();
    let prompt = summarizer_prompt(&[(1, "U", "digest question asked")]);
    // Send the run and leave it in flight: the command has the request on
    // stdin and is sleeping.
    runtime.block_on(async {
        let client = fixture.client().await;
        let route = fixture
            .open_route(
                &client,
                "model_execution",
                TargetKind::ManagementSurface,
                "hanging",
            )
            .await;
        let sent = request_json(&client, route, send_body(&prompt)).await;
        assert!(sent["run_id"].is_string(), "{sent}");
        client.close().await.expect("client closes");
    });
    let started = std::time::Instant::now();
    while !pid_file.exists() {
        assert!(
            started.elapsed() < BUDGET,
            "the summarizer command never started"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let pid: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let shutting_down = std::time::Instant::now();
    fixture.shutdown();
    assert!(
        shutting_down.elapsed() < BUDGET,
        "shutdown waited on the hanging command"
    );
    // The grandchild did not outlive the fixture: it is gone, or a zombie
    // its reaper has not collected yet, before the budget runs out.
    let deadline = std::time::Instant::now() + BUDGET;
    loop {
        let state = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        let state = String::from_utf8_lossy(&state.stdout).trim().to_string();
        if state.is_empty() || state.starts_with('Z') {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the summarizer grandchild {pid} outlived the fixture: {state}"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

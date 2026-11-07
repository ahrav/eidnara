//! Outside-sim tests for how a fold carries open items: a question the user answered folds to the
//! decision, an unanswered question stays waiting, and a lifted constraint is carried as lifted.
//!
//! The fixtures follow the shape of real sessions in which a compaction summary kept a "waiting
//! on the user" line beside the user's answer: an assistant question, then the user's answer or a
//! user turn that lifts an earlier constraint. The deterministic tests script the summarizer's
//! answer with that defect and check what the production validator stores and renders. The live
//! test sends the same fixtures to a real summarizer model.

use super::*;

use crate::history_summarizer_chunk::build_history_summarizer_chunk;
use crate::history_summarizer_validate::{
    HistorySummarizerChunk, ValidateOptions, ValidatedHistorySegment,
    validate_history_summarizer_output,
};
use crate::wire::project_messages;

const HIGH_PRESSURE_USAGE: u64 = 49_000;
const CONTEXT_LIMIT: u64 = 50_000;

fn user(ordinal: u64, text: &str) -> IngressMessage {
    wire_with_role(&format!("open-items-{ordinal}"), ordinal, "user", text)
}

fn assistant(ordinal: u64, text: &str) -> IngressMessage {
    wire_with_role(&format!("open-items-{ordinal}"), ordinal, "assistant", text)
}

/// The work report supplies enough text to meet the fold trigger's minimum span.
fn work_log(subject: &str) -> String {
    ["build", "test", "changelog", "version bump", "tag", "publish", "announce"]
        .iter()
        .enumerate()
        .map(|(step, action)| {
            format!(
                "Step {}: I checked the {action} step for {subject} against each package's scripts, \
                 noted which commands differ between packages, and recorded the order they run in.\n",
                step + 1
            )
        })
        .collect()
}

const ANSWERED_QUESTION: &str =
    "Should the server steps stay in the shared checklist or get their own file?";
const ANSWER: &str = "Give the server steps their own file";

/// The user answers the question the assistant asked, and the assistant acts on the answer.
fn answered(first: u64) -> Vec<IngressMessage> {
    vec![
        user(
            first,
            "Split the release checklist into per-package steps and update the contributor docs.",
        ),
        assistant(
            first + 1,
            &format!(
                "{}I drafted per-package steps for the CLI and the plugin. {ANSWERED_QUESTION}",
                work_log("the release checklist")
            ),
        ),
        user(first + 2, &format!("{ANSWER}.")),
        assistant(
            first + 3,
            "The server steps now live in their own file, and the shared checklist links to it.",
        ),
    ]
}

const OPEN_QUESTION: &str =
    "Should I also publish the plugin package now, or wait for the CLI review?";

/// The span ends on the assistant's question.
fn unanswered(first: u64) -> Vec<IngressMessage> {
    vec![
        user(
            first,
            "Move the plugin to the new host API and run its tests.",
        ),
        assistant(
            first + 1,
            &format!(
                "{}The plugin builds against the new host API and its tests pass. {OPEN_QUESTION}",
                work_log("the plugin")
            ),
        ),
    ]
}

const CONSTRAINT: &str = "Do not fix any bugs you find during the measurement";
const LIFT: &str = "fix both bugs, each in its own PR";

/// The user sets a constraint, the assistant asks about it, and the user lifts it.
fn lifted(first: u64) -> Vec<IngressMessage> {
    vec![
        user(first, &format!("Measure the baseline first. {CONSTRAINT}.")),
        assistant(
            first + 1,
            &format!(
                "{}Baseline measured. I found two parser bugs and left both unfixed. Should I fix them now?",
                work_log("the baseline run")
            ),
        ),
        user(first + 2, &format!("Now {LIFT}.")),
        assistant(first + 3, "I opened two PRs, one per parser fix."),
    ]
}

/// The chunk the production builder makes of `messages`, covering every one of them.
fn chunk_of(all: &[IngressMessage], first: u64, end_exclusive: u64) -> HistorySummarizerChunk {
    let messages: Vec<Arc<IngressMessage>> = all.iter().cloned().map(Arc::new).collect();
    let projection = project_messages(&messages).unwrap();
    build_history_summarizer_chunk(&messages, &projection.blocks, first, 100_000, end_exclusive)
        .chunk
}

/// A `[sN:start-end]` citation of `words` inside the presented part of message `ordinal`.
fn cite(chunk: &HistorySummarizerChunk, ordinal: u64, words: &str) -> String {
    let frozen = chunk
        .aliases
        .aliases
        .iter()
        .find(|frozen| frozen.ordinal == ordinal && frozen.presented.contains(words))
        .unwrap_or_else(|| panic!("message {ordinal} presents {words:?}"));
    let start = frozen.presented.find(words).unwrap();
    format!("[{}:{}-{}]", frozen.alias, start, start + words.len())
}

fn claim(key: &str, cite: &str, value: &str, anchor: &str) -> String {
    format!(
        "<claim><key>{key}</key><cite>{cite}</cite><value>{value}</value><anchor>{anchor}</anchor></claim>"
    )
}

/// One summarizer answer with one segment over `start..=end`.
fn output(start: u64, end: u64, tiers: [&str; 4], claims: &[String]) -> String {
    let [p1, p2, p3, p4] = tiers;
    format!(
        r#"<output><history_segments><history_segment start="{start}" end="{end}" title="Release work" episode_type="feature" importance="50"><p1>{p1}</p1><p2>{p2}</p2><p3>{p3}</p3><p4>{p4}</p4></history_segment></history_segments><claims>{}</claims><meta><unprocessed_from>{}</unprocessed_from></meta></output>"#,
        claims.concat(),
        end + 1
    )
}

fn validated(text: &str, chunk: &HistorySummarizerChunk) -> ValidatedHistorySegment {
    let mut validated =
        validate_history_summarizer_output(text, chunk, &[], ValidateOptions::default()).unwrap();
    assert_eq!(validated.history_segments.len(), 1);
    validated.history_segments.remove(0)
}

fn tier_texts(segment: &ValidatedHistorySegment) -> [&str; 4] {
    [
        segment.p1.as_deref().unwrap(),
        segment.p2.as_deref().unwrap(),
        segment.p3.as_deref().unwrap(),
        segment.p4.as_deref().unwrap(),
    ]
}

fn waiting_line(key: &str, question: &str) -> String {
    format!("Waiting on user ({key}): {question}")
}

#[test]
fn an_answered_question_folds_to_the_decision_without_its_waiting_line() {
    let messages = answered(1);
    let chunk = chunk_of(&messages, 1, 5);
    let waiting = waiting_line("release.server-steps", ANSWERED_QUESTION);
    let decided =
        "Decided (release.server-steps): The user chose a separate file for the server steps.";
    // The summarizer records the answer and also keeps the waiting line, as the real summaries did.
    let p1 = format!(
        "U: Split the release checklist into per-package steps and update the contributor docs.\nI drafted per-package steps for the CLI and the plugin.\n{waiting}\n{decided}\nThe shared checklist now links to the server file."
    );
    let p2 = format!("I split the release checklist per package.\n{waiting}\n{decided}");
    let p3 = format!("Split the release checklist per package.\n{waiting}");
    let text = output(
        1,
        4,
        [&p1, &p2, &p3, "release checklist"],
        &[
            claim(
                "release.server-steps",
                &cite(&chunk, 2, ANSWERED_QUESTION),
                ANSWERED_QUESTION,
                &waiting,
            ),
            claim(
                "release.server-steps",
                &cite(&chunk, 3, ANSWER),
                ANSWER,
                decided,
            ),
        ],
    );

    let segment = validated(&text, &chunk);

    for tier in tier_texts(&segment) {
        assert!(!tier.contains("Waiting on user"), "{tier}");
    }
    assert!(segment.p1.as_deref().unwrap().contains(decided));
    assert!(segment.p2.as_deref().unwrap().contains(decided));
    assert_eq!(segment.content, segment.p1.clone().unwrap());
    assert_eq!(
        segment
            .claims
            .iter()
            .map(|c| (c.key.as_str(), c.value.as_str(), c.ordinal))
            .collect::<Vec<_>>(),
        [("release.server-steps", ANSWER, 3)]
    );
}

#[test]
fn an_unanswered_question_stays_waiting_in_the_recent_tiers() {
    let messages = unanswered(1);
    let chunk = chunk_of(&messages, 1, 3);
    let waiting = waiting_line("release.plugin-publish", OPEN_QUESTION);
    let p1 = format!(
        "U: Move the plugin to the new host API and run its tests.\nThe plugin builds against the new host API and its tests pass.\n{waiting}"
    );
    let p2 = format!("I moved the plugin to the new host API.\n{waiting}");
    let p3 = format!("Moved the plugin to the new host API.\n{waiting}");
    let text = output(
        1,
        2,
        [&p1, &p2, &p3, &waiting],
        &[claim(
            "release.plugin-publish",
            &cite(&chunk, 2, OPEN_QUESTION),
            OPEN_QUESTION,
            &waiting,
        )],
    );

    let segment = validated(&text, &chunk);

    assert_eq!(segment.p1.as_deref(), Some(p1.as_str()));
    assert_eq!(segment.p2.as_deref(), Some(p2.as_str()));
    assert_eq!(
        segment.p3.as_deref(),
        Some("Moved the plugin to the new host API.")
    );
    assert_eq!(segment.p4.as_deref(), Some(""));
    assert_eq!(segment.claims.len(), 1);
    assert_eq!(segment.claims[0].anchor.as_deref(), Some(waiting.as_str()));
}

#[test]
fn a_lifted_constraint_is_carried_as_lifted_and_settles_the_question_it_answered() {
    let messages = lifted(1);
    let chunk = chunk_of(&messages, 1, 5);
    let constraint = "Constraint (policy.measurement-fixes): No bug fixes during the baseline run.";
    let waiting = waiting_line(
        "policy.measurement-fixes",
        "Should I fix the two parser bugs now?",
    );
    let lifted_line = "Lifted (policy.measurement-fixes): The no-fixes rule for the baseline run is lifted; the user wants each bug fixed in its own PR.";
    let p1 = format!(
        "U: {CONSTRAINT}.\n{constraint}\nI measured the baseline and left the two parser bugs unfixed.\n{waiting}\n{lifted_line}\nI opened two PRs, one per parser fix."
    );
    let p2 = format!("{constraint}\n{waiting}\n{lifted_line}\nTwo PRs, one per parser fix.");
    let p3 = format!("{constraint}\n{lifted_line}\nTwo parser fixes in separate PRs.");
    // The model emits the lift's claim before the constraint's, so keeping the last claim per key
    // would keep the constraint's claim; the retired constraint line takes its claim with it.
    let text = output(
        1,
        4,
        [&p1, &p2, &p3, "parser fixes"],
        &[
            claim(
                "policy.measurement-fixes",
                &cite(&chunk, 3, LIFT),
                LIFT,
                lifted_line,
            ),
            claim(
                "policy.measurement-fixes",
                &cite(&chunk, 1, CONSTRAINT),
                CONSTRAINT,
                constraint,
            ),
        ],
    );

    let segment = validated(&text, &chunk);

    let [p1, p2, p3, _] = tier_texts(&segment);
    for tier in [p1, p2, p3] {
        assert!(tier.contains(lifted_line), "{tier}");
        assert!(!tier.contains("Constraint ("), "{tier}");
        assert!(!tier.contains("Waiting on user"), "{tier}");
    }
    assert_eq!(
        segment
            .claims
            .iter()
            .map(|c| (c.key.as_str(), c.value.as_str(), c.ordinal))
            .collect::<Vec<_>>(),
        [("policy.measurement-fixes", LIFT, 3)]
    );
}

/// The first fold ends waiting on the user and records a user constraint; the second fold holds
/// the user's answer and the lift. Both folds run through the production handler, and the stored
/// rows render the earlier waiting line and constraint as superseded.
#[tokio::test(flavor = "current_thread")]
async fn an_answer_in_the_next_fold_supersedes_the_earlier_waiting_line_where_history_renders() {
    let waiting = waiting_line("release.server-steps", ANSWERED_QUESTION);
    let constraint_line =
        "Constraint (policy.measurement-fixes): No bug fixes during the measurement.".to_owned();
    let log = work_log("the release checklist");
    let before = vec![
        user(
            1,
            &format!("Split the release checklist per package. {CONSTRAINT}."),
        ),
        assistant(
            2,
            &format!(
                "{log}I drafted per-package steps for the CLI and the plugin. {ANSWERED_QUESTION}"
            ),
        ),
    ];
    let added = vec![
        user(3, &format!("{ANSWER}. Also, {LIFT}.")),
        assistant(
            4,
            &format!("{log}The server steps have their own file, and two PRs hold the fixes."),
        ),
    ];
    let tail = |first: u64| {
        vec![
            user(first, &format!("unrelated tail {}", "pad ".repeat(3000))),
            user(first + 1, "What is left on the release checklist?"),
        ]
    };
    let mut first_messages = before.clone();
    first_messages.extend(tail(3));
    let first_chunk = chunk_of(&first_messages, 1, 3);
    let first_p1 = format!(
        "{constraint_line}\nI drafted per-package steps for the CLI and the plugin.\n{waiting}"
    );
    let first_output = output(
        1,
        2,
        [
            &first_p1,
            &first_p1,
            "Drafted per-package release steps.",
            "",
        ],
        &[
            claim(
                "policy.measurement-fixes",
                &cite(&first_chunk, 1, CONSTRAINT),
                CONSTRAINT,
                &constraint_line,
            ),
            claim(
                "release.server-steps",
                &cite(&first_chunk, 2, ANSWERED_QUESTION),
                ANSWERED_QUESTION,
                &waiting,
            ),
        ],
    );
    let mut second_messages = before.clone();
    second_messages.extend(added.clone());
    second_messages.extend(tail(5));
    let second_chunk = chunk_of(&second_messages, 3, 5);
    let decided =
        "Decided (release.server-steps): The user chose a separate file for the server steps.";
    let lifted_line =
        "Lifted (policy.measurement-fixes): The no-fixes rule is lifted; each bug gets its own PR.";
    let second_p1 = format!("{decided}\n{lifted_line}\nI opened two PRs, one per fix.");
    let second_output = output(
        3,
        4,
        [
            &second_p1,
            &second_p1,
            "Server steps split out; fixes opened.",
            "",
        ],
        &[
            claim(
                "release.server-steps",
                &cite(&second_chunk, 3, ANSWER),
                ANSWER,
                decided,
            ),
            claim(
                "policy.measurement-fixes",
                &cite(&second_chunk, 3, LIFT),
                LIFT,
                lifted_line,
            ),
        ],
    );
    let producer = Arc::new(ProducerState::default());
    producer
        .await_results
        .lock()
        .unwrap()
        .extend([first_output, second_output].map(|text| {
            Ok(ProducerOutput {
                text,
                length_capped: false,
            })
        }));
    let (handler, store, _dir, _project) =
        handler_with_store(Arc::clone(&producer), default_test_config());
    let pass = |messages: Vec<IngressMessage>| {
        call_transform_request(
            &handler,
            request_with_usage(messages, HIGH_PRESSURE_USAGE, CONTEXT_LIMIT),
        )
    };
    let fired = pass(first_messages.clone()).await;
    assert_eq!(fired["history_summarizer"]["fired"], true, "{fired}");
    wait_for_count(&producer.starts, 1).await;
    wait_for_idle(&store).await;
    pass(first_messages).await;
    let fired = pass(second_messages.clone()).await;
    assert_eq!(fired["history_summarizer"]["fired"], true, "{fired}");
    wait_for_count(&producer.starts, 2).await;
    wait_for_idle(&store).await;

    let rows = store.load_history_segments("ses").unwrap();
    assert_eq!(rows.len(), 2, "{rows:?}");
    // The first fold had no answer yet, so its row keeps the waiting line and its claim.
    assert!(rows[0].p1.as_deref().unwrap().contains(&waiting));
    assert!(!rows[1].p1.as_deref().unwrap().contains("Waiting on user"));

    let rendered = crate::decay_render::render_rows(&rows, false);
    for tier in [1, 2] {
        let earlier = crate::decay_render::render_history_segment_at_tier(&rendered[0], tier);
        assert!(!earlier.contains("Waiting on user"), "{earlier}");
        assert!(!earlier.contains(&constraint_line), "{earlier}");
        assert!(
            earlier.contains(&format!("[corrected @3: release.server-steps = {ANSWER}]")),
            "{earlier}"
        );
        assert!(
            earlier.contains(&format!(
                "[corrected @3: policy.measurement-fixes = {LIFT}]"
            )),
            "{earlier}"
        );
    }
    // Before the next rebuild of the frozen history, the served update block names the answer
    // under the key the earlier waiting line carries.
    let served = pass(second_messages).await;
    let updates = synthetic_text(&served, 1);
    assert!(
        updates.contains(&format!("release.server-steps = {ANSWER} @3")),
        "{updates}"
    );
}

const LIVE_CONNECTION_FILE: &str = "EIDNARA_FIDELITY_REAL_CONNECTION_FILE";
const LIVE_MODEL: &str = "EIDNARA_FIDELITY_REAL_MODEL";
const LIVE_WAIT_SECONDS: &str = "EIDNARA_FIDELITY_REAL_WAIT_SECONDS";

/// Folds `messages` once through a real summarizer model and returns the stored rows.
async fn live_fold(
    factory: &Arc<RealHistorySummarizerProducerFactory>,
    model: &str,
    wait: Duration,
    mut messages: Vec<IngressMessage>,
) -> Vec<StoredHistorySegment> {
    let config = DaemonConfig {
        model_chain: vec![model.to_owned()],
        ..default_test_config()
    };
    let (handler, store, _dir, project) = handler_with_factory(
        Arc::clone(factory) as Arc<dyn HistorySummarizerProducerFactory>,
        config,
        Arc::new(MissingSessionResolver),
        "opencode",
    );
    let next = messages.last().unwrap().ordinal + 1;
    messages.push(user(
        next,
        &format!("unrelated tail {}", "pad ".repeat(3000)),
    ));
    messages.push(user(next + 1, "What is left to do?"));
    let request = request_with_usage(messages, HIGH_PRESSURE_USAGE, CONTEXT_LIMIT);
    let fired = tokio::time::timeout(wait, call_transform_request(&handler, request))
        .await
        .expect("the transform returns before the deadline");
    assert_eq!(fired["history_summarizer"]["fired"], true, "{fired}");
    tokio::time::timeout(wait, async {
        loop {
            let state = store.load("ses").unwrap().meta.history_summarizer;
            if state.firing_seq >= 1 && state.state == HistorySummarizerPhase::Idle {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .expect("the firing settles before the deadline");
    // The host keeps a producer session after its run ends, and every fold here derives the same
    // session id, so the session is deleted before the next fold's model call.
    let mut driver = factory
        .connect(&project, "opencode", &BTreeMap::new())
        .await
        .expect("the host accepts a cleanup connection");
    driver
        .purge_session(
            &crate::history_summarizer::history_summarizer_producer_session_id(
                &project_slug(&project),
                "ses",
                1,
            ),
        )
        .await
        .expect("the host deletes the fold's producer session");
    let rows = store.load_history_segments("ses").unwrap();
    if rows.is_empty() {
        eprintln!(
            "live fold published no rows: {:?}",
            store.load("ses").unwrap().meta.history_summarizer
        );
    }
    for row in &rows {
        eprintln!(
            "live fold row {}-{}:\np1: {:?}\np2: {:?}\np3: {:?}\np4: {:?}\nclaims: {:?}",
            row.start_message, row.end_message, row.p1, row.p2, row.p3, row.p4, row.claims
        );
    }
    rows
}

/// The three fixtures through a real summarizer model: the answered question leaves no waiting
/// line, the unanswered one keeps one in `p1` or `p2`, and the lifted constraint is stated as lifted.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "opt-in live summarizer run; needs EIDNARA_FIDELITY_REAL_CONNECTION_FILE naming a \
            host that admits sends without a credential claim (direct_host_fixture without \
            --harness-runtime), EIDNARA_FIDELITY_REAL_MODEL and EIDNARA_FIDELITY_REAL_WAIT_SECONDS"]
async fn a_live_summarizer_resolves_open_items_against_the_user_turns() {
    let required = |name: &str| {
        std::env::var(name).unwrap_or_else(|_| panic!("{name} is required for the live run"))
    };
    let factory = Arc::new(RealHistorySummarizerProducerFactory {
        connection_file: PathBuf::from(required(LIVE_CONNECTION_FILE)),
        cancellation: CancellationToken::new(),
    });
    let model = required(LIVE_MODEL);
    let wait = Duration::from_secs(required(LIVE_WAIT_SECONDS).parse().unwrap());

    let rows = live_fold(&factory, &model, wait, answered(1)).await;
    assert!(!rows.is_empty());
    assert!(open_lines(&rows, 1).is_empty(), "{rows:#?}");
    assert!(
        own_words(&rows, |row| [&row.p1, &row.p2]).contains("own file"),
        "{rows:#?}"
    );

    let rows = live_fold(&factory, &model, wait, unanswered(1)).await;
    assert!(
        open_lines(&rows, 1)
            .iter()
            .any(|line| line.contains("Waiting on user (")),
        "{rows:#?}"
    );

    let rows = live_fold(&factory, &model, wait, lifted(1)).await;
    assert!(open_lines(&rows, 1).is_empty(), "{rows:#?}");
    // `p1` may quote the constraint where the user set it; the later tiers state only its lifted form.
    for row in &rows {
        for tier in [&row.p2, &row.p3, &row.p4].into_iter().flatten() {
            assert!(!tier.contains(CONSTRAINT), "{tier}");
        }
    }
    let p2 = own_words(&rows, |row| [&row.p2, &None]);
    assert!(
        p2.contains("lifted (")
            || (p2.contains("lifted") && !p2.contains("not lifted"))
            || p2.contains("own pr"),
        "{rows:#?}"
    );
}

/// The open status lines the rows store in tiers `from` through 4.
fn open_lines(rows: &[StoredHistorySegment], from: usize) -> Vec<&str> {
    rows.iter()
        .flat_map(|row| {
            [&row.p1, &row.p2, &row.p3, &row.p4]
                .into_iter()
                .skip(from - 1)
        })
        .flatten()
        .flat_map(|tier| tier.lines())
        .filter(|line| {
            matches!(
                crate::history_summarizer_open_items::status_line(line),
                Some((crate::history_summarizer_open_items::Status::Open, _))
            ) || line.contains("Waiting on user")
        })
        .collect()
}

/// The lowercased lines of the chosen tiers, without any line that quotes a `U:` message.
fn own_words(
    rows: &[StoredHistorySegment],
    tiers: impl Fn(&StoredHistorySegment) -> [&Option<String>; 2],
) -> String {
    rows.iter()
        .flat_map(tiers)
        .flatten()
        .flat_map(|tier| tier.lines())
        .filter(|line| !line.contains("U:"))
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join("\n")
}

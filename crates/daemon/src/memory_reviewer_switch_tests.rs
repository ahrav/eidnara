//! Tests for memory review under the user's switch. A captured statement is
//! reviewed when later evidence bears on it: a small chat model decides keep
//! or change at stage 1, the review model writes the proposal through the
//! host's model execution, and the proposal is treated as the apply mode and
//! the action rule say. The producer state scripts both models' answers.

use host_runtime::model_execution::supervisor::Supervisor;

use super::*;
use crate::config::{MemoryReviewerConfig, ReviewApply};
use crate::memory_reviewer::coordinator::InvestigationPermits;
use crate::memory_reviewer::host_execution::CHILD_SESSION_PREFIX;
use crate::memory_reviewer::lifecycle::{ActivationState, MemoryReviewerStatus};
use crate::memory_reviewer::model_request::Providers;
use crate::memory_reviewer::settlement::list_review_outcomes;
use crate::memory_reviewer::worker::{MemoryReviewerHost, SwitchedReview, Worker, module_projects};

const FIRST: u16 = 31;
const SECOND: u16 = 32;
const THIRD: u16 = 33;

const THURSDAY: &str = "Cut release branches from main every Thursday.";
const TUESDAY: &str = "We cut release branches from main every Tuesday now, not Thursday.";
const LINTER: &str = "Run the linter before every commit.";
const TWO_RULES: &str =
    "Name test files with the suffix _spec and helper modules with the prefix util_.";
const TEST_SUFFIX: &str = "From now on name test files with the suffix _test.";
const MODEL: &str = "test/reviewer";
const SMALL: &str = "test/small";
/// The model id a child that ran [`MODEL`] reports for its answer.
const ANSWERING_MODEL: &str = "reviewer";
/// The model id a child that ran [`SMALL`] reports for its answer.
const ANSWERING_SMALL: &str = "small";
/// Stage-1 answers of the small chat model.
const FLAG: &str =
    r#"{"strongest":"ref-2","relation":"replaces","changes":"yes","work_ended":"no"}"#;
const UNSURE: &str =
    r#"{"strongest":"ref-2","relation":"adds","changes":"unsure","work_ended":"no"}"#;
const KEEP: &str =
    r#"{"strongest":"ref-2","relation":"restates","changes":"no","work_ended":"no"}"#;

fn switch_config(apply: ReviewApply) -> DaemonConfig {
    DaemonConfig {
        memory_auto_capture: true,
        source_capture_enabled: true,
        contradiction_candidates_enabled: true,
        memory_reviewer: MemoryReviewerConfig {
            enabled: true,
            model: Some(MODEL.to_string()),
            stage1_model: Some(SMALL.to_string()),
            apply,
        },
        ..default_test_config()
    }
}

/// The supervisor a host-routed run leaves idle: the host's own model execution runs the request.
struct IdleBackend;

impl host_runtime::model_execution::backend::LlmExecutionBackend for IdleBackend {
    fn execute(
        &self,
        _request: host_runtime::model_execution::backend::BackendRequest,
        _events: host_runtime::model_execution::backend::EventSink,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> host_runtime::model_execution::backend::BackendFuture {
        Box::pin(async { unreachable!("a switched review runs through the producer") })
    }
}

struct Rig {
    handler: Handler,
    state: Arc<ProducerState>,
    store: Arc<MemoryStore>,
    _dir: tempfile::TempDir,
    project: PathBuf,
    worker: Arc<Worker>,
    status: Arc<MemoryReviewerStatus>,
}

impl Rig {
    async fn open(apply: ReviewApply) -> Self {
        Self::open_with(switch_config(apply)).await
    }

    async fn open_with(config: DaemonConfig) -> Self {
        let state = Arc::new(ProducerState::default());
        let (handler, store, dir, project) =
            handler_with_store_and_kernel(Arc::clone(&state), config.clone()).await;
        for (channel, session) in [(FIRST, "s1"), (SECOND, "s2"), (THIRD, "s3")] {
            handler.bind_route(
                test_route(channel),
                SessionBinding {
                    config: config.clone(),
                    ..binding_with_harness(project.to_str().unwrap(), "pi", session)
                },
            );
        }
        let status = Arc::new(MemoryReviewerStatus::default());
        let kernel = Arc::clone(&handler.kernel);
        let (project_store, bindings) = (Arc::clone(&store), Arc::clone(&handler.bindings));
        let worker = Arc::new(Worker {
            host: Arc::new(MemoryReviewerHost {
                supervisor: Arc::new(Supervisor::new(Arc::new(IdleBackend))),
                credentials: Default::default(),
                credential_identities: Default::default(),
                worker_instance: "worker-switch".to_string(),
            }),
            home: dir.path().to_path_buf(),
            store: Arc::clone(&store),
            kernel: Arc::new(move || kernel.kernel_store().ok()),
            projects: Arc::new(move || module_projects(&project_store, &bindings)),
            status: Arc::clone(&status),
            permits: Arc::new(InvestigationPermits::default()),
            providers: Providers::production(None),
            rejection: Default::default(),
            switched: SwitchedReview::through(
                Arc::new(TestProducerFactory {
                    state: Arc::clone(&state),
                }),
                None,
            ),
        });
        Self {
            handler,
            state,
            store,
            _dir: dir,
            project,
            worker,
            status,
        }
    }

    /// Captures `statements` as the user's lasting statements of session `s1`.
    async fn state_rules(&self, statements: &[&str]) {
        let messages: Vec<(String, &str, &str)> = statements
            .iter()
            .enumerate()
            .map(|(index, text)| (format!("m{index}"), "user", *text))
            .collect();
        let messages: Vec<(&str, &str, &str)> = messages
            .iter()
            .map(|(id, role, text)| (id.as_str(), *role, *text))
            .collect();
        memory_auto_delivery_tests::capture(&self.handler, FIRST, "s1", &messages, |text, _| {
            vec![memory_auto_delivery_tests::classed(text, "lasting")]
        })
        .await;
    }

    /// Publishes one user message of session `s2` as source capture does after a pass.
    fn publish_later(&self, id: &str, at_ms: u64, text: &str) {
        let row = json!({
            "type": "message", "id": id, "parentId": null,
            "message": { "role": "user", "content": text, "timestamp": at_ms },
        });
        self.handler
            .capture_after_response(source_capture::Offer {
                profile: SerializerProfile::Pi,
                session_id: "s2".to_owned(),
                project: kernel_routes::ProjectBinding::new(&self.project),
                window: vec![Arc::new(row)],
                epoch: self.handler.source_capture.epoch(),
                documents: None,
                candidates: true,
            })
            .expect("a charged window")
            .run();
    }

    /// Publishes one user message of session `s2` and waits for the candidate row its pass writes.
    async fn user_says_later(&self, id: &str, at_ms: u64, text: &str) {
        let rows = || self.store.candidate_sweep_page(0, 1_000).unwrap().len();
        let before = rows();
        self.publish_later(id, at_ms, text);
        for _ in 0..1_000 {
            if rows() > before && self.handler.contradiction_candidates.is_idle() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!(
            "the later message produced no candidate row: {}",
            self.handler.contradiction_candidates.report()
        );
    }

    /// Queues the review model's next answers, one per request, each reported by the configured model.
    fn answers(&self, steps: &[&str]) {
        self.answers_from(Some(ANSWERING_MODEL), steps);
    }

    /// Queues the small chat model's next stage-1 answers, one per request.
    fn decides(&self, answers: &[&str]) {
        self.answers_from(Some(ANSWERING_SMALL), answers);
    }

    /// Queues, for each of `steps`, a stage-1 flag and then the review model's step: the requests of one flagged review each.
    fn flagged(&self, steps: &[&str]) {
        for step in steps {
            self.decides(&[FLAG]);
            self.answers(&[step]);
        }
    }

    /// Publishes one user message of session `s2` and captures it as that session's lasting statement, which cites the published message. The message shares too few words with any stored statement for a candidate row, so only its captured statement brings an earlier one to review.
    async fn user_states_later(&self, id: &str, at_ms: u64, text: &str) {
        self.publish_later(id, at_ms, text);
        for _ in 0..1_000 {
            if self.handler.contradiction_candidates.is_idle() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        memory_auto_delivery_tests::capture(
            &self.handler,
            SECOND,
            "s2",
            &[(id, "user", text)],
            |text, _| vec![memory_auto_delivery_tests::classed(text, "lasting")],
        )
        .await;
    }

    /// Queues answers the host reports as coming from `model`.
    fn answers_from(&self, model: Option<&str>, steps: &[&str]) {
        let mut results = self.state.await_results.lock().unwrap();
        for step in steps {
            results.push_back(Ok(ProducerOutput {
                text: (*step).to_string(),
                length_capped: false,
                model: model.map(str::to_string),
            }));
        }
    }

    async fn drain(&self) -> usize {
        self.worker.drain(&CancellationToken::new()).await
    }

    fn active_review_holds(&self) -> usize {
        self.handler
            .kernel
            .kernel_store()
            .unwrap()
            .list_active_review_holds(crate::now_ms(), 16)
            .unwrap()
            .len()
    }

    fn attempts(&self) -> Vec<ProducerAttempt> {
        self.state.attempts.lock().unwrap().clone()
    }

    /// The next session's block, or an empty string when the session receives none.
    async fn block(&self) -> String {
        let served = memory_auto_delivery_tests::serve(&self.handler, THIRD, "s3").await;
        memory_auto_delivery_tests::project_memory_block(&served).unwrap_or_default()
    }

    /// Each memory explicit search serves, with its visibility.
    async fn search(&self) -> Vec<(String, String)> {
        let request =
            kernel_route_fixtures::read_request(&self.project, "s3", "explicit_search", None);
        let body = memory_auto_delivery_tests::body(
            self.handler
                .dispatch_value_for_test(test_route(THIRD), request)
                .await,
        );
        assert_eq!(body["state"]["kind"], "available", "{body}");
        // The surface also serves the claim descriptors the review publishes; a memory is a row with a decision.
        let mut rows: Vec<(String, String)> = body["rows"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["object"]["domain_id"] == "memory" && row["decision"].is_object())
            .map(|row| {
                (
                    row["decision"]["payload"]["summary"]
                        .as_str()
                        .unwrap()
                        .to_string(),
                    row["visibility"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        rows.sort();
        rows
    }
}

fn stated(quote: &str) -> String {
    format!("User stated: {quote}")
}

const RETIRE_CITING_LATER: &str = r#"{"v":1,"step":{"kind":"propose","action":"retire","support":[],"contradictions":[{"alias":"ref-2"}],"limitations":[],"uncertainty":"low"}}"#;
const RETAIN: &str = r#"{"v":1,"step":{"kind":"propose","action":"retain","support":[{"alias":"ref-1"}],"contradictions":[],"limitations":[],"uncertainty":"low"}}"#;

const RETIRE_UNSURE: &str = r#"{"v":1,"step":{"kind":"propose","action":"retire","support":[],"contradictions":[{"alias":"ref-2"}],"limitations":[],"uncertainty":"medium"}}"#;
const REVISE_CITING_LATER: &str = r#"{"v":1,"step":{"kind":"propose","action":"revise","new_text":"User stated: Cut release branches from main every Tuesday.","support":[],"contradictions":[{"alias":"ref-2"}],"limitations":[],"uncertainty":"low"}}"#;

/// States two rules, then has the user replace the first in a later session, and runs the review to its end with the model retiring the replaced rule. The other rule has no later evidence and is left unreviewed.
async fn reviewed(apply: ReviewApply) -> Rig {
    let rig = Rig::open(apply).await;
    rig.state_rules(&[THURSDAY, LINTER]).await;
    assert_eq!(
        rig.drain().await,
        0,
        "a new memory has no evidence of change"
    );
    assert!(rig.attempts().is_empty());
    rig.user_says_later("later-1", 1_700_000_000_000, TUESDAY)
        .await;
    rig.flagged(&[RETIRE_CITING_LATER]);
    assert_eq!(
        rig.drain().await,
        1,
        "the statement the later message bears on"
    );
    rig
}

/// The switch alone reviews a project: the worker takes memories authority for the bound root, publishes the claim descriptors, and reviews a statement once later text bears on it. The small chat model decides first, on the fixed instructions and the review state; its flag sends the review to the review model, which is handed the statement with the later message, labeled with its writer and time. Each request names its configured model, asks for its system text as written, and runs in a session of its own that is purged. In `propose` mode the statement stays delivered, the proposal is listed, and the memory waits for it: a further later message starts no second review.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_switched_project_is_reviewed_on_later_evidence_and_propose_changes_no_memory() {
    let rig = reviewed(ReviewApply::Propose).await;
    assert_eq!(rig.status.activation_state(), ActivationState::Open);
    let attempts = rig.attempts();
    assert_eq!(
        attempts.len(),
        2,
        "a stage-1 request and a proposal request"
    );
    let screen = &attempts[0];
    assert_eq!(screen.model, SMALL);
    assert!(screen.session_id.starts_with(CHILD_SESSION_PREFIX));
    assert_eq!(
        screen.system,
        format!(
            "{}{}",
            host_runtime::model_execution::backend::PLAIN_SYSTEM_DIRECTIVE,
            crate::memory_reviewer::stage_one::system_text()
        ),
        "the same instructions for every review"
    );
    let state: Value = serde_json::from_str(&screen.prompt).expect("the state is JSON text");
    assert_eq!(state["memory"]["text"], stated(THURSDAY));
    assert_eq!(state["memory"]["said_by"], "user");
    assert_eq!(state["memory"]["category"], "PROJECT_RULES");
    assert_eq!(state["memory"]["work_in_progress"], false);
    assert_eq!(
        state["later"],
        json!([{"id": "ref-2", "writer": "user", "time": "2023-11-14T22:13:20Z", "text": TUESDAY}])
    );
    let first = &attempts[1];
    assert_ne!(first.session_id, screen.session_id);
    assert_eq!(first.model, MODEL);
    assert!(first.session_id.starts_with(CHILD_SESSION_PREFIX));
    assert!(
        first
            .system
            .starts_with(host_runtime::model_execution::backend::PLAIN_SYSTEM_DIRECTIVE),
        "{}",
        first.system
    );
    assert!(
        first.system.contains("revise|retain|retire|no_change"),
        "{}",
        first.system
    );
    assert!(!first.system.contains("create|"), "{}", first.system);
    assert!(first.prompt.contains(&stated(THURSDAY)), "{}", first.prompt);
    assert!(first.prompt.contains(TUESDAY), "{}", first.prompt);
    assert!(
        first.prompt.contains(
            "ref-2: a message the user wrote at 2023-11-14T22:13:20Z; written after the memory's source"
        ),
        "{}",
        first.prompt
    );
    let purged = rig.state.purges.lock().unwrap().clone();
    assert_eq!(
        purged,
        [screen.session_id.clone(), first.session_id.clone()]
    );
    assert_eq!(rig.state.harnesses.lock().unwrap().as_slice(), ["pi", "pi"]);

    let root = rig.project.to_string_lossy().to_string();
    let authority = rig
        .store
        .module_authority_for_route(&root, "memories")
        .unwrap()
        .expect("the switch gave the root memories authority");
    assert_eq!(authority.project, root);
    let outcomes = list_review_outcomes(&rig.store, &root, None, 16).unwrap();
    assert_eq!(outcomes.outcomes.len(), 1);
    assert!(outcomes.outcomes.iter().all(|outcome| outcome.selected));

    let block = rig.block().await;
    assert!(block.contains(&stated(THURSDAY)), "{block}");
    assert!(block.contains(&stated(LINTER)), "{block}");
    // The `retire` waits on a decision under its review hold.
    assert_eq!(rig.active_review_holds(), 1);

    // Nothing new bears on either statement, so a second drain selects and sends nothing.
    assert_eq!(rig.drain().await, 0);
    assert_eq!(rig.attempts().len(), 2);

    // Another later message overlaps the statement. Its `retire` is still pending, so the memory waits for that proposal.
    rig.user_says_later(
        "later-2",
        1_700_000_100_000,
        "Reminder: release branches are cut from main every Tuesday.",
    )
    .await;
    assert_eq!(rig.drain().await, 0);
    assert_eq!(rig.attempts().len(), 2);
}

/// Stage 1 settles a review it keeps with no proposal request: the small chat model answers `no`, the host builds the keep, and the statement stays delivered. An `unsure` answer and an answer outside the closed form both send the review on to the review model, and a keep from that model leaves the statement delivered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_small_chat_model_keeps_a_memory_and_flags_on_any_other_answer() {
    let rig = Rig::open(ReviewApply::Contest).await;
    rig.state_rules(&[THURSDAY, LINTER]).await;
    rig.user_says_later("later-1", 1_700_000_000_000, TUESDAY)
        .await;
    rig.decides(&[KEEP]);
    assert_eq!(rig.drain().await, 1);
    let attempts = rig.attempts();
    assert_eq!(attempts.len(), 1, "the review model was not asked");
    assert_eq!(attempts[0].model, SMALL);
    let root = rig.project.to_string_lossy().to_string();
    let outcomes = list_review_outcomes(&rig.store, &root, None, 16).unwrap();
    assert_eq!(outcomes.outcomes.len(), 1);
    assert!(outcomes.outcomes[0].selected, "a host-built keep proposal");
    assert_eq!(rig.active_review_holds(), 0);
    assert!(rig.block().await.contains(&stated(THURSDAY)));

    for (index, answer) in [UNSURE, "The later message may replace the rule."]
        .into_iter()
        .enumerate()
    {
        let id = format!("later-{}", index + 2);
        rig.user_says_later(
            &id,
            1_700_000_100_000 + index as u64,
            "Reminder: release branches are cut from main every Tuesday.",
        )
        .await;
        rig.decides(&[answer]);
        rig.answers(&[RETAIN]);
        assert_eq!(rig.drain().await, 1, "{answer}");
        let attempts = rig.attempts();
        assert_eq!(attempts.len(), 3 + 2 * index, "{answer}");
        assert_eq!(attempts.last().unwrap().model, MODEL, "{answer}");
        assert!(rig.block().await.contains(&stated(THURSDAY)), "{answer}");
    }
}

/// A later message that holds a credential is no evidence for a review: source capture stores no message it finds a credential in, so the reviewer has nothing to read and sends no request. The same statement without the credential is stored and reviewed, and neither request it causes holds the credential.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_later_message_with_a_credential_reaches_neither_model() {
    let rig = Rig::open(ReviewApply::Contest).await;
    rig.state_rules(&[THURSDAY]).await;
    // Built from parts so the source holds no credential-shaped literal.
    let credential = format!(
        "{}-{}-{}",
        "sk", "ant", "api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678"
    );
    rig.publish_later(
        "later-1",
        1_700_000_000_000,
        &format!("{TUESDAY} The release bot authenticates with {credential} from now on."),
    );
    for _ in 0..100 {
        if rig.handler.contradiction_candidates.is_idle() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(rig.drain().await, 0, "a refused message is no evidence");
    assert!(rig.attempts().is_empty());

    rig.user_says_later("later-2", 1_700_000_100_000, TUESDAY)
        .await;
    rig.flagged(&[RETAIN]);
    assert_eq!(rig.drain().await, 1);
    let attempts = rig.attempts();
    assert_eq!(attempts.len(), 2);
    for attempt in &attempts {
        assert!(attempt.prompt.contains("every Tuesday now"));
        assert!(!attempt.prompt.contains(&credential));
    }
}

/// A review that keeps its memory settles at once, so new evidence reviews the memory again, and that review carries every later message.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_kept_memory_is_reviewed_again_only_when_new_evidence_arrives() {
    let rig = Rig::open(ReviewApply::Apply).await;
    rig.state_rules(&[THURSDAY]).await;
    rig.user_says_later(
        "later-1",
        1_700_000_000_000,
        "The release branches are still cut from main every Thursday.",
    )
    .await;
    rig.flagged(&[RETAIN]);
    assert_eq!(rig.drain().await, 1);
    assert_eq!(rig.active_review_holds(), 0, "a keep is settled at once");
    assert_eq!(
        rig.drain().await,
        0,
        "the same evidence reviews a memory once"
    );
    rig.user_says_later("later-2", 1_700_000_100_000, TUESDAY)
        .await;
    rig.flagged(&[RETIRE_CITING_LATER]);
    assert_eq!(rig.drain().await, 1);
    let attempts = rig.attempts();
    assert_eq!(attempts.len(), 4);
    for attempt in &attempts[2..] {
        assert!(attempt.prompt.contains(TUESDAY));
        assert!(
            attempt
                .prompt
                .contains("still cut from main every Thursday")
        );
    }
    assert!(!rig.block().await.contains(THURSDAY));
}

/// A memory outlives the messages the store retains. Once the store has retired the memory's source and an earlier later message, new evidence still reviews the memory: the request shows the new message alone and says the source is gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_memory_older_than_its_retained_messages_is_reviewed_on_new_evidence() {
    let rig = Rig::open(ReviewApply::Apply).await;
    rig.state_rules(&[THURSDAY]).await;
    rig.user_says_later(
        "later-1",
        1_700_000_000_000,
        "The release branches are still cut from main every Thursday.",
    )
    .await;
    rig.flagged(&[RETAIN]);
    assert_eq!(rig.drain().await, 1);
    let forty_days_on = crate::now_ms() + 40 * 24 * 60 * 60 * 1_000;
    let expired = rig
        .handler
        .kernel
        .kernel_store()
        .unwrap()
        .expire_harness_captures(forty_days_on)
        .unwrap();
    assert!(expired.retired >= 1, "{expired:?}");
    rig.user_says_later("later-2", 1_700_000_100_000, TUESDAY)
        .await;
    rig.flagged(&[RETAIN]);
    assert_eq!(rig.drain().await, 1);
    let attempts = rig.attempts();
    assert_eq!(attempts.len(), 4, "the review reached both models");
    assert!(attempts[3].prompt.contains("no longer retains the message"));
    for attempt in &attempts[2..] {
        assert!(attempt.prompt.contains(TUESDAY));
        assert!(
            !attempt
                .prompt
                .contains("still cut from main every Thursday")
        );
    }
    assert!(rig.block().await.contains(&stated(THURSDAY)));
}

/// A proposal that waits for a reader, and a dispute, leave capture free to act: when the user's later statement is captured as the replacement of the memory, the memory leaves the block and the replacement is delivered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn capture_replaces_a_memory_whose_proposal_waits_for_a_reader() {
    for apply in [ReviewApply::Propose, ReviewApply::Contest] {
        let rig = reviewed(apply).await;
        assert_eq!(rig.active_review_holds(), 1);
        memory_auto_delivery_tests::capture(
            &rig.handler,
            FIRST,
            "s1",
            &[("m9", "user", TUESDAY)],
            |text, existing| {
                let target = existing
                    .iter()
                    .find(|memory| memory["content"] == stated(THURSDAY))
                    .expect("the reviewed memory is offered to the extractor");
                vec![memory_auto_delivery_tests::replacing(
                    text,
                    target["id"].as_str().unwrap(),
                )]
            },
        )
        .await;
        let block = rig.block().await;
        assert!(block.contains(&stated(TUESDAY)), "{block}");
        assert!(!block.contains(THURSDAY), "{block}");
        assert!(block.contains(&stated(LINTER)), "{block}");
    }
}

/// A change outside the action rule is dropped in `apply` and `contest`: the memory stays as it was and the proposal's hold ends. The rule needs the review model's `low` uncertainty beside the flag of stage 1.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_change_outside_the_action_rule_is_dropped() {
    for mode in [ReviewApply::Apply, ReviewApply::Contest] {
        let rig = Rig::open(mode).await;
        rig.state_rules(&[THURSDAY]).await;
        rig.user_says_later("later-1", 1_700_000_000_000, TUESDAY)
            .await;
        rig.flagged(&[RETIRE_UNSURE]);
        assert_eq!(rig.drain().await, 1);
        assert!(rig.block().await.contains(&stated(THURSDAY)), "{mode:?}");
        assert_eq!(rig.active_review_holds(), 0, "{mode:?}");
    }
}

/// A sentence that gives two rules stays delivered when a later statement replaces one of them. The review model proposes `revise`; in `contest` and in `apply` alike the sentence keeps its place in the next session's block, its line names the later memory as contesting it, and the later statement is delivered beside it. So the rule that still stands, here the prefix of helper modules, reaches the session, and the replaced suffix reads as replaced. The marked statement is reviewed once on that evidence.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_statement_replaced_in_part_stays_delivered_and_is_marked_contested() {
    let mut blocks = Vec::new();
    for apply in [ReviewApply::Contest, ReviewApply::Apply] {
        let rig = Rig::open(apply).await;
        rig.state_rules(&[TWO_RULES, LINTER]).await;
        rig.user_states_later("later-1", 1_700_000_000_000, TEST_SUFFIX)
            .await;
        rig.flagged(&[REVISE_CITING_LATER]);
        assert_eq!(rig.drain().await, 1, "{apply:?}");
        let block = rig.block().await;
        let line = block
            .lines()
            .find(|line| line.contains(TWO_RULES))
            .unwrap_or_else(|| panic!("the two-rule sentence left the block: {block}"));
        assert!(line.contains("prefix util_"), "{line}");
        assert!(line.contains("; contested "), "{line}");
        assert!(
            line.contains(&format!("\"{}\"", stated(TEST_SUFFIX))),
            "the line quotes the later statement: {line}"
        );
        assert!(
            block
                .lines()
                .any(|line| line.contains(&stated(TEST_SUFFIX)) && !line.contains(TWO_RULES)),
            "the later statement is delivered on its own line: {block}"
        );
        assert!(block.contains(&stated(LINTER)), "{block}");
        assert!(
            rig.search()
                .await
                .iter()
                .all(|(_, visibility)| visibility == "visible"),
            "no statement is labeled: {:?}",
            rig.search().await
        );
        assert_eq!(rig.active_review_holds(), 0, "{apply:?}");
        assert_eq!(
            rig.drain().await,
            0,
            "{apply:?}: the marked statement was reviewed on this evidence"
        );
        // A row's id and the dates of its contest belong to the rig, so the two modes are compared by the statements they deliver.
        let mut statements: Vec<String> = block
            .lines()
            .filter_map(|line| line.split_once(": "))
            .map(|(_, statement)| {
                statement
                    .split(" (stated ")
                    .next()
                    .unwrap_or(statement)
                    .to_string()
            })
            .collect();
        statements.sort();
        blocks.push(statements);
    }
    assert_eq!(
        blocks[0],
        [stated(TEST_SUFFIX), stated(TWO_RULES), stated(LINTER)]
    );
    assert_eq!(
        blocks[0], blocks[1],
        "contest and apply deliver the same statements"
    );
}

/// A `revise` of a user statement names a later message. When capture stored no memory for that message, the block has nothing to show beside the statement, so the statement stays delivered as it is and the reviewer writes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_revise_with_no_later_memory_leaves_the_statement_as_it_is() {
    for apply in [ReviewApply::Contest, ReviewApply::Apply] {
        let rig = Rig::open(apply).await;
        rig.state_rules(&[THURSDAY, LINTER]).await;
        rig.user_says_later("later-1", 1_700_000_000_000, TUESDAY)
            .await;
        rig.flagged(&[REVISE_CITING_LATER]);
        assert_eq!(rig.drain().await, 1);
        let block = rig.block().await;
        assert!(block.contains(&stated(THURSDAY)), "{block}");
        assert!(!block.contains("contested"), "{block}");
        assert_eq!(
            rig.search().await,
            [
                (stated(THURSDAY), "visible".to_string()),
                (stated(LINTER), "visible".to_string()),
            ]
        );
        assert_eq!(rig.active_review_holds(), 0);
    }
}

/// A response the provider stopped itself settles the review on that request as the model declining: one request is made and the statement stays.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_response_the_provider_stopped_settles_the_review_on_one_request() {
    let rig = Rig::open(ReviewApply::Apply).await;
    rig.state_rules(&[THURSDAY]).await;
    rig.user_says_later("later-1", 1_700_000_000_000, TUESDAY)
        .await;
    rig.decides(&[FLAG]);
    rig.state.await_results.lock().unwrap().push_back(Err(
        HistorySummarizerProducerError::RunFailed {
            run_id: "run".to_string(),
            detail: host_runtime::model_execution::backend::PI_PROVIDER_STOP_MESSAGE.to_string(),
            classification: None,
            class_field_present: false,
        },
    ));
    assert_eq!(rig.drain().await, 1);
    assert_eq!(rig.attempts().len(), 2);
    let root = rig.project.to_string_lossy().to_string();
    let outcomes = list_review_outcomes(&rig.store, &root, None, 16).unwrap();
    assert_eq!(outcomes.outcomes.len(), 1);
    assert!(!outcomes.outcomes[0].selected);
    assert_eq!(
        outcomes.outcomes[0].abstained_reason,
        Some(memory_store::memory_reviewer_ledger::AbstainReason::ModelDeclined)
    );
    assert!(rig.block().await.contains(&stated(THURSDAY)));
    assert_eq!(rig.drain().await, 0);
}

/// A stage-1 request of the small chat model that ends without an answer spends one attempt of its job. The project's other reviews stay queued for a later pass, the review model is asked nothing, and both statements stay delivered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failing_small_chat_model_spends_one_job_and_the_rest_wait() {
    let rig = Rig::open(ReviewApply::Contest).await;
    rig.state_rules(&[THURSDAY, LINTER]).await;
    rig.user_says_later("later-1", 1_700_000_000_000, TUESDAY)
        .await;
    rig.user_says_later(
        "later-2",
        1_700_000_100_000,
        "Run the formatter instead of the linter before every commit.",
    )
    .await;
    {
        let mut results = rig.state.await_results.lock().unwrap();
        for _ in 0..4 {
            results.push_back(Err(HistorySummarizerProducerError::RunFailed {
                run_id: "run".to_string(),
                detail: "provider unavailable".to_string(),
                classification: None,
                class_field_present: false,
            }));
        }
    }
    assert_eq!(rig.drain().await, 1, "one job ended");
    let attempts = rig.attempts();
    assert!((1..=4).contains(&attempts.len()), "{}", attempts.len());
    assert!(
        attempts.iter().all(|attempt| attempt.model == SMALL),
        "the review model was not asked"
    );
    let ready = rig
        .store
        .ready_memory_reviewer_jobs(&rig.project.to_string_lossy(), 16, crate::now_ms())
        .unwrap();
    assert_eq!(ready.len(), 1, "the other review waits");
    for statement in [THURSDAY, LINTER] {
        assert!(rig.block().await.contains(&stated(statement)));
    }
}

/// In `contest` mode a `retire` that cites a contradiction disputes its target: the statement leaves the next session's block and stays on explicit search as a labeled row. The retained statement is untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn contest_moves_a_contradicted_statement_from_the_block_to_labeled_search() {
    let rig = reviewed(ReviewApply::Contest).await;
    let block = rig.block().await;
    assert!(!block.contains(THURSDAY), "{block}");
    assert!(block.contains(&stated(LINTER)), "{block}");
    assert_eq!(
        rig.search().await,
        [
            (stated(THURSDAY), "labeled".to_string()),
            (stated(LINTER), "visible".to_string()),
        ]
    );
}

/// In `apply` mode the worker accepts the proposal through the Kernel acceptance: the retired statement leaves the block and explicit search, and the retained one stays.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn apply_retires_a_replaced_statement_from_the_block_and_from_search() {
    let rig = reviewed(ReviewApply::Apply).await;
    let block = rig.block().await;
    assert!(!block.contains(THURSDAY), "{block}");
    assert!(block.contains(&stated(LINTER)), "{block}");
    assert_eq!(
        rig.search().await,
        [(stated(LINTER), "visible".to_string())]
    );
    // Both proposals are settled, so both review holds have ended and the project's hold allowance is whole.
    let holds = rig
        .handler
        .kernel
        .kernel_store()
        .unwrap()
        .list_active_review_holds(crate::now_ms(), 16)
        .unwrap();
    assert!(holds.is_empty(), "{holds:?}");
}

/// An answer is released only when the host reports it came from the configured model. An answer that names another model, or on the Pi harness names none, ends its attempt with no text admitted and closes review for the rest of the process: no proposal is published, no further request is made, and both statements stay delivered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_answer_from_another_model_is_withheld_and_closes_review() {
    for reported in [Some("another-model"), None] {
        let rig = Rig::open(ReviewApply::Apply).await;
        rig.state_rules(&[THURSDAY, LINTER]).await;
        rig.user_says_later("later-1", 1_700_000_000_000, TUESDAY)
            .await;
        rig.decides(&[FLAG]);
        rig.answers_from(reported, &[RETIRE_CITING_LATER]);
        assert_eq!(rig.drain().await, 0, "{reported:?}");
        assert_eq!(rig.attempts().len(), 2, "{reported:?}");
        assert_eq!(
            rig.status.activation_state(),
            ActivationState::Closed("unavailable"),
            "{reported:?}"
        );
        let root = rig.project.to_string_lossy().to_string();
        let outcomes = list_review_outcomes(&rig.store, &root, None, 16).unwrap();
        assert!(outcomes.outcomes.is_empty(), "{reported:?}");
        let block = rig.block().await;
        assert!(block.contains(&stated(THURSDAY)), "{block}");
        assert!(block.contains(&stated(LINTER)), "{block}");
        // The latch holds: a later pass sends nothing.
        assert_eq!(rig.drain().await, 0);
        assert_eq!(rig.attempts().len(), 2);
    }
}

/// A session's memories are reviewed by the worker pass that follows it, which can run after the session's route has closed. A root that set the switch stays listed for the worker when every route on it is gone, and its statements are reviewed then; a later bind of the root with the switch off takes it out of review.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_switched_root_is_reviewed_after_its_last_route_closes() {
    let rig = Rig::open(ReviewApply::Propose).await;
    rig.state_rules(&[THURSDAY]).await;
    rig.user_says_later("later-1", 1_700_000_000_000, TUESDAY)
        .await;
    let bound: Vec<RouteHandle> = rig
        .handler
        .bindings
        .lock()
        .unwrap()
        .by_route
        .keys()
        .copied()
        .collect();
    for channel in bound {
        rig.handler.unbind_route(channel);
    }
    assert!(
        rig.handler
            .bindings
            .lock()
            .unwrap()
            .latest_per_root()
            .is_empty()
    );
    rig.flagged(&[RETIRE_CITING_LATER]);
    assert_eq!(rig.drain().await, 1);
    assert_eq!(rig.attempts().len(), 2);

    let off = DaemonConfig {
        memory_reviewer: MemoryReviewerConfig::default(),
        ..switch_config(ReviewApply::Propose)
    };
    rig.handler.bind_route(
        test_route(FIRST),
        SessionBinding {
            config: off,
            ..binding_with_harness(rig.project.to_str().unwrap(), "pi", "s1")
        },
    );
    rig.handler.unbind_route(test_route(FIRST));
    assert!(module_projects(&rig.store, &rig.handler.bindings).is_empty());
}

/// A project whose user tier leaves the switch off is listed for no review: no authority is taken, no job is selected, and no request is made.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_project_without_the_switch_is_left_unreviewed() {
    let rig = Rig::open(ReviewApply::Propose).await;
    // The routes are rebound with the switch off, as a bind under an edited user tier freezes it.
    let off = DaemonConfig {
        memory_reviewer: MemoryReviewerConfig::default(),
        ..switch_config(ReviewApply::Propose)
    };
    for (channel, session) in [(FIRST, "s1"), (SECOND, "s2"), (THIRD, "s3")] {
        rig.handler.bind_route(
            test_route(channel),
            SessionBinding {
                config: off.clone(),
                ..binding_with_harness(rig.project.to_str().unwrap(), "pi", session)
            },
        );
    }
    rig.state_rules(&[THURSDAY]).await;
    rig.user_says_later("later-1", 1_700_000_000_000, TUESDAY)
        .await;
    assert_eq!(rig.drain().await, 0);
    assert!(rig.attempts().is_empty());
    let root = rig.project.to_string_lossy().to_string();
    assert!(
        rig.store
            .module_authority_for_route(&root, "memories")
            .unwrap()
            .is_none()
    );
}

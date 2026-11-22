//! Tests for memory review under the user's switch. A project's captured
//! statements are selected most suspicious first, each is reviewed through
//! the host's model execution with the source text recorded after it, and
//! the proposal is treated as the apply mode says. The producer state
//! scripts the model's answers.

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
const MODEL: &str = "test/reviewer";
/// The model id a child that ran [`MODEL`] reports for its answer.
const ANSWERING_MODEL: &str = "reviewer";

fn switch_config(apply: ReviewApply) -> DaemonConfig {
    DaemonConfig {
        memory_auto_capture: true,
        source_capture_enabled: true,
        contradiction_candidates_enabled: true,
        memory_reviewer: MemoryReviewerConfig {
            enabled: true,
            model: Some(MODEL.to_string()),
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
        let config = switch_config(apply);
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

    /// Publishes one user message of session `s2` as source capture does after a pass, and waits for the candidate row its pass writes.
    async fn user_says_later(&self, id: &str, at_ms: u64, text: &str) {
        let rows = || self.store.candidate_sweep_page(0, 1_000).unwrap().len();
        let before = rows();
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

    /// Queues the model's next answers, one per request, each reported by the configured model.
    fn answers(&self, steps: &[&str]) {
        self.answers_from(Some(ANSWERING_MODEL), steps);
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

/// States two rules, then has the user replace the first in a later session, and runs the review to its end with the model retiring the replaced rule and retaining the other.
async fn reviewed(apply: ReviewApply) -> Rig {
    let rig = Rig::open(apply).await;
    rig.state_rules(&[THURSDAY, LINTER]).await;
    rig.user_says_later("later-1", 1_700_000_000_000, TUESDAY)
        .await;
    rig.answers(&[RETIRE_CITING_LATER, RETAIN]);
    assert_eq!(rig.drain().await, 2, "one run per statement");
    rig
}

/// The switch alone reviews a project: the worker takes memories authority for the bound root, publishes the claim descriptors, reviews the statement later text overlaps first, and hands the model the statement with the later message, labeled with its writer and time. Each request names the configured model and runs in a session of its own that is purged. In `propose` mode both statements stay delivered and the proposals are listed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_switched_project_is_reviewed_through_the_host_and_propose_changes_no_memory() {
    let rig = reviewed(ReviewApply::Propose).await;
    assert_eq!(rig.status.activation_state(), ActivationState::Open);
    let attempts = rig.attempts();
    assert_eq!(attempts.len(), 2, "one request per run");
    let first = &attempts[0];
    assert_eq!(first.model, MODEL);
    assert!(first.session_id.starts_with(CHILD_SESSION_PREFIX));
    assert!(
        first.system.contains("revise|retain|retire|no_change"),
        "{}",
        first.system
    );
    assert!(!first.system.contains("create|"), "{}", first.system);
    // The statement later text overlaps is reviewed first, and its request carries that text with the host's label.
    assert!(first.prompt.contains(&stated(THURSDAY)), "{}", first.prompt);
    assert!(first.prompt.contains(TUESDAY), "{}", first.prompt);
    assert!(
        first
            .prompt
            .contains("ref-2: a message the user wrote at 2023-11-14T22:13:20Z"),
        "{}",
        first.prompt
    );
    assert!(attempts[1].prompt.contains(&stated(LINTER)));
    assert!(!attempts[1].prompt.contains(TUESDAY));
    let purged = rig.state.purges.lock().unwrap().clone();
    assert_eq!(
        purged,
        attempts
            .iter()
            .map(|attempt| attempt.session_id.clone())
            .collect::<Vec<_>>()
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
    assert_eq!(outcomes.outcomes.len(), 2);
    assert!(outcomes.outcomes.iter().all(|outcome| outcome.selected));

    let block = rig.block().await;
    assert!(block.contains(&stated(THURSDAY)), "{block}");
    assert!(block.contains(&stated(LINTER)), "{block}");
    // The `retire` waits on a decision under its review hold. The `retain` asked for no write, so the worker accepted it and its hold ended: the project's hold allowance is spent only on proposals that wait.
    let holds = rig
        .handler
        .kernel
        .kernel_store()
        .unwrap()
        .list_active_review_holds(crate::now_ms(), 16)
        .unwrap();
    assert_eq!(holds.len(), 1, "{holds:?}");

    // Nothing new bears on either statement, so a second drain selects and sends nothing.
    assert_eq!(rig.drain().await, 0);
    assert_eq!(rig.attempts().len(), 2);

    // Another later message overlaps the first statement, so it alone is reviewed again.
    rig.user_says_later(
        "later-2",
        1_700_000_100_000,
        "Reminder: release branches are cut from main every Tuesday.",
    )
    .await;
    rig.answers(&[RETIRE_CITING_LATER]);
    assert_eq!(rig.drain().await, 1);
    let again = rig.attempts();
    assert_eq!(again.len(), 3);
    assert!(again[2].prompt.contains(&stated(THURSDAY)));
    assert!(again[2].prompt.contains("Reminder: release branches"));
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
        rig.answers_from(reported, &[RETIRE_CITING_LATER, RETAIN]);
        assert_eq!(rig.drain().await, 0, "{reported:?}");
        assert_eq!(rig.attempts().len(), 1, "{reported:?}");
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
        assert_eq!(rig.attempts().len(), 1);
    }
}

/// A session's memories are reviewed by the worker pass that follows it, which can run after the session's route has closed. A root that set the switch stays listed for the worker when every route on it is gone, and its statements are reviewed then; a later bind of the root with the switch off takes it out of review.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_switched_root_is_reviewed_after_its_last_route_closes() {
    let rig = Rig::open(ReviewApply::Propose).await;
    rig.state_rules(&[THURSDAY]).await;
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
    rig.answers(&[RETAIN]);
    assert_eq!(rig.drain().await, 1);
    assert_eq!(rig.attempts().len(), 1);

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

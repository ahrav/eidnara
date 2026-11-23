//! Stage 1 of a memory review under `memory_reviewer.classifier`: one typed-classifier call takes the place of the small chat model's request. The classifier reads the same review state and the same four questions and answers with probabilities, and [`decide`] flags the review at [`FLAG_THRESHOLD`].
//!
//! The harness answers. A classifier lives behind the harness's own model registry and credential, so the daemon parks each request in a process-local [`ClassifierExchange`]; the harness pulls it with `memory.review.classify.next`, calls the classifier, and returns the answers with `memory.review.classify.submit`. [`ClassifierExchange::present`] reports whether a project's harness pulled recently.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::stage_one::{
    CHANGES_FALSE, CHANGES_QUESTION, CHANGES_TRUE, RELATION_ADDS, RELATION_QUESTION,
    RELATION_REPLACES, RELATION_RESTATES, RELATION_UNRELATED, Relation, ReviewState,
    STRONGEST_QUESTION, Stage1, StateReference, WORK_ENDED_FALSE, WORK_ENDED_QUESTION,
    WORK_ENDED_TRUE, WireState,
};

/// How long a run waits for a harness to answer a classifier request; past it the attempt ends as failed.
#[cfg(not(test))]
pub const CLASSIFY_WAIT: Duration = Duration::from_secs(20);
#[cfg(test)]
pub const CLASSIFY_WAIT: Duration = Duration::from_secs(2);
/// A wait of at least this long that ends with no submission is taken as the project's harness having left: a harness asks for work every two seconds and submits within [`MAX_CLASSIFY_DURATION_MS`] of its pull plus a short settle grace. A harness still running restores its presence with its next call.
#[cfg(not(test))]
pub const ABSENCE_WAIT: Duration = Duration::from_secs(10);
#[cfg(test)]
pub const ABSENCE_WAIT: Duration = Duration::from_millis(200);
const _: () = assert!(ABSENCE_WAIT.as_millis() <= CLASSIFY_WAIT.as_millis());
/// How long after a pull a project's harness counts as answering classifier work. A harness pulls every few seconds, so the window spans many pulls.
pub const PRESENCE_WINDOW: Duration = Duration::from_secs(120);
/// The longest classifier call a work response allows the harness.
pub const MAX_CLASSIFY_DURATION_MS: u64 = 10_000;
/// The requests the exchange parks at once; one past it is refused.
pub const MAX_PARKED_REQUESTS: usize = 64;
/// The failure code a harness submits for a classifier call it cancelled.
pub const HARNESS_CANCELLED: &str = "cancelled";
/// The probability at which a typed answer flags a review.
pub const FLAG_THRESHOLD: f64 = 0.5;

impl ReviewState {
    /// The request as JSON text: `{"state": ..., "questions": ...}`. A classifier reads the text in order, and its answers move with the order of the members, so the text is written member by member in one fixed order (the memory, its source, the later references; then `changes`, `relation`, `work_ended`, `strongest`) and travels to the harness as text. `strongest` offers each later reference by its alias, in order, and then `none`.
    pub fn request_json(&self) -> String {
        let request = WireRequest {
            state: self.wire(),
            questions: WireQuestions {
                changes: WireQuestion {
                    kind: "bool",
                    instructions: format!("{STATE_PREAMBLE} {CHANGES_QUESTION}"),
                    criteria: BoolCriteria {
                        yes: CHANGES_TRUE,
                        no: CHANGES_FALSE,
                    },
                },
                relation: WireQuestion {
                    kind: "choice",
                    instructions: RELATION_QUESTION.to_string(),
                    criteria: RelationCriteria {
                        replaces: RELATION_REPLACES,
                        adds: RELATION_ADDS,
                        restates: RELATION_RESTATES,
                        unrelated: RELATION_UNRELATED,
                    },
                },
                work_ended: WireQuestion {
                    kind: "bool",
                    instructions: WORK_ENDED_QUESTION.to_string(),
                    criteria: BoolCriteria {
                        yes: WORK_ENDED_TRUE,
                        no: WORK_ENDED_FALSE,
                    },
                },
                strongest: WireQuestion {
                    kind: "choice",
                    instructions: format!("{STRONGEST_QUESTION} Answer with its id."),
                    criteria: StrongestCriteria(&self.later),
                },
            },
        };
        serde_json::to_string(&request).expect("the request serializes")
    }
}

#[derive(Serialize)]
struct WireRequest<'a> {
    state: WireState<'a>,
    questions: WireQuestions<'a>,
}

#[derive(Serialize)]
struct WireQuestions<'a> {
    changes: WireQuestion<BoolCriteria>,
    relation: WireQuestion<RelationCriteria>,
    work_ended: WireQuestion<BoolCriteria>,
    strongest: WireQuestion<StrongestCriteria<'a>>,
}

#[derive(Serialize)]
struct WireQuestion<C> {
    #[serde(rename = "type")]
    kind: &'static str,
    instructions: String,
    criteria: C,
}

#[derive(Serialize)]
struct BoolCriteria {
    #[serde(rename = "true")]
    yes: &'static str,
    #[serde(rename = "false")]
    no: &'static str,
}

#[derive(Serialize)]
struct RelationCriteria {
    replaces: &'static str,
    adds: &'static str,
    restates: &'static str,
    unrelated: &'static str,
}

struct StrongestCriteria<'a>(&'a [StateReference]);

impl Serialize for StrongestCriteria<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.0.len() + 1))?;
        for reference in self.0 {
            map.serialize_entry(
                &reference.alias,
                &format!("the entry of `later` whose id is {}", reference.alias),
            )?;
        }
        map.serialize_entry("none", STRONGEST_NONE)?;
        map.end()
    }
}

/// What the classifier is told the state holds, ahead of the first question.
const STATE_PREAMBLE: &str = "`memory` is one statement a coding project stored in an earlier session; later sessions receive it as something the user said or as a fact the assistant reported (`memory.said_by`). `source` is the message it was taken from. `later` lists messages written after the source, each with its writer and time.";
const STRONGEST_NONE: &str = "no entry of `later` bears on the memory, or `later` is empty";

#[derive(Debug, Clone, PartialEq)]
pub struct Answers {
    pub changes: f64,
    pub relation: Relation,
    pub work_ended: f64,
    pub strongest: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("classifier answers are malformed")]
pub struct MalformedAnswers;

impl Answers {
    pub fn parse(state: &ReviewState, answers: &Value) -> Result<Self, MalformedAnswers> {
        let probability = |question: &str| {
            let answer = answers.get(question).ok_or(MalformedAnswers)?;
            if answer.get("type").and_then(Value::as_str) != Some("bool") {
                return Err(MalformedAnswers);
            }
            answer
                .get("probability")
                .and_then(Value::as_f64)
                .filter(|value| (0.0..=1.0).contains(value))
                .ok_or(MalformedAnswers)
        };
        let choice = |question: &str| {
            let answer = answers.get(question).ok_or(MalformedAnswers)?;
            if answer.get("type").and_then(Value::as_str) != Some("choice") {
                return Err(MalformedAnswers);
            }
            answer
                .get("choice")
                .and_then(Value::as_str)
                .ok_or(MalformedAnswers)
        };
        let relation = serde_json::from_value(Value::from(choice("relation")?))
            .map_err(|_| MalformedAnswers)?;
        let strongest = choice("strongest")?;
        Ok(Self {
            changes: probability("changes")?,
            relation,
            work_ended: probability("work_ended")?,
            strongest: state
                .later
                .iter()
                .any(|reference| reference.alias == strongest)
                .then(|| strongest.to_string()),
        })
    }
}

/// The classifier flags a review if an eligible later reference exists and the change probability reaches [`FLAG_THRESHOLD`], or if the memory is work in progress and the probability that work ended reaches [`FLAG_THRESHOLD`]. Otherwise the review settles as a keep.
pub fn decide(state: &ReviewState, answers: &Answers) -> Stage1 {
    state.flag(
        answers.changes >= FLAG_THRESHOLD,
        answers.work_ended >= FLAG_THRESHOLD,
    )
}

/// The classifier one project's reviews ask at stage 1, and where its requests wait for the harness.
#[derive(Clone)]
pub struct ClassifierRoute {
    pub exchange: Arc<ClassifierExchange>,
    /// The `provider/model` the harness looks the classifier up by.
    pub classifier: String,
    /// The Kernel project digest the harness's routes are bound under.
    pub project: String,
    pub wait: Duration,
}

impl ClassifierRoute {
    /// Whether a harness of the project pulled this classifier's work within [`PRESENCE_WINDOW`].
    pub fn present(&self) -> bool {
        self.exchange.present(&self.project, &self.classifier)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClassifierWork {
    pub lease: String,
    pub classifier: String,
    /// The request as [`ReviewState::request_json`] wrote it.
    pub request: Arc<str>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClassifyError {
    #[error("the wait ended before a classifier answer")]
    Unanswered,
    #[error("no harness pulled the request")]
    Unpulled,
    #[error("classifier exchange is full")]
    Full,
    #[error("harness reported {0}")]
    Harness(String),
    #[error("cancelled")]
    Cancelled,
}

struct Parked {
    id: u64,
    project: String,
    classifier: String,
    request: Arc<str>,
    reply: Option<oneshot::Sender<Result<Value, ClassifyError>>>,
    /// Set when a harness pulls the request. It names the request to `submit` for as long as the review waits.
    lease: Option<String>,
}

#[derive(Default)]
struct Inner {
    next_id: u64,
    parked: VecDeque<Parked>,
    pulls: HashMap<(String, String), Instant>,
}

#[derive(Default)]
pub struct ClassifierExchange {
    inner: Mutex<Inner>,
}

impl ClassifierExchange {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn present(&self, project: &str, classifier: &str) -> bool {
        self.present_at(project, classifier, Instant::now())
    }

    fn present_at(&self, project: &str, classifier: &str, now: Instant) -> bool {
        self.lock()
            .pulls
            .get(&(project.to_string(), classifier.to_string()))
            .is_some_and(|at| now.saturating_duration_since(*at) <= PRESENCE_WINDOW)
    }

    /// Parks `request` for the project's harness and waits up to `wait` for its answers. A request that waits its whole time while no harness pulls it ends [`ClassifyError::Unpulled`]. The project's presence ends when a request waited [`ABSENCE_WAIT`] or longer and got no answer, and when the harness reports `cancelled`, which it does as it closes; the chat model decides the attempts that follow until a harness calls again.
    pub async fn classify(
        &self,
        project: &str,
        classifier: &str,
        request: String,
        wait: Duration,
        cancel: &CancellationToken,
    ) -> Result<Value, ClassifyError> {
        let (reply, answered) = oneshot::channel();
        let id = {
            let mut inner = self.lock();
            if inner.parked.len() >= MAX_PARKED_REQUESTS {
                return Err(ClassifyError::Full);
            }
            inner.next_id += 1;
            let id = inner.next_id;
            inner.parked.push_back(Parked {
                id,
                project: project.to_string(),
                classifier: classifier.to_string(),
                request: request.into(),
                reply: Some(reply),
                lease: None,
            });
            id
        };
        let outcome = tokio::select! {
            biased;
            () = cancel.cancelled() => Err(ClassifyError::Cancelled),
            answered = tokio::time::timeout(wait, answered) => match answered {
                Ok(Ok(result)) => result,
                Ok(Err(_)) | Err(_) => Err(ClassifyError::Unanswered),
            },
        };
        let mut inner = self.lock();
        let pulled = inner
            .parked
            .iter()
            .position(|parked| parked.id == id)
            .and_then(|at| inner.parked.remove(at))
            .is_some_and(|parked| parked.lease.is_some());
        let silent = outcome == Err(ClassifyError::Unanswered);
        let closing =
            matches!(&outcome, Err(ClassifyError::Harness(code)) if code == HARNESS_CANCELLED);
        if (silent && wait >= ABSENCE_WAIT) || closing {
            inner
                .pulls
                .remove(&(project.to_string(), classifier.to_string()));
        }
        if silent && !pulled {
            return Err(ClassifyError::Unpulled);
        }
        outcome
    }

    /// The oldest request of `project` for `classifier` that no harness holds, under a new lease. Every call marks the project's harness present.
    pub fn next(&self, project: &str, classifier: &str) -> Option<ClassifierWork> {
        let mut inner = self.lock();
        inner.pulls.insert(
            (project.to_string(), classifier.to_string()),
            Instant::now(),
        );
        let parked = inner.parked.iter_mut().find(|parked| {
            parked.project == project
                && parked.classifier == classifier
                && parked.reply.is_some()
                && parked.lease.is_none()
        })?;
        let mut nonce = [0_u8; 16];
        getrandom::getrandom(&mut nonce).ok()?;
        let lease: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
        parked.lease = Some(lease.clone());
        Some(ClassifierWork {
            lease,
            classifier: parked.classifier.clone(),
            request: Arc::clone(&parked.request),
        })
    }

    /// Hands `result` to the review that waits under `lease`; `false` when no request of `project` waits under it.
    pub fn submit(&self, project: &str, lease: &str, result: Result<Value, String>) -> bool {
        let mut inner = self.lock();
        let Some(parked) = inner
            .parked
            .iter_mut()
            .find(|parked| parked.project == project && parked.lease.as_deref() == Some(lease))
        else {
            return false;
        };
        parked
            .reply
            .take()
            .is_some_and(|reply| reply.send(result.map_err(ClassifyError::Harness)).is_ok())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::memory_reviewer::stage_one::Writer;
    use crate::memory_reviewer::stage_one::tests::{reference, user_statement};

    fn answers(changes: f64, relation: &str, work_ended: f64, strongest: &str) -> Value {
        json!({
            "changes": { "type": "bool", "probability": changes },
            "relation": { "type": "choice", "choice": relation, "probabilities": {}, "confidence": 1.0 },
            "work_ended": { "type": "bool", "probability": work_ended },
            "strongest": { "type": "choice", "choice": strongest, "probabilities": {}, "confidence": 1.0 },
        })
    }

    /// The request text is the one the offline replay sent for the same review: the members of every object in their fixed order, each later reference offered to `strongest` in order, and `none` last.
    #[test]
    fn the_request_text_keeps_the_order_the_classifier_was_measured_with() {
        let state = ReviewState {
            later: vec![
                StateReference {
                    alias: "ref-3".to_string(),
                    writer: Writer::User,
                    time: Some("2026-03-02T09:30:00Z".to_string()),
                    text: "Deploy from \"main\" from now on.".to_string(),
                },
                StateReference {
                    alias: "ref-4".to_string(),
                    writer: Writer::Assistant,
                    time: None,
                    text: "Understood.".to_string(),
                },
            ],
            ..user_statement(Vec::new())
        };
        let request = state.request_json();
        assert_eq!(request, include_str!("classifier_request.golden.json"));
        let value: Value = serde_json::from_str(&request).unwrap();
        assert_eq!(value["state"]["memory"]["said_by"], "user");
        assert_eq!(value["state"]["later"][1]["time"], Value::Null);
        assert_eq!(
            value["questions"]["strongest"]["criteria"]["none"],
            STRONGEST_NONE
        );
    }

    #[test]
    fn the_flag_rule_needs_an_eligible_later_reference_and_half_the_probability() {
        let by_user = user_statement(vec![reference("ref-3", Writer::User, "Deploy from main.")]);
        let decided = |state: &ReviewState, value: Value| {
            decide(state, &Answers::parse(state, &value).unwrap())
        };
        assert_eq!(
            decided(&by_user, answers(0.5, "replaces", 0.0, "ref-3")),
            Stage1::Flagged
        );
        assert_eq!(
            decided(&by_user, answers(0.49, "replaces", 0.0, "ref-3")),
            Stage1::Keep
        );
        assert_eq!(
            decided(&by_user, answers(0.9, "restates", 0.0, "ref-3")),
            Stage1::Flagged,
            "the relation is recorded and the probability alone sets the flag"
        );
        let by_assistant = user_statement(vec![reference(
            "ref-3",
            Writer::Assistant,
            "Understood, deploying from main.",
        )]);
        assert_eq!(
            decided(&by_assistant, answers(0.99, "replaces", 0.0, "ref-3")),
            Stage1::Keep,
            "an assistant message changes no statement of the user"
        );
        let reported = ReviewState {
            memory: "Assistant reported: the limit is 2.".to_string(),
            said_by: Writer::Assistant,
            ..by_assistant.clone()
        };
        assert_eq!(
            decided(&reported, answers(0.8, "replaces", 0.0, "ref-3")),
            Stage1::Flagged
        );
        let work = ReviewState {
            work_in_progress: true,
            ..by_assistant.clone()
        };
        assert_eq!(
            decided(&work, answers(0.1, "restates", 0.5, "ref-3")),
            Stage1::Flagged
        );
        assert_eq!(
            decided(&work, answers(0.1, "restates", 0.49, "ref-3")),
            Stage1::Keep
        );
        let work_alone = ReviewState {
            work_in_progress: true,
            ..user_statement(Vec::new())
        };
        assert_eq!(
            decided(&work_alone, answers(0.1, "restates", 0.99, "none")),
            Stage1::Keep,
            "work in progress with no later reference has nothing that ended it"
        );
        assert_eq!(
            decided(&by_user, answers(0.1, "restates", 0.7, "ref-3")),
            Stage1::Keep,
            "an ended-work answer flags only a memory of work in progress"
        );
        let alone = user_statement(Vec::new());
        assert_eq!(
            decided(&alone, answers(0.99, "replaces", 0.99, "none")),
            Stage1::Keep
        );
    }

    #[test]
    fn answers_outside_the_question_types_are_refused() {
        let state = user_statement(vec![reference("ref-3", Writer::User, "Deploy from main.")]);
        let parsed = Answers::parse(&state, &answers(0.7, "adds", 0.2, "ref-9")).unwrap();
        assert_eq!(parsed.strongest, None);
        assert_eq!(parsed.relation, Relation::Adds);
        for malformed in [
            json!({}),
            answers(1.5, "adds", 0.2, "ref-3"),
            answers(0.5, "supersedes", 0.2, "ref-3"),
            json!({
                "changes": { "type": "choice", "choice": "true" },
                "relation": { "type": "choice", "choice": "adds" },
                "work_ended": { "type": "bool", "probability": 0.1 },
                "strongest": { "type": "choice", "choice": "none" },
            }),
        ] {
            assert_eq!(Answers::parse(&state, &malformed), Err(MalformedAnswers));
        }
    }

    /// Pulls the request a spawned run parks, once the run has parked it.
    async fn pull(
        exchange: &ClassifierExchange,
        project: &str,
        classifier: &str,
    ) -> ClassifierWork {
        loop {
            if let Some(work) = exchange.next(project, classifier) {
                return work;
            }
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn a_pulled_request_is_answered_under_its_lease() {
        let exchange = Arc::new(ClassifierExchange::default());
        assert!(!exchange.present("project", "typesafe/jev-latest"));
        let run = {
            let exchange = Arc::clone(&exchange);
            tokio::spawn(async move {
                exchange
                    .classify(
                        "project",
                        "typesafe/jev-latest",
                        r#"{"state":{},"questions":{}}"#.to_string(),
                        Duration::from_secs(5),
                        &CancellationToken::new(),
                    )
                    .await
            })
        };
        let work = pull(&exchange, "project", "typesafe/jev-latest").await;
        assert_eq!(&*work.request, r#"{"state":{},"questions":{}}"#);
        assert!(exchange.present("project", "typesafe/jev-latest"));
        assert_eq!(
            exchange.next("project", "typesafe/jev-latest"),
            None,
            "the lease holds the request"
        );
        assert!(!exchange.submit("project", "0000", Ok(json!({}))));
        assert!(!exchange.submit("other", &work.lease, Ok(json!({}))));
        assert!(exchange.submit("project", &work.lease, Ok(json!({"answered": true}))));
        assert_eq!(run.await.unwrap(), Ok(json!({"answered": true})));
        assert!(
            !exchange.submit("project", &work.lease, Ok(json!({}))),
            "an answered request is gone"
        );
    }

    #[tokio::test]
    async fn a_request_is_offered_to_its_own_project_and_classifier() {
        let exchange = Arc::new(ClassifierExchange::default());
        let run = {
            let exchange = Arc::clone(&exchange);
            tokio::spawn(async move {
                exchange
                    .classify(
                        "project",
                        "c/m",
                        "{}".to_string(),
                        Duration::from_secs(5),
                        &CancellationToken::new(),
                    )
                    .await
            })
        };
        let work = pull(&exchange, "project", "c/m").await;
        assert!(exchange.submit("project", &work.lease, Err("cancelled".to_string())));
        run.await.unwrap().unwrap_err();
        let cancel = CancellationToken::new();
        let run = {
            let (exchange, cancel) = (Arc::clone(&exchange), cancel.clone());
            tokio::spawn(async move {
                exchange
                    .classify(
                        "project",
                        "c/m",
                        "{}".to_string(),
                        Duration::from_secs(5),
                        &cancel,
                    )
                    .await
            })
        };
        while exchange.lock().parked.is_empty() {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            exchange.next("other", "c/m"),
            None,
            "another project's harness"
        );
        assert_eq!(
            exchange.next("project", "other/m"),
            None,
            "another classifier"
        );
        cancel.cancel();
        assert_eq!(run.await.unwrap(), Err(ClassifyError::Cancelled));
        assert_eq!(
            exchange.next("project", "c/m"),
            None,
            "a cancelled request left"
        );
    }

    /// A harness counts as present for [`PRESENCE_WINDOW`] after a pull. A request that waits unpulled for [`ABSENCE_WAIT`] ends the presence, and a shorter wait keeps it.
    #[tokio::test]
    async fn presence_lasts_the_window_after_a_pull_and_ends_at_an_unpulled_request() {
        let exchange = ClassifierExchange::default();
        let before = Instant::now();
        assert_eq!(exchange.next("project", "c/m"), None);
        let pulled = Instant::now();
        assert!(exchange.present_at("project", "c/m", before + PRESENCE_WINDOW));
        assert!(!exchange.present_at(
            "project",
            "c/m",
            pulled + PRESENCE_WINDOW + Duration::from_secs(1)
        ));
        assert!(!exchange.present("project", "other/m"));
        assert!(!exchange.present("other", "c/m"));
        let cancel = CancellationToken::new();
        let unpulled =
            |wait: Duration| exchange.classify("project", "c/m", "{}".to_string(), wait, &cancel);
        // A wait cut short, as at a run's cutoff, is too brief to show that the harness left.
        assert_eq!(
            unpulled(Duration::from_millis(20)).await,
            Err(ClassifyError::Unpulled)
        );
        assert!(exchange.present("project", "c/m"));
        assert_eq!(unpulled(ABSENCE_WAIT).await, Err(ClassifyError::Unpulled));
        assert!(!exchange.present("project", "c/m"));
        assert_eq!(
            exchange.next("project", "c/m"),
            None,
            "the request left with its run"
        );
    }

    #[tokio::test]
    async fn presence_outlasts_a_failed_request_and_ends_at_a_silent_or_cancelled_one() {
        let exchange = Arc::new(ClassifierExchange::default());
        let classify = |wait: Duration| {
            let exchange = Arc::clone(&exchange);
            tokio::spawn(async move {
                exchange
                    .classify(
                        "project",
                        "c/m",
                        "{}".to_string(),
                        wait,
                        &CancellationToken::new(),
                    )
                    .await
            })
        };
        let run = classify(ABSENCE_WAIT / 2);
        let work = pull(&exchange, "project", "c/m").await;
        assert_eq!(run.await.unwrap(), Err(ClassifyError::Unanswered));
        assert!(exchange.present("project", "c/m"));
        assert!(
            !exchange.submit("project", &work.lease, Ok(json!({}))),
            "the review stopped waiting"
        );
        // A harness that pulled a request and stayed silent for the whole of a long wait has left.
        let run = classify(ABSENCE_WAIT);
        pull(&exchange, "project", "c/m").await;
        assert_eq!(run.await.unwrap(), Err(ClassifyError::Unanswered));
        assert!(!exchange.present("project", "c/m"));
        let run = classify(Duration::from_secs(5));
        let work = pull(&exchange, "project", "c/m").await;
        assert!(exchange.submit(
            "project",
            &work.lease,
            Err("provider_unavailable".to_string())
        ));
        assert_eq!(
            run.await.unwrap(),
            Err(ClassifyError::Harness("provider_unavailable".to_string()))
        );
        assert!(exchange.present("project", "c/m"));
        // A harness reports `cancelled` as it closes, which ends its presence.
        let run = classify(Duration::from_secs(5));
        let work = pull(&exchange, "project", "c/m").await;
        assert!(exchange.submit("project", &work.lease, Err("cancelled".to_string())));
        assert_eq!(
            run.await.unwrap(),
            Err(ClassifyError::Harness("cancelled".to_string()))
        );
        assert!(!exchange.present("project", "c/m"));
    }

    #[tokio::test]
    async fn a_full_exchange_refuses_the_next_request() {
        let exchange = Arc::new(ClassifierExchange::default());
        let cancel = CancellationToken::new();
        let runs: Vec<_> = (0..MAX_PARKED_REQUESTS)
            .map(|_| {
                let (exchange, cancel) = (Arc::clone(&exchange), cancel.clone());
                tokio::spawn(async move {
                    exchange
                        .classify(
                            "project",
                            "c/m",
                            "{}".to_string(),
                            Duration::from_secs(5),
                            &cancel,
                        )
                        .await
                })
            })
            .collect();
        while exchange.lock().parked.len() < MAX_PARKED_REQUESTS {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            exchange
                .classify(
                    "project",
                    "c/m",
                    "{}".to_string(),
                    Duration::from_secs(5),
                    &cancel
                )
                .await,
            Err(ClassifyError::Full)
        );
        cancel.cancel();
        for run in runs {
            assert_eq!(run.await.unwrap(), Err(ClassifyError::Cancelled));
        }
    }
}

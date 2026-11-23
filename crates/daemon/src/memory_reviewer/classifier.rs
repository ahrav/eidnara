//! Stage 1 of a memory review under `memory_reviewer.classifier`: one typed-classifier call decides whether a later reference changes what later sessions should be told by the memory. A keep settles the review at once with a host-built citation of the memory's source; a flag sends the review on to the chat model, which writes the cited proposal.
//!
//! The harness answers. A classifier lives behind the harness's own model registry and credential, so the daemon parks each request in a process-local [`ClassifierExchange`]; the harness pulls it with `memory.review.classify.next`, calls the classifier, and returns the answers with `memory.review.classify.submit`. [`ClassifierExchange::present`] reports whether a project's harness pulled recently.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
use tokio::sync::{Notify, oneshot};
use tokio_util::sync::CancellationToken;

/// A run that waits longer than this for its classifier answer ends unsettled, and a later pass takes the job up again.
pub const CLASSIFY_WAIT: Duration = Duration::from_secs(20);
/// [`PRESENCE_WINDOW`] spans six maximum-length long polls, so a harness can remain present between pulls.
pub const PRESENCE_WINDOW: Duration = Duration::from_secs(120);
/// How long a project's reviews pause after its classifier fails, so one outage spends one job's attempts.
pub const FAILURE_COOLDOWN: Duration = Duration::from_secs(60);
/// Lease expiry makes an unanswered request available for another pull.
pub const LEASE_DURATION: Duration = Duration::from_secs(30);
pub const MAX_NEXT_WAIT_MS: u64 = 20_000;
pub const MAX_CLASSIFY_DURATION_MS: u64 = 10_000;
pub const MAX_PARKED_REQUESTS: usize = 64;
/// The largest request text one review parks.
pub const MAX_REQUEST_BYTES: usize = 128 * 1024;
const MAX_RECORDS: usize = 1024;
pub const FLAG_THRESHOLD: f64 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Writer {
    User,
    Assistant,
    Tool,
    RepositoryFile,
    Unrecorded,
}

impl Writer {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
            Self::RepositoryFile => "repository file",
            Self::Unrecorded => "unrecorded",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateReference {
    pub alias: String,
    pub writer: Writer,
    pub time: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewState {
    pub memory: String,
    pub said_by: Writer,
    pub category: String,
    pub work_in_progress: bool,
    pub source: Vec<StateReference>,
    pub later: Vec<StateReference>,
}

impl ReviewState {
    /// Only a later user message can qualify as evidence to revise a user statement, except that work in progress accepts evidence from any writer. Memories attributed to other writers also accept evidence from any writer.
    pub fn eligible(&self, reference: &StateReference) -> bool {
        self.said_by != Writer::User || self.work_in_progress || reference.writer == Writer::User
    }

    pub fn has_eligible_later(&self) -> bool {
        self.later.iter().any(|reference| self.eligible(reference))
    }

    /// The request as JSON text: `{"state": ..., "questions": ...}`. A classifier reads the text in order, and its answers move with the order of the members, so the text is written member by member in one fixed order (the memory, its source, the later references; then `changes`, `relation`, `work_ended`, `strongest`) and travels to the harness as text. `strongest` offers each later reference by its alias, in order, and then `none`.
    pub fn request_json(&self) -> String {
        fn references(references: &[StateReference]) -> Vec<WireReference<'_>> {
            references
                .iter()
                .map(|reference| WireReference {
                    id: &reference.alias,
                    writer: reference.writer.as_str(),
                    time: reference.time.as_deref(),
                    text: &reference.text,
                })
                .collect()
        }
        let request = WireRequest {
            state: WireState {
                memory: WireMemory {
                    text: &self.memory,
                    said_by: self.said_by.as_str(),
                    category: &self.category,
                    work_in_progress: self.work_in_progress,
                },
                source: references(&self.source),
                later: references(&self.later),
            },
            questions: WireQuestions {
                changes: WireQuestion {
                    kind: "bool",
                    instructions: CHANGES_INSTRUCTIONS,
                    criteria: BoolCriteria {
                        yes: CHANGES_TRUE,
                        no: CHANGES_FALSE,
                    },
                },
                relation: WireQuestion {
                    kind: "choice",
                    instructions: RELATION_INSTRUCTIONS,
                    criteria: RelationCriteria {
                        replaces: RELATION_REPLACES,
                        adds: RELATION_ADDS,
                        restates: RELATION_RESTATES,
                        unrelated: RELATION_UNRELATED,
                    },
                },
                work_ended: WireQuestion {
                    kind: "bool",
                    instructions: WORK_ENDED_INSTRUCTIONS,
                    criteria: BoolCriteria {
                        yes: WORK_ENDED_TRUE,
                        no: WORK_ENDED_FALSE,
                    },
                },
                strongest: WireQuestion {
                    kind: "choice",
                    instructions: STRONGEST_INSTRUCTIONS,
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
struct WireState<'a> {
    memory: WireMemory<'a>,
    source: Vec<WireReference<'a>>,
    later: Vec<WireReference<'a>>,
}

#[derive(Serialize)]
struct WireMemory<'a> {
    text: &'a str,
    said_by: &'a str,
    category: &'a str,
    work_in_progress: bool,
}

#[derive(Serialize)]
struct WireReference<'a> {
    id: &'a str,
    writer: &'a str,
    time: Option<&'a str>,
    text: &'a str,
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
    instructions: &'static str,
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

const CHANGES_INSTRUCTIONS: &str = "`memory` is one statement a coding project stored in an earlier session; later sessions receive it as something the user said or as a fact the assistant reported (`memory.said_by`). `source` is the message it was taken from. `later` lists messages written after the source, each with its writer and time. Does an entry of `later` change what later sessions should be told by this memory? A statement of the user changes only on a later message the user wrote. A fact the assistant reported changes on a later assistant message or tool result that states the new value, or on a later user message.";
const CHANGES_TRUE: &str = "A later message withdraws the memory's statement, gives a different value for the same subject and the same slot, or reports that the work the memory describes is finished or dropped.";
const CHANGES_FALSE: &str = "No later message does that: the later messages repeat, acknowledge or build on the memory, add a separate rule or fact beside it, or concern something else.";
const RELATION_INSTRUCTIONS: &str = "Take the entry of `later` that bears most strongly on `memory`. How does it relate to the memory's statement?";
const RELATION_REPLACES: &str = "It sets a different value for the same subject and the same slot as the memory (the same kind of name in the same position, the same tool for the same purpose, the same setting), or it withdraws the memory's statement.";
const RELATION_ADDS: &str = "It states a separate rule or fact about another subject or another slot, such as another kind of name, a suffix beside a prefix, another tool, or another scope. The memory and the later message can both hold.";
const RELATION_RESTATES: &str =
    "It repeats, confirms, acknowledges or builds on the memory without changing it.";
const RELATION_UNRELATED: &str = "It does not bear on the memory, or `later` is empty.";
const WORK_ENDED_INSTRUCTIONS: &str = "When `memory.work_in_progress` is true the memory describes work that was unfinished when it was stored. Does an entry of `later` say that this work is finished or dropped?";
const WORK_ENDED_TRUE: &str = "A later message says the work the memory describes is finished, merged, shipped, abandoned or no longer needed.";
const WORK_ENDED_FALSE: &str =
    "The memory describes no work in progress, or no later message says its work ended.";
const STRONGEST_INSTRUCTIONS: &str =
    "Which entry of `later` bears most strongly on `memory`? Answer with its id.";
const STRONGEST_NONE: &str = "no entry of `later` bears on the memory, or `later` is empty";

#[derive(Debug, Clone, PartialEq)]
pub struct Answers {
    pub changes: f64,
    pub relation: String,
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
        let relation = choice("relation")?;
        if !["replaces", "adds", "restates", "unrelated"].contains(&relation) {
            return Err(MalformedAnswers);
        }
        let strongest = choice("strongest")?;
        Ok(Self {
            changes: probability("changes")?,
            relation: relation.to_string(),
            work_ended: probability("work_ended")?,
            strongest: state
                .later
                .iter()
                .any(|reference| reference.alias == strongest)
                .then(|| strongest.to_string()),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage1 {
    Keep,
    Flagged,
}

impl Stage1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Keep => "keep",
            Self::Flagged => "flagged",
        }
    }
}

/// The classifier flags a review if an eligible later reference exists and the change probability reaches [`FLAG_THRESHOLD`], or if the memory is work in progress and the probability that work ended reaches [`FLAG_THRESHOLD`]. Otherwise the review settles as a keep.
pub fn decide(state: &ReviewState, answers: &Answers) -> Stage1 {
    let changed = state.has_eligible_later() && answers.changes >= FLAG_THRESHOLD;
    let ended =
        state.work_in_progress && !state.later.is_empty() && answers.work_ended >= FLAG_THRESHOLD;
    if changed || ended {
        Stage1::Flagged
    } else {
        Stage1::Keep
    }
}

/// The classifier one project's reviews ask first, and where its requests wait for the harness.
#[derive(Clone)]
pub struct ClassifierRoute {
    pub exchange: Arc<ClassifierExchange>,
    /// The `provider/model` the harness looks the classifier up by.
    pub classifier: String,
    /// The Kernel project digest the harness's routes are bound under.
    pub project: String,
    pub wait: Duration,
    /// The pause between two attempts of one run after an attempt ended without answers.
    pub retry_pause: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stage1Record {
    pub decision: Stage1,
    pub answers: Answers,
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
    lease: Option<(String, Instant)>,
}

#[derive(Default)]
struct Inner {
    next_id: u64,
    parked: VecDeque<Parked>,
    pulls: HashMap<(String, String), Instant>,
    failures: HashMap<(String, String), Instant>,
    records: BTreeMap<String, Stage1Record>,
    record_order: VecDeque<String>,
}

#[derive(Default)]
pub struct ClassifierExchange {
    inner: Mutex<Inner>,
    arrived: Notify,
}

impl ClassifierExchange {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn present(&self, project: &str, classifier: &str) -> bool {
        self.lock()
            .pulls
            .get(&(project.to_string(), classifier.to_string()))
            .is_some_and(|at| at.elapsed() <= PRESENCE_WINDOW)
    }

    /// Records that a classifier request of `project` ended without answers.
    pub fn note_failure(&self, project: &str, classifier: &str) {
        self.lock().failures.insert(
            (project.to_string(), classifier.to_string()),
            Instant::now(),
        );
    }

    /// Whether a classifier request of `project` failed within [`FAILURE_COOLDOWN`].
    pub fn failing(&self, project: &str, classifier: &str) -> bool {
        self.lock()
            .failures
            .get(&(project.to_string(), classifier.to_string()))
            .is_some_and(|at| at.elapsed() <= FAILURE_COOLDOWN)
    }

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
        self.arrived.notify_waiters();
        let outcome = tokio::select! {
            biased;
            () = cancel.cancelled() => Err(ClassifyError::Cancelled),
            answered = tokio::time::timeout(wait, answered) => match answered {
                Ok(Ok(result)) => result,
                Ok(Err(_)) | Err(_) => Err(ClassifyError::Unanswered),
            },
        };
        self.lock().parked.retain(|parked| parked.id != id);
        outcome
    }

    pub async fn next(
        &self,
        project: &str,
        classifier: &str,
        wait: Duration,
    ) -> Option<ClassifierWork> {
        let deadline = Instant::now() + wait;
        loop {
            let arrived = self.arrived.notified();
            tokio::pin!(arrived);
            // Registering before the scan means a request parked between the scan and the wait still wakes this pull.
            arrived.as_mut().enable();
            if let Some(work) = self.take(project, classifier) {
                return Some(work);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            // A lapsed lease frees its request silently, so the wait runs in short slices that rescan.
            let slice = remaining.min(Duration::from_millis(250));
            let _ = tokio::time::timeout(slice, arrived).await;
        }
    }

    fn take(&self, project: &str, classifier: &str) -> Option<ClassifierWork> {
        let mut inner = self.lock();
        let now = Instant::now();
        inner
            .pulls
            .insert((project.to_string(), classifier.to_string()), now);
        let parked = inner.parked.iter_mut().find(|parked| {
            parked.project == project
                && parked.classifier == classifier
                && parked.reply.is_some()
                && parked.lease.as_ref().is_none_or(|(_, until)| *until <= now)
        })?;
        let mut nonce = [0_u8; 16];
        getrandom::getrandom(&mut nonce).ok()?;
        let lease: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
        parked.lease = Some((lease.clone(), now + LEASE_DURATION));
        Some(ClassifierWork {
            lease,
            classifier: parked.classifier.clone(),
            request: Arc::clone(&parked.request),
        })
    }

    pub fn submit(&self, project: &str, lease: &str, result: Result<Value, String>) -> bool {
        let mut inner = self.lock();
        let now = Instant::now();
        let Some(parked) = inner.parked.iter_mut().find(|parked| {
            parked.project == project
                && parked
                    .lease
                    .as_ref()
                    .is_some_and(|(held, until)| held == lease && *until > now)
        }) else {
            return false;
        };
        parked
            .reply
            .take()
            .is_some_and(|reply| reply.send(result.map_err(ClassifyError::Harness)).is_ok())
    }

    pub fn record(&self, causal_identity: &str, record: Stage1Record) {
        let mut inner = self.lock();
        if inner
            .records
            .insert(causal_identity.to_string(), record)
            .is_none()
        {
            inner.record_order.push_back(causal_identity.to_string());
        }
        while inner.record_order.len() > MAX_RECORDS {
            if let Some(oldest) = inner.record_order.pop_front() {
                inner.records.remove(&oldest);
            }
        }
    }

    pub fn take_record(&self, causal_identity: &str) -> Option<Stage1Record> {
        let mut inner = self.lock();
        let record = inner.records.remove(causal_identity)?;
        inner.record_order.retain(|kept| kept != causal_identity);
        Some(record)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn reference(alias: &str, writer: Writer, text: &str) -> StateReference {
        StateReference {
            alias: alias.to_string(),
            writer,
            time: Some("2026-03-01T10:00:00Z".to_string()),
            text: text.to_string(),
        }
    }

    fn user_statement(later: Vec<StateReference>) -> ReviewState {
        ReviewState {
            memory: "User stated: Deploy from the release branch.".to_string(),
            said_by: Writer::User,
            category: "PROJECT_RULES".to_string(),
            work_in_progress: false,
            source: vec![reference(
                "ref-2",
                Writer::User,
                "Deploy from the release branch.",
            )],
            later,
        }
    }

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
            decided(&work, answers(0.1, "restates", 0.7, "ref-3")),
            Stage1::Flagged
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
        assert_eq!(parsed.relation, "adds");
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
        assert_eq!(
            exchange
                .next("other", "typesafe/jev-latest", Duration::from_millis(20))
                .await,
            None,
            "another project's harness"
        );
        assert_eq!(
            exchange
                .next("project", "other/classifier", Duration::from_millis(20))
                .await,
            None,
            "another classifier's harness"
        );
        let work = exchange
            .next("project", "typesafe/jev-latest", Duration::from_secs(5))
            .await
            .expect("the parked request");
        assert_eq!(&*work.request, r#"{"state":{},"questions":{}}"#);
        assert!(exchange.present("project", "typesafe/jev-latest"));
        assert_eq!(
            exchange
                .next("project", "typesafe/jev-latest", Duration::from_millis(20))
                .await,
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
    async fn an_unanswered_failed_or_cancelled_request_leaves_the_exchange() {
        let exchange = Arc::new(ClassifierExchange::default());
        let classify = |wait: Duration, cancel: CancellationToken| {
            let exchange = Arc::clone(&exchange);
            async move {
                exchange
                    .classify("project", "c/m", "{}".to_string(), wait, &cancel)
                    .await
            }
        };
        assert_eq!(
            classify(Duration::from_millis(20), CancellationToken::new()).await,
            Err(ClassifyError::Unanswered)
        );
        assert_eq!(
            exchange
                .next("project", "c/m", Duration::from_millis(5))
                .await,
            None,
            "the unanswered request left with its run"
        );
        let run = tokio::spawn(classify(Duration::from_secs(5), CancellationToken::new()));
        let work = exchange
            .next("project", "c/m", Duration::from_secs(5))
            .await
            .unwrap();
        assert!(exchange.submit(
            "project",
            &work.lease,
            Err("provider_unavailable".to_string())
        ));
        assert_eq!(
            run.await.unwrap(),
            Err(ClassifyError::Harness("provider_unavailable".to_string()))
        );
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(
            classify(Duration::from_secs(5), cancel).await,
            Err(ClassifyError::Cancelled)
        );
        assert_eq!(
            exchange
                .next("project", "c/m", Duration::from_millis(5))
                .await,
            None
        );
    }

    #[test]
    fn a_failure_marks_the_project_as_failing() {
        let exchange = ClassifierExchange::default();
        assert!(!exchange.failing("project", "c/m"));
        exchange.note_failure("project", "c/m");
        assert!(exchange.failing("project", "c/m"));
        assert!(!exchange.failing("other", "c/m"));
        assert!(!exchange.failing("project", "other/m"));
    }

    #[test]
    fn a_stage_one_record_is_read_once() {
        let exchange = ClassifierExchange::default();
        let state = user_statement(vec![reference("ref-3", Writer::User, "Deploy from main.")]);
        let parsed = Answers::parse(&state, &answers(0.8, "replaces", 0.0, "ref-3")).unwrap();
        exchange.record(
            "job",
            Stage1Record {
                decision: decide(&state, &parsed),
                answers: parsed.clone(),
            },
        );
        assert_eq!(
            exchange.take_record("job"),
            Some(Stage1Record {
                decision: Stage1::Flagged,
                answers: parsed,
            })
        );
        assert_eq!(exchange.take_record("job"), None);
    }
}

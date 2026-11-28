//! Change detection at the end of a session: one request to the review model that reads a session's user messages beside every statement the project's memory delivers, and names each statement a message changes.
//!
//! A later message that changes a standing statement rarely names it or repeats its words: the user restates the value, replaces one part, adds an exception, or moves on to other work. So the pass does not pick the statements a message may bear on; it lists them. The statements are the rows a new session of the project receives now (the block, bounded by the user's memory budget), and the messages are the user messages of one session that ended, newest first within [`MAX_MESSAGE_BYTES`]. The model answers with the statements a message changes and how: replaced, narrowed, moot, or, for work in progress, ended, each with the message and its words.
//!
//! A session is read once it has ended: when a caller drains the worker at a session's end (a host that knows the session ended), when its newest message is [`QUIET`] old, or when a newer session of the project has started. Each pass reads at most one session, the oldest of the project's [`MAX_SESSIONS_CONSIDERED`] most recent sessions that holds user messages no pass covered.
//!
//! The daemon checks each answer against the stores before it writes: the statement is one the request listed, the message is one the request listed and no earlier pass covered, the message is later than the statement and is not the message the statement was taken from, and the words are copied from that message exactly. Each change is written in a commit that first checks the message's blocks are still live, so a session deleted while the request waited writes nothing. A change is written so it can be undone, and the changed row stays on explicit search with what changed it:
//! - replaced: the user's later sentence is recorded as the user's statement, approved for delivery when the model read it as the user's own words and it is one or more whole sentences of the message, and the old statement is superseded by a labeled row whose rationale names the change. Otherwise the old statement stays delivered and its line is marked contested by the sentence.
//! - narrowed: the statement stays delivered and its line is marked contested by the sentence, with both dates.
//! - moot: the statement is superseded by a labeled row that names the change, when the model read the words as the user's own; otherwise it is contested.
//! - ended: the work in progress leaves the work section and stays on search as ended by the sentence, when the model read the words as the user's own; otherwise it is contested.
//! - an answer the model gives with low confidence marks the statement contested at most.
//!
//! Each pass records a receipt for the newest user message it covered, so a session is read again only when it holds a newer user message; the messages earlier passes covered are then sent as context, and a change citing one of them is not written. A request that fails is asked again after [`RETRY_AFTER`], doubled at each failure, at most [`MAX_ATTEMPTS`] times for one session's newest message; an answer from a model other than the one configured, or a refused credential, closes review for the rest of the process as it does on the per-memory route.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kernel::applicability::EvalBudget;
use kernel::source_identity::OccurrenceClass;
use kernel::{
    ArtifactDestination, ArtifactHandle, CommitIntent, DecisionRow, EligibilityCandidate,
    KernelError, KernelStore, MAX_ELIGIBILITY_CANDIDATES, ProjectScope, Sensitivity, SourceClass,
    Surface,
};
use memory_store::memory_reviewer_ledger::ResponseAllowance;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::broker::check_render;
use super::coordinator::{MAX_REPORTED_MODEL_CHARS, Rejection};
use super::host_execution::{CHILD_SESSION_PREFIX, HostExecution};
use super::model_request::{ResponseAccounting, SendError};
use super::model_response::StopReason;
use super::steps::unfenced;
use crate::canonical_memory::{CanonicalMemory, delivered_rows};
use crate::kernel_routes::ProjectBinding;
use crate::memory_capture::{self, Change, ReviewedSentence};
use crate::memory_render::CURRENT_WORK_SECTION;

/// The producer of every commit a pass makes.
pub const PRODUCER: &str = "eidnara-daemon/memory-reviewer-change-detection";
/// The user messages one request carries, newest first: about 6,000 tokens of prose, which holds a long session's typed messages and keeps the request inside a model's reliable reading length beside the statements.
pub const MAX_MESSAGE_BYTES: usize = 24 * 1024;
/// One message's share of [`MAX_MESSAGE_BYTES`]: a pasted log or file stays a part of the session's messages and leaves room for the others. The head is kept, where a user's own words come first.
pub const MAX_ONE_MESSAGE_BYTES: usize = 6 * 1024;
/// The statements one request carries. The block a session receives is bounded by the memory budget, 4,000 tokens by default, so its statements fit one request; a larger budget splits them by section into requests of at most this many bytes, each with the same messages.
pub const MAX_STATEMENT_BYTES: usize = 24 * 1024;
/// Requests one pass sends at most: a block four times the default budget. Statements past them are not read by the pass, which says how many in its log line.
pub const MAX_REQUESTS_PER_PASS: usize = 4;
/// The answer's output cap. An answer lists only the statements that changed.
pub const MAX_TOKENS: u32 = 4096;
/// Attempts at one session's newest message: after the last failed one the session is recorded covered with no answer, and its log line says so.
pub const MAX_ATTEMPTS: u32 = 3;
/// The wait after a failed attempt before the next, doubled at each further failure.
pub const RETRY_AFTER: Duration = Duration::from_secs(60);
/// The longest a pass waits for capture's queue of the project to settle; after it the pass reads the session beside the statements capture has written so far, and logs that it did.
pub const CAPTURE_WAIT: Duration = Duration::from_secs(30 * 60);
/// Recent sessions of a project a pass considers for one whose user messages no pass covered: the newest four, by their newest message.
pub const MAX_SESSIONS_CONSIDERED: usize = 4;
/// The longest a request waits for its answer.
const REQUEST_WAIT: Duration = Duration::from_secs(90);
/// Changes one answer may carry; past it the answer is read up to the bound.
const MAX_CHANGES_PER_ANSWER: usize = 32;
/// The longest sentence a change may cite; a user's statement is a few sentences.
const MAX_SENTENCE_BYTES: usize = 2048;
/// Message descriptors a pass examines to find the project's sessions: 32 pages of 1,024. A project with more makes the pass stop without a reading, and it logs that once.
const MAX_SCANNED_DESCRIPTORS: usize = 32 * 1024;
const DESCRIPTOR_PAGE: NonZeroUsize = NonZeroUsize::new(1024).unwrap();
/// The longest the pass's store reads and its eligibility judgement take.
const READ_DEADLINE: Duration = Duration::from_secs(10);

/// The fixed instructions of every request. They quote no project, so a provider can cache them.
pub const SYSTEM_TEXT: &str = r#"You check whether the stored statements of a coding project still hold after the user's latest session.

You receive one JSON object with two lists. `statements` is what the project's memory tells every new session now. Each statement has an id, its section, the day it was recorded, and its text; most are statements the user made earlier ("User stated: ..."). A statement with "work": true is work the user left in progress. A statement with "from" was taken from that message of the list. `messages` is what the user wrote in the latest session, oldest first, each with an id and its time.

For each statement, decide whether something the user wrote in `messages` changes what a new session should be told by it. A message changes a statement when it:
- replaced: gives a new value, a new choice, or a new rule for the same thing, so the statement no longer holds as recorded. The message need not mention the statement or its words; a restatement with another value replaces it, and so does a reversal to an earlier value.
- narrowed: changes one part of the statement, adds an exception or a condition, or limits where it applies, while the rest of it still holds.
- moot: makes the statement pointless without giving a replacement, for example by removing or abandoning what it is about.
- ended: for work in progress, shows that the work is finished, cancelled, or set aside: the user says so, or moves the work on to something else in its place.

A statement no message changes is left out of your answer. A message that repeats a statement, agrees with it, applies it, adds a fact about something else, asks a question, or asks for a one-off task that leaves the standing statement in force does not change it. A statement taken from a message is not changed by that same message.

Read who is speaking. A message can quote or paste the words of another person, a document, an issue, a tool's output, or an assistant. Words the user passes on from another source change a statement only where the user adopts them in their own words.

For each change give:
- statement: the statement's id
- effect: replaced, narrowed, moot, or ended
- message: the id of the message that makes the change; the latest one when several do
- sentence: the words of that message that state the change, copied exactly, as one or more whole sentences
- speaker: user when the sentence is the user's own words; other when it holds words the user passes on from another source
- confidence: high when the message plainly changes the statement; low when it reads either way

Answer with one JSON object and nothing else:
{"changes":[{"statement":"s3","effect":"replaced","message":"m2","sentence":"Exact words from m2.","speaker":"user","confidence":"high"}]}
When nothing changed, answer {"changes":[]}."#;

/// When a pass runs: at a session's end, which a drain marks, or on the worker's own loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moment {
    SessionEnd,
    Idle,
}

/// How long the newest session of a project is left alone after its newest message before the worker's loop reads it: a session that has gone this long without a message has ended in practice, so its messages are read once rather than once per turn.
pub const QUIET: Duration = Duration::from_secs(10 * 60);

/// One session's newest user message and the failed attempts at it.
struct Retry {
    session: String,
    newest: String,
    failures: u32,
    next_ms: i64,
}

/// What a pass remembers between passes for each project: the Kernel tip at which it last found nothing left to read, the failed attempts at a session, since when capture's queue has held the pass back, and whether its scan stopped at its bound. Each map holds at most [`Self::MAX_PROJECTS`] projects.
#[derive(Default)]
pub struct DetectionMemory {
    quiet_at: Mutex<HashMap<String, i64>>,
    waiting_until: Mutex<HashMap<String, (i64, i64)>>,
    retries: Mutex<HashMap<String, Retry>>,
    capture_wait: Mutex<HashMap<String, i64>>,
    cut_scans: Mutex<HashSet<String>>,
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl DetectionMemory {
    const MAX_PROJECTS: usize = 4096;

    fn quiet(&self, project: &str, tip: i64) -> bool {
        locked(&self.quiet_at).get(project) == Some(&tip)
    }

    fn note_quiet(&self, project: &str, tip: i64) {
        let mut quiet = locked(&self.quiet_at);
        if quiet.len() >= Self::MAX_PROJECTS && !quiet.contains_key(project) {
            quiet.clear();
        }
        quiet.insert(project.to_string(), tip);
    }

    /// Whether the loop found only sessions still going at `tip` and none of them is quiet yet at `now_ms`.
    fn still_waiting(&self, project: &str, tip: i64, now_ms: i64) -> bool {
        locked(&self.waiting_until)
            .get(project)
            .is_some_and(|(at_tip, until)| *at_tip == tip && now_ms < *until)
    }

    fn note_waiting(&self, project: &str, tip: i64, until: i64) {
        let mut waiting = locked(&self.waiting_until);
        if waiting.len() >= Self::MAX_PROJECTS && !waiting.contains_key(project) {
            waiting.clear();
        }
        waiting.insert(project.to_string(), (tip, until));
    }

    /// Whether the failed attempts at `session`'s `newest` message leave the next one to wait at `now_ms`.
    fn backing_off(&self, project: &str, session: &str, newest: &str, now_ms: i64) -> bool {
        locked(&self.retries).get(project).is_some_and(|retry| {
            retry.session == session && retry.newest == newest && now_ms < retry.next_ms
        })
    }

    /// The attempts that failed at `session`'s `newest` message so far.
    fn failures(&self, project: &str, session: &str, newest: &str) -> u32 {
        locked(&self.retries)
            .get(project)
            .filter(|retry| retry.session == session && retry.newest == newest)
            .map_or(0, |retry| retry.failures)
    }

    /// Records a failed attempt at `session`'s `newest` message and returns how many have failed.
    fn note_failure(&self, project: &str, session: &str, newest: &str, now_ms: i64) -> u32 {
        let mut retries = locked(&self.retries);
        if retries.len() >= Self::MAX_PROJECTS && !retries.contains_key(project) {
            retries.clear();
        }
        let failures = retries
            .get(project)
            .filter(|retry| retry.session == session && retry.newest == newest)
            .map_or(0, |retry| retry.failures)
            + 1;
        let wait = RETRY_AFTER.saturating_mul(1 << (failures - 1).min(8));
        retries.insert(
            project.to_string(),
            Retry {
                session: session.to_string(),
                newest: newest.to_string(),
                failures,
                next_ms: now_ms.saturating_add(i64::try_from(wait.as_millis()).unwrap_or(i64::MAX)),
            },
        );
        failures
    }

    fn clear_failures(&self, project: &str) {
        locked(&self.retries).remove(project);
    }

    /// Whether capture's unsettled queue still holds the pass back at `now_ms`; the first call of a wait logs it and starts its clock.
    fn capture_holds(&self, project: &str, now_ms: i64) -> bool {
        let mut waits = locked(&self.capture_wait);
        if waits.len() >= Self::MAX_PROJECTS && !waits.contains_key(project) {
            waits.clear();
        }
        let since = *waits.entry(project.to_string()).or_insert_with(|| {
            eprintln!(
                "daemon: memory_reviewer change detection waits for capture's queue of a project to settle"
            );
            now_ms
        });
        let wait = i64::try_from(CAPTURE_WAIT.as_millis()).unwrap_or(i64::MAX);
        if now_ms.saturating_sub(since) < wait {
            return true;
        }
        eprintln!(
            "daemon: memory_reviewer change detection reads a session while capture's queue of its project is unsettled after {} minutes",
            CAPTURE_WAIT.as_secs() / 60
        );
        waits.remove(project);
        false
    }

    fn capture_settled(&self, project: &str) {
        locked(&self.capture_wait).remove(project);
    }

    /// Logs once per project that its descriptor scan stopped at its bound.
    fn note_cut_scan(&self, project: &str) {
        let mut cut = locked(&self.cut_scans);
        if cut.len() >= Self::MAX_PROJECTS {
            return;
        }
        if cut.insert(project.to_string()) {
            eprintln!(
                "daemon: memory_reviewer change detection examines at most {MAX_SCANNED_DESCRIPTORS} message descriptors of a project and the store holds more; its sessions are not read"
            );
        }
    }
}

/// One user message of the session a pass reads.
#[derive(Debug, Clone)]
pub struct SessionMessage {
    /// The native message id.
    pub message_id: String,
    /// The descriptor of the message's first text block, whose evidence a change names.
    pub object_id: String,
    /// Every text block of the message: its descriptor and its artifact digest. A write first checks each is still live.
    pub blocks: Vec<(String, String)>,
    /// The harness's timestamp, in milliseconds.
    pub written_ms: i64,
    /// The commit that published the message's last text block.
    pub published_seq: i64,
    pub text: String,
    /// Whether the text was cut at [`MAX_ONE_MESSAGE_BYTES`].
    pub cut: bool,
    /// Whether an earlier pass covered the message; it is sent as context and grounds no change.
    pub covered: bool,
}

/// One statement the block delivers, as a pass reads it.
#[derive(Debug, Clone)]
pub struct DeliveredStatement {
    pub row: CanonicalMemory,
    pub decision: DecisionRow,
    pub work: bool,
    /// The commit that first recorded the statement.
    pub stated_seq: i64,
}

/// The session a pass covers and what it reads of it.
#[derive(Debug)]
pub struct SessionRead {
    pub session: String,
    /// The user's messages, oldest first.
    pub messages: Vec<SessionMessage>,
    /// The newest user block of the session by publication: the pass's receipt names it.
    pub newest: String,
    /// Messages left out for their sensitivity, their egress verdict, a malformed block, or the render check.
    pub withheld: usize,
}

/// How a message changes a statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// A new value, choice, or rule for the same thing.
    Replaced,
    /// One part changes, or an exception or condition is added.
    Narrowed,
    /// The statement is pointless now, with no replacement.
    Moot,
    /// Work in progress is finished, cancelled, or set aside.
    Ended,
}

impl Effect {
    fn parse(text: &str) -> Option<Self> {
        match text {
            "replaced" => Some(Self::Replaced),
            "narrowed" => Some(Self::Narrowed),
            "moot" => Some(Self::Moot),
            "ended" => Some(Self::Ended),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Replaced => "replaced",
            Self::Narrowed => "narrowed",
            Self::Moot => "moot",
            Self::Ended => "ended",
        }
    }
}

/// Whose words a change cites, as the model read them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speaker {
    /// The user's own words.
    User,
    /// Words the user passes on from another source.
    Other,
    /// The answer named no speaker.
    Unknown,
}

/// One change of an answer, with its aliases resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The index of the statement in the pass's statements.
    pub statement: usize,
    pub effect: Effect,
    /// The index of the message in the pass's messages.
    pub message: usize,
    /// The words of the message the change cites.
    pub sentence: String,
    pub speaker: Speaker,
    /// Whether the model answered with high confidence.
    pub high: bool,
}

impl Finding {
    fn by_user(&self) -> bool {
        self.speaker == Speaker::User
    }
}

impl Speaker {
    /// The speaker a recorded row's rationale carries: none when the answer named none.
    fn recorded(self) -> Option<memory_capture::Speaker> {
        match self {
            Self::User => Some(memory_capture::Speaker::User),
            Self::Other => Some(memory_capture::Speaker::Other),
            Self::Unknown => None,
        }
    }
}

#[derive(Deserialize)]
struct RawAnswer {
    #[serde(default)]
    changes: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct RawChange {
    statement: String,
    effect: String,
    message: String,
    sentence: String,
    #[serde(default)]
    speaker: Option<String>,
    #[serde(default)]
    confidence: Option<String>,
}

/// The alias of statement `index` and of message `index` in a request.
fn statement_alias(index: usize) -> String {
    format!("s{}", index + 1)
}

fn message_alias(index: usize) -> String {
    format!("m{}", index + 1)
}

fn utc_minute(ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms).map_or_else(
        || "unknown time".to_string(),
        |at| at.format("%Y-%m-%d %H:%M").to_string(),
    )
}

fn utc_day(ms: Option<i64>) -> String {
    ms.and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
        .map_or_else(
            || "unknown day".to_string(),
            |at| at.format("%Y-%m-%d").to_string(),
        )
}

/// The message a statement was taken from, as its capture rationale records it, when that message is in this session.
fn source_message(statement: &DeliveredStatement, read_session: &str) -> Option<String> {
    let source = memory_capture::capture_source(&statement.decision);
    source
        .message
        .filter(|_| source.session.as_deref() == Some(read_session))
}

/// The request's user message: the statements of `chunk` and the session's messages, as JSON in a fixed member order.
pub fn request_json(
    statements: &[DeliveredStatement],
    chunk: &[usize],
    read: &SessionRead,
    recorded_ms: &HashMap<i64, i64>,
) -> String {
    let from = |statement: &DeliveredStatement| {
        let source = source_message(statement, &read.session)?;
        read.messages
            .iter()
            .position(|message| message.message_id == source)
            .map(message_alias)
    };
    let listed: Vec<serde_json::Value> = chunk
        .iter()
        .map(|&index| {
            let statement = &statements[index];
            let mut entry = serde_json::Map::new();
            entry.insert("id".into(), statement_alias(index).into());
            entry.insert("section".into(), statement.row.category.clone().into());
            entry.insert(
                "recorded".into(),
                utc_day(recorded_ms.get(&statement.stated_seq).copied()).into(),
            );
            if statement.work {
                entry.insert("work".into(), true.into());
            }
            if let Some(alias) = from(statement) {
                entry.insert("from".into(), alias.into());
            }
            entry.insert("text".into(), statement.row.content.clone().into());
            serde_json::Value::Object(entry)
        })
        .collect();
    let written: Vec<serde_json::Value> = read
        .messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            serde_json::json!({
                "id": message_alias(index),
                "time": utc_minute(message.written_ms),
                "text": message.text,
            })
        })
        .collect();
    serde_json::json!({ "statements": listed, "messages": written }).to_string()
}

/// The statements of `statements` split into requests: by section, in the block's order, each request at most [`MAX_STATEMENT_BYTES`] of statement text and at most [`MAX_REQUESTS_PER_PASS`] requests. Returns the requests and how many statements they leave out: a statement larger than one request, and every statement past the last request.
pub fn chunks(statements: &[DeliveredStatement]) -> (Vec<Vec<usize>>, usize) {
    let mut sections: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    let mut order: Vec<&str> = Vec::new();
    for (index, statement) in statements.iter().enumerate() {
        let section = statement.row.category.as_str();
        if !sections.contains_key(section) {
            order.push(section);
        }
        sections.entry(section).or_default().push(index);
    }
    let mut out: Vec<Vec<usize>> = vec![Vec::new()];
    let mut omitted = 0;
    let mut bytes = 0;
    for section in order {
        for &index in &sections[section] {
            let size = statements[index].row.content.len();
            if size > MAX_STATEMENT_BYTES {
                omitted += 1;
                continue;
            }
            if bytes + size > MAX_STATEMENT_BYTES && !out.last().is_some_and(Vec::is_empty) {
                if out.len() == MAX_REQUESTS_PER_PASS {
                    omitted += 1;
                    continue;
                }
                out.push(Vec::new());
                bytes = 0;
            }
            out.last_mut().expect("one request").push(index);
            bytes += size;
        }
    }
    out.retain(|chunk| !chunk.is_empty());
    (out, omitted)
}

/// The changes `text` names, read in the closed form of [`SYSTEM_TEXT`]: an alias outside the request, an effect or speaker outside its words, or an empty sentence drops that change. `None` for an answer that is not the object. A fenced object reads as the object.
pub fn parse_answer(text: &str, chunk: &[usize], messages: usize) -> Option<Vec<Finding>> {
    let answer = serde_json::from_str::<RawAnswer>(unfenced(text)).ok()?;
    let statement_of = |alias: &str| {
        chunk
            .iter()
            .copied()
            .find(|&index| statement_alias(index) == alias)
    };
    let message_of = |alias: &str| (0..messages).find(|&index| message_alias(index) == alias);
    Some(
        answer
            .changes
            .into_iter()
            .take(MAX_CHANGES_PER_ANSWER)
            .filter_map(|value| {
                let raw: RawChange = serde_json::from_value(value).ok()?;
                let speaker = match raw.speaker.as_deref() {
                    Some("user") => Speaker::User,
                    Some("other") => Speaker::Other,
                    None => Speaker::Unknown,
                    Some(_) => return None,
                };
                let high = match raw.confidence.as_deref() {
                    Some("high") => true,
                    Some("low") | None => false,
                    Some(_) => return None,
                };
                let sentence = raw.sentence.trim().to_string();
                if sentence.is_empty() || sentence.len() > MAX_SENTENCE_BYTES {
                    return None;
                }
                Some(Finding {
                    statement: statement_of(&raw.statement)?,
                    effect: Effect::parse(&raw.effect)?,
                    message: message_of(&raw.message)?,
                    sentence,
                    speaker,
                    high,
                })
            })
            .collect(),
    )
}

/// Whether `finding` holds against the stores: the words stand in the message, no earlier pass covered the message, the message is not the one the statement was taken from and is later than the statement, and the effect fits the statement (only work in progress ends). A message is later when it was published after the statement was first recorded, or when it was written after the message the statement was taken from, which a capture of the same session can publish after it.
pub fn grounded(finding: &Finding, statements: &[DeliveredStatement], read: &SessionRead) -> bool {
    let statement = &statements[finding.statement];
    let message = &read.messages[finding.message];
    let source = source_message(statement, &read.session);
    let written_after_source = source
        .as_deref()
        .and_then(|source| {
            read.messages
                .iter()
                .find(|other| other.message_id == source)
        })
        .is_some_and(|source| message.written_ms > source.written_ms);
    message.text.contains(&finding.sentence)
        && !message.covered
        && source.as_deref() != Some(message.message_id.as_str())
        && (message.published_seq > statement.stated_seq || written_after_source)
        && (finding.effect != Effect::Ended || statement.work)
}

/// What a grounded finding writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// The sentence is approved as the user's statement and the old statement is retired.
    Replace,
    /// The old statement stays delivered, marked contested by the sentence.
    Contest,
    /// The old statement is retired, naming the sentence.
    Retire,
    /// The work in progress ends, naming the sentence.
    End,
}

/// The action for a grounded finding: the effect, held to a contest when the model's confidence is low or the words are not the user's own, and a replacement held to a contest when the sentence is not whole sentences of the message or ends where the message was cut.
pub fn action(finding: &Finding, message: &SessionMessage) -> Action {
    if !finding.high || !finding.by_user() {
        return Action::Contest;
    }
    let whole = memory_capture::states_whole_sentences(&message.text, &finding.sentence)
        && !(message.cut && message.text.trim_end().ends_with(finding.sentence.as_str()));
    match finding.effect {
        Effect::Narrowed => Action::Contest,
        Effect::Moot => Action::Retire,
        Effect::Ended => Action::End,
        Effect::Replaced if whole => Action::Replace,
        Effect::Replaced => Action::Contest,
    }
}

/// Counts of one pass's outcome, for its log line, and what the worker does next.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PassOutcome {
    pub requests: usize,
    pub findings: usize,
    pub ungrounded: usize,
    pub replaced: usize,
    pub contested: usize,
    pub retired: usize,
    pub ended: usize,
    pub refused: usize,
    /// Statements past the pass's requests.
    pub omitted: usize,
    /// Whether the pass recorded its receipt for a session.
    pub covered: bool,
    /// A provider's rejection, which closes review for the rest of the process.
    pub rejection: Option<Rejection>,
}

fn intent(key: String, request: &str, cause: &str) -> CommitIntent {
    CommitIntent {
        producer: PRODUCER.to_string(),
        operation_key: key,
        request_digest: format!("{:x}", Sha256::digest(request.as_bytes())),
        actor: "memory-reviewer".to_string(),
        cause: cause.to_string(),
    }
}

fn pass_intent(project: &ProjectBinding, session: &str, newest: &str) -> CommitIntent {
    let key = format!("{}#pass#{session}#{newest}", project.digest());
    intent(
        key.clone(),
        &key,
        "change detection covered a session's messages",
    )
}

fn identity_value<'a>(detail: &'a kernel::SourceDescriptorDetail, field: &str) -> Option<&'a str> {
    detail
        .identity
        .iter()
        .find(|(name, _)| name == field)
        .map(|(_, value)| value.as_str())
}

/// A message block as the scan finds it.
#[derive(Debug, Clone)]
struct Block {
    object_id: String,
    session: String,
    message_id: String,
    /// `None` for an index that does not read as one.
    block_index: Option<u64>,
    written_ms: i64,
    published_seq: i64,
    sensitivity: Sensitivity,
    handle: ArtifactHandle,
    span: Option<(u64, u64)>,
}

/// A session the pass found to read: its user blocks, the newest of them by publication, and the commit up to which earlier passes covered it.
struct Found {
    session: String,
    newest: String,
    blocks: Vec<Block>,
    covered_through: i64,
}

/// The session a pass reads, with its user messages up to [`MAX_MESSAGE_BYTES`]. The candidates are the project's [`MAX_SESSIONS_CONSIDERED`] most recent sessions by their newest message; on the worker's loop a candidate waits until it has been quiet for [`QUIET`] or a session that started after its newest message has begun, and at a session's end every candidate is read at once. The oldest candidate whose newest user block no pass covered is read. [`Reading::Nothing`] when every candidate is covered; [`Reading::Waiting`] when only candidates still going are not; [`Reading::Cut`] when the scan stopped at its bound.
fn read_session(
    store: &KernelStore,
    project: &ProjectBinding,
    moment: Moment,
    now_ms: i64,
    allowed: &dyn Fn(&[Block]) -> Vec<bool>,
) -> Result<Reading, KernelError> {
    let quiet_ms = i64::try_from(QUIET.as_millis()).unwrap_or(i64::MAX);
    let (_, found) = store.preview(std::time::Instant::now() + READ_DEADLINE, |preview| {
        let mut blocks: Vec<Block> = Vec::new();
        let mut after: Option<String> = None;
        let mut scanned = 0;
        loop {
            let page = preview.live_source_descriptors_with(
                OccurrenceClass::Messages,
                "project_id",
                &[project.digest()],
                after.as_deref(),
                DESCRIPTOR_PAGE,
            )?;
            scanned += page.rows.len();
            for row in page.rows {
                let detail = &row.detail;
                let (Some(session), Some(message_id), Some(block)) = (
                    identity_value(detail, "session_id"),
                    identity_value(detail, "message_id"),
                    identity_value(detail, "block_index"),
                ) else {
                    continue;
                };
                let Ok(written_ms) = detail.revision.parse::<i64>() else {
                    continue;
                };
                blocks.push(Block {
                    object_id: row.object_id.clone(),
                    session: session.to_string(),
                    message_id: message_id.to_string(),
                    block_index: block.parse().ok(),
                    written_ms,
                    published_seq: row.created_commit_seq,
                    sensitivity: row.sensitivity,
                    handle: ArtifactHandle {
                        digest: detail.artifact_digest.clone(),
                        evidence_id: detail.evidence_id.clone(),
                    },
                    span: detail.span,
                });
            }
            match page.next {
                None => break,
                Some(_) if scanned >= MAX_SCANNED_DESCRIPTORS => return Ok(None),
                Some(next) => after = Some(next),
            }
        }
        // Each session's newest message and its first, newest session first.
        let mut sessions: Vec<((i64, i64), i64, String)> = Vec::new();
        for block in &blocks {
            let at = (block.written_ms, block.published_seq);
            match sessions
                .iter_mut()
                .find(|(_, _, name)| *name == block.session)
            {
                Some(entry) => {
                    entry.0 = entry.0.max(at);
                    entry.1 = entry.1.min(block.written_ms);
                }
                None => sessions.push((at, block.written_ms, block.session.clone())),
            }
        }
        sessions.sort_by(|left, right| right.cmp(left));
        let starts: Vec<i64> = sessions.iter().map(|(_, first, _)| *first).collect();
        sessions.truncate(MAX_SESSIONS_CONSIDERED);
        // On the worker's loop a session is read once it is quiet or a session that started after its newest message has begun; the earliest time a waiting one becomes quiet is when to look again.
        let mut waiting: Option<i64> = None;
        // Oldest candidate first, so a session that ended before a newer one is read before it.
        for ((newest_ms, _), _, session) in sessions.iter().rev() {
            let ended = now_ms.saturating_sub(*newest_ms) >= quiet_ms
                || starts.iter().any(|first| first > newest_ms);
            if moment == Moment::Idle && !ended {
                let quiet_at = newest_ms.saturating_add(quiet_ms);
                waiting = Some(waiting.map_or(quiet_at, |at| at.min(quiet_at)));
                continue;
            }
            // The session's user blocks: the writer is the source class the Kernel admitted each block under.
            let mut mine = Vec::new();
            for block in blocks.iter().filter(|block| block.session == *session) {
                let user = preview
                    .subject_admission(&block.object_id)?
                    .is_some_and(|(prior, _)| prior.source_class == SourceClass::ExplicitUser);
                if user {
                    mine.push(block.clone());
                }
            }
            let Some(newest) = mine
                .iter()
                .max_by(|left, right| {
                    (left.published_seq, &left.object_id)
                        .cmp(&(right.published_seq, &right.object_id))
                })
                .map(|block| block.object_id.clone())
            else {
                continue;
            };
            if preview
                .stored_receipt(pass_intent(project, session, &newest))?
                .is_some()
            {
                continue;
            }
            // The latest block an earlier pass named as its newest: what was published up to it was read then.
            let mut by_publication: Vec<&Block> = mine.iter().collect();
            by_publication.sort_by_key(|block| std::cmp::Reverse(block.published_seq));
            let mut covered_through = i64::MIN;
            for block in by_publication {
                if preview
                    .stored_receipt(pass_intent(project, session, &block.object_id))?
                    .is_some()
                {
                    covered_through = block.published_seq;
                    break;
                }
            }
            mine.sort_by(|left, right| {
                (left.written_ms, &left.message_id, left.block_index).cmp(&(
                    right.written_ms,
                    &right.message_id,
                    right.block_index,
                ))
            });
            return Ok(Some(Ok(Found {
                session: session.clone(),
                newest,
                blocks: mine,
                covered_through,
            })));
        }
        Ok(Some(Err(waiting)))
    })?;
    let found = match found {
        None => return Ok(Reading::Cut),
        Some(Err(Some(quiet_at))) => return Ok(Reading::Waiting(quiet_at)),
        Some(Err(None)) => return Ok(Reading::Nothing),
        Some(Ok(found)) => found,
    };
    let (messages, withheld) = messages_of(store, &found, allowed);
    Ok(Reading::Read(SessionRead {
        session: found.session,
        messages,
        newest: found.newest,
        withheld,
    }))
}

/// What a pass found to read.
#[derive(Debug)]
pub enum Reading {
    Read(SessionRead),
    /// No session holds a user message the pass has not covered.
    Nothing,
    /// The sessions left to read have not ended yet; the first becomes quiet at the time given, in milliseconds.
    Waiting(i64),
    /// The project holds more message descriptors than a pass examines.
    Cut,
}

/// The messages of `found`, oldest first, newest first within [`MAX_MESSAGE_BYTES`] and each cut at [`MAX_ONE_MESSAGE_BYTES`]: the blocks of one message joined in block order. Only the messages that fit are read. A message is left out when a block of it is `Secret`, refused by the Kernel's egress verdict for the configured model, has an index or span that does not read, is unreadable or not UTF-8, or when it fails the render check. Returns the messages and how many were left out.
fn messages_of(
    store: &KernelStore,
    found: &Found,
    allowed: &dyn Fn(&[Block]) -> Vec<bool>,
) -> (Vec<SessionMessage>, usize) {
    // The message's blocks in block order, messages newest first.
    let mut grouped: Vec<Vec<&Block>> = Vec::new();
    for block in &found.blocks {
        match grouped
            .iter_mut()
            .find(|group| group[0].message_id == block.message_id)
        {
            Some(group) => group.push(block),
            None => grouped.push(vec![block]),
        }
    }
    grouped.reverse();
    let mut kept: Vec<SessionMessage> = Vec::new();
    let mut withheld = 0;
    let mut bytes = 0;
    for group in grouped {
        if bytes >= MAX_MESSAGE_BYTES {
            break;
        }
        let owned: Vec<Block> = group.iter().map(|block| (*block).clone()).collect();
        let mut indices: Vec<Option<u64>> = owned.iter().map(|block| block.block_index).collect();
        let malformed = indices.iter().any(Option::is_none) || {
            indices.sort_unstable();
            indices.windows(2).any(|pair| pair[0] == pair[1])
        };
        if malformed
            || owned
                .iter()
                .any(|block| block.sensitivity == Sensitivity::Secret)
            || !allowed(&owned).into_iter().all(|permitted| permitted)
        {
            withheld += 1;
            continue;
        }
        let mut text = String::new();
        let mut readable = true;
        for block in &owned {
            let Ok(bytes) = store.read_artifact(&block.handle) else {
                readable = false;
                break;
            };
            let slice = match block.span {
                Some((start, end)) => match (usize::try_from(start), usize::try_from(end)) {
                    (Ok(start), Ok(end)) => bytes.get(start..end).map(<[u8]>::to_vec),
                    _ => None,
                },
                None => Some(bytes),
            };
            let Some(Ok(part)) = slice.map(String::from_utf8) else {
                readable = false;
                break;
            };
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&part);
        }
        if !readable || text.trim().is_empty() || check_render(text.as_bytes(), None).is_err() {
            withheld += 1;
            continue;
        }
        let cut = text.len() > MAX_ONE_MESSAGE_BYTES;
        if cut {
            let at = text.floor_char_boundary(MAX_ONE_MESSAGE_BYTES);
            text.truncate(at);
        }
        if bytes + text.len() > MAX_MESSAGE_BYTES {
            break;
        }
        bytes += text.len();
        let published_seq = owned
            .iter()
            .map(|block| block.published_seq)
            .max()
            .unwrap_or_default();
        kept.push(SessionMessage {
            message_id: owned[0].message_id.clone(),
            object_id: owned[0].object_id.clone(),
            blocks: owned
                .iter()
                .map(|block| (block.object_id.clone(), block.handle.digest.clone()))
                .collect(),
            written_ms: owned[0].written_ms,
            published_seq,
            text,
            cut,
            covered: published_seq <= found.covered_through,
        });
    }
    kept.reverse();
    (kept, withheld)
}

/// The Kernel's egress verdict for the configured model on each of `blocks`: a block is sent only when the verdict permits it, and a judgement that fails permits none.
fn egress_allowed(store: &KernelStore, project: &ProjectBinding, blocks: &[Block]) -> Vec<bool> {
    let refused = || vec![false; blocks.len()];
    let Ok(scope) = ProjectScope::new(project.digest()) else {
        return refused();
    };
    let budget = EvalBudget::new(
        Some(std::time::Instant::now() + READ_DEADLINE),
        Arc::default(),
    );
    let mut permitted = Vec::with_capacity(blocks.len());
    for chunk in blocks.chunks(MAX_ELIGIBILITY_CANDIDATES) {
        let candidates: Vec<EligibilityCandidate> = chunk
            .iter()
            .map(|block| EligibilityCandidate {
                object_id: block.object_id.clone(),
                source_revision: block.written_ms,
                artifact_digest: Some(block.handle.digest.clone()),
            })
            .collect();
        match store.judge_surface_eligibility_within_budget(
            &scope,
            ArtifactDestination::ConfiguredModel,
            Surface::ExplicitSearch,
            &candidates,
            &budget,
        ) {
            Ok(batch) if batch.verdicts.len() == chunk.len() => {
                permitted.extend(batch.verdicts.iter().map(|verdict| verdict.permits()));
            }
            _ => return refused(),
        }
    }
    permitted
}

/// The statements the block delivers now under `memory_budget_tokens` that pass the render check, each with the commit that first recorded it.
pub fn read_statements(
    store: &KernelStore,
    project: &ProjectBinding,
    memory_budget_tokens: f64,
) -> Result<Vec<DeliveredStatement>, KernelError> {
    Ok(delivered_rows(store, project, memory_budget_tokens)?
        .into_iter()
        .filter(|(row, _)| check_render(row.content.as_bytes(), None).is_ok())
        .map(|(row, decision)| DeliveredStatement {
            work: row.category == CURRENT_WORK_SECTION,
            stated_seq: memory_capture::stated_commit(&decision),
            row,
            decision,
        })
        .collect())
}

/// What a pass may write beside a change: whether capture's consent lets it record the user's sentence as a row of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Writes {
    /// `memory_enabled`, `auto_promote`, and `memory_auto_capture` all hold.
    pub record_sentences: bool,
}

/// The row a finding names as what changed its statement: a delivered row of the same session and message whose quotation holds the whole sentence, else a row the pass records for the sentence, approved when `approve`. `None` when no row of the message holds the sentence and the user's consent leaves capture off.
fn later_row(
    envelope: &mut kernel::Envelope<'_>,
    project: &ProjectBinding,
    statements: &[DeliveredStatement],
    read: &SessionRead,
    finding: &Finding,
    approve: bool,
    writes: Writes,
) -> Result<Option<String>, KernelError> {
    let target = &statements[finding.statement];
    let message = &read.messages[finding.message];
    let reuse = statements.iter().find(|statement| {
        source_message(statement, &read.session).as_deref() == Some(message.message_id.as_str())
            && memory_capture::capture_source(&statement.decision)
                .quote
                .is_some_and(|quote| quote.contains(finding.sentence.as_str()))
            && statement.decision.object_id != target.decision.object_id
    });
    if let Some(statement) = reuse {
        let live = envelope
            .object_state(&statement.decision.object_id)?
            .is_some_and(|state| state.object.invalidated_commit_seq.is_none());
        if live {
            return Ok(Some(statement.decision.object_id.clone()));
        }
    }
    if !writes.record_sentences {
        return Ok(None);
    }
    memory_capture::record_reviewed_sentence(
        envelope,
        project,
        &ReviewedSentence {
            session: &read.session,
            message: &message.message_id,
            text: &message.text,
            sentence: &finding.sentence,
            category: &target.decision.decision_kind,
            current_work: target.work,
            speaker: finding.speaker.recorded(),
        },
        approve,
    )
    .map(Some)
}

/// Whether every block of `message` is still live, normal or sensitive, and holds the bytes the pass read, checked inside the commit that writes from it: a session deleted or a block reclassified while the request waited grounds no change.
fn message_live(
    envelope: &kernel::Envelope<'_>,
    message: &SessionMessage,
) -> Result<bool, KernelError> {
    for (object_id, digest) in &message.blocks {
        let Some(state) = envelope.object_state(object_id)? else {
            return Ok(false);
        };
        if state.object.invalidated_commit_seq.is_some()
            || state.object.sensitivity == Sensitivity::Secret
            || state.artifact_digest.as_deref() != Some(digest.as_str())
        {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Writes one grounded finding in a commit of its own, keyed by the statement, the message, the effect, and the action, so a replayed pass writes nothing twice. The commit first checks that the message's blocks are live and that the statement is the row the pass read and one capture wrote; it writes the later row only after those checks. Returns the action written, or `None` when the stores left the statement as it was.
pub fn apply(
    store: &KernelStore,
    project: &ProjectBinding,
    statements: &[DeliveredStatement],
    read: &SessionRead,
    finding: &Finding,
    writes: Writes,
    now: i64,
) -> Result<Option<Action>, KernelError> {
    let target = &statements[finding.statement];
    let message = &read.messages[finding.message];
    let action = action(finding, message);
    let key = format!(
        "{}#{}#{}#{}#{action:?}",
        project.digest(),
        target.decision.object_id,
        message.object_id,
        finding.effect.as_str()
    );
    let request = format!("{key}\u{1f}{}", finding.sentence);
    let mut left_unchanged = false;
    let receipt = store.commit(
        intent(
            key,
            &request,
            "change detection found a later user message that changes a statement",
        ),
        |envelope| {
            if !message_live(envelope, message)?
                || !memory_capture::reviewed_target(envelope, project, &target.decision)?
                || (action == Action::End && !memory_capture::states_current_work(&target.decision))
            {
                return Ok(String::new());
            }
            let Some(later) = later_row(
                envelope,
                project,
                statements,
                read,
                finding,
                action == Action::Replace,
                writes,
            )?
            else {
                return Ok(String::new());
            };
            let change = |effect: &str| Change {
                effect: effect.to_string(),
                memory: later.clone(),
                session: read.session.clone(),
                message: message.message_id.clone(),
            };
            let written = match action {
                Action::Replace => memory_capture::retire_reviewed_statement(
                    envelope,
                    project,
                    &target.decision,
                    change("replaced"),
                    now,
                )?,
                Action::Retire => memory_capture::retire_reviewed_statement(
                    envelope,
                    project,
                    &target.decision,
                    change(finding.effect.as_str()),
                    now,
                )?,
                Action::End => memory_capture::end_reviewed_work(
                    envelope,
                    project,
                    &target.decision,
                    &later,
                    &read.session,
                    now,
                )?,
                Action::Contest => memory_capture::contest_reviewed_statement(
                    envelope,
                    project,
                    &target.decision,
                    &later,
                    now,
                )?,
            };
            // A target the rewrite left unchanged takes no later row either: the commit rolls back what it wrote.
            match written {
                Some(written) if !written.is_empty() => Ok(written),
                _ => {
                    left_unchanged = true;
                    Err(KernelError::Conflict)
                }
            }
        },
    );
    match receipt {
        Ok(receipt) => Ok((!receipt.result.is_empty()).then_some(action)),
        Err(KernelError::Conflict) if left_unchanged => Ok(None),
        Err(error) => Err(error),
    }
}

/// What one pass reads and writes for one project: when it runs, the user's memory budget it reads the block under, and whether capture's consent lets it record sentences.
#[derive(Debug, Clone, Copy)]
pub struct PassSettings {
    pub moment: Moment,
    pub memory_budget_tokens: f64,
    pub writes: Writes,
}

/// One pass for one project: reads one ended session's user messages and the delivered statements, asks `host` per chunk of statements, writes each grounded change, and records the pass's receipt once every request answered. Returns the outcome; zero requests when nothing was left to read.
pub async fn pass(
    store: &KernelStore,
    ledger: &memory_store::MemoryStore,
    project: &ProjectBinding,
    host: &HostExecution,
    memory: &DetectionMemory,
    settings: PassSettings,
    cancel: &CancellationToken,
) -> PassOutcome {
    let mut outcome = PassOutcome::default();
    let Ok(tip) = store.tip() else {
        return outcome;
    };
    if memory.quiet(project.digest(), tip) {
        return outcome;
    }
    let now_ms = crate::now_ms();
    if settings.moment == Moment::Idle && memory.still_waiting(project.digest(), tip, now_ms) {
        return outcome;
    }
    // Capture's statements of the session's messages are read beside them, so a pass waits, for at most CAPTURE_WAIT, until capture has settled the project's queue.
    match tokio::task::block_in_place(|| ledger.memory_capture_status(project.digest())) {
        Ok(status) if status.pending + status.prepared == 0 => {
            memory.capture_settled(project.digest())
        }
        Ok(_) => {
            if memory.capture_holds(project.digest(), now_ms) {
                return outcome;
            }
        }
        Err(error) => {
            eprintln!(
                "daemon: memory_reviewer change detection could not read capture's queue: {error}"
            );
            return outcome;
        }
    }
    let read = tokio::task::block_in_place(|| {
        let allowed = |blocks: &[Block]| egress_allowed(store, project, blocks);
        let read = match read_session(store, project, settings.moment, now_ms, &allowed)? {
            Reading::Read(read) => read,
            Reading::Nothing => return Ok(Err(true)),
            Reading::Waiting(until) => {
                memory.note_waiting(project.digest(), tip, until);
                return Ok(Err(false));
            }
            Reading::Cut => {
                memory.note_cut_scan(project.digest());
                return Ok(Err(false));
            }
        };
        let statements = read_statements(store, project, settings.memory_budget_tokens)?;
        let seqs: Vec<i64> = statements
            .iter()
            .map(|statement| statement.stated_seq)
            .collect();
        let recorded: HashMap<i64, i64> = store.commits_recorded_at(&seqs)?.into_iter().collect();
        Ok::<_, KernelError>(Ok((read, statements, recorded)))
    });
    let (read, statements, recorded) = match read {
        Ok(Ok(found)) => found,
        // Nothing to read at this tip; a waiting session is read once it is quiet, whatever the tip.
        Ok(Err(nothing)) => {
            if nothing {
                memory.note_quiet(project.digest(), tip);
            }
            return outcome;
        }
        Err(error) => {
            eprintln!(
                "daemon: memory_reviewer change detection could not read the project: {error}"
            );
            return outcome;
        }
    };
    if memory.backing_off(project.digest(), &read.session, &read.newest, now_ms) {
        return outcome;
    }
    let attempt = memory.failures(project.digest(), &read.session, &read.newest);
    let mut changed: HashSet<usize> = HashSet::new();
    let mut complete = true;
    let readable = read.messages.iter().any(|message| !message.covered);
    if readable && !statements.is_empty() {
        let (requests, omitted) = chunks(&statements);
        outcome.omitted = omitted;
        for (index, chunk) in requests.into_iter().enumerate() {
            if cancel.is_cancelled() {
                complete = false;
                break;
            }
            let prompt = request_json(&statements, &chunk, &read, &recorded);
            if check_render(prompt.as_bytes(), None).is_err() {
                continue;
            }
            let session = {
                let digest = format!(
                    "{:x}",
                    Sha256::digest(
                        format!(
                            "{}:{}:{}:{index}:{attempt}",
                            project.digest(),
                            read.session,
                            read.newest
                        )
                        .as_bytes()
                    )
                );
                format!("{CHILD_SESSION_PREFIX}{}", &digest[..16])
            };
            let mut accounting = ResponseAccounting::default();
            outcome.requests += 1;
            let answered = tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    complete = false;
                    break;
                }
                answered = host.complete(
                    &session,
                    (SYSTEM_TEXT, &prompt),
                    MAX_TOKENS,
                    tokio::time::Instant::now() + REQUEST_WAIT,
                    ResponseAllowance::FULL,
                    &mut accounting,
                ) => answered,
            };
            let text = match answered {
                // A response the provider stopped carries no answer and no model id: the request is settled with no change.
                Ok(text) if text.stop_reason == Some(StopReason::Refusal) => continue,
                Ok(text) if !host.answered_by_requested(text.model.as_deref()) => {
                    outcome.rejection = Some(Rejection::ModelMismatch {
                        reported: text
                            .model
                            .as_deref()
                            .map(|model| model.chars().take(MAX_REPORTED_MODEL_CHARS).collect()),
                        expected: host.expected_model().to_string(),
                    });
                    complete = false;
                    break;
                }
                Ok(text) if text.stop_reason == Some(StopReason::EndTurn) => text.text,
                Ok(_) => {
                    eprintln!(
                        "daemon: memory_reviewer change detection answer ended before its end"
                    );
                    complete = false;
                    continue;
                }
                Err(failure) => {
                    let rejection = match &failure.error {
                        SendError::AuthRequired => Some(Rejection::AuthRequired),
                        error => error.refused_credential().map(Rejection::Status),
                    };
                    if let Some(rejection) = rejection {
                        outcome.rejection = Some(rejection);
                        complete = false;
                        break;
                    }
                    eprintln!(
                        "daemon: memory_reviewer change detection request ended without an answer: {:?}",
                        failure.error
                    );
                    complete = false;
                    continue;
                }
            };
            let Some(mut findings) = parse_answer(&text, &chunk, read.messages.len()) else {
                eprintln!("daemon: memory_reviewer change detection answer is not the closed form");
                complete = false;
                continue;
            };
            outcome.findings += findings.len();
            // The latest message's change of a statement is the one written.
            findings.sort_by_key(|finding| std::cmp::Reverse(finding.message));
            if cancel.is_cancelled() {
                complete = false;
                break;
            }
            let now = crate::now_ms();
            for finding in findings {
                if !grounded(&finding, &statements, &read) {
                    outcome.ungrounded += 1;
                    continue;
                }
                if !changed.insert(finding.statement) {
                    continue;
                }
                match tokio::task::block_in_place(|| {
                    apply(
                        store,
                        project,
                        &statements,
                        &read,
                        &finding,
                        settings.writes,
                        now,
                    )
                }) {
                    Ok(Some(Action::Replace)) => outcome.replaced += 1,
                    Ok(Some(Action::Contest)) => outcome.contested += 1,
                    Ok(Some(Action::Retire)) => outcome.retired += 1,
                    Ok(Some(Action::End)) => outcome.ended += 1,
                    Ok(None) => {}
                    Err(error) => {
                        outcome.refused += 1;
                        eprintln!(
                            "daemon: memory_reviewer change detection left a change unwritten: {error}"
                        );
                    }
                }
            }
        }
    }
    let mut gave_up = false;
    if !complete && outcome.rejection.is_none() && !cancel.is_cancelled() {
        let failures = memory.note_failure(project.digest(), &read.session, &read.newest, now_ms);
        gave_up = failures >= MAX_ATTEMPTS;
    }
    // A pass whose requests all answered covers the session's messages up to its newest; one cut short is asked again after its wait, and the last failed attempt covers the session with no answer.
    if (complete || gave_up) && outcome.rejection.is_none() && !cancel.is_cancelled() {
        let covered = tokio::task::block_in_place(|| {
            store.commit(pass_intent(project, &read.session, &read.newest), |_| {
                Ok(format!("{outcome:?}"))
            })
        });
        match covered {
            Ok(_) => {
                outcome.covered = true;
                memory.clear_failures(project.digest());
            }
            Err(error) => eprintln!(
                "daemon: memory_reviewer change detection could not record its pass: {error}"
            ),
        }
    }
    let digest = format!("{:x}", Sha256::digest(read.session.as_bytes()));
    eprintln!(
        "daemon: memory_reviewer change detection session={} messages={} withheld={} statements={} omitted={} requests={} findings={} ungrounded={} replaced={} contested={} retired={} ended={} refused={} covered={}{}",
        &digest[..16],
        read.messages.len(),
        read.withheld,
        statements.len(),
        outcome.omitted,
        outcome.requests,
        outcome.findings,
        outcome.ungrounded,
        outcome.replaced,
        outcome.contested,
        outcome.retired,
        outcome.ended,
        outcome.refused,
        outcome.covered,
        if gave_up {
            " after its last attempt"
        } else {
            ""
        },
    );
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decision(object_id: &str, summary: &str, rationale: &str, seq: i64) -> DecisionRow {
        DecisionRow {
            decision_id: format!("dec_{object_id}"),
            object_id: object_id.to_string(),
            proposition_id: None,
            scope_id: None,
            anchor_id: None,
            evidence_id: None,
            decision_kind: "PROJECT_RULES".to_string(),
            payload: kernel::DecisionPayload {
                summary: summary.to_string(),
                rationale: rationale.to_string(),
            },
            created_commit_seq: seq,
            sensitivity: Sensitivity::Normal,
        }
    }

    /// A statement capture took from message `n1` of session `s`.
    fn statement(id: &str, text: &str, section: &str, seq: i64) -> DeliveredStatement {
        DeliveredStatement {
            row: CanonicalMemory {
                object_id: id.to_string(),
                category: section.to_string(),
                content: text.to_string(),
            },
            decision: decision(
                id,
                text,
                r#"{"source":"memory_capture","session":"s","message":"n1","quote":"Deploy from main."}"#,
                seq,
            ),
            work: section == CURRENT_WORK_SECTION,
            stated_seq: seq,
        }
    }

    fn message(id: &str, text: &str, seq: i64) -> SessionMessage {
        SessionMessage {
            message_id: id.to_string(),
            object_id: format!("srcdesc:{id}"),
            blocks: vec![(format!("srcdesc:{id}"), "digest".to_string())],
            written_ms: 1_700_000_000_000 + seq,
            published_seq: seq,
            text: text.to_string(),
            cut: false,
            covered: false,
        }
    }

    fn read(messages: Vec<SessionMessage>) -> SessionRead {
        SessionRead {
            session: "s".to_string(),
            messages,
            newest: "srcdesc:last".to_string(),
            withheld: 0,
        }
    }

    fn finding(statement: usize, effect: Effect, message: usize, sentence: &str) -> Finding {
        Finding {
            statement,
            effect,
            message,
            sentence: sentence.to_string(),
            speaker: Speaker::User,
            high: true,
        }
    }

    #[test]
    fn an_answer_names_statements_and_messages_by_their_aliases() {
        let answer = r#"{"changes":[
            {"statement":"s2","effect":"replaced","message":"m1","sentence":"Deploy from the release branch.","speaker":"user","confidence":"high"},
            {"statement":"s9","effect":"replaced","message":"m1","sentence":"x","speaker":"user","confidence":"high"},
            {"statement":"s1","effect":"renamed","message":"m1","sentence":"x","speaker":"user","confidence":"high"},
            {"statement":"s1","effect":"narrowed","message":"m3","sentence":"x","speaker":"user","confidence":"high"},
            {"statement":"s1","effect":"moot","message":"m2","sentence":"  ","speaker":"user","confidence":"high"},
            {"statement":"s1","effect":"ended","message":"m2","sentence":"That is done.","speaker":"other","confidence":"low"},
            {"statement":"s1","effect":"narrowed","message":"m2","sentence":"Only on weekdays.","confidence":"high"}
        ]}"#;
        let found = parse_answer(answer, &[0, 1], 2).expect("the closed form");
        assert_eq!(
            found,
            vec![
                Finding {
                    speaker: Speaker::User,
                    ..finding(1, Effect::Replaced, 0, "Deploy from the release branch.")
                },
                Finding {
                    speaker: Speaker::Other,
                    high: false,
                    ..finding(0, Effect::Ended, 1, "That is done.")
                },
                Finding {
                    speaker: Speaker::Unknown,
                    ..finding(0, Effect::Narrowed, 1, "Only on weekdays.")
                },
            ],
            "an alias outside the request, an unknown effect, and an empty sentence drop their change; a missing speaker reads as unknown"
        );
        let fenced = "``` json\n{\"changes\":[{\"statement\":\"s1\",\"effect\":\"moot\",\"message\":\"m1\",\"sentence\":\"Drop it.\",\"speaker\":\"user\",\"confidence\":\"high\"}]}\n```";
        assert_eq!(
            parse_answer(fenced, &[0], 1),
            Some(vec![finding(0, Effect::Moot, 0, "Drop it.")]),
            "a fenced answer reads as the object it holds"
        );
        assert_eq!(parse_answer("{\"changes\":[]}", &[0], 1), Some(Vec::new()));
        assert_eq!(
            parse_answer("no changes here", &[0], 1),
            None,
            "an answer that is not the object is told apart from an empty one"
        );
    }

    #[test]
    fn a_change_holds_only_on_new_words_of_a_later_message() {
        let statements = vec![
            statement(
                "mem_a",
                "User stated: Deploy from main.",
                "PROJECT_RULES",
                10,
            ),
            statement(
                "mem_w",
                "User stated: Next, add the export command.",
                CURRENT_WORK_SECTION,
                10,
            ),
        ];
        let mut covered = message("n3", "Deploy from tags.", 30);
        covered.covered = true;
        let session = read(vec![
            message("n0", "Deploy from staging.", 8),
            message("n1", "Deploy from main.", 12),
            message(
                "n2",
                "We ship from the release branch now. Deploy from the release branch.",
                20,
            ),
            covered,
        ]);
        assert!(grounded(
            &finding(0, Effect::Replaced, 2, "Deploy from the release branch."),
            &statements,
            &session
        ));
        assert!(
            !grounded(
                &finding(0, Effect::Replaced, 2, "Deploy from the release branches."),
                &statements,
                &session
            ),
            "words that are not in the message"
        );
        assert!(
            !grounded(
                &finding(0, Effect::Replaced, 0, "Deploy from staging."),
                &statements,
                &session
            ),
            "a message published and written before the statement's own message"
        );
        assert!(
            !grounded(
                &finding(0, Effect::Replaced, 1, "Deploy from main."),
                &statements,
                &session
            ),
            "the message the statement was taken from, published after the statement's commit"
        );
        assert!(
            !grounded(
                &finding(0, Effect::Replaced, 3, "Deploy from tags."),
                &statements,
                &session
            ),
            "a message an earlier pass covered"
        );
        assert!(
            !grounded(
                &finding(0, Effect::Ended, 2, "Deploy from the release branch."),
                &statements,
                &session
            ),
            "only work in progress ends"
        );
        assert!(grounded(
            &finding(1, Effect::Ended, 2, "We ship from the release branch now."),
            &statements,
            &session
        ));
        // A correction capture published before the statement's commit, written after the statement's message.
        let early = read(vec![
            message("n1", "Deploy from main.", 3),
            message("n5", "Deploy from tags.", 5),
        ]);
        assert!(
            grounded(
                &finding(0, Effect::Replaced, 1, "Deploy from tags."),
                &statements,
                &early
            ),
            "a message written after the statement's own message"
        );
    }

    #[test]
    fn a_replacement_is_written_only_on_the_users_own_whole_sentences_with_high_confidence() {
        let text = "We ship from the release branch now. A reviewer wrote: \"deploy from tags\".";
        let whole = message("n1", text, 1);
        let with = |sentence: &str, speaker, high, effect| Finding {
            speaker,
            high,
            ..finding(0, effect, 0, sentence)
        };
        let rule = "We ship from the release branch now.";
        assert_eq!(
            action(&with(rule, Speaker::User, true, Effect::Replaced), &whole),
            Action::Replace
        );
        for speaker in [Speaker::Other, Speaker::Unknown] {
            for (effect, label) in [
                (Effect::Replaced, "replaced"),
                (Effect::Moot, "moot"),
                (Effect::Ended, "ended"),
            ] {
                assert_eq!(
                    action(&with(rule, speaker, true, effect), &whole),
                    Action::Contest,
                    "{label} on words not read as the user's own ({speaker:?})"
                );
            }
        }
        assert_eq!(
            action(
                &with(
                    "ship from the release branch",
                    Speaker::User,
                    true,
                    Effect::Replaced
                ),
                &whole
            ),
            Action::Contest,
            "part of a sentence"
        );
        for (effect, label) in [
            (Effect::Replaced, "replaced"),
            (Effect::Moot, "moot"),
            (Effect::Ended, "ended"),
            (Effect::Narrowed, "narrowed"),
        ] {
            assert_eq!(
                action(&with(rule, Speaker::User, false, effect), &whole),
                Action::Contest,
                "{label} at low confidence"
            );
        }
        assert_eq!(
            action(&with(rule, Speaker::User, true, Effect::Narrowed), &whole),
            Action::Contest
        );
        assert_eq!(
            action(&with(rule, Speaker::User, true, Effect::Moot), &whole),
            Action::Retire
        );
        assert_eq!(
            action(&with(rule, Speaker::User, true, Effect::Ended), &whole),
            Action::End
        );
        // The message was cut where the cited words end, so they may continue past the cut.
        let mut cut = message("n2", "Use the release branch. Deploy from tags.", 2);
        cut.cut = true;
        assert_eq!(
            action(
                &with("Deploy from tags.", Speaker::User, true, Effect::Replaced),
                &cut
            ),
            Action::Contest,
            "words that end at the cut"
        );
        assert_eq!(
            action(
                &with(
                    "Use the release branch.",
                    Speaker::User,
                    true,
                    Effect::Replaced
                ),
                &cut
            ),
            Action::Replace,
            "a whole sentence before the cut"
        );
    }

    #[test]
    fn statements_past_the_requests_are_counted_out() {
        let big = "x".repeat(MAX_STATEMENT_BYTES / 2 + 1);
        let statements = vec![
            statement("mem_1", &big, "PROJECT_RULES", 1),
            statement("mem_2", "short", "NAMING", 1),
            statement("mem_3", &big, "PROJECT_RULES", 1),
        ];
        assert_eq!(chunks(&statements), (vec![vec![0], vec![2, 1]], 0));
        let small = vec![
            statement("mem_1", "a", "PROJECT_RULES", 1),
            statement("mem_2", "b", "NAMING", 1),
        ];
        assert_eq!(chunks(&small), (vec![vec![0, 1]], 0));
        let oversized = "y".repeat(MAX_STATEMENT_BYTES + 1);
        let mut many: Vec<DeliveredStatement> = (0..MAX_REQUESTS_PER_PASS + 1)
            .map(|index| statement(&format!("mem_{index}"), &big, "PROJECT_RULES", 1))
            .collect();
        many.insert(1, statement("mem_huge", &oversized, "PROJECT_RULES", 1));
        many.push(statement("mem_tail", "short", "PROJECT_RULES", 1));
        let (requests, omitted) = chunks(&many);
        assert_eq!(requests.len(), MAX_REQUESTS_PER_PASS);
        assert!(
            requests.iter().flatten().all(|&index| index != 1),
            "a statement larger than one request is left out"
        );
        assert!(
            requests.last().unwrap().contains(&(many.len() - 1)),
            "a statement that fits the last request still goes in it"
        );
        assert_eq!(
            omitted, 2,
            "the oversized statement and the one past the last request"
        );
    }

    #[test]
    fn a_request_lists_each_statement_with_its_section_day_and_source_message() {
        let statements = vec![
            statement(
                "mem_a",
                "User stated: Deploy from main.",
                "PROJECT_RULES",
                10,
            ),
            statement(
                "mem_w",
                "User stated: Next, add the export command.",
                CURRENT_WORK_SECTION,
                11,
            ),
        ];
        let session = read(vec![
            message("n1", "Deploy from main.", 9),
            message("n2", "Use tags.", 20),
        ]);
        let recorded = HashMap::from([(10, 1_700_000_000_000), (11, i64::MAX)]);
        let json: serde_json::Value =
            serde_json::from_str(&request_json(&statements, &[0, 1], &session, &recorded)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "statements": [
                    {"id": "s1", "section": "PROJECT_RULES", "recorded": "2023-11-14", "from": "m1",
                     "text": "User stated: Deploy from main."},
                    {"id": "s2", "section": CURRENT_WORK_SECTION, "recorded": "unknown day", "work": true, "from": "m1",
                     "text": "User stated: Next, add the export command."}
                ],
                "messages": [
                    {"id": "m1", "time": "2023-11-14 22:13", "text": "Deploy from main."},
                    {"id": "m2", "time": "2023-11-14 22:13", "text": "Use tags."}
                ]
            })
        );
        let other = SessionRead {
            session: "another".to_string(),
            ..read(vec![message("n1", "Deploy from main.", 9)])
        };
        let json: serde_json::Value =
            serde_json::from_str(&request_json(&statements, &[0], &other, &recorded)).unwrap();
        assert!(
            json["statements"][0].get("from").is_none(),
            "a message of another session with the same native id is not the statement's source"
        );
    }

    #[test]
    fn a_failed_attempt_waits_and_doubles_its_wait() {
        let memory = DetectionMemory::default();
        let now = 1_000_000;
        assert!(!memory.backing_off("p", "s", "n", now));
        assert_eq!(memory.note_failure("p", "s", "n", now), 1);
        let first = i64::try_from(RETRY_AFTER.as_millis()).unwrap();
        assert!(memory.backing_off("p", "s", "n", now + first - 1));
        assert!(!memory.backing_off("p", "s", "n", now + first));
        assert!(
            !memory.backing_off("p", "s", "newer", now + 1),
            "a newer message is read at once"
        );
        assert_eq!(memory.note_failure("p", "s", "n", now), 2);
        assert!(memory.backing_off("p", "s", "n", now + 2 * first - 1));
        assert_eq!(memory.failures("p", "s", "n"), 2);
        memory.clear_failures("p");
        assert_eq!(memory.failures("p", "s", "n"), 0);
    }
}

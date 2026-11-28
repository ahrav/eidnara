//! Change detection at the end of a session: one request to the small chat model that reads the latest session's user messages beside every statement the project's memory delivers, and names each statement a message changes.
//!
//! A later message that changes a standing statement rarely names it or repeats its words: the user restates the value, replaces one part, adds an exception, or moves on to other work. So the pass does not pick the statements a message may bear on; it lists them. The statements are the rows a new session of the project receives now (the block, bounded by the memory budget), and the messages are the user messages of the project's newest session, newest first within [`MAX_MESSAGE_BYTES`]. The model answers with the statements a message changes and how: replaced, narrowed, moot, or, for work in progress, ended, each with the message and its words.
//!
//! The daemon then checks each answer against the stores before it writes: the statement is one the request listed, the message is one the request listed and was published after the statement was recorded, and the words are copied from that message exactly. A change is written so it can be undone, and the changed row stays on explicit search with what changed it:
//! - replaced: the user's later sentence is recorded as the user's statement, approved for delivery when it reads as the user's own whole sentences, and the old statement is superseded by a labeled row whose rationale names the change. When the sentence fails that check, the old statement stays delivered and its line is marked contested by the sentence.
//! - narrowed: the statement stays delivered and its line is marked contested by the sentence, with both dates.
//! - moot: the statement is superseded by a labeled row that names the change.
//! - ended: the work in progress leaves the work section and stays on search as ended by the sentence.
//! - an answer the model gives with low confidence marks the statement contested at most.
//!
//! Each pass records a receipt for the newest user message it covered, so the same messages are read once; the next message of the session, or of a newer session, starts the next pass.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::time::Duration;

use kernel::source_identity::OccurrenceClass;
use kernel::{
    ArtifactHandle, CommitIntent, DecisionRow, KernelError, KernelStore, Sensitivity, SourceClass,
};
use memory_store::memory_reviewer_ledger::ResponseAllowance;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::broker::check_render;
use super::host_execution::{CHILD_SESSION_PREFIX, HostExecution};
use super::model_request::ResponseAccounting;
use super::model_response::StopReason;
use crate::canonical_memory::{CanonicalMemory, delivered_rows};
use crate::kernel_routes::ProjectBinding;
use crate::memory_capture::{self, Change, ReviewedSentence};
use crate::memory_render::CURRENT_WORK_SECTION;

/// The producer of every commit a pass makes.
pub const PRODUCER: &str = "eidnara-daemon/memory-reviewer-change-detection";
/// The user messages one request carries, newest first: about 6,000 tokens of prose, which holds a long session's typed messages and keeps the request inside a small model's reliable reading length beside the statements.
pub const MAX_MESSAGE_BYTES: usize = 24 * 1024;
/// One message's share of [`MAX_MESSAGE_BYTES`]: a pasted log or file stays a part of the session's messages and leaves room for the others. The head is kept, where a user's own words come first.
pub const MAX_ONE_MESSAGE_BYTES: usize = 6 * 1024;
/// The statements one request carries. The block a session receives is bounded by the memory budget, 4,000 tokens by default, so its statements fit one request; a larger budget splits them by section into requests of at most this many bytes, each with the same messages.
pub const MAX_STATEMENT_BYTES: usize = 24 * 1024;
/// Requests one pass sends at most: a block four times the default budget.
pub const MAX_REQUESTS_PER_PASS: usize = 4;
/// The answer's output cap. An answer lists only the statements that changed.
pub const MAX_TOKENS: u32 = 4096;
/// The longest a request waits for its answer.
const REQUEST_WAIT: Duration = Duration::from_secs(90);
/// Changes one answer may carry; past it the answer is read up to the bound.
const MAX_CHANGES_PER_ANSWER: usize = 32;
/// The longest sentence a change may cite; a user's statement is a few sentences.
const MAX_SENTENCE_BYTES: usize = 2048;
/// Message descriptors a pass examines to find the project's newest session: 32 pages of 1,024.
const MAX_SCANNED_DESCRIPTORS: usize = 32 * 1024;
const DESCRIPTOR_PAGE: NonZeroUsize = NonZeroUsize::new(1024).unwrap();
/// The memory budget the pass reads the block under: the daemon's default.
const MEMORY_BUDGET_TOKENS: f64 = crate::config::DEFAULT_MEMORY_BUDGET_TOKENS;

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

/// How long a session stays quiet before the worker's own loop reads it: a pause this long ends a working session in practice, and reading a session once it has ended reads each message once instead of once per turn.
pub const QUIET: Duration = Duration::from_secs(10 * 60);

/// What a pass remembers between passes for each project: the Kernel tip at which it last found nothing left to read, so an unchanged store is not scanned again.
#[derive(Default)]
pub struct DetectionMemory {
    quiet_at: Mutex<HashMap<String, i64>>,
}

impl DetectionMemory {
    const MAX_PROJECTS: usize = 4096;

    fn quiet(&self, project: &str, tip: i64) -> bool {
        self.quiet_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(project)
            == Some(&tip)
    }

    fn note_quiet(&self, project: &str, tip: i64) {
        let mut quiet = self
            .quiet_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if quiet.len() >= Self::MAX_PROJECTS && !quiet.contains_key(project) {
            quiet.clear();
        }
        quiet.insert(project.to_string(), tip);
    }
}

/// One user message of the session a pass reads.
#[derive(Debug, Clone)]
pub struct SessionMessage {
    /// The native message id.
    pub message_id: String,
    /// The descriptor of the message's first text block, whose evidence a change names.
    pub object_id: String,
    /// The harness's timestamp, in milliseconds.
    pub written_ms: i64,
    /// The commit that published the message's last text block.
    pub published_seq: i64,
    pub text: String,
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
    /// The newest user message of the session: the pass's receipt names it.
    pub newest: String,
    /// Messages left out for their sensitivity or the render check.
    pub withheld: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    Replaced,
    Narrowed,
    Moot,
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

/// One change of an answer, with its aliases resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub statement: usize,
    pub effect: Effect,
    pub message: usize,
    pub sentence: String,
    pub by_user: bool,
    pub high: bool,
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

/// The request's user message: the statements of `chunk` and the session's messages, as JSON in a fixed member order.
pub fn request_json(
    statements: &[DeliveredStatement],
    chunk: &[usize],
    messages: &[SessionMessage],
    recorded_ms: &HashMap<i64, i64>,
) -> String {
    let from = |statement: &DeliveredStatement| {
        let rationale = memory_capture::capture_rationale_message(&statement.decision)?;
        messages
            .iter()
            .position(|message| message.message_id == rationale)
            .map(message_alias)
    };
    let listed: Vec<serde_json::Value> = chunk
        .iter()
        .map(|&index| {
            let statement = &statements[index];
            let mut entry = serde_json::Map::new();
            entry.insert("id".into(), statement_alias(index).into());
            entry.insert("section".into(), statement.row.category.clone().into());
            let recorded = recorded_ms.get(&statement.stated_seq).map_or_else(
                || "unknown day".to_string(),
                |ms| utc_minute(*ms)[..10].to_string(),
            );
            entry.insert("recorded".into(), recorded.into());
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
    let written: Vec<serde_json::Value> = messages
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

/// The statements of `statements` split into requests: by section, in the block's order, each request at most [`MAX_STATEMENT_BYTES`] of statement text and at most [`MAX_REQUESTS_PER_PASS`] requests. A statement past the last request is left for the next pass's block.
pub fn chunks(statements: &[DeliveredStatement]) -> Vec<Vec<usize>> {
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
    let mut bytes = 0;
    for section in order {
        for &index in &sections[section] {
            let size = statements[index].row.content.len();
            if bytes + size > MAX_STATEMENT_BYTES && !out.last().is_some_and(Vec::is_empty) {
                if out.len() == MAX_REQUESTS_PER_PASS {
                    return out;
                }
                out.push(Vec::new());
                bytes = 0;
            }
            if size <= MAX_STATEMENT_BYTES {
                out.last_mut().expect("one request").push(index);
                bytes += size;
            }
        }
    }
    out.retain(|chunk| !chunk.is_empty());
    out
}

/// The changes `text` names, read in the closed form of [`SYSTEM_TEXT`]: an alias outside the request, an effect or speaker outside its words, or an empty sentence drops that change, and an answer that is not the object drops them all. A fenced object reads as the object.
pub fn parse_answer(text: &str, chunk: &[usize], messages: usize) -> Vec<Finding> {
    let trimmed = text.trim();
    let body = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|rest| rest.strip_suffix("```"))
        .map_or(trimmed, str::trim);
    let Ok(answer) = serde_json::from_str::<RawAnswer>(body) else {
        return Vec::new();
    };
    let statement_of = |alias: &str| {
        chunk
            .iter()
            .copied()
            .find(|&index| statement_alias(index) == alias)
    };
    let message_of = |alias: &str| (0..messages).find(|&index| message_alias(index) == alias);
    answer
        .changes
        .into_iter()
        .take(MAX_CHANGES_PER_ANSWER)
        .filter_map(|value| {
            let raw: RawChange = serde_json::from_value(value).ok()?;
            let by_user = match raw.speaker.as_deref() {
                Some("user") => true,
                Some("other") | None => false,
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
                by_user,
                high,
            })
        })
        .collect()
}

/// Whether `finding` holds against the stores: the words stand in the message, the message was published after the statement was recorded, and the effect fits the statement (only work in progress ends).
pub fn grounded(
    finding: &Finding,
    statements: &[DeliveredStatement],
    messages: &[SessionMessage],
) -> bool {
    let statement = &statements[finding.statement];
    let message = &messages[finding.message];
    message.text.contains(&finding.sentence)
        && message.published_seq > statement.stated_seq
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

/// The action for a grounded finding: the effect, held to a contest when the model's confidence is low, and a replacement held to a contest when the sentence is not the user's own whole sentences.
pub fn action(finding: &Finding, message_text: &str) -> Action {
    if !finding.high {
        return Action::Contest;
    }
    match finding.effect {
        Effect::Narrowed => Action::Contest,
        Effect::Moot => Action::Retire,
        Effect::Ended => Action::End,
        Effect::Replaced
            if finding.by_user
                && memory_capture::states_whole_sentences(message_text, &finding.sentence) =>
        {
            Action::Replace
        }
        Effect::Replaced => Action::Contest,
    }
}

/// Counts of one pass's outcome, for its log line.
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
struct Block {
    object_id: String,
    session: String,
    message_id: String,
    block_index: u64,
    written_ms: i64,
    published_seq: i64,
    sensitivity: Sensitivity,
    handle: ArtifactHandle,
    span: Option<(u64, u64)>,
}

/// The session a pass reads, with its user messages up to [`MAX_MESSAGE_BYTES`]: at a session's end, the project's newest session; on the worker's loop, the newest session once it has been quiet for [`QUIET`], and before then the session before it, which the newer one ended. [`Reading::Nothing`] when that session's newest user message is covered or the project holds no user message; [`Reading::Waiting`] when the newest session is still going and nothing else waits.
pub fn read_session(
    store: &KernelStore,
    project: &ProjectBinding,
    moment: Moment,
    now_ms: i64,
) -> Result<Reading, KernelError> {
    let quiet_ms = i64::try_from(QUIET.as_millis()).unwrap_or(i64::MAX);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let (_, found) = store.preview(deadline, |preview| {
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
            scanned += DESCRIPTOR_PAGE.get();
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
                    block_index: block.parse().unwrap_or(0),
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
                Some(next) if scanned < MAX_SCANNED_DESCRIPTORS => after = Some(next),
                _ => break,
            }
        }
        let Some(newest) = blocks
            .iter()
            .max_by_key(|block| (block.written_ms, block.published_seq))
        else {
            return Ok(None);
        };
        // The newest session is read at its end or once it is quiet; on the worker's loop before then, the session before it, which a newer one has ended, is read when it holds messages no pass covered.
        let newest_session = newest.session.clone();
        let newest_quiet = now_ms.saturating_sub(newest.written_ms) >= quiet_ms;
        let previous_session = blocks
            .iter()
            .filter(|block| block.session != newest_session)
            .max_by_key(|block| (block.written_ms, block.published_seq))
            .map(|block| block.session.clone());
        let session = match (moment, newest_quiet, previous_session) {
            (Moment::SessionEnd, _, _) | (Moment::Idle, true, _) => newest_session.clone(),
            (Moment::Idle, false, Some(previous)) => previous,
            (Moment::Idle, false, None) => return Ok(Some(Err(()))),
        };
        // The newest session waits for its end when the session the loop reads in its place has nothing left.
        let waiting = if session == newest_session {
            None
        } else {
            Some(Err(()))
        };
        // The session's user blocks: the writer is the source class the Kernel admitted each block under.
        let mut mine = Vec::new();
        for block in blocks.into_iter().filter(|block| block.session == session) {
            let user = preview
                .subject_admission(&block.object_id)?
                .is_some_and(|(prior, _)| prior.source_class == SourceClass::ExplicitUser);
            if user {
                mine.push(block);
            }
        }
        if mine.is_empty() {
            return Ok(waiting);
        }
        mine.sort_by(|left, right| {
            (left.written_ms, &left.message_id, left.block_index).cmp(&(
                right.written_ms,
                &right.message_id,
                right.block_index,
            ))
        });
        let newest = mine
            .last()
            .map(|block| block.object_id.clone())
            .unwrap_or_default();
        if preview
            .stored_receipt(pass_intent(project, &session, &newest))?
            .is_some()
        {
            return Ok(waiting);
        }
        Ok(Some(Ok((session, newest, mine))))
    })?;
    let (session, newest, blocks) = match found {
        None => return Ok(Reading::Nothing),
        Some(Err(())) => return Ok(Reading::Waiting),
        Some(Ok(found)) => found,
    };
    let (messages, withheld) = messages_of(store, blocks);
    let mut kept = Vec::new();
    let mut bytes = 0;
    for mut message in messages.into_iter().rev() {
        if message.text.len() > MAX_ONE_MESSAGE_BYTES {
            let cut = message.text.floor_char_boundary(MAX_ONE_MESSAGE_BYTES);
            message.text.truncate(cut);
        }
        if bytes + message.text.len() > MAX_MESSAGE_BYTES {
            break;
        }
        bytes += message.text.len();
        kept.push(message);
    }
    kept.reverse();
    Ok(Reading::Read(SessionRead {
        session,
        messages: kept,
        newest,
        withheld,
    }))
}

/// What a pass found to read.
#[derive(Debug)]
pub enum Reading {
    Read(SessionRead),
    /// No session holds a user message the pass has not covered.
    Nothing,
    /// The newest session is not quiet yet and no earlier session waits; a later pass reads it.
    Waiting,
}

/// The messages `blocks` hold, oldest first: the blocks of one message joined in block order, a message left out when a block of it is `Secret`, unreadable, not UTF-8, or fails the render check. Returns the messages and how many were left out.
fn messages_of(store: &KernelStore, blocks: Vec<Block>) -> (Vec<SessionMessage>, usize) {
    let mut messages: Vec<SessionMessage> = Vec::new();
    let mut withheld_messages: HashSet<String> = HashSet::new();
    for block in blocks {
        if block.sensitivity == Sensitivity::Secret {
            withheld_messages.insert(block.message_id.clone());
            continue;
        }
        let Ok(bytes) = store.read_artifact(&block.handle) else {
            withheld_messages.insert(block.message_id.clone());
            continue;
        };
        let bytes = match block.span {
            Some((start, end)) => {
                let (Ok(start), Ok(end)) = (usize::try_from(start), usize::try_from(end)) else {
                    withheld_messages.insert(block.message_id.clone());
                    continue;
                };
                bytes
                    .get(start..end)
                    .map(<[u8]>::to_vec)
                    .unwrap_or_default()
            }
            None => bytes,
        };
        let Ok(text) = String::from_utf8(bytes) else {
            withheld_messages.insert(block.message_id.clone());
            continue;
        };
        match messages.last_mut() {
            Some(last) if last.message_id == block.message_id => {
                last.text.push('\n');
                last.text.push_str(&text);
                last.published_seq = last.published_seq.max(block.published_seq);
            }
            _ => messages.push(SessionMessage {
                message_id: block.message_id,
                object_id: block.object_id,
                written_ms: block.written_ms,
                published_seq: block.published_seq,
                text,
            }),
        }
    }
    let mut withheld = 0;
    messages.retain(|message| {
        let keep = !withheld_messages.contains(&message.message_id)
            && !message.text.trim().is_empty()
            && check_render(message.text.as_bytes(), None).is_ok();
        if !keep {
            withheld += 1;
        }
        keep
    });
    (messages, withheld)
}

/// The statements the block delivers now that pass the render check, each with the commit that first recorded it.
pub fn read_statements(
    store: &KernelStore,
    project: &ProjectBinding,
) -> Result<Vec<DeliveredStatement>, KernelError> {
    Ok(delivered_rows(store, project, MEMORY_BUDGET_TOKENS)?
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

/// The row a finding names as what changed its statement: a delivered row of the same message whose quotation holds the sentence or is held by it, else a row the pass records for the sentence, approved when `approve`.
fn later_row(
    envelope: &mut kernel::Envelope<'_>,
    project: &ProjectBinding,
    statements: &[DeliveredStatement],
    session: &str,
    message: &SessionMessage,
    finding: &Finding,
    approve: bool,
) -> Result<String, KernelError> {
    let target = &statements[finding.statement];
    let reuse = statements.iter().find(|statement| {
        memory_capture::capture_rationale_message(&statement.decision).as_deref()
            == Some(message.message_id.as_str())
            && memory_capture::capture_rationale_quote(&statement.decision).is_some_and(|quote| {
                quote.contains(finding.sentence.as_str())
                    || finding.sentence.contains(quote.as_str())
            })
            && statement.decision.object_id != target.decision.object_id
    });
    if let Some(statement) = reuse {
        return Ok(statement.decision.object_id.clone());
    }
    memory_capture::record_reviewed_sentence(
        envelope,
        project,
        &ReviewedSentence {
            session,
            message: &message.message_id,
            text: &message.text,
            sentence: &finding.sentence,
            category: &target.decision.decision_kind,
            current_work: target.work,
        },
        approve,
    )
}

/// Writes one grounded finding in a commit of its own, keyed by the statement, the message and the effect, so a replayed pass writes nothing twice. Returns the action written, or `None` when the stores left the statement as it was.
pub fn apply(
    store: &KernelStore,
    project: &ProjectBinding,
    statements: &[DeliveredStatement],
    read: &SessionRead,
    finding: &Finding,
    now: i64,
) -> Result<Option<Action>, KernelError> {
    let target = &statements[finding.statement];
    let message = &read.messages[finding.message];
    let action = action(finding, &message.text);
    let key = format!(
        "{}#{}#{}#{}",
        project.digest(),
        target.decision.object_id,
        message.object_id,
        finding.effect.as_str()
    );
    let request = format!("{key}\u{1f}{}\u{1f}{action:?}", finding.sentence);
    let receipt = store.commit(
        intent(
            key,
            &request,
            "change detection found a later user message that changes a statement",
        ),
        |envelope| {
            let written = match action {
                Action::Replace => {
                    let later = later_row(
                        envelope,
                        project,
                        statements,
                        &read.session,
                        message,
                        finding,
                        true,
                    )?;
                    memory_capture::retire_reviewed_statement(
                        envelope,
                        project,
                        &target.decision,
                        Change {
                            effect: "replaced".to_string(),
                            memory: Some(later),
                            session: read.session.clone(),
                            message: message.message_id.clone(),
                        },
                        now,
                    )?
                }
                Action::Retire => {
                    let later = later_row(
                        envelope,
                        project,
                        statements,
                        &read.session,
                        message,
                        finding,
                        false,
                    )?;
                    memory_capture::retire_reviewed_statement(
                        envelope,
                        project,
                        &target.decision,
                        Change {
                            effect: finding.effect.as_str().to_string(),
                            memory: Some(later),
                            session: read.session.clone(),
                            message: message.message_id.clone(),
                        },
                        now,
                    )?
                }
                Action::End => {
                    let later = later_row(
                        envelope,
                        project,
                        statements,
                        &read.session,
                        message,
                        finding,
                        false,
                    )?;
                    memory_capture::end_reviewed_work(
                        envelope,
                        project,
                        &target.decision,
                        &later,
                        &read.session,
                        now,
                    )?
                }
                Action::Contest => {
                    let later = later_row(
                        envelope,
                        project,
                        statements,
                        &read.session,
                        message,
                        finding,
                        false,
                    )?;
                    memory_capture::contest_reviewed_statement(
                        envelope,
                        project,
                        &target.decision,
                        &later,
                        now,
                    )?
                }
            };
            Ok(written.unwrap_or_default())
        },
    )?;
    Ok((!receipt.result.is_empty()).then_some(action))
}

/// One pass for one project: reads the newest session's uncovered user messages and the delivered statements, asks `host` per chunk of statements, writes each grounded change, and records the pass's receipt. Returns the outcome; zero requests when nothing was left to read.
pub async fn pass(
    store: &KernelStore,
    ledger: &memory_store::MemoryStore,
    project_root: &std::path::Path,
    host: &HostExecution,
    memory: &DetectionMemory,
    moment: Moment,
    cancel: &CancellationToken,
) -> PassOutcome {
    let project = ProjectBinding::new(project_root);
    let mut outcome = PassOutcome::default();
    let Ok(tip) = store.tip() else {
        return outcome;
    };
    if memory.quiet(project.digest(), tip) {
        return outcome;
    }
    // Capture's statements of the session's messages are read beside them, so a pass waits until capture has settled the project's queue.
    match ledger.memory_capture_status(project.digest()) {
        Ok(status) if status.pending + status.prepared == 0 => {}
        _ => return outcome,
    }
    let read = tokio::task::block_in_place(|| {
        let read = match read_session(store, &project, moment, crate::now_ms())? {
            Reading::Read(read) => read,
            Reading::Nothing => return Ok(Err(true)),
            Reading::Waiting => return Ok(Err(false)),
        };
        let statements = read_statements(store, &project)?;
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
    let mut changed: HashSet<usize> = HashSet::new();
    let mut complete = true;
    if !read.messages.is_empty() && !statements.is_empty() {
        for (index, chunk) in chunks(&statements).into_iter().enumerate() {
            if cancel.is_cancelled() {
                complete = false;
                break;
            }
            let prompt = request_json(&statements, &chunk, &read.messages, &recorded);
            if check_render(prompt.as_bytes(), None).is_err() {
                continue;
            }
            let session = {
                let digest = format!(
                    "{:x}",
                    Sha256::digest(
                        format!(
                            "{}:{}:{}:{index}",
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
            let answered = host
                .complete(
                    &session,
                    (SYSTEM_TEXT, &prompt),
                    MAX_TOKENS,
                    tokio::time::Instant::now() + REQUEST_WAIT,
                    ResponseAllowance::FULL,
                    &mut accounting,
                )
                .await;
            let text = match answered {
                Ok(text)
                    if text.stop_reason == Some(StopReason::EndTurn)
                        && host.answered_by_requested(text.model.as_deref()) =>
                {
                    text.text
                }
                Ok(_) => continue,
                Err(failure) => {
                    eprintln!(
                        "daemon: memory_reviewer change detection request ended without an answer: {:?}",
                        failure.error
                    );
                    complete = false;
                    continue;
                }
            };
            let findings = parse_answer(&text, &chunk, read.messages.len());
            outcome.findings += findings.len();
            let now = crate::now_ms();
            for finding in findings {
                if !grounded(&finding, &statements, &read.messages) {
                    outcome.ungrounded += 1;
                    continue;
                }
                if !changed.insert(finding.statement) {
                    continue;
                }
                match tokio::task::block_in_place(|| {
                    apply(store, &project, &statements, &read, &finding, now)
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
    // A pass whose requests all answered covers the session's messages up to its newest; one cut short is asked again by the next pass.
    if complete {
        let covered = tokio::task::block_in_place(|| {
            store.commit(pass_intent(&project, &read.session, &read.newest), |_| {
                Ok(format!("{outcome:?}"))
            })
        });
        if let Err(error) = covered {
            eprintln!(
                "daemon: memory_reviewer change detection could not record its pass: {error}"
            );
        }
    }
    let digest = format!("{:x}", Sha256::digest(read.session.as_bytes()));
    eprintln!(
        "daemon: memory_reviewer change detection session={} messages={} withheld={} statements={} requests={} findings={} ungrounded={} replaced={} contested={} retired={} ended={} refused={}",
        &digest[..16],
        read.messages.len(),
        read.withheld,
        statements.len(),
        outcome.requests,
        outcome.findings,
        outcome.ungrounded,
        outcome.replaced,
        outcome.contested,
        outcome.retired,
        outcome.ended,
        outcome.refused,
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
                r#"{"source":"memory_capture","session":"s","message":"n1"}"#,
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
            written_ms: 1_700_000_000_000 + seq,
            published_seq: seq,
            text: text.to_string(),
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
            {"statement":"s1","effect":"ended","message":"m2","sentence":"That is done.","speaker":"other","confidence":"low"}
        ]}"#;
        let found = parse_answer(answer, &[0, 1], 2);
        assert_eq!(
            found,
            vec![
                Finding {
                    statement: 1,
                    effect: Effect::Replaced,
                    message: 0,
                    sentence: "Deploy from the release branch.".to_string(),
                    by_user: true,
                    high: true,
                },
                Finding {
                    statement: 0,
                    effect: Effect::Ended,
                    message: 1,
                    sentence: "That is done.".to_string(),
                    by_user: false,
                    high: false,
                },
            ],
            "an alias outside the request, an unknown effect, and an empty sentence drop their change"
        );
        assert_eq!(
            parse_answer("```json\n{\"changes\":[]}\n```", &[0], 1),
            Vec::new()
        );
        assert_eq!(parse_answer("no changes here", &[0], 1), Vec::new());
    }

    #[test]
    fn a_change_holds_only_on_words_of_a_message_published_after_the_statement() {
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
        let messages = vec![
            message("n0", "Deploy from main.", 8),
            message(
                "n1",
                "We ship from the release branch now. Deploy from the release branch.",
                20,
            ),
        ];
        let finding = |statement, effect, message, sentence: &str| Finding {
            statement,
            effect,
            message,
            sentence: sentence.to_string(),
            by_user: true,
            high: true,
        };
        assert!(grounded(
            &finding(0, Effect::Replaced, 1, "Deploy from the release branch."),
            &statements,
            &messages
        ));
        assert!(
            !grounded(
                &finding(0, Effect::Replaced, 1, "Deploy from the release branches."),
                &statements,
                &messages
            ),
            "words that are not in the message"
        );
        assert!(
            !grounded(
                &finding(0, Effect::Replaced, 0, "Deploy from main."),
                &statements,
                &messages
            ),
            "a message published before the statement was recorded"
        );
        assert!(
            !grounded(
                &finding(0, Effect::Ended, 1, "Deploy from the release branch."),
                &statements,
                &messages
            ),
            "only work in progress ends"
        );
        assert!(grounded(
            &finding(1, Effect::Ended, 1, "We ship from the release branch now."),
            &statements,
            &messages
        ));
    }

    #[test]
    fn a_replacement_is_written_only_on_the_users_own_whole_sentences_with_high_confidence() {
        let text = "We ship from the release branch now. A reviewer wrote: \"deploy from tags\".";
        let finding = |sentence: &str, by_user, high, effect| Finding {
            statement: 0,
            effect,
            message: 0,
            sentence: sentence.to_string(),
            by_user,
            high,
        };
        assert_eq!(
            action(
                &finding(
                    "We ship from the release branch now.",
                    true,
                    true,
                    Effect::Replaced
                ),
                text
            ),
            Action::Replace
        );
        assert_eq!(
            action(
                &finding(
                    "We ship from the release branch now.",
                    false,
                    true,
                    Effect::Replaced
                ),
                text
            ),
            Action::Contest,
            "words the user passes on from another source"
        );
        assert_eq!(
            action(
                &finding("ship from the release branch", true, true, Effect::Replaced),
                text
            ),
            Action::Contest,
            "part of a sentence"
        );
        assert_eq!(
            action(
                &finding(
                    "We ship from the release branch now.",
                    true,
                    false,
                    Effect::Replaced
                ),
                text
            ),
            Action::Contest,
            "low confidence"
        );
        assert_eq!(
            action(
                &finding(
                    "We ship from the release branch now.",
                    true,
                    true,
                    Effect::Narrowed
                ),
                text
            ),
            Action::Contest
        );
        assert_eq!(
            action(
                &finding(
                    "We ship from the release branch now.",
                    true,
                    true,
                    Effect::Moot
                ),
                text
            ),
            Action::Retire
        );
        assert_eq!(
            action(
                &finding(
                    "We ship from the release branch now.",
                    true,
                    true,
                    Effect::Ended
                ),
                text
            ),
            Action::End
        );
    }

    #[test]
    fn statements_past_one_request_split_by_section() {
        let big = "x".repeat(MAX_STATEMENT_BYTES / 2 + 1);
        let statements = vec![
            statement("mem_1", &big, "PROJECT_RULES", 1),
            statement("mem_2", "short", "NAMING", 1),
            statement("mem_3", &big, "PROJECT_RULES", 1),
        ];
        assert_eq!(chunks(&statements), vec![vec![0], vec![2, 1]]);
        let small = vec![
            statement("mem_1", "a", "PROJECT_RULES", 1),
            statement("mem_2", "b", "NAMING", 1),
        ];
        assert_eq!(chunks(&small), vec![vec![0, 1]]);
    }

    #[test]
    fn a_request_lists_each_statement_with_its_section_day_and_source_message() {
        let statements = vec![statement(
            "mem_a",
            "User stated: Deploy from main.",
            "PROJECT_RULES",
            10,
        )];
        let messages = vec![
            message("n1", "Deploy from main.", 9),
            message("n2", "Use tags.", 20),
        ];
        let recorded = HashMap::from([(10, 1_700_000_000_000)]);
        let json: serde_json::Value =
            serde_json::from_str(&request_json(&statements, &[0], &messages, &recorded)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "statements": [{"id": "s1", "section": "PROJECT_RULES", "recorded": "2023-11-14", "from": "m1",
                                "text": "User stated: Deploy from main."}],
                "messages": [
                    {"id": "m1", "time": "2023-11-14 22:13", "text": "Deploy from main."},
                    {"id": "m2", "time": "2023-11-14 22:13", "text": "Use tags."}
                ]
            })
        );
    }
}

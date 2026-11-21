//! Records of failed commands, written from tool results without a model
//! call.
//!
//! A later session is asked what went wrong the last time a command failed.
//! Capture reads message text, and the error a command printed stands in its
//! tool result. Each transform pass carries the window's tool calls and
//! results, so the daemon reads them there: a result of a tool call that
//! carries a command line, when the harness marks it as an error, when it
//! states a nonzero exit code, or when its output holds failure lines, becomes
//! one labeled memory row of category [`FAILURE_CATEGORY`]. The row holds the
//! command, the outcome the result states, and the output's failure lines. It
//! is served on explicit search and reaches no automatic surface: the block
//! renders positive categories only.
//!
//! One row is live per command of a project. A later failure of the same
//! command supersedes the row, and a later run that reports no failure
//! rewrites it as resolved, so search shows the last failure and whether a
//! later run passed.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use memory_store::{BlockKind, OutputKind, ResultBlockKind};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{CAPTURE_SOURCE_KIND, COMMIT_WAIT, MAX_CAPTURE_MEMORY_BYTES, capture_enabled};
use crate::kernel_routes::{self, ProjectBinding};
use crate::{HandlerCore, SessionBinding, now_ms};

/// The source lineage of every failure record.
pub(crate) const FAILURE_SOURCE_ID: &str = "tool_failure";
/// The category of a failure record. The block renders positive categories,
/// and this is none of them, so a record is served on explicit search alone.
pub(crate) const FAILURE_CATEGORY: &str = "TOOL_FAILURE";

/// Lines that state an error kept from one result, in output order. A
/// compiler or a test runner lists several, and the first ones name the
/// failure.
const MAX_ERROR_LINES: usize = 8;
/// Closing lines kept from one result: a failing command states its outcome
/// last.
const MAX_CLOSING_LINES: usize = 3;
/// One kept line, in bytes. A longer line is cut and ends in `…`.
const MAX_LINE_BYTES: usize = 240;
/// The command as shown in the record, in bytes.
const MAX_COMMAND_BYTES: usize = 512;
/// The bytes of one result read for its outcome, taken from its end: the
/// exit code and the summary of a failing run stand in the closing lines.
const MAX_RESULT_BYTES: usize = 64 * 1024;
/// Records one session writes. A session that fails more commands than this
/// keeps the records it has.
const MAX_RECORDS_PER_SESSION: usize = 64;
/// Tool calls one session's memo remembers.
const MEMO_CALLS_PER_SESSION: usize = 4096;
const MEMO_SESSIONS: usize = 256;
/// The line a resolved record ends with.
const RESOLVED_LINE: &str = "\nResolved: a later run of this command reported no failure.";

/// What one tool result says about its command.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    /// The result reports no failure.
    Passed,
    Failed {
        /// The exit code the result states.
        exit_code: Option<i64>,
        /// The harness marked the result as an error.
        flagged: bool,
        /// The failure lines and closing lines of the output.
        lines: Vec<String>,
    },
}

/// One command result of the window that the session's memo has not seen.
struct Observed {
    call: String,
    tool: String,
    command: String,
    key: String,
    at_ms: i64,
    /// The result's place in its session's window.
    place: u64,
    outcome: Outcome,
}

/// The live record of one command.
#[derive(Clone)]
struct Head {
    id: String,
    revision: i64,
    resolved: bool,
    at_ms: i64,
    summary: String,
    rationale: serde_json::Map<String, Value>,
}

/// Tool calls already judged, by session, and the live record of each
/// command, by project. Both are rebuilt from the window and the Kernel after
/// a restart: a record's id derives from its tool call, so a call judged
/// again writes nothing.
#[derive(Default)]
pub(crate) struct FailureMemo {
    seen: HashMap<String, (HashSet<String>, usize)>,
    heads: HashMap<String, HashMap<String, Head>>,
}

impl FailureMemo {
    fn has_seen(&self, session: &str, call: &str) -> bool {
        self.seen
            .get(session)
            .is_some_and(|(calls, _)| calls.contains(call))
    }

    fn records(&self, session: &str) -> usize {
        self.seen.get(session).map_or(0, |(_, records)| *records)
    }

    fn learn(&mut self, session: &str, call: &str, recorded: bool) {
        if !self.seen.contains_key(session) && self.seen.len() >= MEMO_SESSIONS {
            self.seen.clear();
        }
        let (calls, records) = self.seen.entry(session.to_owned()).or_default();
        if calls.len() >= MEMO_CALLS_PER_SESSION {
            calls.clear();
        }
        calls.insert(call.to_owned());
        *records += usize::from(recorded);
    }

    pub(crate) fn forget_session(&mut self, session: &str) {
        self.seen.remove(session);
    }
}

/// The command line a tool call carries: the string under `command` or
/// `cmd`, or an array of strings there joined by spaces. A shell tool of any
/// harness and any name carries one.
fn command_of(input: &Value) -> Option<String> {
    let field = input.get("command").or_else(|| input.get("cmd"))?;
    let command = match field {
        Value::String(text) => text.clone(),
        Value::Array(words) => words
            .iter()
            .map(Value::as_str)
            .collect::<Option<Vec<_>>>()?
            .join(" "),
        _ => return None,
    };
    (!command.trim().is_empty()).then_some(command)
}

/// The command without what varies between runs of one operation: leading
/// `cd <dir> &&` steps, leading `NAME=value` assignments, and the width of
/// whitespace runs.
fn normalized(command: &str) -> String {
    let mut words: Vec<&str> = command.split_whitespace().collect();
    loop {
        match words.as_slice() {
            ["cd", _, "&&" | ";", ..] => {
                words.drain(..3);
            }
            [first, ..]
                if first.split_once('=').is_some_and(|(name, _)| {
                    !name.is_empty()
                        && name
                            .chars()
                            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
                        && !name.starts_with(|ch: char| ch.is_ascii_digit())
                }) && words.len() > 1 =>
            {
                words.remove(0);
            }
            _ => break,
        }
    }
    words.join(" ")
}

/// The key of a command's live record: the normalized command as the record
/// stores it, redacted and within [`MAX_COMMAND_BYTES`]. The key is plain
/// text, so the Kernel's redaction of stored fields leaves it as written and
/// a rebuilt memo reads back the key it wrote.
fn command_key(command: &str) -> String {
    let redacted = context_core::redaction::redact_durable_text(&normalized(command)).text;
    let mut end = redacted.len().min(MAX_COMMAND_BYTES);
    while !redacted.is_char_boundary(end) {
        end -= 1;
    }
    redacted[..end].to_owned()
}

/// The text of a tool output and whether the harness marked it as an error.
fn output_text(output: &OutputKind) -> Option<(String, bool)> {
    let joined = |blocks: &[memory_store::ResultBlock]| {
        blocks
            .iter()
            .filter_map(|block| match &block.kind {
                ResultBlockKind::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    Some(match output {
        OutputKind::Text { text } => (text.clone(), false),
        OutputKind::ErrorText { text } => (text.clone(), true),
        OutputKind::Content { blocks } => (joined(blocks), false),
        OutputKind::ErrorContent { blocks } => (joined(blocks), true),
        OutputKind::Json { value } => (value.to_string(), false),
        OutputKind::ErrorJson { value } => (value.to_string(), true),
        OutputKind::ExecutionDenied { .. } => return None,
    })
}

/// The exit code a line states: digits after `exit code`, `exit_code`,
/// `exited with code`, or `exit status`, in any letter case, past a `:`,
/// `=`, `"`, or spaces. Shell tools print the code in these words.
fn stated_exit_code(line: &str) -> Option<i64> {
    let lower = line.to_ascii_lowercase();
    ["exit code", "exit_code", "exited with code", "exit status"]
        .iter()
        .filter_map(|phrase| {
            let rest = &lower[lower.find(phrase)? + phrase.len()..];
            let digits = rest.trim_start_matches([':', '=', ' ', '"']);
            let end = digits
                .find(|ch: char| !ch.is_ascii_digit())
                .unwrap_or(digits.len());
            digits[..end].parse().ok()
        })
        .next()
}

/// The words a shell prints when it finds no program of the name it was
/// given.
const MISSING_COMMAND: &str = "command not found";

/// Level words that mark a line of a failed command's output as one that
/// states an error, in any letter case.
const ERROR_WORDS: [&str; 9] = [
    "error",
    "errors",
    "failed",
    "failure",
    "fatal",
    "panic",
    "panicked",
    "exception",
    "traceback",
];

/// Whether a line of a failed command's output states an error: it holds one
/// of [`ERROR_WORDS`] as a word, a type name that ends in `Error` or
/// `Exception`, or the shell's [`MISSING_COMMAND`] report. The rule chooses
/// which lines of a known failure to keep; it decides no failure.
fn states_error(line: &str) -> bool {
    line.contains(MISSING_COMMAND)
        || line.split(|ch: char| !ch.is_alphanumeric()).any(|word| {
            ERROR_WORDS
                .iter()
                .any(|known| word.eq_ignore_ascii_case(known))
                || ((word.ends_with("Error") || word.ends_with("Exception"))
                    && word.len() > "Exception".len())
        })
}

/// A tool result that is one JSON object, read as lines: each string value
/// stands as its own lines, and each number or boolean as `name: value`.
/// Objects nest to two levels. A structured result names its output and its
/// exit code in fields, and the line rules then read them as they read a
/// shell tool's text. Any other text is returned as it is.
fn result_lines(text: &str) -> std::borrow::Cow<'_, str> {
    fn flatten(value: &Value, depth: usize, lines: &mut String) {
        let Value::Object(fields) = value else {
            return;
        };
        for (name, value) in fields {
            match value {
                Value::String(text) => {
                    lines.push_str(text);
                    lines.push('\n');
                }
                Value::Number(_) | Value::Bool(_) => {
                    lines.push_str(&format!("{name}: {value}\n"));
                }
                Value::Object(_) if depth < 2 => flatten(value, depth + 1, lines),
                _ => {}
            }
        }
    }
    if !text.trim_start().starts_with('{') {
        return text.into();
    }
    match serde_json::from_str::<Value>(text) {
        Ok(value @ Value::Object(_)) => {
            let mut lines = String::new();
            flatten(&value, 1, &mut lines);
            lines.into()
        }
        _ => text.into(),
    }
}

fn clipped(line: &str, limit: usize) -> String {
    if line.len() <= limit {
        return line.to_owned();
    }
    let mut end = limit;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &line[..end])
}

/// What a command's result reports. `text` is the output and `flagged` the
/// harness's error mark. The last exit code the output states decides when
/// one is stated: zero is a pass and any other code a failure. Without a
/// stated code, the result is a failure when the harness marked it, when a
/// line is a failure line as capture reads one in an output block, or when
/// the shell reports a missing command. The kept lines are the first lines
/// that state an error, each failure line with the line that completes it,
/// and then the closing lines of the output.
fn outcome_of(text: &str, flagged: bool) -> Outcome {
    let text = result_lines(text);
    let mut from = text.len().saturating_sub(MAX_RESULT_BYTES);
    while !text.is_char_boundary(from) {
        from += 1;
    }
    let text = &text[from..];
    let printable =
        |line: &&str| !line.is_empty() && !line.chars().any(|ch| ch.is_control() && ch != '\t');
    let exit_code = text.lines().rev().find_map(stated_exit_code);
    let mut fails = false;
    let mut errors: Vec<&str> = Vec::new();
    // A failure line that ends in `:`, as a panic header does, and one that
    // a location or a stack frame follows, is completed by the next line.
    let mut completes = false;
    for line in text.lines().map(str::trim).filter(printable) {
        let failure = super::native::is_output_failure_line(line);
        fails |= failure || line.contains(MISSING_COMMAND);
        let completing = completes
            && (errors.last().is_some_and(|last| last.ends_with(':'))
                || line.starts_with("-->")
                || line.starts_with("at "));
        if errors.len() < MAX_ERROR_LINES && (failure || completing || states_error(line)) {
            errors.push(line);
        }
        completes = failure;
    }
    let failed = match exit_code {
        Some(code) => code != 0,
        None => flagged || fails,
    };
    if !failed {
        return Outcome::Passed;
    }
    let mut closing: Vec<&str> = text
        .lines()
        .rev()
        .map(str::trim)
        .filter(printable)
        .take(MAX_CLOSING_LINES)
        .collect();
    closing.reverse();
    let mut lines: Vec<String> = Vec::new();
    for line in errors.into_iter().chain(closing) {
        let line = clipped(line, MAX_LINE_BYTES);
        if !lines.contains(&line) {
            lines.push(line);
        }
    }
    Outcome::Failed {
        exit_code,
        flagged,
        lines,
    }
}

/// The record's text: what the result states, the command, and the kept
/// lines, within one memory's bytes.
fn summary_of(tool: &str, command: &str, outcome: &Outcome) -> Option<String> {
    let Outcome::Failed {
        exit_code,
        flagged,
        lines,
    } = outcome
    else {
        return None;
    };
    let stated = match (exit_code, flagged) {
        (Some(code), _) => format!("the command failed with exit code {code}"),
        (None, true) => "the command failed and the tool reported an error".to_owned(),
        (None, false) => {
            "the command failed by the failure lines of its output, which states no exit code"
                .to_owned()
        }
    };
    let command = clipped(
        &command.split_whitespace().collect::<Vec<_>>().join(" "),
        MAX_COMMAND_BYTES,
    );
    let mut summary = format!(
        "Tool failure recorded from the result of {tool}: {stated}.\nCommand: {command}\nOutput lines:"
    );
    // The closing lines state the outcome, so they are kept whole and the
    // lines before them fill the bytes that remain.
    let closing = lines.len().saturating_sub(MAX_CLOSING_LINES);
    let reserved: usize = lines[closing..].iter().map(|line| line.len() + 1).sum();
    for (index, line) in lines.iter().enumerate() {
        let reserve = if index < closing { reserved } else { 0 };
        if summary.len() + 1 + line.len() + reserve > MAX_CAPTURE_MEMORY_BYTES {
            continue;
        }
        summary.push('\n');
        summary.push_str(line);
    }
    Some(context_core::redaction::redact_durable_text(&summary).text)
}

/// Command results of a transform window that wait for the Kernel.
pub(crate) struct FailureCheckpoint {
    kernel: Arc<kernel_routes::KernelOpenCoordinator>,
    project: ProjectBinding,
    session: String,
    memo: Arc<Mutex<FailureMemo>>,
    observed: Vec<Observed>,
}

impl HandlerCore {
    /// The command results of the window that this session's memo has not
    /// judged, when capture is on for a primary session. Returns `None`
    /// when the window holds none, so the caller spawns no task.
    pub(crate) fn failure_checkpoint(
        &self,
        binding: &SessionBinding,
        request: &crate::transform::TransformRequest,
    ) -> Option<FailureCheckpoint> {
        if request.is_subagent || !capture_enabled(binding) {
            return None;
        }
        let mut calls: HashMap<&str, (&str, String)> = HashMap::new();
        let mut observed = Vec::new();
        {
            let memo = self.failure_memo.lock().expect("failure memo mutex");
            for message in request.messages.iter() {
                for block in message.ck.content() {
                    match block.kind() {
                        BlockKind::ToolCall {
                            id, name, input, ..
                        } => {
                            if let Some(command) = command_of(input) {
                                calls.insert(id.as_str(), (name.as_str(), command));
                            }
                        }
                        BlockKind::ToolResult { id, output, .. } => {
                            let Some((tool, command)) = calls.get(id.as_str()) else {
                                continue;
                            };
                            if memo.has_seen(&binding.session, id) {
                                continue;
                            }
                            let Some((text, flagged)) = output_text(&output.kind) else {
                                continue;
                            };
                            observed.push(Observed {
                                call: id.clone(),
                                tool: (*tool).to_owned(),
                                key: command_key(command),
                                command: command.clone(),
                                at_ms: message.ck.meta.created_at_ms.unwrap_or_else(now_ms),
                                place: message.ordinal,
                                outcome: outcome_of(&text, flagged),
                            });
                        }
                        _ => {}
                    }
                }
            }
        }
        (!observed.is_empty()).then(|| FailureCheckpoint {
            kernel: Arc::clone(&self.kernel),
            project: binding.kernel_project.clone(),
            session: binding.session.clone(),
            memo: Arc::clone(&self.failure_memo),
            observed,
        })
    }
}

impl FailureCheckpoint {
    /// Kernel reads and commits run on the blocking pool. Returns the number
    /// of rows written.
    pub(crate) async fn run(self) -> usize {
        kernel_routes::blocking(move || self.record())
            .await
            .unwrap_or(0)
    }

    fn record(self) -> usize {
        let Self {
            kernel,
            project,
            session,
            memo,
            observed,
        } = self;
        let Ok(kernel) = kernel.kernel_store() else {
            return 0;
        };
        let writer = Writer {
            kernel: &kernel,
            project: &project,
            memo: &memo,
            scope: project.digest(),
            session: &session,
        };
        let mut written = 0;
        for unit in observed {
            match writer.judge(&unit) {
                Ok(wrote) => {
                    written += usize::from(wrote);
                    memo.lock().expect("failure memo mutex").learn(
                        &session,
                        &unit.call,
                        wrote && unit.outcome != Outcome::Passed,
                    );
                }
                // The memo has not learned the call, so the next pass judges
                // it again.
                Err(_) => {
                    eprintln!("daemon: tool failure record deferred");
                    return written;
                }
            }
        }
        written
    }
}

/// Whether `object` has the shape of a failure record: a memory-domain
/// decision on the failure lineage.
pub(crate) fn is_failure_record(object: &kernel::ObjectRow) -> bool {
    object.object_kind == "decision"
        && object.domain_id == "memory"
        && object.source_id == FAILURE_SOURCE_ID
}

/// The time as the record shows it, in UTC to the second, so a reader can
/// order the failures of a project.
fn utc_time(at_ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(at_ms)
        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

fn record_id(scope: &str, session: &str, call: &str) -> String {
    format!(
        "mem_{:x}",
        Sha256::digest(format!("{FAILURE_SOURCE_ID}:{scope}:{session}:{call}").as_bytes())
    )[..36]
        .to_owned()
}

/// The Kernel, the project, and the session one checkpoint writes for.
struct Writer<'a> {
    kernel: &'a kernel::KernelStore,
    project: &'a ProjectBinding,
    memo: &'a Mutex<FailureMemo>,
    /// The project's digest, which keys the memo's live records.
    scope: &'a str,
    session: &'a str,
}

impl Writer<'_> {
    /// Writes what one command result calls for, and returns whether it
    /// wrote a row. A failure writes its record unless the Kernel holds the
    /// record of this tool call, the session has written
    /// [`MAX_RECORDS_PER_SESSION`], or the command's live record is of a
    /// later run. A pass rewrites an unresolved live record of an earlier
    /// run as resolved.
    fn judge(&self, unit: &Observed) -> Result<bool, kernel::KernelError> {
        let head = self.live_head(&unit.key)?;
        if unit.outcome == Outcome::Passed {
            return match head {
                Some(head) if !head.resolved && head.at_ms <= unit.at_ms => {
                    self.resolve(unit, head)
                }
                _ => Ok(false),
            };
        }
        let id = record_id(self.scope, self.session, &unit.call);
        let (_, states) = self.kernel.object_states(std::slice::from_ref(&id))?;
        let held = self
            .memo
            .lock()
            .expect("failure memo mutex")
            .records(self.session);
        if states[0].is_some()
            || held >= MAX_RECORDS_PER_SESSION
            || head.as_ref().is_some_and(|head| head.at_ms > unit.at_ms)
        {
            return Ok(false);
        }
        self.fail(unit, id, head)
    }

    /// The live record of `key`, from the memo, which reads the project's
    /// records from the Kernel the first time the project is asked for.
    fn live_head(&self, key: &str) -> Result<Option<Head>, kernel::KernelError> {
        if let Some(heads) = self
            .memo
            .lock()
            .expect("failure memo mutex")
            .heads
            .get(self.scope)
        {
            return Ok(heads.get(key).cloned());
        }
        let read = kernel_routes::read::read_visible(
            self.kernel,
            self.project,
            kernel::Surface::ExplicitSearch,
            None,
            kernel_routes::read::RowSelection::DomainDecisions("memory"),
        )?;
        let mut heads = HashMap::new();
        for row in &read.rows {
            if !is_failure_record(&row.object) {
                continue;
            }
            let Some(decision) = read.decisions.get(&row.object.object_id) else {
                continue;
            };
            let Ok(Value::Object(rationale)) = serde_json::from_str(&decision.payload.rationale)
            else {
                continue;
            };
            let Some(key) = rationale.get("command").and_then(Value::as_str) else {
                continue;
            };
            heads.insert(
                key.to_owned(),
                Head {
                    id: row.object.object_id.clone(),
                    revision: row.object.source_revision,
                    resolved: rationale.get("state").and_then(Value::as_str) == Some("resolved"),
                    at_ms: rationale.get("at_ms").and_then(Value::as_i64).unwrap_or(0),
                    summary: decision.payload.summary.clone(),
                    rationale,
                },
            );
        }
        let mut memo = self.memo.lock().expect("failure memo mutex");
        let heads = memo.heads.entry(self.scope.to_owned()).or_insert(heads);
        Ok(heads.get(key).cloned())
    }

    /// Commits `head` as the live record of `key`, in place of `superseded`
    /// when one is given, as a labeled row.
    fn commit(
        &self,
        key: &str,
        superseded: Option<&Head>,
        head: Head,
    ) -> Result<bool, kernel::KernelError> {
        let project = self.project;
        let rationale = Value::Object(head.rationale.clone()).to_string();
        let intent = kernel::CommitIntent {
            producer: FAILURE_SOURCE_ID.into(),
            operation_key: project
                .operation_key(FAILURE_SOURCE_ID, &head.id)
                .ok_or(kernel::KernelError::InvalidInput)?,
            request_digest: format!(
                "{:x}",
                Sha256::digest(format!("{}\n{rationale}", head.summary).as_bytes())
            ),
            actor: "agent:tool_failure".into(),
            cause: "record of a failed command".into(),
        };
        let spec = kernel::DecisionSpec {
            decision_id: format!("dec_{}", &head.id[4..]),
            object_id: head.id.clone(),
            domain_id: "memory".into(),
            proposition_id: None,
            scope_id: Some(project.scope_id()),
            anchor_id: None,
            evidence_id: None,
            decision_kind: FAILURE_CATEGORY.into(),
            payload: kernel::DecisionPayload {
                summary: head.summary.clone(),
                rationale,
            },
            source_kind: CAPTURE_SOURCE_KIND.into(),
            source_id: FAILURE_SOURCE_ID.into(),
            source_revision: head.revision,
            sensitivity: kernel::Sensitivity::Normal,
        };
        let target = superseded.map(|earlier| earlier.id.clone());
        let id = head.id.clone();
        self.kernel
            .commit_before(Instant::now() + COMMIT_WAIT, intent, |envelope| {
                let mut domains = HashSet::new();
                let (mut ready, mut refusal) = (false, None);
                kernel_routes::commit::ensure_domain(envelope, "memory", &mut domains)?;
                kernel_routes::commit::ensure_scope(
                    envelope,
                    project,
                    &mut ready,
                    &mut domains,
                    &mut refusal,
                )?;
                match &target {
                    Some(target) => envelope.supersede_decision(target, spec)?,
                    None => envelope.insert_decision(spec)?,
                };
                kernel_routes::commit::admit_capture(envelope, &id, None)?;
                Ok(id.clone())
            })?;
        self.memo
            .lock()
            .expect("failure memo mutex")
            .heads
            .entry(self.scope.to_owned())
            .or_default()
            .insert(key.to_owned(), head);
        Ok(true)
    }

    fn fail(
        &self,
        unit: &Observed,
        id: String,
        earlier: Option<Head>,
    ) -> Result<bool, kernel::KernelError> {
        let Some(summary) = summary_of(&unit.tool, &unit.command, &unit.outcome) else {
            return Ok(false);
        };
        let mut rationale: serde_json::Map<String, Value> = [
            ("source", json!(FAILURE_SOURCE_ID)),
            ("session", json!(self.session)),
            ("call", json!(unit.call)),
            ("tool", json!(unit.tool)),
            ("command", json!(unit.key)),
            ("state", json!("failed")),
            ("at_ms", json!(unit.at_ms)),
            ("at", json!(utc_time(unit.at_ms))),
            ("place", json!(unit.place)),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect();
        if let Outcome::Failed {
            exit_code: Some(code),
            ..
        } = &unit.outcome
        {
            rationale.insert("exit_code".to_owned(), json!(code));
        }
        let head = Head {
            id,
            revision: earlier.as_ref().map_or(1, |earlier| earlier.revision + 1),
            resolved: false,
            at_ms: unit.at_ms,
            summary,
            rationale,
        };
        self.commit(&unit.key, earlier.as_ref(), head)
    }

    /// Rewrites the live record of a command as resolved: the same text with
    /// a closing line, and the session and time of the run that passed.
    fn resolve(&self, unit: &Observed, earlier: Head) -> Result<bool, kernel::KernelError> {
        let mut summary = earlier.summary.clone();
        let mut room = MAX_CAPTURE_MEMORY_BYTES
            .saturating_sub(RESOLVED_LINE.len())
            .min(summary.len());
        while !summary.is_char_boundary(room) {
            room -= 1;
        }
        summary.truncate(room);
        summary.push_str(RESOLVED_LINE);
        let mut rationale = earlier.rationale.clone();
        rationale.insert("state".to_owned(), json!("resolved"));
        rationale.insert("resolved_session".to_owned(), json!(self.session));
        rationale.insert("resolved_at".to_owned(), json!(utc_time(unit.at_ms)));
        let id = format!(
            "mem_{:x}",
            Sha256::digest(format!("{}:resolved", earlier.id).as_bytes())
        )[..36]
            .to_owned();
        let head = Head {
            id,
            revision: earlier.revision + 1,
            resolved: true,
            at_ms: earlier.at_ms,
            summary,
            rationale,
        };
        self.commit(&unit.key, Some(&earlier), head)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_is_read_from_the_call_and_keyed_without_its_directory_and_assignments() {
        assert_eq!(
            command_of(&json!({"command": "cargo test -p api"})).as_deref(),
            Some("cargo test -p api")
        );
        assert_eq!(
            command_of(&json!({"cmd": ["npm", "run", "build"]})).as_deref(),
            Some("npm run build")
        );
        assert_eq!(command_of(&json!({"path": "src/main.rs"})), None);
        assert_eq!(command_of(&json!({"command": "  "})), None);
        assert_eq!(command_of(&json!({"command": 7})), None);
        for (left, right) in [
            ("cd services/api && cargo test", "cargo   test"),
            ("RUST_LOG=debug CI=1 cargo test", "cargo test"),
            ("cd a && cd b && make check", "make check"),
        ] {
            assert_eq!(command_key(left), command_key(right), "{left:?}");
        }
        assert_ne!(command_key("cargo test"), command_key("cargo test -p api"));
        // An assignment that is the whole command is the command.
        assert_eq!(normalized("FOO=bar"), "FOO=bar");
    }

    #[test]
    fn a_result_fails_by_its_exit_code_the_harness_mark_or_its_failure_lines() {
        let failed = |text: &str, flagged: bool| match outcome_of(text, flagged) {
            Outcome::Failed {
                exit_code, lines, ..
            } => Some((exit_code, lines)),
            Outcome::Passed => None,
        };
        // A stated code decides, in the words shell tools print.
        for text in [
            "boom\n\nCommand exited with code 2",
            "Exit code 2\nboom",
            "boom\nProcess exited with code 2",
            "boom\nExit code: 2",
            "boom\nexit status 2",
        ] {
            assert_eq!(
                failed(text, false).map(|(code, _)| code),
                Some(Some(2)),
                "{text:?}"
            );
        }
        assert_eq!(failed("ERROR in the log\nExit code: 0", false), None);
        assert_eq!(failed("all good", false), None);
        // The harness mark, with no code stated.
        assert_eq!(
            failed("permission denied", true),
            Some((None, vec!["permission denied".to_owned()]))
        );
        // Failure lines, with no code stated: a pipeline hides the code of
        // the command that failed.
        let (code, lines) = failed(
            "running 2 tests\ntest parses ... ok\ntest renders ... FAILED\n\nthread 'renders' panicked at src/render.rs:9:5:\nwidth is zero\ntest result: FAILED. 1 passed; 1 failed",
            false,
        )
        .unwrap();
        assert_eq!(code, None);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("panicked at src/render.rs:9:5")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|line| line == "width is zero"),
            "{lines:?}"
        );
        assert!(
            lines.last().unwrap().starts_with("test result: FAILED"),
            "{lines:?}"
        );
        // A long line is cut.
        let long = format!("error: {}", "x".repeat(600));
        let (_, lines) = failed(&long, false).unwrap();
        assert!(lines[0].len() <= MAX_LINE_BYTES + "…".len() && lines[0].ends_with('…'));
        // The shell's report of a missing program, with no code stated.
        assert_eq!(
            failed("/usr/bin/bash: line 1: yarn: command not found", false),
            Some((
                None,
                vec!["/usr/bin/bash: line 1: yarn: command not found".to_owned()]
            ))
        );
        // A line that names an error word decides no failure on its own.
        assert_eq!(
            failed("0 errors, 0 warnings\nno failure detected", false),
            None
        );
    }

    #[test]
    fn a_failed_result_keeps_its_first_error_lines_and_its_closing_lines() {
        let Outcome::Failed { lines, .. } = outcome_of(
            "> tsc --noEmit\n\nsrc/app.ts(3,10): error TS2307: Cannot find module './render'.\nsrc/app.ts(9,5): error TS7006: Parameter 'event' implicitly has an 'any' type.\nM src/app.ts\n?? notes.txt\n\nCommand exited with code 2",
            true,
        ) else {
            panic!("the result failed");
        };
        assert_eq!(
            lines,
            [
                "src/app.ts(3,10): error TS2307: Cannot find module './render'.",
                "src/app.ts(9,5): error TS7006: Parameter 'event' implicitly has an 'any' type.",
                "M src/app.ts",
                "?? notes.txt",
                "Command exited with code 2",
            ]
        );
        // More error lines than are kept: the first ones and the closing lines.
        let many: String = (0..20)
            .map(|index| format!("FAILED tests/test_{index}.py::test_case\n"))
            .chain(["20 failed, 3 passed in 1.2s\n".to_owned()])
            .collect();
        let Outcome::Failed { lines, .. } = outcome_of(&many, false) else {
            panic!("the result failed");
        };
        assert_eq!(lines.len(), MAX_ERROR_LINES + MAX_CLOSING_LINES);
        assert_eq!(lines[0], "FAILED tests/test_0.py::test_case");
        assert_eq!(lines.last().unwrap(), "20 failed, 3 passed in 1.2s");
        // The closing lines stay when the kept lines pass one memory's bytes.
        let long: Vec<String> = (0..MAX_ERROR_LINES)
            .map(|index| format!("error: {index} {}", "x".repeat(MAX_LINE_BYTES)))
            .chain(["3 failed".to_owned()])
            .collect();
        let summary = summary_of(
            "bash",
            "make",
            &Outcome::Failed {
                exit_code: Some(2),
                flagged: false,
                lines: long
                    .iter()
                    .map(|line| clipped(line, MAX_LINE_BYTES))
                    .collect(),
            },
        )
        .unwrap();
        assert!(summary.len() <= MAX_CAPTURE_MEMORY_BYTES);
        assert!(summary.ends_with("\n3 failed"), "{summary}");
        assert!(summary.contains("error: 0 "));
    }

    #[test]
    fn a_structured_result_is_read_by_its_fields() {
        let result = json!({
            "output": "zsh:1: command not found: fd\n",
            "metadata": { "exit_code": 127, "duration_seconds": 0.1 },
        })
        .to_string();
        assert_eq!(
            outcome_of(&result, false),
            Outcome::Failed {
                exit_code: Some(127),
                flagged: false,
                // Fields are read in name order.
                lines: vec![
                    "zsh:1: command not found: fd".to_owned(),
                    "duration_seconds: 0.1".to_owned(),
                    "exit_code: 127".to_owned(),
                ],
            }
        );
        let passed = json!({ "output": "ok", "metadata": { "exit_code": 0 } }).to_string();
        assert_eq!(outcome_of(&passed, false), Outcome::Passed);
        // Text that only starts like an object is read as text.
        assert_eq!(
            outcome_of("{ not json\nerror: bad config", false),
            Outcome::Failed {
                exit_code: None,
                flagged: false,
                // The error line, then the closing line it leaves.
                lines: vec!["error: bad config".to_owned(), "{ not json".to_owned()],
            }
        );
    }

    #[test]
    fn the_record_states_the_outcome_the_command_and_the_lines_within_one_memory() {
        let outcome = Outcome::Failed {
            exit_code: Some(101),
            flagged: true,
            lines: vec!["error[E0308]: mismatched types".to_owned()],
        };
        assert_eq!(
            summary_of("bash", "cd api &&  cargo build", &outcome).unwrap(),
            "Tool failure recorded from the result of bash: the command failed with exit code 101.\nCommand: cd api && cargo build\nOutput lines:\nerror[E0308]: mismatched types"
        );
        let many = Outcome::Failed {
            exit_code: None,
            flagged: false,
            lines: vec!["e".repeat(MAX_LINE_BYTES); 40],
        };
        let summary = summary_of("bash", "make", &many).unwrap();
        assert!(summary.len() <= MAX_CAPTURE_MEMORY_BYTES);
        assert!(summary.len() > MAX_CAPTURE_MEMORY_BYTES - MAX_LINE_BYTES - 1);
        assert!(summary.contains("which states no exit code"));
        assert_eq!(summary_of("bash", "make", &Outcome::Passed), None);
    }
}

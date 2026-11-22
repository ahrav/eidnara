//! Records of failed commands, written from tool results without a model
//! call.
//!
//! A later session is asked what went wrong the last time a command failed.
//! Capture reads message text, and the error a command printed stands in its
//! tool result. Each transform pass carries the window's tool calls and
//! results, so the daemon reads them there: a result of a tool call that
//! carries a command line becomes one labeled memory row of category
//! [`FAILURE_CATEGORY`] when the result reports a failure (see
//! [`outcome_of`]). The row is served on explicit search and reaches no
//! automatic surface: the block renders positive categories only.
//!
//! A record is a durable copy of part of a tool result, so it is written
//! under the consent that already retains tool results, source capture
//! ([`crate::source_capture::consents`]), and it holds text under source
//! capture's rule for a tool span: text in which the durable-text redactor
//! ([`context_core::redaction`]) finds nothing is kept as it is, and text
//! with a finding is withheld whole. The record keeps the program's name and
//! its first plain operand from the command line and leaves the arguments
//! out.
//!
//! One row is live per command of a project. The rows of one command share a
//! source lineage, `tool_failure:<digest>`, and the Kernel's newest row on
//! that lineage is the command's record: a later failure supersedes it, and a
//! later run that reports no failure rewrites it as resolved.
//!
//! A record's id derives from its tool call's id, so the daemon records the
//! results of a harness whose call ids name one execution. Pi's do.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use kernel::applicability::EvalBudget;
use memory_store::{BlockKind, OutputKind, ResultBlockKind};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::native::{completes_failure_line, is_output_failure_line, printable};
use super::{CAPTURE_SOURCE_KIND, COMMIT_WAIT, capture_enabled};
use crate::healing::SerializerProfile;
use crate::kernel_routes::{self, ProjectBinding};
use crate::retained_size::ingress_message_retained_bytes;
use crate::wire::IngressMessage;
use crate::{HandlerCore, SessionBinding};

/// The first part of the source lineage of every failure record. The lineage
/// of one command's records is this, `:`, and the command's digest.
pub(crate) const FAILURE_SOURCE_ID: &str = "tool_failure";
/// The category of a failure record. The block renders positive categories,
/// and this is none of them, so a record is served on explicit search alone.
pub(crate) const FAILURE_CATEGORY: &str = "TOOL_FAILURE";

/// Whether `source_id` names the lineage of a command's failure records.
pub(crate) fn is_failure_lineage(source_id: &str) -> bool {
    source_id
        .strip_prefix(FAILURE_SOURCE_ID)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(':'))
}

/// Lines that state an error kept from one result, in output order. A
/// compiler or a test runner lists several, and the first ones name the
/// failure.
const MAX_ERROR_LINES: usize = 5;
/// Closing lines kept from one result: a failing command states its outcome
/// last.
const MAX_CLOSING_LINES: usize = 2;
/// One kept line, in bytes, with the mark that ends a cut line.
const MAX_LINE_BYTES: usize = 160;
/// The program's name, and its first operand, as a record shows them, in
/// bytes each.
const MAX_WORD_BYTES: usize = 96;
/// The tool's name as the record's text shows it, in bytes.
const MAX_TOOL_BYTES: usize = 64;
/// A tool call's id and a session's id as a record shows them, in bytes.
const MAX_ID_BYTES: usize = 128;
/// The path and the error name a record keeps, in bytes each.
const MAX_TOKEN_BYTES: usize = 160;
/// The record's text, in bytes. The search tool renders 1,024 bytes of a
/// fact's content, so the text and the first fields of the rationale fit.
const MAX_SUMMARY_BYTES: usize = 900;
/// The text of one result that is read, which is the largest tool span
/// source capture retains. A longer result is left out, and so is a command
/// line longer than this.
const MAX_OUTPUT_BYTES: usize = crate::harness_sources::MAX_SOURCE_UNIT_BYTES;
/// The first line of a result states an exit code when it ends within these
/// bytes of the output.
const MAX_FIRST_LINE_BYTES: usize = 1024;
/// Closing lines read for a stated exit code.
const EXIT_CODE_CLOSING_LINES: usize = 3;
/// Records one session writes in one project, resolved copies included.
const MAX_RECORDS_PER_SESSION: usize = 64;
/// Command results one pass hands off, the earliest unjudged ones first, so
/// the results of one command are judged in the order they ran.
const MAX_RESULTS_PER_PASS: usize = 256;
/// Tool calls one session's memo remembers, and the command results at the
/// end of a window that a pass considers. The two are equal, so a call the
/// memo evicts has left the results a later pass considers.
const MEMO_CALLS_PER_SESSION: usize = 1024;
const MEMO_SESSIONS: usize = 256;
/// Checkpoints that read results at one time. Their judgments run one at a
/// time under the capture publication gate, so more of them add no work
/// done.
const MAX_CHECKPOINTS: usize = 8;
/// The bytes of window messages that the checkpoints in flight hold, over
/// all sessions. A pass hands off the results whose messages fit.
const MAX_HELD_WINDOW_BYTES: usize = 32 * 1024 * 1024;
/// The bytes one checkpoint holds beside its window messages while it reads
/// one result: the command and the output with their redacted copies, and
/// the JSON value of an output that is one object.
const CHECKPOINT_SCRATCH_BYTES: usize = 64 * MAX_OUTPUT_BYTES;
/// Marks a line or a word cut to its byte limit.
const CUT_MARK: &str = "…";
/// The sentence a resolved record's first line ends with.
const RESOLVED_NOTE: &str = " Resolved: a later run of this command reported no failure.";
/// What follows the program's name in a record whose command line holds
/// more than the words shown.
const ARGUMENTS_NOTE: &str = " [arguments left out]";

/// The resident bytes this module retains at its bounds: each session's
/// call memo, at the 2,048 slots its set and its queue grow to; the
/// references of each session's checkpoint; the window messages the
/// checkpoints hold; and the scratch of the checkpoints that read at one
/// time.
pub(crate) const RETAINED_BYTES_BOUND: u64 =
    (MEMO_SESSIONS * (MEMO_CALLS_PER_SESSION * 80 + MAX_RESULTS_PER_PASS * size_of::<Pending>())
        + MAX_HELD_WINDOW_BYTES
        + MAX_CHECKPOINTS * CHECKPOINT_SCRATCH_BYTES) as u64;

/// What one tool result says about its command.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    /// The result reports no failure.
    Passed,
    Failed(Failure),
}

/// What makes a result a failure, the strongest ground first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Evidence {
    /// The harness marked the result as an error, or the result states a
    /// nonzero exit code.
    ToolStatus,
    /// The command line printed its own exit status, and it is nonzero.
    PrintedStatus,
    /// The shell reported a program it did not find.
    ShellReport,
    /// The command line hides an exit status from the tool, and the output
    /// holds a failure line.
    FailureLines,
}

impl Evidence {
    /// The name of the ground in a record's rationale.
    const fn name(self) -> &'static str {
        match self {
            Self::ToolStatus => "tool_status",
            Self::PrintedStatus => "printed_status",
            Self::ShellReport => "shell_report",
            Self::FailureLines => "failure_lines",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Failure {
    /// The nonzero exit code the result states or the command line printed.
    exit_code: Option<i64>,
    evidence: Evidence,
    /// The lines that state an error, in output order, and then the closing
    /// lines that are none of them. A closing line that states an error
    /// stands among the error lines.
    lines: Vec<Kept>,
}

/// One line kept from a failed result, within [`MAX_LINE_BYTES`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct Kept {
    text: String,
    /// The line is one of the output's closing lines.
    closing: bool,
    /// The bytes of the line in the output it was read from.
    span: Range<usize>,
}

/// What a record keeps of a failed result's output.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Excerpt {
    /// The redactor found nothing in the output, so its kept lines stand in
    /// the record.
    Kept,
    /// The redactor found credential-shaped content in the output. The
    /// record holds no line of it.
    Finding,
    /// The redactor's scan of the output did not finish. The record holds
    /// no text of it.
    Unscanned,
}

impl Excerpt {
    /// The sentence that stands in a record in place of the output's lines.
    const fn reason(&self) -> &'static str {
        match self {
            Self::Kept => "",
            Self::Finding => "the redactor found credential-shaped content in the output",
            Self::Unscanned => "the redactor's scan of the output did not finish",
        }
    }
}

/// One command result of the window that the session's memo has not seen:
/// the messages that hold the call and the result, shared with the pass.
struct Pending {
    call: Arc<IngressMessage>,
    call_block: usize,
    result: Arc<IngressMessage>,
    result_block: usize,
    /// The name of the tool call in the memo.
    digest: u64,
}

/// A command line as a record names it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Command {
    /// The program's name and its first plain operand, and
    /// [`ARGUMENTS_NOTE`] when the command line holds more. Empty when the
    /// command line begins with no plain word.
    shown: String,
    /// The digest of the project and the redacted, normalized command line,
    /// which names the lineage of the command's records.
    digest: String,
}

/// One failed or passing command result as the detached task read it.
struct Observed {
    call: String,
    tool: String,
    command: Command,
    /// When the harness wrote the result, in milliseconds.
    at_ms: i64,
    outcome: Outcome,
    /// What the record keeps of the output, with the path and the error
    /// name its kept lines state.
    excerpt: Excerpt,
    path: Option<String>,
    error: Option<String>,
}

/// The tool calls of one session that its checkpoints have judged.
#[derive(Default)]
struct SessionCalls {
    /// Distinguishes this entry from one created after the session's delete.
    epoch: u64,
    /// A checkpoint of the session is admitted and has not finished. The
    /// entry stays in the memo while it is set.
    in_flight: bool,
    calls: HashSet<u64>,
    /// The judged calls from the earliest learned to the latest.
    order: VecDeque<u64>,
}

/// Tool calls already judged, by project and session. The memo spares a
/// pass the reading of results an earlier pass judged; a record's id derives
/// from its tool call, and the Kernel holds it, so a call judged again after
/// a restart writes nothing.
#[derive(Default)]
pub(crate) struct FailureMemo {
    sessions: HashMap<String, SessionCalls>,
    /// Sessions in the order their entries were created.
    order: VecDeque<String>,
    next_epoch: u64,
    /// Checkpoints admitted and not finished.
    in_flight: usize,
    /// The bytes of window messages that checkpoints in flight hold.
    held_window_bytes: usize,
}

/// The memo's name of a tool call: the project and the call's id. The name
/// lives in this process alone.
fn call_digest(scope: &str, call: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (scope, call).hash(&mut hasher);
    hasher.finish()
}

impl FailureMemo {
    fn has_seen(&self, session: &str, call: u64) -> bool {
        self.sessions
            .get(session)
            .is_some_and(|entry| entry.calls.contains(&call))
    }

    /// Whether a new checkpoint of `session` waits: one of the session is in
    /// flight, or [`MAX_CHECKPOINTS`] are.
    fn busy(&self, session: &str) -> bool {
        self.in_flight >= MAX_CHECKPOINTS
            || self
                .sessions
                .get(session)
                .is_some_and(|entry| entry.in_flight)
    }

    /// Marks the session's entry as held by one checkpoint and returns its
    /// epoch, creating the entry when absent. A new entry at the session
    /// limit takes the place of the oldest entry no checkpoint holds, and
    /// `None` is returned when a checkpoint holds every entry.
    fn pin(&mut self, session: &str) -> Option<u64> {
        if !self.sessions.contains_key(session) {
            if self.sessions.len() >= MEMO_SESSIONS {
                let victim = self
                    .order
                    .iter()
                    .position(|kept| self.sessions.get(kept).is_none_or(|entry| !entry.in_flight))
                    .and_then(|index| self.order.remove(index))?;
                self.sessions.remove(&victim);
            }
            self.order.push_back(session.to_owned());
            self.next_epoch += 1;
            self.sessions.insert(
                session.to_owned(),
                SessionCalls {
                    epoch: self.next_epoch,
                    ..Default::default()
                },
            );
        }
        let entry = self.sessions.get_mut(session)?;
        entry.in_flight = true;
        self.in_flight += 1;
        Some(entry.epoch)
    }

    fn release(&mut self, session: &str, epoch: u64) {
        self.in_flight = self.in_flight.saturating_sub(1);
        if let Some(entry) = self
            .sessions
            .get_mut(session)
            .filter(|entry| entry.epoch == epoch)
        {
            entry.in_flight = false;
        }
    }

    /// Whether the entry pinned at `epoch` is the session's entry. A session
    /// delete removes the entry, so its earlier checkpoints read `false`.
    fn current(&self, session: &str, epoch: u64) -> bool {
        self.sessions
            .get(session)
            .is_some_and(|entry| entry.epoch == epoch)
    }

    /// Remembers a judged call. A full entry gives up the call it learned
    /// first, which has left the results a pass considers.
    fn learn(&mut self, session: &str, epoch: u64, call: u64) {
        let Some(entry) = self
            .sessions
            .get_mut(session)
            .filter(|entry| entry.epoch == epoch)
        else {
            return;
        };
        if !entry.calls.insert(call) {
            return;
        }
        entry.order.push_back(call);
        while entry.order.len() > MEMO_CALLS_PER_SESSION {
            if let Some(evicted) = entry.order.pop_front() {
                entry.calls.remove(&evicted);
            }
        }
    }

    pub(crate) fn forget_session(&mut self, session: &str) {
        if self.sessions.remove(session).is_some() {
            self.order.retain(|kept| kept != session);
        }
    }

    #[cfg(test)]
    pub(crate) fn has_seen_for_test(&self, scope: &str, session: &str, call: &str) -> bool {
        self.has_seen(session, call_digest(scope, call))
    }

    #[cfg(test)]
    pub(crate) fn calls_for_test(&self, session: &str) -> usize {
        self.sessions
            .get(session)
            .map_or(0, |entry| entry.calls.len())
    }

    /// The bytes of window messages that checkpoints in flight hold, and
    /// the number of those checkpoints.
    #[cfg(test)]
    pub(crate) fn held_for_test(&self) -> (usize, usize) {
        (self.held_window_bytes, self.in_flight)
    }
}

/// Whether a field of a tool call's input is a command line: a string, or an
/// array of strings, that holds a character other than whitespace.
fn carries_command(field: &Value) -> bool {
    let filled = |text: &str| !text.trim().is_empty();
    match field {
        Value::String(text) => filled(text),
        Value::Array(words) => {
            words.iter().all(Value::is_string) && words.iter().filter_map(Value::as_str).any(filled)
        }
        _ => false,
    }
}

/// The field that carries a tool call's command line: `command` when it is a
/// command line, else `cmd`. A shell tool of any name carries one.
fn command_field(input: &Value) -> Option<&Value> {
    ["command", "cmd"]
        .into_iter()
        .filter_map(|name| input.get(name))
        .find(|field| carries_command(field))
}

/// The command line a tool call carries, with an array's strings joined by
/// single spaces.
fn command_of(input: &Value) -> Option<String> {
    match command_field(input)? {
        Value::String(text) => Some(text.clone()),
        Value::Array(words) => Some(
            words
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "),
        ),
        _ => None,
    }
}

/// Each character of `text` with the quoting it stands in, by the shell's
/// reading of single quotes, double quotes, and backslash escapes: `None`
/// for a character outside them, the quote character for one inside a
/// quoted span and for the quotes that open and close it, and a backslash
/// for a backslash and the character it escapes. The reading follows quotes and
/// backslashes and no other shell grammar, so the text of a here-document
/// is read as command words.
fn quoting(text: &str) -> impl Iterator<Item = (usize, char, Option<char>)> + '_ {
    let mut quote = None;
    let mut escaped = false;
    text.char_indices().map(move |(index, ch)| {
        if escaped {
            escaped = false;
            return (index, ch, Some('\\'));
        }
        let state = match (quote, ch) {
            (Some('\''), '\'') | (Some('"'), '"') => quote.take(),
            (Some('\''), _) => quote,
            (_, '\\') => {
                escaped = true;
                Some('\\')
            }
            (None, '\'' | '"') => {
                quote = Some(ch);
                quote
            }
            _ => quote,
        };
        (index, ch, state)
    })
}

/// The places in `text` of the characters among `marks` that stand outside
/// quotes and backslash escapes (see [`quoting`]).
fn unquoted(text: &str, marks: &[char]) -> Vec<usize> {
    quoting(text)
        .filter(|(_, ch, state)| state.is_none() && marks.contains(ch))
        .map(|(index, ..)| index)
        .collect()
}

/// The characters that end a shell word outside quotes.
const WORD_ENDS: [char; 7] = [' ', '\t', '\n', '\r', ';', '&', '|'];

/// The end of the shell word `text` starts with: the first whitespace, `;`,
/// `&`, or `|` outside quotes and backslash escapes, or the end of the text.
fn word_end(text: &str) -> usize {
    unquoted(text, &WORD_ENDS)
        .first()
        .copied()
        .unwrap_or(text.len())
}

/// What follows a leading `cd <path> &&` or `cd <path>;` step, where the
/// path is one shell word.
fn after_cd(text: &str) -> Option<&str> {
    let path = text.strip_prefix("cd")?;
    if !path.starts_with(char::is_whitespace) {
        return None;
    }
    let path = path.trim_start();
    let end = Some(word_end(path)).filter(|end| *end > 0)?;
    let tail = path[end..].trim_start();
    let after = tail.strip_prefix("&&").or_else(|| tail.strip_prefix(';'))?;
    Some(after.trim_start())
}

/// What follows a leading `NAME=value` assignment, where the value is one
/// shell word that whitespace ends.
fn after_assignment(text: &str) -> Option<&str> {
    let name = text.find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))?;
    if name == 0 || text.starts_with(|ch: char| ch.is_ascii_digit()) {
        return None;
    }
    let value = text[name..].strip_prefix('=')?;
    let after = &value[word_end(value)..];
    after
        .starts_with(char::is_whitespace)
        .then(|| after.trim_start())
}

/// The command without what varies between runs of one operation: leading
/// `cd <path> &&` and `cd <path>;` steps, leading `NAME=value` assignments,
/// and the width of whitespace runs. A step or an assignment that ends the
/// command stays.
fn normalized(command: &str) -> String {
    let mut rest = command.trim_start();
    while let Some(after) = after_cd(rest)
        .or_else(|| after_assignment(rest))
        .filter(|after| !after.is_empty())
    {
        rest = after;
    }
    collapsed(rest)
}

/// `text` with each whitespace run as one space.
fn collapsed(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` within `limit` bytes: a longer text is cut at a character boundary
/// and ends in [`CUT_MARK`].
fn clipped(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let keep = text.floor_char_boundary(limit - CUT_MARK.len());
    format!("{}{CUT_MARK}", &text[..keep])
}

/// Whether `word` is a program's name, a subcommand, or a path as a shell
/// writes one bare: ASCII letters, digits, and `_`, `.`, `/`, `+`, `-`, at
/// most [`MAX_WORD_BYTES`]. A word the redactor rewrote holds `<`, and a
/// URL, an assignment, a quoted word, and an expansion hold a character
/// outside these.
fn plain_word(word: &str) -> bool {
    !word.is_empty()
        && word.len() <= MAX_WORD_BYTES
        && word
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_./+-".contains(&byte))
}

/// The command line as a record names it, or `None` when the redactor's
/// scan of it does not finish. The command line passes the redactor first,
/// so the words shown and the digest derive from redacted text: two command
/// lines that differ in a credential the redactor finds are one command.
/// The record shows the first word of the normalized command when it is a
/// plain word, and the word after it when that is a plain word that begins
/// with no `-`; every other argument stays out of the record.
fn command_named(scope: &str, command: &str) -> Option<Command> {
    let redacted = context_core::redaction::redact_windowed_durable_text(command, usize::MAX)
        .ok()?
        .text;
    let normal = normalized(&redacted);
    let mut hash = Sha256::new();
    for part in [scope, normal.as_str()] {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part.as_bytes());
    }
    let digest = format!("{:x}", hash.finalize())[..32].to_owned();
    let name = &normal[..word_end(&normal)];
    if !plain_word(name) {
        return Some(Command {
            shown: String::new(),
            digest,
        });
    }
    let rest = &normal[name.len()..];
    let operand = rest
        .strip_prefix(' ')
        .map(|rest| &rest[..word_end(rest)])
        .filter(|word| plain_word(word) && !word.starts_with('-'));
    let mut shown = name.to_owned();
    if let Some(operand) = operand {
        shown.push(' ');
        shown.push_str(operand);
    }
    if shown.len() < normal.len() {
        shown.push_str(ARGUMENTS_NOTE);
    }
    Some(Command { shown, digest })
}

/// The words a shell tool prints before an exit code.
const EXIT_PHRASES: [&str; 4] = ["exit code", "exit_code", "exited with code", "exit status"];

/// The words that may stand before one of [`EXIT_PHRASES`] in a line that
/// states the command's own exit code.
const EXIT_SUBJECTS: [&str; 7] = [
    "",
    "command",
    "the command",
    "process",
    "the process",
    "shell",
    "script",
];

/// The words a runner puts between the subject and the exit code of the
/// script it ran.
const EXIT_LINK: &str = "failed with";

/// Whether `word` is a log tag as a runner prints one before its own status
/// line, such as `ELIFECYCLE` or `ERR!`: three to sixteen ASCII capitals,
/// digits, `_`, `!`, and `:`, the first a capital. A child's name in prose,
/// as in `worker exited with code 7`, is no tag.
fn log_tag(word: &str) -> bool {
    (3..=16).contains(&word.len())
        && word.starts_with(|ch: char| ch.is_ascii_uppercase())
        && word.bytes().all(|byte| {
            byte.is_ascii_uppercase() || byte.is_ascii_digit() || b"_!:".contains(&byte)
        })
}

/// The exit code a line states when the whole line is a status line: one of
/// [`EXIT_SUBJECTS`], then [`EXIT_LINK`] or nothing, then one of
/// [`EXIT_PHRASES`], a run of `:`, `=`, `"`, and spaces, and digits, in any
/// letter case. Whitespace parts the subject from what follows it. The line
/// may stand inside one pair of brackets or of parentheses, end in a `.`,
/// and open with one [`log_tag`]. A line that says more, such as a worker's
/// or a child's report, leaves the command's code unstated. The reading is a
/// heuristic: a command can print such a line itself.
fn stated_exit_code(line: &str) -> Option<i64> {
    let line = line.trim();
    let line = line
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .or_else(|| {
            line.strip_prefix('(')
                .and_then(|inner| inner.strip_suffix(')'))
        })
        .unwrap_or(line)
        .trim();
    let line = line.strip_suffix('.').unwrap_or(line);
    let untagged = line
        .split_once(char::is_whitespace)
        .filter(|(tag, _)| log_tag(tag))
        .map(|(_, rest)| rest.trim_start());
    [Some(line), untagged]
        .into_iter()
        .flatten()
        .find_map(|line| {
            let lower = line.to_ascii_lowercase();
            EXIT_PHRASES.iter().find_map(|phrase| {
                let at = lower.find(phrase)?;
                let before = &lower[..at];
                if !(before.is_empty() || before.ends_with(char::is_whitespace)) {
                    return None;
                }
                let before = before.trim_end();
                let subject = before
                    .strip_suffix(EXIT_LINK)
                    .filter(|subject| subject.is_empty() || subject.ends_with(char::is_whitespace))
                    .unwrap_or(before)
                    .trim_end();
                if !EXIT_SUBJECTS.contains(&subject) {
                    return None;
                }
                let digits = lower[at + phrase.len()..]
                    .trim_start_matches([':', '=', ' ', '"'])
                    .trim_end_matches('"');
                let whole = !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit());
                whole.then(|| digits.parse().ok()).flatten()
            })
        })
}

/// The shell parameters that expand to an exit status: of the last command,
/// and of the commands of the last pipeline.
const STATUS_PARAMETERS: [&str; 3] = ["$?", "${?}", "${PIPESTATUS["];

/// The characters that part two commands of a command line.
const COMMAND_ENDS: [char; 4] = [';', '|', '&', '\n'];

/// The text an `echo` of the command line prints around an exit status:
/// what stands before the status parameter and what stands after it, with
/// quote characters left out and whitespace runs as single spaces. `None`
/// when no `echo` of the command line holds exactly one status parameter
/// outside single quotes. The first such `echo` is read. Its words end at a
/// redirection and at the `)` that closes a subshell.
fn status_echo(command: &str) -> Option<(String, String)> {
    let mut start = 0;
    let ends = unquoted(command, &COMMAND_ENDS);
    let steps = ends.iter().copied().chain([command.len()]).map(|end| {
        let step = &command[start..end];
        start = end + 1;
        step
    });
    for step in steps {
        let step = step.trim_start_matches(|ch: char| ch.is_whitespace() || "({".contains(ch));
        let Some(words) = step
            .strip_prefix("echo")
            .filter(|words| words.starts_with(char::is_whitespace))
        else {
            continue;
        };
        let words = &words[..unquoted(words, &['>', ')'])
            .first()
            .copied()
            .unwrap_or(words.len())];
        let found: Vec<(usize, &str)> = STATUS_PARAMETERS
            .iter()
            .flat_map(|parameter| words.match_indices(parameter))
            .collect();
        let [(at, parameter)] = found[..] else {
            continue;
        };
        // A parameter in single quotes or behind a backslash stays as it is
        // written, unexpanded.
        let state = quoting(words).find_map(|(index, _, state)| (index == at).then_some(state));
        if matches!(state, Some(Some('\'' | '\\'))) {
            continue;
        }
        let after = &words[at + parameter.len()..];
        let after = match parameter.ends_with('[') {
            true => after.split_once("]}")?.1,
            false => after,
        };
        let bare = |text: &str| collapsed(&text.replace(['"', '\''], ""));
        return Some((bare(&words[..at]), bare(after)));
    }
    None
}

/// The exit status the command line printed, when it asks for one (see
/// [`status_echo`]): the digits of the last line of `lines` that is the
/// echo's text with one or more numbers in place of the status parameter,
/// the greatest of them. An echo of the parameter alone is read from the
/// last non-empty line alone, since a bare number stands in other lines of
/// an output too.
fn printed_status(command: &str, lines: &[&str]) -> Option<i64> {
    let (before, after) = status_echo(command)?;
    let filled: Vec<&&str> = lines.iter().filter(|line| !line.is_empty()).collect();
    let considered = match before.is_empty() && after.is_empty() {
        true => filled.len().min(1),
        false => filled.len(),
    };
    filled.iter().rev().take(considered).find_map(|line| {
        let line = collapsed(line);
        let middle = line.strip_prefix(&before)?.strip_suffix(&after)?.trim();
        let numbers: Option<Vec<i64>> = middle
            .split(' ')
            .map(|number| {
                (!number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
                    .then(|| number.parse().ok())
                    .flatten()
            })
            .collect();
        numbers?.into_iter().max()
    })
}

/// Whether the command line hides an exit status from the tool: it joins
/// commands with a pipe, `||`, or `;`, and the shell reports a pipeline or a
/// list by its last command, so the tool's status covers that command alone.
/// An `&&` chain ends at its first failure and hides none.
fn hides_status(command: &str) -> bool {
    !unquoted(command, &['|', ';']).is_empty()
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
                || word
                    .strip_suffix("Error")
                    .or_else(|| word.strip_suffix("Exception"))
                    .is_some_and(|stem| !stem.is_empty())
        })
}

/// The closing line a harness writes when it ended the command itself: Pi
/// on an abort and on its timeout. The command's own outcome stays unknown,
/// so such a result is skipped.
const HARNESS_ENDED: [&str; 4] = [
    "Command aborted",
    "Operation aborted",
    "Tool execution aborted",
    "Command timed out after ",
];

/// The whole text of the result Pi writes for a tool call that an extension
/// blocked with Pi's default reason.
const HARNESS_BLOCKED: &str = "Tool execution was blocked";
/// The first line of the result Pi writes for a tool call whose arguments
/// its schema refuses, up to the tool's name.
const HARNESS_INVALID: &str = "Validation failed for tool \"";

/// Fields of a result's JSON object whose integer is the command's exit
/// code.
const EXIT_FIELDS: [&str; 3] = ["exit_code", "exitCode", "exit"];

/// A tool result as its outcome is read.
struct Reading {
    /// The result's text as the harness carries it: the text of a `text` or
    /// `error_text` result, and the text blocks of a `content` or
    /// `error_content` result joined by line breaks.
    raw: String,
    /// The string values of a result whose text is one JSON object, to two
    /// levels of nesting in field order, joined by line breaks. The lines of
    /// such a result are read from here.
    values: Option<String>,
    /// The harness marked the result as an error.
    flagged: bool,
    /// The exit code a field of the JSON object holds.
    field_exit: Option<i64>,
}

impl Reading {
    /// `None` for a result that is no text (a `json` or `error_json` value,
    /// which Pi's codec never gives, and a result the harness denied) and
    /// for text longer than [`MAX_OUTPUT_BYTES`], which source capture
    /// retains no span of.
    fn of_output(output: &OutputKind) -> Option<Self> {
        let joined = |blocks: &[memory_store::ResultBlock]| {
            let texts = || {
                blocks.iter().filter_map(|block| match &block.kind {
                    ResultBlockKind::Text { text } => Some(text.as_str()),
                    _ => None,
                })
            };
            let bytes: usize = texts().map(|text| text.len() + 1).sum();
            (bytes <= MAX_OUTPUT_BYTES + 1).then(|| texts().collect::<Vec<_>>().join("\n"))
        };
        let (raw, flagged) = match output {
            OutputKind::Text { text } => (text.clone(), false),
            OutputKind::ErrorText { text } => (text.clone(), true),
            OutputKind::Content { blocks } => (joined(blocks)?, false),
            OutputKind::ErrorContent { blocks } => (joined(blocks)?, true),
            OutputKind::Json { .. }
            | OutputKind::ErrorJson { .. }
            | OutputKind::ExecutionDenied { .. } => return None,
        };
        if raw.len() > MAX_OUTPUT_BYTES {
            return None;
        }
        let object = raw
            .trim_start()
            .starts_with('{')
            .then(|| serde_json::from_str::<Value>(&raw).ok())
            .flatten();
        let Some(Value::Object(fields)) = object else {
            return Some(Self {
                raw,
                values: None,
                flagged,
                field_exit: None,
            });
        };
        let exit = |fields: &serde_json::Map<String, Value>| {
            EXIT_FIELDS
                .iter()
                .find_map(|name| fields.get(*name)?.as_i64())
        };
        let field_exit =
            exit(&fields).or_else(|| exit(fields.get("metadata").and_then(Value::as_object)?));
        let mut values: Vec<&str> = Vec::new();
        for value in fields.values() {
            match value {
                Value::String(text) => values.push(text),
                Value::Object(nested) => values.extend(nested.values().filter_map(Value::as_str)),
                _ => {}
            }
        }
        Some(Self {
            values: Some(values.join("\n")),
            raw,
            flagged,
            field_exit,
        })
    }

    /// The text whose lines are read.
    fn text(&self) -> &str {
        self.values.as_deref().unwrap_or(&self.raw)
    }

    /// Whether the harness ended or refused the command itself, which
    /// leaves the command's own outcome unknown: the result carries the
    /// harness's error mark and closes with one of [`HARNESS_ENDED`], is
    /// [`HARNESS_BLOCKED`] or `Tool <name> not found` as its whole text, or
    /// opens with [`HARNESS_INVALID`].
    fn harness_decided(&self) -> bool {
        if !self.flagged {
            return false;
        }
        let text = self.raw.trim();
        let missing = text
            .strip_prefix("Tool ")
            .and_then(|rest| rest.strip_suffix(" not found"))
            .is_some_and(|name| !name.is_empty() && !name.contains(char::is_whitespace));
        let ended = text.lines().next_back().is_some_and(|last| {
            HARNESS_ENDED
                .iter()
                .any(|mark| last.trim().starts_with(mark))
        });
        ended || text.starts_with(HARNESS_INVALID) || text == HARNESS_BLOCKED || missing
    }
}

/// What a command's result reports.
///
/// The tool's own status decides first. The harness's error mark is a
/// failure, and so is a nonzero exit code that the result states: a field
/// of its JSON object, else a status line (see [`stated_exit_code`]). For a
/// result the harness marks, the last non-empty line alone is read for the
/// code, which is where the harness states it. For any other result the
/// lines read are the first non-empty line, when it ends within the
/// output's first [`MAX_FIRST_LINE_BYTES`], and the last
/// [`EXIT_CODE_CLOSING_LINES`] non-empty lines; the last of them that states
/// a code decides.
///
/// A result without that status, or with status 0, is the result of a
/// command line whose last command passed. Three grounds make it a failure,
/// each for a stated reason. The exit status the command line itself
/// printed (see [`printed_status`]) is the shell's report of the command
/// before the `echo`, so it has the tool status's strength: nonzero is a
/// failure and zero a pass. The shell's [`MISSING_COMMAND`] line reports a
/// program that did not run, whose status a later command of the line
/// replaced. A failure line, as capture reads one in an output block, is a
/// failure where the command line hides an exit status (see
/// [`hides_status`]): the tool's status then covers the line's last command
/// alone, and the output is the only report of the others. A failure line
/// in the output of a single command whose status is 0, such as a search
/// through a log, is that command's ordinary output.
///
/// The kept lines are the first lines that state an error, each failure
/// line with the line that completes it, and then the closing lines of the
/// output.
fn outcome_of(command: &str, reading: &Reading) -> Outcome {
    let text = reading.text();
    let mut start = 0;
    // Each line, trimmed, with the bytes it stands at.
    let lines: Vec<(&str, Range<usize>)> = text
        .split('\n')
        .map(|line| {
            let span = start..start + line.len();
            start = span.end + 1;
            (line.trim(), span)
        })
        .collect();
    let texts: Vec<&str> = lines.iter().map(|(line, _)| *line).collect();
    let shown = |line: &str| !line.is_empty() && printable(line);
    let stated = || {
        let mut closing = texts.iter().rev().filter(|line| !line.is_empty());
        if reading.flagged {
            return closing.next().and_then(|line| stated_exit_code(line));
        }
        let first = lines
            .iter()
            .find(|(line, _)| !line.is_empty())
            .filter(|(_, span)| span.end <= MAX_FIRST_LINE_BYTES);
        closing
            .take(EXIT_CODE_CLOSING_LINES)
            .find_map(|line| stated_exit_code(line))
            .or_else(|| first.and_then(|(line, _)| stated_exit_code(line)))
    };
    let code = reading.field_exit.or_else(stated).filter(|code| *code != 0);
    let mut fails = false;
    let mut missing = false;
    let mut errors: Vec<usize> = Vec::new();
    for (index, line) in texts.iter().enumerate().filter(|(_, line)| shown(line)) {
        let failure = is_output_failure_line(line);
        fails |= failure;
        missing |= line.contains(MISSING_COMMAND);
        let completing = index > 0
            && is_output_failure_line(texts[index - 1])
            && completes_failure_line(texts[index - 1], line);
        if errors.len() < MAX_ERROR_LINES && (failure || completing || states_error(line)) {
            errors.push(index);
        }
    }
    let (exit_code, evidence) = if reading.flagged || code.is_some() {
        (code, Evidence::ToolStatus)
    } else {
        match printed_status(command, &texts) {
            Some(0) => return Outcome::Passed,
            Some(printed) => (Some(printed), Evidence::PrintedStatus),
            None if missing => (None, Evidence::ShellReport),
            None if fails && hides_status(command) => (None, Evidence::FailureLines),
            None => return Outcome::Passed,
        }
    };
    let mut closing: Vec<usize> = (0..texts.len())
        .rev()
        .filter(|index| shown(texts[*index]))
        .take(MAX_CLOSING_LINES)
        .collect();
    closing.reverse();
    let mut kept: Vec<Kept> = Vec::new();
    let chosen = errors
        .into_iter()
        .map(|index| (index, false))
        .chain(closing.into_iter().map(|index| (index, true)));
    for (index, closing) in chosen {
        let (line, span) = &lines[index];
        let text = clipped(line, MAX_LINE_BYTES);
        // A closing line whose text an earlier kept line holds marks that
        // line, so each text is kept once.
        match kept.iter_mut().find(|kept| kept.text == text) {
            Some(earlier) => earlier.closing |= closing,
            None => kept.push(Kept {
                text,
                closing,
                span: span.clone(),
            }),
        }
    }
    Outcome::Failed(Failure {
        exit_code,
        evidence,
        lines: kept,
    })
}

/// What a record keeps of the output `reading` holds, and the byte ranges
/// of that output's text that the kept path and error name stay out of.
///
/// The rule is source capture's rule for a tool span
/// ([`kernel::exact_text_refusal`]): text in which the redactor finds
/// nothing is kept as it is, and text with a finding is withheld. The text
/// the harness carries is scanned, and for a JSON object its string values
/// too, which the object's escapes hide from a scan of its text. With a
/// finding the excerpt is withheld, and the ranges are the redactor's
/// findings in a plain text, and the whole text of a JSON object, whose
/// findings stand at other offsets. When a scan does not finish, the ranges
/// cover the whole text.
fn excerpt_of(reading: &Reading) -> (Excerpt, Vec<Range<usize>>) {
    use kernel::ArtifactErrorKind::ExactBytesRewritten;
    let whole = || std::iter::once(0..reading.text().len()).collect::<Vec<_>>();
    let refusals = [Some(reading.raw.as_str()), reading.values.as_deref()]
        .into_iter()
        .flatten()
        .filter_map(kernel::exact_text_refusal)
        .collect::<Vec<_>>();
    if refusals.is_empty() {
        return (Excerpt::Kept, Vec::new());
    }
    if refusals
        .iter()
        .any(|refusal| *refusal != ExactBytesRewritten)
    {
        return (Excerpt::Unscanned, whole());
    }
    if reading.values.is_some() {
        return (Excerpt::Finding, whole());
    }
    match context_core::redaction::redact_windowed_durable_text(&reading.raw, usize::MAX) {
        Ok(redaction) => (
            Excerpt::Finding,
            redaction
                .detections
                .iter()
                .map(|finding| finding.offset..finding.offset + finding.length)
                .collect(),
        ),
        Err(_) => (Excerpt::Unscanned, whole()),
    }
}

/// The source location a line names: its first word that is a file path
/// with an extension and with a directory, a position, or a test's name.
/// The path is ASCII letters, digits, and `_`, `.`, `/`, `-`; the position
/// after it is `:` or `(` and then digits, `:`, and `,`; a test's name
/// follows `::` in letters, digits, `_`, and `:`. Quotes, brackets, and
/// closing punctuation around the word are left out.
fn named_path(line: &str) -> Option<&str> {
    line.split_whitespace().find_map(|word| {
        let word = word
            .trim_start_matches(['"', '\'', '`', '(', '[', '<'])
            .trim_end_matches(['"', '\'', '`', ']', '>', ',', ';', ':', '.']);
        let word = match word.contains('(') {
            true => word,
            false => word.trim_end_matches(')'),
        };
        let path_end = word.find([':', '(']).unwrap_or(word.len());
        let (path, rest) = word.split_at(path_end);
        let file = path.rsplit('/').next()?;
        let (stem, extension) = file.rsplit_once('.')?;
        let path_shaped = path
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_./-".contains(&byte));
        let extension_shaped = (1..=5).contains(&extension.len())
            && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
            && extension.bytes().any(|byte| byte.is_ascii_alphabetic());
        let rest_shaped = match rest.strip_prefix("::") {
            Some(test) => {
                !test.is_empty()
                    && test
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"_:".contains(&byte))
            }
            None => rest
                .bytes()
                .skip(1)
                .all(|byte| byte.is_ascii_digit() || b":,)".contains(&byte)),
        };
        let located = path.contains('/') || rest.len() > 1;
        (path_shaped
            && !stem.is_empty()
            && extension_shaped
            && rest_shaped
            && located
            && word.len() <= MAX_TOKEN_BYTES)
            .then_some(word)
    })
}

/// The error a line names: its first word that is a type name ending in
/// `Error` or `Exception` (`ModuleNotFoundError`,
/// `java.lang.NullPointerException`) or a diagnostic code of one to four
/// ASCII capitals and two to six digits (`E0308`, `TS2307`).
fn named_error(line: &str) -> Option<&str> {
    line.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '.'))
        .map(|word| word.trim_matches('.'))
        .find(|word| {
            let typed = word
                .strip_suffix("Error")
                .or_else(|| word.strip_suffix("Exception"))
                .is_some_and(|stem| stem.starts_with(|ch: char| ch.is_ascii_alphabetic()));
            let letters = word.bytes().take_while(u8::is_ascii_uppercase).count();
            let digits = word.len() - letters;
            let coded = (1..=4).contains(&letters)
                && (2..=6).contains(&digits)
                && word.bytes().skip(letters).all(|byte| byte.is_ascii_digit());
            (typed || coded) && word.len() <= MAX_TOKEN_BYTES
        })
}

/// The time as the record shows it, in UTC to the millisecond, so a reader
/// can order two failures of one second.
fn utc_time(at_ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(at_ms)
        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .unwrap_or_default()
}

/// The record's text within [`MAX_SUMMARY_BYTES`], with room for
/// [`RESOLVED_NOTE`]: the time of the result and what it states, the
/// command's name, the path and the error the output names, and the kept
/// lines or the sentence that says why they are withheld and where the
/// output stands. The time stands in the first line, so a reader of a search
/// result that is cut still orders the failures of a project. The closing
/// lines are kept whole, and the lines that state an error fill the bytes
/// that remain.
fn summary_of(unit: &Observed, session: &str, failure: &Failure) -> String {
    let stated = match (failure.evidence, failure.exit_code) {
        (Evidence::ToolStatus, Some(code)) => format!("the command failed with exit code {code}"),
        (Evidence::ToolStatus, None) => {
            "the command failed and the tool reported an error".to_owned()
        }
        (Evidence::PrintedStatus, code) => format!(
            "the command failed: its command line printed exit status {}",
            code.unwrap_or_default()
        ),
        (Evidence::ShellReport, _) => {
            "the command failed: the shell found no program of a name in its command line"
                .to_owned()
        }
        (Evidence::FailureLines, _) => {
            "the output holds failure lines and the command line hides its exit status from the tool"
                .to_owned()
        }
    };
    let tool = clipped(&collapsed(&unit.tool), MAX_TOOL_BYTES);
    let command = match unit.command.shown.as_str() {
        "" => "[left out]",
        shown => shown,
    };
    let mut summary = format!(
        "Tool failure recorded from the result of {tool} at {}: {stated}.\nCommand: {command}",
        utc_time(unit.at_ms)
    );
    if let Some(path) = &unit.path {
        summary.push_str(&format!("\nFailing path: {path}"));
    }
    if let Some(error) = &unit.error {
        summary.push_str(&format!("\nError: {error}"));
    }
    let limit = MAX_SUMMARY_BYTES - RESOLVED_NOTE.len();
    if unit.excerpt != Excerpt::Kept {
        summary.push_str(&format!(
            "\nOutput text withheld: {}. The whole output stands in the result of tool call {} of session {} while that session's history holds it.",
            unit.excerpt.reason(),
            clipped(&unit.call, MAX_ID_BYTES),
            clipped(session, MAX_ID_BYTES),
        ));
        return clipped(&summary, limit);
    }
    summary.push_str("\nOutput lines:");
    let cost = |line: &Kept| line.text.len() + 1;
    let mut reserved: usize = failure
        .lines
        .iter()
        .filter(|line| line.closing)
        .map(cost)
        .sum();
    for line in &failure.lines {
        if line.closing {
            reserved -= cost(line);
        }
        if summary.len() + cost(line) + reserved <= limit {
            summary.push('\n');
            summary.push_str(&line.text);
        }
    }
    clipped(&summary, limit)
}

/// Command results of a transform window that wait for the detached task.
///
/// Session deletion removes the session's memo entry under the capture
/// publication gate. The task judges each result under the same gate while
/// its pinned entry is current, so a result handed off before a delete
/// writes nothing after it.
pub(crate) struct FailureCheckpoint {
    kernel: Arc<kernel_routes::KernelOpenCoordinator>,
    commit_gate: Arc<Mutex<()>>,
    project: ProjectBinding,
    pin: MemoPin,
    pending: Vec<Pending>,
    /// The memo's names of the calls whose messages pass
    /// [`MAX_HELD_WINDOW_BYTES`] on their own. The task settles them unread.
    unheld: Vec<u64>,
}

/// Holds a session's memo entry and the charge for the window messages of
/// one checkpoint; dropping it, on any path, admits the session's next
/// checkpoint and returns the charge.
struct MemoPin {
    memo: Arc<Mutex<FailureMemo>>,
    session: String,
    epoch: u64,
    window_bytes: usize,
}

impl Drop for MemoPin {
    fn drop(&mut self) {
        let mut memo = self.memo.lock().expect("failure memo mutex");
        memo.release(&self.session, self.epoch);
        memo.held_window_bytes = memo.held_window_bytes.saturating_sub(self.window_bytes);
    }
}

impl HandlerCore {
    /// References to the command results of the window that this session's
    /// memo has not judged, for a primary Pi session whose configuration
    /// permits memory capture and consents to source capture: a record is a
    /// durable copy of part of a tool result, which source capture retains
    /// under that consent. The reply path walks the window's blocks and
    /// copies no output: the detached task reads the text. A result whose
    /// call id two tool calls of the window share is left out, since the id
    /// names neither call alone. The pass considers the last
    /// [`MEMO_CALLS_PER_SESSION`] command results of the window and hands
    /// off the earliest [`MAX_RESULTS_PER_PASS`] unjudged ones whose messages
    /// fit the room left of [`MAX_HELD_WINDOW_BYTES`]; a result whose
    /// messages pass that bound on their own is handed off by name alone
    /// and settled unread, so the results after it are reached. One
    /// checkpoint of a session is in flight at a time, and
    /// [`MAX_CHECKPOINTS`] over all sessions. Returns `None` when the window
    /// holds no unseen result, so the caller spawns no task.
    pub(crate) fn failure_checkpoint(
        &self,
        binding: &SessionBinding,
        request: &crate::transform::TransformRequest,
    ) -> Option<FailureCheckpoint> {
        let pi = SerializerProfile::parse(&request.serializer_profile)
            .filter(|profile| *profile == SerializerProfile::Pi)?;
        let consents = crate::source_capture::consents(
            &binding.config,
            request.is_subagent,
            pi,
            &binding.harness,
        );
        if !consents || !capture_enabled(binding) {
            return None;
        }
        let scope = binding.kernel_project.digest();
        let mut memo = self.failure_memo.lock().expect("failure memo mutex");
        if memo.busy(&binding.session) {
            return None;
        }
        let mut calls: HashMap<&str, (usize, usize)> = HashMap::new();
        let (mut named, mut shared) = (HashSet::new(), HashSet::new());
        let mut considered = VecDeque::new();
        for (index, message) in request.messages.iter().enumerate() {
            for (block_index, block) in message.ck.content().iter().enumerate() {
                match block.kind() {
                    BlockKind::ToolCall { id, input, .. } => {
                        if !named.insert(id.as_str()) {
                            shared.insert(id.as_str());
                        }
                        if command_field(input).is_some() {
                            calls.insert(id.as_str(), (index, block_index));
                        }
                    }
                    BlockKind::ToolResult { id, .. } => {
                        let Some(&(call, call_block)) = calls.get(id.as_str()) else {
                            continue;
                        };
                        considered.push_back((id, call, call_block, index, block_index));
                        if considered.len() > MEMO_CALLS_PER_SESSION {
                            considered.pop_front();
                        }
                    }
                    _ => {}
                }
            }
        }
        considered.retain(|(id, ..)| !shared.contains(id.as_str()));
        let bytes = |message: usize| ingress_message_retained_bytes(&request.messages[message]);
        let room = MAX_HELD_WINDOW_BYTES.saturating_sub(memo.held_window_bytes);
        let mut pending = Vec::new();
        let mut unheld = Vec::new();
        let mut charged = HashSet::new();
        let mut window_bytes = 0;
        let mut full = false;
        for &(id, call, call_block, result, result_block) in &considered {
            let digest = call_digest(scope, id);
            full |= pending.len() == MAX_RESULTS_PER_PASS;
            if full || memo.has_seen(&binding.session, digest) {
                continue;
            }
            // Each message is counted once, whatever this checkpoint has
            // charged already.
            let alone = bytes(call) + usize::from(result != call) * bytes(result);
            if alone > MAX_HELD_WINDOW_BYTES {
                unheld.push(digest);
                continue;
            }
            let cost: usize = [call, result]
                .into_iter()
                .enumerate()
                .filter(|(nth, message)| {
                    (*nth == 0 || *message != call) && !charged.contains(message)
                })
                .map(|(_, message)| bytes(message))
                .sum();
            if window_bytes + cost > room {
                full = true;
                continue;
            }
            charged.extend([call, result]);
            window_bytes += cost;
            pending.push(Pending {
                call: Arc::clone(&request.messages[call]),
                call_block,
                result: Arc::clone(&request.messages[result]),
                result_block,
                digest,
            });
        }
        if pending.is_empty() && unheld.is_empty() {
            return None;
        }
        let epoch = memo.pin(&binding.session)?;
        memo.held_window_bytes += window_bytes;
        Some(FailureCheckpoint {
            kernel: Arc::clone(&self.kernel),
            commit_gate: Arc::clone(&self.capture_commit_gate),
            project: binding.kernel_project.clone(),
            pin: MemoPin {
                memo: Arc::clone(&self.failure_memo),
                session: binding.session.clone(),
                epoch,
                window_bytes,
            },
            pending,
            unheld,
        })
    }
}

impl Pending {
    /// Reads the command and the result's outcome. `None` for a result that
    /// changes no record: one the harness denied, refused, or ended itself,
    /// one that is no text or passes [`MAX_OUTPUT_BYTES`], one whose command
    /// line passes that bound or fails the redactor's scan, and one whose
    /// message carries no creation time, which orders it among the
    /// command's runs.
    fn observe(&self, scope: &str) -> Option<Observed> {
        let BlockKind::ToolCall { name, input, .. } =
            self.call.ck.content().get(self.call_block)?.kind()
        else {
            return None;
        };
        let BlockKind::ToolResult { id, output, .. } =
            self.result.ck.content().get(self.result_block)?.kind()
        else {
            return None;
        };
        let at_ms = self.result.ck.meta.created_at_ms?;
        let line = command_of(input).filter(|line| line.len() <= MAX_OUTPUT_BYTES)?;
        let reading =
            Reading::of_output(&output.kind).filter(|reading| !reading.harness_decided())?;
        let command = command_named(scope, &line)?;
        let outcome = outcome_of(&line, &reading);
        let mut unit = Observed {
            call: id.clone(),
            tool: name.clone(),
            command,
            at_ms,
            outcome,
            excerpt: Excerpt::Kept,
            path: None,
            error: None,
        };
        if let Outcome::Failed(failure) = &unit.outcome {
            let (excerpt, findings) = excerpt_of(&reading);
            // A path and an error name come from the kept lines that hold
            // no finding.
            let clear = |line: &&Kept| {
                findings
                    .iter()
                    .all(|finding| finding.end <= line.span.start || line.span.end <= finding.start)
            };
            let text = reading.text();
            let named = |name: fn(&str) -> Option<&str>| {
                failure
                    .lines
                    .iter()
                    .filter(clear)
                    .find_map(|line| name(&text[line.span.clone()]))
                    .map(str::to_owned)
            };
            unit.path = named(named_path);
            unit.error = named(named_error);
            unit.excerpt = excerpt;
        }
        Some(unit)
    }
}

/// The deadline of the waits for a Kernel connection of one judgment.
fn kernel_budget() -> EvalBudget {
    EvalBudget::new(
        Some(Instant::now() + COMMIT_WAIT),
        Arc::new(AtomicBool::new(false)),
    )
}

impl FailureCheckpoint {
    /// Text scans, Kernel reads, and commits run on the blocking pool.
    /// Returns the number of rows written.
    pub(crate) async fn run(self) -> usize {
        kernel_routes::blocking(move || self.record())
            .await
            .unwrap_or(0)
    }

    /// Judges the results in window order. Each result is read ahead of the
    /// capture publication gate and judged under it, while the session's
    /// memo entry is the one this checkpoint pinned; the memo learns a call
    /// once its judgment has ended. A Kernel that is busy or failing, and a
    /// row another writer changed under the judgment, end the pass with the
    /// call unlearned, so the session's next request carries it and the
    /// results after it again, in the order they ran. Any other refusal of
    /// the Kernel settles the call: the same result would meet it again.
    fn record(self) -> usize {
        let Self {
            kernel,
            commit_gate,
            project,
            pin,
            pending,
            unheld,
        } = self;
        let Ok(kernel) = kernel.kernel_store() else {
            return 0;
        };
        let writer = Writer {
            kernel: &kernel,
            project: &project,
            scope: project.digest(),
            session: &pin.session,
        };
        let memo = || pin.memo.lock().expect("failure memo mutex");
        if !unheld.is_empty() {
            let _gate = commit_gate.lock().expect("capture publication mutex");
            let mut memo = memo();
            for call in &unheld {
                memo.learn(&pin.session, pin.epoch, *call);
            }
        }
        let mut written = 0;
        for result in &pending {
            let unit = result.observe(writer.scope);
            // The gate orders the Kernel reads, the commit, and the memo
            // update of every checkpoint, and session deletion takes it too.
            let _gate = commit_gate.lock().expect("capture publication mutex");
            if !memo().current(&pin.session, pin.epoch) {
                break;
            }
            let judged = match &unit {
                Some(unit) => writer.judge(unit),
                None => Ok(false),
            };
            match judged {
                Ok(wrote) => written += usize::from(wrote),
                Err(error) => {
                    eprintln!("daemon: tool failure record left unwritten: {error}");
                    use kernel::KernelError::{
                        Busy, Conflict, Deadline, Fault, FenceLost, Held, Io, NotFound,
                    };
                    if matches!(
                        error,
                        Deadline | Busy | Held | Io | FenceLost | Fault | Conflict | NotFound
                    ) {
                        break;
                    }
                }
            }
            memo().learn(&pin.session, pin.epoch, result.digest);
        }
        written
    }
}

/// The id of the record a tool call writes. Each part is hashed behind its
/// length, so two calls share an id only when project, session, and call id
/// are equal.
fn record_id(scope: &str, session: &str, call: &str) -> String {
    let mut hash = Sha256::new();
    for part in [FAILURE_SOURCE_ID, scope, session, call] {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part.as_bytes());
    }
    format!("mem_{:x}", hash.finalize())[..36].to_owned()
}

/// The operation keys of the commits that write a session's records in a
/// project: `<number>:<record id>`, with the number the first 64 bits of the
/// session id's SHA-256 in decimal. The Kernel's commit log keeps each key
/// for the store's lifetime, so the keys between the returned bounds count
/// the records the session has written, also the ones a later record
/// superseded.
fn session_keys(session: &str) -> (String, String) {
    let digest = Sha256::digest(session.as_bytes());
    let number = digest[..8]
        .iter()
        .fold(0u64, |sum, byte| sum << 8 | u64::from(*byte));
    (format!("{number}:"), format!("{number};"))
}

/// The run a stored record is ordered by, read from its rationale: the
/// failed run, or the passing run once the record is resolved.
struct Stored {
    rationale: serde_json::Map<String, Value>,
    resolved: bool,
    at_ms: i64,
}

impl Stored {
    /// `None` for a rationale that holds no run, which the recorder never
    /// writes.
    fn read(rationale: &str) -> Option<Self> {
        let Ok(Value::Object(rationale)) = serde_json::from_str(rationale) else {
            return None;
        };
        let resolved = rationale.get("state").and_then(Value::as_str) == Some("resolved");
        let field = if resolved { "resolved_at_ms" } else { "at_ms" };
        let at_ms = rationale.get(field).and_then(Value::as_i64)?;
        Some(Self {
            rationale,
            resolved,
            at_ms,
        })
    }
}

/// The text and the rationale of the record of a failed run of `session`.
fn failed_record(session: &str, unit: &Observed, failure: &Failure) -> kernel::DecisionPayload {
    let mut rationale = serde_json::Map::new();
    let text = match unit.excerpt {
        Excerpt::Kept => "kept",
        Excerpt::Finding | Excerpt::Unscanned => "withheld",
    };
    for (name, value) in [
        ("source", json!(FAILURE_SOURCE_ID)),
        ("session", json!(session)),
        ("call", json!(clipped(&unit.call, MAX_ID_BYTES))),
        (
            "tool",
            json!(clipped(&collapsed(&unit.tool), MAX_TOOL_BYTES)),
        ),
        ("command", json!(unit.command.shown)),
        ("digest", json!(unit.command.digest)),
        ("state", json!("failed")),
        ("evidence", json!(failure.evidence.name())),
        ("output_text", json!(text)),
        ("at_ms", json!(unit.at_ms)),
        ("at", json!(utc_time(unit.at_ms))),
    ] {
        rationale.insert(name.to_owned(), value);
    }
    for (name, value) in [
        ("exit_code", failure.exit_code.map(Value::from)),
        ("path", unit.path.clone().map(Value::from)),
        ("error", unit.error.clone().map(Value::from)),
    ] {
        if let Some(value) = value {
            rationale.insert(name.to_owned(), value);
        }
    }
    kernel::DecisionPayload {
        summary: summary_of(unit, session, failure),
        rationale: Value::Object(rationale).to_string(),
    }
}

/// The live record of a command rewritten as resolved by a passing run of
/// `session`: its stored text with [`RESOLVED_NOTE`] at the end of its first
/// line, and the passing run in the rationale. The record is ordered by the
/// passing run from then on.
fn resolved_record(
    session: &str,
    unit: &Observed,
    stored: kernel::DecisionPayload,
    mut rationale: serde_json::Map<String, Value>,
) -> kernel::DecisionPayload {
    let line = stored.summary.find('\n').unwrap_or(stored.summary.len());
    let (stated, rest) = stored.summary.split_at(line);
    for (name, value) in [
        ("state", json!("resolved")),
        ("resolved_session", json!(session)),
        ("resolved_call", json!(clipped(&unit.call, MAX_ID_BYTES))),
        ("resolved_at_ms", json!(unit.at_ms)),
        ("resolved_at", json!(utc_time(unit.at_ms))),
    ] {
        rationale.insert(name.to_owned(), value);
    }
    kernel::DecisionPayload {
        summary: clipped(&format!("{stated}{RESOLVED_NOTE}{rest}"), MAX_SUMMARY_BYTES),
        rationale: Value::Object(rationale).to_string(),
    }
}

/// The Kernel, the project, and the session one checkpoint writes for.
struct Writer<'a> {
    kernel: &'a kernel::KernelStore,
    project: &'a ProjectBinding,
    /// The project's digest, which scopes a record's id and a command's
    /// lineage.
    scope: &'a str,
    session: &'a str,
}

impl Writer<'_> {
    /// Writes what one command result calls for and returns whether a row
    /// was written. The caller holds the capture publication gate, so no
    /// other checkpoint writes between the reads and the commit. Every wait
    /// for a Kernel connection ends at one deadline.
    ///
    /// A call whose record the Kernel holds writes nothing. The command's
    /// record is the newest row of its lineage. A failure writes the first
    /// row of a lineage, a row after a retired one, and a row in place of a
    /// live one whose run it follows. A pass rewrites a live, unresolved row
    /// whose run it follows as resolved. A run follows a record when its
    /// result's creation time is greater than the time of the record's run.
    /// Either writes while the session has written fewer than
    /// [`MAX_RECORDS_PER_SESSION`] records by the Kernel's commit log.
    fn judge(&self, unit: &Observed) -> Result<bool, kernel::KernelError> {
        let budget = kernel_budget();
        let id = record_id(self.scope, self.session, &unit.call);
        let (_, held) = self
            .kernel
            .object_states_within_budget(&budget, std::slice::from_ref(&id))?;
        if held.first().is_some_and(Option::is_some) {
            return Ok(false);
        }
        let lineage = format!("{FAILURE_SOURCE_ID}:{}", unit.command.digest);
        let (tip, newest) = self.kernel.newest_decision_on_lineage_within_budget(
            &budget,
            CAPTURE_SOURCE_KIND,
            &lineage,
        )?;
        let (live, revision) = match &newest {
            None => (None, Some(1)),
            // The route keeps a successor on its predecessor's lineage, so
            // the newest row of a lineage has none.
            Some(row) if row.superseded_by.is_some() => return Ok(false),
            Some(row) => (
                row.invalidated_commit_seq.is_none().then_some(row),
                row.source_revision.checked_add(1),
            ),
        };
        let Some(revision) = revision else {
            return Ok(false);
        };
        let stored = match live {
            Some(row) => {
                let ids = std::slice::from_ref(&row.object_id);
                let decision = self
                    .kernel
                    .decisions_for_objects_as_of_within_budget(ids, tip, &budget)?
                    .pop()
                    .ok_or(kernel::KernelError::NotFound)?;
                match Stored::read(&decision.payload.rationale) {
                    Some(stored) if unit.at_ms > stored.at_ms => Some((decision.payload, stored)),
                    _ => return Ok(false),
                }
            }
            None => None,
        };
        let payload = match (&unit.outcome, stored) {
            (Outcome::Failed(failure), _) => failed_record(self.session, unit, failure),
            (Outcome::Passed, Some((payload, stored))) if !stored.resolved => {
                resolved_record(self.session, unit, payload, stored.rationale)
            }
            (Outcome::Passed, _) => return Ok(false),
        };
        if self.written(&budget)? >= MAX_RECORDS_PER_SESSION {
            return Ok(false);
        }
        let target = live.map(|row| row.object_id.as_str());
        self.commit(id, (&lineage, revision), target, payload, &budget)
    }

    /// The number of records the session has written in the project: the
    /// commits of this writer that the Kernel's log holds under the
    /// session's operation keys, counted up to one past the session's
    /// bound.
    fn written(&self, budget: &EvalBudget) -> Result<usize, kernel::KernelError> {
        let (from, to) = session_keys(self.session);
        let key = |part: &str| {
            self.project
                .operation_key(FAILURE_SOURCE_ID, part)
                .ok_or(kernel::KernelError::InvalidInput)
        };
        self.kernel.commit_count_in_key_range_within_budget(
            budget,
            FAILURE_SOURCE_ID,
            (&key(&from)?, &key(&to)?),
            MAX_RECORDS_PER_SESSION + 1,
        )
    }

    /// Commits the record `id` at `revision` of `lineage`, in place of the
    /// live row `target` when one is given, as a labeled row. The Kernel
    /// stores the text and the rationale through the durable-text redactor;
    /// a payload the redactor would change writes no record, so the stored
    /// row is the row composed here. A commit the Kernel answers from an
    /// earlier receipt wrote nothing now.
    fn commit(
        &self,
        id: String,
        (lineage, revision): (&str, i64),
        target: Option<&str>,
        payload: kernel::DecisionPayload,
        budget: &EvalBudget,
    ) -> Result<bool, kernel::KernelError> {
        let project = self.project;
        let clean = [&payload.summary, &payload.rationale]
            .into_iter()
            .all(|text| kernel::exact_text_refusal(text).is_none());
        if !clean {
            return Ok(false);
        }
        let (session_key, _) = session_keys(self.session);
        let intent = kernel::CommitIntent {
            producer: FAILURE_SOURCE_ID.into(),
            operation_key: project
                .operation_key(FAILURE_SOURCE_ID, &format!("{session_key}{id}"))
                .ok_or(kernel::KernelError::InvalidInput)?,
            request_digest: format!(
                "{:x}",
                Sha256::digest(format!("{}\n{}", payload.summary, payload.rationale).as_bytes())
            ),
            actor: "agent:tool_failure".into(),
            cause: "record of a failed command".into(),
        };
        let spec = kernel::DecisionSpec {
            decision_id: format!("dec_{}", &id[4..]),
            object_id: id.clone(),
            domain_id: "memory".into(),
            proposition_id: None,
            scope_id: Some(project.scope_id()),
            anchor_id: None,
            evidence_id: None,
            decision_kind: FAILURE_CATEGORY.into(),
            payload,
            source_kind: CAPTURE_SOURCE_KIND.into(),
            source_id: lineage.to_owned(),
            source_revision: revision,
            sensitivity: kernel::Sensitivity::Normal,
        };
        let receipt = self
            .kernel
            .commit_within_budget(budget, intent, |envelope| {
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
                match target {
                    Some(target) => envelope.supersede_decision(target, spec)?,
                    None => envelope.insert_decision(spec)?,
                };
                kernel_routes::commit::admit_capture(envelope, &id, None)?;
                Ok(id.clone())
            })?;
        Ok(!receipt.replayed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCOPE: &str = "project";

    fn reading(text: &str, flagged: bool) -> Reading {
        let output = match flagged {
            true => OutputKind::ErrorText { text: text.into() },
            false => OutputKind::Text { text: text.into() },
        };
        Reading::of_output(&output).unwrap()
    }

    /// The outcome of a text result of `command`.
    fn judged(command: &str, text: &str, flagged: bool) -> Outcome {
        outcome_of(command, &reading(text, flagged))
    }

    /// The ground, the exit code, and the kept lines of a failed result.
    fn failed(
        command: &str,
        text: &str,
        flagged: bool,
    ) -> Option<(Evidence, Option<i64>, Vec<String>)> {
        match judged(command, text, flagged) {
            Outcome::Passed => None,
            Outcome::Failed(failure) => Some((
                failure.evidence,
                failure.exit_code,
                failure.lines.into_iter().map(|line| line.text).collect(),
            )),
        }
    }

    fn named(command: &str) -> Command {
        command_named(SCOPE, command).unwrap()
    }

    /// A credential under a name the redactor reads as a secret's, joined at
    /// run time so no line of this file holds one.
    fn keyed_credential() -> (String, String) {
        let value = ["v4lu3", "-of-", "the", "-fixture", "-5524"].concat();
        (format!("{}={value}", ["api_", "tok", "en"].concat()), value)
    }

    #[test]
    fn a_command_is_read_from_the_call_and_named_by_its_program_and_first_plain_operand() {
        assert_eq!(
            command_of(&json!({ "command": "cargo test" })).as_deref(),
            Some("cargo test")
        );
        assert_eq!(
            command_of(&json!({ "command": "  ", "cmd": ["npm", "run", "build"] })).as_deref(),
            Some("npm run build")
        );
        for input in [
            json!({ "path": "src/lib.rs" }),
            json!({ "command": "" }),
            json!({ "command": ["ls", 3] }),
            json!({ "command": 7 }),
        ] {
            assert_eq!(command_of(&input), None, "{input}");
        }
        for (command, shown) in [
            ("ls", "ls"),
            ("cargo test", "cargo test"),
            ("cargo test -p api", "cargo test [arguments left out]"),
            (
                "  cargo   test  -p  api ",
                "cargo test [arguments left out]",
            ),
            ("npm run build", "npm run [arguments left out]"),
            (
                "python3 scripts/migrate.py --db main",
                "python3 scripts/migrate.py [arguments left out]",
            ),
            ("./gradlew check", "./gradlew check"),
            // A flag follows the program, so no operand is shown.
            ("psql -p 5433 -h db", "psql [arguments left out]"),
            // A URL, an assignment, a quoted word, and an expansion are no
            // plain words.
            ("curl https://example.com/hook", "curl [arguments left out]"),
            (
                "git clone git@example.com:team/api.git",
                "git clone [arguments left out]",
            ),
            ("env MODE=fast make", "env [arguments left out]"),
            ("echo \"two words\" more", "echo [arguments left out]"),
            ("echo $HOME", "echo [arguments left out]"),
            ("make; ls", "make [arguments left out]"),
            ("make | tail", "make [arguments left out]"),
            ("\"my tool\" run", ""),
            ("(cd api && make)", ""),
            ("$TOOL run", ""),
        ] {
            assert_eq!(named(command).shown, shown, "{command}");
        }
        let long = "x".repeat(MAX_WORD_BYTES + 1);
        assert_eq!(named(&long).shown, "");
        assert_eq!(
            named(&format!("make {long}")).shown,
            "make [arguments left out]"
        );
    }

    #[test]
    fn a_command_s_lineage_leaves_out_its_directory_its_assignments_and_a_found_credential() {
        let digest = |command: &str| named(command).digest;
        let plain = digest("cargo test -p api");
        assert_eq!(plain.len(), 32);
        for same in [
            "cd services && cargo test -p api",
            "cd 'my services'; RUST_LOG=debug cargo  test -p api",
            "RUST_LOG=debug TZ=UTC cargo test -p api",
        ] {
            assert_eq!(digest(same), plain, "{same}");
        }
        for other in [
            "cargo test -p web",
            "cargo test",
            "cargo test -p api && cd ..",
            "cd services || cargo test -p api",
        ] {
            assert_ne!(digest(other), plain, "{other}");
        }
        assert_ne!(
            command_named("another project", "cargo test -p api")
                .unwrap()
                .digest,
            plain
        );
        // A step or an assignment that ends the command stays.
        assert_eq!(named("cd services").shown, "cd services");
        assert_ne!(digest("cd services &&"), digest("cd services"));
        // Two command lines that differ in a credential the redactor finds
        // are one command, and the credential reaches no word shown.
        let (assignment, value) = keyed_credential();
        let rotated = assignment.replace(&value, "an0ther-fixture-value-88");
        let deploy = |assignment: &str| format!("deploy --{assignment} --target api");
        assert_eq!(digest(&deploy(&assignment)), digest(&deploy(&rotated)));
        assert_ne!(
            digest(&deploy(&assignment)),
            digest(&deploy(&assignment).replace("api", "web"))
        );
        for command in [deploy(&assignment), format!("deploy {assignment}")] {
            let shown = named(&command).shown;
            assert_eq!(shown, "deploy [arguments left out]", "{command}");
        }
    }

    #[test]
    fn an_exit_code_is_stated_by_a_whole_status_line() {
        for (line, code) in [
            ("exit code 1", 1),
            ("Exit code: 2", 2),
            ("[exit code 3]", 3),
            ("(exit status 4)", 4),
            ("Command exited with code 101", 101),
            ("command exited with code 5.", 5),
            ("The process exited with code 6", 6),
            ("exit_code=7", 7),
            ("Exit code: \"8\"", 8),
            ("  Exit status: 0  ", 0),
            ("Command failed with exit code 9.", 9),
            ("script failed with exit status 10", 10),
            // A runner's tag before its own status line.
            ("ELIFECYCLE  Command failed with exit code 1.", 1),
            ("ERR! Exit status 2", 2),
            ("ERROR: process exited with code 3", 3),
        ] {
            assert_eq!(stated_exit_code(line), Some(code), "{line}");
        }
        for line in [
            "worker exited with code 7",
            "child process exited with code 7",
            "the suite printed exit code 1 twice",
            "exit code 1 (ignored)",
            "processexit code 0",
            "commandexited with code 2",
            "]Exit code 0(",
            "[exit code 0",
            "exit code 0)",
            "exit code",
            "exit code -1",
            "exit code 1.5",
            "Worker failed with exit code 1",
            "elifecycle Command failed with exit code 1",
            "Build Command failed with exit code 1",
            "AB exit code 1",
            "ELIFECYCLE WARN exit code 1",
            "failed withexit code 1",
        ] {
            assert_eq!(stated_exit_code(line), None, "{line}");
        }
    }

    #[test]
    fn a_marked_result_and_a_stated_exit_code_decide_by_the_tool_s_status() {
        use Evidence::ToolStatus;
        let lines = |lines: &[&str]| lines.iter().map(|line| (*line).to_owned()).collect();
        // The harness's mark decides, also against a stated code of zero,
        // and its code is read from the last non-empty line alone.
        assert_eq!(
            failed("make", "ok\n\nCommand exited with code 2\n", true),
            Some((
                ToolStatus,
                Some(2),
                lines(&["ok", "Command exited with code 2"])
            ))
        );
        assert_eq!(
            failed("make", "exit code 3\nall done", true),
            Some((ToolStatus, None, lines(&["exit code 3", "all done"])))
        );
        assert_eq!(
            failed("make", "Exit code: 0", true),
            Some((ToolStatus, None, lines(&["Exit code: 0"])))
        );
        // An unmarked result states its code in its first line or in one of
        // its last three lines, and the last of them that states one
        // decides.
        assert_eq!(
            failed("make", "Exit code: 4\nfirst\nsecond\nthird\nfourth", false)
                .map(|failure| failure.1),
            Some(Some(4))
        );
        assert_eq!(
            failed("make", "one\nexit code 5\ntwo\nthree", false).map(|failure| failure.1),
            Some(Some(5))
        );
        assert_eq!(
            judged("make", "one\nexit code 5\ntwo\nthree\nfour", false),
            Outcome::Passed
        );
        assert_eq!(
            failed("make", "exit code 6\nwork\nexit code 7", false).map(|failure| failure.1),
            Some(Some(7))
        );
        assert_eq!(
            judged("make", "exit code 6\nwork\nexit code 0", false),
            Outcome::Passed
        );
        // The first line counts when it ends within the output's first
        // 1,024 bytes, blank lines and leading whitespace included.
        let pad = |bytes: usize| format!("{}exit code 8\nrest\nof\nthe\noutput", " ".repeat(bytes));
        let within = MAX_FIRST_LINE_BYTES - "exit code 8".len();
        assert!(matches!(
            judged("make", &pad(within), false),
            Outcome::Failed(_)
        ));
        assert_eq!(judged("make", &pad(within + 1), false), Outcome::Passed);
    }

    #[test]
    fn a_result_the_tool_reports_as_passed_fails_on_stated_grounds_alone() {
        use Evidence::{FailureLines, PrintedStatus, ShellReport};
        const ERROR: &str = "error: could not compile `api`";
        let ground = |command: &str, text: &str| {
            failed(command, text, false).map(|failure| (failure.0, failure.1))
        };
        // The exit status the command line printed decides as the tool's
        // status does.
        for (command, text, code) in [
            ("make check; echo \"exit=$?\"", "exit=2", 2),
            ("make check; echo exit=$?", "work\nexit=2\n", 2),
            ("make check; echo \"EXIT CODE: ${?}\"", "EXIT   CODE: 3", 3),
            (
                "make check; echo \"status $? reported\"",
                "status 4 reported\nbye",
                4,
            ),
            ("make check; echo $?", "work\n5", 5),
            ("make check && echo ok || echo \"rc=$?\"", "rc=6", 6),
            ("make check; echo \"exit=$?\" >&2", "exit=7", 7),
            ("(make check; echo \"exit=$?\") | cat", "exit=8", 8),
            (
                "make check | tail -n 3; echo \"exit=${PIPESTATUS[0]}\"",
                "exit=9",
                9,
            ),
            (
                "make check | tail -n 3; echo \"all: ${PIPESTATUS[@]}\"",
                "all: 0 10 0",
                10,
            ),
        ] {
            assert_eq!(
                ground(command, text),
                Some((PrintedStatus, Some(code))),
                "{command}"
            );
        }
        // A printed status of zero is a pass, whatever the output holds.
        assert_eq!(
            judged(
                "make check; echo \"exit=$?\"",
                &format!("{ERROR}\nexit=0"),
                false
            ),
            Outcome::Passed
        );
        // The command line asked for no status, or the shell left the
        // parameter as written, or the output states none at the echo's
        // place: the line is ordinary output.
        for (command, text) in [
            ("make check", "exit=2"),
            ("make check; echo 'exit=$?'", "exit=2"),
            ("make check; echo \"exit=\\$?\"", "exit=2"),
            ("make check; echo \"exit=$?\"", "exit=two"),
            ("make check; echo \"exit=$?\"", "the exit=2 line"),
            ("make check; echo $?", "2\ndone"),
            ("make check; echo \"$? $?\"", "2 2"),
            ("make check; printf 'exit=%s' $?", "exit=2"),
        ] {
            assert_eq!(ground(command, text), None, "{command}: {text}");
        }
        // The shell's report of a program it did not find.
        assert_eq!(
            ground("pnpm typecheck", "bash: line 1: pnpm: command not found"),
            Some((ShellReport, None))
        );
        // A failure line is a failure where the command line hides an exit
        // status behind a pipe, `||`, or `;`.
        for command in [
            "make check | tail -n 40",
            "make check 2>&1 | tee build.log",
            "make check || true",
            "make check; ls target",
            "cd api && make check | head -5",
        ] {
            assert_eq!(
                ground(command, ERROR),
                Some((FailureLines, None)),
                "{command}"
            );
        }
        // A single command, an `&&` chain, and a quoted or escaped `|` and
        // `;` hide none: the tool's status is the command's own.
        for command in [
            "make check",
            "grep -rn error: logs/",
            "cd api && make check",
            "make check 2>&1",
            "grep -E 'error|warning' build.log",
            "echo \"a; b\"",
            "find . -name x -exec cat {} \\;",
        ] {
            assert_eq!(ground(command, ERROR), None, "{command}");
        }
        // Without a failure line a hidden status is a pass.
        assert_eq!(
            judged(
                "make check | tail -n 40",
                "warning: unused import\nall targets built",
                false
            ),
            Outcome::Passed
        );
        // A stated status of zero is the status of the line's last command.
        assert_eq!(
            ground("make check | tail -n 40", &format!("{ERROR}\nExit code: 0")),
            Some((FailureLines, None))
        );
    }

    #[test]
    fn a_text_that_is_one_json_object_is_read_by_its_exit_field_and_its_string_values() {
        let object = json!({
            "output": "running\nerror: no such target\n",
            "metadata": { "exit_code": 2, "duration_seconds": 0.4 },
        })
        .to_string();
        let reading = reading(&object, false);
        assert_eq!(reading.field_exit, Some(2));
        assert_eq!(reading.text(), "running\nerror: no such target\n");
        assert_eq!(
            failed("make", &object, false),
            Some((
                Evidence::ToolStatus,
                Some(2),
                vec!["error: no such target".to_owned(), "running".to_owned()]
            ))
        );
        for (object, exit) in [
            (json!({ "exitCode": 3, "stderr": "boom" }), Some(3)),
            (
                json!({ "exit": 0, "metadata": { "exit_code": 4 } }),
                Some(0),
            ),
            (json!({ "status_code": 5, "code": 6, "stdout": "ok" }), None),
            (json!({ "exit_code": "7" }), None),
        ] {
            assert_eq!(
                self::reading(&object.to_string(), false).field_exit,
                exit,
                "{object}"
            );
        }
        // Text that only begins like an object, and a JSON array, are read
        // as text.
        for text in ["{ not json\nexit code 1", "[\"exit code 1\"]\nexit code 1"] {
            let reading = self::reading(text, false);
            assert_eq!((reading.values.is_none(), reading.field_exit), (true, None));
        }
        // A `json` value and a denied execution are no text, and a text
        // past the bound is left out.
        for output in [
            OutputKind::Json {
                value: json!({ "exit_code": 1 }),
            },
            OutputKind::ErrorJson {
                value: json!("boom"),
            },
        ] {
            assert!(Reading::of_output(&output).is_none());
        }
        let at_bound = "x".repeat(MAX_OUTPUT_BYTES);
        assert!(
            Reading::of_output(&OutputKind::Text {
                text: at_bound.clone()
            })
            .is_some()
        );
        let past = OutputKind::Text {
            text: format!("{at_bound}x"),
        };
        assert!(Reading::of_output(&past).is_none());
    }

    #[test]
    fn a_result_the_harness_ended_or_refused_is_skipped() {
        for text in [
            "Tool execution was blocked",
            "Tool bash not found",
            "Validation failed for tool \"bash\":\n  - command: must be string",
            "partial output\n\nCommand aborted",
            "partial output\nOperation aborted\n",
            "Tool execution aborted",
            "error: slow\n\nCommand timed out after 120 seconds",
        ] {
            assert!(reading(text, true).harness_decided(), "{text}");
            assert!(!reading(text, false).harness_decided(), "unmarked: {text}");
        }
        for text in [
            "Command aborted\nand then more output",
            "Tool execution was blocked by policy",
            "Tool bash and zsh not found",
            "error: Validation failed for tool \"bash\"",
            "Command exited with code 1",
        ] {
            assert!(!reading(text, true).harness_decided(), "{text}");
        }
    }

    #[test]
    fn a_failed_result_keeps_its_first_error_lines_and_its_closing_lines() {
        let text = "Compiling api\nerror[E0308]: mismatched types\n  --> src/render.rs:9:5\nwarning: unused\nerror: aborting\nTraceback (most recent call last):\nValueError: bad\nfatal: stop\nERROR extra one\nERROR extra two\nlast but one\n\nCommand exited with code 1";
        let (_, _, kept) = failed("make", text, true).unwrap();
        assert_eq!(
            kept,
            [
                "error[E0308]: mismatched types",
                "--> src/render.rs:9:5",
                "error: aborting",
                "Traceback (most recent call last):",
                "ValueError: bad",
                "last but one",
                "Command exited with code 1",
            ]
        );
        // A long line is cut to its bytes with the cut mark, and a line
        // with a control character is left out.
        let long = format!(
            "error: {}\nbell \u{7} line\nCommand exited with code 1",
            "é".repeat(200)
        );
        let (_, _, kept) = failed("make", &long, true).unwrap();
        assert_eq!(kept.len(), 2, "{kept:?}");
        assert!(kept[0].len() <= MAX_LINE_BYTES && kept[0].ends_with(CUT_MARK));
        assert_eq!(kept[1], "Command exited with code 1");
        // A line the output repeats is kept once.
        let repeated = "error: build failed\nexit status 1\nexit status 1";
        let (_, code, kept) = failed("make", repeated, true).unwrap();
        assert_eq!(
            (code, kept),
            (
                Some(1),
                vec!["error: build failed".to_owned(), "exit status 1".to_owned()]
            )
        );
    }

    #[test]
    fn a_line_names_a_path_with_a_place_and_an_error_by_its_type_or_code() {
        for (line, path) in [
            (
                "thread 'main' panicked at src/render.rs:9:5:",
                Some("src/render.rs:9:5"),
            ),
            (
                "  --> crates/api/src/lib.rs:120:17",
                Some("crates/api/src/lib.rs:120:17"),
            ),
            (
                "src/App.tsx(8,3): error TS1484: a type",
                Some("src/App.tsx(8,3)"),
            ),
            (
                "File \"/srv/app/jobs/sync.py\", line 16, in run",
                Some("/srv/app/jobs/sync.py"),
            ),
            (
                "FAILED tests/test_sync.py::TestSync::test_retry - boom",
                Some("tests/test_sync.py::TestSync::test_retry"),
            ),
            ("./main.go:12:3: undefined: handler", Some("./main.go:12:3")),
            ("(see build/report.html)", Some("build/report.html")),
            ("error: main.rs:4", Some("main.rs:4")),
            // A bare file name, a version, a URL, and a word with other
            // characters name no place.
            ("error in main.rs", None),
            ("upgrade to 1.41.9 or v2.0", None),
            (
                "fatal: unable to access 'https://example.com/team/repo.git/'",
                None,
            ),
            ("error: user@host.example:22 refused", None),
            ("cannot open src/mode=fast.txt", None),
            (
                "tests/test_sync.py:: (no test named)",
                Some("tests/test_sync.py"),
            ),
            ("test_sync.py::", None),
            ("see .env and ./.cache", None),
        ] {
            assert_eq!(named_path(line), path, "{line}");
        }
        for (line, error) in [
            (
                "ModuleNotFoundError: No module named 'yaml'",
                Some("ModuleNotFoundError"),
            ),
            (
                "Caused by: java.lang.NullPointerException: null",
                Some("java.lang.NullPointerException"),
            ),
            ("error[E0308]: mismatched types", Some("E0308")),
            ("src/App.tsx(8,3): error TS1484: a type", Some("TS1484")),
            ("main.c(4): error C2065: undeclared", Some("C2065")),
            ("error: linking failed", None),
            ("Error: 2 tests failed", None),
            ("ERROR code E1 in ABCDE12", None),
            ("3Error and _Exception", None),
        ] {
            assert_eq!(named_error(line), error, "{line}");
        }
    }

    /// What a failed text result keeps: the excerpt's state and the path
    /// and error name from its clear lines, as [`Pending::observe`] reads
    /// them.
    fn kept_of(text: &str) -> (Excerpt, Option<String>, Option<String>) {
        let reading = reading(text, true);
        let Outcome::Failed(failure) = outcome_of("make", &reading) else {
            panic!("a marked result is a failure");
        };
        let (excerpt, findings) = excerpt_of(&reading);
        let named = |name: fn(&str) -> Option<&str>| {
            failure
                .lines
                .iter()
                .filter(|line| {
                    findings.iter().all(|finding| {
                        finding.end <= line.span.start || line.span.end <= finding.start
                    })
                })
                .find_map(|line| name(&reading.text()[line.span.clone()]))
                .map(str::to_owned)
        };
        (excerpt, named(named_path), named(named_error))
    }

    #[test]
    fn an_output_the_redactor_finds_a_credential_in_keeps_no_line() {
        let (assignment, _) = keyed_credential();
        let clean =
            "error[E0308]: mismatched types\n  --> src/deploy.rs:14:9\nCommand exited with code 1";
        assert_eq!(
            kept_of(clean),
            (
                Excerpt::Kept,
                Some("src/deploy.rs:14:9".to_owned()),
                Some("E0308".to_owned())
            )
        );
        // A finding anywhere in the output withholds every line. The path
        // and the error name come from kept lines without a finding.
        let found = format!("{clean}\nnote: retry with {assignment}");
        assert_eq!(
            kept_of(&found),
            (
                Excerpt::Finding,
                Some("src/deploy.rs:14:9".to_owned()),
                Some("E0308".to_owned())
            )
        );
        let in_line =
            format!("ValueError: {assignment} at src/deploy.rs:14:9\nCommand exited with code 1");
        assert_eq!(kept_of(&in_line), (Excerpt::Finding, None, None));
        // The fields of a JSON object are scanned beside their names, and
        // its string values as text too; a finding in either keeps no
        // text of the object.
        let named_field = json!({ ["api_", "tok", "en"].concat(): "v4lu3-of-the-fixture-5524", "output": "ValueError: bad input" }).to_string();
        assert_eq!(kept_of(&named_field), (Excerpt::Finding, None, None));
        let in_value = json!({ "output": format!("ValueError: bad\n{assignment}") }).to_string();
        assert_eq!(kept_of(&in_value), (Excerpt::Finding, None, None));
        // A scan that does not finish keeps no text at all: a key block
        // longer than the redactor matches.
        let block = format!(
            "ValueError: bad input at src/deploy.rs:14:9\n-----BEGIN {kind}-----\n{body}-----END {kind}-----\nCommand exited with code 1",
            kind = ["RSA PRIVATE", " KEY"].concat(),
            body = "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVphYmNkZWZnaGlqa2xtbm9wcXJzdHV2\n".repeat(600),
        );
        assert!(block.len() <= MAX_OUTPUT_BYTES);
        assert_eq!(kept_of(&block), (Excerpt::Unscanned, None, None));
    }

    fn observed(excerpt: Excerpt, failure: &Failure) -> Observed {
        Observed {
            call: "call-1".into(),
            tool: "bash".into(),
            command: named("cargo test -p api"),
            at_ms: 1_000,
            outcome: Outcome::Failed(failure.clone()),
            excerpt,
            path: Some("src/render.rs:9:5".into()),
            error: Some("E0308".into()),
        }
    }

    #[test]
    fn the_record_states_the_outcome_the_command_and_the_lines_within_its_bytes() {
        let kept = |text: &str, closing: bool| Kept {
            text: text.into(),
            closing,
            span: 0..0,
        };
        let failure = |evidence, exit_code, lines| Failure {
            exit_code,
            evidence,
            lines,
        };
        let lines = vec![kept("error: first", false), kept("the end", true)];
        let head = "Tool failure recorded from the result of bash at 1970-01-01T00:00:01.000Z: ";
        let tail = ".\nCommand: cargo test [arguments left out]\nFailing path: src/render.rs:9:5\nError: E0308\nOutput lines:\nerror: first\nthe end";
        for (evidence, code, stated) in [
            (
                Evidence::ToolStatus,
                Some(101),
                "the command failed with exit code 101",
            ),
            (
                Evidence::ToolStatus,
                None,
                "the command failed and the tool reported an error",
            ),
            (
                Evidence::PrintedStatus,
                Some(2),
                "the command failed: its command line printed exit status 2",
            ),
            (
                Evidence::ShellReport,
                None,
                "the command failed: the shell found no program of a name in its command line",
            ),
            (
                Evidence::FailureLines,
                None,
                "the output holds failure lines and the command line hides its exit status from the tool",
            ),
        ] {
            let failure = failure(evidence, code, lines.clone());
            assert_eq!(
                summary_of(&observed(Excerpt::Kept, &failure), "s1", &failure),
                format!("{head}{stated}{tail}")
            );
        }
        // A withheld output states why and where the output stands, with
        // the ids within their bytes.
        let failure = failure(Evidence::ToolStatus, Some(1), lines);
        let mut unit = observed(Excerpt::Unscanned, &failure);
        unit.call = "c".repeat(MAX_ID_BYTES + 40);
        unit.command = named("\"my tool\"");
        let withheld = summary_of(&unit, &"s".repeat(MAX_ID_BYTES + 40), &failure);
        assert!(
            withheld.contains("\nCommand: [left out]\n")
                && withheld.contains(&format!(
                    "\nOutput text withheld: the redactor's scan of the output did not finish. The whole output stands in the result of tool call {}{CUT_MARK} of session {}{CUT_MARK} while",
                    "c".repeat(MAX_ID_BYTES - CUT_MARK.len()),
                    "s".repeat(MAX_ID_BYTES - CUT_MARK.len()),
                )),
            "{withheld}"
        );
        assert!(!withheld.contains("error: first"));
        // The closing lines stay whole when the kept lines pass the
        // record's bytes, and the record leaves room for the resolved note.
        let long = "e".repeat(MAX_LINE_BYTES);
        let mut lines: Vec<Kept> = (0..MAX_ERROR_LINES).map(|_| kept(&long, false)).collect();
        lines.extend([kept(&long, true), kept("Command exited with code 1", true)]);
        let failure = Failure {
            exit_code: Some(1),
            evidence: Evidence::ToolStatus,
            lines,
        };
        let mut unit = observed(Excerpt::Kept, &failure);
        unit.tool = "t".repeat(MAX_TOOL_BYTES + 10);
        let summary = summary_of(&unit, "s1", &failure);
        assert!(
            summary.len() + RESOLVED_NOTE.len() <= MAX_SUMMARY_BYTES,
            "{}",
            summary.len()
        );
        assert!(summary.ends_with(&format!("\n{long}\nCommand exited with code 1")));
        assert!(summary.matches(&long).count() < MAX_ERROR_LINES + 1);
    }

    /// A record is written when the redactor leaves its text and its
    /// rationale as composed. The names a record carries beside secret
    /// words (a program, a path, an error type) and the ids a provider
    /// gives a tool call and a session are ordinary text to the redactor,
    /// for a failed record and for its resolved copy.
    #[test]
    fn a_record_of_ordinary_names_and_ids_passes_the_redactor_as_composed() {
        let failure = Failure {
            exit_code: Some(1),
            evidence: Evidence::ToolStatus,
            lines: vec![Kept {
                text: "Command exited with code 1".into(),
                closing: true,
                span: 0..0,
            }],
        };
        let session = "0f8fad5b-d9cb-469f-a165-70867728950e";
        for (command, path, error) in [
            (
                "gh auth login --web",
                "src/auth/token.rs:12:5",
                "AuthenticationError",
            ),
            (
                "ssh-keygen -t ed25519",
                "lib/keys.py:40",
                "InvalidTokenError",
            ),
            ("git credential fill", "config/secrets.yaml:3", "E0308"),
            (
                "npm token list",
                "packages/api/password_reset.ts(8,3)",
                "TS2307",
            ),
            (
                "kubectl secret",
                "tests/test_bearer.py::test_refresh",
                "KeyError",
            ),
            ("keytool", "auth.rs:9", "java.security.KeyException"),
        ] {
            for call in [
                "toolu_01A9bC3dE5fG7hJ9kL1mN3pQ",
                "call_Zx81kLmQp09RtYvWs77AbCdE|fc_68a1b2c3d4e5f6",
            ] {
                for excerpt in [Excerpt::Kept, Excerpt::Finding] {
                    let mut unit = observed(excerpt, &failure);
                    unit.call = call.into();
                    unit.command = named(command);
                    unit.path = Some(path.into());
                    unit.error = Some(error.into());
                    let failed = failed_record(session, &unit, &failure);
                    let stored = Stored::read(&failed.rationale).unwrap();
                    let resolved =
                        resolved_record(session, &unit, failed.clone(), stored.rationale);
                    for text in [
                        failed.summary,
                        failed.rationale,
                        resolved.summary,
                        resolved.rationale,
                    ] {
                        assert_eq!(kernel::exact_text_refusal(&text), None, "{text}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_record_id_frames_its_parts_and_a_lineage_is_named_by_its_prefix() {
        assert_eq!(record_id("p", "s", "c").len(), 36);
        assert_eq!(record_id("p", "s", "c"), record_id("p", "s", "c"));
        assert_ne!(record_id("ps", "", "c"), record_id("p", "s", "c"));
        assert_ne!(record_id("p", "sc", ""), record_id("p", "s", "c"));
        for lineage in ["tool_failure", "tool_failure:", "tool_failure:0f3a"] {
            assert!(is_failure_lineage(lineage), "{lineage}");
        }
        for other in [
            "tool_failures",
            "tool_failure_2",
            "a:tool_failure",
            "eidnara_memory",
            "",
        ] {
            assert!(!is_failure_lineage(other), "{other}");
        }
        let (from, to) = session_keys("s1");
        assert!(from < to && from.ends_with(':') && to.ends_with(';'));
        assert_ne!(session_keys("s2").0, from);
    }

    #[test]
    fn the_memo_holds_its_bound_of_calls_sessions_and_checkpoints() {
        let mut memo = FailureMemo::default();
        let epoch = memo.pin("s1").unwrap();
        assert!(memo.busy("s1"), "one checkpoint of a session at a time");
        assert!(!memo.busy("s2"));
        for call in 0..MEMO_CALLS_PER_SESSION as u64 + 1 {
            memo.learn("s1", epoch, call);
        }
        assert_eq!(memo.calls_for_test("s1"), MEMO_CALLS_PER_SESSION);
        assert!(!memo.has_seen("s1", 0), "the call learned first gives way");
        assert!(memo.has_seen("s1", 1));
        // A session's delete ends the entry: its checkpoint learns nothing
        // more, and its release leaves a later entry of the session held.
        memo.forget_session("s1");
        assert!(!memo.current("s1", epoch));
        let later = memo.pin("s1").unwrap();
        memo.learn("s1", epoch, 7);
        assert_eq!(memo.calls_for_test("s1"), 0);
        memo.release("s1", epoch);
        assert!(memo.busy("s1") && memo.current("s1", later));
        memo.release("s1", later);
        assert_eq!(memo.held_for_test(), (0, 0));
        // Checkpoints of other sessions run up to their bound.
        let held: Vec<(String, u64)> = (0..MAX_CHECKPOINTS)
            .map(|index| {
                let session = format!("held-{index}");
                let epoch = memo.pin(&session).unwrap();
                (session, epoch)
            })
            .collect();
        assert!(memo.busy("s1"), "the bound of checkpoints is in flight");
        let (session, epoch) = &held[0];
        memo.release(session, *epoch);
        assert!(!memo.busy("s1"));
        // A full memo gives up its oldest entry that no checkpoint holds.
        for index in 0..MEMO_SESSIONS {
            let session = format!("filler-{index}");
            if let Some(epoch) = memo.pin(&session) {
                memo.release(&session, epoch);
            }
        }
        assert_eq!(memo.sessions.len(), MEMO_SESSIONS);
        assert!(memo.sessions.contains_key("held-1"), "a held entry stays");
        assert!(!memo.sessions.contains_key("s1"));
    }
}
